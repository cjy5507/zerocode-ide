use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener as StdListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use judgment_provider::Provider;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::adapter;
use crate::catalog::{self, Artifact, Catalog, EngineKind, Model, RuntimeProfile};
use crate::engine::{self, Engine, Running};
use crate::selection::{self, Binding};
use crate::source::{self, Inspected, SourceInput};
use crate::storage::{self, Store};
use crate::{Error, Result};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub instance_id: String,
    pub revision: u64,
    pub catalog_version: u32,
    pub catalog_fingerprint: String,
    pub models: Vec<ModelInfo>,
    pub runtime: RuntimeInfo,
    pub operation: Option<Operation>,
    pub server: ServerInfo,
    pub provider_identity: Option<String>,
    pub configured_model_id: Option<String>,
    pub applied_model_id: Option<String>,
    pub source_inspection: Option<SourceInspection>,
    pub model_switch: Option<ModelSwitchInfo>,
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
    pub weights_bytes: u64,
    pub manifest_fingerprint: String,
    pub origin: &'static str,
    pub server_model_id: Option<String>,
    pub native_format: Option<String>,
    pub local: LocalSupport,
    pub existing_server: ExistingServerSupport,
    pub runtime: RuntimeInfo,
    pub installed: bool,
    pub installed_directory: Option<String>,
    pub compatibility: &'static str,
    pub compatibility_reason: Option<&'static str>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSupport {
    pub supported: bool,
    pub reason: Option<&'static str>,
    pub inputs: [&'static str; 2],
    pub unsupported_inputs: [&'static str; 2],
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingServerSupport {
    pub supported: bool,
    pub protocol: judgment_provider::Protocol,
    pub model_id: Option<String>,
    pub requires_check: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInspection {
    pub inspection_id: String,
    pub instance_id: String,
    pub catalog_fingerprint: String,
    pub model: ModelInfo,
    pub local: LocalSupport,
    pub existing_server: ExistingServerSupport,
    pub ready_for_install: bool,
    pub reason: Option<&'static str>,
    pub default_destination_directory: String,
    pub allows_destination_selection: bool,
    pub source_kind: &'static str,
    pub download_bytes: u64,
    pub copy_bytes: u64,
    pub runtime_download_bytes: u64,
}

struct PendingInspection {
    id: String,
    catalog_fingerprint: String,
    inspected: Inspected,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSwitchInfo {
    pub previous_model_id: String,
    pub target_model_id: String,
    pub state: &'static str,
    pub error: Option<&'static str>,
    pub restore_error: Option<&'static str>,
}

struct StartOptions {
    complete: bool,
    state: &'static str,
    check_before_listen: bool,
    require_listener: bool,
}

struct SwitchRequest {
    model: Model,
    profile: RuntimeProfile,
    server_id: String,
    expected: String,
}

struct PreviousServer {
    model: Model,
    profile: RuntimeProfile,
    provider: Provider,
}

#[cfg(test)]
pub(crate) type EngineFactory = Arc<dyn Fn(&Model) -> Result<Arc<Engine>> + Send + Sync>;

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

    async fn until_cancelled(
        &self,
        operation: impl std::future::Future<Output = Result<()>>,
    ) -> Result<()> {
        self.check()?;
        let result = tokio::select! {
            _ = self.cancelled() => Err(Error::Cancelled),
            result = operation => result,
        };
        self.check()?;
        result
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
    inspection: Option<PendingInspection>,
    model_switch: Option<ModelSwitchInfo>,
    restoring: bool,
    held_listeners: BTreeMap<String, StdListener>,
}

pub struct Manager {
    store: Store,
    catalog: Mutex<Catalog>,
    bundled_ids: BTreeSet<String>,
    profile: Option<RuntimeProfile>,
    profiles: Vec<RuntimeProfile>,
    validation_mode: bool,
    settings: PathBuf,
    instance: String,
    inner: Mutex<Inner>,
    sink: Arc<dyn Fn(Snapshot) + Send + Sync>,
    closed: AtomicBool,
    #[cfg(test)]
    engine_factory: Mutex<Option<EngineFactory>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn same_model_data(left: &Model, right: &Model) -> bool {
    if left.engine != right.engine
        || left.revision != right.revision
        || left.native_format != right.native_format
        || left.files.len() != right.files.len()
    {
        return false;
    }
    left.files.iter().all(|file| {
        right.files.iter().any(|other| {
            file.path == other.path && file.bytes == other.bytes && file.sha256 == other.sha256
        })
    })
}

impl Manager {
    pub fn new(
        root: &Path,
        settings: &Path,
        sink: impl Fn(Snapshot) + Send + Sync + 'static,
    ) -> Result<Arc<Self>> {
        Self::with_profiles(
            root,
            settings,
            catalog::catalog()?,
            catalog::runtime_profiles()?,
            false,
            Arc::new(sink),
        )
    }

    #[cfg(feature = "runtime-validation")]
    pub fn for_validation(
        root: &Path,
        settings: &Path,
        sink: impl Fn(Snapshot) + Send + Sync + 'static,
    ) -> Result<Arc<Self>> {
        if settings != root.join("settings.json") {
            return Err(Error::UnsafePath);
        }
        storage::plain(root.parent().ok_or(Error::UnsafePath)?)?;
        fs::create_dir(root).map_err(|_| Error::Storage)?;
        storage::private_dir(root)?;
        Self::with_profiles(
            root,
            settings,
            catalog::catalog()?,
            catalog::runtime_profiles()?,
            true,
            Arc::new(sink),
        )
    }

    #[cfg(test)]
    fn with_catalog(
        root: &Path,
        settings: &Path,
        catalog: Catalog,
        profile: Option<RuntimeProfile>,
        sink: Arc<dyn Fn(Snapshot) + Send + Sync>,
    ) -> Result<Arc<Self>> {
        Self::with_profiles(
            root,
            settings,
            catalog,
            profile.into_iter().collect(),
            false,
            sink,
        )
    }

    fn with_profiles(
        root: &Path,
        settings: &Path,
        catalog: Catalog,
        profiles: Vec<RuntimeProfile>,
        validation_mode: bool,
        sink: Arc<dyn Fn(Snapshot) + Send + Sync>,
    ) -> Result<Arc<Self>> {
        let store = Store::open(root)?;
        let bundled_ids = catalog
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        let mut catalog = catalog;
        for registered in store.registered_catalogs()? {
            catalog = catalog.merge(&registered)?;
        }
        Ok(Arc::new(Self {
            store,
            catalog: Mutex::new(catalog),
            bundled_ids,
            profile: profiles.first().cloned(),
            profiles,
            validation_mode,
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
                inspection: None,
                model_switch: None,
                restoring: false,
                held_listeners: BTreeMap::new(),
            }),
            sink,
            closed: AtomicBool::new(false),
            #[cfg(test)]
            engine_factory: Mutex::new(None),
        }))
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut inner = lock(&self.inner);
        let catalog = lock(&self.catalog);
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
                catalog
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
            catalog_version: catalog.version,
            catalog_fingerprint: catalog.fingerprint().unwrap_or_default(),
            models: catalog
                .models
                .iter()
                .map(|model| {
                    self.model_info(
                        model,
                        if self.bundled_ids.contains(&model.id) {
                            "bundled"
                        } else {
                            "registered"
                        },
                    )
                })
                .collect(),
            runtime: self.runtime_info(self.profile.as_ref()),
            operation: inner.operation.clone(),
            server: inner.server.clone(),
            provider_identity,
            configured_model_id: configured,
            applied_model_id: applied,
            source_inspection: inner
                .inspection
                .as_ref()
                .map(|pending| self.inspection_info(pending)),
            model_switch: inner.model_switch.clone(),
        }
    }

    fn model_info(&self, model: &Model, origin: &'static str) -> ModelInfo {
        let profile = self
            .profiles
            .iter()
            .find(|profile| profile.engine == model.engine);
        let local_error = adapter::local_profile(model, profile, false).err();
        let installed = self.store.installed(model);
        ModelInfo {
            id: model.id.clone(),
            label: model.label.clone(),
            engine: model.engine,
            protocol: model.protocol,
            revision: model.revision.clone(),
            license: model.license.clone(),
            source: model.source.clone(),
            quality: model.quality.clone(),
            download_bytes: model.download_bytes(),
            weights_bytes: model.weights_bytes(),
            manifest_fingerprint: model.fingerprint().unwrap_or_default(),
            origin,
            server_model_id: model.server_model_id.clone(),
            native_format: model.native_format.clone(),
            local: LocalSupport {
                supported: local_error.is_none(),
                reason: local_error.map(Error::token),
                inputs: ["text", "json"],
                unsupported_inputs: ["images", "videos"],
            },
            existing_server: ExistingServerSupport {
                supported: true,
                protocol: model.protocol,
                model_id: model.server_model_id.clone(),
                requires_check: true,
            },
            runtime: self.runtime_info(profile),
            installed,
            installed_directory: if installed {
                self.store
                    .model_path(model)
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
            } else {
                None
            },
            compatibility: if local_error.is_none() {
                "supported"
            } else {
                "unsupported"
            },
            compatibility_reason: local_error.map(Error::token),
        }
    }

    fn inspection_info(&self, pending: &PendingInspection) -> SourceInspection {
        let model = self.model_info(&pending.inspected.model, "inspection");
        let local_source = pending.inspected.directory.is_some();
        SourceInspection {
            inspection_id: pending.id.clone(),
            instance_id: self.instance.clone(),
            catalog_fingerprint: pending.catalog_fingerprint.clone(),
            ready_for_install: model.local.supported,
            reason: model.local.reason,
            local: model.local.clone(),
            existing_server: model.existing_server.clone(),
            download_bytes: if local_source {
                0
            } else {
                model.download_bytes
            },
            copy_bytes: if local_source {
                model.download_bytes
            } else {
                0
            },
            runtime_download_bytes: if model.runtime.state == "ready" {
                0
            } else {
                model.runtime.download_bytes
            },
            source_kind: if local_source {
                "local_directory"
            } else {
                "hugging_face"
            },
            model,
            default_destination_directory: self
                .store
                .default_models_directory()
                .to_string_lossy()
                .into_owned(),
            allows_destination_selection: true,
        }
    }

    pub async fn inspect_source(self: &Arc<Self>, input: SourceInput) -> Result<SourceInspection> {
        let (operation_id, cancel) = self.begin("inspect", None, 0)?;
        let work_stop = Arc::new(AtomicBool::new(false));
        let stop = work_stop.clone();
        let request_cancel = cancel.clone();
        let cancelled: source::Cancelled = Arc::new(move || {
            request_cancel.flag.load(Ordering::Acquire) || stop.load(Ordering::Acquire)
        });
        let manager = self.clone();
        let progress_id = operation_id.clone();
        let progress: source::Progress =
            Arc::new(move |bytes| manager.progress(&progress_id, bytes));
        let result = tokio::select! {
            _ = cancel.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(Duration::from_secs(300), source::inspect(input, cancelled, progress)) => result.unwrap_or(Err(Error::Timeout)),
        };
        work_stop.store(true, Ordering::Release);
        let result = result.and_then(|mut inspected| {
            cancel.check()?;
            let mut inner = lock(&self.inner);
            cancel.check()?;
            if self.closed.load(Ordering::Acquire) {
                return Err(Error::Cancelled);
            }
            let catalog = lock(&self.catalog);
            if let Some(existing) = catalog
                .models
                .iter()
                .find(|model| same_model_data(model, &inspected.model))
            {
                inspected.model = existing.clone();
            }
            let pending = PendingInspection {
                id: uuid::Uuid::new_v4().to_string(),
                catalog_fingerprint: catalog.fingerprint()?,
                inspected,
            };
            let result = self.inspection_info(&pending);
            inner.inspection = Some(pending);
            Self::complete(&mut inner);
            Ok(result)
        });
        self.finish(
            &operation_id,
            result.as_ref().map(|_| ()).map_err(|error| *error),
        );
        result
    }

    pub fn install_source(
        self: &Arc<Self>,
        inspection_id: &str,
        instance_id: &str,
        expected_catalog: &str,
        destination: Option<PathBuf>,
    ) -> Result<Snapshot> {
        if instance_id != self.instance {
            return Err(Error::InstanceChanged);
        }
        let mut inner = lock(&self.inner);
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::ServerStopped);
        }
        if inner.cancel.is_some() {
            return Err(Error::Busy);
        }
        let pending = inner.inspection.as_ref().ok_or(Error::InspectionChanged)?;
        if pending.id != inspection_id || pending.catalog_fingerprint != expected_catalog {
            return Err(Error::InspectionChanged);
        }
        let mut catalog = lock(&self.catalog);
        if catalog.fingerprint()? != expected_catalog {
            return Err(Error::RevisionChanged);
        }
        let inspected = pending.inspected.clone();
        inspected.check_source()?;
        let model = inspected.model;
        self.model_profile(&model)?;
        if self.store.installed(&model) {
            drop(catalog);
            drop(inner);
            return Ok(self.snapshot());
        }
        if inner
            .running
            .as_ref()
            .is_some_and(|running| running.model_id == model.id)
        {
            return Err(Error::Busy);
        }
        self.store
            .require_model_space(model.download_bytes(), destination.as_deref())?;
        if let Some(existing) = catalog
            .models
            .iter()
            .find(|existing| existing.id == model.id)
        {
            if existing.fingerprint()? != model.fingerprint()? {
                return Err(Error::CatalogConflict);
            }
        } else {
            let addition = Catalog {
                version: 1,
                models: vec![model.clone()],
            };
            let merged = catalog.merge(&addition)?;
            self.store.register_catalog(&addition)?;
            *catalog = merged;
        }
        let kind = if inspected.directory.is_some() {
            "import"
        } else {
            "download"
        };
        let (id, cancel) = self.reserve(&mut inner, kind, Some(&model), model.download_bytes())?;
        inner.inspection = None;
        drop(catalog);
        drop(inner);
        self.emit();
        let manager = self.clone();
        tokio::spawn(async move {
            let result = manager
                .install_model_at(&model, inspected.directory, destination, &id, &cancel)
                .await;
            manager.finish(&id, result);
        });
        Ok(self.snapshot())
    }

    fn runtime_info(&self, profile: Option<&RuntimeProfile>) -> RuntimeInfo {
        match profile {
            Some(profile) => RuntimeInfo {
                state: if self.store.runtime_ready(profile) {
                    "ready"
                } else if !profile.validated {
                    "unverified"
                } else {
                    "missing"
                },
                python_requirement: profile
                    .python
                    .version
                    .clone()
                    .unwrap_or_else(|| profile.python_version.clone()),
                download_bytes: profile.download_bytes(),
                reason: (!profile.validated).then_some(Error::EngineUnverified.token()),
            },
            None => RuntimeInfo {
                state: "unsupported",
                python_requirement: String::new(),
                download_bytes: 0,
                reason: Some(Error::Unsupported.token()),
            },
        }
    }

    fn model_profile(&self, model: &Model) -> Result<&RuntimeProfile> {
        adapter::local_profile(
            model,
            self.profiles
                .iter()
                .find(|profile| profile.engine == model.engine),
            self.validation_mode,
        )
    }

    fn emit(&self) {
        (self.sink)(self.snapshot());
    }

    pub fn notify_settings_changed(&self) {
        lock(&self.inner).revision += 1;
        self.emit();
    }

    fn model(&self, id: &str, revision: &str) -> Result<Model> {
        let catalog = lock(&self.catalog);
        let model = catalog
            .models
            .iter()
            .find(|model| model.id == id)
            .ok_or(Error::UnknownModel)?;
        if model.revision != revision {
            return Err(Error::RevisionChanged);
        }
        Ok(model.clone())
    }

    pub fn register_catalog(
        &self,
        instance_id: &str,
        expected: &str,
        document: &str,
    ) -> Result<Snapshot> {
        if instance_id != self.instance {
            return Err(Error::InstanceChanged);
        }
        let addition = Catalog::parse(document)?;
        let mut inner = lock(&self.inner);
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::ServerStopped);
        }
        if inner.cancel.is_some() {
            return Err(Error::Busy);
        }
        let mut catalog = lock(&self.catalog);
        if catalog.fingerprint()? != expected {
            return Err(Error::RevisionChanged);
        }
        let merged = catalog.merge(&addition)?;
        self.store.register_catalog(&addition)?;
        *catalog = merged;
        inner.inspection = None;
        inner.revision += 1;
        drop(catalog);
        drop(inner);
        self.emit();
        Ok(self.snapshot())
    }

    fn begin(
        &self,
        kind: &'static str,
        model: Option<&Model>,
        total: u64,
    ) -> Result<(String, Arc<Cancel>)> {
        let mut inner = lock(&self.inner);
        let result = self.reserve(&mut inner, kind, model, total)?;
        drop(inner);
        self.emit();
        Ok(result)
    }

    fn reserve(
        &self,
        inner: &mut Inner,
        kind: &'static str,
        model: Option<&Model>,
        total: u64,
    ) -> Result<(String, Arc<Cancel>)> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::ServerStopped);
        }
        if inner.cancel.is_some() {
            return Err(Error::Busy);
        }
        if kind == "inspect" {
            inner.inspection = None;
        }
        inner.model_switch = None;
        inner.restoring = false;
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
        let restore_failed = inner
            .model_switch
            .as_ref()
            .is_some_and(|model_switch| model_switch.state == "restore_failed");
        if let Some(operation) = &mut inner.operation {
            let result = if restore_failed {
                Err(Error::RestoreFailed)
            } else if cancelled {
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
        let pending = inner.pending_engine.take();
        inner.cancel = None;
        inner.restoring = false;
        inner.revision += 1;
        drop(inner);
        drop(pending);
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
        operation.received_bytes = operation.received_bytes.saturating_add(added);
        if operation.total_bytes != 0 {
            operation.received_bytes = operation.received_bytes.min(operation.total_bytes);
        }
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
        if inner.restoring {
            return Err(Error::Busy);
        }
        if inner.cancel.is_some()
            && inner.model_switch.as_ref().is_some_and(|switch| {
                inner.running.as_ref().is_some_and(|running| {
                    switch.target_model_id == running.model_id
                        && switch.previous_model_id != switch.target_model_id
                        && Provider::read(&self.settings).is_ok_and(|provider| {
                            provider.identity() == running.provider.identity()
                        })
                })
            })
        {
            return Err(Error::SettingsChanged);
        }
        if inner
            .operation
            .as_ref()
            .is_none_or(|operation| operation.id != operation_id)
        {
            return Err(Error::RevisionChanged);
        }
        let pending = if let Some(cancel) = &inner.cancel {
            cancel.stop();
            let pending = inner.pending_engine.clone();
            if let Some(operation) = &mut inner.operation {
                operation.state = "cancelling";
            }
            inner.revision += 1;
            pending
        } else {
            None
        };
        drop(inner);
        if let Some(engine) = pending {
            engine.stop();
        }
        self.emit();
        Ok(self.snapshot())
    }

    pub fn prepare_runtime(self: &Arc<Self>) -> Result<Snapshot> {
        let profile = self.profile.clone().ok_or(Error::Unsupported)?;
        self.prepare_profile(profile)
    }

    pub fn prepare_model_runtime(
        self: &Arc<Self>,
        model_id: &str,
        revision: &str,
    ) -> Result<Snapshot> {
        let model = self.model(model_id, revision)?;
        self.prepare_profile(self.model_profile(&model)?.clone())
    }

    fn prepare_profile(self: &Arc<Self>, profile: RuntimeProfile) -> Result<Snapshot> {
        if !profile.validated && !self.validation_mode {
            return Err(Error::EngineUnverified);
        }
        if self.store.runtime_ready(&profile) {
            return Ok(self.snapshot());
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
        for artifact in std::iter::once(&profile.python)
            .chain(&profile.wheels)
            .chain(&profile.runtime_files)
        {
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
            for artifact in &unpack_profile.runtime_files {
                let source = downloads.join(&artifact.path);
                let destination = storage::child(
                    &unpack_root,
                    &format!("{}/{}", unpack_profile.site_packages, artifact.path),
                )?;
                storage::private_dir(destination.parent().ok_or(Error::UnsafePath)?)?;
                fs::rename(source, destination).map_err(|_| Error::Storage)?;
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
        self.model_profile(&model)?;
        if source.is_none()
            && model
                .files
                .iter()
                .any(|file| catalog::local_source(&file.url))
        {
            return Err(Error::SourceRequiresFolder);
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
        self.install_model_at(model, source, None, id, cancel).await
    }

    async fn install_model_at(
        self: &Arc<Self>,
        model: &Model,
        source: Option<PathBuf>,
        destination: Option<PathBuf>,
        id: &str,
        cancel: &Arc<Cancel>,
    ) -> Result<()> {
        let mut stage = self.store.model_stage(model, destination.as_deref())?;
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
        let verify_model = model.clone();
        let verify_path = stage.path().to_owned();
        let verify_cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            adapter::verify_model(&verify_path, &verify_model, &verify_cancel.flag)
        })
        .await
        .map_err(|_| Error::Storage)??;
        cancel.check()?;
        self.commit(cancel, || self.store.commit_model(&mut stage, model))
    }

    fn complete(inner: &mut Inner) {
        inner.cancel = None;
        inner.pending_engine.take();
        inner.restoring = false;
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
        let profile = self.model_profile(&model)?.clone();
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
            let result = manager
                .start_model(
                    &model,
                    &profile,
                    &cancel,
                    StartOptions {
                        complete: true,
                        state: "ready",
                        check_before_listen: false,
                        require_listener: false,
                    },
                )
                .await;
            manager.finish(&id, result);
        });
        Ok(self.snapshot())
    }

    async fn start_model(
        self: &Arc<Self>,
        model: &Model,
        profile: &RuntimeProfile,
        cancel: &Arc<Cancel>,
        options: StartOptions,
    ) -> Result<()> {
        let path = self.store.model_path(model)?;
        let verify_model = model.clone();
        let verify_path = path.clone();
        let verify_cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            adapter::verify_model(&verify_path, &verify_model, &verify_cancel.flag)
        })
        .await
        .map_err(|_| Error::Storage)??;
        cancel.check()?;
        let listener = self
            .model_listener(&model.id, options.require_listener)
            .await?;
        let address = listener.local_addr().map_err(|_| Error::RuntimeFailed)?;
        let provider = Provider {
            kind: judgment_provider::ProviderKind::Local,
            protocol: judgment_provider::Protocol::SystemOne,
            base_url: format!("http://127.0.0.1:{}", address.port()),
            model: model.id.clone(),
            requires_auth: false,
        };
        let runtime = self.store.runtime_path(profile)?;
        let engine = self.model_engine(&runtime, profile, &path, model)?;
        self.pending(engine.clone(), cancel)?;
        wait_ready(&engine, cancel).await?;
        if options.check_before_listen {
            {
                let mut inner = lock(&self.inner);
                if !inner.restoring
                    && let Some(switch) = &mut inner.model_switch
                {
                    switch.state = "checking";
                }
                inner.revision += 1;
            }
            self.emit();
            native_pipe_check(&provider, &engine, cancel).await?;
        }
        let manager = Arc::downgrade(self);
        let running = engine::serve_on(
            model,
            engine,
            Arc::new(move || {
                if let Some(manager) = manager.upgrade() {
                    manager.emit();
                }
            }),
            Some(listener),
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
            state: options.state,
            model_id: Some(model.id.clone()),
            provider: Some(running.provider.clone()),
            checked: options.check_before_listen,
            error: None,
        };
        inner.held_listeners.remove(&model.id);
        inner.running = Some(running);
        if options.complete {
            Self::complete(&mut inner);
        } else {
            inner.revision += 1;
        }
        Ok(())
    }

    async fn model_listener(&self, model_id: &str, require_existing: bool) -> Result<StdListener> {
        {
            let inner = lock(&self.inner);
            if let Some(listener) = inner.held_listeners.get(model_id) {
                return listener.try_clone().map_err(|_| Error::RuntimeFailed);
            }
        }
        if require_existing {
            return Err(Error::ServerStopped);
        }
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| Error::RuntimeFailed)?
            .into_std()
            .map_err(|_| Error::RuntimeFailed)?;
        let duplicate = listener.try_clone().map_err(|_| Error::RuntimeFailed)?;
        let mut inner = lock(&self.inner);
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if inner.held_listeners.len() >= catalog::MAX_MODELS {
            return Err(Error::TooLarge);
        }
        inner.held_listeners.insert(model_id.to_owned(), listener);
        Ok(duplicate)
    }

    async fn retire_running(&self, mut running: Running) -> Result<()> {
        let listener = running.take_listener()?;
        {
            let mut inner = lock(&self.inner);
            if !self.closed.load(Ordering::Acquire) {
                inner
                    .held_listeners
                    .insert(running.model_id.clone(), listener);
            }
        }
        running.retire().await;
        Ok(())
    }

    fn model_engine(
        &self,
        runtime: &Path,
        profile: &RuntimeProfile,
        path: &Path,
        model: &Model,
    ) -> Result<Arc<Engine>> {
        #[cfg(test)]
        if let Some(factory) = lock(&self.engine_factory).clone() {
            return factory(model);
        }
        Engine::spawn(runtime, profile, Some((path, model)))
    }

    pub fn switch_model(
        self: &Arc<Self>,
        model_id: &str,
        revision: &str,
        server_id: &str,
        expected: &str,
        instance_id: &str,
        apply: impl FnOnce(&Provider, &str, &Binding) -> Result<()> + Send + 'static,
    ) -> Result<Snapshot> {
        self.switch_model_guarded(
            model_id,
            revision,
            server_id,
            expected,
            instance_id,
            (apply, |action: &mut dyn FnMut() -> Result<()>| action()),
        )
    }

    pub fn switch_model_guarded<Apply, Guard>(
        self: &Arc<Self>,
        model_id: &str,
        revision: &str,
        server_id: &str,
        expected: &str,
        instance_id: &str,
        callbacks: (Apply, Guard),
    ) -> Result<Snapshot>
    where
        Apply: FnOnce(&Provider, &str, &Binding) -> Result<()> + Send + 'static,
        Guard: Fn(&mut dyn FnMut() -> Result<()>) -> Result<()> + Send + Sync + 'static,
    {
        let (apply, guard) = callbacks;
        if instance_id != self.instance {
            return Err(Error::InstanceChanged);
        }
        let model = self.model(model_id, revision)?;
        let profile = self.model_profile(&model)?.clone();
        if !self.store.runtime_ready(&profile) {
            return Err(Error::RuntimeMissing);
        }
        if !self.store.installed(&model) {
            return Err(Error::ModelMissing);
        }
        let mut inner = lock(&self.inner);
        let running = inner.running.as_ref().ok_or(Error::ServerStopped)?;
        if running.id != server_id {
            return Err(Error::ServerChanged);
        }
        if !running.engine.alive() {
            return Err(Error::ServerStopped);
        }
        if Provider::read(&self.settings)
            .map_err(|_| Error::SettingsChanged)?
            .identity()
            != expected
        {
            return Err(Error::SettingsChanged);
        }
        let previous_model_id = running.model_id.clone();
        let (id, cancel) = self.reserve(&mut inner, "switch", Some(&model), 0)?;
        inner.model_switch = Some(ModelSwitchInfo {
            previous_model_id,
            target_model_id: model.id.clone(),
            state: "verifying",
            error: None,
            restore_error: None,
        });
        drop(inner);
        self.emit();
        let request = SwitchRequest {
            model,
            profile,
            server_id: server_id.to_owned(),
            expected: expected.to_owned(),
        };
        let manager = self.clone();
        tokio::spawn(async move {
            let result = manager
                .transition(request, &cancel, apply, &guard, |provider| async move {
                    native_check(&provider).await
                })
                .await;
            manager.finish(&id, result);
        });
        Ok(self.snapshot())
    }

    async fn verify_files(&self, model: &Model, cancel: &Arc<Cancel>) -> Result<()> {
        let path = self.store.model_path(model)?;
        let model = model.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || adapter::verify_model(&path, &model, &cancel.flag))
            .await
            .map_err(|_| Error::Storage)?
    }

    async fn transition<ProbeResult>(
        self: &Arc<Self>,
        request: SwitchRequest,
        cancel: &Arc<Cancel>,
        apply: impl FnOnce(&Provider, &str, &Binding) -> Result<()> + Send,
        guard: &(impl Fn(&mut dyn FnMut() -> Result<()>) -> Result<()> + Send + Sync),
        probe: impl FnOnce(Provider) -> ProbeResult + Send,
    ) -> Result<()>
    where
        ProbeResult: std::future::Future<Output = Result<()>> + Send,
    {
        let mut previous = None;
        let result = async {
            cancel.check()?;
            self.verify_files(&request.model, cancel).await?;
            cancel.check()?;
            let mut old_running = None;
            guard(&mut || {
                let mut inner = lock(&self.inner);
                cancel.check()?;
                let running = inner.running.as_ref().ok_or(Error::ServerStopped)?;
                if running.id != request.server_id {
                    return Err(Error::ServerChanged);
                }
                if !running.engine.alive() {
                    return Err(Error::ServerStopped);
                }
                if Provider::read(&self.settings)
                    .map_err(|_| Error::SettingsChanged)?
                    .identity()
                    != request.expected
                {
                    return Err(Error::SettingsChanged);
                }
                if running.model_id != request.model.id {
                    let old_model = lock(&self.catalog)
                        .models
                        .iter()
                        .find(|model| model.id == running.model_id)
                        .cloned()
                        .ok_or(Error::UnknownModel)?;
                    previous = Some(PreviousServer {
                        profile: self.model_profile(&old_model)?.clone(),
                        model: old_model,
                        provider: running.provider.clone(),
                    });
                    old_running = inner.running.take();
                    inner.server = ServerInfo {
                        state: "starting",
                        model_id: Some(request.model.id.clone()),
                        ..ServerInfo::default()
                    };
                    if let Some(switch) = &mut inner.model_switch {
                        switch.state = "starting";
                    }
                    inner.revision += 1;
                }
                drop(inner);
                if let Some(running) = &old_running {
                    running.engine.stop();
                }
                Ok(())
            })?;
            if let Some(running) = old_running {
                self.retire_running(running).await?;
            }
            if previous.is_some() {
                self.emit();
                self.start_model(
                    &request.model,
                    &request.profile,
                    cancel,
                    StartOptions {
                        complete: false,
                        state: "ready",
                        check_before_listen: true,
                        require_listener: false,
                    },
                )
                .await?;
            }
            let (server_id, provider, checked) = {
                let mut inner = lock(&self.inner);
                let running = inner.running.as_ref().ok_or(Error::ServerStopped)?;
                let result = (
                    running.id.clone(),
                    running.provider.clone(),
                    inner.server.checked,
                );
                if let Some(switch) = &mut inner.model_switch {
                    switch.state = if result.2 { "applying" } else { "checking" };
                }
                inner.revision += 1;
                result
            };
            self.emit();
            if !checked {
                cancel.until_cancelled(probe(provider.clone())).await?;
            }
            let mut inner = lock(&self.inner);
            cancel.check()?;
            let running = inner.running.as_ref().ok_or(Error::ServerStopped)?;
            if running.id != server_id {
                return Err(Error::ServerChanged);
            }
            if !running.engine.alive() {
                return Err(Error::ServerStopped);
            }
            inner.server.checked = true;
            inner.server.error = None;
            let binding = Binding {
                model_id: request.model.id.clone(),
                revision: request.model.revision.clone(),
                provider_identity: provider.identity(),
            };
            if Provider::read(&self.settings)
                .map_err(|_| Error::SettingsChanged)?
                .identity()
                != request.expected
            {
                return Err(Error::SettingsChanged);
            }
            apply(&provider, &request.expected, &binding)?;
            if let Some(switch) = &mut inner.model_switch {
                switch.state = "applied";
            }
            Self::complete(&mut inner);
            Ok(())
        }
        .await;
        if let Err(error) = result {
            if let Some(previous) = previous {
                self.restore_after_switch(previous, &request.expected, error, guard)
                    .await;
            } else {
                let mut inner = lock(&self.inner);
                if matches!(
                    error,
                    Error::Protocol | Error::Timeout | Error::RuntimeFailed
                ) {
                    inner.server.checked = false;
                    inner.server.error = Some(error.token());
                }
                if let Some(switch) = &mut inner.model_switch {
                    switch.state = if error == Error::Cancelled {
                        "cancelled"
                    } else if error == Error::SettingsChanged {
                        "selection_changed"
                    } else {
                        "failed"
                    };
                    switch.error = Some(error.token());
                }
            }
        }
        result
    }

    async fn restore_after_switch(
        self: &Arc<Self>,
        previous: PreviousServer,
        expected: &str,
        failure: Error,
        guard: &(impl Fn(&mut dyn FnMut() -> Result<()>) -> Result<()> + Send + Sync),
    ) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let mut current = None;
        let mut stopped = (None, None);
        let mut preserve = false;
        let guarded = guard(&mut || {
            current = Provider::read(&self.settings)
                .ok()
                .map(|provider| provider.identity());
            let mut inner = lock(&self.inner);
            if let Some(switch) = &mut inner.model_switch {
                switch.error = Some(failure.token());
            }
            if inner
                .running
                .as_ref()
                .is_some_and(|running| Some(running.provider.identity()) == current)
            {
                if !inner.server.checked {
                    inner.server.error = Some(failure.token());
                }
                if let Some(switch) = &mut inner.model_switch {
                    switch.state = "selection_changed";
                }
                preserve = true;
                return Ok(());
            }
            let running = inner.running.take();
            let pending = inner.pending_engine.take();
            inner.server = ServerInfo {
                state: "stopped",
                ..ServerInfo::default()
            };
            inner.restoring = current.as_deref() == Some(expected);
            if let Some(switch) = &mut inner.model_switch {
                switch.state = if current.as_deref() == Some(expected) {
                    "restoring"
                } else {
                    "selection_changed"
                };
            }
            inner.revision += 1;
            stopped = (running, pending);
            drop(inner);
            if let Some(running) = &stopped.0 {
                running.engine.stop();
            }
            if let Some(engine) = &stopped.1 {
                engine.stop();
            }
            Ok(())
        });
        if preserve {
            return;
        }
        if let Err(error) = guarded {
            let mut inner = lock(&self.inner);
            if let Some(switch) = &mut inner.model_switch {
                switch.state = "selection_changed";
                switch.error = Some(failure.token());
                switch.restore_error = Some(error.token());
            }
            return;
        }
        if let Some(running) = stopped.0 {
            let _ = self.retire_running(running).await;
        }
        self.emit();
        if current.as_deref() != Some(expected) {
            return;
        }
        let recovery_cancel = Arc::new(Cancel::new());
        let restored = async {
            self.start_model(
                &previous.model,
                &previous.profile,
                &recovery_cancel,
                StartOptions {
                    complete: false,
                    state: "restoring",
                    check_before_listen: true,
                    require_listener: true,
                },
            )
            .await?;
            let mut inner = lock(&self.inner);
            let running = inner.running.as_ref().ok_or(Error::ServerStopped)?;
            if !running.engine.alive()
                || running.provider.identity() != previous.provider.identity()
            {
                return Err(Error::ServerChanged);
            }
            inner.server.state = "ready";
            inner.server.checked = true;
            inner.server.error = None;
            inner.pending_engine.take();
            if let Some(switch) = &mut inner.model_switch {
                switch.state = "restored";
            }
            inner.revision += 1;
            Ok(())
        }
        .await;
        if let Err(error) = restored {
            let stopped = {
                let mut inner = lock(&self.inner);
                let running = inner.running.take();
                let pending = inner.pending_engine.take();
                inner.server = ServerInfo {
                    state: "failed",
                    model_id: Some(previous.model.id),
                    error: Some(Error::RestoreFailed.token()),
                    ..ServerInfo::default()
                };
                if let Some(switch) = &mut inner.model_switch {
                    switch.state = "restore_failed";
                    switch.restore_error = Some(error.token());
                }
                inner.revision += 1;
                (running, pending)
            };
            if let Some(engine) = stopped.1 {
                engine.stop();
            }
            if let Some(running) = stopped.0 {
                let _ = self.retire_running(running).await;
            }
        }
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
        let catalog = lock(&self.catalog);
        let model = catalog
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
        drop(catalog);
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
        let running = inner.running.take();
        inner.server = ServerInfo {
            state: "stopped",
            ..ServerInfo::default()
        };
        inner.revision += 1;
        drop(inner);
        drop(running);
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
        inner.held_listeners.remove(model_id);
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
        let pending = inner.pending_engine.take();
        let running = inner.running.take();
        inner.held_listeners.clear();
        inner.server = ServerInfo {
            state: "stopped",
            ..ServerInfo::default()
        };
        inner.revision += 1;
        drop(inner);
        if let Some(engine) = pending {
            engine.stop();
        }
        drop(running);
    }
}

