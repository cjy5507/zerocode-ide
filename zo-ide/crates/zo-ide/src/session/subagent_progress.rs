//! Asynchronous snapshots for the TUI's in-flight sub-agent details.
//!
//! Agent workers already persist a compact manifest beside their incremental
//! transcript. This watcher reads those files on Tokio's blocking pool and
//! sends owned snapshots to the UI task. The renderer therefore only reads
//! memory: no frame, width calculation, or paint call ever touches the disk.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tools::AgentRegistry;

/// Manifests normally refresh every few seconds while a provider streams, and
/// faster while output is landing. Five minutes is deliberately conservative:
/// it avoids turning an ordinary quiet provider request or long tool call into
/// an alarm. Even after this threshold the UI reports only the measured lack
/// of new output; it never diagnoses the agent as stuck.
const NO_NEW_OUTPUT_AFTER: Duration = Duration::from_secs(5 * 60);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(1);
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentProgress {
    pub(crate) agent_id: String,
    /// The `tool_use` id of the delegation call that spawned this helper, as
    /// the worker stamped it on its own manifest. This — not the label, which
    /// two concurrent helpers may share — is what ties a live row to the spawn
    /// cell sitting in the viewport.
    pub(crate) tool_call_id: Option<String>,
    pub(crate) label: String,
    pub(crate) model: Option<String>,
    pub(crate) activity: String,
    pub(crate) recent_tools: Vec<String>,
    /// How many tools the helper has started so far — Claude Code's
    /// "N tool uses" on a running task, the one number that says a helper
    /// is moving even between two identical activity lines.
    pub(crate) tool_calls: u64,
    pub(crate) output_tail: String,
    pub(crate) started_epoch: u64,
    pub(crate) elapsed: Duration,
    pub(crate) no_new_output_for: Option<Duration>,
    /// The helper's own transcript (`<store>/<agent_id>.session.jsonl`), once
    /// it has written one. The IDE frame carries it so a window can open the
    /// helper's conversation — the prompt it was sent first — instead of only
    /// its parent's pane.
    pub(crate) transcript_path: Option<std::path::PathBuf>,
    /// The pane this helper is RUNNING IN, when it got one of its own
    /// (`runtime::subagent_panes`). `None` for an in-process helper, which is
    /// most of them — and the difference matters to a reader: a pane id says
    /// "there is a screen you can look at", and inventing one for a thread
    /// would point at the parent's.
    pub(crate) pane: Option<String>,
    /// What the most recent `SendMessage` to this helper came to (t-2513
    /// §2.3): `consumed` — a boundary read it; `queued` — it waits for the
    /// next boundary or turn; `rejected` — nothing could take it. `None`
    /// until somebody sends. Carried so a window can say "queued since …"
    /// beside a helper instead of only "running".
    pub(crate) last_receipt: Option<runtime::subagent_panes::SteerReceiptRecord>,
    /// A tool call of the helper's is running now. A helper inside a long
    /// tool is working however long it has been quiet; one quiet past
    /// [`NO_NEW_OUTPUT_AFTER`] outside any tool may be stuck
    /// ([`Self::may_be_stuck`]).
    pub(crate) in_tool: bool,
}

impl SubagentProgress {
    /// Quiet past [`NO_NEW_OUTPUT_AFTER`] — no tool call, no output — and no
    /// tool running: the helper may be stuck (t-11354). A measured lack of
    /// activity, said as a doubt; the helper is never stopped for it.
    #[must_use]
    pub(crate) fn may_be_stuck(&self) -> bool {
        self.no_new_output_for.is_some() && !self.in_tool
    }
}

/// One observation of the session's running children, with the registry's
/// roster generation at the moment it was taken.
///
/// The generation is what a `subagents` frame carries so a window can tell a
/// stale snapshot from a fresh one (design §3): it moves only when the SET of
/// running ids moves, so two observations of the same helpers doing different
/// things share a generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RosterSnapshot {
    pub(crate) agents: Vec<SubagentProgress>,
    pub(crate) generation: u64,
}

/// Read the current session's live rows on demand (the idle Alt+A path).
///
/// Regular status rendering remains fully asynchronous through
/// [`SubagentProgressWatcher`]; this function is called only in direct response
/// to the user opening the overview, and the caller runs it on the blocking
/// pool rather than in a paint/frame path.
///
/// Reads through the SESSION's registry — its root and adopted mirrors —
/// never a store re-derived from the process cwd, so a session sitting in a
/// worktree still sees the children it spawned before entering it.
pub(crate) fn snapshot_for_session(
    registry: &AgentRegistry,
    parent_session_id: &str,
) -> Vec<SubagentProgress> {
    scan_registry(
        registry,
        parent_session_id,
        epoch_seconds_now(),
        None,
        &mut ManifestCache::shared(),
    )
}

/// This session's live helpers, in the shape the window's roster reads.
///
/// Every row here is running by construction — [`scan_store`] keeps only
/// manifests whose status says so — which is exactly what the reader needs:
/// it retires a tracked row that a COMPLETE list does not name. So a helper
/// that finished simply stops appearing, and nothing has to report its end.
pub(crate) fn background_tasks_for_session(
    registry: &AgentRegistry,
    parent_session_id: &str,
) -> Vec<crate::ide::reporter::BackgroundTask> {
    snapshot_for_session(registry, parent_session_id)
        .into_iter()
        .map(|agent| crate::ide::reporter::BackgroundTask {
            id: agent.agent_id,
            agent_type: agent.label,
            description: agent.activity,
            pane: agent.pane,
            last_receipt: agent.last_receipt.map(|record| record.receipt.as_str()),
        })
        .collect()
}

/// Owns the poller task and its latest-value channel for one foreground turn.
pub(crate) struct SubagentProgressWatcher {
    receiver: watch::Receiver<RosterSnapshot>,
    task: JoinHandle<()>,
    /// The registry the snapshots are the whole of — `None` for a test seam
    /// fed by hand, whose frames then carry no registry mark.
    registry_id: Option<String>,
}

