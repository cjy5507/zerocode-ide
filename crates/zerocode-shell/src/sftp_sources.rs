//! The file manager's host list: saved SSH hosts and `~/.ssh/config` targets
//! folded into one row per server.
//!
//! A saved host and a config target that name the same `user@host:port` are
//! one server, so they are one row. The target wins because the system `ssh`
//! honours everything in the person's own config — jump hosts, proxy commands,
//! identity files — and reuses the terminal's live connection; the saved host
//! is remembered on the row (`native_host_id`) so a door that only knows the
//! saved id (a remote workspace card) still opens the same row.
use std::collections::BTreeSet;

use crate::sftp_runtime::TARGET_PREFIX;
use crate::ssh_hosts::SshHostEntry;
use crate::ssh_store::SshTarget;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Source {
    pub(crate) host_id: String,
    pub(crate) label: String,
    pub(crate) target_id: Option<String>,
    /// The saved host this row also answers for, when a target absorbed one.
    pub(crate) native_host_id: Option<String>,
    pub(crate) roots: Vec<String>,
}

/// Fold saved hosts and targets into rows. `roots` are `(host id, path)`
/// pairs from remote workspaces and bookmarks; a pair keyed by either identity
/// of a folded row lands on that row.
pub(crate) fn fold<'a>(
    hosts: &[SshHostEntry],
    targets: &[SshTarget],
    roots: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<Source> {
    let mut absorbed = vec![false; hosts.len()];
    let mut target_rows: Vec<Source> = targets
        .iter()
        .map(|target| {
            let twin = twin_of(target, hosts, &absorbed);
            if let Some(index) = twin {
                absorbed[index] = true;
            }
            Source {
                host_id: format!("{TARGET_PREFIX}{}", target.id),
                label: if target.label.is_empty() {
                    target.host.clone()
                } else {
                    target.label.clone()
                },
                target_id: Some(target.id.clone()),
                native_host_id: twin.map(|index| hosts[index].id().to_string()),
                roots: Vec::new(),
            }
        })
        .collect();
    let mut sources: Vec<Source> = hosts
        .iter()
        .zip(&absorbed)
        .filter(|(_, absorbed)| !**absorbed)
        .map(|(host, _)| Source {
            host_id: host.id().to_string(),
            label: host.label().to_string(),
            target_id: None,
            native_host_id: None,
            roots: Vec::new(),
        })
        .collect();
    sources.append(&mut target_rows);
    let roots: Vec<(&str, &str)> = roots.into_iter().collect();
    for source in &mut sources {
        let mine: BTreeSet<&str> = roots
            .iter()
            .filter(|(id, _)| {
                *id == source.host_id || source.native_host_id.as_deref() == Some(*id)
            })
            .map(|(_, path)| *path)
            .collect();
        source.roots = mine.into_iter().map(str::to_owned).collect();
    }
    sources
}

