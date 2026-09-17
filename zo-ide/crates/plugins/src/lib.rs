//! Plugin system — re-export hub.
//!
//! Each domain lives in its own module:
//!
//! | Module           | Responsibility                                                    |
//! |------------------|-------------------------------------------------------------------|
//! | `manifest`       | Wire + resolved manifest types (`PluginKind`, `PluginManifest`, …)|
//! | `installed`      | On-disk `installed.json` schema (`InstalledPluginRegistry`)       |
//! | `error`          | `PluginError` + `PluginManifestValidationError`                   |
//! | `plugin`         | `Plugin` trait + `Builtin/Bundled/External/PluginDefinition`      |
//! | `tool`           | `PluginTool` resolved entry + `execute`                           |
//! | `registry`       | `PluginRegistry`, `RegisteredPlugin`, `PluginManagerConfig`, …    |
//! | `builtin`        | Compiled-in scaffolding + `load_plugin_definition` worker         |
//! | `manifest_io`    | Read + validate `plugin.json`, resolve relative paths             |
//! | `path_validators`| `validate_*_paths` + `run_lifecycle_commands`                     |
//! | `install`        | parse / materialize / copy / settings-mutator helpers             |
//! | `manager`        | `PluginManager` — orchestrates everything above                   |
//!
//! Everything callers need is re-exported below; internal modules
//! stay `pub(crate)` or `pub(super)` so the surface stays small.

mod builtin;
mod error;
mod install;
mod installed;
mod manager;
mod manifest;
mod manifest_io;
mod path_validators;
mod plugin;
mod process_runner;
mod registry;
mod tool;
mod util;

#[cfg(test)]
mod tests;

pub use builtin::builtin_plugins;
pub use error::{PluginError, PluginManifestValidationError};
pub use installed::{InstalledPluginRecord, InstalledPluginRegistry, PluginInstallSource};
pub use manager::{InstallOutcome, PluginManager, UpdateOutcome};
pub use manifest::{
    PluginCommandManifest, PluginHooks, PluginKind, PluginLifecycle, PluginManifest,
    PluginMetadata, PluginPermission, PluginToolDefinition, PluginToolManifest,
    PluginToolPermission,
};
pub use manifest_io::load_plugin_from_directory;
pub use plugin::{BuiltinPlugin, BundledPlugin, ExternalPlugin, Plugin, PluginDefinition};
pub use registry::{
    PluginLoadFailure, PluginManagerConfig, PluginRegistry, PluginRegistryReport, PluginSummary,
    RegisteredPlugin,
};
pub use tool::PluginTool;