impl SubagentProgressWatcher {
    /// Watch this foreground turn's children only (the TUI's turn floor).
    ///
    /// The floor is an INTENT, not a store boundary: the TUI shows what this
    /// turn started, while the IDE relay ([`Self::start_for_session`]) shows
    /// every running child of the session. A floored view is a subset of the
    /// registry roster, so it never moves the registry generation — only the
    /// session-wide watcher does (`docs/events-channel.md` §4.2).
    pub(crate) fn start(registry: Arc<AgentRegistry>, parent_session_id: String) -> Self {
        // The session id spans many foreground turns. Capture this boundary
        // before the turn task is spawned, then reject workers that were
        // already alive for an interrupted/finished prior turn. Without this,
        // its 19-second rows reappeared under the next turn's 0-second header.
        let turn_started_at = epoch_seconds_now();
        Self::start_since(registry, parent_session_id, Some(turn_started_at))
    }

    /// Watch every running child of this session, across foreground turns.
    ///
    /// The IDE events channel lives for the whole pane session, so it must keep
    /// reporting a detached child after the foreground turn that spawned it
    /// has ended. The TUI continues to use [`Self::start`] and its turn floor.
    /// This is the one watcher that notes the roster with the registry, so
    /// its snapshots carry a generation that moves with the roster.
    pub(crate) fn start_for_session(registry: Arc<AgentRegistry>, parent_session_id: String) -> Self {
        Self::start_since(registry, parent_session_id, None)
    }

    fn start_since(
        registry: Arc<AgentRegistry>,
        parent_session_id: String,
        started_at_floor: Option<u64>,
    ) -> Self {
        let registry_id = registry.session_id().map(str::to_string);
        let notes_roster = started_at_floor.is_none();
        let (sender, receiver) = watch::channel(RosterSnapshot::default());
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(POLL_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let cache = Arc::new(Mutex::new(ManifestCache::shared()));
            loop {
                interval.tick().await;
                let registry = Arc::clone(&registry);
                let session_id = parent_session_id.clone();
                let cache = Arc::clone(&cache);
                let scan = tokio::task::spawn_blocking(move || {
                    let mut cache = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let agents = scan_registry(
                        &registry,
                        &session_id,
                        epoch_seconds_now(),
                        started_at_floor,
                        &mut cache,
                    );
                    // Noted on the blocking pool: a roster change may rewrite
                    // the registry record, and that is a disk write nobody
                    // wants on the async core.
                    let generation = if notes_roster {
                        registry.note_roster(agents.iter().map(|agent| agent.agent_id.clone()))
                    } else {
                        registry.generation()
                    };
                    RosterSnapshot { agents, generation }
                })
                .await;
                let Ok(snapshot) = scan else {
                    // A join failure must not erase the last truthful
                    // snapshot. The next poll can recover it.
                    continue;
                };
                if sender.is_closed() {
                    return;
                }
                sender.send_if_modified(|current| {
                    if *current == snapshot {
                        return false;
                    }
                    *current = snapshot;
                    true
                });
            }
        });
        Self {
            receiver,
            task,
            registry_id,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_receiver(receiver: watch::Receiver<RosterSnapshot>) -> Self {
        let task = tokio::spawn(std::future::pending::<()>());
        Self {
            receiver,
            task,
            registry_id: None,
        }
    }

    /// A test seam whose frames say which registry they are the whole of.
    #[cfg(test)]
    pub(crate) fn from_receiver_for_registry(
        receiver: watch::Receiver<RosterSnapshot>,
        registry_id: &str,
    ) -> Self {
        let mut watcher = Self::from_receiver(receiver);
        watcher.registry_id = Some(registry_id.to_string());
        watcher
    }

    /// The registry these snapshots are the whole of, when the watcher has one.
    pub(crate) fn registry_id(&self) -> Option<&str> {
        self.registry_id.as_deref()
    }

    /// Wait until the collector publishes a different snapshot.
    pub(crate) async fn changed(&mut self) -> Option<Vec<SubagentProgress>> {
        self.changed_roster().await.map(|snapshot| snapshot.agents)
    }

    /// [`Self::changed`] with the generation the snapshot was taken at.
    pub(crate) async fn changed_roster(&mut self) -> Option<RosterSnapshot> {
        self.receiver.changed().await.ok()?;
        Some(self.receiver.borrow_and_update().clone())
    }
}

impl Drop for SubagentProgressWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Deserialize)]
struct AgentManifest {
    #[serde(default)]
    execution: Option<String>,
    #[serde(rename = "subagentType")]
    subagent_type: Option<String>,
    #[serde(rename = "agentId")]
    agent_id: String,
    #[serde(rename = "parentSessionId")]
    parent_session_id: Option<String>,
    #[serde(rename = "toolCallId")]
    tool_call_id: Option<String>,
    name: String,
    label: Option<String>,
    model: Option<String>,
    #[serde(rename = "resolvedModel")]
    resolved_model: Option<String>,
    status: String,
    #[serde(rename = "startedAt")]
    started_at: Option<String>,
    #[serde(rename = "currentTool")]
    current_tool: Option<String>,
    #[serde(rename = "currentPhase")]
    current_phase: Option<String>,
    #[serde(rename = "recentTools", default)]
    recent_tools: Vec<String>,
    #[serde(rename = "toolCalls", default)]
    tool_calls: u64,
    #[serde(rename = "lastActivityAt")]
    last_activity_at: Option<u64>,
    #[serde(rename = "outputTail", default)]
    output_tail: String,
    #[serde(default)]
    pane: Option<String>,
    /// The manifest's `lastReceipt` (t-2513 §2.3), one of the lifecycle keys
    /// the tools crate flattens onto the manifest.
    #[serde(rename = "lastReceipt", default)]
    last_receipt: Option<runtime::subagent_panes::SteerReceiptRecord>,
    /// A pane child's own session transcript, once a result of its named it
    /// (another flattened lifecycle key).
    #[serde(default)]
    transcript: Option<PathBuf>,
}

/// What this process last read of each manifest file, whichever watcher
/// read it.
///
/// A project's store keeps every helper it ever ran — 432 manifests in the
/// person's zerocode project on 2026-09-28, 2,875 in another — and a watcher
/// asks every second, twice during a turn (the turn's own watcher and the IDE
/// relay's). Reading and parsing all of them each time cost 2.6% CPU at idle
/// with 432 in the store; a manifest whose file has not changed says what it
/// said last time.
#[derive(Default)]
struct ManifestFiles {
    files: HashMap<PathBuf, CachedManifest>,
}

