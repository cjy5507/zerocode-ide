use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use judgment_provider::Provider;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::catalog::{self, Artifact, Catalog, EngineKind, Model, RuntimeProfile};
use crate::engine::{self, Engine, Running};
use crate::selection::{self, Binding};
use crate::storage::{self, Store};
use crate::{Error, Result};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub instance_id: String,
    pub revision: u64,
    pub catalog_version: u32,
    pub models: Vec<ModelInfo>,
    pub runtime: RuntimeInfo,
    pub operation: Option<Operation>,
    pub server: ServerInfo,
    pub provider_identity: Option<String>,
    pub configured_model_id: Option<String>,
    pub applied_model_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub engine: EngineKind,
    pub protocol: judgment_provider::Protocol,
    pub revision: String,
    pub license: String,
    pub source: String,
    pub quality: String,
    pub download_bytes: u64,
    pub installed: bool,
    pub compatibility: &'static str,
    pub compatibility_reason: Option<&'static str>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub state: &'static str,
    pub python_requirement: String,
    pub download_bytes: u64,
    pub reason: Option<&'static str>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub id: String,
    pub kind: &'static str,
    pub model_id: Option<String>,
    pub state: &'static str,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<&'static str>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub id: Option<String>,
    pub state: &'static str,
    pub model_id: Option<String>,
    pub provider: Option<Provider>,
    pub checked: bool,
    pub error: Option<&'static str>,
}

struct Cancel {
    flag: AtomicBool,
    signal: tokio::sync::Notify,
}

impl Cancel {
    fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            signal: tokio::sync::Notify::new(),
        }
    }

    fn stop(&self) {
        self.flag.store(true, Ordering::Release);
        self.signal.notify_one();
    }

    fn check(&self) -> Result<()> {
        if self.flag.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    async fn cancelled(&self) {
        if !self.flag.load(Ordering::Acquire) {
            self.signal.notified().await;
        }
    }
}

struct Inner {
    revision: u64,
    operation: Option<Operation>,
    cancel: Option<Arc<Cancel>>,
    server: ServerInfo,
    running: Option<Running>,
    pending_engine: Option<Arc<Engine>>,
    last_event: Instant,
    provider_identity: Option<String>,
}

