//! Session-owned agent registry — WHERE one session's helper manifests live.
//!
//! Design: `docs/design/zo-session-agent-registry.md` (t-2507, implemented as
//! t-2511). The store used to be resolved from the process cwd on every read
//! (`labels::agent_store_dir`), so a session that entered a worktree mid-turn
//! read a different directory than the one its children were written to, and
//! every id-keyed road — stamps, stops, resumes, the roster scan — quietly
//! lost them. A registry fixes the root ONCE per session, from the cwd the
//! session was born in (`origin_cwd`), and every consumer takes the handle as
//! an argument. There is no process-global "current registry".
//!
//! Ownership, in the design's words:
//!
//! - **origin**: `root = zo_project_state_dir(origin_cwd)/agents` (or the
//!   `ZO_AGENT_STORE` override), decided when the session opens and persisted
//!   in `<root>/registries/<session-id>.json` so `/resume` from another cwd
//!   comes back to the same root.
//! - **execution cwd**: `EnterWorktree`/`ExitWorktree` and a child's own
//!   `cwd` change nothing here; they are recorded, never consulted.
//! - **read ownership**: every manifest whose `parentSessionId` is this
//!   session's, across the root and the `mirrors` adopted from legacy stores.
//! - **destructive rights**: unchanged and not weakened — stopping, reaping
//!   and resuming still require the session id, the live owner pid and the
//!   matching `run_generation` (`AgentStopOutcome`).
//!
//! The one fallback is [`AgentRegistry::unowned_from_cwd`]: unit tests,
//! hermetic runs pinned by `ZO_AGENT_STORE`, and headless utilities that call
//! tools without a session. A session process that reaches it is a bug, and
//! says so with a debug assertion ([`mark_session_process`]).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use super::labels::{AGENT_STORE_DIR_NAME, AGENT_STORE_ENV};
use super::manifest::load_agent_manifest_from_scanned_path;
use super::AgentOutput;

/// Directory under the store root that holds one record per session.
pub const REGISTRIES_DIR_NAME: &str = "registries";

/// How many project stores the one-time legacy sweep may look into when a
/// pre-registry session is first resumed. Past this the record says the
/// adoption is incomplete and a later [`AgentRegistry::adopt_store`] can
/// finish it; a missing child is never turned into a finished one.
const LEGACY_SWEEP_MAX_PROJECTS: usize = 64;

/// The persisted half — `<root>/registries/<session-id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryRecord {
    /// The session this record belongs to; also the file name.
    pub session_id: String,
    /// The cwd the session was born in — the one input `root` is derived from.
    pub origin_cwd: String,
    /// The store new spawns are written to.
    pub root: String,
    /// Legacy stores still holding this session's children, in place.
    #[serde(default)]
    pub mirrors: Vec<String>,
    /// Roster-change counter: +1 whenever the SET of running children changes,
    /// never for an activity or heartbeat update. Carried on every `subagents`
    /// frame so a reconnecting window can drop a snapshot older than a delta
    /// it already folded.
    #[serde(default)]
    pub generation: u64,
    /// Epoch seconds when the record was first written.
    #[serde(default)]
    pub created_at: u64,
    /// Where the session last ran a child, for diagnosis only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_execution_cwd: Option<String>,
    /// The bounded legacy sweep was cut by `LEGACY_SWEEP_MAX_PROJECTS`:
    /// some of this session's pre-registry children may still be unadopted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub adoption_incomplete: bool,
}

/// The mutable half, behind one lock.
#[derive(Debug)]
struct RegistryState {
    mirrors: Vec<PathBuf>,
    generation: u64,
    created_at: u64,
    last_execution_cwd: Option<PathBuf>,
    adoption_incomplete: bool,
    /// The last roster [`AgentRegistry::note_roster`] saw, so a repeated
    /// observation of the same set bumps nothing and writes nothing.
    roster: Option<BTreeSet<String>>,
}

/// One session's agent store: the root new children are written to, the
/// legacy mirrors still holding older ones, and the roster generation.
///
/// Shared as an `Arc` by the session object, its tool context, every child
/// job and the roster watcher — the handle IS the argument.
#[derive(Debug)]
pub struct AgentRegistry {
    session_id: Option<String>,
    origin_cwd: PathBuf,
    root: PathBuf,
    locator: Option<PathBuf>,
    state: Mutex<RegistryState>,
}

static SESSION_PROCESS: AtomicBool = AtomicBool::new(false);

