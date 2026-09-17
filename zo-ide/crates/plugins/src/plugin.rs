//! Plugin variants and the [`Plugin`] trait that unifies them.
//!
//! Three concrete kinds share the same shape (`metadata` + `hooks` +
//! `lifecycle` + `tools`) but differ in how aggressively they validate
//! and what they do during initialize/shutdown:
//!
//! - [`BuiltinPlugin`] — compiled in; no path validation, no lifecycle
//!   commands. Always trusted.
//! - [`BundledPlugin`] — shipped on disk under the binary's data
//!   directory. Validates referenced paths and runs lifecycle commands.
//! - [`ExternalPlugin`] — user-installed under the config home. Same
//!   validation as `Bundled` but the source is mutable.
//!
//! [`PluginDefinition`] is the enum the registry stores; it forwards
//! `Plugin` method calls to whichever concrete variant it wraps.

use super::path_validators::{
    run_lifecycle_commands, validate_hook_paths, validate_lifecycle_paths, validate_tool_paths,
};
use super::{
    PluginCommandManifest, PluginError, PluginHooks, PluginLifecycle, PluginMetadata, PluginTool,
};

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltinPlugin {
    pub(crate) metadata: PluginMetadata,
    pub(crate) hooks: PluginHooks,
    pub(crate) lifecycle: PluginLifecycle,
    pub(crate) tools: Vec<PluginTool>,
    pub(crate) commands: Vec<PluginCommandManifest>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BundledPlugin {
    pub(crate) metadata: PluginMetadata,
    pub(crate) hooks: PluginHooks,
    pub(crate) lifecycle: PluginLifecycle,
    pub(crate) tools: Vec<PluginTool>,
    pub(crate) commands: Vec<PluginCommandManifest>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalPlugin {
    pub(crate) metadata: PluginMetadata,
    pub(crate) hooks: PluginHooks,
    pub(crate) lifecycle: PluginLifecycle,
    pub(crate) tools: Vec<PluginTool>,
    pub(crate) commands: Vec<PluginCommandManifest>,
}

pub trait Plugin {
    fn metadata(&self) -> &PluginMetadata;
    fn hooks(&self) -> &PluginHooks;
    fn lifecycle(&self) -> &PluginLifecycle;
    fn tools(&self) -> &[PluginTool];
    /// Top-level slash commands this plugin contributes. Each entry's
    /// `command` is a script path relative to the plugin root.
    fn commands(&self) -> &[PluginCommandManifest];
    fn validate(&self) -> Result<(), PluginError>;
    fn initialize(&self) -> Result<(), PluginError>;
    fn shutdown(&self) -> Result<(), PluginError>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginDefinition {
    Builtin(BuiltinPlugin),
    Bundled(BundledPlugin),
    External(ExternalPlugin),
}

impl Plugin for BuiltinPlugin {
    fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    fn hooks(&self) -> &PluginHooks {
        &self.hooks
    }

    fn lifecycle(&self) -> &PluginLifecycle {
        &self.lifecycle
    }

    fn tools(&self) -> &[PluginTool] {
        &self.tools
    }

    fn commands(&self) -> &[PluginCommandManifest] {
        &self.commands
    }

    fn validate(&self) -> Result<(), PluginError> {
        Ok(())
    }

    fn initialize(&self) -> Result<(), PluginError> {
        Ok(())
    }

    fn shutdown(&self) -> Result<(), PluginError> {
        Ok(())
    }
}

impl Plugin for BundledPlugin {
    fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    fn hooks(&self) -> &PluginHooks {
        &self.hooks
    }

    fn lifecycle(&self) -> &PluginLifecycle {
        &self.lifecycle
    }

    fn tools(&self) -> &[PluginTool] {
        &self.tools
    }

    fn commands(&self) -> &[PluginCommandManifest] {
        &self.commands
    }

    fn validate(&self) -> Result<(), PluginError> {
        validate_hook_paths(self.metadata.root.as_deref(), &self.hooks)?;
        validate_lifecycle_paths(self.metadata.root.as_deref(), &self.lifecycle)?;
        validate_tool_paths(self.metadata.root.as_deref(), &self.tools)
    }

    fn initialize(&self) -> Result<(), PluginError> {
        run_lifecycle_commands(
            self.metadata(),
            self.lifecycle(),
            "init",
            &self.lifecycle.init,
        )
    }

    fn shutdown(&self) -> Result<(), PluginError> {
        run_lifecycle_commands(
            self.metadata(),
            self.lifecycle(),
            "shutdown",
            &self.lifecycle.shutdown,
        )
    }
}

impl Plugin for ExternalPlugin {
    fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    fn hooks(&self) -> &PluginHooks {
        &self.hooks
    }

    fn lifecycle(&self) -> &PluginLifecycle {
        &self.lifecycle
    }

    fn tools(&self) -> &[PluginTool] {
        &self.tools
    }

    fn commands(&self) -> &[PluginCommandManifest] {
        &self.commands
    }

    fn validate(&self) -> Result<(), PluginError> {
        validate_hook_paths(self.metadata.root.as_deref(), &self.hooks)?;
        validate_lifecycle_paths(self.metadata.root.as_deref(), &self.lifecycle)?;
        validate_tool_paths(self.metadata.root.as_deref(), &self.tools)
    }

    fn initialize(&self) -> Result<(), PluginError> {
        run_lifecycle_commands(
            self.metadata(),
            self.lifecycle(),
            "init",
            &self.lifecycle.init,
        )
    }

    fn shutdown(&self) -> Result<(), PluginError> {
        run_lifecycle_commands(
            self.metadata(),
            self.lifecycle(),
            "shutdown",
            &self.lifecycle.shutdown,
        )
    }
}

impl Plugin for PluginDefinition {
    fn metadata(&self) -> &PluginMetadata {
        match self {
            Self::Builtin(plugin) => plugin.metadata(),
            Self::Bundled(plugin) => plugin.metadata(),
            Self::External(plugin) => plugin.metadata(),
        }
    }

    fn hooks(&self) -> &PluginHooks {
        match self {
            Self::Builtin(plugin) => plugin.hooks(),
            Self::Bundled(plugin) => plugin.hooks(),
            Self::External(plugin) => plugin.hooks(),
        }
    }

    fn lifecycle(&self) -> &PluginLifecycle {
        match self {
            Self::Builtin(plugin) => plugin.lifecycle(),
            Self::Bundled(plugin) => plugin.lifecycle(),
            Self::External(plugin) => plugin.lifecycle(),
        }
    }

    fn tools(&self) -> &[PluginTool] {
        match self {
            Self::Builtin(plugin) => plugin.tools(),
            Self::Bundled(plugin) => plugin.tools(),
            Self::External(plugin) => plugin.tools(),
        }
    }

    fn commands(&self) -> &[PluginCommandManifest] {
        match self {
            Self::Builtin(plugin) => plugin.commands(),
            Self::Bundled(plugin) => plugin.commands(),
            Self::External(plugin) => plugin.commands(),
        }
    }

    fn validate(&self) -> Result<(), PluginError> {
        match self {
            Self::Builtin(plugin) => plugin.validate(),
            Self::Bundled(plugin) => plugin.validate(),
            Self::External(plugin) => plugin.validate(),
        }
    }

    fn initialize(&self) -> Result<(), PluginError> {
        match self {
            Self::Builtin(plugin) => plugin.initialize(),
            Self::Bundled(plugin) => plugin.initialize(),
            Self::External(plugin) => plugin.initialize(),
        }
    }

    fn shutdown(&self) -> Result<(), PluginError> {
        match self {
            Self::Builtin(plugin) => plugin.shutdown(),
            Self::Bundled(plugin) => plugin.shutdown(),
            Self::External(plugin) => plugin.shutdown(),
        }
    }
}