pub struct Manager {
    store: Store,
    catalog: Catalog,
    profile: Option<RuntimeProfile>,
    settings: PathBuf,
    instance: String,
    inner: Mutex<Inner>,
    sink: Arc<dyn Fn(Snapshot) + Send + Sync>,
    closed: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Manager {
    pub fn new(
        root: &Path,
        settings: &Path,
        sink: impl Fn(Snapshot) + Send + Sync + 'static,
    ) -> Result<Arc<Self>> {
        Self::with_catalog(
            root,
            settings,
            catalog::catalog()?,
            catalog::runtime()?,
            Arc::new(sink),
        )
    }

    fn with_catalog(
        root: &Path,
        settings: &Path,
        catalog: Catalog,
        profile: Option<RuntimeProfile>,
        sink: Arc<dyn Fn(Snapshot) + Send + Sync>,
    ) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            store: Store::open(root)?,
            catalog,
            profile,
            settings: settings.to_owned(),
            instance: uuid::Uuid::new_v4().to_string(),
            inner: Mutex::new(Inner {
                revision: 1,
                operation: None,
                cancel: None,
                server: ServerInfo {
                    state: "stopped",
                    ..ServerInfo::default()
                },
                running: None,
                pending_engine: None,
                last_event: Instant::now(),
                provider_identity: None,
            }),
            sink,
            closed: AtomicBool::new(false),
        }))
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut inner = lock(&self.inner);
        let selected = selection::read(&self.settings);
        let provider_identity = selected
            .as_ref()
            .ok()
            .map(|selected| selected.provider.identity())
            .or_else(|| {
                Provider::read(&self.settings)
                    .ok()
                    .map(|provider| provider.identity())
            });
        if inner.provider_identity != provider_identity {
            inner.provider_identity.clone_from(&provider_identity);
            inner.revision += 1;
        }
        if inner
            .running
            .as_ref()
            .is_some_and(|running| !running.engine.alive())
        {
            inner.running.take();
            inner.server.state = "failed";
            inner.server.checked = false;
            inner.server.provider = None;
            inner.server.error = Some(Error::ServerStopped.token());
            inner.revision += 1;
        }
        let configured = selected
            .as_ref()
            .ok()
            .and_then(|selected| selected.binding.as_ref())
            .filter(|binding| {
                self.catalog
                    .models
                    .iter()
                    .any(|model| model.id == binding.model_id && model.revision == binding.revision)
            })
            .map(|binding| binding.model_id.clone());
        let applied = inner
            .running
            .as_ref()
            .filter(|running| {
                Some(running.provider.identity()) == provider_identity && inner.server.checked
            })
            .map(|running| running.model_id.clone());
        Snapshot {
            instance_id: self.instance.clone(),
            revision: inner.revision,
            catalog_version: self.catalog.version,
            models: self
                .catalog
                .models
                .iter()
                .map(|model| ModelInfo {
                    id: model.id.clone(),
                    label: model.label.clone(),
                    engine: model.engine.clone(),
                    protocol: model.protocol,
                    revision: model.revision.clone(),
                    license: model.license.clone(),
                    source: model.source.clone(),
                    quality: model.quality.clone(),
                    download_bytes: model.download_bytes(),
                    installed: self.store.installed(model),
                    compatibility: if self.profile.is_some() {
                        "supported"
                    } else {
                        "unsupported"
                    },
                    compatibility_reason: self
                        .profile
                        .is_none()
                        .then_some(Error::Unsupported.token()),
                })
                .collect(),
            runtime: match &self.profile {
                Some(profile) => RuntimeInfo {
                    state: if self.store.runtime_ready(profile) {
                        "ready"
                    } else {
                        "missing"
                    },
                    python_requirement: profile
                        .python
                        .version
                        .clone()
                        .unwrap_or_else(|| profile.python_version.clone()),
                    download_bytes: profile.download_bytes(),
                    reason: None,
                },
                None => RuntimeInfo {
                    state: "unsupported",
                    python_requirement: String::new(),
                    download_bytes: 0,
                    reason: Some(Error::Unsupported.token()),
                },
            },
            operation: inner.operation.clone(),
            server: inner.server.clone(),
            provider_identity,
            configured_model_id: configured,
            applied_model_id: applied,
        }
    }

    fn emit(&self) {
        (self.sink)(self.snapshot());
    }

    pub fn notify_settings_changed(&self) {
        lock(&self.inner).revision += 1;
        self.emit();
    }

    fn model(&self, id: &str, revision: &str) -> Result<Model> {
        let model = self
            .catalog
            .models
            .iter()
            .find(|model| model.id == id)
            .ok_or(Error::UnknownModel)?;
        if model.revision != revision {
            return Err(Error::RevisionChanged);
        }
        Ok(model.clone())
    }

    fn begin(
        &self,
        kind: &'static str,
        model: Option<&Model>,
        total: u64,
    ) -> Result<(String, Arc<Cancel>)> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::ServerStopped);
        }
        let mut inner = lock(&self.inner);
        if inner.cancel.is_some() {
            return Err(Error::Busy);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(Cancel::new());
        inner.operation = Some(Operation {
            id: id.clone(),
            kind,
            model_id: model.map(|model| model.id.clone()),
            state: "running",
            received_bytes: 0,
            total_bytes: total,
            error: None,
        });
        inner.cancel = Some(cancel.clone());
        inner.revision += 1;
        drop(inner);
        self.emit();
        Ok((id, cancel))
    }

    fn finish(&self, id: &str, result: Result<()>) {
        let mut inner = lock(&self.inner);
        if inner
            .operation
            .as_ref()
            .is_none_or(|operation| operation.id != id)
        {
            return;
        }
        let cancelled = inner
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.flag.load(Ordering::Acquire));
        if let Some(operation) = &mut inner.operation {
            let result = if cancelled {
                Err(Error::Cancelled)
            } else {
                result
            };
            operation.state = match result {
                Ok(()) => "done",
                Err(Error::Cancelled) => "cancelled",
                Err(_) => "failed",
            };
            operation.error = result.err().map(Error::token);
            if let Err(error) = result
                && operation.kind == "start"
            {
                inner.server.state = if error == Error::Cancelled {
                    "stopped"
                } else {
                    "failed"
                };
                inner.server.error = Some(error.token());
                inner.server.provider = None;
                inner.server.checked = false;
            }
        }
        inner.pending_engine.take();
        inner.cancel = None;
        inner.revision += 1;
        drop(inner);
        self.emit();
    }

    fn progress(&self, id: &str, added: u64) {
        let mut inner = lock(&self.inner);
        let Some(operation) = &mut inner.operation else {
            return;
        };
        if operation.id != id {
            return;
        }
        operation.received_bytes = operation
            .received_bytes
            .saturating_add(added)
            .min(operation.total_bytes);
        let emit = inner.last_event.elapsed() >= Duration::from_millis(250);
        inner.revision += 1;
        if emit {
            inner.last_event = Instant::now();
        }
        drop(inner);
        if emit {
            self.emit();
        }
    }

    pub fn cancel(&self, operation_id: &str) -> Result<Snapshot> {
        let mut inner = lock(&self.inner);
        if inner
            .operation
            .as_ref()
            .is_none_or(|operation| operation.id != operation_id)
        {
            return Err(Error::RevisionChanged);
        }
        if let Some(cancel) = &inner.cancel {
            cancel.stop();
            if let Some(engine) = &inner.pending_engine {
                engine.stop();
            }
            if let Some(operation) = &mut inner.operation {
                operation.state = "cancelling";
            }
            inner.revision += 1;
        }
        drop(inner);
        self.emit();
        Ok(self.snapshot())
    }

    pub fn prepare_runtime(self: &Arc<Self>) -> Result<Snapshot> {
        let profile = self.profile.clone().ok_or(Error::Unsupported)?;
        if self.store.runtime_ready(&profile) {
            return Ok(self.snapshot());
        }
        if lock(&self.inner).running.is_some() {
            return Err(Error::Busy);
        }
        self.store.require_space(5 * 1024 * 1024 * 1024)?;
        let (id, cancel) = self.begin("runtime", None, profile.download_bytes())?;
        let manager = self.clone();
        tokio::spawn(async move {
            let result = manager.prepare(&profile, &id, &cancel).await;
            manager.finish(&id, result);
        });
        Ok(self.snapshot())
    }

    async fn prepare(
        self: &Arc<Self>,
        profile: &RuntimeProfile,
        id: &str,
        cancel: &Arc<Cancel>,
    ) -> Result<()> {
        let mut stage = self.store.stage()?;
        let downloads = storage::child(stage.path(), "downloads")?;
        storage::private_dir(&downloads)?;
        for artifact in std::iter::once(&profile.python).chain(&profile.wheels) {
            self.download(artifact, &downloads, id, cancel).await?;
        }
        let unpack_root = stage.path().to_owned();
        let unpack_profile = profile.clone();
        let unpack_cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let mut budget = crate::archive::Budget::new();
            crate::archive::python(
                &downloads.join(&unpack_profile.python.path),
                &unpack_root,
                &mut budget,
                &unpack_cancel.flag,
            )?;
            for wheel in &unpack_profile.wheels {
                crate::archive::wheel(
                    &downloads.join(&wheel.path),
                    &unpack_root,
                    &unpack_profile.site_packages,
                    &mut budget,
                    &unpack_cancel.flag,
                )?;
            }
            fs::remove_dir_all(downloads).map_err(|_| Error::Storage)
        })
        .await
        .map_err(|_| Error::RuntimeFailed)??;
        cancel.check()?;
        let engine = Engine::spawn(stage.path(), profile, None)?;
        self.pending(engine.clone(), cancel)?;
        let result = wait_ready(&engine, cancel).await;
        engine.stop();
        result?;
        cancel.check()?;
        self.commit(cancel, || self.store.commit_runtime(&mut stage, profile))
    }

    pub fn install(
        self: &Arc<Self>,
        model_id: &str,
        revision: &str,
        source: Option<PathBuf>,
    ) -> Result<Snapshot> {
        let model = self.model(model_id, revision)?;
        if self.profile.is_none() {
            return Err(Error::Unsupported);
        }
        if self.store.installed(&model) {
            return Ok(self.snapshot());
        }
        if lock(&self.inner)
            .running
            .as_ref()
            .is_some_and(|running| running.model_id == model.id)
        {
            return Err(Error::Busy);
        }
        self.store.require_space(model.download_bytes())?;
        let kind = if source.is_some() {
            "import"
        } else {
            "download"
        };
        let (id, cancel) = self.begin(kind, Some(&model), model.download_bytes())?;
        let manager = self.clone();
        tokio::spawn(async move {
            let result = manager.install_model(&model, source, &id, &cancel).await;
            manager.finish(&id, result);
        });
        Ok(self.snapshot())
    }

    async fn install_model(
        self: &Arc<Self>,
        model: &Model,
        source: Option<PathBuf>,
        id: &str,
        cancel: &Arc<Cancel>,
    ) -> Result<()> {
        let mut stage = self.store.stage()?;
        if let Some(source) = source {
            let manager = self.clone();
            let model = model.clone();
            let path = stage.path().to_owned();
            let id = id.to_owned();
            let cancel = cancel.clone();
            tokio::task::spawn_blocking(move || {
                for artifact in &model.files {
                    copy_import(&source, &path, artifact, &cancel.flag, |bytes| {
                        manager.progress(&id, bytes)
                    })?;
                }
                Ok(())
            })
            .await
            .map_err(|_| Error::Storage)??;
        } else {
            for artifact in &model.files {
                self.download(artifact, stage.path(), id, cancel).await?;
            }
        }
        cancel.check()?;
        self.commit(cancel, || self.store.commit_model(&mut stage, model))
    }

    fn complete(inner: &mut Inner) {
        inner.cancel = None;
        inner.pending_engine.take();
        if let Some(operation) = &mut inner.operation {
            operation.state = "done";
            operation.error = None;
        }
        inner.revision += 1;
    }

    fn commit(&self, cancel: &Cancel, commit: impl FnOnce() -> Result<()>) -> Result<()> {
        let mut inner = lock(&self.inner);
        cancel.check()?;
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        commit()?;
        Self::complete(&mut inner);
        Ok(())
    }

    async fn download(
        &self,
        artifact: &Artifact,
        destination: &Path,
        id: &str,
        cancel: &Cancel,
    ) -> Result<()> {
        cancel.check()?;
        if !catalog::https(&artifact.url) {
            return Err(Error::Manifest);
        }
        let client = download_client()?;
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            response = client.get(&artifact.url).send() => response.map_err(|_| Error::Transport)?,
        };
        self.receive(response, artifact, destination, id, cancel)
            .await
    }

    async fn receive(
        &self,
        mut response: reqwest::Response,
        artifact: &Artifact,
        destination: &Path,
        id: &str,
        cancel: &Cancel,
    ) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        cancel.check()?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Transport);
        }
        if response
            .content_length()
            .is_some_and(|bytes| bytes != artifact.bytes)
        {
            return Err(Error::Integrity);
        }
        let path = storage::child(destination, &artifact.path)?;
        storage::private_dir(path.parent().ok_or(Error::UnsafePath)?)?;
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .await
            .map_err(|_| Error::Storage)?;
        let mut digest = Sha256::new();
        let mut received = 0u64;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                chunk = response.chunk() => chunk.map_err(|_| Error::Transport)?,
            };
            let Some(chunk) = chunk else { break };
            received = received
                .checked_add(chunk.len() as u64)
                .ok_or(Error::TooLarge)?;
            if received > artifact.bytes {
                return Err(Error::Integrity);
            }
            digest.update(&chunk);
            file.write_all(&chunk).await.map_err(|_| Error::Storage)?;
            self.progress(id, chunk.len() as u64);
        }
        if received != artifact.bytes || format!("{:x}", digest.finalize()) != artifact.sha256 {
            return Err(Error::Integrity);
        }
        file.sync_all().await.map_err(|_| Error::Storage)?;
        Ok(())
    }

    fn pending(&self, engine: Arc<Engine>, cancel: &Cancel) -> Result<()> {
        cancel.check()?;
        let mut inner = lock(&self.inner);
        if self.closed.load(Ordering::Acquire) {
            engine.stop();
            return Err(Error::Cancelled);
        }
        inner.pending_engine = Some(engine);
        Ok(())
    }

    pub fn start(self: &Arc<Self>, model_id: &str, revision: &str) -> Result<Snapshot> {
        let model = self.model(model_id, revision)?;
        let profile = self.profile.clone().ok_or(Error::Unsupported)?;
        if !self.store.runtime_ready(&profile) {
            return Err(Error::RuntimeMissing);
        }
        if !self.store.installed(&model) {
            return Err(Error::ModelMissing);
        }
        let existing = lock(&self.inner)
            .running
            .as_ref()
            .map(|running| running.model_id == model.id && running.engine.alive());
        if let Some(same) = existing {
            return if same {
                Ok(self.snapshot())
            } else {
                Err(Error::Busy)
            };
        }
        let (id, cancel) = self.begin("start", Some(&model), 0)?;
        {
            let mut inner = lock(&self.inner);
            inner.server = ServerInfo {
                state: "starting",
                model_id: Some(model.id.clone()),
                ..ServerInfo::default()
            };
            inner.revision += 1;
        }
        self.emit();
        let manager = self.clone();
        tokio::spawn(async move {
            let result = manager.start_model(&model, &profile, &cancel).await;
            manager.finish(&id, result);
        });
        Ok(self.snapshot())
    }

    async fn start_model(
        self: &Arc<Self>,
        model: &Model,
        profile: &RuntimeProfile,
        cancel: &Arc<Cancel>,
    ) -> Result<()> {
        let path = self.store.model_path(model)?;
        let verify_model = model.clone();
        let verify_path = path.clone();
        let verify_cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            for artifact in &verify_model.files {
                storage::verify(
                    &storage::child(&verify_path, &artifact.path)?,
                    artifact,
                    &verify_cancel.flag,
                )?;
            }
            Ok(())
        })
        .await
        .map_err(|_| Error::Storage)??;
        cancel.check()?;
        let runtime = self.store.runtime_path(profile)?;
        let engine = Engine::spawn(&runtime, profile, Some((&path, model)))?;
        self.pending(engine.clone(), cancel)?;
        wait_ready(&engine, cancel).await?;
        let manager = Arc::downgrade(self);
        let running = engine::serve(
            model,
            engine,
            Arc::new(move || {
                if let Some(manager) = manager.upgrade() {
                    manager.emit();
                }
            }),
        )
        .await?;
        let mut inner = lock(&self.inner);
        cancel.check()?;
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if !running.engine.alive() {
            return Err(Error::RuntimeFailed);
        }
        inner.server = ServerInfo {
            id: Some(running.id.clone()),
            state: "ready",
            model_id: Some(model.id.clone()),
            provider: Some(running.provider.clone()),
            checked: false,
            error: None,
        };
        inner.running = Some(running);
        Self::complete(&mut inner);
        Ok(())
    }

    pub fn check(self: &Arc<Self>, server_id: &str) -> Result<Snapshot> {
        let provider = {
            let inner = lock(&self.inner);
            let running = inner.running.as_ref().ok_or(Error::ServerStopped)?;
            if running.id != server_id {
                return Err(Error::ServerChanged);
            }
            running.provider.clone()
        };
        let (id, cancel) = self.begin("check", None, 0)?;
        let manager = self.clone();
        let server_id = server_id.to_owned();
        tokio::spawn(async move {
            let mut result = tokio::select! {
                _ = cancel.cancelled() => Err(Error::Cancelled),
                result = native_check(&provider) => result,
            };
            {
                let mut inner = lock(&manager.inner);
                if cancel.flag.load(Ordering::Acquire) {
                    result = Err(Error::Cancelled);
                }
                if inner
                    .running
                    .as_ref()
                    .is_some_and(|running| running.id == server_id)
                {
                    inner.server.checked = result.is_ok();
                    inner.server.error = result.err().map(Error::token);
                    if result == Err(Error::Cancelled) {
                        inner.running.take();
                        inner.server.state = "stopped";
                        inner.server.provider = None;
                    }
                    inner.revision += 1;
                }
                if result.is_ok() {
                    Self::complete(&mut inner);
                }
            }
            manager.finish(&id, result);
        });
        Ok(self.snapshot())
    }

    pub fn apply(
        &self,
        server_id: &str,
        expected: &str,
        apply: impl FnOnce(&Provider, &str, &Binding) -> Result<()>,
    ) -> Result<Snapshot> {
        let mut inner = lock(&self.inner);
        if inner.cancel.is_some() {
            return Err(Error::Busy);
        }
        let running = inner.running.as_ref().ok_or(Error::ServerStopped)?;
        if running.id != server_id {
            return Err(Error::ServerChanged);
        }
        if !running.engine.alive() {
            return Err(Error::ServerStopped);
        }
        if !inner.server.checked {
            return Err(Error::NotChecked);
        }
        let provider = running.provider.clone();
        let model = self
            .catalog
            .models
            .iter()
            .find(|model| model.id == running.model_id)
            .ok_or(Error::UnknownModel)?;
        let current = Provider::read(&self.settings).map_err(|_| Error::SettingsChanged)?;
        if current.identity() != expected {
            return Err(Error::SettingsChanged);
        }
        let binding = Binding {
            model_id: model.id.clone(),
            revision: model.revision.clone(),
            provider_identity: provider.identity(),
        };
        apply(&provider, expected, &binding)?;
        inner.revision += 1;
        drop(inner);
        self.emit();
        Ok(self.snapshot())
    }

    pub fn stop(&self, server_id: &str) -> Result<Snapshot> {
        let mut inner = lock(&self.inner);
        if inner.cancel.is_some() {
            return Err(Error::Busy);
        }
        let running = inner.running.as_ref().ok_or(Error::ServerStopped)?;
        if running.id != server_id {
            return Err(Error::ServerChanged);
        }
        inner.running.take();
        inner.server = ServerInfo {
            state: "stopped",
            ..ServerInfo::default()
        };
        inner.revision += 1;
        drop(inner);
        self.emit();
        Ok(self.snapshot())
    }

    pub fn remove(&self, model_id: &str, revision: &str) -> Result<Snapshot> {
        self.remove_guarded(model_id, revision, |remove| remove())
    }

    pub fn remove_guarded(
        &self,
        model_id: &str,
        revision: &str,
        guard: impl FnOnce(&dyn Fn() -> Result<()>) -> Result<()>,
    ) -> Result<Snapshot> {
        let model = self.model(model_id, revision)?;
        let mut inner = lock(&self.inner);
        if inner.cancel.is_some()
            || inner
                .running
                .as_ref()
                .is_some_and(|running| running.model_id == model_id)
        {
            return Err(Error::Busy);
        }
        guard(&|| {
            let selected = selection::read(&self.settings)?;
            if selected
                .binding
                .is_some_and(|binding| binding.model_id == model_id)
            {
                return Err(Error::Configured);
            }
            self.store.remove(&model)
        })?;
        inner.revision += 1;
        drop(inner);
        self.emit();
        Ok(self.snapshot())
    }

    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        let mut inner = lock(&self.inner);
        if let Some(cancel) = &inner.cancel {
            cancel.stop();
        }
        if let Some(engine) = inner.pending_engine.take() {
            engine.stop();
        }
        inner.running.take();
        inner.server = ServerInfo {
            state: "stopped",
            ..ServerInfo::default()
        };
        inner.revision += 1;
    }
}

