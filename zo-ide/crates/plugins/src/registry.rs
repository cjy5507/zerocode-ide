//! In-memory registry types backed by the plugin manager.
//!
//! - [`RegisteredPlugin`] wraps a [`PluginDefinition`] with the
//!   enabled/disabled flag that the manager persists in
//!   `settings.json`.
//! - [`PluginRegistry`] is the sorted collection of registered plugins
//!   the manager hands out. Aggregation methods (`aggregated_hooks`,
//!   `aggregated_tools`) compose enabled plugins for the hook/tool
//!   pipeline.
//! - [`PluginRegistryReport`] is what `load_registry_report` returns
//!   when discovery runs — the registry plus any non-fatal load
//!   failures encountered along the way.
//! - [`PluginDiscovery`] is the manager-internal accumulator used while
//!   walking installed/bundled/builtin plugins; the fields and helpers
//!   stay `pub(crate)` so the manager can drive it directly.
//! - [`PluginManagerConfig`] is the user-facing config struct.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use super::{
    Plugin, PluginCommandManifest, PluginDefinition, PluginError, PluginHooks, PluginKind,
    PluginMetadata, PluginTool,
};

#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredPlugin {
    pub(crate) definition: PluginDefinition,
    enabled: bool,
}

impl RegisteredPlugin {
    #[must_use]
    pub fn new(definition: PluginDefinition, enabled: bool) -> Self {
        Self {
            definition,
            enabled,
        }
    }

    #[must_use]
    pub fn metadata(&self) -> &PluginMetadata {
        self.definition.metadata()
    }

    #[must_use]
    pub fn hooks(&self) -> &PluginHooks {
        self.definition.hooks()
    }

    #[must_use]
    pub fn tools(&self) -> &[PluginTool] {
        self.definition.tools()
    }

    #[must_use]
    pub fn commands(&self) -> &[PluginCommandManifest] {
        self.definition.commands()
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn validate(&self) -> Result<(), PluginError> {
        self.definition.validate()
    }

    pub fn initialize(&self) -> Result<(), PluginError> {
        self.definition.initialize()
    }

    pub fn shutdown(&self) -> Result<(), PluginError> {
        self.definition.shutdown()
    }

    #[must_use]
    pub fn summary(&self) -> PluginSummary {
        PluginSummary {
            metadata: self.metadata().clone(),
            enabled: self.enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSummary {
    pub metadata: PluginMetadata,
    pub enabled: bool,
}

#[derive(Debug)]
pub struct PluginLoadFailure {
    pub plugin_root: PathBuf,
    pub kind: PluginKind,
    pub source: String,
    error: Box<PluginError>,
}

impl PluginLoadFailure {
    #[must_use]
    pub fn new(plugin_root: PathBuf, kind: PluginKind, source: String, error: PluginError) -> Self {
        Self {
            plugin_root,
            kind,
            source,
            error: Box::new(error),
        }
    }

    #[must_use]
    pub fn error(&self) -> &PluginError {
        self.error.as_ref()
    }
}

impl Display for PluginLoadFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to load {} plugin from `{}` (source: {}): {}",
            self.kind,
            self.plugin_root.display(),
            self.source,
            self.error()
        )
    }
}

#[derive(Debug)]
pub struct PluginRegistryReport {
    registry: PluginRegistry,
    failures: Vec<PluginLoadFailure>,
}

impl PluginRegistryReport {
    #[must_use]
    pub fn new(registry: PluginRegistry, failures: Vec<PluginLoadFailure>) -> Self {
        Self { registry, failures }
    }

    #[must_use]
    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    #[must_use]
    pub fn failures(&self) -> &[PluginLoadFailure] {
        &self.failures
    }

    #[must_use]
    pub fn has_failures(&self) -> bool {
        !self.failures.is_empty()
    }

    #[must_use]
    pub fn summaries(&self) -> Vec<PluginSummary> {
        self.registry.summaries()
    }

