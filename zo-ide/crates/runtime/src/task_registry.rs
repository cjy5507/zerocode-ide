//! Task lifecycle registry with bounded, session-scoped background process evidence.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use api::sync_bridge::lock_recovered;
use serde::{Deserialize, Serialize};

use crate::registry_io;
use crate::secure_fs::Durability;
use crate::{validate_packet, TaskPacket, TaskPacketValidationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Created,
    Running,
    Completed,
    Failed,
    Stopped,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

impl TaskStatus {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Stopped)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// Generic `TaskCreate`, team, and task-packet work. This is the serde
    /// default so registries written before task kinds existed remain generic
    /// and can never be mistaken for live operating-system processes.
    #[default]
    Generic,
    BackgroundProcess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub task_id: String,
    pub prompt: String,
    pub description: Option<String>,
    pub task_packet: Option<TaskPacket>,
    pub status: TaskStatus,
    pub created_at: u64,
    pub updated_at: u64,
    pub messages: Vec<TaskMessage>,
    pub output: String,
    /// Process exit code; absent for signals, unknown exits, and generic tasks.
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub team_id: Option<String>,
    #[serde(default)]
    pub kind: TaskKind,
    /// Foreground session that launched a background process. Missing IDs fail
    /// closed: the task remains persisted and controllable, but is never shown
    /// as live in any session's `bg N` indicator.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Monotonic registry revision at which this task was last written. Drives
    /// merge conflict resolution instead of the whole-second `updated_at`, whose
    /// ties would silently drop a concurrent update. Defaults to 0 for
    /// registries written before revisions existed; a 0 always loses to any
    /// stamped write, so legacy entries reconcile correctly.
    #[serde(default)]
    pub rev: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskMessage {
    pub role: String,
    pub content: String,
    pub timestamp: u64,
}

/// Invoked (outside the registry lock) when a task reaches a terminal status,
/// with `(task_id, status, output, session_id)`. Lets the session bridge a
/// background bash task's completion into the agent push-notification path —
/// the `runtime` crate cannot call into `tools` directly (the dependency runs
/// the other way).
pub type TaskCompletionCallback =
    Box<dyn Fn(String, TaskStatus, String, Option<String>) + Send + Sync>;

/// Session-scoped view used by the TUI render loop. It resolves the current
/// counter on each load so zero-count session entries can be evicted without
/// making an existing HUD handle miss later process launches.
#[derive(Clone, Debug, Default)]
pub struct LiveBackgroundProcessCount {
    inner: Option<Arc<SessionLiveBackgroundProcessCount>>,
}

#[derive(Debug)]
struct SessionLiveBackgroundProcessCount {
    processes: Arc<LiveBackgroundProcesses>,
    session_id: String,
}

impl LiveBackgroundProcessCount {
    #[must_use]
    pub fn load(&self) -> usize {
        let Some(count) = &self.inner else {
            return 0;
        };
        let inner = lock_recovered(&count.processes.inner);
        inner
            .sessions
            .get(&count.session_id)
            .copied()
            .unwrap_or(0)
    }
}

#[derive(Debug, Default)]
struct LiveBackgroundProcesses {
    inner: Mutex<LiveBackgroundProcessesInner>,
}

#[derive(Debug, Default)]
struct LiveBackgroundProcessesInner {
    sessions: HashMap<String, usize>,
    task_sessions: HashMap<String, String>,
}

impl LiveBackgroundProcesses {
    fn count_for_session(
        self: &Arc<Self>,
        session_id: Option<&str>,
    ) -> LiveBackgroundProcessCount {
        let Some(session_id) = session_id.filter(|id| !id.trim().is_empty()) else {
            // Fail closed: an unstamped launch must not appear in whichever
            // interactive session happens to be visible.
            return LiveBackgroundProcessCount::default();
        };
        LiveBackgroundProcessCount {
            inner: Some(Arc::new(SessionLiveBackgroundProcessCount {
                processes: Arc::clone(self),
                session_id: session_id.to_string(),
            })),
        }
    }

    fn started(&self, task_id: &str, session_id: Option<&str>) {
        let Some(session_id) = session_id.filter(|id| !id.trim().is_empty()) else {
            return;
        };
        let mut inner = lock_recovered(&self.inner);
        if inner.task_sessions.contains_key(task_id) {
            return;
        }
        *inner.sessions.entry(session_id.to_string()).or_default() += 1;
        inner
            .task_sessions
            .insert(task_id.to_string(), session_id.to_string());
    }

    fn finished(&self, task_id: &str) {
        let mut inner = lock_recovered(&self.inner);
        let Some(session_id) = inner.task_sessions.remove(task_id) else {
            return;
        };
        let reached_zero = inner.sessions.get_mut(&session_id).is_some_and(|count| {
            *count = count.saturating_sub(1);
            *count == 0
        });
        if reached_zero {
            inner.sessions.remove(&session_id);
        }
    }
}

#[derive(Clone, Default)]
pub struct TaskRegistry {
    /// Poison policy: recover (`lock_recovered`). Every write completes its
    /// in-memory mutation before the only fallible step (persistence, whose
    /// failures are logged), so the map is consistent at every panic
    /// point — and propagating poison used to brick the whole registry when
    /// one background watcher thread panicked.
    inner: Arc<Mutex<RegistryInner>>,
    persistence_path: Arc<Option<PathBuf>>,
    /// Warn-once latch for persistence failures (see
    /// [`registry_io::save_registry_inner_warn_once`]); re-armed by a
    /// successful save.
    persist_warned: Arc<AtomicBool>,
    /// Fail-closed persistence latch. Set the first time a persist fails; once
    /// set, this instance never writes to disk again (mutations stay in memory
    /// so local reads are truthful, but can never clobber a peer's committed
    /// state). Recovery is a process restart that reloads from disk.
    persist_disabled: Arc<AtomicBool>,
    /// Set once by the session; fired on terminal status. `dyn Fn` is not
    /// `Debug`, hence the manual `Debug` impl below.
    completion_callback: Arc<Mutex<Option<Arc<TaskCompletionCallback>>>>,
    /// Ephemeral live-process state. Deliberately never serialized or rebuilt
    /// from persisted tasks: after restart no old nonterminal record represents
    /// a process owned by this runtime.
    live_background_processes: Arc<LiveBackgroundProcesses>,
}

impl std::fmt::Debug for TaskRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskRegistry")
            .field("inner", &self.inner)
            .field("persistence_path", &self.persistence_path)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct RegistryInner {
    tasks: HashMap<String, Task>,
    counter: u64,
    /// Monotonic revision advanced on every mutation. Stamped onto each task's
    /// `rev` at write time and max-merged across processes so committed writes
    /// get strictly increasing, comparable revisions even within one wall-clock
    /// second. `#[serde(default)]` keeps pre-revision files loadable.
    #[serde(default)]
    revision: u64,
    /// Removal records: task id → the `revision` at which it was deleted or
    /// pruned. A tombstone suppresses re-insertion of an on-disk entry whose
    /// `rev` it covers, so a delete is never resurrected by the merge.
    /// Tombstones are unbounded and durable: they are never evicted, because a
    /// safe cap would require cross-process coordination (a generation/GC
    /// protocol) to prove no arbitrarily-stale peer still holds the deleted
    /// entry. Correctness (no resurrection) is preferred over the bounded file
    /// growth an eviction cap would give. `#[serde(default)]` keeps
    /// pre-tombstone files loadable.
    #[serde(default)]
    tombstones: HashMap<String, u64>,
}

impl RegistryInner {
    /// Advance the registry revision and return the new value to stamp onto the
    /// entry being written. Every mutation calls this so concurrent writers get
    /// distinct, ordered revisions.
    fn bump_revision(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    /// Record a removal at the current revision so the merge cannot resurrect it
    /// from a peer's on-disk copy. Tombstones are durable and never evicted (see
    /// the `tombstones` field docs), so a delete survives arbitrarily-stale
    /// peers.
    fn tombstone(&mut self, task_id: String) {
        let rev = self.bump_revision();
        self.tombstones.insert(task_id, rev);
    }
}

impl registry_io::MergeInto for RegistryInner {
    /// Fold a concurrently-persisted on-disk copy into this process's pending
    /// write so a peer Zo process's tasks are not clobbered, while keeping
    /// deletions authoritative. The reconciliation is order-independent: take
    /// the max revision and counter, union tombstones by max revision, keep
    /// whichever side of a live entry carries the newer `rev` (ties keep ours,
    /// the value we are about to publish), then normalize so no entry survives a
    /// tombstone that is at least as new and no tombstone outlives a strictly
    /// newer re-insertion of the same id.
    fn merge_in(&mut self, on_disk: Self) {
        self.revision = self.revision.max(on_disk.revision);
        self.counter = self.counter.max(on_disk.counter);
        for (id, disk_rev) in on_disk.tombstones {
            let slot = self.tombstones.entry(id).or_insert(0);
            *slot = (*slot).max(disk_rev);
        }
        for (id, disk_task) in on_disk.tasks {
            match self.tasks.get(&id) {
                Some(mine) if mine.rev >= disk_task.rev => {}
                _ => {
                    self.tasks.insert(id, disk_task);
                }
            }
        }
        // A tombstone at least as new as a live entry's rev wins: the entry was
        // deleted after (or at) the revision it was last written, so drop it.
        let tombstones = &self.tombstones;
        self.tasks
            .retain(|id, task| !matches!(tombstones.get(id), Some(&rev) if rev >= task.rev));
        // A live entry strictly newer than its tombstone means the id was
        // re-created after deletion; that tombstone is spent, so drop it.
        let tasks = &self.tasks;
        self.tombstones
            .retain(|id, rev| !matches!(tasks.get(id), Some(task) if task.rev > *rev));
    }
}

/// The single retention/notification budget for background process evidence.
pub struct BackgroundTaskLimits {
    pub tail_bytes: usize,
    pub retention_secs: u64,
    pub retained_tasks: usize,
    pub notice_tail_chars: usize,
}

pub const BACKGROUND_TASK_LIMITS: BackgroundTaskLimits = BackgroundTaskLimits {
    tail_bytes: 64 * 1024,
    retention_secs: 7 * 24 * 60 * 60,
    retained_tasks: 128,
    notice_tail_chars: 240,
};
pub const BACKGROUND_TASK_STORE_DIR: &str = "tasks";

const MAX_TASK_MESSAGES: usize = 256;
const MAX_TASK_OUTPUT_BYTES: usize = 1024 * 1024;
#[cfg(test)]
const MAX_TASK_REGISTRY_ENTRIES: usize = 8;
#[cfg(not(test))]
const MAX_TASK_REGISTRY_ENTRIES: usize = 512;
const TASK_OUTPUT_TRUNCATED_PREFIX: &str = crate::background_log::OLDER_OUTPUT_TRUNCATED;

/// The task registry is published, not flushed. Every background `bash` spawn
/// commits three times (create, `Running`, output head) and its exit two more,
/// and on macOS each flushed commit paid two `F_FULLFSYNC` (~9 ms): measured
/// on the parity harness (axis F, twenty spawns) as a 31–85 ms gap per spawn
/// and a 0.86 s spread of exit times, two of which then missed the parent's
/// poll. Nothing here must outlive a power loss — a non-terminal task is not
/// restored as live on restart anyway.
const TASK_REGISTRY_DURABILITY: Durability = Durability::Published;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl TaskRegistry {
    /// Open this session's task evidence under its host-selected state directory.
    pub fn for_session(state_dir: &std::path::Path, session_id: &str) -> Result<Self, String> {
        if session_id.is_empty() || matches!(session_id, "." | "..")
            || !session_id.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')) {
            return Err("invalid task-registry session id".to_string());
        }
        Ok(Self::with_persistence_path(Some(state_dir.join(BACKGROUND_TASK_STORE_DIR).join(format!("{session_id}.json")))))
    }

    #[must_use]
    pub fn new() -> Self {
        Self::with_persistence_path(registry_io::default_registry_path("tasks.json"))
    }

    #[must_use]
    pub fn new_in_memory() -> Self {
        Self::with_persistence_path(None)
    }

    #[must_use]
    pub fn with_persistence_path(path: Option<PathBuf>) -> Self {
        let mut inner = path
            .as_deref()
            .and_then(registry_io::load_registry_inner::<RegistryInner>)
            .unwrap_or_default();
        prune_terminal_tasks(&mut inner, &LiveBackgroundProcesses::default(), false);
        Self {
            inner: Arc::new(Mutex::new(inner)),
            persistence_path: Arc::new(path),
            persist_warned: Arc::default(),
            persist_disabled: Arc::default(),
            completion_callback: Arc::new(Mutex::new(None)),
            live_background_processes: Arc::default(),
        }
    }

    /// Install the terminal-status completion callback (last set wins; `None`
    /// clears). Fired outside the registry lock from [`Self::set_status`].
    pub fn set_completion_callback(&self, callback: Option<Arc<TaskCompletionCallback>>) {
        *self
            .completion_callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = callback;
    }

    fn notify_terminal(
        &self,
        task_id: &str,
        status: TaskStatus,
        output: String,
        session_id: Option<String>,
    ) {
        let callback = self
            .completion_callback
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(callback) = callback {
            callback(task_id.to_string(), status, output, session_id);
        }
    }

    pub fn create(&self, prompt: &str, description: Option<&str>) -> Task {
        self.create_task(
            prompt.to_owned(),
            description.map(str::to_owned),
            None,
            TaskKind::Generic,
            None,
        )
    }

    /// Register a successfully spawned background process. The session id is
    /// copied from `ToolContext` at launch; if it is absent, persistence and
    /// `TaskList` history still work but the live UI count fails closed to zero.
    pub fn create_background_process(
        &self,
        prompt: &str,
        description: Option<&str>,
        session_id: Option<&str>,
    ) -> Task {
        let session_id = session_id
            .filter(|id| !id.trim().is_empty())
            .map(str::to_owned);
        let task = self.create_task(
            prompt.to_owned(),
            description.map(str::to_owned),
            None,
            TaskKind::BackgroundProcess,
            session_id.clone(),
        );
        self.live_background_processes
            .started(&task.task_id, session_id.as_deref());
        task
    }

    /// Obtain the render-loop handle for one visible session. Each load reads
    /// only the small live-counter map; task output and persistence state stay
    /// out of the render path.
    #[must_use]
    pub fn live_background_process_count(
        &self,
        session_id: Option<&str>,
    ) -> LiveBackgroundProcessCount {
        self.live_background_processes
            .count_for_session(session_id)
    }

    pub fn create_from_packet(
        &self,
        packet: TaskPacket,
    ) -> Result<Task, TaskPacketValidationError> {
        let packet = validate_packet(packet)?.into_inner();
        Ok(self.create_task(
            packet.objective.clone(),
            Some(packet.scope.clone()),
            Some(packet),
            TaskKind::Generic,
            None,
        ))
    }

    fn create_task(
        &self,
        prompt: String,
        description: Option<String>,
        task_packet: Option<TaskPacket>,
        kind: TaskKind,
        session_id: Option<String>,
    ) -> Task {
        let mut inner = lock_recovered(&self.inner);
        // Allocate the counter/revision/id INSIDE the transaction, after the
        // merge of the newest disk state, so a peer process that committed on
        // the same base revision cannot collide: our counter is incremented off
        // the merged (post-peer) value, yielding a distinct id and a strictly
        // newer rev. See `registry_io::commit_registry_mutation`.
        self.commit_mutation(&mut inner, move |inner| {
            inner.counter += 1;
            let rev = inner.bump_revision();
            let ts = now_secs();
            let task_id = format!("task_{:08x}_{}", ts, inner.counter);
            let task = Task {
                task_id: task_id.clone(),
                prompt,
                description,
                task_packet,
                status: TaskStatus::Created,
                created_at: ts,
                updated_at: ts,
                messages: Vec::new(),
                output: String::new(),
                exit_code: None,
                team_id: None,
                kind,
                session_id,
                rev,
            };
            inner.tasks.insert(task_id, task.clone());
            prune_terminal_tasks(inner, &self.live_background_processes, true);
            task
        })
    }

    #[must_use]
    pub fn get(&self, task_id: &str) -> Option<Task> {
        let inner = lock_recovered(&self.inner);
        inner.tasks.get(task_id).cloned()
    }

    #[must_use]
    pub fn list(&self, status_filter: Option<TaskStatus>) -> Vec<Task> {
        let inner = lock_recovered(&self.inner);
        inner
            .tasks
            .values()
            .filter(|t| status_filter.is_none_or(|s| t.status == s))
            .cloned()
            .collect()
    }

    pub fn stop(&self, task_id: &str) -> Result<Task, String> {
        let snapshot = {
            let mut inner = lock_recovered(&self.inner);
            let (result, persisted) = self.commit_mutation_status(&mut inner, |inner| {
                let rev = inner.bump_revision();
                let task = inner
                    .tasks
                    .get_mut(task_id)
                    .ok_or_else(|| format!("task not found: {task_id}"))?;

                match task.status {
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Stopped => {
                        return Err(format!(
                            "task {task_id} is already in terminal state: {}",
                            task.status
                        ));
                    }
                    _ => {}
                }

                task.status = TaskStatus::Stopped;
                task.updated_at = now_secs();
                task.rev = rev;
                let snapshot = task.clone();
                prune_terminal_tasks(inner, &self.live_background_processes, true);
                Ok(snapshot)
            });
            fold_persistence(result, persisted)?
        };
        self.notify_terminal(
            task_id,
            TaskStatus::Stopped,
            snapshot.output.clone(),
            snapshot.session_id.clone(),
        );
        Ok(snapshot)
    }

    /// Confirm that a stopped background process has actually exited. The
    /// watcher calls this only after reaping the child; `finished` owns the
    /// task-id idempotence, so a concurrent natural terminal path is harmless.
    pub fn confirm_background_exit(&self, task_id: &str) {
        self.live_background_processes.finished(task_id);
    }

    pub fn update(&self, task_id: &str, message: &str) -> Result<Task, String> {
        let mut inner = lock_recovered(&self.inner);
        let (result, persisted) = self.commit_mutation_status(&mut inner, |inner| {
            let rev = inner.bump_revision();
            let task = inner
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| format!("task not found: {task_id}"))?;

            task.messages.push(TaskMessage {
                role: String::from("user"),
                content: message.to_owned(),
                timestamp: now_secs(),
            });
            trim_task_messages(task);
            task.updated_at = now_secs();
            task.rev = rev;
            Ok(task.clone())
        });
        fold_persistence(result, persisted)
    }

    pub fn output(&self, task_id: &str) -> Result<String, String> {
        let inner = lock_recovered(&self.inner);
        let task = inner
            .tasks
            .get(task_id)
            .ok_or_else(|| format!("task not found: {task_id}"))?;
        Ok(task.output.clone())
    }

    pub fn append_output(&self, task_id: &str, output: &str) -> Result<(), String> {
        let mut inner = lock_recovered(&self.inner);
        let (result, persisted) = self.commit_mutation_status(&mut inner, |inner| {
            let rev = inner.bump_revision();
            let task = inner
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| format!("task not found: {task_id}"))?;
            task.output.push_str(output);
            let limit = if task.kind == TaskKind::BackgroundProcess { BACKGROUND_TASK_LIMITS.tail_bytes } else { MAX_TASK_OUTPUT_BYTES };
            trim_task_output_to(&mut task.output, limit);
            task.updated_at = now_secs();
            task.rev = rev;
            Ok(())
        });
        fold_persistence(result, persisted)
    }

    pub fn set_status(&self, task_id: &str, status: TaskStatus) -> Result<(), String> {
        // `transition` is `None` when no state change happened (the task was
        // already terminal, so this call is a no-op). It is `Some(_)` when the
        // status was actually written; the inner option carries the terminal
        // snapshot when the new status is terminal. Distinguishing "no
        // transition" from "terminal transition" is what stops a late watcher's
        // no-op `Completed` on an already-`Stopped` task from decrementing the
        // live-process gauge or firing a second completion notification.
        let transition = {
            let mut inner = lock_recovered(&self.inner);
            let (result, persisted) =
                self.commit_mutation_status(&mut inner, |inner| -> Result<_, String> {
                    let rev = inner.bump_revision();
                    let task = inner
                        .tasks
                        .get_mut(task_id)
                        .ok_or_else(|| format!("task not found: {task_id}"))?;
                    // Terminal states are absorbing. This check and the write
                    // happen under one registry lock, so a watcher racing
                    // `TaskStop` cannot overwrite `Stopped` with
                    // `Completed`/`Failed` (or publish a second completion
                    // notification).
                    if task.status.is_terminal() {
                        return Ok(None);
                    }
                    task.status = status;
                    if status.is_terminal() && task.kind == TaskKind::BackgroundProcess {
                        task.exit_code = crate::background_log::exit_code(&task.output);
                    }
                    task.updated_at = now_secs();
                    task.rev = rev;
                    let snapshot = status
                        .is_terminal()
                        .then(|| (task.output.clone(), task.session_id.clone()));
                    prune_terminal_tasks(inner, &self.live_background_processes, true);
                    Ok(Some(snapshot))
                });
            fold_persistence(result, persisted)?
        };
        // Only a real terminal transition retires the live-process gauge and
        // fires the completion callback; a no-op call (`transition == None`)
        // does neither.
        if let Some(terminal_snapshot) = transition {
            if status.is_terminal() {
                self.live_background_processes.finished(task_id);
            }
            // Fire the completion callback OUTSIDE the registry lock: it funnels
            // into the agent completion channel, so holding `inner` across it
            // would risk a re-entrant deadlock and pin the lock behind a slow
            // consumer.
            if let Some((output, session_id)) = terminal_snapshot {
                self.notify_terminal(task_id, status, output, session_id);
            }
        }
        Ok(())
    }

    pub fn assign_team(&self, task_id: &str, team_id: &str) -> Result<(), String> {
        let mut inner = lock_recovered(&self.inner);
        let (result, persisted) = self.commit_mutation_status(&mut inner, |inner| {
            let rev = inner.bump_revision();
            let task = inner
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| format!("task not found: {task_id}"))?;
            task.team_id = Some(team_id.to_owned());
            task.updated_at = now_secs();
            task.rev = rev;
            Ok(())
        });
        fold_persistence(result, persisted)
    }

    #[must_use]
    pub fn remove(&self, task_id: &str) -> Option<Task> {
        let removed = {
            let mut inner = lock_recovered(&self.inner);
            // Remove and tombstone INSIDE the transaction, after the on-disk
            // merge, so a concurrently-persisted peer copy (which the merge would
            // otherwise re-introduce) cannot resurrect this task, and so the
            // tombstone lands in the same committed write as the removal.
            self.commit_mutation(&mut inner, |inner| {
                let removed = inner.tasks.remove(task_id);
                if removed.is_some() {
                    inner.tombstone(task_id.to_string());
                }
                removed
            })
        };
        if removed.is_some() {
            self.live_background_processes.finished(task_id);
        }
        removed
    }

    #[must_use]
    pub fn len(&self) -> usize {
        let inner = lock_recovered(&self.inner);
        inner.tasks.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Run `mutate` as a persisted transaction: when a persistence path is
    /// configured, the merge→mutate→write happens under the cross-process lock
    /// so `mutate` can allocate ids/revisions rebased onto the newest committed
    /// disk state (see [`registry_io::commit_registry_mutation`]). Without a
    /// path (in-memory registry) `mutate` runs directly on the held guard.
    ///
    /// Best-effort persistence variant: the mutation result is returned and a
    /// persistence failure is only warned about. The fail-closed latch in
    /// `commit_registry_mutation` keeps the mutation in memory (local reads
    /// remain truthful) while ensuring no peer can be clobbered by a later
    /// commit from this instance. Used by mutations whose public API is
    /// infallible.
    fn commit_mutation<R>(
        &self,
        inner: &mut RegistryInner,
        mutate: impl FnOnce(&mut RegistryInner) -> R,
    ) -> R {
        self.commit_mutation_status(inner, mutate).0
    }

    /// Like [`Self::commit_mutation`] but also returns the persistence status so
    /// a `Result`-returning method can surface a persistence failure as an error
    /// instead of a silent success. On failure the mutation is kept in memory
    /// (local reads remain truthful) and this instance is latched fail-closed so
    /// no future write reaches disk; the caller must handle the error or restart
    /// to recover.
    fn commit_mutation_status<R>(
        &self,
        inner: &mut RegistryInner,
        mutate: impl FnOnce(&mut RegistryInner) -> R,
    ) -> (R, Result<(), String>) {
        if let Some(path) = self.persistence_path.as_ref().as_ref() {
            registry_io::commit_registry_mutation_warn_once_status(
                "task registry",
                path,
                &self.persist_disabled,
                &mut *inner,
                &self.persist_warned,
                TASK_REGISTRY_DURABILITY,
                mutate,
            )
        } else {
            (mutate(inner), Ok(()))
        }
    }
}