async fn wait_ready(engine: &Arc<Engine>, cancel: &Cancel) -> Result<()> {
    wait_ready_for(engine, cancel, engine::START_TIMEOUT).await
}

async fn wait_ready_for(engine: &Arc<Engine>, cancel: &Cancel, timeout: Duration) -> Result<()> {
    let process = engine.clone();
    let ready = tokio::task::spawn_blocking(move || process.ready());
    let result = tokio::select! {
        _ = cancel.cancelled() => Err(Error::Cancelled),
        ready = tokio::time::timeout(timeout, ready) => match ready {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Error::RuntimeFailed),
            Err(_) => Err(Error::Timeout),
        },
    };
    if result.is_err() {
        engine.stop();
    }
    result
}

fn download_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(900))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 8 || !catalog::https(attempt.url().as_str()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| Error::Transport)
}

pub(crate) fn copy_import(
    source: &Path,
    destination: &Path,
    artifact: &Artifact,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    let path = source.join(catalog::relative(&artifact.path)?);
    let source_info = fs::metadata(&path).map_err(|_| Error::Storage)?;
    if !source_info.is_file() {
        return Err(Error::Integrity);
    }
    let mut source = File::open(path).map_err(|_| Error::Storage)?;
    let metadata = source.metadata().map_err(|_| Error::Storage)?;
    if !metadata.is_file() || metadata.len() != artifact.bytes {
        return Err(Error::Integrity);
    }
    let target = storage::child(destination, &artifact.path)?;
    storage::private_dir(target.parent().ok_or(Error::UnsafePath)?)?;
    let mut target = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)
        .map_err(|_| Error::Storage)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut copied = 0u64;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let read = source.read(&mut buffer).map_err(|_| Error::Storage)?;
        if read == 0 {
            break;
        }
        copied += read as u64;
        if copied > artifact.bytes {
            return Err(Error::Integrity);
        }
        digest.update(&buffer[..read]);
        target
            .write_all(&buffer[..read])
            .map_err(|_| Error::Storage)?;
        progress(read as u64);
    }
    if copied != artifact.bytes || format!("{:x}", digest.finalize()) != artifact.sha256 {
        return Err(Error::Integrity);
    }
    target.sync_all().map_err(|_| Error::Storage)
}