/// One manifest file as last read: what identifies its bytes, and — for a
/// running helper — what they said. `None` is a file that is no row: not a
/// manifest, or one whose helper has settled, which only a rewrite (a new
/// stamp) could change. Only running manifests are kept whole.
struct CachedManifest {
    stamp: FileStamp,
    manifest: Option<Arc<AgentManifest>>,
}

/// Length, time and (on unix) inode: a rewrite through a temporary file and
/// a rename — how the tools crate writes every manifest — is a new inode even
/// when it lands within the clock's resolution at the same length.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    inode: u64,
}

impl FileStamp {
    fn of(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(metadata);
        #[cfg(not(unix))]
        let inode = 0;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            inode,
        }
    }
}

impl ManifestFiles {
    /// The manifest at `path`, its file read (and counted in `reads`) only
    /// when it changed since anybody last read it.
    fn manifest(&mut self, path: &Path, metadata: &std::fs::Metadata, reads: &mut usize) -> Option<Arc<AgentManifest>> {
        let stamp = FileStamp::of(metadata);
        if let Some(cached) = self.files.get(path) {
            if cached.stamp == stamp {
                return cached.manifest.clone();
            }
        }
        *reads += 1;
        let manifest = read_manifest(path, metadata)
            .filter(|manifest| manifest.status == "running")
            .map(Arc::new);
        self.files.insert(
            path.to_path_buf(),
            CachedManifest {
                stamp,
                manifest: manifest.clone(),
            },
        );
        manifest
    }

    /// Forget files of the listed stores that the listing did not see — a
    /// swept manifest must not be remembered forever. Another store's files
    /// are another watcher's to keep. The guard goes with it: the listing is
    /// over.
    fn keep_only(
        mut table: std::sync::MutexGuard<'_, Self>,
        seen: &HashSet<PathBuf>,
        stores: &[(PathBuf, Option<SystemTime>)],
    ) {
        table.files.retain(|path, _| {
            seen.contains(path)
                || !path
                    .parent()
                    .is_some_and(|parent| stores.iter().any(|(store, _)| store == parent))
        });
    }
}

/// One watcher's view of the stores: the files every watcher shares, and
/// whether its own last listing found nothing running — which depends on the
/// watcher, since a turn's watcher leaves out what started before its turn.
#[derive(Default)]
pub(crate) struct ManifestCache {
    files: Arc<Mutex<ManifestFiles>>,
    /// Each store directory's time at the last listing, kept while that
    /// listing found nothing of this session running.
    quiet: Option<Vec<(PathBuf, Option<SystemTime>)>>,
    /// Listings skipped since the last one made.
    skipped: u32,
    /// Manifest files this watcher read since the last `take_reads`.
    reads: usize,
    /// Store listings since the last `take_listings` (the tests' count).
    listings: usize,
}

/// A quiet store is listed anyway once in this many scans — the bound on how
/// long a change that moved no directory's time could go unseen.
const QUIET_RELIST_EVERY: u32 = 30;

impl ManifestCache {
    /// A watcher's cache over what every watcher of this process has read.
    pub(crate) fn shared() -> Self {
        static FILES: OnceLock<Arc<Mutex<ManifestFiles>>> = OnceLock::new();
        Self {
            files: Arc::clone(FILES.get_or_init(Arc::default)),
            ..Self::default()
        }
    }

    /// The stores and their times when this scan must list them; `None` when
    /// nothing of this session ran at the last listing and no store directory
    /// has moved since. The tools crate publishes every manifest by a rename,
    /// which moves its directory's time, so there is nothing new to read.
    fn listing_due(&mut self, registry: &AgentRegistry) -> Option<Vec<(PathBuf, Option<SystemTime>)>> {
        let stores = store_times(registry);
        if self.quiet.as_ref() == Some(&stores) && self.skipped < QUIET_RELIST_EVERY {
            self.skipped += 1;
            return None;
        }
        self.listings += 1;
        Some(stores)
    }

    /// What a listing found: the stores' times, kept only when nothing of
    /// this session was running.
    fn settle(&mut self, stores: Vec<(PathBuf, Option<SystemTime>)>, nothing_running: bool) {
        self.quiet = nothing_running.then_some(stores);
        self.skipped = 0;
    }

    #[cfg(test)]
    fn take_reads(&mut self) -> usize {
        std::mem::take(&mut self.reads)
    }

    #[cfg(test)]
    fn take_listings(&mut self) -> usize {
        std::mem::take(&mut self.listings)
    }

    #[cfg(test)]
    fn remembered(&self) -> usize {
        self.files.lock().expect("manifest files").files.len()
    }

    #[cfg(test)]
    fn bodies_kept(&self) -> usize {
        let files = self.files.lock().expect("manifest files");
        files.files.values().filter(|cached| cached.manifest.is_some()).count()
    }
}

/// Each store directory of `registry` with its own modification time.
fn store_times(registry: &AgentRegistry) -> Vec<(PathBuf, Option<SystemTime>)> {
    registry
        .stores()
        .into_iter()
        .map(|store| {
            let modified = std::fs::metadata(&store).and_then(|metadata| metadata.modified()).ok();
            (store, modified)
        })
        .collect()
}

fn read_manifest(path: &Path, metadata: &std::fs::Metadata) -> Option<AgentManifest> {
    if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<AgentManifest>(&contents).ok()
}