/// The saved host `target` stands for: same `user@host:port`, not absorbed
/// yet, and — because one server often carries several aliases (a shell
/// alias and a tunnel alias, say) — the one wearing the same label first.
fn twin_of(target: &SshTarget, hosts: &[SshHostEntry], absorbed: &[bool]) -> Option<usize> {
    if target.host.is_empty() || target.username.is_empty() {
        return None;
    }
    let candidates = || {
        hosts.iter().enumerate().filter(|(index, host)| {
            let endpoint = host.record().endpoint();
            !absorbed[*index]
                && endpoint.host() == target.host
                && endpoint.port() == target.port
                && endpoint.user() == target.username
        })
    };
    candidates()
        .find(|(_, host)| host.label() == target.label)
        .or_else(|| candidates().next())
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(label: &str, id: &str, address: &str, port: u16, user: &str) -> SshHostEntry {
        serde_json::from_value(serde_json::json!({
            "label": label,
            "record": {
                "id": id,
                "endpoint": { "host": address, "port": port, "user": user },
                "authentication": "agent",
                "pinned_host_key": {
                    "algorithm": "ssh-ed25519",
                    "encoded_key": crate::ssh_hosts::TEST_HOST_KEY,
                },
            },
        }))
        .expect("a saved host in the settings document's shape")
    }

    fn target(label: &str, id: &str, address: &str, port: u16, user: &str) -> SshTarget {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "label": label,
            "host": address,
            "configHost": label.rsplit('@').next().unwrap_or_default(),
            "port": port,
            "username": user,
        }))
        .expect("a target in the settings document's shape")
    }

    const EDGE: &str = "903640b2-e3eb-4bd5-b5fc-4a1dd2cfed74";
    const TUNNEL: &str = "8b9eb85a-bb94-4e47-a542-e9fd6eba4a38";

    #[test]
    fn a_saved_host_and_a_target_for_the_same_server_are_one_row_and_the_target_answers() {
        let hosts = [host("acme@edge-ssh", EDGE, "192.0.2.161", 22012, "acme")];
        let targets = [target(
            "acme@edge-ssh",
            "ssh-1",
            "192.0.2.161",
            22012,
            "acme",
        )];
        let rows = fold(&hosts, &targets, []);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].host_id, "target:ssh-1");
        assert_eq!(rows[0].target_id.as_deref(), Some("ssh-1"));
        assert_eq!(rows[0].native_host_id.as_deref(), Some(EDGE));
        assert_eq!(rows[0].label, "acme@edge-ssh");
    }

    #[test]
    fn roots_saved_under_either_identity_land_on_the_folded_row() {
        let hosts = [host("acme@edge-ssh", EDGE, "192.0.2.161", 22012, "acme")];
        let targets = [target(
            "acme@edge-ssh",
            "ssh-1",
            "192.0.2.161",
            22012,
            "acme",
        )];
        let rows = fold(
            &hosts,
            &targets,
            [
                (EDGE, "/home/acme/vol_02"),
                ("target:ssh-1", "/home/acme"),
                (EDGE, "/home/acme"),
            ],
        );
        assert_eq!(rows[0].roots, ["/home/acme", "/home/acme/vol_02"]);
    }

    #[test]
    fn different_servers_stay_separate_rows_in_saved_then_target_order() {
        let hosts = [host(
            "acme@edge-db-tunnel",
            TUNNEL,
            "192.0.2.161",
            22013,
            "acme",
        )];
        let targets = [target(
            "acme@edge-ssh",
            "ssh-1",
            "192.0.2.161",
            22012,
            "acme",
        )];
        let rows = fold(&hosts, &targets, []);
        let ids: Vec<&str> = rows.iter().map(|r| r.host_id.as_str()).collect();
        assert_eq!(ids, [TUNNEL, "target:ssh-1"]);
        assert!(rows.iter().all(|r| r.native_host_id.is_none()));
    }

    #[test]
    fn aliases_of_one_server_pair_with_the_saved_host_wearing_their_label() {
        // Four `~/.ssh/config` aliases of one account on one machine, each
        // also saved as a host under the same label: four rows, not seven.
        let hosts = [
            host("acme@edge-ssh", EDGE, "192.0.2.161", 22012, "acme"),
            host("acme@edge-db-tunnel", TUNNEL, "192.0.2.161", 22012, "acme"),
        ];
        let targets = [
            target("acme@edge-db-tunnel", "ssh-2", "192.0.2.161", 22012, "acme"),
            target("acme@edge-ssh", "ssh-1", "192.0.2.161", 22012, "acme"),
        ];
        let rows = fold(&hosts, &targets, []);
        let pairs: Vec<(&str, Option<&str>)> = rows
            .iter()
            .map(|r| (r.label.as_str(), r.native_host_id.as_deref()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("acme@edge-db-tunnel", Some(TUNNEL)),
                ("acme@edge-ssh", Some(EDGE))
            ]
        );
    }

    #[test]
    fn a_target_that_only_names_a_config_alias_cannot_be_folded() {
        // With no address of its own, the target's server is whatever the
        // config says — unknown here, so both rows stay.
        let hosts = [host("acme@edge-ssh", EDGE, "192.0.2.161", 22012, "acme")];
        let targets = [target("acme@edge-ssh", "ssh-1", "", 22012, "acme")];
        assert_eq!(fold(&hosts, &targets, []).len(), 2);
    }
}