/// Say that this process owns at least one session registry. From here on the
/// cwd fallback is a bug, and [`AgentRegistry::unowned_from_cwd`] asserts so
/// in debug builds.
pub fn mark_session_process() {
    SESSION_PROCESS.store(true, Ordering::Relaxed);
}

/// Whether [`mark_session_process`] was called.
#[must_use]
pub fn session_process_marked() -> bool {
    SESSION_PROCESS.load(Ordering::Relaxed)
}

/// The store root for a session born in `origin_cwd`: the `ZO_AGENT_STORE`
/// override when set, else the per-project state directory's `agents/`.
#[must_use]
pub fn store_root_for(origin_cwd: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os(AGENT_STORE_ENV) {
        if !path.to_string_lossy().trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    runtime::zo_project_state_dir(origin_cwd).join(AGENT_STORE_DIR_NAME)
}

/// Where a session's record lives under a store root.
#[must_use]
pub fn locator_for(root: &Path, session_id: &str) -> PathBuf {
    root.join(REGISTRIES_DIR_NAME)
        .join(format!("{session_id}.json"))
}

fn epoch_seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// A session id is about to become a file name: refuse anything that could
/// climb out of `registries/`.
fn checked_session_id(session_id: &str) -> Result<&str, String> {
    let session_id = session_id.trim();
    if session_id.is_empty()
        || session_id == "."
        || session_id == ".."
        || session_id
            .chars()
            .any(|ch| matches!(ch, '/' | '\\' | '\0') || ch.is_control())
    {
        return Err(format!("`{session_id}` is not a session id a registry can be named for"));
    }
    Ok(session_id)
}

fn load_record(path: &Path) -> Result<RegistryRecord, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// Temp-then-rename, no fsync: the record is rewritten only when the roster
/// SET changes or a mirror is adopted, never per display snapshot, and a
/// torn write is impossible with the rename while a lost last write costs
/// one generation number a reconnecting window will simply see again.
fn write_record(path: &Path, record: &RegistryRecord) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{}: registry record has no parent", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let json = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("registry"),
        std::process::id()
    ));
    std::fs::write(&temporary, json).map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("{}: {error}", path.display())
    })
}

/// Whether a store holds at least one manifest that names `session_id` as its
/// parent. A directory read plus one small JSON parse per manifest, done once
/// when a registry opens — never on a frame path.
fn store_holds_children_of(store: &Path, session_id: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(store) else {
        return false;
    };
    entries
        .flatten()
        .filter(is_manifest_entry)
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str::<ManifestParentOnly>(&text).ok())
        .any(|manifest| manifest.parent_session_id.as_deref() == Some(session_id))
}

#[derive(Deserialize)]
struct ManifestParentOnly {
    #[serde(rename = "parentSessionId")]
    parent_session_id: Option<String>,
}

fn is_manifest_entry(entry: &std::fs::DirEntry) -> bool {
    let path = entry.path();
    path.extension().and_then(std::ffi::OsStr::to_str) == Some("json")
        && !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".resume.json"))
        && entry.file_type().is_ok_and(|kind| kind.is_file())
}

fn same_store(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Every project store under the state root, for the one-time legacy sweep:
/// `<state root>/projects/*/state/agents`. Capped by the caller.
fn project_stores() -> Vec<PathBuf> {
    let projects = if let Some(dir) = std::env::var_os(core_types::paths::ZO_STATE_DIR_ENV)
        .filter(|dir| !dir.is_empty())
    {
        PathBuf::from(dir).join("projects")
    } else {
        runtime::default_config_home().join("projects")
    };
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return Vec::new();
    };
    let mut stores: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path().join("state").join(AGENT_STORE_DIR_NAME))
        .filter(|store| store.is_dir())
        .collect();
    stores.sort();
    stores
}

