//! Asynchronous snapshots for the TUI's in-flight sub-agent details.
//!
//! Agent workers already persist a compact manifest beside their incremental
//! transcript. This watcher reads those files on Tokio's blocking pool and
//! sends owned snapshots to the UI task. The renderer therefore only reads
//! memory: no frame, width calculation, or paint call ever touches the disk.

use std::path::Path;
use std::sync::Arc;
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
const POLL_INTERVAL: Duration = Duration::from_secs(1);
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
    scan_registry(registry, parent_session_id, epoch_seconds_now(), None)
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
            loop {
                interval.tick().await;
                let registry = Arc::clone(&registry);
                let session_id = parent_session_id.clone();
                let scan = tokio::task::spawn_blocking(move || {
                    let agents = scan_registry(
                        &registry,
                        &session_id,
                        epoch_seconds_now(),
                        started_at_floor,
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
) -> Vec<SubagentProgress> {
    let mut progress = Vec::new();
    for path in registry.manifest_paths() {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_MANIFEST_BYTES {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<AgentManifest>(&contents) else {
            continue;
        };
        if manifest.status != "running"
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
        let transcript_path = transcript_path_for(&path, &manifest.agent_id);
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
        let activity = manifest
            .current_tool
            .as_deref()
            .and_then(|tool| {
                manifest
                    .recent_tools
                    .last()
                    .filter(|recent| recent.starts_with(tool))
            })
            .cloned()
            .or_else(|| manifest.current_tool.clone())
            .or_else(|| manifest.current_phase.clone())
            .unwrap_or_else(|| "working".to_string());
        let label = manifest
            .label
            .filter(|label| !label.trim().is_empty())
            .unwrap_or(manifest.name);
        progress.push(SubagentProgress {
            agent_id: manifest.agent_id,
            tool_call_id: manifest.tool_call_id,
            label,
            model: manifest.resolved_model.or(manifest.model),
            activity,
            recent_tools: manifest.recent_tools,
            tool_calls: manifest.tool_calls,
            output_tail: manifest.output_tail,
            started_epoch: started_at,
            elapsed: Duration::from_secs(now_epoch_seconds.saturating_sub(started_at)),
            no_new_output_for: (quiet >= NO_NEW_OUTPUT_AFTER).then_some(quiet),
            transcript_path,
            pane: manifest.pane,
            last_receipt: manifest.last_receipt,
        });
    }
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
    scan_registry(&registry, parent_session_id, now_epoch_seconds, started_at_floor)
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

    use super::{scan_store, NO_NEW_OUTPUT_AFTER};

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
}