/// Every running child of `parent_session_id` across the registry's stores
/// (root first, then adopted mirrors; a duplicated id resolved to its canonical
/// copy by the registry), sorted by `(started_at, agent_id)` — a stable order
/// that two same-labelled helpers cannot swap.
fn scan_registry(
    registry: &AgentRegistry,
    parent_session_id: &str,
    now_epoch_seconds: u64,
    started_at_floor: Option<u64>,
    cache: &mut ManifestCache,
) -> Vec<SubagentProgress> {
    let Some(stores) = cache.listing_due(registry) else {
        return Vec::new();
    };
    let mut progress = Vec::new();
    let mut seen = HashSet::new();
    let mut files = cache.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for path in registry.manifest_paths() {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let manifest = files.manifest(&path, &metadata, &mut cache.reads);
        seen.insert(path.clone());
        let Some(manifest) = manifest else {
            continue;
        };
        if manifest.status != "running"
            || manifest.execution.as_deref() == Some("ledger")
            || manifest.subagent_type.as_deref() == Some("classifier")
            || manifest.parent_session_id.as_deref() != Some(parent_session_id)
        {
            continue;
        }

        let started_at = manifest
            .started_at
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(now_epoch_seconds);
        if started_at_floor.is_some_and(|floor| started_at < floor) {
            continue;
        }
        // A helper in a pane of its own writes no progress into its
        // manifest: its work is in its own session transcript, which it named
        // when it started (t-11354 — the row read "0 tool uses · no new output
        // for 33m" while that transcript grew by sixty requests).
        let pane_transcript = manifest
            .pane
            .is_some()
            .then(|| {
                pane_transcript_path(&path, &manifest.agent_id)
                    .or_else(|| manifest.transcript.clone())
            })
            .flatten();
        let tally = pane_transcript.as_deref().and_then(transcript_tally);
        let transcript_path =
            pane_transcript.or_else(|| transcript_path_for(&path, &manifest.agent_id));
        let last_output_at = transcript_path
            .as_deref()
            .and_then(modified_epoch_of)
            .or(manifest.last_activity_at)
            .unwrap_or(started_at);
        let quiet = Duration::from_secs(now_epoch_seconds.saturating_sub(last_output_at));
        // `activity` means observable runtime state, not the task's static
        // assignment. `description` never changes and made a live row flicker
        // between "Read …" and "summarize Cargo.toml" whenever the worker
        // cleared currentTool between calls. Prefer the detailed matching tool
        // stamp, then the live tool/phase, and use a neutral fact when none has
        // landed yet.
        // A pane child's running call, when its transcript shows one, is
        // what it is doing; the manifest has nothing newer for it.
        let running_tool = tally.as_ref().and_then(|tally| tally.running.clone());
        let activity = running_tool
            .clone()
            .or_else(|| {
                manifest
                    .current_tool
                    .as_deref()
                    .and_then(|tool| {
                        manifest
                            .recent_tools
                            .last()
                            .filter(|recent| recent.starts_with(tool))
                    })
                    .cloned()
            })
            .or_else(|| manifest.current_tool.clone())
            .or_else(|| manifest.current_phase.clone())
            .unwrap_or_else(|| "working".to_string());
        let label = manifest
            .label
            .clone()
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| manifest.name.clone());
        progress.push(SubagentProgress {
            agent_id: manifest.agent_id.clone(),
            tool_call_id: manifest.tool_call_id.clone(),
            label,
            model: manifest.resolved_model.clone().or_else(|| manifest.model.clone()),
            activity,
            recent_tools: manifest.recent_tools.clone(),
            tool_calls: tally.as_ref().map_or(manifest.tool_calls, |tally| tally.tool_calls),
            output_tail: manifest.output_tail.clone(),
            started_epoch: started_at,
            elapsed: Duration::from_secs(now_epoch_seconds.saturating_sub(started_at)),
            no_new_output_for: (quiet >= NO_NEW_OUTPUT_AFTER).then_some(quiet),
            transcript_path,
            pane: manifest.pane.clone(),
            last_receipt: manifest.last_receipt.clone(),
            in_tool: if tally.is_some() {
                running_tool.is_some()
            } else {
                manifest.current_tool.is_some()
            },
        });
    }
    ManifestFiles::keep_only(files, &seen, &stores);
    cache.settle(stores, progress.is_empty());
    progress.sort_by(|left, right| {
        left.started_epoch
            .cmp(&right.started_epoch)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    progress
}

/// The test seam the store-level scan keeps: one directory, no registry.
#[cfg(test)]
fn scan_store(
    store: &Path,
    parent_session_id: &str,
    now_epoch_seconds: u64,
    started_at_floor: Option<u64>,
) -> Vec<SubagentProgress> {
    let registry = AgentRegistry::at_root_for_tests(parent_session_id, store);
    scan_registry(
        &registry,
        parent_session_id,
        now_epoch_seconds,
        started_at_floor,
        &mut ManifestCache::default(),
    )
}

/// Where a pane child said its session transcript is
/// (`<store>/<agent_id>/`[`runtime::subagent_panes::TRANSCRIPT_FILE`]).
fn pane_transcript_path(manifest_path: &Path, agent_id: &str) -> Option<PathBuf> {
    runtime::subagent_panes::named_transcript(&manifest_path.parent()?.join(agent_id))
}

/// A transcript's marks for a tool call, a tool result and a prompt. The
/// session writes compact JSON, so none can occur inside a string, where
/// quotes are escaped.
const TOOL_USE_MARK: &str = "\"type\":\"tool_use\"";
const TOOL_RESULT_MARK: &str = "\"type\":\"tool_result\"";
const PROMPT_MARK: &str = "\"role\":\"user\"";

/// What a pane child's transcript says of its work in this run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TranscriptTally {
    /// Tool calls since the run's prompt — beside the run's elapsed time, the
    /// count an in-process helper's row shows.
    tool_calls: u64,
    /// The call it is making now: the last one with no result yet.
    running: Option<String>,
}

/// One transcript file read.
#[derive(Debug, Clone)]
struct FileTally {
    uses: u64,
    results: u64,
    /// Tool calls after the file's last prompt — all of them when it has none.
    uses_since_prompt: u64,
    /// The file holds a prompt, so the run began in it or after.
    prompted: bool,
    last_tool: Option<String>,
}

/// The run's tally from `transcript` and, when the run began before a
/// compaction rotated the file, its rotated siblings (`<stem>.rot-<ms>.jsonl`)
/// newest first, back to the prompt. Each file is read again only when its
/// size or time changes — the watcher asks every second, the files change
/// every few.
fn transcript_tally(transcript: &Path) -> Option<TranscriptTally> {
    let live = file_tally(transcript)?;
    let mut tool_calls = live.uses_since_prompt;
    if !live.prompted {
        let stem = transcript.file_stem()?.to_str()?;
        let rotated_prefix = format!("{stem}.rot-");
        let mut rotated: Vec<(u64, PathBuf)> = std::fs::read_dir(transcript.parent()?)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter_map(|path| {
                let at = path
                    .file_name()?
                    .to_str()?
                    .strip_prefix(&rotated_prefix)?
                    .strip_suffix(".jsonl")?
                    .parse::<u64>()
                    .ok()?;
                Some((at, path))
            })
            .collect();
        rotated.sort_unstable_by_key(|(at, _)| std::cmp::Reverse(*at));
        for (_, path) in rotated {
            let Some(tally) = file_tally(&path) else {
                break;
            };
            tool_calls += tally.uses_since_prompt;
            if tally.prompted {
                break;
            }
        }
    }
    Some(TranscriptTally {
        tool_calls,
        running: (live.uses > live.results).then_some(live.last_tool).flatten(),
    })
}