pub async fn native_check(provider: &Provider) -> Result<()> {
    let prepared = provider.prepare(serde_json::to_vec(&json!({
        "model":provider.identity(), "state":{"value":7},
        "questions":{
            "choice":{"type":"choice","instructions":"Choose the description matching value 7.","criteria":{"seven":"value equals seven","other":"another value"}},
            "score":{"type":"score","instructions":"How well does the value equal seven?","criteria":["not seven","seven"]},
            "noul":{"type":"noul","instructions":"The value equals seven."}
        }
    })).map_err(|_| Error::Protocol)?).map_err(|_| Error::Protocol)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(35))
        .build()
        .map_err(|_| Error::Protocol)?;
    let mut response = client
        .post(provider.request_url())
        .header("content-type", "application/json")
        .body(prepared.body.clone())
        .send()
        .await
        .map_err(|_| Error::Protocol)?;
    if !response.status().is_success() {
        return Err(Error::Protocol);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Protocol)? {
        if chunk.len() > engine::MAX_FRAME.saturating_sub(bytes.len()) {
            return Err(Error::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    provider
        .decode(&prepared, bytes)
        .map_err(|_| Error::Protocol)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) use super::copy_import;
    use super::*;

    pub fn manager(root: &Path, settings: &Path, model: Model) -> Arc<Manager> {
        let runtimes: serde_json::Value =
            serde_json::from_str(include_str!("../manifests/runtime.json")).unwrap();
        let profile = serde_json::from_value(runtimes["profiles"][0].clone()).unwrap();
        Manager::with_catalog(
            root,
            settings,
            Catalog {
                version: 1,
                models: vec![model],
            },
            Some(profile),
            Arc::new(|_| {}),
        )
        .unwrap()
    }

    #[cfg(unix)]
    pub async fn running(
        manager: &Arc<Manager>,
        engine: Arc<Engine>,
        checked: bool,
    ) -> (String, Provider) {
        let model = &manager.catalog.models[0];
        let running = engine::serve(model, engine, Arc::new(|| {})).await.unwrap();
        let id = running.id.clone();
        let provider = running.provider.clone();
        let mut inner = lock(&manager.inner);
        inner.server = ServerInfo {
            id: Some(id.clone()),
            state: "ready",
            model_id: Some(model.id.clone()),
            provider: Some(provider.clone()),
            checked,
            error: None,
        };
        inner.running = Some(running);
        (id, provider)
    }

    pub fn stream(manager: &Arc<Manager>, artifact: Artifact, address: String) -> Snapshot {
        let (id, cancel) = manager.begin("download", None, artifact.bytes).unwrap();
        let snapshot = manager.snapshot();
        let manager = manager.clone();
        tokio::spawn(async move {
            let result = async {
                let stage = manager.store.stage()?;
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .map_err(|_| Error::Transport)?;
                let response = client
                    .get(address)
                    .send()
                    .await
                    .map_err(|_| Error::Transport)?;
                manager
                    .receive(response, &artifact, stage.path(), &id, &cancel)
                    .await
            }
            .await;
            manager.finish(&id, result);
        });
        snapshot
    }

    #[cfg(unix)]
    pub async fn ready_timeout(engine: &Arc<Engine>) -> Result<()> {
        wait_ready_for(engine, &Cancel::new(), Duration::from_millis(20)).await
    }

    #[cfg(unix)]
    pub fn fail_provider_write(
        manager: &Manager,
        server: &str,
        expected: &str,
        settings: &Path,
    ) -> Result<Snapshot> {
        manager.apply(server, expected, |_, _, _| {
            let backup = settings.with_extension("backup");
            fs::rename(settings, &backup).unwrap();
            fs::create_dir(settings).unwrap();
            let result = fs::write(settings, b"{}");
            fs::remove_dir(settings).unwrap();
            fs::rename(backup, settings).unwrap();
            result.map_err(|_| Error::Storage)
        })
    }
}