async fn wait_ready(engine: &Arc<Engine>, cancel: &Cancel) -> Result<()> {
    wait_ready_for(engine, cancel, engine::START_TIMEOUT).await
}

async fn wait_ready_for(engine: &Arc<Engine>, cancel: &Cancel, timeout: Duration) -> Result<()> {
    let process = engine.clone();
    let ready = tokio::task::spawn_blocking(move || process.ready());
    let result = cancel
        .until_cancelled(async {
            match tokio::time::timeout(timeout, ready).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(Error::RuntimeFailed),
                Err(_) => Err(Error::Timeout),
            }
        })
        .await;
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

fn native_probe(provider: &Provider) -> Result<Vec<u8>> {
    serde_json::to_vec(&json!({
        "model":provider.identity(), "state":{"value":7},
        "questions":{
            "choice":{"type":"choice","instructions":"Choose the description matching value 7.","criteria":{"seven":"value equals seven","other":"another value"}},
            "score":{"type":"score","instructions":"How well does the value equal seven?","criteria":["not seven","seven"]},
            "noul":{"type":"noul","instructions":"The value equals seven."}
        }
    })).map_err(|_| Error::Protocol)
}

async fn native_pipe_check(
    provider: &Provider,
    engine: &Arc<Engine>,
    cancel: &Cancel,
) -> Result<()> {
    let prepared = provider
        .prepare(native_probe(provider)?)
        .map_err(|_| Error::Protocol)?;
    let body = prepared.body.clone();
    let process = engine.clone();
    let exchange = tokio::task::spawn_blocking(move || process.exchange(&body));
    let result = cancel
        .until_cancelled(async {
            match tokio::time::timeout(engine::REQUEST_TIMEOUT, exchange).await {
                Ok(Ok(Ok(bytes))) => provider
                    .decode(&prepared, bytes)
                    .map(|_| ())
                    .map_err(|_| Error::Protocol),
                Ok(Ok(Err(error))) => Err(error),
                Ok(Err(_)) => Err(Error::RuntimeFailed),
                Err(_) => Err(Error::Timeout),
            }
        })
        .await;
    if result.is_err() {
        engine.stop();
    }
    result
}