/// Each transcript file's last read: its size and time then, and its tally.
type SeenTallies = Mutex<HashMap<PathBuf, (u64, SystemTime, FileTally)>>;

fn file_tally(path: &Path) -> Option<FileTally> {
    static SEEN: OnceLock<SeenTallies> = OnceLock::new();
    let metadata = std::fs::metadata(path).ok()?;
    let stamp = (metadata.len(), metadata.modified().ok()?);
    let seen = SEEN.get_or_init(Mutex::default);
    if let Some((len, modified, tally)) = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(path)
    {
        if (*len, *modified) == stamp {
            return Some(tally.clone());
        }
    }
    let text = std::fs::read_to_string(path).ok()?;
    let prompt_at = text.rfind(PROMPT_MARK);
    let since_prompt = prompt_at.map_or(text.as_str(), |at| &text[at..]);
    let tally = FileTally {
        uses: text.matches(TOOL_USE_MARK).count() as u64,
        results: text.matches(TOOL_RESULT_MARK).count() as u64,
        uses_since_prompt: since_prompt.matches(TOOL_USE_MARK).count() as u64,
        prompted: prompt_at.is_some(),
        last_tool: text
            .lines()
            .rev()
            .find(|line| line.contains(TOOL_USE_MARK))
            .and_then(last_tool_in_line),
    };
    seen.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(path.to_path_buf(), (stamp.0, stamp.1, tally.clone()));
    Some(tally)
}