impl AgentRegistry {
    /// Open (or create) the registry of `session_id`, born in `origin_cwd`.
    ///
    /// `locator_hint` is what the session record remembered — the path of
    /// `registries/<sid>.json` — and wins when it still loads, so a session
    /// resumed from another directory keeps its root. Without a record the
    /// root is derived from `origin_cwd`, and a `resumed` session that
    /// predates registries gets the bounded legacy adoption: the current
    /// process cwd's store, every `EnterWorktree` stack entry's store, and a
    /// capped sweep over the project stores, each adopted as a mirror only
    /// when it actually holds this session's children.
    ///
    /// # Errors
    ///
    /// The session id cannot be a file name, or the record cannot be written.
    pub fn open_for_session(
        session_id: &str,
        origin_cwd: &Path,
        locator_hint: Option<&Path>,
        resumed: bool,
    ) -> Result<Arc<Self>, String> {
        let session_id = checked_session_id(session_id)?;
        if let Some(hint) = locator_hint {
            if let Ok(record) = load_record(hint) {
                if record.session_id == session_id {
                    return Ok(Arc::new(Self::from_record(record, hint.to_path_buf())));
                }
            }
        }
        let root = store_root_for(origin_cwd);
        let locator = locator_for(&root, session_id);
        if let Ok(record) = load_record(&locator) {
            if record.session_id == session_id {
                return Ok(Arc::new(Self::from_record(record, locator)));
            }
        }
        let registry = Self {
            session_id: Some(session_id.to_string()),
            origin_cwd: origin_cwd.to_path_buf(),
            root,
            locator: Some(locator),
            state: Mutex::new(RegistryState {
                mirrors: Vec::new(),
                generation: 0,
                created_at: epoch_seconds_now(),
                last_execution_cwd: None,
                adoption_incomplete: false,
                roster: None,
            }),
        };
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(store_root_for(&cwd));
        }
        for saved in crate::worktree_tools::saved_cwd_stack() {
            candidates.push(store_root_for(&saved));
        }
        if resumed {
            let stores = project_stores();
            if stores.len() > LEGACY_SWEEP_MAX_PROJECTS {
                registry.lock().adoption_incomplete = true;
                eprintln!(
                    "[zo] registry {session_id}: legacy sweep looked at {LEGACY_SWEEP_MAX_PROJECTS} of {} project stores; adoption is incomplete",
                    stores.len()
                );
            }
            candidates.extend(stores.into_iter().take(LEGACY_SWEEP_MAX_PROJECTS));
        }
        for candidate in candidates {
            registry.adopt_store_unpersisted(&candidate);
        }
        registry.persist()?;
        Ok(Arc::new(registry))
    }

    fn from_record(record: RegistryRecord, locator: PathBuf) -> Self {
        Self {
            session_id: Some(record.session_id),
            origin_cwd: PathBuf::from(record.origin_cwd),
            root: PathBuf::from(record.root),
            locator: Some(locator),
            state: Mutex::new(RegistryState {
                mirrors: record.mirrors.into_iter().map(PathBuf::from).collect(),
                generation: record.generation,
                created_at: record.created_at,
                last_execution_cwd: record.last_execution_cwd.map(PathBuf::from),
                adoption_incomplete: record.adoption_incomplete,
                roster: None,
            }),
        }
    }

    /// The documented fallback for code that runs with no session: the store
    /// the process cwd resolves to, no record, no mirrors, no generation.
    ///
    /// Logged once per process. In a debug build of a process that has opened
    /// a session registry ([`mark_session_process`]) this is a bug — some
    /// road forgot to carry the handle — and the assertion names it.
    #[must_use]
    pub fn unowned_from_cwd() -> Arc<Self> {
        static SAID: std::sync::Once = std::sync::Once::new();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let root = store_root_for(&cwd);
        debug_assert!(
            !session_process_marked(),
            "registry: unowned cwd store {} used inside a session process — carry the session's AgentRegistry instead",
            root.display()
        );
        SAID.call_once(|| eprintln!("registry: unowned cwd store {}", root.display()));
        Arc::new(Self {
            session_id: None,
            origin_cwd: cwd,
            root,
            locator: None,
            state: Mutex::new(RegistryState {
                mirrors: Vec::new(),
                generation: 0,
                created_at: epoch_seconds_now(),
                last_execution_cwd: None,
                adoption_incomplete: false,
                roster: None,
            }),
        })
    }

    /// A registry rooted at an explicit store, for tests and hermetic
    /// harnesses that want the session semantics without a state home.
    #[must_use]
    pub fn at_root_for_tests(session_id: &str, root: &Path) -> Arc<Self> {
        Arc::new(Self {
            session_id: Some(session_id.to_string()),
            origin_cwd: root.to_path_buf(),
            root: root.to_path_buf(),
            locator: Some(locator_for(root, session_id)),
            state: Mutex::new(RegistryState {
                mirrors: Vec::new(),
                generation: 0,
                created_at: epoch_seconds_now(),
                last_execution_cwd: None,
                adoption_incomplete: false,
                roster: None,
            }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The session this registry belongs to; `None` for the cwd fallback.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// The store new spawns are written to.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The cwd the session was born in.
    #[must_use]
    pub fn origin_cwd(&self) -> &Path {
        &self.origin_cwd
    }

    /// Where the record is persisted, when there is one.
    #[must_use]
    pub fn locator(&self) -> Option<&Path> {
        self.locator.as_deref()
    }

    /// Legacy stores adopted as mirrors, in adoption order.
    #[must_use]
    pub fn mirrors(&self) -> Vec<PathBuf> {
        self.lock().mirrors.clone()
    }

    /// Every store this registry reads: the root first, then the mirrors.
    #[must_use]
    pub fn stores(&self) -> Vec<PathBuf> {
        let mut stores = vec![self.root.clone()];
        stores.extend(self.lock().mirrors.iter().cloned());
        stores
    }

    /// The roster-change counter as it stands.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Whether the legacy sweep was cut short (see [`RegistryRecord`]).
    #[must_use]
    pub fn adoption_incomplete(&self) -> bool {
        self.lock().adoption_incomplete
    }

    /// The current record, as it would be written.
    #[must_use]
    pub fn record(&self) -> Option<RegistryRecord> {
        let session_id = self.session_id.clone()?;
        let state = self.lock();
        Some(RegistryRecord {
            session_id,
            origin_cwd: self.origin_cwd.to_string_lossy().into_owned(),
            root: self.root.to_string_lossy().into_owned(),
            mirrors: state
                .mirrors
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            generation: state.generation,
            created_at: state.created_at,
            last_execution_cwd: state
                .last_execution_cwd
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            adoption_incomplete: state.adoption_incomplete,
        })
    }

    fn persist(&self) -> Result<(), String> {
        let (Some(locator), Some(record)) = (self.locator.as_ref(), self.record()) else {
            return Ok(());
        };
        write_record(locator, &record)
    }

    /// Tell the registry which running children it has right now.
    ///
    /// Bumps `generation` and rewrites the record only when the SET of ids
    /// differs from the last one noted — an activity line or a heartbeat
    /// changing on the same children is not a roster change. Returns the
    /// generation the caller should stamp on the frame it is about to send.
    pub fn note_roster<I, S>(&self, running: I) -> u64
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let roster: BTreeSet<String> = running.into_iter().map(Into::into).collect();
        let changed = {
            let mut state = self.lock();
            if state.roster.as_ref() == Some(&roster) {
                return state.generation;
            }
            state.roster = Some(roster);
            state.generation = state.generation.saturating_add(1);
            state.generation
        };
        if let Err(error) = self.persist() {
            eprintln!("[zo] registry: could not persist generation {changed}: {error}");
        }
        changed
    }

    /// Record where a child is about to run. Diagnostic only — it never moves
    /// the root — and persisted only when it actually changes.
    pub fn note_execution_cwd(&self, cwd: &Path) {
        let changed = {
            let mut state = self.lock();
            if state.last_execution_cwd.as_deref() == Some(cwd) {
                return;
            }
            state.last_execution_cwd = Some(cwd.to_path_buf());
            true
        };
        if changed {
            let _ = self.persist();
        }
    }

    /// Adopt `store` as a mirror when it holds children of this session and
    /// is not already read. Persists the record on success. Returns whether
    /// the mirror was added.
    ///
    /// Adoption is where duplicate generations get settled: a dead `running`
    /// copy of an id whose canonical manifest lives elsewhere is stamped
    /// stopped here, so a zombie cannot resurrect on the next scan. Files are
    /// never deleted, and nothing is moved.
    ///
    /// # Errors
    ///
    /// The record could not be written.
    pub fn adopt_store(&self, store: &Path) -> Result<bool, String> {
        if !self.adopt_store_unpersisted(store) {
            return Ok(false);
        }
        self.persist()?;
        Ok(true)
    }

    fn adopt_store_unpersisted(&self, store: &Path) -> bool {
        let Some(session_id) = self.session_id.as_deref() else {
            return false;
        };
        if same_store(store, &self.root) || !store.is_dir() {
            return false;
        }
        if self
            .lock()
            .mirrors
            .iter()
            .any(|mirror| same_store(mirror, store))
        {
            return false;
        }
        if !store_holds_children_of(store, session_id) {
            return false;
        }
        self.lock().mirrors.push(store.to_path_buf());
        self.settle_duplicates();
        true
    }

    /// Stamp `stopped` onto every dead `running` duplicate that lost the
    /// canonical choice, once per adoption.
    fn settle_duplicates(&self) {
        for (canonical, others) in self.duplicate_groups() {
            for path in others {
                if same_store(&path, &canonical) {
                    continue;
                }
                let Ok(manifest) = load_agent_manifest_from_scanned_path(&path) else {
                    continue;
                };
                if manifest.status == "running" {
                    let _ = super::settle_dead_owner_agent(&manifest);
                }
            }
        }
    }

    /// Manifest file name → every store path carrying it, in store order.
    fn manifests_by_name(&self) -> BTreeMap<String, Vec<PathBuf>> {
        let mut by_name: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for store in self.stores() {
            let Ok(entries) = std::fs::read_dir(&store) else {
                continue;
            };
            for entry in entries.flatten() {
                if !is_manifest_entry(&entry) {
                    continue;
                }
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                by_name.entry(name).or_default().push(entry.path());
            }
        }
        by_name
    }

    fn duplicate_groups(&self) -> Vec<(PathBuf, Vec<PathBuf>)> {
        self.manifests_by_name()
            .into_values()
            .filter(|paths| paths.len() > 1)
            .map(|paths| (canonical_of(&paths), paths))
            .collect()
    }

    /// Every manifest file this registry reads, root first, with a duplicate
    /// id resolved to its canonical copy.
    ///
    /// A store scan, so it belongs on the blocking pool — the roster watcher
    /// calls it once a second from there, never from a paint path.
    #[must_use]
    pub fn manifest_paths(&self) -> Vec<PathBuf> {
        self.manifests_by_name()
            .into_values()
            .map(|paths| {
                if paths.len() == 1 {
                    paths.into_iter().next().expect("one path")
                } else {
                    canonical_of(&paths)
                }
            })
            .collect()
    }

    /// The canonical manifest file of `agent_id`, wherever it lives, or
    /// `None` when no store holds one. The id must already be validated as a
    /// file-name-safe agent id; this joins it to a path.
    #[must_use]
    pub fn manifest_path(&self, agent_id: &str) -> Option<PathBuf> {
        let Ok(filename) = super::manifest::expected_manifest_filename(agent_id) else {
            return None;
        };
        let candidates: Vec<PathBuf> = self
            .stores()
            .into_iter()
            .map(|store| store.join(&filename))
            .filter(|path| path.is_file())
            .collect();
        match candidates.len() {
            0 => None,
            1 => candidates.into_iter().next(),
            _ => Some(canonical_of(&candidates)),
        }
    }

    /// The store holding `agent_id`'s canonical manifest.
    #[must_use]
    pub fn store_of(&self, agent_id: &str) -> Option<PathBuf> {
        self.manifest_path(agent_id)
            .and_then(|path| path.parent().map(Path::to_path_buf))
    }

    /// `agent_id`'s canonical manifest, loaded.
    #[must_use]
    pub(crate) fn manifest_by_id(&self, agent_id: &str) -> Option<AgentOutput> {
        let path = self.manifest_path(agent_id)?;
        load_agent_manifest_from_scanned_path(&path).ok()
    }

}

/// The canonical copy among several manifests of one id (design §2):
///
/// 1. the one whose owner pid is alive AND, when it is this process, whose
///    `run_generation` is the live worker's;
/// 2. else the newest `created_at`;
/// 3. ties keep store order (root before mirrors).
fn canonical_of(paths: &[PathBuf]) -> PathBuf {
    let loaded: Vec<(PathBuf, AgentOutput)> = paths
        .iter()
        .filter_map(|path| {
            load_agent_manifest_from_scanned_path(path)
                .ok()
                .map(|manifest| (path.clone(), manifest))
        })
        .collect();
    if loaded.is_empty() {
        return paths[0].clone();
    }
    let own_pid = std::process::id();
    let foreign: HashSet<u32> = loaded
        .iter()
        .filter_map(|(_, manifest)| manifest.owner_pid)
        .filter(|pid| *pid != own_pid)
        .collect();
    let alive = if foreign.is_empty() {
        HashSet::new()
    } else {
        super::live_zo_pids(foreign.into_iter())
    };
    let owned_and_live = |manifest: &AgentOutput| match manifest.owner_pid {
        Some(pid) if pid == own_pid => {
            super::agent_worker_generation_is_live(&manifest.agent_id, manifest.run_generation)
        }
        Some(pid) => alive.contains(&pid),
        None => false,
    };
    if let Some((path, _)) = loaded.iter().find(|(_, manifest)| owned_and_live(manifest)) {
        return path.clone();
    }
    let created = |manifest: &AgentOutput| manifest.created_at.trim().parse::<u64>().unwrap_or(0);
    let newest = loaded
        .iter()
        .map(|(_, manifest)| created(manifest))
        .max()
        .unwrap_or(0);
    loaded
        .iter()
        .find(|(_, manifest)| created(manifest) == newest)
        .map_or_else(|| paths[0].clone(), |(path, _)| path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_manifest(store: &Path, id: &str, session: &str, status: &str, extra: &serde_json::Value) {
        std::fs::create_dir_all(store).expect("store");
        let mut manifest = json!({
            "agentId": id,
            "parentSessionId": session,
            "name": id,
            "description": "d",
            "status": status,
            "outputFile": store.join(format!("{id}.md")).display().to_string(),
            "manifestFile": store.join(format!("{id}.json")).display().to_string(),
            "createdAt": "100",
            "startedAt": "100",
            "runGeneration": 1,
        });
        if let (Some(object), Some(more)) = (manifest.as_object_mut(), extra.as_object()) {
            for (key, value) in more {
                object.insert(key.clone(), value.clone());
            }
        }
        std::fs::write(
            store.join(format!("{id}.json")),
            serde_json::to_vec(&manifest).expect("json"),
        )
        .expect("write");
    }

    /// Two sessions in one project keep two records, and neither reads the
    /// other's children.
    #[test]
    fn two_sessions_in_one_project_keep_separate_records() {
        let root = tempfile::tempdir().expect("root");
        let a = AgentRegistry::at_root_for_tests("session-a", root.path());
        let b = AgentRegistry::at_root_for_tests("session-b", root.path());
        a.persist().expect("persist a");
        b.persist().expect("persist b");
        assert_ne!(a.locator(), b.locator());
        assert!(root.path().join("registries/session-a.json").is_file());
        assert!(root.path().join("registries/session-b.json").is_file());
        assert_eq!(a.generation(), 0);
        assert_eq!(a.note_roster(["x"]), 1);
        assert_eq!(b.generation(), 0, "one session's roster is not the other's");
    }

    /// The generation moves only when the SET of running ids changes.
    #[test]
    fn generation_counts_roster_changes_not_observations() {
        let root = tempfile::tempdir().expect("root");
        let registry = AgentRegistry::at_root_for_tests("s", root.path());
        assert_eq!(registry.note_roster(["a1"]), 1);
        assert_eq!(registry.note_roster(["a1"]), 1, "same set, same generation");
        assert_eq!(registry.note_roster(["a1", "b1"]), 2);
        assert_eq!(registry.note_roster(["b1", "a1"]), 2, "order is not a change");
        assert_eq!(registry.note_roster(Vec::<String>::new()), 3, "an empty roster is a change");
        let record = load_record(registry.locator().expect("locator")).expect("record");
        assert_eq!(record.generation, 3, "the counter is persisted with the record");
    }

    /// A locator hint that loads wins over the cwd-derived root — `/resume`
    /// from another directory comes back to the same store.
    #[test]
    fn a_resumed_session_follows_its_locator_not_the_cwd() {
        let born = tempfile::tempdir().expect("born");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        let first = AgentRegistry::at_root_for_tests("session-r", born.path());
        first.note_roster(["a1"]);
        let locator = first.locator().expect("locator").to_path_buf();
        let resumed = AgentRegistry::open_for_session(
            "session-r",
            elsewhere.path(),
            Some(&locator),
            true,
        )
        .expect("reopen");
        assert_eq!(resumed.root(), born.path());
        assert_eq!(resumed.generation(), 1);
        assert_eq!(resumed.locator(), Some(locator.as_path()));
    }

    /// A store is adopted only when it actually holds this session's
    /// children; the root is never a mirror of itself.
    #[test]
    fn adoption_is_exact_parent_membership() {
        let root = tempfile::tempdir().expect("root");
        let legacy = tempfile::tempdir().expect("legacy");
        let stranger = tempfile::tempdir().expect("stranger");
        write_manifest(legacy.path(), "agent-1", "session-a", "running", &json!({}));
        write_manifest(stranger.path(), "agent-2", "session-b", "running", &json!({}));
        let registry = AgentRegistry::at_root_for_tests("session-a", root.path());
        assert!(registry.adopt_store(legacy.path()).expect("adopt"));
        assert!(!registry.adopt_store(stranger.path()).expect("stranger"));
        assert!(!registry.adopt_store(root.path()).expect("root"));
        assert!(!registry.adopt_store(legacy.path()).expect("again"), "idempotent");
        assert_eq!(registry.mirrors(), vec![legacy.path().to_path_buf()]);
        assert_eq!(
            registry.manifest_path("agent-1").as_deref(),
            Some(legacy.path().join("agent-1.json").as_path()),
            "an id lookup lands on the legacy file in place"
        );
        assert!(registry.manifest_path("agent-2").is_none());
        let record = load_record(registry.locator().expect("locator")).expect("record");
        assert_eq!(record.mirrors, vec![legacy.path().display().to_string()]);
    }

    /// The same id in two stores resolves to one canonical copy: the live
    /// owner wins, else the newest `createdAt`, and the scan lists it once.
    #[test]
    fn a_duplicate_id_resolves_to_the_live_owner_then_the_newest() {
        let root = tempfile::tempdir().expect("root");
        let mirror = tempfile::tempdir().expect("mirror");
        // Both dead (pid 1 is never a `zo`): the newer createdAt wins.
        write_manifest(root.path(), "agent-9", "s", "completed", &json!({"createdAt": "100", "ownerPid": 1}));
        write_manifest(mirror.path(), "agent-9", "s", "completed", &json!({"createdAt": "200", "ownerPid": 1}));
        let registry = AgentRegistry::at_root_for_tests("s", root.path());
        registry.lock().mirrors.push(mirror.path().to_path_buf());
        assert_eq!(
            registry.manifest_path("agent-9").as_deref(),
            Some(mirror.path().join("agent-9.json").as_path())
        );
        assert_eq!(registry.manifest_paths().len(), 1, "one id, one row");

        // A copy owned by THIS process with a live worker generation wins
        // over a newer dead one.
        let generation = 7;
        super::super::register_agent_cancel_signal_for_tests("agent-9", generation);
        write_manifest(
            root.path(),
            "agent-9",
            "s",
            "running",
            &json!({"createdAt": "100", "ownerPid": std::process::id(), "runGeneration": generation}),
        );
        assert_eq!(
            registry.manifest_path("agent-9").as_deref(),
            Some(root.path().join("agent-9.json").as_path())
        );
        super::super::unregister_agent_cancel_signal_for_tests("agent-9", generation);
    }

    /// An id living in an adopted legacy store is found by every id-keyed
    /// road — and the destructive guard is exactly as strict as before: a
    /// child owned by another process is `NotOwned`, never stopped. A second
    /// session's registry does not adopt the store and does not see the id.
    #[test]
    fn an_id_in_an_adopted_mirror_is_found_and_the_owner_guard_still_holds() {
        let root = tempfile::tempdir().expect("root");
        let legacy = tempfile::tempdir().expect("legacy");
        write_manifest(
            legacy.path(),
            "agent-legacy",
            "session-a",
            "running",
            &json!({"ownerPid": 1, "runGeneration": 1, "label": "scout"}),
        );
        let mine = AgentRegistry::at_root_for_tests("session-a", root.path());
        assert!(mine.adopt_store(legacy.path()).expect("adopt"));
        assert!(super::super::agent_manifest_by_id(&mine, "agent-legacy").is_some());
        assert_eq!(
            super::super::stop_agent_for_session(&mine, "agent-legacy", "session-a", "test"),
            super::super::AgentStopOutcome::NotOwned {
                name: "scout".to_string()
            },
            "found through the mirror, refused by the owner-pid guard"
        );

        let theirs = AgentRegistry::at_root_for_tests("session-b", root.path());
        assert!(!theirs.adopt_store(legacy.path()).expect("no children of b"));
        assert!(super::super::agent_manifest_by_id(&theirs, "agent-legacy").is_none());
        assert_eq!(
            super::super::stop_agent_for_session(&theirs, "agent-legacy", "session-b", "test"),
            super::super::AgentStopOutcome::NotFound
        );
    }

    /// Restore an env var on drop, so a test that pins the state root never
    /// leaks it into a sibling.
    struct EnvRestore(&'static str, Option<std::ffi::OsString>);
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match self.1.take() {
                Some(value) => std::env::set_var(self.0, value),
                None => std::env::remove_var(self.0),
            }
        }
    }

    /// S4, the bounded candidates at first open: a store the session may
    /// have spawned from before entering a worktree — the `EnterWorktree`
    /// stack entry's project store — is adopted when it holds this session's
    /// children; the sweep over every project store runs only for a RESUMED
    /// pre-registry session, and a store holding nobody's children is left
    /// alone either way.
    #[test]
    fn first_open_adopts_the_worktree_stack_store_and_sweeps_projects_only_when_resumed() {
        let _lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = tempfile::tempdir().expect("state root");
        let _state = EnvRestore(
            core_types::paths::ZO_STATE_DIR_ENV,
            std::env::var_os(core_types::paths::ZO_STATE_DIR_ENV),
        );
        let _store = EnvRestore(AGENT_STORE_ENV, std::env::var_os(AGENT_STORE_ENV));
        std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, state.path());
        std::env::remove_var(AGENT_STORE_ENV);

        let origin = tempfile::tempdir().expect("origin cwd");
        let before_hop = tempfile::tempdir().expect("cwd before EnterWorktree");
        let elsewhere = tempfile::tempdir().expect("an unrelated project");
        // The session spawned a child from `before_hop`, then entered a
        // worktree: that store is on the stack.
        let hop_store = store_root_for(before_hop.path());
        write_manifest(&hop_store, "agent-hop", "session-s4", "running", &json!({}));
        // Another project's store holds a child of this session too — only
        // the resumed sweep can find that one — and one holds a stranger's.
        let far_store = store_root_for(elsewhere.path());
        write_manifest(&far_store, "agent-far", "session-s4", "completed", &json!({}));
        let stranger = tempfile::tempdir().expect("stranger");
        let stranger_store = store_root_for(stranger.path());
        write_manifest(&stranger_store, "agent-other", "session-zz", "running", &json!({}));

        let fresh = crate::worktree_tools::with_saved_cwd_for_tests(before_hop.path(), || {
            AgentRegistry::open_for_session("session-s4", origin.path(), None, false)
                .expect("open")
        });
        assert_eq!(fresh.root(), store_root_for(origin.path()));
        assert_eq!(
            fresh.mirrors(),
            vec![hop_store.clone()],
            "the stack entry's store is adopted; the far project is not swept for a fresh session"
        );
        assert!(fresh.manifest_path("agent-hop").is_some());
        assert!(fresh.manifest_path("agent-far").is_none());
        assert!(!fresh.adoption_incomplete());

        // A resumed pre-registry session (no record yet) sweeps the projects.
        let resumed_origin = tempfile::tempdir().expect("resumed origin");
        let resumed = crate::worktree_tools::with_saved_cwd_for_tests(before_hop.path(), || {
            AgentRegistry::open_for_session("session-s4", resumed_origin.path(), None, true)
                .expect("open resumed")
        });
        let mirrors = resumed.mirrors();
        assert!(mirrors.iter().any(|m| same_store(m, &hop_store)), "{mirrors:?}");
        assert!(mirrors.iter().any(|m| same_store(m, &far_store)), "{mirrors:?}");
        assert!(
            !mirrors.iter().any(|m| same_store(m, &stranger_store)),
            "a store with nobody's children of this session is never adopted: {mirrors:?}"
        );
        assert!(resumed.manifest_path("agent-far").is_some());
        // The record persisted what was adopted, so the next open is a load.
        let again = AgentRegistry::open_for_session("session-s4", resumed_origin.path(), None, true)
            .expect("reopen");
        assert_eq!(again.mirrors().len(), mirrors.len());
    }

    /// The session id becomes a file name, so it is checked as one.
    #[test]
    fn a_session_id_that_is_not_a_file_name_is_refused() {
        let root = tempfile::tempdir().expect("root");
        for bad in ["", "..", "a/b", "a\\b"] {
            assert!(
                AgentRegistry::open_for_session(bad, root.path(), None, false).is_err(),
                "{bad:?}"
            );
        }
    }

    /// The record round-trips through the file, and an older record without
    /// the newer keys still loads.
    #[test]
    fn the_record_round_trips_and_tolerates_missing_keys() {
        let root = tempfile::tempdir().expect("root");
        let registry = AgentRegistry::at_root_for_tests("s", root.path());
        registry.note_execution_cwd(Path::new("/tmp/elsewhere"));
        let record = load_record(registry.locator().expect("locator")).expect("record");
        assert_eq!(record.last_execution_cwd.as_deref(), Some("/tmp/elsewhere"));
        assert_eq!(registry.record(), Some(record));

        let bare = root.path().join("registries/old.json");
        std::fs::write(
            &bare,
            br#"{"session_id":"old","origin_cwd":"/o","root":"/r"}"#,
        )
        .expect("write");
        let old = load_record(&bare).expect("old record loads");
        assert_eq!(old.generation, 0);
        assert!(old.mirrors.is_empty());
        assert!(!old.adoption_incomplete);
    }
}