pub async fn native_check(provider: &Provider) -> Result<()> {
    let prepared = provider
        .prepare(native_probe(provider)?)
        .map_err(|_| Error::Protocol)?;
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

    pub fn begin_registration_blocker(manager: &Manager) -> String {
        manager.begin("download", None, 0).unwrap().0
    }

    #[cfg(unix)]
    pub fn held_listener_count(manager: &Manager) -> usize {
        lock(&manager.inner).held_listeners.len()
    }

    #[cfg(unix)]
    pub fn switching_manager(
        root: &Path,
        settings: &Path,
        previous: Model,
        target: Model,
        factory: EngineFactory,
    ) -> Arc<Manager> {
        let runtimes: serde_json::Value =
            serde_json::from_str(include_str!("../manifests/runtime.json")).unwrap();
        let profile: RuntimeProfile =
            serde_json::from_value(runtimes["profiles"][0].clone()).unwrap();
        let manager = Manager::with_catalog(
            root,
            settings,
            Catalog {
                version: 1,
                models: vec![previous.clone(), target.clone()],
            },
            Some(profile.clone()),
            Arc::new(|_| {}),
        )
        .unwrap();
        *lock(&manager.engine_factory) = Some(factory);
        for model in [previous, target] {
            let mut stage = manager.store.stage().unwrap();
            fs::write(stage.path().join("model.safetensors"), b"weights").unwrap();
            manager.store.commit_model(&mut stage, &model).unwrap();
        }
        let mut stage = manager.store.stage().unwrap();
        let executable = storage::child(stage.path(), &profile.executable).unwrap();
        storage::private_dir(executable.parent().unwrap()).unwrap();
        fs::write(executable, b"fixture").unwrap();
        manager.store.commit_runtime(&mut stage, &profile).unwrap();
        manager
    }

    #[cfg(unix)]
    pub fn replace_engine_factory(manager: &Manager, factory: EngineFactory) {
        *lock(&manager.engine_factory) = Some(factory);
    }

    #[cfg(unix)]
    pub async fn running(
        manager: &Arc<Manager>,
        engine: Arc<Engine>,
        checked: bool,
    ) -> (String, Provider) {
        let model = lock(&manager.catalog).models[0].clone();
        let running = engine::serve(&model, engine, Arc::new(|| {}))
            .await
            .unwrap();
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

#[cfg(test)]
mod cancellation_tests {
    use super::{AtomicBool, Cancel, Error, Ordering};

    #[tokio::test]
    async fn a_recorded_stop_does_not_poll_the_operation() {
        let cancellation = Cancel::new();
        let polled = AtomicBool::new(false);
        cancellation.stop();
        let result = cancellation
            .until_cancelled(async {
                polled.store(true, Ordering::Relaxed);
                Ok(())
            })
            .await;
        assert_eq!(result, Err(Error::Cancelled));
        assert!(!polled.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn cancellation_during_a_failed_poll_wins_over_the_pipe_error() {
        let cancellation = Cancel::new();
        let result = cancellation
            .until_cancelled(async {
                cancellation.stop();
                Err(Error::RuntimeFailed)
            })
            .await;
        assert_eq!(result, Err(Error::Cancelled));
    }

    #[tokio::test]
    async fn a_stop_during_a_successful_poll_prevents_completion() {
        let cancellation = Cancel::new();
        let result = cancellation
            .until_cancelled(async {
                cancellation.stop();
                Ok(())
            })
            .await;
        assert_eq!(result, Err(Error::Cancelled));
    }

    #[tokio::test]
    async fn ordinary_failures_keep_their_original_reason() {
        let cancellation = Cancel::new();
        assert_eq!(
            cancellation
                .until_cancelled(async { Err(Error::RuntimeFailed) })
                .await,
            Err(Error::RuntimeFailed)
        );
    }

    #[tokio::test]
    async fn an_uninterrupted_operation_completes() {
        let cancellation = Cancel::new();
        assert_eq!(cancellation.until_cancelled(async { Ok(()) }).await, Ok(()));
    }

    #[cfg(unix)]
    mod existing_model_probe {
        use super::super::*;
        use std::sync::atomic::AtomicUsize;

        struct Fixture {
            _home: tempfile::TempDir,
            manager: Arc<Manager>,
            model: Model,
            server_id: String,
            expected: String,
            operation_id: String,
            cancel: Arc<Cancel>,
        }

        impl Fixture {
            fn request(&self) -> SwitchRequest {
                SwitchRequest {
                    model: self.model.clone(),
                    profile: self.manager.model_profile(&self.model).unwrap().clone(),
                    server_id: self.server_id.clone(),
                    expected: self.expected.clone(),
                }
            }
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                self.manager.shutdown();
            }
        }

        async fn fixture(checked: bool) -> Fixture {
            let home = tempfile::tempdir().unwrap();
            let settings = home.path().join("settings.json");
            fs::write(&settings, b"{}").unwrap();
            let model = crate::tests::fixture_model();
            let mut unused = model.clone();
            unused.id = "unused-fixture".to_string();
            let manager = test_support::switching_manager(
                &home.path().join("data"),
                &settings,
                model.clone(),
                unused,
                Arc::new(|_| panic!("the existing model must not be restarted")),
            );
            let engine = crate::tests::running_fixture_engine();
            let (server_id, provider) = test_support::running(&manager, engine, checked).await;
            fs::write(
                &settings,
                serde_json::to_vec(&serde_json::json!({"smart":{"judgmentProvider":provider}}))
                    .unwrap(),
            )
            .unwrap();
            let mut inner = lock(&manager.inner);
            let (operation_id, cancel) = manager
                .reserve(&mut inner, "switch", Some(&model), 0)
                .unwrap();
            inner.model_switch = Some(ModelSwitchInfo {
                previous_model_id: model.id.clone(),
                target_model_id: model.id.clone(),
                state: "verifying",
                error: None,
                restore_error: None,
            });
            drop(inner);
            Fixture {
                _home: home,
                manager,
                model,
                server_id,
                expected: provider.identity(),
                operation_id,
                cancel,
            }
        }

        fn allow(change: &mut dyn FnMut() -> Result<()>) -> Result<()> {
            change()
        }

        #[tokio::test]
        async fn a_cancelled_existing_model_probe_is_not_a_model_failure() {
            let fixture = fixture(false).await;
            let original = fs::read(&fixture.manager.settings).unwrap();
            let probes = Arc::new(AtomicUsize::new(0));
            let counted = probes.clone();
            let cancelled = fixture.cancel.clone();
            let result = fixture
                .manager
                .transition(
                    fixture.request(),
                    &fixture.cancel,
                    |_, _, _| panic!("a cancelled probe must not apply a provider"),
                    &allow,
                    move |_| async move {
                        counted.fetch_add(1, Ordering::Relaxed);
                        cancelled.stop();
                        Err(Error::Protocol)
                    },
                )
                .await;
            let outcome = result.as_ref().err().map(|error| error.token());
            fixture.manager.finish(&fixture.operation_id, result);
            let snapshot = fixture.manager.snapshot();
            assert_eq!(probes.load(Ordering::Relaxed), 1);
            assert_eq!(outcome, Some(Error::Cancelled.token()));
            assert_eq!(snapshot.operation.unwrap().state, "cancelled");
            let switching = snapshot.model_switch.unwrap();
            assert_eq!(switching.state, "cancelled");
            assert_eq!(switching.error, Some(Error::Cancelled.token()));
            assert!(!snapshot.server.checked);
            assert_eq!(snapshot.server.error, None);
            assert_eq!(
                snapshot.server.id.as_deref(),
                Some(fixture.server_id.as_str())
            );
            assert!(
                lock(&fixture.manager.inner)
                    .running
                    .as_ref()
                    .unwrap()
                    .engine
                    .alive()
            );
            assert_eq!(fs::read(&fixture.manager.settings).unwrap(), original);
        }

        #[tokio::test]
        async fn an_unchecked_existing_model_probes_once_before_applying() {
            let fixture = fixture(false).await;
            let probes = Arc::new(AtomicUsize::new(0));
            let counted = probes.clone();
            let applications = AtomicUsize::new(0);
            let result = fixture
                .manager
                .transition(
                    fixture.request(),
                    &fixture.cancel,
                    |provider, expected, binding| {
                        assert_eq!(provider.identity(), fixture.expected);
                        assert_eq!(expected, fixture.expected);
                        assert_eq!(binding.model_id, fixture.model.id);
                        assert_eq!(probes.load(Ordering::Relaxed), 1);
                        applications.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    },
                    &allow,
                    move |_| async move {
                        counted.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    },
                )
                .await;
            assert_eq!(result, Ok(()));
            assert_eq!(applications.load(Ordering::Relaxed), 1);
            assert!(fixture.manager.snapshot().server.checked);
            assert_eq!(
                fixture.manager.snapshot().model_switch.unwrap().state,
                "applied"
            );
        }

        #[tokio::test]
        async fn an_already_checked_existing_model_does_not_probe_again() {
            let fixture = fixture(true).await;
            let applications = AtomicUsize::new(0);
            let result = fixture
                .manager
                .transition(
                    fixture.request(),
                    &fixture.cancel,
                    |_, _, _| {
                        applications.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    },
                    &allow,
                    |_| async { panic!("a checked running model must not be probed again") },
                )
                .await;
            assert_eq!(result, Ok(()));
            assert_eq!(applications.load(Ordering::Relaxed), 1);
            assert!(fixture.manager.snapshot().server.checked);
        }
    }
}