    pub fn into_registry(self) -> Result<PluginRegistry, PluginError> {
        if self.failures.is_empty() {
            Ok(self.registry)
        } else {
            Err(PluginError::LoadFailures(self.failures))
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct PluginDiscovery {
    pub(crate) plugins: Vec<PluginDefinition>,
    pub(crate) failures: Vec<PluginLoadFailure>,
}

impl PluginDiscovery {
    pub(crate) fn push_plugin(&mut self, plugin: PluginDefinition) {
        self.plugins.push(plugin);
    }

    pub(crate) fn push_failure(&mut self, failure: PluginLoadFailure) {
        self.failures.push(failure);
    }

    pub(crate) fn extend(&mut self, other: Self) {
        self.plugins.extend(other.plugins);
        self.failures.extend(other.failures);
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginRegistry {
    pub(crate) plugins: Vec<RegisteredPlugin>,
}

impl PluginRegistry {
    #[must_use]
    pub fn new(mut plugins: Vec<RegisteredPlugin>) -> Self {
        plugins.sort_by(|left, right| left.metadata().id.cmp(&right.metadata().id));
        Self { plugins }
    }

    #[must_use]
    pub fn plugins(&self) -> &[RegisteredPlugin] {
        &self.plugins
    }

    #[must_use]
    pub fn get(&self, plugin_id: &str) -> Option<&RegisteredPlugin> {
        self.plugins
            .iter()
            .find(|plugin| plugin.metadata().id == plugin_id)
    }

    #[must_use]
    pub fn contains(&self, plugin_id: &str) -> bool {
        self.get(plugin_id).is_some()
    }

    #[must_use]
    pub fn summaries(&self) -> Vec<PluginSummary> {
        self.plugins.iter().map(RegisteredPlugin::summary).collect()
    }

    pub fn aggregated_hooks(&self) -> Result<PluginHooks, PluginError> {
        self.plugins
            .iter()
            .filter(|plugin| plugin.is_enabled())
            .try_fold(PluginHooks::default(), |acc, plugin| {
                plugin.validate()?;
                Ok(acc.merged_with(plugin.hooks()))
            })
    }

    pub fn aggregated_tools(&self) -> Result<Vec<PluginTool>, PluginError> {
        let mut tools = Vec::new();
        let mut seen_names = BTreeMap::new();
        for plugin in self.plugins.iter().filter(|plugin| plugin.is_enabled()) {
            plugin.validate()?;
            for tool in plugin.tools() {
                if let Some(existing_plugin) =
                    seen_names.insert(tool.definition().name.clone(), tool.plugin_id().to_string())
                {
                    return Err(PluginError::InvalidManifest(format!(
                        "plugin tool `{}` is defined by both `{existing_plugin}` and `{}`",
                        tool.definition().name,
                        tool.plugin_id()
                    )));
                }
                tools.push(tool.clone());
            }
        }
        Ok(tools)
    }

    /// Slash command metadata contributed by enabled plugins, as
    /// `(plugin_id, name, description)`. Feeds help listing and the
    /// `SlashCommandRegistry` wiring.
    #[must_use]
    pub fn slash_command_specs(&self) -> Vec<(String, String, String)> {
        self.plugins
            .iter()
            .filter(|plugin| plugin.is_enabled())
            .flat_map(|plugin| {
                let plugin_id = plugin.metadata().id.clone();
                plugin.commands().iter().map(move |command| {
                    (
                        plugin_id.clone(),
                        command.name.clone(),
                        command.description.clone(),
                    )
                })
            })
            .collect()
    }

    /// Find the enabled plugin (and matching manifest) that contributes the
    /// slash command `name` (leading `/` optional).
    #[must_use]
    pub fn find_slash_command(
        &self,
        name: &str,
    ) -> Option<(&RegisteredPlugin, &PluginCommandManifest)> {
        let name = name.trim().trim_start_matches('/');
        self.plugins
            .iter()
            .filter(|plugin| plugin.is_enabled())
            .find_map(|plugin| {
                plugin
                    .commands()
                    .iter()
                    .find(|command| command.name == name)
                    .map(|command| (plugin, command))
            })
    }

    /// Execute the plugin slash command `name` with `args`, returning the
    /// script's trimmed stdout. The command script runs with the plugin root
    /// as its working directory, mirroring [`PluginTool`] execution.
    ///
    /// # Errors
    /// [`PluginError::NotFound`] when no enabled plugin contributes the
    /// command; [`PluginError::CommandFailed`]/[`PluginError::Io`] when the
    /// script cannot be spawned or exits non-zero.
    pub fn run_slash_command(&self, name: &str, args: &str) -> Result<String, PluginError> {
        let (plugin, command) = self.find_slash_command(name).ok_or_else(|| {
            PluginError::NotFound(format!(
                "no enabled plugin contributes slash command `/{}`",
                name.trim().trim_start_matches('/')
            ))
        })?;
        run_plugin_command_script(plugin.metadata(), command, args)
    }

    pub fn initialize(&self) -> Result<(), PluginError> {
        for plugin in self.plugins.iter().filter(|plugin| plugin.is_enabled()) {
            plugin.validate()?;
            plugin.initialize()?;
        }
        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), PluginError> {
        for plugin in self
            .plugins
            .iter()
            .rev()
            .filter(|plugin| plugin.is_enabled())
        {
            // Validate before running shutdown commands, mirroring `initialize`,
            // so the containment/path checks gate this execution boundary too
            // rather than trusting paths that may have changed since load.
            plugin.validate()?;
            plugin.shutdown()?;
        }
        Ok(())
    }
}

/// Spawn a plugin slash command's script with the plugin root as its working
/// directory, mirroring [`PluginTool`] execution. `args` is passed both as a
/// single argv argument and via `ZO_SLASH_ARGS`; the script's trimmed
/// stdout is returned on success.
fn run_plugin_command_script(
    metadata: &PluginMetadata,
    command: &PluginCommandManifest,
    args: &str,
) -> Result<String, PluginError> {
    use std::process::Command;

    use super::process_runner::run_plugin_process;

    let mut process = Command::new(&command.command);
    process
        .arg(args)
        .env("ZO_PLUGIN_ID", &metadata.id)
        .env("ZO_PLUGIN_NAME", &metadata.name)
        .env("ZO_SLASH_COMMAND", &command.name)
        .env("ZO_SLASH_ARGS", args);
    if let Some(root) = &metadata.root {
        process
            .current_dir(root)
            .env("ZO_PLUGIN_ROOT", root.display().to_string());
    }

    // Bounded wall-clock + output via the shared plugin process runner so a hung
    // or chatty slash command cannot freeze the agent or exhaust memory.
    let context = format!(
        "plugin slash command `/{}` from `{}`",
        command.name, metadata.id
    );
    let output = run_plugin_process(process, None, &context)?;
    if output.success {
        Ok(output.stdout.trim().to_string())
    } else {
        let stderr = output.stderr.trim().to_string();
        Err(PluginError::CommandFailed(format!(
            "plugin slash command `/{}` from `{}` failed: {}",
            command.name,
            metadata.id,
            if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            }
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManagerConfig {
    pub config_home: PathBuf,
    pub enabled_plugins: BTreeMap<String, bool>,
    pub external_dirs: Vec<PathBuf>,
    pub install_root: Option<PathBuf>,
    pub registry_path: Option<PathBuf>,
    pub bundled_root: Option<PathBuf>,
    /// Read-only install roots discovered under the *lower-priority* canonical
    /// homes (`ZO_HOME`, `$HOME/.zo`) when `config_home` is a higher-priority
    /// root. Installed-plugin discovery merges these in so a plugin installed
    /// under any canonical home is still found; every write/install/update
    /// target remains the primary `config_home`-derived [`install_root`]. The
    /// primary root always wins on plugin-id collisions.
    ///
    /// [`install_root`]: super::PluginManager::install_root
    pub discovery_install_roots: Vec<PathBuf>,
}

impl PluginManagerConfig {
    #[must_use]
    pub fn new(config_home: impl Into<PathBuf>) -> Self {
        Self {
            config_home: config_home.into(),
            enabled_plugins: BTreeMap::new(),
            external_dirs: Vec::new(),
            install_root: None,
            registry_path: None,
            bundled_root: None,
            discovery_install_roots: Vec::new(),
        }
    }
}