fn prune_terminal_tasks(
    inner: &mut RegistryInner,
    live_background_processes: &LiveBackgroundProcesses,
    include_generic_overflow: bool,
) {
    let excess = if include_generic_overflow { inner.tasks.len().saturating_sub(MAX_TASK_REGISTRY_ENTRIES) } else { 0 };
    let background_excess = inner.tasks.values().filter(|task| task.kind == TaskKind::BackgroundProcess).count()
        .saturating_sub(BACKGROUND_TASK_LIMITS.retained_tasks);
    let now = now_secs();
    let live = lock_recovered(&live_background_processes.inner);
    let mut terminal: Vec<_> = inner
        .tasks
        .values()
        .filter(|task| {
            task.status.is_terminal() && !live.task_sessions.contains_key(&task.task_id)
        })
        .map(|task| (task.updated_at, task.created_at, task.task_id.clone()))
        .collect();
    terminal.sort_unstable();
    let mut background_removed = 0;
    for (index, (updated, _, task_id)) in terminal.into_iter().enumerate() {
        let background = inner.tasks.get(&task_id).is_some_and(|task| task.kind == TaskKind::BackgroundProcess);
        let expired = background && now.saturating_sub(updated) > BACKGROUND_TASK_LIMITS.retention_secs;
        if index >= excess && !expired && !(background && background_removed < background_excess) { continue; }
        background_removed += usize::from(background);
        inner.tasks.remove(&task_id);
        // Prune is a removal too: tombstone it so the on-disk merge does not
        // revive an entry this process just evicted for overflow.
        inner.tombstone(task_id);
    }
}

