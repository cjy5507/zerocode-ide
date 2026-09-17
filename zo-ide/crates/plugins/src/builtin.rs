//! Built-in plugin scaffolding and the on-disk plugin loader.
//!
//! [`builtin_plugins`] returns the compiled-in plugin set — currently
//! just the `example-builtin` scaffold so the manager always has at
//! least one entry to surface.
//!
//! [`load_plugin_definition`] is the manager's worker that turns a
//! discovered on-disk root (built-in / bundled / external) into a
//! resolved [`PluginDefinition`]. It runs the validated manifest
//! through `resolve_hooks` / `resolve_lifecycle` / `resolve_tools` so
//! every entry the plugin lists ends up absolute relative to its root.

use std::path::Path;

use super::manifest::BUILTIN_MARKETPLACE;
use super::manifest_io::{resolve_hooks, resolve_lifecycle, resolve_tools};
use super::util::plugin_id;
use super::{
    load_plugin_from_directory, BuiltinPlugin, BundledPlugin, ExternalPlugin, PluginDefinition,
    PluginError, PluginHooks, PluginKind, PluginLifecycle, PluginMetadata,
};

#[must_use]
pub fn builtin_plugins() -> Vec<PluginDefinition> {
    vec![PluginDefinition::Builtin(BuiltinPlugin {
        metadata: PluginMetadata {
            id: plugin_id("example-builtin", BUILTIN_MARKETPLACE),
            name: "example-builtin".to_string(),
            version: "0.1.0".to_string(),
            description: "Example built-in plugin scaffold for the Rust plugin system".to_string(),
            kind: PluginKind::Builtin,
            source: BUILTIN_MARKETPLACE.to_string(),
            default_enabled: false,
            root: None,
        },
        hooks: PluginHooks::default(),
        lifecycle: PluginLifecycle::default(),
        tools: Vec::new(),
        commands: Vec::new(),
    })]
}

pub(crate) fn load_plugin_definition(
    root: &Path,
    kind: PluginKind,
    source: String,
    marketplace: &str,
) -> Result<PluginDefinition, PluginError> {
    let manifest = load_plugin_from_directory(root)?;
    let metadata = PluginMetadata {
        id: plugin_id(&manifest.name, marketplace),
        name: manifest.name,
        version: manifest.version,
        description: manifest.description,
        kind,
        source,
        default_enabled: manifest.default_enabled,
        root: Some(root.to_path_buf()),
    };
    let hooks = resolve_hooks(root, &manifest.hooks);
    let lifecycle = resolve_lifecycle(root, &manifest.lifecycle);
    let tools = resolve_tools(root, &metadata.id, &metadata.name, &manifest.tools);
    let commands = manifest.commands;
    Ok(match kind {
        PluginKind::Builtin => PluginDefinition::Builtin(BuiltinPlugin {
            metadata,
            hooks,
            lifecycle,
            tools,
            commands,
        }),
        PluginKind::Bundled => PluginDefinition::Bundled(BundledPlugin {
            metadata,
            hooks,
            lifecycle,
            tools,
            commands,
        }),
        PluginKind::External => PluginDefinition::External(ExternalPlugin {
            metadata,
            hooks,
            lifecycle,
            tools,
            commands,
        }),
    })
}
