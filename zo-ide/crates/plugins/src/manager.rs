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
use super::published::{self, PublishLock, PUBLISH_LOCK_WAIT};
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

/// Scratch directory where zo binaries from before t-76269 staged bundled
/// copies and the trees they displaced. Nothing writes here any more; what an
/// older binary (or a crash of one) leaves is still pruned.
const STAGING_DIR_NAME: &str = ".staging";

/// A staging entry older than this cannot belong to a live swap (a swap is a
/// recursive copy of a handful of small files), so it is a crash leftover and is
/// pruned. The bound is deliberately generous: deleting a peer's in-flight copy
/// would be far worse than leaving a stale directory around for one more run.
const STALE_STAGING_AGE: Duration = Duration::from_secs(600);

/// Serialises bundled sync against every other bundled sync *and* against the
/// installed-plugin scans in this process, so a scan never runs while this
/// process is publishing. Other processes are kept apart by
/// [`PublishLock`], and a reader anywhere only ever reads a tree that is
/// complete and pinned (see the `published` module).
static BUNDLED_SYNC_LOCK: Mutex<()> = Mutex::new(());

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
        let retired = published::retired_trees(&primary_install_root);
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
            // Records and seen paths speak of the logical path; the tree read
            // is whatever it points at.
            let tree = published::hold_installed_tree(&install_path, kind == PluginKind::Bundled);
            if tree == install_path && retired.contains(&install_path) {
                // A bundled plugin that left the bundle while a reader still
                // held it: it waits for removal and is never loaded again.
                seen_paths.insert(install_path);
                continue;
            }
            if let Err(error) = validate_installed_plugin_path(&primary_install_root, &tree) {
                if let Some(record) = matched_record {
                    // A registered higher-priority plugin reserves its ID when
                    // validation fails, so a lower-root copy cannot take over.
                    seen_ids.insert(record.id.clone());
                }
                seen_paths.insert(install_path);
                discovery.push_failure(PluginLoadFailure::new(tree, kind, source, error));
                continue;
            }
            // Supply-chain guard: reject a tampered on-disk copy before loading.
            if let Some(record) = matched_record {
                if let Err(error) = verify_plugin_integrity(record, &tree) {
                    // A registered higher-priority plugin reserves its ID even
                    // when its contents fail integrity verification.
                    seen_ids.insert(record.id.clone());
                    // Mark the path seen so the registry-fallback loop below
                    // does not re-load the very tree we just rejected.
                    seen_paths.insert(install_path);
                    discovery.push_failure(PluginLoadFailure::new(tree, kind, source, error));
                    continue;
                }
            }
            match load_plugin_definition(&tree, kind, source.clone(), kind.marketplace()) {
                Ok(plugin) => {
                    if seen_ids.insert(plugin.metadata().id.clone()) {
                        seen_paths.insert(install_path);
                        discovery.push_plugin(plugin);
                    }
                }
                Err(error) => {
                    seen_paths.insert(install_path);
                    discovery.push_failure(PluginLoadFailure::new(tree, kind, source, error));
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
            let tree = published::hold_installed_tree(
                &record.install_path,
                record.kind == PluginKind::Bundled,
            );
            if !tree.exists() || plugin_manifest_path(&tree).is_err() {
                stale_registry_ids.push(record.id.clone());
                continue;
            }
            let source = describe_install_source(&record.source);
            if let Err(error) = validate_installed_plugin_path(&primary_install_root, &tree) {
                seen_ids.insert(record.id.clone());
                discovery.push_failure(PluginLoadFailure::new(tree, record.kind, source, error));
                continue;
            }
            if let Err(error) = verify_plugin_integrity(record, &tree) {
                seen_ids.insert(record.id.clone());
                discovery.push_failure(PluginLoadFailure::new(tree, record.kind, source, error));
                continue;
            }
            match load_plugin_definition(&tree, record.kind, source.clone(), record.kind.marketplace())
            {
                Ok(plugin) => {
                    if seen_ids.insert(plugin.metadata().id.clone()) {
                        seen_paths.insert(record.install_path.clone());
                        discovery.push_plugin(plugin);
                    }
                }
                Err(error) => {
                    discovery.push_failure(PluginLoadFailure::new(tree, record.kind, source, error));
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
            let retired = published::retired_trees(secondary_root);
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
                let tree =
                    published::hold_installed_tree(&install_path, kind == PluginKind::Bundled);
                if tree == install_path && retired.contains(&install_path) {
                    seen_paths.insert(install_path);
                    continue;
                }
                if let Err(error) = validate_installed_plugin_path(secondary_root, &tree) {
                    if let Some(record) = matched_record {
                        seen_ids.insert(record.id.clone());
                    }
                    seen_paths.insert(install_path);
                    discovery.push_failure(PluginLoadFailure::new(tree, kind, source, error));
                    continue;
                }
                if let Some(record) = matched_record {
                    if let Err(error) = verify_plugin_integrity(record, &tree) {
                        seen_ids.insert(record.id.clone());
                        seen_paths.insert(install_path);
                        discovery.push_failure(PluginLoadFailure::new(tree, kind, source, error));
                        continue;
                    }
                }
                match load_plugin_definition(&tree, kind, source.clone(), kind.marketplace()) {
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
                        seen_paths.insert(install_path);
                        discovery.push_failure(PluginLoadFailure::new(tree, kind, source, error));
                    }
                }
            }

            for record in registry.plugins.values() {
                if seen_paths.contains(&record.install_path) {
                    continue;
                }
                let tree = published::hold_installed_tree(
                    &record.install_path,
                    record.kind == PluginKind::Bundled,
                );
                if !tree.exists() || plugin_manifest_path(&tree).is_err() {
                    continue;
                }
                let source = describe_install_source(&record.source);
                if let Err(error) = validate_installed_plugin_path(secondary_root, &tree) {
                    seen_ids.insert(record.id.clone());
                    discovery.push_failure(PluginLoadFailure::new(tree, record.kind, source, error));
                    continue;
                }
                if let Err(error) = verify_plugin_integrity(record, &tree) {
                    seen_ids.insert(record.id.clone());
                    discovery.push_failure(PluginLoadFailure::new(tree, record.kind, source, error));
                    continue;
                }
                match load_plugin_definition(
                    &tree,
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
                            tree,
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
        self.sync_bundled_plugins_within(PUBLISH_LOCK_WAIT)?;
        Ok(())
    }

    /// [`Self::sync_bundled_plugins`], waiting at most `wait` for another
    /// process's publish.
    ///
    /// Every publish, record change and removal happens under the install
    /// root's [`PublishLock`]. A sync with nothing to publish does not wait
    /// for it (removals can wait for a later sync); one that cannot get it in
    /// time publishes nothing, logs that, and the trees already published are
    /// read as they are.
    fn sync_bundled_plugins_within(&self, wait: Duration) -> Result<BundledSync, PluginError> {
        let bundled_root = self
            .config
            .bundled_root
            .clone()
            .unwrap_or_else(Self::bundled_root);
        let install_root = self.install_root();
        prune_stale_staging(&install_root);
        let mut bundled = Vec::new();
        for source_root in discover_plugin_dirs(&bundled_root)? {
            let manifest = load_plugin_from_directory(&source_root)?;
            let plugin_id = plugin_id(&manifest.name, BUNDLED_MARKETPLACE);
            let install_path = install_root.join(sanitize_plugin_id(&plugin_id));
            bundled.push(BundledSource {
                source_root,
                manifest,
                plugin_id,
                install_path,
            });
        }
        let active_bundled_ids = bundled
            .iter()
            .map(|source| source.plugin_id.clone())
            .collect::<BTreeSet<_>>();
        let left_the_bundle = |registry: &InstalledPluginRegistry| {
            registry
                .plugins
                .iter()
                .filter_map(|(plugin_id, record)| {
                    (record.kind == PluginKind::Bundled && !active_bundled_ids.contains(plugin_id))
                        .then_some(plugin_id.clone())
                })
                .collect::<Vec<_>>()
        };

        let registry = self.load_registry()?;
        let publish_due = !left_the_bundle(&registry).is_empty()
            || bundled
                .iter()
                .any(|source| source.copy_is_stale(registry.plugins.get(&source.plugin_id)));
        if !publish_due && !published::retired_work_pending(&install_root) {
            return Ok(BundledSync::Done);
        }
        let wait = if publish_due { wait } else { Duration::ZERO };
        let Some(lock) = PublishLock::acquire(&install_root, wait)? else {
            if !publish_due {
                return Ok(BundledSync::Done);
            }
            let notice = format!(
                "[zo] plugins: bundled plugins were not published this time: another process held {} for {} ms; the trees already published stay in use",
                published::lock_path(&install_root).display(),
                wait.as_millis(),
            );
            eprintln!("{notice}");
            return Ok(BundledSync::SkippedLockBusy { notice });
        };

        // Read again under the lock: a peer may have published while this waited.
        let mut registry = self.load_registry()?;
        let mut changed = false;
        for source in bundled {
            let existing_record = registry.plugins.get(&source.plugin_id);
            if !source.copy_is_stale(existing_record) {
                continue;
            }
            published::publish_version(&lock, &source.source_root, &source.install_path)?;
            let now = unix_time_ms();
            let installed_at_unix_ms =
                existing_record.map_or(now, |record| record.installed_at_unix_ms);
            registry.plugins.insert(
                source.plugin_id.clone(),
                InstalledPluginRecord {
                    kind: PluginKind::Bundled,
                    id: source.plugin_id,
                    name: source.manifest.name,
                    version: source.manifest.version,
                    description: source.manifest.description,
                    // The logical path: readers follow its pointer to the tree.
                    install_path: source.install_path,
                    source: PluginInstallSource::LocalPath {
                        path: source.source_root,
                    },
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

        for plugin_id in left_the_bundle(&registry) {
            if let Some(record) = registry.plugins.remove(&plugin_id) {
                published::retire_removed_plugin(&lock, &install_root, &record.install_path);
                changed = true;
            }
        }

        if changed {
            self.store_registry(&registry)?;
        }
        published::sweep_retired(&lock, &install_root);
        Ok(BundledSync::Done)
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

/// What one bundled sync did.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BundledSync {
    /// Everything due was published and every removable tree removed.
    Done,
    /// Another process held the publish lock past the wait, so nothing was
    /// published; `notice` is the line that was logged.
    SkippedLockBusy { notice: String },
}

/// One plugin shipped in the bundle, and the logical path it installs at.
struct BundledSource {
    source_root: PathBuf,
    manifest: PluginManifest,
    plugin_id: String,
    install_path: PathBuf,
}

impl BundledSource {
    /// Whether the installed copy must be published again: no complete tree
    /// behind the logical path, or a record that does not describe this
    /// bundle's plugin at that path.
    fn copy_is_stale(&self, record: Option<&InstalledPluginRecord>) -> bool {
        let tree = published::installed_tree(&self.install_path);
        let complete = tree.is_dir() && load_plugin_from_directory(&tree).is_ok();
        !complete
            || record.is_none_or(|record| {
                record.kind != PluginKind::Bundled
                    || record.version != self.manifest.version
                    || record.name != self.manifest.name
                    || record.description != self.manifest.description
                    || record.install_path != self.install_path
            })
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
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread;

    use super::*;

    /// Wide enough that a byte-wise `remove_dir_all` + `copy_dir_all` publish
    /// leaves an observable half-populated directory for milliseconds — the
    /// regression this module pins. A publish into a fresh folder is
    /// unaffected by the count.
    const TREE_FILE_COUNT: usize = 400;
    const SWAP_ROUNDS: usize = 6;

    static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("plugins-manager-{label}-{nanos}-{sequence}"))
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("staging parent should be creatable");
        }
        fs::write(path, contents).expect("test file should be writable");
    }

    fn write_bundled_source(root: &Path, name: &str) {
        write_bundled_source_at(root, name, "1.0.0");
    }

    fn write_bundled_source_at(root: &Path, name: &str, version: &str) {
        write_file(
            &root.join("hooks").join("pre.sh"),
            "#!/bin/sh\nprintf 'pre'\n",
        );
        write_file(
            &root.join("plugin.json"),
            &format!(
                "{{\"name\":\"{name}\",\"version\":\"{version}\",\"description\":\"bundled swap test\",\
                 \"defaultEnabled\":false,\"hooks\":{{\"PreToolUse\":[\"./hooks/pre.sh\"]}}}}"
            ),
        );
    }

    /// The tree a fresh manager loads for `plugin_id`, as its hooks will run it.
    fn loaded_tree(config: &PluginManagerConfig, plugin_id: &str) -> (PathBuf, String) {
        let summary = PluginManager::new(config.clone())
            .list_installed_plugins()
            .expect("installed plugins should load")
            .into_iter()
            .find(|plugin| plugin.metadata.id == plugin_id)
            .expect("the bundled plugin should be installed");
        (
            summary
                .metadata
                .root
                .expect("an installed plugin carries the tree it was loaded from"),
            summary.metadata.version,
        )
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
        /// Nothing at the tree the logical path names. A publish into a fresh
        /// folder has no gap, so after the first publish this is a failure.
        Absent,
        /// Present but not fully populated — the state that made a concurrent
        /// reader report a bundled plugin's hook as missing.
        Partial,
        Complete,
    }

    fn observe(tree: &Path) -> Observation {
        let Ok(entries) = fs::read_dir(tree.join("files")) else {
            return Observation::Absent;
        };
        let count = entries.flatten().count();
        if count == TREE_FILE_COUNT && tree.join("plugin.json").exists() {
            Observation::Complete
        } else {
            Observation::Partial
        }
    }

    fn publish(install_root: &Path, source: &Path, install_path: &Path) {
        let lock = PublishLock::acquire(install_root, PUBLISH_LOCK_WAIT)
            .expect("lock file should open")
            .expect("nobody else holds the publish lock");
        published::publish_version(&lock, source, install_path).expect("publish should work");
        published::sweep_retired(&lock, install_root);
    }

    /// The core guarantee: while publishes are in flight, a reader that knows
    /// nothing about the manager's locks and follows the logical path the way
    /// discovery does only ever sees a complete tree — never a partial one,
    /// and never none at all.
    #[test]
    fn publishing_a_tree_never_exposes_a_partial_directory() {
        let root = scratch("swap-atomicity");
        let install_root = root.join("installed");
        let source = root.join("source");
        write_wide_source(&source);
        fs::create_dir_all(&install_root).expect("install root should be creatable");
        let install_path = install_root.join("swap-demo-bundled");
        publish(&install_root, &source, &install_path);

        let stop = Arc::new(AtomicBool::new(false));
        let partial_views = Arc::new(AtomicU64::new(0));
        let absent_views = Arc::new(AtomicU64::new(0));
        let complete_views = Arc::new(AtomicU64::new(0));
        let reader = thread::spawn({
            let stop = Arc::clone(&stop);
            let partial_views = Arc::clone(&partial_views);
            let absent_views = Arc::clone(&absent_views);
            let complete_views = Arc::clone(&complete_views);
            let install_path = install_path.clone();
            move || {
                while !stop.load(Ordering::Relaxed) {
                    let tree = published::hold_installed_tree(&install_path, true);
                    let counter = match observe(&tree) {
                        Observation::Partial => &partial_views,
                        Observation::Absent => &absent_views,
                        Observation::Complete => &complete_views,
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        for _ in 0..SWAP_ROUNDS {
            publish(&install_root, &source, &install_path);
        }
        stop.store(true, Ordering::Relaxed);
        reader.join().expect("reader thread should not panic");

        assert_eq!(
            partial_views.load(Ordering::Relaxed),
            0,
            "a concurrent reader observed a half-populated tree (publish is not atomic)",
        );
        assert_eq!(
            absent_views.load(Ordering::Relaxed),
            0,
            "a concurrent reader found no tree behind the install path mid-publish",
        );
        assert!(
            complete_views.load(Ordering::Relaxed) > 0,
            "the reader never saw the tree at all — the atomicity check was vacuous",
        );
        assert_eq!(
            observe(&published::installed_tree(&install_path)),
            Observation::Complete,
            "the final tree must hold every source file",
        );
        assert!(
            !install_path.exists(),
            "the logical path itself is never created, so it is never renamed",
        );
        assert!(
            staging_entries(&install_root).is_empty(),
            "publishing must leave no staging or displaced directories behind",
        );

        let _ = fs::remove_dir_all(root);
    }

    /// t-76269: a publish never moves the tree a reader is already in. Windows
    /// refuses to rename a directory while a file inside it is open (os error
    /// 5), and macOS lets that rename through, so this checks the cause itself:
    /// after a newer bundled version is published, the tree an earlier load
    /// handed out is still at its path with what the reader left in it, and the
    /// newer tree is complete at a path of its own.
    #[test]
    fn a_publish_leaves_the_tree_a_reader_is_in_where_it_was() {
        let config_home = scratch("reader-tree-home");
        let bundled_root = scratch("reader-tree-source");
        let source = bundled_root.join("starter");
        write_bundled_source_at(&source, "reader-starter", "1.0.0");
        let mut config = PluginManagerConfig::new(&config_home);
        config.bundled_root = Some(bundled_root.clone());

        let (first_tree, first_version) = loaded_tree(&config, "reader-starter@bundled");
        assert_eq!(first_version, "1.0.0");
        // What a reader holds open: a file inside the tree it was handed.
        let held = first_tree.join("reader-was-here");
        write_file(&held, "open");

        write_bundled_source_at(&source, "reader-starter", "1.0.1");
        let (second_tree, second_version) = loaded_tree(&config, "reader-starter@bundled");

        assert!(
            held.exists(),
            "publishing 1.0.1 moved or replaced the 1.0.0 tree a reader was in ({}); \
             Windows refuses that rename while a file inside is open",
            first_tree.display(),
        );
        assert!(
            first_tree.join("hooks").join("pre.sh").exists(),
            "the tree a reader was in must stay complete",
        );
        assert_eq!(second_version, "1.0.1");
        assert_ne!(
            second_tree, first_tree,
            "the newer version must be published at a path no reader holds",
        );
        assert!(
            second_tree.join("hooks").join("pre.sh").exists(),
            "the newer tree must be complete",
        );
        assert!(
            !second_tree.join("reader-was-here").exists(),
            "the newer tree is a fresh copy, not the reader's tree",
        );

        let _ = fs::remove_dir_all(config_home);
        let _ = fs::remove_dir_all(bundled_root);
    }

    /// A sync that has to publish but cannot get the publish lock in time
    /// publishes nothing, logs that it did not, and the tree already current
    /// stays in use; the next sync after the peer lets go publishes.
    #[test]
    fn a_sync_that_cannot_get_the_publish_lock_says_so_and_keeps_the_current_tree() {
        let config_home = scratch("busy-lock-home");
        let bundled_root = scratch("busy-lock-source");
        let source = bundled_root.join("starter");
        write_bundled_source_at(&source, "busy-starter", "1.0.0");
        let mut config = PluginManagerConfig::new(&config_home);
        config.bundled_root = Some(bundled_root.clone());
        let (first_tree, _) = loaded_tree(&config, "busy-starter@bundled");

        write_bundled_source_at(&source, "busy-starter", "1.0.1");
        let manager = PluginManager::new(config.clone());
        let install_root = manager.install_root();
        let peer = PublishLock::acquire(&install_root, Duration::ZERO)
            .expect("lock file should open")
            .expect("nobody else holds the publish lock");
        let outcome = {
            let _guard = bundled_sync_guard();
            manager
                .sync_bundled_plugins_within(Duration::from_millis(50))
                .expect("a busy lock is not a sync failure")
        };
        let notice = match outcome {
            BundledSync::SkippedLockBusy { notice } => notice,
            BundledSync::Done => panic!("a sync that could not publish must say so"),
        };
        assert!(
            notice.contains(&published::lock_path(&install_root).display().to_string()),
            "the logged line names the lock it waited for: {notice}",
        );
        assert_eq!(
            published::installed_tree(&install_root.join("busy-starter-bundled")),
            first_tree,
            "nothing is published without the lock",
        );

        drop(peer);
        let (second_tree, version) = loaded_tree(&config, "busy-starter@bundled");
        assert_eq!(version, "1.0.1", "the next sync publishes");
        assert_ne!(second_tree, first_tree);

        let _ = fs::remove_dir_all(config_home);
        let _ = fs::remove_dir_all(bundled_root);
    }

    /// An install from before pointers — a plain tree at the logical path and
    /// no pointer — is read where it is and not moved while its version is
    /// current. Once a newer version is published the pointer wins: the plugin
    /// loads once, from the new tree, and the legacy tree neither loads again
    /// as an external plugin nor disappears at the switch.
    #[test]
    fn a_legacy_install_is_read_in_place_and_left_alone_after_the_switch() {
        let config_home = scratch("legacy-home");
        let bundled_root = scratch("legacy-source");
        let source = bundled_root.join("starter");
        write_bundled_source_at(&source, "legacy-starter", "1.0.0");
        let mut config = PluginManagerConfig::new(&config_home);
        config.bundled_root = Some(bundled_root.clone());
        let manager = PluginManager::new(config.clone());
        let install_root = manager.install_root();
        let legacy = install_root.join("legacy-starter-bundled");
        copy_dir_all(&source, &legacy).expect("legacy tree should be copyable");
        let mut registry = InstalledPluginRegistry::default();
        registry.plugins.insert(
            "legacy-starter@bundled".to_string(),
            InstalledPluginRecord {
                kind: PluginKind::Bundled,
                id: "legacy-starter@bundled".to_string(),
                name: "legacy-starter".to_string(),
                version: "1.0.0".to_string(),
                description: "bundled swap test".to_string(),
                install_path: legacy.clone(),
                source: PluginInstallSource::LocalPath {
                    path: source.clone(),
                },
                installed_at_unix_ms: 1,
                updated_at_unix_ms: 1,
                resolved_commit: None,
                content_sha256: None,
            },
        );
        manager.store_registry(&registry).expect("registry should be writable");

        let (tree, version) = loaded_tree(&config, "legacy-starter@bundled");
        assert_eq!(tree, legacy, "a legacy install is read in place");
        assert_eq!(version, "1.0.0");
        assert_eq!(
            published::installed_tree(&legacy),
            legacy,
            "a current legacy install is not moved",
        );

        write_bundled_source_at(&source, "legacy-starter", "1.0.1");
        let installed = PluginManager::new(config.clone())
            .list_installed_plugins()
            .expect("installed plugins should load");
        let loaded: Vec<(String, String, Option<PathBuf>)> = installed
            .into_iter()
            .filter(|plugin| plugin.metadata.name == "legacy-starter")
            .map(|plugin| (plugin.metadata.id, plugin.metadata.version, plugin.metadata.root))
            .collect();
        assert_eq!(
            loaded.len(),
            1,
            "the legacy tree must not load a second time as an external plugin: {loaded:?}",
        );
        let (id, version, root) = &loaded[0];
        assert_eq!(id, "legacy-starter@bundled");
        assert_eq!(version, "1.0.1");
        assert_ne!(root.as_deref(), Some(legacy.as_path()), "the pointer wins");
        assert!(
            legacy.join("hooks").join("pre.sh").exists(),
            "the legacy tree is neither renamed nor removed at the switch",
        );
        let record = manager
            .load_registry()
            .expect("registry should load")
            .plugins
            .remove("legacy-starter@bundled")
            .expect("the plugin stays recorded");
        assert_eq!(record.install_path, legacy, "the record keeps the logical path");

        let _ = fs::remove_dir_all(config_home);
        let _ = fs::remove_dir_all(bundled_root);
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
        let hook = published::installed_tree(&install_path)
            .join("hooks")
            .join("pre.sh");
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
        let republished = published::installed_tree(&install_path)
            .join("hooks")
            .join("pre.sh");
        assert!(republished.exists(), "the hook must be restored by the re-sync");
        assert_ne!(
            republished, hook,
            "the damaged tree is replaced by a fresh one, not repaired in place",
        );
        assert!(
            staging_entries(&install_root).is_empty(),
            "the republish must leave no staging behind",
        );

        let _ = fs::remove_dir_all(config_home);
        let _ = fs::remove_dir_all(bundled_root);
    }
}