/// The name of the last tool call in one transcript line.
fn last_tool_in_line(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value
        .pointer("/message/blocks")?
        .as_array()?
        .iter()
        .rev()
        .find(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use"))?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

/// Where this helper's own transcript is, beside its manifest
/// (`<store>/<agent_id>.session.jsonl`) — `None` until it has written one.
fn transcript_path_for(manifest_path: &Path, agent_id: &str) -> Option<std::path::PathBuf> {
    let transcript = manifest_path
        .parent()?
        .join(format!("{agent_id}.session.jsonl"));
    transcript.is_file().then_some(transcript)
}

fn modified_epoch_of(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn epoch_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{epoch_seconds_now, scan_registry, scan_store, ManifestCache, NO_NEW_OUTPUT_AFTER};

    /// One line of a zo session transcript, as the session writes it.
    fn transcript_line(role: &str, blocks: &serde_json::Value) -> String {
        format!(
            "{}\n",
            json!({"message": {"blocks": blocks, "role": role}, "type": "message"})
        )
    }

    #[test]
    fn ledger_workers_are_not_announced_as_helpers_inside_the_parent_tab() {
        let root = tempfile::tempdir().unwrap();
        for (id, execution) in [("agent-native", "pane"), ("agent-ledger", "ledger")] {
            fs::write(root.path().join(format!("{id}.json")), serde_json::json!({
                "agentId":id, "name":id, "parentSessionId":"session-ledger",
                "status":"running", "execution":execution, "startedAt":"100"
            }).to_string()).unwrap();
        }
        let rows = scan_store(root.path(), "session-ledger", 200, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent_id, "agent-native");
    }

    /// A helper running in a pane of its own writes no progress into its
    /// manifest: its work is in its own session transcript (t-11354 —
    /// board3d-impl read "0 tool uses · 33m 34s · working · no new output for
    /// 33m 34s" while its session made 60 model requests). The row reads that
    /// transcript, wherever the pane said it is: the tool calls it made, the
    /// one running now, and when it last wrote.
    #[test]
    fn a_pane_helpers_row_reads_the_transcript_it_named() {
        let store = tempfile::tempdir().expect("store");
        let sessions = tempfile::tempdir().expect("sessions");
        let transcript = sessions.path().join("session-child.jsonl");
        let mut lines = transcript_line("user", &json!([{"type": "text", "text": "the brief"}]));
        lines += &transcript_line(
            "assistant",
            &json!([
                {"id": "t1", "input": "{}", "name": "bash", "type": "tool_use"},
                {"id": "t2", "input": "{}", "name": "read_file", "type": "tool_use"}
            ]),
        );
        for (id, name) in [("t1", "bash"), ("t2", "read_file")] {
            lines += &transcript_line(
                "tool",
                &json!([{"is_error": false, "output": "ok", "tool_name": name, "tool_use_id": id, "type": "tool_result"}]),
            );
        }
        lines += &transcript_line("system", &json!([{"type": "text", "text": "<system-reminder>"}]));
        lines += &transcript_line(
            "assistant",
            &json!([{"id": "t3", "input": "{\"command\":\"node ui/tests/board-orbit.mjs\"}", "name": "bash", "type": "tool_use"}]),
        );
        fs::write(&transcript, lines).expect("transcript");
        fs::write(
            store.path().join("agent-pane.json"),
            serde_json::to_vec(&json!({
                "agentId": "agent-pane", "parentSessionId": "session-a", "name": "board3d-impl",
                "status": "running", "startedAt": "100", "lastActivityAt": 100,
                "execution": "pane", "pane": "%3"
            }))
            .expect("json"),
        )
        .expect("manifest");
        let directory = store.path().join("agent-pane");
        fs::create_dir_all(&directory).expect("agent directory");
        fs::write(
            directory.join(runtime::subagent_panes::TRANSCRIPT_FILE),
            transcript.display().to_string(),
        )
        .expect("the pane names its transcript");

        let now = epoch_seconds_now() + 30;
        let rows = scan_store(store.path(), "session-a", now, None);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.tool_calls, 3, "the calls in its transcript: {row:?}");
        assert!(row.in_tool, "its last call has no result yet: {row:?}");
        assert_eq!(row.activity, "bash", "{row:?}");
        assert_eq!(row.no_new_output_for, None, "it wrote seconds ago: {row:?}");
        assert!(!row.may_be_stuck());
        assert_eq!(row.transcript_path.as_deref(), Some(transcript.as_path()));
    }

    /// Quiet past the bar with no tool running may be stuck; the same quiet
    /// inside a tool call is a long tool, and is only reported as quiet.
    #[test]
    fn a_helper_quiet_past_the_bar_outside_any_tool_may_be_stuck() {
        let store = tempfile::tempdir().expect("store");
        for (id, current_tool) in [("agent-silent", None), ("agent-in-a-tool", Some("bash"))] {
            let mut manifest = json!({
                "agentId": id, "parentSessionId": "session-a", "name": id,
                "status": "running", "startedAt": "100", "lastActivityAt": 100
            });
            if let Some(tool) = current_tool {
                manifest["currentTool"] = json!(tool);
            }
            fs::write(
                store.path().join(format!("{id}.json")),
                serde_json::to_vec(&manifest).expect("json"),
            )
            .expect("manifest");
        }
        let now = 100 + NO_NEW_OUTPUT_AFTER.as_secs() + 1;
        let rows = scan_store(store.path(), "session-a", now, None);
        let row = |id: &str| {
            rows.iter()
                .find(|row| row.agent_id == id)
                .unwrap_or_else(|| panic!("{id} is not a row"))
        };
        assert!(row("agent-silent").may_be_stuck(), "{:?}", row("agent-silent"));
        assert!(row("agent-in-a-tool").no_new_output_for.is_some());
        assert!(
            !row("agent-in-a-tool").may_be_stuck(),
            "a helper inside a tool call is working"
        );
    }

    #[test]
    fn internal_classifiers_do_not_become_visible_workers() {
        let temp = tempfile::tempdir().expect("tempdir");
        for (id, role) in [("planner", "classifier"), ("reviewer", "general-purpose")] {
            fs::write(temp.path().join(format!("{id}.json")), serde_json::to_vec(&json!({
                "agentId": id, "parentSessionId": "parent", "name": id,
                "subagentType": role, "status": "running", "startedAt": "100"
            })).expect("json")).expect("manifest");
        }
        let rows = scan_store(temp.path(), "parent", 200, None);
        assert_eq!(rows.iter().map(|row| row.agent_id.as_str()).collect::<Vec<_>>(), ["reviewer"]);
    }

    #[test]
    fn scan_is_session_scoped_and_reports_only_running_agents() {
        let temp = tempfile::tempdir().expect("tempdir");
        let write = |id: &str, session: &str, status: &str| {
            let path = temp.path().join(format!("{id}.json"));
            fs::write(
                path,
                serde_json::to_vec(&json!({
                    "agentId": id,
                    "parentSessionId": session,
                    "name": id,
                    "description": "inspect the renderer",
                    "resolvedModel": "gpt-5.6-sol",
                    "status": status,
                    "startedAt": "100",
                    "currentTool": "Read",
                    "recentTools": ["Read · src/tui/view.rs"],
                    "outputTail": "found the shared picker",
                    "lastActivityAt": 190,
                }))
                .expect("manifest json"),
            )
            .expect("write manifest");
        };
        write("agent-live", "session-a", "running");
        write("agent-done", "session-a", "completed");
        write("agent-other", "session-b", "running");
        fs::write(temp.path().join("agent-live.session.jsonl"), "{}\n").expect("write transcript");

        let rows = scan_store(temp.path(), "session-a", 200, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent_id, "agent-live");
        assert_eq!(rows[0].label, "agent-live");
        assert_eq!(rows[0].model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(rows[0].activity, "Read · src/tui/view.rs");
        assert_eq!(rows[0].recent_tools, ["Read · src/tui/view.rs"]);
        assert_eq!(rows[0].output_tail, "found the shared picker");
        assert_eq!(rows[0].started_epoch, 100);
        assert_eq!(rows[0].elapsed.as_secs(), 100);
        assert!(rows[0].no_new_output_for.is_none());
        assert_eq!(
            rows[0].transcript_path.as_deref(),
            Some(temp.path().join("agent-live.session.jsonl").as_path()),
            "the row names the helper's own transcript"
        );
    }

    /// A helper that got a pane of its own carries its pane id, and one that
    /// is a thread of this process carries none.
    ///
    /// A row is what the window's roster and the events frame are drawn from,
    /// so this is where "there is a second screen to open" is either a fact or
    /// absent. A legacy manifest — every one written before panes existed —
    /// simply has no key, which must read as `None` rather than fail the scan.
    #[test]
    fn a_row_says_which_pane_its_helper_runs_in_when_it_has_one() {
        let temp = tempfile::tempdir().expect("tempdir");
        let write = |id: &str, extra: serde_json::Value| {
            let mut manifest = json!({
                "agentId": id,
                "parentSessionId": "session-a",
                "name": id,
                "description": "inspect the renderer",
                "status": "running",
                "startedAt": "100",
            });
            if let (Some(object), Some(more)) = (manifest.as_object_mut(), extra.as_object()) {
                for (key, value) in more {
                    object.insert(key.clone(), value.clone());
                }
            }
            fs::write(
                temp.path().join(format!("{id}.json")),
                serde_json::to_vec(&manifest).expect("manifest json"),
            )
            .expect("write manifest");
        };
        write("agent-in-a-pane", json!({"pane": "%4"}));
        write("agent-in-a-thread", json!({}));

        let rows = scan_store(temp.path(), "session-a", 200, None);
        let pane_of = |id: &str| {
            rows.iter()
                .find(|row| row.agent_id == id)
                .unwrap_or_else(|| panic!("{id} is not a row"))
                .pane
                .clone()
        };
        assert_eq!(pane_of("agent-in-a-pane").as_deref(), Some("%4"));
        assert_eq!(pane_of("agent-in-a-thread"), None);
    }

    /// The row must carry the `tool_use` id its worker stamped: two helpers of
    /// one delegation can share a label, so only this ties a row to the spawn
    /// cell that announced it.
    #[test]
    fn a_row_carries_the_spawn_call_that_announced_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("agent-live.json"),
            serde_json::to_vec(&json!({
                "agentId": "agent-live",
                "parentSessionId": "session-a",
                "toolCallId": "toolu_01",
                "name": "scout",
                "description": "inspect the renderer",
                "status": "running",
                "startedAt": "100",
                "toolCalls": 12,
            }))
            .expect("manifest json"),
        )
        .expect("write manifest");

        let rows = scan_store(temp.path(), "session-a", 200, None);
        assert_eq!(rows[0].tool_call_id.as_deref(), Some("toolu_01"));
        assert_eq!(rows[0].tool_calls, 12);
    }

    /// A manifest written before the id was stamped (and a host-spawned agent,
    /// which has no spawn call at all) must still produce a row.
    #[test]
    fn a_row_without_a_spawn_call_is_still_a_row() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("agent-live.json"),
            serde_json::to_vec(&json!({
                "agentId": "agent-live",
                "parentSessionId": "session-a",
                "name": "scout",
                "description": "inspect the renderer",
                "status": "running",
                "startedAt": "100",
            }))
            .expect("manifest json"),
        )
        .expect("write manifest");

        let rows = scan_store(temp.path(), "session-a", 200, None);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].tool_call_id.is_none());
        assert_eq!(rows[0].tool_calls, 0);
    }

    /// t-2511: the roster is the union of the session's root and an adopted
    /// legacy store — a child spawned before the session entered a worktree
    /// and one spawned after are ONE roster, ordered by start, each with the
    /// transcript beside its own manifest — and a second session sharing the
    /// same root sees only its own child.
    #[test]
    fn the_roster_merges_the_root_and_an_adopted_mirror_and_orders_by_start() {
        let root = tempfile::tempdir().expect("root");
        let legacy = tempfile::tempdir().expect("legacy");
        let write = |store: &std::path::Path, id: &str, session: &str, started: &str, label: &str| {
            fs::write(
                store.join(format!("{id}.json")),
                serde_json::to_vec(&json!({
                    "agentId": id,
                    "parentSessionId": session,
                    "name": id,
                    "label": label,
                    "description": "d",
                    "status": "running",
                    "startedAt": started,
                    "createdAt": started,
                    "outputFile": store.join(format!("{id}.md")).display().to_string(),
                    "manifestFile": store.join(format!("{id}.json")).display().to_string(),
                }))
                .expect("json"),
            )
            .expect("write");
            fs::write(store.join(format!("{id}.session.jsonl")), "{}\n").expect("transcript");
        };
        // Spawned from A before the worktree hop, in the legacy store …
        write(legacy.path(), "agent-a1", "session-a", "100", "zeta");
        // … and from B after it, in the session's root.
        write(root.path(), "agent-b1", "session-a", "200", "alpha");
        // Another session's child in the same root.
        write(root.path(), "agent-c1", "session-b", "150", "other");

        let registry = tools::AgentRegistry::at_root_for_tests("session-a", root.path());
        assert!(registry.adopt_store(legacy.path()).expect("adopt the legacy store"));
        let rows = super::snapshot_for_session(&registry, "session-a");
        assert_eq!(
            rows.iter().map(|row| row.agent_id.as_str()).collect::<Vec<_>>(),
            ["agent-a1", "agent-b1"],
            "ordered by start, not by label (zeta before alpha)"
        );
        assert_eq!(
            rows[0].transcript_path.as_deref(),
            Some(legacy.path().join("agent-a1.session.jsonl").as_path()),
            "the legacy child's transcript is still beside its manifest, in place"
        );
        assert_eq!(
            rows[1].transcript_path.as_deref(),
            Some(root.path().join("agent-b1.session.jsonl").as_path())
        );

        let other = tools::AgentRegistry::at_root_for_tests("session-b", root.path());
        let rows = super::snapshot_for_session(&other, "session-b");
        assert_eq!(
            rows.iter().map(|row| row.agent_id.as_str()).collect::<Vec<_>>(),
            ["agent-c1"],
            "a concurrent session in the same project sees only its own children"
        );
    }

    /// Two helpers that started in the same second keep a stable order by id;
    /// the label — which two helpers may share — never decides.
    #[test]
    fn rows_order_by_start_then_id_never_by_label() {
        let temp = tempfile::tempdir().expect("tempdir");
        for (id, started, label) in [
            ("agent-2", "100", "same"),
            ("agent-1", "100", "same"),
            ("agent-0", "300", "aaa"),
        ] {
            fs::write(
                temp.path().join(format!("{id}.json")),
                serde_json::to_vec(&json!({
                    "agentId": id,
                    "parentSessionId": "session-a",
                    "name": id,
                    "label": label,
                    "description": "d",
                    "status": "running",
                    "startedAt": started,
                }))
                .expect("json"),
            )
            .expect("write");
        }
        let rows = scan_store(temp.path(), "session-a", 400, None);
        assert_eq!(
            rows.iter().map(|row| row.agent_id.as_str()).collect::<Vec<_>>(),
            ["agent-1", "agent-2", "agent-0"]
        );
    }

    #[test]
    fn inactivity_is_a_fact_only_after_the_conservative_threshold() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("agent-quiet.json"),
            serde_json::to_vec(&json!({
                "agentId": "agent-quiet",
                "parentSessionId": "session-a",
                "name": "quiet",
                "description": "verify the gates",
                "status": "running",
                "startedAt": "100",
                "lastActivityAt": 200,
            }))
            .expect("manifest json"),
        )
        .expect("write manifest");

        let before = scan_store(
            temp.path(),
            "session-a",
            200 + NO_NEW_OUTPUT_AFTER.as_secs() - 1,
            None,
        );
        assert!(before[0].no_new_output_for.is_none());

        let at = scan_store(
            temp.path(),
            "session-a",
            200 + NO_NEW_OUTPUT_AFTER.as_secs(),
            None,
        );
        assert_eq!(
            at[0].no_new_output_for.map(|duration| duration.as_secs()),
            Some(NO_NEW_OUTPUT_AFTER.as_secs())
        );
        assert_eq!(at[0].activity, "working");
    }

    #[test]
    fn foreground_turn_excludes_agents_started_before_its_boundary() {
        let temp = tempfile::tempdir().expect("tempdir");
        for (id, started_at) in [("previous-turn", "100"), ("this-turn", "200")] {
            fs::write(
                temp.path().join(format!("{id}.json")),
                serde_json::to_vec(&json!({
                    "agentId": id,
                    "parentSessionId": "session-a",
                    "name": id,
                    "description": "static assignment",
                    "status": "running",
                    "startedAt": started_at,
                }))
                .expect("manifest json"),
            )
            .expect("write manifest");
        }

        let rows = scan_store(temp.path(), "session-a", 210, Some(200));
        assert_eq!(
            rows.iter().map(|row| row.agent_id.as_str()).collect::<Vec<_>>(),
            ["this-turn"]
        );
    }

    /// A store that did not change is not read again: the watcher stats each
    /// manifest and reuses what its bytes said. A rewrite — the tools crate's
    /// temporary file and rename — is read once, and a swept file is dropped.
    #[test]
    fn an_unchanged_store_is_not_read_again() {
        let temp = tempfile::tempdir().expect("tempdir");
        let write = |id: &str, session: &str, status: &str| {
            let staged = temp.path().join(format!(".{id}.json.tmp"));
            fs::write(
                &staged,
                serde_json::to_vec(&json!({
                    "agentId": id,
                    "parentSessionId": session,
                    "name": id,
                    "status": status,
                    "startedAt": "100",
                }))
                .expect("manifest json"),
            )
            .expect("write manifest");
            fs::rename(staged, temp.path().join(format!("{id}.json"))).expect("publish manifest");
        };
        for index in 0..5 {
            write(&format!("finished-{index}"), "session-other", "completed");
        }
        write("live", "session-a", "running");
        let registry = tools::AgentRegistry::at_root_for_tests("session-a", temp.path());
        let mut cache = ManifestCache::default();

        let first = scan_registry(&registry, "session-a", 200, None, &mut cache);
        assert_eq!(first.len(), 1);
        assert_eq!(cache.take_reads(), 6);

        let second = scan_registry(&registry, "session-a", 200, None, &mut cache);
        assert_eq!(second, first);
        assert_eq!(cache.take_reads(), 0, "an unchanged store was read again");

        write("live", "session-a", "completed");
        assert!(scan_registry(&registry, "session-a", 200, None, &mut cache).is_empty());
        assert_eq!(cache.take_reads(), 1, "only the rewritten manifest is read");

        fs::remove_file(temp.path().join("finished-0.json")).expect("sweep");
        let _ = scan_registry(&registry, "session-a", 200, None, &mut cache);
        assert_eq!(cache.remembered(), 5, "a swept manifest stays remembered");
    }

    /// A store where nothing of this session runs is not listed again until
    /// a manifest lands in it: the tools crate publishes every manifest by a
    /// rename, which moves the directory's time. While something runs, every
    /// scan lists — its row's clock moves each second.
    #[test]
    fn a_quiet_store_is_not_listed_again_until_a_manifest_lands() {
        let temp = tempfile::tempdir().expect("tempdir");
        let publish = |id: &str, session: &str, status: &str| {
            let staged = temp.path().join(format!(".{id}.json.tmp"));
            fs::write(
                &staged,
                serde_json::to_vec(&json!({
                    "agentId": id,
                    "parentSessionId": session,
                    "name": id,
                    "status": status,
                    "startedAt": "100",
                }))
                .expect("manifest json"),
            )
            .expect("write manifest");
            fs::rename(staged, temp.path().join(format!("{id}.json"))).expect("publish manifest");
        };
        for index in 0..3 {
            publish(&format!("finished-{index}"), "session-other", "completed");
        }
        let registry = tools::AgentRegistry::at_root_for_tests("session-a", temp.path());
        let mut cache = ManifestCache::default();
        let scan = |cache: &mut ManifestCache| scan_registry(&registry, "session-a", 200, None, cache);

        assert!(scan(&mut cache).is_empty());
        assert_eq!(cache.take_listings(), 1);
        for _ in 0..3 {
            assert!(scan(&mut cache).is_empty());
        }
        assert_eq!(cache.take_listings(), 0, "a quiet store was listed again");

        publish("live", "session-a", "running");
        assert_eq!(scan(&mut cache).len(), 1, "a manifest that landed was not seen");
        assert_eq!(scan(&mut cache).len(), 1);
        assert_eq!(cache.take_listings(), 2, "a store with a running helper is listed every time");
    }

    /// A turn's watcher starts with each turn and finds the session's files
    /// already read: the store is not read again at every turn's start.
    #[test]
    fn a_new_turns_watcher_reads_nothing_the_session_already_read() {
        let temp = tempfile::tempdir().expect("tempdir");
        for (id, session, status) in [
            ("finished-0", "session-other", "completed"),
            ("finished-1", "session-other", "completed"),
            ("finished-2", "session-other", "completed"),
            ("live", "session-a", "running"),
        ] {
            fs::write(
                temp.path().join(format!("{id}.json")),
                serde_json::to_vec(&json!({
                    "agentId": id,
                    "parentSessionId": session,
                    "name": id,
                    "status": status,
                    "startedAt": "100",
                }))
                .expect("manifest json"),
            )
            .expect("write manifest");
        }
        let registry = tools::AgentRegistry::at_root_for_tests("session-a", temp.path());
        let mut session_watcher = ManifestCache::shared();
        let rows = scan_registry(&registry, "session-a", 200, None, &mut session_watcher);
        assert_eq!(rows.len(), 1);
        assert_eq!(session_watcher.take_reads(), 4);

        let mut turn_watcher = ManifestCache::shared();
        assert_eq!(scan_registry(&registry, "session-a", 200, Some(100), &mut turn_watcher), rows);
        assert_eq!(turn_watcher.take_reads(), 0, "a new turn's watcher read the store again");
    }

    /// A finished helper's manifest is remembered by its stamp alone: it
    /// cannot become a row until it is rewritten, and a rewrite changes the
    /// stamp. A store of thousands then costs the process no copy of their
    /// bodies — only the running ones are kept whole.
    #[test]
    fn a_finished_manifest_is_remembered_without_its_body() {
        let temp = tempfile::tempdir().expect("tempdir");
        for (id, session, status) in [
            ("finished-0", "session-other", "completed"),
            ("finished-1", "session-a", "failed"),
            ("finished-2", "session-a", "completed"),
            ("live", "session-a", "running"),
            ("elsewhere", "session-other", "running"),
        ] {
            fs::write(
                temp.path().join(format!("{id}.json")),
                serde_json::to_vec(&json!({
                    "agentId": id,
                    "parentSessionId": session,
                    "name": id,
                    "status": status,
                    "startedAt": "100",
                    "outputTail": "a long tail of output that a finished helper left behind",
                }))
                .expect("manifest json"),
            )
            .expect("write manifest");
        }
        let registry = tools::AgentRegistry::at_root_for_tests("session-a", temp.path());
        let mut cache = ManifestCache::default();
        assert_eq!(scan_registry(&registry, "session-a", 200, None, &mut cache).len(), 1);
        assert_eq!(cache.remembered(), 5);
        assert_eq!(cache.bodies_kept(), 2, "finished manifests were kept whole");
    }
}
