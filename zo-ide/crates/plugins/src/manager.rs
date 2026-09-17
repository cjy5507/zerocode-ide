//! `PluginManager` — discovery + install/update/uninstall + state I/O.
//!
//! Owns the [`PluginManagerConfig`] and the cached [`PluginRegistry`]
//! (and the "installed-from-disk" snapshot used during sync). Every
//! observable change to `~/.zo/plugins/installed.json` or the
//! enabled-flag in `settings.json` flows through one of this type's
//! methods so the caches stay in sync.
//!
//! The manager composes helpers from the rest of the crate:
//! [`builtin::load_plugin_definition`] resolves a discovered root,
//! [`install::*`] handles parsing + materialising + copying sources,
//! [`manifest_io::plugin_manifest_path`] locates `plugin.json`, and
//! [`path_validators::*`] (re-exported via the registered plugin) keeps
//! command paths valid at activation time.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::builtin::load_plugin_definition;
use super::install::{
    copy_dir_all, discover_plugin_dirs, ensure_object, git_head_commit, hash_plugin_tree,
    materialize_source, parse_install_source, resolve_local_source, update_settings_json,
    write_atomic,
};
use super::manifest::{BUNDLED_MARKETPLACE, EXTERNAL_MARKETPLACE};
use super::manifest_io::plugin_manifest_path;
use super::registry::PluginDiscovery;
use super::util::{describe_install_source, plugin_id, sanitize_plugin_id, unix_time_ms};
use super::{
    builtin_plugins, load_plugin_from_directory, InstalledPluginRecord, InstalledPluginRegistry,
    Plugin, PluginDefinition, PluginError, PluginHooks, PluginInstallSource, PluginKind,
    PluginLoadFailure, PluginManagerConfig, PluginManifest, PluginMetadata, PluginRegistry,
    PluginRegistryReport, PluginSummary, PluginTool, RegisteredPlugin,
};

pub(crate) const SETTINGS_FILE_NAME: &str = "settings.json";
pub(crate) const REGISTRY_FILE_NAME: &str = "installed.json";

/// Scratch directory (inside the install root) that holds in-flight bundled
/// copies and the trees they displace. It lives one level *above* the per-plugin
/// directories, and carries no `plugin.json` of its own, so `discover_plugin_dirs`
/// — which looks exactly one level deep and requires a manifest — never mistakes
/// a half-copied staging tree for an installed plugin.
const STAGING_DIR_NAME: &str = ".staging";

/// A staging entry older than this cannot belong to a live swap (a swap is a
/// recursive copy of a handful of small files), so it is a crash leftover and is
/// pruned. The bound is deliberately generous: deleting a peer's in-flight copy
/// would be far worse than leaving a stale directory around for one more run.
const STALE_STAGING_AGE: Duration = Duration::from_secs(600);

/// Serialises bundled sync against every other bundled sync *and* against the
/// installed-plugin scans in this process. Parallel test threads and concurrent
/// sessions share one install root, so without this a scan could run while a
/// peer was mid-swap. Cross-process safety comes from the rename-based publish
/// in [`publish_plugin_tree`]; this lock closes the in-process window entirely.
static BUNDLED_SYNC_LOCK: Mutex<()> = Mutex::new(());

/// Per-process counter making every staging slot unique, so two swaps of the
/// same plugin in one process cannot share (and clobber) a scratch directory.
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Take the in-process bundled-sync lock, ignoring poisoning: the guarded data
/// is `()`, and a panicking peer must not turn plugin discovery into a hard
/// failure for every later caller.
fn bundled_sync_guard() -> MutexGuard<'static, ()> {
    BUNDLED_SYNC_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone)]
pub struct PluginManager {
    config: PluginManagerConfig,
    cache: Arc<Mutex<PluginManagerCache>>,
}

#[derive(Debug, Default)]
struct PluginManagerCache {
    registry: Option<PluginRegistry>,
    installed_registry: Option<PluginRegistry>,
}

impl PartialEq for PluginManager {
    fn eq(&self, other: &Self) -> bool {
        self.config == other.config
    }
}

impl Eq for PluginManager {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub plugin_id: String,
    pub version: String,
    pub install_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOutcome {
    pub plugin_id: String,
    pub old_version: String,
    pub new_version: String,
    pub install_path: PathBuf,
}

impl PluginManager {
    #[must_use]
    pub fn new(config: PluginManagerConfig) -> Self {
        Self {
            config,
            cache: Arc::new(Mutex::new(PluginManagerCache::default())),
        }
    }

    #[must_use]
    pub fn bundled_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bundled")
    }

    #[must_use]
    pub fn install_root(&self) -> PathBuf {
        self.config
            .install_root
            .clone()
            .unwrap_or_else(|| self.config.config_home.join("plugins").join("installed"))
    }

    #[must_use]
    pub fn registry_path(&self) -> PathBuf {
        self.config.registry_path.clone().unwrap_or_else(|| {
            self.config
                .config_home
                .join("plugins")
                .join(REGISTRY_FILE_NAME)
        })
    }

    #[must_use]
    pub fn settings_path(&self) -> PathBuf {
        self.config.config_home.join(SETTINGS_FILE_NAME)
    }

    pub fn plugin_registry(&self) -> Result<PluginRegistry, PluginError> {
        if let Some(registry) = self.cached_registry() {
            return Ok(registry);
        }

        let registry = self.plugin_registry_report()?.into_registry()?;
        self.store_cached_registry(registry.clone());
        Ok(registry)
    }

