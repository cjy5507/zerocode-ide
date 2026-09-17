//! Persisted remote workspace identities.
//!
//! A workspace points at a stable SSH host UUID and a validated POSIX root.
//! Network verification stays in `zerocode-ssh`; this module owns only the
//! settings wire, normalization and mutation invariants.

use crate::ssh_hosts::SshHostEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;
use zerocode_core::host::{ExecutionHostId, RemotePath, RemoteWorkspace};

const MAX_WORKSPACES: usize = 64;
const MAX_LABEL_CHARS: usize = 80;

/// Map the server-reported home once, preserving an existing person's label.
pub(crate) fn ensure_home(
    entries: &mut Vec<RemoteWorkspaceEntry>,
    hosts: &[SshHostEntry],
    host_id: &str,
    root: &str,
) -> Result<(), String> {
    let host_id = ExecutionHostId::from_str(host_id).map_err(|e| e.to_string())?;
    if entries
        .iter()
        .any(|entry| entry.host_id() == host_id && entry.root().as_str() == root)
    {
        return Ok(());
    }
    let host = hosts
        .iter()
        .find(|host| host.id() == host_id)
        .ok_or("SSH host was removed")?;
    let prepared = RemoteWorkspaceInput {
        id: None,
        label: host.label().to_string(),
        host_id: host_id.to_string(),
        root: root.to_string(),
    }
    .prepare()
    .map_err(|e| e.to_string())?;
    upsert_entry(entries, hosts, prepared.entry, false).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteWorkspaceEntry {
    id: Uuid,
    label: String,
    workspace: RemoteWorkspace,
}

impl RemoteWorkspaceEntry {
    pub(crate) fn id(&self) -> Uuid {
        self.id
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn host_id(&self) -> ExecutionHostId {
        self.workspace.host_id
    }

    pub(crate) fn root(&self) -> &RemotePath {
        &self.workspace.root
    }

    pub(crate) fn with_verified_root(mut self, root: RemotePath) -> Self {
        self.workspace.root = root;
        self
    }

    fn normalized(mut self) -> Self {
        self.label = normalized_stored_label(&self.label, &self.workspace.root);
        self
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteWorkspaceInput {
    pub(crate) id: Option<String>,
    pub(crate) label: String,
    pub(crate) host_id: String,
    pub(crate) root: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteWorkspaceProbeInput {
    pub(crate) host_id: String,
    pub(crate) root: String,
}

impl RemoteWorkspaceProbeInput {
    pub(crate) fn prepare(self) -> Result<(ExecutionHostId, RemotePath), RemoteWorkspaceError> {
        let host_id = ExecutionHostId::from_str(self.host_id.trim())
            .map_err(|_| RemoteWorkspaceError::InvalidHostId)?;
        let root =
            RemotePath::parse(self.root.trim()).map_err(|_| RemoteWorkspaceError::InvalidRoot)?;
        Ok((host_id, root))
    }
}

pub(crate) struct PreparedRemoteWorkspace {
    pub(crate) entry: RemoteWorkspaceEntry,
    pub(crate) update: bool,
}

impl RemoteWorkspaceInput {
    pub(crate) fn prepare(self) -> Result<PreparedRemoteWorkspace, RemoteWorkspaceError> {
        let update = self.id.as_deref().is_some_and(|id| !id.trim().is_empty());
        let id = match self.id.as_deref().map(str::trim) {
            Some("") | None => Uuid::new_v4(),
            Some(id) => Uuid::parse_str(id).map_err(|_| RemoteWorkspaceError::InvalidId)?,
        };
        let host_id = ExecutionHostId::from_str(self.host_id.trim())
            .map_err(|_| RemoteWorkspaceError::InvalidHostId)?;
        let root =
            RemotePath::parse(self.root.trim()).map_err(|_| RemoteWorkspaceError::InvalidRoot)?;
        let label = normalized_input_label(&self.label, &root)?;
        Ok(PreparedRemoteWorkspace {
            entry: RemoteWorkspaceEntry {
                id,
                label,
                workspace: RemoteWorkspace { host_id, root },
            },
            update,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteWorkspaceView {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) host_id: String,
    pub(crate) host_label: String,
    pub(crate) root: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteWorkspacesReport {
    pub(crate) workspaces: Vec<RemoteWorkspaceView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerifiedRemoteRoot {
    pub(crate) root: String,
}

pub(crate) fn report(
    entries: &[RemoteWorkspaceEntry],
    hosts: &[SshHostEntry],
) -> RemoteWorkspacesReport {
    let workspaces = entries
        .iter()
        .filter_map(|entry| {
            let host = hosts.iter().find(|host| host.id() == entry.host_id())?;
            Some(RemoteWorkspaceView {
                id: entry.id().to_string(),
                label: entry.label().to_string(),
                host_id: entry.host_id().to_string(),
                host_label: host.label().to_string(),
                root: entry.root().to_string(),
            })
        })
        .collect();
    RemoteWorkspacesReport { workspaces }
}

pub(crate) fn normalize_entries(
    entries: Vec<RemoteWorkspaceEntry>,
    hosts: &[SshHostEntry],
) -> Vec<RemoteWorkspaceEntry> {
    let host_ids: HashSet<_> = hosts.iter().map(SshHostEntry::id).collect();
    let mut ids = HashSet::new();
    let mut locations = HashSet::new();
    entries
        .into_iter()
        .map(RemoteWorkspaceEntry::normalized)
        .filter(|entry| host_ids.contains(&entry.host_id()))
        .filter(|entry| ids.insert(entry.id()))
        .filter(|entry| locations.insert((entry.host_id(), entry.root().clone())))
        .take(MAX_WORKSPACES)
        .collect()
}

pub(crate) fn upsert_entry(
    entries: &mut Vec<RemoteWorkspaceEntry>,
    hosts: &[SshHostEntry],
    entry: RemoteWorkspaceEntry,
    update: bool,
) -> Result<(), RemoteWorkspaceError> {
    if !hosts.iter().any(|host| host.id() == entry.host_id()) {
        return Err(RemoteWorkspaceError::HostNotFound);
    }
    if entries.iter().any(|saved| {
        saved.id() != entry.id()
            && saved.host_id() == entry.host_id()
            && saved.root() == entry.root()
    }) {
        return Err(RemoteWorkspaceError::WorkspaceAlreadyExists);
    }
    match entries.iter().position(|saved| saved.id() == entry.id()) {
        Some(index) if update => entries[index] = entry,
        Some(_) => return Err(RemoteWorkspaceError::WorkspaceAlreadyExists),
        None if update => return Err(RemoteWorkspaceError::WorkspaceNotFound),
        None if entries.len() >= MAX_WORKSPACES => {
            return Err(RemoteWorkspaceError::WorkspaceLimitReached);
        }
        None => entries.push(entry),
    }
    Ok(())
}

pub(crate) fn remove_entry(
    entries: &mut Vec<RemoteWorkspaceEntry>,
    id: Uuid,
) -> Result<RemoteWorkspaceEntry, RemoteWorkspaceError> {
    let index = entries
        .iter()
        .position(|entry| entry.id() == id)
        .ok_or(RemoteWorkspaceError::WorkspaceNotFound)?;
    Ok(entries.remove(index))
}

pub(crate) fn find_entry(
    entries: &[RemoteWorkspaceEntry],
    id: Uuid,
) -> Result<&RemoteWorkspaceEntry, RemoteWorkspaceError> {
    entries
        .iter()
        .find(|entry| entry.id() == id)
        .ok_or(RemoteWorkspaceError::WorkspaceNotFound)
}

pub(crate) fn parse_id(id: &str) -> Result<Uuid, RemoteWorkspaceError> {
    Uuid::parse_str(id.trim()).map_err(|_| RemoteWorkspaceError::InvalidId)
}

pub(crate) fn remove_for_host(entries: &mut Vec<RemoteWorkspaceEntry>, host_id: ExecutionHostId) {
    entries.retain(|entry| entry.host_id() != host_id);
}

fn normalized_input_label(label: &str, root: &RemotePath) -> Result<String, RemoteWorkspaceError> {
    let label = label.trim();
    let label = if label.is_empty() {
        default_label(root)
    } else {
        label.to_string()
    };
    if label.chars().count() > MAX_LABEL_CHARS || label.chars().any(char::is_control) {
        return Err(RemoteWorkspaceError::InvalidLabel);
    }
    Ok(label)
}

fn normalized_stored_label(label: &str, root: &RemotePath) -> String {
    let trimmed = label.trim();
    let fallback = default_label(root);
    let source = if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        fallback.as_str()
    } else {
        trimmed
    };
    source.chars().take(MAX_LABEL_CHARS).collect()
}

fn default_label(root: &RemotePath) -> String {
    root.as_str()
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("/")
        .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteWorkspaceError {
    InvalidId,
    InvalidLabel,
    InvalidHostId,
    InvalidRoot,
    HostNotFound,
    WorkspaceAlreadyExists,
    WorkspaceNotFound,
    WorkspaceLimitReached,
}

impl fmt::Display for RemoteWorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "remote workspace identity is invalid",
            Self::InvalidLabel => "remote workspace label is invalid",
            Self::InvalidHostId => "remote workspace host identity is invalid",
            Self::InvalidRoot => "remote workspace root is invalid",
            Self::HostNotFound => "remote workspace SSH host was not found",
            Self::WorkspaceAlreadyExists => "remote workspace already exists",
            Self::WorkspaceNotFound => "remote workspace was not found",
            Self::WorkspaceLimitReached => "remote workspace limit reached",
        })
    }
}

impl std::error::Error for RemoteWorkspaceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh_hosts::{SshHostInput, TEST_HOST_KEY};
    use zerocode_core::host::SshAuthentication;

    #[test]
    fn automatic_home_mapping_is_idempotent_and_preserves_saved_labels() {
        let host = host();
        let mut entries = Vec::new();
        ensure_home(
            &mut entries,
            std::slice::from_ref(&host),
            &host.id().to_string(),
            "/home/builder",
        )
        .expect("first mapping");
        let first = entries[0].id();
        entries[0].label = "Personal workspace name".to_string();
        ensure_home(
            &mut entries,
            std::slice::from_ref(&host),
            &host.id().to_string(),
            "/home/builder",
        )
        .expect("repeat mapping");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id(), first);
        assert_eq!(entries[0].label(), "Personal workspace name");
    }

    fn host() -> SshHostEntry {
        SshHostInput {
            id: None,
            label: "Build machine".to_string(),
            host: "build.example.test".to_string(),
            port: 22,
            user: "builder".to_string(),
            authentication: SshAuthentication::Agent,
            key_algorithm: "ssh-ed25519".to_string(),
            encoded_key: TEST_HOST_KEY.to_string(),
            password: None,
        }
        .prepare()
        .expect("host")
        .entry
    }

    fn input(host: &SshHostEntry, root: &str) -> RemoteWorkspaceInput {
        RemoteWorkspaceInput {
            id: None,
            label: "Build checkout".to_string(),
            host_id: host.id().to_string(),
            root: root.to_string(),
        }
    }

    #[test]
    fn a_workspace_keeps_a_host_identity_and_validated_posix_root() {
        let host = host();
        let prepared = input(&host, "/srv/project").prepare().expect("workspace");
        assert_eq!(prepared.entry.host_id(), host.id());
        assert_eq!(prepared.entry.root().as_str(), "/srv/project");
        assert!(!prepared.update);
    }

    #[test]
    fn ambiguous_or_local_roots_never_reach_the_settings_document() {
        let host = host();
        for root in ["srv/project", "/srv/../secret", "/srv//project"] {
            assert!(matches!(
                input(&host, root).prepare(),
                Err(RemoteWorkspaceError::InvalidRoot)
            ));
        }
    }

    #[test]
    fn an_unknown_host_cannot_own_a_workspace() {
        let host = host();
        let entry = input(&host, "/srv/project")
            .prepare()
            .expect("workspace")
            .entry;
        let error = upsert_entry(&mut Vec::new(), &[], entry, false).expect_err("unknown host");
        assert_eq!(error, RemoteWorkspaceError::HostNotFound);
    }

    #[test]
    fn one_remote_location_has_one_stable_workspace_identity() {
        let host = host();
        let first = input(&host, "/srv/project").prepare().expect("first").entry;
        let second = input(&host, "/srv/project")
            .prepare()
            .expect("second")
            .entry;
        let mut entries = vec![first];
        assert_eq!(
            upsert_entry(&mut entries, &[host], second, false),
            Err(RemoteWorkspaceError::WorkspaceAlreadyExists)
        );
    }

    #[test]
    fn normalization_drops_orphans_and_duplicate_locations() {
        let host = host();
        let first = input(&host, "/srv/project").prepare().expect("first").entry;
        let duplicate = input(&host, "/srv/project")
            .prepare()
            .expect("duplicate")
            .entry;
        assert_eq!(normalize_entries(vec![first, duplicate], &[host]).len(), 1);
    }

    #[test]
    fn removing_a_host_removes_only_its_remote_workspaces() {
        let first_host = host();
        let second_host = host();
        let first = input(&first_host, "/srv/first")
            .prepare()
            .expect("first")
            .entry;
        let second = input(&second_host, "/srv/second")
            .prepare()
            .expect("second")
            .entry;
        let mut entries = vec![first, second];
        remove_for_host(&mut entries, first_host.id());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].host_id(), second_host.id());
    }
}