/// Fold a persistence status into a mutation's result so a persist failure is
/// surfaced as an error rather than a silent success. A mutation error (e.g.
/// "task not found") wins, because in that case no write was attempted; only a
/// mutation that succeeded in memory but failed to persist is downgraded to
/// `Err`. Paired with the fail-closed latch in `commit_registry_mutation`,
/// this guarantees a reported success is durable and a reported failure never
/// writes to disk, so no peer can be clobbered by this instance later.
fn fold_persistence<T>(result: Result<T, String>, persisted: Result<(), String>) -> Result<T, String> {
    match result {
        Err(error) => Err(error),
        Ok(value) => persisted.map(|()| value),
    }
}

fn trim_task_messages(task: &mut Task) {
    let excess = task.messages.len().saturating_sub(MAX_TASK_MESSAGES);
    if excess > 0 {
        task.messages.drain(0..excess);
    }
}

fn trim_task_output_to(output: &mut String, limit: usize) {
    if output.len() <= limit {
        return;
    }

    let keep_budget = limit.saturating_sub(TASK_OUTPUT_TRUNCATED_PREFIX.len());
    let mut start = output.len().saturating_sub(keep_budget);
    while start < output.len() && !output.is_char_boundary(start) {
        start += 1;
    }
    let tail = output[start..].to_string();
    output.clear();
    output.push_str(TASK_OUTPUT_TRUNCATED_PREFIX);
    output.push_str(&tail);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_quality_retention_expires_only_finished_background_evidence() {
        let registry = TaskRegistry::new_in_memory();
        let finished = registry.create_background_process("old", None, Some("s"));
        registry.set_status(&finished.task_id, TaskStatus::Completed).unwrap();
        registry.confirm_background_exit(&finished.task_id);
        let running = registry.create_background_process("live", None, Some("s"));
        let generic = registry.create("generic", None);
        registry.set_status(&generic.task_id, TaskStatus::Completed).unwrap();
        let mut inner = lock_recovered(&registry.inner);
        for task in inner.tasks.values_mut() { task.updated_at = now_secs() - BACKGROUND_TASK_LIMITS.retention_secs - 1; }
        prune_terminal_tasks(&mut inner, &registry.live_background_processes, true);
        assert!(!inner.tasks.contains_key(&finished.task_id));
        assert!(inner.tasks.contains_key(&running.task_id) && inner.tasks.contains_key(&generic.task_id));
    }

    #[test]
    fn execution_quality_session_registries_are_isolated_and_reject_paths() {
        let dir = tempfile::tempdir().unwrap();
        let first = TaskRegistry::for_session(dir.path(), "session-a").unwrap();
        let task = first.create_background_process("server", None, Some("session-a"));
        let second = TaskRegistry::for_session(dir.path(), "session-b").unwrap();
        assert!(second.get(&task.task_id).is_none());
        assert!(TaskRegistry::for_session(dir.path(), "../escape").is_err());
        assert!(TaskRegistry::for_session(dir.path(), "session-a").unwrap().get(&task.task_id).is_some());
    }

    #[test]
    fn execution_quality_background_exit_and_tail_survive_registry_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-tasks.json");
        let registry = TaskRegistry::with_persistence_path(Some(path.clone()));
        let task = registry.create_background_process("fixture server", None, Some("session-a"));
        registry.set_status(&task.task_id, TaskStatus::Running).unwrap();
        registry.append_output(&task.task_id, &"앞".repeat(100_000)).unwrap();
        registry.append_output(&task.task_id, "\nlast diagnostic\n[exit 7]\n").unwrap();
        registry.set_status(&task.task_id, TaskStatus::Failed).unwrap();
        let loaded = TaskRegistry::with_persistence_path(Some(path));
        let recovered = loaded.get(&task.task_id).unwrap();
        assert_eq!(serde_json::to_value(&recovered).unwrap()["exit_code"], 7);
        assert!(recovered.output.len() <= BACKGROUND_TASK_LIMITS.tail_bytes);
        assert!(loaded.output(&task.task_id).unwrap().ends_with("last diagnostic\n[exit 7]\n"));
    }

    #[test]
    fn set_status_terminal_fires_completion_callback_once() {
        let registry = TaskRegistry::new_in_memory();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&seen);
        registry.set_completion_callback(Some(Arc::new(Box::new(
            move |id: String,
                  status: TaskStatus,
                  output: String,
                  session_id: Option<String>| {
                writer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((id, status, output, session_id));
            },
        ))));

        let task = registry.create_background_process("echo hi", None, Some("session-a"));
        registry.append_output(&task.task_id, "hello\n").unwrap();
        registry
            .set_status(&task.task_id, TaskStatus::Completed)
            .unwrap();
        // Terminal states are absorbing: a late watcher verdict or accidental
        // nonterminal write must neither change the final status nor fire the
        // completion callback again.
        registry
            .set_status(&task.task_id, TaskStatus::Failed)
            .unwrap();
        registry
            .set_status(&task.task_id, TaskStatus::Running)
            .unwrap();
        assert_eq!(
            registry.get(&task.task_id).map(|task| task.status),
            Some(TaskStatus::Completed)
        );

        // A non-terminal transition must NOT fire the callback.
        let other = registry.create("still going", None);
        registry
            .set_status(&other.task_id, TaskStatus::Running)
            .unwrap();

        let fired = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(fired.len(), 1, "only terminal status fires the callback");
        assert_eq!(fired[0].0, task.task_id);
        assert_eq!(fired[0].1, TaskStatus::Completed);
        assert!(
            fired[0].2.contains("hello"),
            "callback receives the task output: {:?}",
            fired[0].2
        );
        assert_eq!(fired[0].3.as_deref(), Some("session-a"));
    }
    use std::fs;

    #[test]
    fn creates_and_retrieves_tasks() {
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("Do something", Some("A test task"));
        assert_eq!(task.status, TaskStatus::Created);
        assert_eq!(task.prompt, "Do something");
        assert_eq!(task.description.as_deref(), Some("A test task"));
        assert_eq!(task.task_packet, None);

        let fetched = registry.get(&task.task_id).expect("task should exist");
        assert_eq!(fetched.task_id, task.task_id);
    }

    #[test]
    fn creates_task_from_packet() {
        let registry = TaskRegistry::new_in_memory();
        let packet = TaskPacket {
            objective: "Ship task packet support".to_string(),
            scope: "runtime/task system".to_string(),
            repo: "zo-parity".to_string(),
            branch_policy: "origin/main only".to_string(),
            acceptance_tests: vec!["cargo test --workspace".to_string()],
            commit_policy: "single commit".to_string(),
            reporting_contract: "print commit sha".to_string(),
            escalation_policy: "manual escalation".to_string(),
        };

        let task = registry
            .create_from_packet(packet.clone())
            .expect("packet-backed task should be created");

        assert_eq!(task.prompt, packet.objective);
        assert_eq!(task.description.as_deref(), Some("runtime/task system"));
        assert_eq!(task.task_packet, Some(packet.clone()));

        let fetched = registry.get(&task.task_id).expect("task should exist");
        assert_eq!(fetched.task_packet, Some(packet));
    }

    #[test]
    fn lists_tasks_with_optional_filter() {
        let registry = TaskRegistry::new_in_memory();
        registry.create("Task A", None);
        let task_b = registry.create("Task B", None);
        registry
            .set_status(&task_b.task_id, TaskStatus::Running)
            .expect("set status should succeed");

        let all = registry.list(None);
        assert_eq!(all.len(), 2);

        let running = registry.list(Some(TaskStatus::Running));
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].task_id, task_b.task_id);

        let created = registry.list(Some(TaskStatus::Created));
        assert_eq!(created.len(), 1);
    }

    #[test]
    fn generic_tasks_never_enter_live_background_counts() {
        let registry = TaskRegistry::new_in_memory();
        let session = registry.live_background_process_count(Some("session-a"));

        let task = registry.create("Task A", None);
        registry
            .set_status(&task.task_id, TaskStatus::Running)
            .expect("set status should succeed");

        assert_eq!(session.load(), 0);
        assert_eq!(task.kind, TaskKind::Generic);
        assert_eq!(task.session_id, None);
    }

    #[test]
    fn live_background_counts_are_filtered_by_launch_session() {
        let registry = TaskRegistry::new_in_memory();
        let session_a = registry.live_background_process_count(Some("session-a"));
        let session_b = registry.live_background_process_count(Some("session-b"));
        let unstamped = registry.live_background_process_count(None);

        let task = registry.create_background_process(
            "sleep 30",
            Some("background bash"),
            Some("session-a"),
        );

        assert_eq!(session_a.load(), 1);
        assert_eq!(session_b.load(), 0);
        assert_eq!(unstamped.load(), 0);
        assert_eq!(task.kind, TaskKind::BackgroundProcess);
        assert_eq!(task.session_id.as_deref(), Some("session-a"));
    }

    #[test]
    fn terminal_transition_decrements_live_count_exactly_once() {
        let registry = TaskRegistry::new_in_memory();
        let count = registry.live_background_process_count(Some("session-a"));
        let task = registry.create_background_process("exit 0", None, Some("session-a"));
        assert_eq!(count.load(), 1);

        registry
            .set_status(&task.task_id, TaskStatus::Completed)
            .expect("completion should succeed");
        registry
            .set_status(&task.task_id, TaskStatus::Failed)
            .expect("repeated terminal transition should remain idempotent");

        assert_eq!(count.load(), 0);
        assert_eq!(
            registry.get(&task.task_id).map(|task| task.status),
            Some(TaskStatus::Completed),
            "the first terminal verdict is final"
        );
    }

    #[test]
    fn session_counts_do_not_insert_on_read_and_evict_at_zero() {
        let registry = TaskRegistry::new_in_memory();
        let count = registry.live_background_process_count(Some("session-a"));
        assert!(
            lock_recovered(&registry.live_background_processes.inner)
                .sessions
                .is_empty(),
            "reading an absent session must not allocate a counter"
        );

        let first =
            registry.create_background_process("sleep 1", None, Some("session-a"));
        assert_eq!(count.load(), 1);
        registry
            .set_status(&first.task_id, TaskStatus::Completed)
            .expect("completion should succeed");
        assert_eq!(count.load(), 0);
        assert!(
            lock_recovered(&registry.live_background_processes.inner)
                .sessions
                .is_empty(),
            "the zero-count session entry must be evicted"
        );

        let second =
            registry.create_background_process("sleep 1", None, Some("session-a"));
        assert_eq!(count.load(), 1, "an existing HUD handle observes a reinsert");
        registry.confirm_background_exit(&second.task_id);
        assert_eq!(count.load(), 0);
    }

    #[test]
    fn stop_wins_over_late_watcher_terminal_transition() {
        use std::sync::Barrier;

        let registry = TaskRegistry::new_in_memory();
        let count = registry.live_background_process_count(Some("session-a"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&seen);
        registry.set_completion_callback(Some(Arc::new(Box::new(
            move |_id: String,
                  status: TaskStatus,
                  _output: String,
                  _session_id: Option<String>| {
                writer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(status);
            },
        ))));
        let task = registry.create_background_process(
            "sleep 30",
            Some("background bash"),
            Some("session-a"),
        );
        registry
            .set_status(&task.task_id, TaskStatus::Running)
            .expect("background task should run");
        assert_eq!(count.load(), 1);

        let gate = Arc::new(Barrier::new(2));
        let late_gate = Arc::clone(&gate);
        let late_registry = registry.clone();
        let late_task_id = task.task_id.clone();
        let late_watcher = std::thread::spawn(move || {
            late_gate.wait();
            late_registry
                .set_status(&late_task_id, TaskStatus::Completed)
                .expect("late watcher completion should be an idempotent no-op");
        });

        let stopped = registry
            .stop(&task.task_id)
            .expect("TaskStop should win before the watcher is released");
        gate.wait();
        late_watcher.join().expect("late watcher should finish");

        assert_eq!(stopped.status, TaskStatus::Stopped);
        assert_eq!(
            registry.get(&task.task_id).map(|task| task.status),
            Some(TaskStatus::Stopped),
            "a late watcher verdict must not overwrite Stopped"
        );
        assert_eq!(
            count.load(),
            1,
            "Stopped is only a request until the watcher confirms process exit"
        );
        registry.confirm_background_exit(&task.task_id);
        registry.confirm_background_exit(&task.task_id);
        assert_eq!(count.load(), 0, "exit confirmation decrements exactly once");
        assert_eq!(
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[TaskStatus::Stopped],
            "only the winning terminal transition publishes completion"
        );
    }

    #[test]
    fn stops_running_task() {
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("Stoppable", None);
        registry
            .set_status(&task.task_id, TaskStatus::Running)
            .unwrap();

        let stopped = registry.stop(&task.task_id).expect("stop should succeed");
        assert_eq!(stopped.status, TaskStatus::Stopped);

        // Stopping again should fail
        let result = registry.stop(&task.task_id);
        assert!(result.is_err());
    }

    #[test]
    fn updates_task_with_messages() {
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("Messageable", None);
        let updated = registry
            .update(&task.task_id, "Here's more context")
            .expect("update should succeed");
        assert_eq!(updated.messages.len(), 1);
        assert_eq!(updated.messages[0].content, "Here's more context");
        assert_eq!(updated.messages[0].role, "user");
    }

    #[test]
    fn appends_and_retrieves_output() {
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("Output task", None);
        registry
            .append_output(&task.task_id, "line 1\n")
            .expect("append should succeed");
        registry
            .append_output(&task.task_id, "line 2\n")
            .expect("append should succeed");

        let output = registry.output(&task.task_id).expect("output should exist");
        assert_eq!(output, "line 1\nline 2\n");
    }

    #[test]
    fn update_keeps_only_recent_task_messages() {
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("Message cap task", None);

        for index in 0..(MAX_TASK_MESSAGES + 5) {
            registry
                .update(&task.task_id, &format!("message {index}"))
                .expect("update should succeed");
        }

        let fetched = registry.get(&task.task_id).expect("task should exist");
        assert_eq!(fetched.messages.len(), MAX_TASK_MESSAGES);
        assert_eq!(fetched.messages[0].content, "message 5");
        let expected_last = format!("message {}", MAX_TASK_MESSAGES + 4);
        assert_eq!(
            fetched
                .messages
                .last()
                .map(|message| message.content.as_str()),
            Some(expected_last.as_str())
        );
    }

    #[test]
    fn append_output_keeps_recent_tail_with_truncation_notice() {
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("Large output task", None);
        let output = "한".repeat((MAX_TASK_OUTPUT_BYTES / "한".len()) + 100);

        registry
            .append_output(&task.task_id, &output)
            .expect("append should succeed");

        let stored = registry.output(&task.task_id).expect("output should exist");
        assert!(stored.len() <= MAX_TASK_OUTPUT_BYTES);
        assert!(stored.starts_with(TASK_OUTPUT_TRUNCATED_PREFIX));
        assert!(stored.ends_with('한'));
    }

    #[test]
    fn assigns_team_and_removes_task() {
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("Team task", None);
        registry
            .assign_team(&task.task_id, "team_abc")
            .expect("assign should succeed");

        let fetched = registry.get(&task.task_id).unwrap();
        assert_eq!(fetched.team_id.as_deref(), Some("team_abc"));

        let removed = registry.remove(&task.task_id);
        assert!(removed.is_some());
        assert!(registry.get(&task.task_id).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn rejects_operations_on_missing_task() {
        let registry = TaskRegistry::new_in_memory();
        assert!(registry.stop("nonexistent").is_err());
        assert!(registry.update("nonexistent", "msg").is_err());
        assert!(registry.output("nonexistent").is_err());
        assert!(registry.append_output("nonexistent", "data").is_err());
        assert!(registry
            .set_status("nonexistent", TaskStatus::Running)
            .is_err());
    }

    #[test]
    fn task_status_display_all_variants() {
        // given
        let cases = [
            (TaskStatus::Created, "created"),
            (TaskStatus::Running, "running"),
            (TaskStatus::Completed, "completed"),
            (TaskStatus::Failed, "failed"),
            (TaskStatus::Stopped, "stopped"),
        ];

        // when
        let rendered: Vec<_> = cases
            .into_iter()
            .map(|(status, expected)| (status.to_string(), expected))
            .collect();

        // then
        assert_eq!(
            rendered,
            vec![
                ("created".to_string(), "created"),
                ("running".to_string(), "running"),
                ("completed".to_string(), "completed"),
                ("failed".to_string(), "failed"),
                ("stopped".to_string(), "stopped"),
            ]
        );
    }

    #[test]
    fn stop_rejects_completed_task() {
        // given
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("done", None);
        registry
            .set_status(&task.task_id, TaskStatus::Completed)
            .expect("set status should succeed");

        // when
        let result = registry.stop(&task.task_id);

        // then
        let error = result.expect_err("completed task should be rejected");
        assert!(error.contains("already in terminal state"));
        assert!(error.contains("completed"));
    }

    #[test]
    fn stop_rejects_failed_task() {
        // given
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("failed", None);
        registry
            .set_status(&task.task_id, TaskStatus::Failed)
            .expect("set status should succeed");

        // when
        let result = registry.stop(&task.task_id);

        // then
        let error = result.expect_err("failed task should be rejected");
        assert!(error.contains("already in terminal state"));
        assert!(error.contains("failed"));
    }

    #[test]
    fn stop_succeeds_from_created_state() {
        // given
        let registry = TaskRegistry::new_in_memory();
        let task = registry.create("created task", None);

        // when
        let stopped = registry.stop(&task.task_id).expect("stop should succeed");

        // then
        assert_eq!(stopped.status, TaskStatus::Stopped);
        assert!(stopped.updated_at >= task.updated_at);
    }

    #[test]
    fn new_registry_is_empty() {
        // given
        let registry = TaskRegistry::new_in_memory();

        // when
        let all_tasks = registry.list(None);

        // then
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(all_tasks.is_empty());
    }

    #[test]
    fn create_without_description() {
        // given
        let registry = TaskRegistry::new_in_memory();

        // when
        let task = registry.create("Do the thing", None);

        // then
        assert!(task.task_id.starts_with("task_"));
        assert_eq!(task.description, None);
        assert_eq!(task.task_packet, None);
        assert!(task.messages.is_empty());
        assert!(task.output.is_empty());
        assert_eq!(task.team_id, None);
    }

    #[test]
    fn remove_nonexistent_returns_none() {
        // given
        let registry = TaskRegistry::new_in_memory();

        // when
        let removed = registry.remove("missing");

        // then
        assert!(removed.is_none());
    }

    #[test]
    fn assign_team_rejects_missing_task() {
        // given
        let registry = TaskRegistry::new_in_memory();

        // when
        let result = registry.assign_team("missing", "team_123");

        // then
        let error = result.expect_err("missing task should be rejected");
        assert_eq!(error, "task not found: missing");
    }

    #[test]
    fn prunes_old_terminal_tasks_but_preserves_active_tasks() {
        let registry = TaskRegistry::new_in_memory();
        let active = registry.create("active", None);
        registry
            .set_status(&active.task_id, TaskStatus::Running)
            .expect("active should run");

        for i in 0..(MAX_TASK_REGISTRY_ENTRIES + 3) {
            let task = registry.create(&format!("done {i}"), None);
            registry
                .set_status(&task.task_id, TaskStatus::Completed)
                .expect("terminal status should set");
        }

        assert!(registry.len() <= MAX_TASK_REGISTRY_ENTRIES);
        assert_eq!(
            registry.get(&active.task_id).map(|task| task.status),
            Some(TaskStatus::Running),
            "running task must not be pruned to satisfy retention"
        );
    }

    #[test]
    fn persisted_nonterminal_background_task_is_not_restored_as_live() {
        let root = std::env::temp_dir().join(format!(
            "task-registry-stale-live-test-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let path = root.join("tasks.json");
        let registry = TaskRegistry::with_persistence_path(Some(path.clone()));
        let task = registry.create_background_process(
            "sleep 30",
            Some("background bash"),
            Some("session-a"),
        );
        registry
            .set_status(&task.task_id, TaskStatus::Running)
            .expect("running status should persist");
        assert_eq!(
            registry
                .live_background_process_count(Some("session-a"))
                .load(),
            1
        );
        drop(registry);

        let restored = TaskRegistry::with_persistence_path(Some(path));
        assert_eq!(
            restored
                .get(&task.task_id)
                .map(|task| (task.kind, task.status)),
            Some((TaskKind::BackgroundProcess, TaskStatus::Running)),
            "TaskList history remains available"
        );
        assert_eq!(
            restored
                .live_background_process_count(Some("session-a"))
                .load(),
            0,
            "persisted status is not evidence of a live process after restart"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn legacy_task_json_defaults_to_generic_and_unstamped() {
        let task: Task = serde_json::from_value(serde_json::json!({
            "task_id": "legacy",
            "prompt": "old task",
            "description": null,
            "task_packet": null,
            "status": "running",
            "created_at": 1,
            "updated_at": 1,
            "messages": [],
            "output": "",
            "team_id": null
        }))
        .expect("legacy task should deserialize");

        assert_eq!(task.kind, TaskKind::Generic);
        assert_eq!(task.session_id, None);
    }

    #[test]
    fn persists_tasks_to_disk_when_configured() {
        let root = std::env::temp_dir().join(format!("task-registry-test-{}", now_secs()));
        let path = root.join("tasks.json");
        let registry = TaskRegistry::with_persistence_path(Some(path.clone()));
        let created = registry.create("Persist me", Some("disk-backed"));
        registry
            .append_output(&created.task_id, "done")
            .expect("append should succeed");

        let restored = TaskRegistry::with_persistence_path(Some(path.clone()));
        let fetched = restored.get(&created.task_id).expect("task should reload");
        assert_eq!(fetched.prompt, "Persist me");
        assert_eq!(fetched.output, "done");

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn removed_task_is_not_resurrected_by_on_disk_merge() {
        let root = std::env::temp_dir().join(format!("task-remove-nores-{}", unique_suffix()));
        let path = root.join("tasks.json");

        // A prior process persisted this task to disk.
        let writer = TaskRegistry::with_persistence_path(Some(path.clone()));
        let created = writer.create("resurrect me?", None);
        drop(writer);

        // This process loads that on-disk copy, then removes the task. The
        // remove's read/merge/write reloads the on-disk copy that still holds
        // the entry; a plain union merge would revive it.
        let remover = TaskRegistry::with_persistence_path(Some(path.clone()));
        assert!(remover.get(&created.task_id).is_some());
        assert!(remover.remove(&created.task_id).is_some());
        assert!(remover.get(&created.task_id).is_none());
        drop(remover);

        // Reload from disk: the removal must be durable.
        let reloaded = TaskRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&created.task_id).is_none(),
            "removed task was resurrected from disk"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn pruned_task_is_not_resurrected_by_on_disk_merge() {
        let root = std::env::temp_dir().join(format!("task-prune-nores-{}", unique_suffix()));
        let path = root.join("tasks.json");

        let registry = TaskRegistry::with_persistence_path(Some(path.clone()));
        // Overflow the terminal-task cap so the oldest terminal tasks are
        // pruned. Each task is completed so it is eligible for eviction.
        let mut ids = Vec::new();
        for i in 0..(MAX_TASK_REGISTRY_ENTRIES + 4) {
            let task = registry.create(&format!("task {i}"), None);
            registry
                .set_status(&task.task_id, TaskStatus::Completed)
                .expect("set terminal status");
            ids.push(task.task_id);
        }
        // The earliest ids were pruned out of the live set.
        let pruned: Vec<String> = ids
            .iter()
            .filter(|id| registry.get(id).is_none())
            .cloned()
            .collect();
        assert!(!pruned.is_empty(), "expected some tasks to be pruned");
        drop(registry);

        let reloaded = TaskRegistry::with_persistence_path(Some(path.clone()));
        for id in &pruned {
            assert!(
                reloaded.get(id).is_none(),
                "pruned task {id} was resurrected from disk"
            );
        }

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn two_task_registries_on_same_base_create_distinct_ids() {
        let root = std::env::temp_dir().join(format!("task-concurrent-create-{}", unique_suffix()));
        let path = root.join("tasks.json");

        // A and B both open the SAME empty on-disk registry, so they share base
        // revision 0 and counter 0. Before the fix, both minted
        // `task_<ts>_1` in the same second — an id collision that silently
        // collapsed the two creates into one on reload. The create now allocates
        // its counter/id INSIDE the cross-process lock, after merging the peer's
        // committed write, so B rebases onto A's counter.
        let a = TaskRegistry::with_persistence_path(Some(path.clone()));
        let b = TaskRegistry::with_persistence_path(Some(path.clone()));

        let task_a = a.create("alpha", None);
        let task_b = b.create("bravo", None);

        assert_ne!(
            task_a.task_id, task_b.task_id,
            "concurrent creates on the same base produced a colliding task id"
        );

        // Both survive a fresh reload from disk: neither create was overwritten.
        let reloaded = TaskRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&task_a.task_id).is_some(),
            "task A was lost by a same-base concurrent create"
        );
        assert!(
            reloaded.get(&task_b.task_id).is_some(),
            "task B was lost by a same-base concurrent create"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn same_base_concurrent_task_updates_serialize_without_tie_overwrite() {
        let root = std::env::temp_dir().join(format!("task-concurrent-update-{}", unique_suffix()));
        let path = root.join("tasks.json");

        // Seed one task, then open two registries that both loaded that same
        // base revision.
        let seed = TaskRegistry::with_persistence_path(Some(path.clone()));
        let task = seed.create("seed", None);
        let base_rev = seed.get(&task.task_id).expect("seed present").rev;
        drop(seed);

        let a = TaskRegistry::with_persistence_path(Some(path.clone()));
        let b = TaskRegistry::with_persistence_path(Some(path.clone()));
        assert_eq!(a.get(&task.task_id).expect("A sees task").rev, base_rev);
        assert_eq!(b.get(&task.task_id).expect("B sees task").rev, base_rev);

        // Both append to the same task. Before the fix each stamped rev =
        // base + 1 and the merge tie-rule kept `self`, so B silently overwrote
        // A's append. With the lock-serialized rebase, B allocates its revision
        // after merging A's committed write, so the task's revision strictly
        // advances by two and both appends are retained.
        a.append_output(&task.task_id, "A-output").expect("A append");
        b.append_output(&task.task_id, "B-output").expect("B append");

        let reloaded = TaskRegistry::with_persistence_path(Some(path.clone()));
        let final_task = reloaded.get(&task.task_id).expect("task survives");
        assert_eq!(
            final_task.rev,
            base_rev + 2,
            "same-base concurrent updates did not serialize; a tie silently overwrote one write"
        );
        assert!(
            final_task.output.contains("A-output") && final_task.output.contains("B-output"),
            "a concurrent append was lost to a silent tie overwrite: {:?}",
            final_task.output
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn stale_peer_cannot_resurrect_a_deleted_task_after_tombstone_cap_overflow() {
        // Defect: tombstones were capped and the lowest-revision ones evicted, so
        // a peer that held a deleted task on disk (untouched) could resurrect it
        // once enough later removals aged its tombstone out. Removal must be
        // durable regardless of how many later deletions occur, so tombstones are
        // now unbounded. `stale` captures an on-disk snapshot that still contains
        // the victim task; `live` then removes the victim and overflows the old
        // tombstone cap with many further removals; when `stale` finally persists
        // its old snapshot, the victim must stay dead.
        let root = std::env::temp_dir().join(format!("task-tombstone-resurrect-{}", unique_suffix()));
        let path = root.join("tasks.json");

        let live = TaskRegistry::with_persistence_path(Some(path.clone()));
        let victim = live.create("victim", None);
        // Pre-create the tasks the stale peer must NOT know about yet, so its
        // snapshot is genuinely stale.
        let victim_id = victim.task_id.clone();

        // The stale peer loads a snapshot that still contains the victim.
        let stale = TaskRegistry::with_persistence_path(Some(path.clone()));
        assert!(stale.get(&victim_id).is_some(), "stale peer must see the victim");

        // Live removes the victim, then performs many more create+remove cycles
        // to overflow the former tombstone cap (MAX_TASK_REGISTRY_ENTRIES) so the
        // victim's tombstone would previously have been evicted.
        live.remove(&victim_id).expect("remove victim");
        for i in 0..(MAX_TASK_REGISTRY_ENTRIES * 3) {
            let t = live.create(&format!("filler {i}"), None);
            live.remove(&t.task_id).expect("remove filler");
        }
        assert!(live.get(&victim_id).is_none(), "victim removed from live");

        // The stale peer (whose in-memory snapshot still contains the victim)
        // performs any persisting mutation, which re-merges disk and rewrites the
        // file. That merge must honor the durable tombstone and keep the victim
        // dead rather than unioning the stale victim back in.
        let _ = stale.create("stale-write", None);

        let reloaded = TaskRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&victim_id).is_none(),
            "a deleted task was resurrected by a stale peer after tombstone eviction"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn failed_persist_is_reported_and_never_clobbers_a_peer_commit_on_retry() {
        // Contract (fail-closed latch): a status-aware mutation whose persist
        // fails must be reported as an error (not a silent success); the instance
        // then latches fail-closed and never writes to disk again, so a peer's
        // committed write can never be clobbered — even after the write path is
        // "repaired" and the failed instance keeps mutating.
        let root = std::env::temp_dir().join(format!("task-retry-clobber-{}", unique_suffix()));
        let path = root.join("tasks.json");

        let seed = TaskRegistry::with_persistence_path(Some(path.clone()));
        let task = seed.create("seed", None);
        let task_id = task.task_id.clone();
        drop(seed);

        // A and B both load the same base revision.
        let a = TaskRegistry::with_persistence_path(Some(path.clone()));
        let b = TaskRegistry::with_persistence_path(Some(path.clone()));

        // B's persist fails (unwritable target). The mutation is reported as an
        // error, but stays visible LOCALLY (truthful reads), and B is now latched.
        make_registry_path_unwritable(&path);
        let b_result = b.append_output(&task_id, "B-1");
        assert!(
            b_result.is_err(),
            "a mutation whose persist failed must be reported as an error, not a silent success"
        );
        assert!(
            b.get(&task_id).expect("B still sees the task").output.contains("B-1"),
            "a failed mutation must stay visible locally (truthful reads)"
        );
        restore_registry_path(&path);

        // A commits successfully after B's failed attempt.
        a.append_output(&task_id, "A-1").expect("A append");

        // B keeps mutating on a now-repaired path; because B is latched, these are
        // memory-only and can never reach disk to clobber A.
        let b_retry = b.append_output(&task_id, "B-2");
        assert!(
            b_retry.is_err(),
            "a latched instance must keep reporting persistence errors"
        );

        let reloaded = TaskRegistry::with_persistence_path(Some(path.clone()));
        let final_task = reloaded.get(&task_id).expect("task survives");
        assert!(
            final_task.output.contains("A-1"),
            "a peer's committed write was clobbered after a failed persist: {:?}",
            final_task.output
        );
        assert!(
            !final_task.output.contains("B-1") && !final_task.output.contains("B-2"),
            "a latched instance's memory-only writes must never reach disk: {:?}",
            final_task.output
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn failed_create_stays_visible_locally_but_never_reaches_disk() {
        // Best-effort/infallible API contract: `create` returns a Task; when the
        // persist fails, that Task must still exist for this instance's own
        // get/list (no false-negative locally) AND a subsequent local create must
        // also be visible — but none of it may reach disk (fail-closed), so a peer
        // is never clobbered.
        let root = std::env::temp_dir().join(format!("task-failed-create-{}", unique_suffix()));
        let path = root.join("tasks.json");

        // A peer seeds a durable task so we can prove it is never clobbered.
        let peer = TaskRegistry::with_persistence_path(Some(path.clone()));
        let peer_task = peer.create("peer", None);
        let peer_id = peer_task.task_id.clone();
        drop(peer);

        let reg = TaskRegistry::with_persistence_path(Some(path.clone()));
        make_registry_path_unwritable(&path);

        let created = reg.create("local-1", None);
        assert!(
            reg.get(&created.task_id).is_some(),
            "a failed create must still be visible to the creating instance's get"
        );
        assert!(
            reg.list(None).iter().any(|t| t.task_id == created.task_id),
            "a failed create must still appear in the creating instance's list"
        );

        // A second local create is also visible locally (consistent memory-only).
        let created2 = reg.create("local-2", None);
        assert!(
            reg.get(&created2.task_id).is_some(),
            "a subsequent local create must be visible locally after latching"
        );
        restore_registry_path(&path);

        // Nothing the latched instance did reached disk; the peer's task survives
        // and the local tasks are absent on a fresh reload.
        let reloaded = TaskRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&peer_id).is_some(),
            "a peer's committed task was clobbered by a latched instance"
        );
        assert!(
            reloaded.get(&created.task_id).is_none() && reloaded.get(&created2.task_id).is_none(),
            "a latched instance's memory-only creates must never reach disk"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn failed_remove_actually_removes_locally_and_never_reaches_disk() {
        // Best-effort `remove` contract: it returns `Some(removed)` and the task
        // must actually be gone from THIS instance (no false success where the
        // task lingers), while the removal never reaches disk (fail-closed) so a
        // peer is not clobbered.
        let root = std::env::temp_dir().join(format!("task-failed-remove-{}", unique_suffix()));
        let path = root.join("tasks.json");

        let reg = TaskRegistry::with_persistence_path(Some(path.clone()));
        let task = reg.create("to-remove", None);
        let task_id = task.task_id.clone();

        make_registry_path_unwritable(&path);
        let removed = reg.remove(&task_id);
        assert!(removed.is_some(), "remove must report the removed task");
        assert!(
            reg.get(&task_id).is_none(),
            "a failed remove must still remove the task from the local instance"
        );
        restore_registry_path(&path);

        // The removal never reached disk (fail-closed): a fresh reload still has
        // the task, matching the durable on-disk state at the time of failure.
        let reloaded = TaskRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&task_id).is_some(),
            "a latched instance's memory-only removal must not have reached disk"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn failed_background_create_keeps_gauge_consistent_with_returned_task() {
        // The live-background gauge must reflect the actually-returned local task
        // even when persistence fails: the returned task exists locally, so the
        // gauge counting it is consistent (not a phantom).
        let root = std::env::temp_dir().join(format!("task-failed-bg-{}", unique_suffix()));
        let path = root.join("tasks.json");

        let reg = TaskRegistry::with_persistence_path(Some(path.clone()));
        make_registry_path_unwritable(&path);

        let bg = reg.create_background_process("bg", None, Some("sess-1"));
        // The returned task is visible locally, so the gauge entry for it is
        // backed by a real local task rather than a phantom.
        assert!(
            reg.get(&bg.task_id).is_some(),
            "a failed background create must still be visible locally"
        );
        assert_eq!(
            reg.live_background_process_count(Some("sess-1")).load(),
            1,
            "the live-background gauge must match the returned local task"
        );
        restore_registry_path(&path);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    /// Force the atomic write to fail WITHOUT destroying any existing committed
    /// file: make the parent directory read-only so the same-directory temp file
    /// cannot be created (and `create_dir_all` on an existing dir still
    /// succeeds). The peer's already-persisted file is left intact, so a test can
    /// prove the latched instance never clobbers it.
    #[cfg(unix)]
    fn make_registry_path_unwritable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let parent = path.parent().expect("registry path has a parent");
        fs::create_dir_all(parent).expect("ensure parent exists");
        fs::set_permissions(parent, fs::Permissions::from_mode(0o555))
            .expect("make parent read-only to force write failure");
    }

    #[cfg(unix)]
    fn restore_registry_path(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let parent = path.parent().expect("registry path has a parent");
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o755));
    }

    fn unique_suffix() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!(
            "{}-{}-{}",
            std::process::id(),
            now_secs(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }
}
