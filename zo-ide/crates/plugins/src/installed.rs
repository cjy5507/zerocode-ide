//! Persistent record of plugins that have been installed.
//!
//! [`InstalledPluginRegistry`] is the deserialized form of
//! `~/.zo/plugins/installed.json`. The plugin manager reads it on
//! startup to populate the in-memory [`super::PluginRegistry`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::PluginKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginInstallSource {
    LocalPath {
        path: PathBuf,
    },
    GitUrl {
        url: String,
        /// Optional supply-chain pin: a commit SHA, tag, or branch the
        /// install/update must check out (parsed from a `url#ref` suffix).
        /// `None` tracks the default branch's tip. Old records without this
        /// field deserialize to `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPluginRecord {
    #[serde(default = "default_plugin_kind")]
    pub kind: PluginKind,
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub install_path: PathBuf,
    pub source: PluginInstallSource,
    pub installed_at_unix_ms: u128,
    pub updated_at_unix_ms: u128,
    /// Git commit SHA actually checked out at install time (provenance for
    /// `GitUrl` sources). `None` for local installs or pre-supply-chain
    /// records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_commit: Option<String>,
    /// SHA-256 over the installed plugin tree at install time. On load the
    /// digest is recomputed and compared so on-disk tampering is rejected.
    /// `None` for pre-supply-chain records (verification is skipped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPluginRegistry {
    #[serde(default)]
    pub plugins: BTreeMap<String, InstalledPluginRecord>,
}

pub(crate) fn default_plugin_kind() -> PluginKind {
    PluginKind::External
}