    pub fn plugin_registry_report(&self) -> Result<PluginRegistryReport, PluginError> {
        if let Some(registry) = self.cached_registry() {
            return Ok(PluginRegistryReport::new(registry, Vec::new()));
        }

        // Held across sync *and* the installed scan: the scan must never observe
        // the install root while a peer in this process is swapping a tree in.
        let sync_guard = bundled_sync_guard();
        self.sync_bundled_plugins()?;

        let mut discovery = PluginDiscovery::default();
        discovery.plugins.extend(builtin_plugins());

        let installed = self.discover_installed_plugins_with_failures()?;
        discovery.extend(installed);
        drop(sync_guard);

        let external =
            self.discover_external_directory_plugins_with_failures(&discovery.plugins)?;
        discovery.extend(external);

        let report = self.build_registry_report(discovery);
        if !report.has_failures() {
            self.store_cached_registry(report.registry().clone());
        }
        Ok(report)
    }

    pub fn list_plugins(&self) -> Result<Vec<PluginSummary>, PluginError> {
        Ok(self.plugin_registry()?.summaries())
    }

    pub fn list_installed_plugins(&self) -> Result<Vec<PluginSummary>, PluginError> {
        Ok(self.installed_plugin_registry()?.summaries())
    }

    pub fn aggregated_hooks(&self) -> Result<PluginHooks, PluginError> {
        self.plugin_registry()?.aggregated_hooks()
    }

    pub fn aggregated_tools(&self) -> Result<Vec<PluginTool>, PluginError> {
        self.plugin_registry()?.aggregated_tools()
    }

    pub fn validate_plugin_source(&self, source: &str) -> Result<PluginManifest, PluginError> {
        let path = resolve_local_source(source)?;
        load_plugin_from_directory(&path)
    }

    pub fn install(&mut self, source: &str) -> Result<InstallOutcome, PluginError> {
        let install_source = parse_install_source(source)?;
        let temp_root = self.install_root().join(".tmp");
        let staged_source = materialize_source(&install_source, &temp_root)?;
        let is_git_source = matches!(install_source, PluginInstallSource::GitUrl { .. });
        let manifest = load_plugin_from_directory(&staged_source)?;
        // Record the exact commit checked out (provenance) before the staged
        // git tree is cleaned up.
        let resolved_commit = is_git_source
            .then(|| git_head_commit(&staged_source))
            .flatten();

        let plugin_id = plugin_id(&manifest.name, EXTERNAL_MARKETPLACE);
        let install_path = self.install_root().join(sanitize_plugin_id(&plugin_id));
        if install_path.exists() {
            fs::remove_dir_all(&install_path)?;
        }
        copy_dir_all(&staged_source, &install_path)?;
        if is_git_source {
            let _ = fs::remove_dir_all(&staged_source);
        }
        // Integrity baseline over the materialised copy, checked on every load.
        let content_sha256 = Some(hash_plugin_tree(&install_path)?);

        let now = unix_time_ms();
        let record = InstalledPluginRecord {
            kind: PluginKind::External,
            id: plugin_id.clone(),
            name: manifest.name,
            version: manifest.version.clone(),
            description: manifest.description,
            install_path: install_path.clone(),
            source: install_source,
            installed_at_unix_ms: now,
            updated_at_unix_ms: now,
            resolved_commit,
            content_sha256,
        };

        let mut registry = self.load_registry()?;
        registry.plugins.insert(plugin_id.clone(), record);
        self.store_registry(&registry)?;
        self.write_enabled_state(&plugin_id, Some(true))?;
        self.config.enabled_plugins.insert(plugin_id.clone(), true);
        self.invalidate_cache();

        Ok(InstallOutcome {
            plugin_id,
            version: manifest.version,
            install_path,
        })
    }

    pub fn enable(&mut self, plugin_id: &str) -> Result<(), PluginError> {
        self.ensure_known_plugin(plugin_id)?;
        self.write_enabled_state(plugin_id, Some(true))?;
        self.config
            .enabled_plugins
            .insert(plugin_id.to_string(), true);
        self.invalidate_cache();
        Ok(())
    }

    pub fn disable(&mut self, plugin_id: &str) -> Result<(), PluginError> {
        self.ensure_known_plugin(plugin_id)?;
        self.write_enabled_state(plugin_id, Some(false))?;
        self.config
            .enabled_plugins
            .insert(plugin_id.to_string(), false);
        self.invalidate_cache();
        Ok(())
    }

    pub fn uninstall(&mut self, plugin_id: &str) -> Result<(), PluginError> {
        let mut registry = self.load_registry()?;
        let record = registry.plugins.remove(plugin_id).ok_or_else(|| {
            PluginError::NotFound(format!("plugin `{plugin_id}` is not installed"))
        })?;
        if record.kind == PluginKind::Bundled {
            registry.plugins.insert(plugin_id.to_string(), record);
            return Err(PluginError::CommandFailed(format!(
                "plugin `{plugin_id}` is bundled and managed automatically; disable it instead"
            )));
        }
        if record.install_path.exists() {
            fs::remove_dir_all(&record.install_path)?;
        }
        self.store_registry(&registry)?;
        self.write_enabled_state(plugin_id, None)?;
        self.config.enabled_plugins.remove(plugin_id);
        self.invalidate_cache();
        Ok(())
    }

