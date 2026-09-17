//! The supply-chain layer's disk and network half (t-4502): the lockfiles a
//! workspace holds, what OSV answers about them, and the cache that keeps a
//! day's answer. Everything that is decided rather than fetched — what a
//! component is, what may be asked, what an answer means, what is drawn — is
//! `zerocode_core::supply_chain`'s.
//!
//! The answer's shape is the window's contract:
//! docs/design/knowledge-supply-chain-20260917.md §5.3.

mod cache;
mod discover;
mod osv;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use zerocode_core::supply_chain::{
    self, Ecosystem, OsvFindings, OsvRecord, SupplyError, SupplyGraph,
};

pub(crate) use osv::OsvWire;

/// One call's answer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SupplyChainAnswer {
    /// The workspace the lockfiles were read from, canonicalized.
    pub(crate) root: String,
    /// Every lockfile found, in path order, with what reading it gave.
    pub(crate) lockfiles: Vec<LockfileRead>,
    /// `components`, `vulnerabilities` and `edges`, at the top level.
    #[serde(flatten)]
    pub(crate) graph: SupplyGraph,
    pub(crate) lookup: Lookup,
}

/// One lockfile the walk found.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LockfileRead {
    /// Workspace-relative, `/`-separated.
    pub(crate) path: String,
    pub(crate) ecosystem: Ecosystem,
    /// The components it names; 0 when it could not be read.
    pub(crate) component_count: usize,
    /// Why it could not be read. The other lockfiles still stand.
    pub(crate) unreadable: Option<String>,
}

/// How the vulnerabilities in an answer were come by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LookupState {
    /// Current as of `checkedAt`, which is now: OSV answered this call, or
    /// no component may be asked about at all.
    Fresh,
    /// The cache's answer about these same lockfiles, less than a day old
    /// ([`cache::FRESH_FOR_MS`]). Nothing was sent.
    Cached,
    /// OSV could not be asked or did not answer; `reason` says which. The
    /// vulnerabilities are the last answer about these same lockfiles when
    /// there is one, as of its `checkedAt`, and none otherwise.
    Failed,
}

/// The lookup behind an answer, counted.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Lookup {
    pub(crate) state: LookupState,
    /// `offline`, `timeout`, `http_<status>` or `bad_answer` — set only when
    /// the state is `failed` ([`osv`]'s closed table).
    pub(crate) reason: Option<String>,
    /// When OSV gave the answer the vulnerabilities come from, epoch ms.
    pub(crate) checked_at: Option<i64>,
    /// Components the privacy rule lets OSV be asked about.
    pub(crate) asked: usize,
    /// `POST /v1/querybatch` requests this call sent, later pages included.
    pub(crate) batches: usize,
    /// `GET /v1/vulns/{id}` requests this call sent.
    pub(crate) records: usize,
    /// The whole call, from the walk to the graph.
    pub(crate) elapsed_ms: u64,
}

/// The directory a caller named, as a workspace root: canonical, and a
/// directory — "no such folder" is refused rather than answered with an
/// empty supply chain.
pub(crate) fn workspace_root(asked: &Path) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(asked)
        .map_err(|error| format!("공급망을 읽을 워크스페이스를 찾지 못했습니다: {error}"))?;
    if root.is_dir() {
        Ok(root)
    } else {
        Err("공급망은 워크스페이스 폴더에서 읽습니다".to_string())
    }
}

/// The supply chain of the workspace at `root`: its lockfiles read into
/// components, and — unless the cache holds a day-fresh answer about these
/// same lockfiles, or `refresh` asks past it — OSV asked about the ones that
/// may be asked. A lookup that fails still answers with every component; a
/// cache that cannot be written is a line in the window's event log under
/// `local_data_root`, not a failure of the answer.
pub(crate) fn supply_chain_answer(
    root: &Path,
    cache_root: &Path,
    local_data_root: &Path,
    wire: &OsvWire,
    refresh: bool,
    now_ms: i64,
) -> SupplyChainAnswer {
    let began = Instant::now();
    let found = discover::lockfiles(root);
    let mut lockfiles = Vec::with_capacity(found.len());
    let mut lists = Vec::with_capacity(found.len());
    for lockfile in &found {
        let read = lockfile
            .text
            .as_deref()
            .map_err(String::clone)
            .and_then(|text| {
                supply_chain::lockfile_components(lockfile.ecosystem, text, &lockfile.relative)
                    .map_err(|error| match error {
                        SupplyError::Unreadable { why, .. } | SupplyError::Answer(why) => why,
                    })
            });
        lockfiles.push(LockfileRead {
            path: lockfile.relative.clone(),
            ecosystem: lockfile.ecosystem,
            component_count: read.as_ref().map_or(0, Vec::len),
            unreadable: read.as_ref().err().cloned(),
        });
        if let Ok(components) = read {
            lists.push(components);
        }
    }
    let components = supply_chain::merge_components(lists);
    let fingerprint =
        supply_chain::lockfile_fingerprint(found.iter().filter_map(|lockfile| {
            Some((lockfile.relative.as_str(), lockfile.text.as_deref().ok()?))
        }));
    let queries = supply_chain::osv_queries(&components);
    let cache_file = cache::file(cache_root, root);
    let held = cache::read(&cache_file).filter(|held| held.fingerprint == fingerprint);

    let mut sent = osv::Sent::default();
    let (state, reason, checked_at, findings, records): (
        LookupState,
        Option<String>,
        Option<i64>,
        OsvFindings,
        Vec<OsvRecord>,
    ) = match held {
        Some(held) if !refresh && held.fresh_at(now_ms) => (
            LookupState::Cached,
            None,
            Some(held.checked_at),
            held.findings,
            held.records,
        ),
        _ if queries.is_empty() => (
            LookupState::Fresh,
            None,
            Some(now_ms),
            OsvFindings::new(),
            Vec::new(),
        ),
        held => match osv::look_up(wire, &queries, &mut sent) {
            Ok(looked) => {
                let answer =
                    cache::CachedLookup::new(fingerprint, now_ms, looked.findings, looked.records);
                if let Err(why) = cache::write(&cache_file, &answer) {
                    crate::note_window_event(
                        local_data_root,
                        &format!(
                            "supply-chain: the OSV answer for {} was not cached: {why}",
                            root.display()
                        ),
                    );
                }
                (
                    LookupState::Fresh,
                    None,
                    Some(now_ms),
                    answer.findings,
                    answer.records,
                )
            }
            Err(token) => match held {
                Some(held) => (
                    LookupState::Failed,
                    Some(token),
                    Some(held.checked_at),
                    held.findings,
                    held.records,
                ),
                None => (
                    LookupState::Failed,
                    Some(token),
                    None,
                    OsvFindings::new(),
                    Vec::new(),
                ),
            },
        },
    };
    let asked = queries.len();
    let graph = supply_chain::supply_graph(components, &findings, &records);
    SupplyChainAnswer {
        root: root.to_string_lossy().into_owned(),
        lockfiles,
        graph,
        lookup: Lookup {
            state,
            reason,
            checked_at,
            asked,
            batches: sent.batches,
            records: sent.records,
            elapsed_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
        },
    }
}