    pub fn update(&mut self, plugin_id: &str) -> Result<UpdateOutcome, PluginError> {
        let mut registry = self.load_registry()?;
        let record = registry.plugins.get(plugin_id).cloned().ok_or_else(|| {
            PluginError::NotFound(format!("plugin `{plugin_id}` is not installed"))
        })?;

        let temp_root = self.install_root().join(".tmp");
        let staged_source = materialize_source(&record.source, &temp_root)?;
        let is_git_source = matches!(record.source, PluginInstallSource::GitUrl { .. });
        let manifest = load_plugin_from_directory(&staged_source)?;
        let resolved_commit = is_git_source
            .then(|| git_head_commit(&staged_source))
            .flatten();

        if record.install_path.exists() {
            fs::remove_dir_all(&record.install_path)?;
        }
        copy_dir_all(&staged_source, &record.install_path)?;
        if is_git_source {
            let _ = fs::remove_dir_all(&staged_source);
        }
        let content_sha256 = Some(hash_plugin_tree(&record.install_path)?);

        let updated_record = InstalledPluginRecord {
            version: manifest.version.clone(),
            description: manifest.description,
            updated_at_unix_ms: unix_time_ms(),
            resolved_commit,
            content_sha256,
            ..record.clone()
        };
        registry
            .plugins
            .insert(plugin_id.to_string(), updated_record);
        self.store_registry(&registry)?;
        self.invalidate_cache();

        Ok(UpdateOutcome {
            plugin_id: plugin_id.to_string(),
            old_version: record.version,
            new_version: manifest.version,
            install_path: record.install_path,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn discover_installed_plugins_with_failures(&self) -> Result<PluginDiscovery, PluginError> {
        let mut registry = self.load_registry()?;
        let mut discovery = PluginDiscovery::default();
        let mut seen_ids = BTreeSet::<String>::new();
        let mut seen_paths = BTreeSet::<PathBuf>::new();
        let mut stale_registry_ids = Vec::new();

        let primary_install_root = self.install_root();
        for install_path in discover_plugin_dirs(&primary_install_root)? {
            let matched_record = registry
                .plugins
                .values()
                .find(|record| record.install_path == install_path);
            let kind = matched_record.map_or(PluginKind::External, |record| record.kind);
            let source = matched_record.map_or_else(
                || install_path.display().to_string(),
                |record| describe_install_source(&record.source),
            );
            if let Err(error) = validate_installed_plugin_path(&primary_install_root, &install_path)
            {
                if let Some(record) = matched_record {
                    // A registered higher-priority plugin reserves its ID when
                    // validation fails, so a lower-root copy cannot take over.
                    seen_ids.insert(record.id.clone());
                }
                seen_paths.insert(install_path.clone());
                discovery.push_failure(PluginLoadFailure::new(install_path, kind, source, error));
                continue;
            }
            // Supply-chain guard: reject a tampered on-disk copy before loading.
            if let Some(record) = matched_record {
                if let Err(error) = verify_plugin_integrity(record, &install_path) {
                    // A registered higher-priority plugin reserves its ID even
                    // when its contents fail integrity verification.
                    seen_ids.insert(record.id.clone());
                    // Mark the path seen so the registry-fallback loop below
                    // does not re-load the very tree we just rejected.
                    seen_paths.insert(install_path.clone());
                    discovery.push_failure(PluginLoadFailure::new(
                        install_path,
                        kind,
                        source,
                        error,
                    ));
                    continue;
                }
            }
            match load_plugin_definition(&install_path, kind, source.clone(), kind.marketplace()) {
                Ok(plugin) => {
                    if seen_ids.insert(plugin.metadata().id.clone()) {
                        seen_paths.insert(install_path);
                        discovery.push_plugin(plugin);
                    }
                }
                Err(error) => {
                    seen_paths.insert(install_path.clone());
                    discovery.push_failure(PluginLoadFailure::new(
                        install_path,
                        kind,
                        source,
                        error,
                    ));
                }
            }
        }

        self.discover_secondary_installed_plugins(
            &mut discovery,
            &mut seen_ids,
            &mut seen_paths,
        )?;

        for record in registry.plugins.values() {
            if seen_paths.contains(&record.install_path) {
                continue;
            }
            if !record.install_path.exists() || plugin_manifest_path(&record.install_path).is_err()
            {
                stale_registry_ids.push(record.id.clone());
                continue;
            }
            let source = describe_install_source(&record.source);
            if let Err(error) = validate_installed_plugin_path(&primary_install_root, &record.install_path)
            {
                seen_ids.insert(record.id.clone());
                discovery.push_failure(PluginLoadFailure::new(
                    record.install_path.clone(),
                    record.kind,
                    source,
                    error,
                ));
                continue;
            }
            if let Err(error) = verify_plugin_integrity(record, &record.install_path) {
                seen_ids.insert(record.id.clone());
                discovery.push_failure(PluginLoadFailure::new(
                    record.install_path.clone(),
                    record.kind,
                    source,
                    error,
                ));
                continue;
            }
            match load_plugin_definition(
                &record.install_path,
                record.kind,
                source.clone(),
                record.kind.marketplace(),
            ) {
                Ok(plugin) => {
                    if seen_ids.insert(plugin.metadata().id.clone()) {
                        seen_paths.insert(record.install_path.clone());
                        discovery.push_plugin(plugin);
                    }
                }
                Err(error) => {
                    discovery.push_failure(PluginLoadFailure::new(
                        record.install_path.clone(),
                        record.kind,
                        source,
                        error,
                    ));
                }
            }
        }

        if !stale_registry_ids.is_empty() {
            for plugin_id in stale_registry_ids {
                registry.plugins.remove(&plugin_id);
            }
            self.store_registry(&registry)?;
        }

        Ok(discovery)
    }

    #[allow(clippy::too_many_lines)]
    fn discover_secondary_installed_plugins(
        &self,
        discovery: &mut PluginDiscovery,
        seen_ids: &mut BTreeSet<String>,
        seen_paths: &mut BTreeSet<PathBuf>,
    ) -> Result<(), PluginError> {
        // The primary install root is scanned first, so these sets preserve its
        // precedence. Secondary canonical roots are discovery-only, but their
        // installed registries remain the authority for provenance and digests.
        for secondary_root in &self.config.discovery_install_roots {
            let registry = load_registry_at(&secondary_registry_path(secondary_root))?;
            for install_path in discover_plugin_dirs(secondary_root)? {
                if seen_paths.contains(&install_path) {
                    continue;
                }
                let matched_record = registry
                    .plugins
                    .values()
                    .find(|record| record.install_path == install_path);
                let kind = matched_record.map_or(PluginKind::External, |record| record.kind);
                let source = matched_record.map_or_else(
                    || install_path.display().to_string(),
                    |record| describe_install_source(&record.source),
                );
                if let Err(error) = validate_installed_plugin_path(secondary_root, &install_path) {
                    if let Some(record) = matched_record {
                        seen_ids.insert(record.id.clone());
                    }
                    seen_paths.insert(install_path.clone());
                    discovery.push_failure(PluginLoadFailure::new(install_path, kind, source, error));
                    continue;
                }
                if let Some(record) = matched_record {
                    if let Err(error) = verify_plugin_integrity(record, &install_path) {
                        seen_ids.insert(record.id.clone());
                        seen_paths.insert(install_path.clone());
                        discovery.push_failure(PluginLoadFailure::new(
                            install_path,
                            kind,
                            source,
                            error,
                        ));
                        continue;
                    }
                }
                match load_plugin_definition(&install_path, kind, source.clone(), kind.marketplace()) {
                    Ok(plugin) => {
                        if seen_ids.insert(plugin.metadata().id.clone()) {
                            seen_paths.insert(install_path);
                            discovery.push_plugin(plugin);
                        }
                    }
                    Err(error) => {
                        if let Some(record) = matched_record {
                            seen_ids.insert(record.id.clone());
                        }
                        seen_paths.insert(install_path.clone());
                        discovery.push_failure(PluginLoadFailure::new(
                            install_path,
                            kind,
                            source,
                            error,
                        ));
                    }
                }
            }

            for record in registry.plugins.values() {
                if seen_paths.contains(&record.install_path)
                    || !record.install_path.exists()
                    || plugin_manifest_path(&record.install_path).is_err()
                {
                    continue;
                }
                let source = describe_install_source(&record.source);
                if let Err(error) = validate_installed_plugin_path(secondary_root, &record.install_path)
                {
                    seen_ids.insert(record.id.clone());
                    discovery.push_failure(PluginLoadFailure::new(
                        record.install_path.clone(),
                        record.kind,
                        source,
                        error,
                    ));
                    continue;
                }
                if let Err(error) = verify_plugin_integrity(record, &record.install_path) {
                    seen_ids.insert(record.id.clone());
                    discovery.push_failure(PluginLoadFailure::new(
                        record.install_path.clone(),
                        record.kind,
                        source,
                        error,
                    ));
                    continue;
                }
                match load_plugin_definition(
                    &record.install_path,
                    record.kind,
                    source.clone(),
                    record.kind.marketplace(),
                ) {
                    Ok(plugin) => {
                        if seen_ids.insert(plugin.metadata().id.clone()) {
                            seen_paths.insert(record.install_path.clone());
                            discovery.push_plugin(plugin);
                        }
                    }
                    Err(error) => {
                        seen_ids.insert(record.id.clone());
                        discovery.push_failure(PluginLoadFailure::new(
                            record.install_path.clone(),
                            record.kind,
                            source,
                            error,
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn discover_external_directory_plugins_with_failures(
        &self,
        existing_plugins: &[PluginDefinition],
    ) -> Result<PluginDiscovery, PluginError> {
        let mut discovery = PluginDiscovery::default();

        for directory in &self.config.external_dirs {
            for root in discover_plugin_dirs(directory)? {
                let source = root.display().to_string();
                match load_plugin_definition(
                    &root,
                    PluginKind::External,
                    source.clone(),
                    EXTERNAL_MARKETPLACE,
                ) {
                    Ok(plugin) => {
                        if existing_plugins
                            .iter()
                            .chain(discovery.plugins.iter())
                            .all(|existing| existing.metadata().id != plugin.metadata().id)
                        {
                            discovery.push_plugin(plugin);
                        }
                    }
                    Err(error) => {
                        discovery.push_failure(PluginLoadFailure::new(
                            root,
                            PluginKind::External,
                            source,
                            error,
                        ));
                    }
                }
            }
        }

        Ok(discovery)
    }

    pub fn installed_plugin_registry_report(&self) -> Result<PluginRegistryReport, PluginError> {
        if let Some(registry) = self.cached_installed_registry() {
            return Ok(PluginRegistryReport::new(registry, Vec::new()));
        }

        // See `plugin_registry_report`: sync and scan share one critical section.
        let sync_guard = bundled_sync_guard();
        self.sync_bundled_plugins()?;
        let report = self.build_registry_report(self.discover_installed_plugins_with_failures()?);
        drop(sync_guard);
        if !report.has_failures() {
            self.store_cached_installed_registry(report.registry().clone());
        }
        Ok(report)
    }

    /// Mirror the shipped bundled plugins into the install root.
    ///
    /// Callers must already hold [`bundled_sync_guard`]; the guard spans the
    /// caller's subsequent scan, which is why this method does not take it.
    fn sync_bundled_plugins(&self) -> Result<(), PluginError> {
        let bundled_root = self
            .config
            .bundled_root
            .clone()
            .unwrap_or_else(Self::bundled_root);
        let bundled_plugins = discover_plugin_dirs(&bundled_root)?;
        let mut registry = self.load_registry()?;
        let mut changed = false;
        let install_root = self.install_root();
        let mut active_bundled_ids = BTreeSet::new();
        prune_stale_staging(&install_root);

        for source_root in bundled_plugins {
            let manifest = load_plugin_from_directory(&source_root)?;
            let plugin_id = plugin_id(&manifest.name, BUNDLED_MARKETPLACE);
            active_bundled_ids.insert(plugin_id.clone());
            let install_path = install_root.join(sanitize_plugin_id(&plugin_id));
            let now = unix_time_ms();
            let existing_record = registry.plugins.get(&plugin_id);
            let installed_copy_is_valid =
                install_path.exists() && load_plugin_from_directory(&install_path).is_ok();
            let needs_sync = existing_record.is_none_or(|record| {
                record.kind != PluginKind::Bundled
                    || record.version != manifest.version
                    || record.name != manifest.name
                    || record.description != manifest.description
                    || record.install_path != install_path
                    || !record.install_path.exists()
                    || !installed_copy_is_valid
            });

            if !needs_sync {
                continue;
            }

            publish_plugin_tree(&install_root, &source_root, &install_path)?;

            let installed_at_unix_ms =
                existing_record.map_or(now, |record| record.installed_at_unix_ms);
            registry.plugins.insert(
                plugin_id.clone(),
                InstalledPluginRecord {
                    kind: PluginKind::Bundled,
                    id: plugin_id,
                    name: manifest.name,
                    version: manifest.version,
                    description: manifest.description,
                    install_path,
                    source: PluginInstallSource::LocalPath { path: source_root },
                    installed_at_unix_ms,
                    updated_at_unix_ms: now,
                    // Bundled plugins ship with the binary; their integrity is
                    // governed by the binary, not by a per-install digest.
                    resolved_commit: None,
                    content_sha256: None,
                },
            );
            changed = true;
        }

        let stale_bundled_ids = registry
            .plugins
            .iter()
            .filter_map(|(plugin_id, record)| {
                (record.kind == PluginKind::Bundled && !active_bundled_ids.contains(plugin_id))
                    .then_some(plugin_id.clone())
            })
            .collect::<Vec<_>>();

        for plugin_id in stale_bundled_ids {
            if let Some(record) = registry.plugins.remove(&plugin_id) {
                if record.install_path.exists() {
                    fs::remove_dir_all(&record.install_path)?;
                }
                changed = true;
            }
        }

        if changed {
            self.store_registry(&registry)?;
        }

        Ok(())
    }

    fn is_enabled(&self, metadata: &PluginMetadata) -> bool {
        self.config
            .enabled_plugins
            .get(&metadata.id)
            .copied()
            .unwrap_or(match metadata.kind {
                PluginKind::External => false,
                PluginKind::Builtin | PluginKind::Bundled => metadata.default_enabled,
            })
    }

    fn ensure_known_plugin(&self, plugin_id: &str) -> Result<(), PluginError> {
        if self.plugin_registry()?.contains(plugin_id) {
            Ok(())
        } else {
            Err(PluginError::NotFound(format!(
                "plugin `{plugin_id}` is not installed or discoverable"
            )))
        }
    }

    pub(crate) fn load_registry(&self) -> Result<InstalledPluginRegistry, PluginError> {
        load_registry_at(&self.registry_path())
    }

    pub(crate) fn store_registry(
        &self,
        registry: &InstalledPluginRegistry,
    ) -> Result<(), PluginError> {
        let path = self.registry_path();
        write_atomic(&path, serde_json::to_string_pretty(registry)?.as_bytes())?;
        Ok(())
    }

    pub(crate) fn write_enabled_state(
        &self,
        plugin_id: &str,
        enabled: Option<bool>,
    ) -> Result<(), PluginError> {
        update_settings_json(&self.settings_path(), |root| {
            let enabled_plugins = ensure_object(root, "enabledPlugins");
            match enabled {
                Some(value) => {
                    enabled_plugins.insert(plugin_id.to_string(), Value::Bool(value));
                }
                None => {
                    enabled_plugins.remove(plugin_id);
                }
            }
        })
    }

    fn installed_plugin_registry(&self) -> Result<PluginRegistry, PluginError> {
        if let Some(registry) = self.cached_installed_registry() {
            return Ok(registry);
        }

        let registry = self.installed_plugin_registry_report()?.into_registry()?;
        self.store_cached_installed_registry(registry.clone());
        Ok(registry)
    }

    fn build_registry_report(&self, discovery: PluginDiscovery) -> PluginRegistryReport {
        PluginRegistryReport::new(
            PluginRegistry::new(
                discovery
                    .plugins
                    .into_iter()
                    .map(|plugin| {
                        let enabled = self.is_enabled(plugin.metadata());
                        RegisteredPlugin::new(plugin, enabled)
                    })
                    .collect(),
            ),
            discovery.failures,
        )
    }

    fn cached_registry(&self) -> Option<PluginRegistry> {
        self.cache
            .lock()
            // Poison policy: recover — the cache is two independent
            // Option<PluginRegistry> memo fields (single-value writes); a
            // poisoned holder leaves at worst a stale/None entry that the
            // next load recomputes. (plugins deliberately has no api dep,
            // so the shared lock_recovered helper is out of reach here.)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .registry
            .clone()
    }

    fn store_cached_registry(&self, registry: PluginRegistry) {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .registry = Some(registry);
    }

    fn cached_installed_registry(&self) -> Option<PluginRegistry> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .installed_registry
            .clone()
    }

    fn store_cached_installed_registry(&self, registry: PluginRegistry) {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .installed_registry = Some(registry);
    }

    fn invalidate_cache(&self) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.registry = None;
        cache.installed_registry = None;
    }
}

fn load_registry_at(path: &Path) -> Result<InstalledPluginRegistry, PluginError> {
    match fs::read_to_string(path) {
        Ok(contents) if contents.trim().is_empty() => Ok(InstalledPluginRegistry::default()),
        Ok(contents) => Ok(serde_json::from_str(&contents)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(InstalledPluginRegistry::default())
        }
        Err(error) => Err(PluginError::Io(error)),
    }
}

fn secondary_registry_path(install_root: &Path) -> PathBuf {
    install_root
        .parent()
        .map_or_else(|| install_root.join(REGISTRY_FILE_NAME), |parent| {
            parent.join(REGISTRY_FILE_NAME)
        })
}

fn validate_installed_plugin_path(install_root: &Path, install_path: &Path) -> Result<(), PluginError> {
    let metadata = fs::symlink_metadata(install_path)?;
    if metadata.file_type().is_symlink() {
        return Err(PluginError::InvalidManifest(format!(
            "installed plugin directory `{}` must not be a symlink",
            install_path.display(),
        )));
    }

    let canonical_root = install_root.canonicalize().map_err(|error| {
        PluginError::InvalidManifest(format!(
            "configured install root `{}` could not be resolved for containment check: {error}",
            install_root.display(),
        ))
    })?;
    let canonical_path = install_path.canonicalize().map_err(|error| {
        PluginError::InvalidManifest(format!(
            "installed plugin directory `{}` could not be resolved for containment check: {error}",
            install_path.display(),
        ))
    })?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err(PluginError::InvalidManifest(format!(
            "installed plugin directory `{}` resolves outside configured install root `{}`",
            install_path.display(),
            install_root.display(),
        )));
    }
    Ok(())
}

fn staging_root(install_root: &Path) -> PathBuf {
    install_root.join(STAGING_DIR_NAME)
}

/// Reserve a fresh scratch base name for one swap. The plugin directory name
/// keeps it readable, the PID keeps it unique across concurrent sessions, and the
/// counter keeps it unique within this process.
fn staging_slot(install_root: &Path, install_path: &Path) -> PathBuf {
    let label = install_path.file_name().map_or_else(
        || "plugin".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let process = std::process::id();
    staging_root(install_root).join(format!("{label}.{process}.{sequence}"))
}

/// Drop staging trees abandoned by a crashed run. Anything young enough to be a
/// peer's in-flight copy is left alone; failures are ignored because a losing
/// race against another pruner is not an error worth failing discovery for.
fn prune_stale_staging(install_root: &Path) {
    prune_stale_staging_older_than(install_root, STALE_STAGING_AGE);
}

fn prune_stale_staging_older_than(install_root: &Path, age_limit: Duration) {
    let Ok(entries) = fs::read_dir(staging_root(install_root)) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                now.duration_since(modified)
                    .is_ok_and(|age| age > age_limit)
            });
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Append a literal suffix to a path's final component. `Path::with_extension`
/// would instead replace the counter that makes a staging slot unique.
fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Publish `source_root` at `install_path` without ever exposing a partially
/// populated tree.
///
/// The old code removed `install_path` and then copied file-by-file into it, so
/// any concurrent reader that scanned during the copy saw a directory holding a
/// `plugin.json` but not yet the hooks it declares — reported as a `MissingPath`
/// load failure. Here the copy lands in a sibling staging directory and is
/// published with `rename`, which is atomic on APFS: a reader sees either the
/// complete old tree or the complete new one.
///
/// `rename` cannot replace a non-empty directory (`ENOTEMPTY`), so an existing
/// tree is first swung aside into staging. That leaves a gap of exactly two
/// syscalls (rather than a whole recursive copy) in which `install_path` is
/// absent — and an absent directory is skipped by discovery rather than reported
/// as a broken plugin. In-process readers never observe even that gap, since
/// every scan path holds [`bundled_sync_guard`].
fn publish_plugin_tree(
    install_root: &Path,
    source_root: &Path,
    install_path: &Path,
) -> Result<(), PluginError> {
    let slot = staging_slot(install_root, install_path);
    let staged = append_suffix(&slot, ".staged");
    let displaced = append_suffix(&slot, ".displaced");
    fs::create_dir_all(staging_root(install_root))?;
    if staged.exists() {
        fs::remove_dir_all(&staged)?;
    }
    copy_dir_all(source_root, &staged)?;

    if install_path.exists() {
        if let Err(error) = fs::rename(install_path, &displaced) {
            let _ = fs::remove_dir_all(&staged);
            return Err(PluginError::Io(error));
        }
    }

    if let Err(error) = fs::rename(&staged, install_path) {
        let _ = fs::remove_dir_all(&staged);
        // Never leave the plugin missing: put the displaced tree back unless a
        // peer already published its own (identical) copy into the slot.
        if !install_path.exists() && displaced.exists() {
            let _ = fs::rename(&displaced, install_path);
        }
        let _ = fs::remove_dir_all(&displaced);
        // A peer that won the race published the same bundled content, so the
        // post-condition holds and this is success, not a sync failure.
        if load_plugin_from_directory(install_path).is_ok() {
            return Ok(());
        }
        return Err(PluginError::Io(error));
    }

    let _ = fs::remove_dir_all(&displaced);
    Ok(())
}

/// Verify that an installed plugin's on-disk tree still matches the SHA-256
/// recorded at install time. Records without a stored digest (installed before
/// supply-chain checks existed) pass unconditionally.
fn verify_plugin_integrity(
    record: &InstalledPluginRecord,
    install_path: &Path,
) -> Result<(), PluginError> {
    let Some(expected) = record.content_sha256.as_deref() else {
        return Ok(());
    };
    let actual = hash_plugin_tree(install_path)?;
    if actual == expected {
        Ok(())
    } else {
        Err(PluginError::IntegrityMismatch(format!(
            "plugin `{}` failed its integrity check: contents at {} no longer match the digest recorded at install time",
            record.id,
            install_path.display(),
        )))
    }
}

#[cfg(test)]
mod bundled_sync_tests {
    use std::sync::atomic::AtomicBool;
    use std::thread;
    use std::time::Instant;

    use super::*;

    /// Wide enough that a byte-wise `remove_dir_all` + `copy_dir_all` publish
    /// leaves an observable half-populated directory for milliseconds — the
    /// regression this module pins. A rename-based publish is unaffected by the
    /// count.
    const TREE_FILE_COUNT: usize = 400;
    const SWAP_ROUNDS: usize = 6;
    /// Longest a legitimate "directory momentarily absent" window can last: the
    /// two renames of a publish. Retried observations ride it out.
    const SETTLE_ATTEMPTS: usize = 6;
    const SETTLE_PAUSE: Duration = Duration::from_millis(1);

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("plugins-manager-{label}-{nanos}-{sequence}"))
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("staging parent should be creatable");
        }
        fs::write(path, contents).expect("test file should be writable");
    }

    fn write_bundled_source(root: &Path, name: &str) {
        write_file(
            &root.join("hooks").join("pre.sh"),
            "#!/bin/sh\nprintf 'pre'\n",
        );
        write_file(
            &root.join("plugin.json"),
            &format!(
                "{{\"name\":\"{name}\",\"version\":\"1.0.0\",\"description\":\"bundled swap test\",\
                 \"defaultEnabled\":false,\"hooks\":{{\"PreToolUse\":[\"./hooks/pre.sh\"]}}}}"
            ),
        );
    }

    fn write_wide_source(root: &Path) {
        write_file(
            &root.join("plugin.json"),
            "{\"name\":\"swap-demo\",\"version\":\"1.0.0\",\"description\":\"swap\"}",
        );
        for index in 0..TREE_FILE_COUNT {
            write_file(&root.join("files").join(format!("part-{index:04}.txt")), "x");
        }
    }

    fn staging_entries(install_root: &Path) -> Vec<PathBuf> {
        fs::read_dir(staging_root(install_root))
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default()
    }

    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    enum Observation {
        /// Publishing swings the old tree aside before renaming the new one in,
        /// so the slot is legitimately empty for the width of two syscalls.
        Absent,
        /// Present but not fully populated — the state that made a concurrent
        /// reader report a bundled plugin's hook as missing.
        Partial,
        Complete,
    }

    fn observe(install_path: &Path) -> Observation {
        let Ok(entries) = fs::read_dir(install_path.join("files")) else {
            return Observation::Absent;
        };
        let count = entries.flatten().count();
        if count == TREE_FILE_COUNT && install_path.join("plugin.json").exists() {
            Observation::Complete
        } else {
            Observation::Partial
        }
    }

    /// A single stat cannot tell the two-rename gap from a genuine partial copy,
    /// so a `Partial` reading is re-checked across a window far longer than the
    /// gap and far shorter than a recursive copy of the whole tree.
    fn settled_observation(install_path: &Path) -> Observation {
        let mut observation = observe(install_path);
        for _ in 0..SETTLE_ATTEMPTS {
            if observation != Observation::Partial {
                return observation;
            }
            thread::sleep(SETTLE_PAUSE);
            observation = observe(install_path);
        }
        observation
    }

    /// The core guarantee: while a publish is in flight, a reader that knows
    /// nothing about the manager's lock never sees `install_path` populated with
    /// only part of the tree.
    #[test]
    fn publishing_a_tree_never_exposes_a_partial_directory() {
        let root = scratch("swap-atomicity");
        let install_root = root.join("installed");
        let source = root.join("source");
        write_wide_source(&source);
        fs::create_dir_all(&install_root).expect("install root should be creatable");
        let install_path = install_root.join("swap-demo-bundled");

        let stop = Arc::new(AtomicBool::new(false));
        let partial_views = Arc::new(AtomicU64::new(0));
        let complete_views = Arc::new(AtomicU64::new(0));
        let reader = thread::spawn({
            let stop = Arc::clone(&stop);
            let partial_views = Arc::clone(&partial_views);
            let complete_views = Arc::clone(&complete_views);
            let install_path = install_path.clone();
            move || {
                while !stop.load(Ordering::Relaxed) {
                    match settled_observation(&install_path) {
                        Observation::Partial => {
                            partial_views.fetch_add(1, Ordering::Relaxed);
                        }
                        Observation::Complete => {
                            complete_views.fetch_add(1, Ordering::Relaxed);
                        }
                        Observation::Absent => {}
                    }
                }
            }
        });

        let started = Instant::now();
        for _ in 0..SWAP_ROUNDS {
            publish_plugin_tree(&install_root, &source, &install_path).expect("publish should work");
        }
        let elapsed = started.elapsed();
        stop.store(true, Ordering::Relaxed);
        reader.join().expect("reader thread should not panic");

        assert_eq!(
            partial_views.load(Ordering::Relaxed),
            0,
            "a concurrent reader observed a half-populated install path (publish is not atomic)",
        );
        assert!(
            complete_views.load(Ordering::Relaxed) > 0,
            "the reader never saw the tree at all — the atomicity check was vacuous",
        );
        assert_eq!(
            observe(&install_path),
            Observation::Complete,
            "the final tree must hold every source file",
        );
        assert!(
            staging_entries(&install_root).is_empty(),
            "publishing must leave no staging or displaced directories behind",
        );
        // Guards the calibration above: the settle window must stay well below
        // the cost of a whole-tree copy, or a real partial copy would be waited
        // out instead of reported.
        let copy_cost = elapsed / u32::try_from(SWAP_ROUNDS).expect("round count fits in u32");
        assert!(
            copy_cost > SETTLE_PAUSE,
            "tree is too small to distinguish a partial copy from the rename gap: {copy_cost:?}",
        );

        let _ = fs::remove_dir_all(root);
    }

    /// Staging left behind by a crashed run is reclaimed, while a peer's
    /// in-flight copy (young) is never touched.
    #[test]
    fn stale_staging_is_pruned_and_in_flight_staging_is_kept() {
        let install_root = scratch("staging-prune");
        let stale = staging_root(&install_root).join("crashed.1.0.staged");
        write_file(&stale.join("plugin.json"), "{}");
        fs::create_dir_all(&install_root).expect("install root should be creatable");

        prune_stale_staging(&install_root);
        assert!(
            stale.exists(),
            "a staging directory younger than the stale bound belongs to a live peer",
        );

        thread::sleep(Duration::from_millis(5));
        prune_stale_staging_older_than(&install_root, Duration::from_millis(1));
        assert!(
            !stale.exists(),
            "staging abandoned by a crashed run must be reclaimed",
        );

        let _ = fs::remove_dir_all(install_root);
    }

    /// End-to-end: bundled sync installs a complete tree, re-syncs over an
    /// existing (damaged) copy, and leaves no scratch directories — the swap is
    /// invisible to everything downstream.
    #[test]
    fn bundled_sync_republishes_a_damaged_copy_and_cleans_staging() {
        let config_home = scratch("bundled-swap-home");
        let bundled_root = scratch("bundled-swap-source");
        write_bundled_source(&bundled_root.join("starter"), "swap-starter");

        let mut config = PluginManagerConfig::new(&config_home);
        config.bundled_root = Some(bundled_root.clone());
        let manager = PluginManager::new(config.clone());
        let installed = manager
            .list_installed_plugins()
            .expect("bundled plugins should install");
        assert!(installed
            .iter()
            .any(|plugin| plugin.metadata.id == "swap-starter@bundled"));

        let install_root = manager.install_root();
        let install_path = install_root.join("swap-starter-bundled");
        let hook = install_path.join("hooks").join("pre.sh");
        assert!(hook.exists(), "the installed copy must carry its hook");
        assert!(
            staging_entries(&install_root).is_empty(),
            "the first publish must clean up after itself",
        );

        // Simulate the exact damage the racy publish used to expose: a manifest
        // with its hook missing. A fresh manager (cold cache) must republish.
        fs::remove_file(&hook).expect("hook should be removable");
        let report = PluginManager::new(config)
            .installed_plugin_registry_report()
            .expect("re-sync should succeed");
        assert!(
            !report.has_failures(),
            "re-sync must restore the tree instead of reporting a missing hook",
        );
        assert!(hook.exists(), "the hook must be restored by the re-sync");
        assert!(
            staging_entries(&install_root).is_empty(),
            "the displacing publish must clean up its staging and old tree",
        );

        let _ = fs::remove_dir_all(config_home);
        let _ = fs::remove_dir_all(bundled_root);
    }
}
