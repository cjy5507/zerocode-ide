//! Event-driven invalidation for disk-backed TUI snapshots.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use zo_cli::tui::workspace_status::{
    WorkspaceStatusSource, session_workspace_status_source,
};

/// Maximum interval between self-healing scans while OS notifications are healthy.
pub(crate) const HEALTHY_HEAL_INTERVAL: Duration = Duration::from_secs(30);
/// Workspace scan cadence when the OS watcher could not be started.
pub(crate) const FAILED_WATCHER_WORKSPACE_INTERVAL: Duration = Duration::from_secs(1);
/// Agent/HUD scan cadence when the OS watcher could not be started.
pub(crate) const FAILED_WATCHER_AGENTS_INTERVAL: Duration = Duration::from_millis(500);
/// Wakeup scan cadence when the OS watcher could not be started.
pub(crate) const FAILED_WATCHER_WAKEUPS_INTERVAL: Duration = Duration::from_secs(1);
/// Maximum time Git operation events may suppress workspace invalidations.
const GIT_OP_MAX_HOLD: Duration = Duration::from_secs(30);
/// Git lock and state paths that indicate a mutating operation is active.
const GIT_OP_PATHS: [&str; 5] = [
    ".git/index.lock",
    ".git/rebase-merge",
    ".git/rebase-apply",
    ".git/MERGE_HEAD",
    ".git/CHERRY_PICK_HEAD",
];

/// A disk-backed TUI domain that can be invalidated independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FreshnessDomain {
    Workspace,
    Agents,
    Wakeups,
}

impl FreshnessDomain {
    const fn index(self) -> usize {
        match self {
            Self::Workspace => 0,
            Self::Agents => 1,
            Self::Wakeups => 2,
        }
    }
}

/// Event-layer gate that coalesces workspace invalidations during Git operations.
#[derive(Default)]
enum GitOpState {
    #[default]
    Idle,
    InProgress {
        entered_at: Instant,
        workspace_pending: bool,
    },
}

impl GitOpState {
    fn enter(&mut self, now: Instant) {
        if matches!(self, Self::Idle) {
            *self = Self::InProgress {
                entered_at: now,
                workspace_pending: false,
            };
        }
    }

    fn coalesce_workspace_mark(&mut self) -> bool {
        let Self::InProgress {
            workspace_pending, ..
        } = self
        else {
            return false;
        };
        *workspace_pending = true;
        true
    }

    fn held_too_long(&self, now: Instant) -> bool {
        matches!(
            self,
            Self::InProgress { entered_at, .. }
                if now.saturating_duration_since(*entered_at) >= GIT_OP_MAX_HOLD
        )
    }

    fn finish(&mut self) -> bool {
        let pending = matches!(
            self,
            Self::InProgress {
                workspace_pending: true,
                ..
            }
        );
        *self = Self::Idle;
        pending
    }

    fn is_in_progress(&self) -> bool {
        matches!(self, Self::InProgress { .. })
    }
}

/// Consumer boundary for event sources that invalidate disk-backed snapshots.
pub(crate) trait FreshnessSource: Send + Sync {
    /// Start a scan if an event or the self-heal cadence makes it due.
    ///
    /// The dirty bit is cleared before this returns `true`, so an event arriving
    /// during the scan remains observable for a follow-up scan.
    fn begin_scan(&self, domain: FreshnessDomain, now: Instant) -> bool;

    /// Shared dirty flag, used by long-running scans as their interrupt signal.
    fn dirty_flag(&self, domain: FreshnessDomain) -> Arc<AtomicBool>;

    /// Request a new scan without coupling the consumer to a concrete watcher.
    fn mark_dirty(&self, domain: FreshnessDomain);
}

/// Atomic invalidation flags plus the last-started time for each domain.
pub(crate) struct FreshnessSignals {
    workspace_dirty: Arc<AtomicBool>,
    agents_dirty: Arc<AtomicBool>,
    wakeups_dirty: Arc<AtomicBool>,
    watcher_healthy: AtomicBool,
    last_scan_started: Mutex<[Option<Instant>; 3]>,
    git_op_state: Mutex<GitOpState>,
    consecutive_failed_cycles: AtomicU32,
}

impl FreshnessSignals {
    fn new() -> Self {
        Self {
            workspace_dirty: Arc::new(AtomicBool::new(true)),
            agents_dirty: Arc::new(AtomicBool::new(true)),
            wakeups_dirty: Arc::new(AtomicBool::new(true)),
            watcher_healthy: AtomicBool::new(false),
            last_scan_started: Mutex::new([None; 3]),
            git_op_state: Mutex::new(GitOpState::default()),
            consecutive_failed_cycles: AtomicU32::new(0),
        }
    }

    fn set_watcher_healthy(&self, healthy: bool) {
        self.watcher_healthy.store(healthy, Ordering::Release);
    }

    fn mark_dirty(&self, domain: FreshnessDomain) {
        self.flag(domain).store(true, Ordering::Release);
    }

    fn mark_all_dirty(&self) {
        self.workspace_dirty.store(true, Ordering::Release);
        self.agents_dirty.store(true, Ordering::Release);
        self.wakeups_dirty.store(true, Ordering::Release);
    }

    fn mark_workspace_dirty_from_event(&self) {
        let suppressed = self
            .git_op_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .coalesce_workspace_mark();
        if !suppressed {
            self.mark_dirty(FreshnessDomain::Workspace);
        }
    }

    fn mark_all_dirty_from_event(&self) {
        self.mark_workspace_dirty_from_event();
        self.mark_dirty(FreshnessDomain::Agents);
        self.mark_dirty(FreshnessDomain::Wakeups);
    }

    fn enter_git_operation(&self, now: Instant) {
        self.git_op_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .enter(now);
    }

    fn finish_git_operation_if_clear(&self, workspace_root: &Path) {
        let pending = {
            let mut state = self
                .git_op_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.is_in_progress() || git_operation_paths_exist(workspace_root) {
                return;
            }
            state.finish()
        };
        if pending {
            self.mark_dirty(FreshnessDomain::Workspace);
        }
    }

    fn reverify_stale_git_operation(&self, workspace_root: &Path, now: Instant) {
        let pending = {
            let mut state = self
                .git_op_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.held_too_long(now) {
                return;
            }
            // Re-stat before applying the hard timeout. A surviving path may be
            // an orphaned lock, so it cannot extend suppression indefinitely.
            let _ = git_operation_paths_exist(workspace_root);
            state.finish()
        };
        if pending {
            self.mark_dirty(FreshnessDomain::Workspace);
        }
    }

    fn flag(&self, domain: FreshnessDomain) -> &Arc<AtomicBool> {
        match domain {
            FreshnessDomain::Workspace => &self.workspace_dirty,
            FreshnessDomain::Agents => &self.agents_dirty,
            FreshnessDomain::Wakeups => &self.wakeups_dirty,
        }
    }
}

impl FreshnessSource for FreshnessSignals {
    fn begin_scan(&self, domain: FreshnessDomain, now: Instant) -> bool {
        let dirty = self.flag(domain).swap(false, Ordering::AcqRel);
        let healthy = self.watcher_healthy.load(Ordering::Acquire);
        if healthy {
            self.consecutive_failed_cycles.store(0, Ordering::Release);
        }
        let consecutive_cycles = self.consecutive_failed_cycles.load(Ordering::Acquire);
        let mut last_started = self
            .last_scan_started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let due = dirty
            || heal_scan_due(
                last_started[domain.index()],
                now,
                domain,
                healthy,
                consecutive_cycles,
            );
        if due {
            last_started[domain.index()] = Some(now);
            if !healthy {
                self.consecutive_failed_cycles.fetch_add(1, Ordering::SeqCst);
            }
        }
        due
    }

    fn dirty_flag(&self, domain: FreshnessDomain) -> Arc<AtomicBool> {
        Arc::clone(self.flag(domain))
    }

    fn mark_dirty(&self, domain: FreshnessDomain) {
        FreshnessSignals::mark_dirty(self, domain);
    }
}

/// Owns the OS watcher for one interactive session. Dropping it stops notify's thread.
pub(crate) struct FreshnessWatcher {
    watcher: Option<RecommendedWatcher>,
    source: Arc<FreshnessSignals>,
}

impl FreshnessWatcher {
    /// Watch the workspace, agent manifests, and scheduled wakeups for one session.
    pub(crate) fn start(workspace_root: &Path) -> Self {
        let agent_store = tools::agent_store_dir().map_err(|error| format!("agent store: {error}"));
        let wakeups = super::wakeups::wakeups_dir();
        Self::start_with_paths(workspace_root, agent_store, &wakeups)
    }

    fn start_with_paths(
        workspace_root: &Path,
        agent_store: Result<PathBuf, String>,
        wakeups: &Path,
    ) -> Self {
        let source = Arc::new(FreshnessSignals::new());
        let setup = (|| {
            let agent_store = agent_store?;
            std::fs::create_dir_all(&agent_store)
                .map_err(|error| format!("create {}: {error}", agent_store.display()))?;
            std::fs::create_dir_all(wakeups)
                .map_err(|error| format!("create {}: {error}", wakeups.display()))?;
            let workspace_root = canonical_watch_root(workspace_root);
            let agent_store = canonical_watch_root(&agent_store);
            let wakeups = canonical_watch_root(wakeups);

            let event_source = Arc::clone(&source);
            let event_workspace = workspace_root.clone();
            let event_agents = agent_store.clone();
            let event_wakeups = wakeups.clone();
            let mut watcher = notify::recommended_watcher(move |result| {
                handle_notify_result(
                    result,
                    &event_source,
                    &event_workspace,
                    &event_agents,
                    &event_wakeups,
                );
            })
            .map_err(|error| error.to_string())?;
            watcher
                .watch(&workspace_root, RecursiveMode::Recursive)
                .map_err(|error| error.to_string())?;
            watcher
                .watch(&agent_store, RecursiveMode::Recursive)
                .map_err(|error| error.to_string())?;
            watcher
                .watch(&wakeups, RecursiveMode::NonRecursive)
                .map_err(|error| error.to_string())?;
            Ok::<_, String>(watcher)
        })();

        match setup {
            Ok(watcher) => {
                source.set_watcher_healthy(true);
                Self {
                    watcher: Some(watcher),
                    source,
                }
            }
            Err(error) => {
                source.mark_all_dirty();
                eprintln!(
                    "[zo] filesystem watcher unavailable ({error}); using polling fallback"
                );
                Self {
                    watcher: None,
                    source,
                }
            }
        }
    }

    pub(crate) fn source(&self) -> Arc<dyn FreshnessSource> {
        Arc::clone(&self.source) as Arc<dyn FreshnessSource>
    }

    #[cfg(test)]
    fn is_active(&self) -> bool {
        self.watcher.is_some()
    }
}

impl Drop for FreshnessWatcher {
    fn drop(&mut self) {
        drop(self.watcher.take());
    }
}

fn canonical_watch_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Session-scoped consumer handles; the watcher itself remains separately owned.
#[derive(Clone)]
pub(crate) struct SessionFreshness {
    source: Arc<dyn FreshnessSource>,
    workspace_status: Arc<dyn WorkspaceStatusSource>,
}

impl SessionFreshness {
    pub(crate) fn new(watcher: &FreshnessWatcher, workspace_root: &Path) -> Self {
        Self {
            source: watcher.source(),
            workspace_status: session_workspace_status_source(workspace_root),
        }
    }

    pub(crate) fn begin_scan(&self, domain: FreshnessDomain, now: Instant) -> bool {
        self.source.begin_scan(domain, now)
    }

    pub(crate) fn mark_dirty(&self, domain: FreshnessDomain) {
        self.source.mark_dirty(domain);
    }

    pub(crate) fn dirty_flag(&self, domain: FreshnessDomain) -> Arc<AtomicBool> {
        self.source.dirty_flag(domain)
    }

    pub(crate) fn workspace_status(&self) -> Arc<dyn WorkspaceStatusSource> {
        Arc::clone(&self.workspace_status)
    }
}

fn handle_notify_result(
    result: notify::Result<Event>,
    source: &FreshnessSignals,
    workspace_root: &Path,
    agent_store: &Path,
    wakeups: &Path,
) {
    handle_notify_result_at(
        result,
        source,
        workspace_root,
        agent_store,
        wakeups,
        Instant::now(),
    );
}

fn handle_notify_result_at(
    result: notify::Result<Event>,
    source: &FreshnessSignals,
    workspace_root: &Path,
    agent_store: &Path,
    wakeups: &Path,
    now: Instant,
) {
    source.reverify_stale_git_operation(workspace_root, now);

    let Ok(event) = result else {
        source.mark_all_dirty_from_event();
        return;
    };
    if event.need_rescan() {
        source.mark_all_dirty_from_event();
        return;
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return;
    }
    if event.paths.is_empty() {
        source.mark_all_dirty_from_event();
        return;
    }

    if matches!(event.kind, EventKind::Create(_))
        && event.paths.iter().any(|path| {
            is_git_operation_path(workspace_root, path) && std::fs::metadata(path).is_ok()
        })
    {
        source.enter_git_operation(now);
    }
    for path in event.paths {
        if path.starts_with(agent_store) {
            source.mark_dirty(FreshnessDomain::Agents);
        }
        if path.starts_with(wakeups) {
            source.mark_dirty(FreshnessDomain::Wakeups);
        }
        if workspace_event_affects_status(workspace_root, agent_store, &path) {
            source.mark_workspace_dirty_from_event();
        }
    }
    source.finish_git_operation_if_clear(workspace_root);
}

fn is_git_operation_path(workspace_root: &Path, path: &Path) -> bool {
    path.strip_prefix(workspace_root).is_ok_and(|relative| {
        GIT_OP_PATHS
            .iter()
            .any(|operation_path| relative == Path::new(operation_path))
    })
}

fn git_operation_paths_exist(workspace_root: &Path) -> bool {
    GIT_OP_PATHS
        .iter()
        .any(|path| std::fs::metadata(workspace_root.join(path)).is_ok())
}

fn workspace_event_affects_status(workspace_root: &Path, agent_store: &Path, path: &Path) -> bool {
    if path.starts_with(agent_store) {
        return false;
    }
    let Ok(relative) = path.strip_prefix(workspace_root) else {
        return false;
    };
    let mut components = relative.components();
    let Some(first) = components.next() else {
        return true;
    };
    if first.as_os_str() == ".git" {
        let Some(second) = components.next() else {
            return false;
        };
        return second.as_os_str() == "index"
            || second.as_os_str() == "HEAD"
            || second.as_os_str() == "refs";
    }
    !zo_cli::tui::sidebar::is_workspace_status_path_filtered(
        &relative.to_string_lossy(),
    )
}

pub(crate) fn failed_watcher_interval(base: Duration, consecutive_cycles: u32) -> Duration {
    let exponent = consecutive_cycles.min(3);
    base * (1 << exponent)
}

fn fallback_interval(domain: FreshnessDomain, watcher_healthy: bool, consecutive_cycles: u32) -> Duration {
    if watcher_healthy {
        return HEALTHY_HEAL_INTERVAL;
    }
    let base = match domain {
        FreshnessDomain::Workspace => FAILED_WATCHER_WORKSPACE_INTERVAL,
        FreshnessDomain::Agents => FAILED_WATCHER_AGENTS_INTERVAL,
        FreshnessDomain::Wakeups => FAILED_WATCHER_WAKEUPS_INTERVAL,
    };
    failed_watcher_interval(base, consecutive_cycles)
}

fn heal_scan_due(
    last_started: Option<Instant>,
    now: Instant,
    domain: FreshnessDomain,
    watcher_healthy: bool,
    consecutive_cycles: u32,
) -> bool {
    last_started.is_none_or(|last| {
        now.saturating_duration_since(last) >= fallback_interval(domain, watcher_healthy, consecutive_cycles)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind, RenameMode};

    const WATCHER_SMOKE_TIMEOUT: Duration = Duration::from_secs(5);
    const WATCHER_SMOKE_POLL_INTERVAL: Duration = Duration::from_millis(25);
    const CADENCE_BOUNDARY_MARGIN: Duration = Duration::from_millis(1);

    fn temp_workspace(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zo-freshness-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    fn send_event(
        signals: &FreshnessSignals,
        root: &Path,
        now: Instant,
        kind: EventKind,
        path: &Path,
    ) {
        handle_notify_result_at(
            Ok(Event::new(kind).add_path(path.to_path_buf())),
            signals,
            root,
            &root.join("agents"),
            &root.join("wakeups"),
            now,
        );
    }

    #[test]
    fn git_operation_coalesces_workspace_events_and_exits_once_when_all_paths_clear() {
        let root = temp_workspace("git-operation");
        let git_dir = root.join(".git");
        let lock = git_dir.join("index.lock");
        let rebase = git_dir.join("rebase-merge");
        std::fs::create_dir_all(&rebase).expect("create Git operation paths");
        std::fs::write(&lock, "").expect("create index lock");

        let signals = FreshnessSignals::new();
        signals.set_watcher_healthy(true);
        let now = Instant::now();
        assert!(signals.begin_scan(FreshnessDomain::Workspace, now));
        assert!(signals.begin_scan(FreshnessDomain::Agents, now));
        assert!(signals.begin_scan(FreshnessDomain::Wakeups, now));

        send_event(
            &signals,
            &root,
            now,
            EventKind::Create(CreateKind::File),
            &lock,
        );
        send_event(
            &signals,
            &root,
            now,
            EventKind::Modify(ModifyKind::Any),
            &root.join("changed.txt"),
        );
        send_event(
            &signals,
            &root,
            now,
            EventKind::Modify(ModifyKind::Any),
            &root.join("agents/agent.json"),
        );
        send_event(
            &signals,
            &root,
            now,
            EventKind::Modify(ModifyKind::Any),
            &root.join("wakeups/wakeup.json"),
        );

        assert!(!signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));
        assert!(signals
            .dirty_flag(FreshnessDomain::Agents)
            .load(Ordering::Acquire));
        assert!(signals
            .dirty_flag(FreshnessDomain::Wakeups)
            .load(Ordering::Acquire));

        std::fs::remove_file(&lock).expect("remove index lock");
        send_event(
            &signals,
            &root,
            now,
            EventKind::Remove(RemoveKind::File),
            &lock,
        );
        assert!(!signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));

        std::fs::remove_dir(&rebase).expect("remove rebase state");
        send_event(
            &signals,
            &root,
            now,
            EventKind::Remove(RemoveKind::Folder),
            &rebase,
        );
        assert!(signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));
        assert!(signals.begin_scan(FreshnessDomain::Workspace, now));

        send_event(
            &signals,
            &root,
            now,
            EventKind::Remove(RemoveKind::Folder),
            &rebase,
        );
        assert!(!signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn stale_git_operation_create_event_does_not_enter_when_stat_is_absent() {
        let root = temp_workspace("stale-git-operation");
        let lock = root.join(".git/index.lock");
        std::fs::create_dir_all(root.join(".git")).expect("create Git directory");

        let signals = FreshnessSignals::new();
        signals.set_watcher_healthy(true);
        let now = Instant::now();
        assert!(signals.begin_scan(FreshnessDomain::Workspace, now));

        send_event(
            &signals,
            &root,
            now,
            EventKind::Create(CreateKind::File),
            &lock,
        );
        send_event(
            &signals,
            &root,
            now,
            EventKind::Modify(ModifyKind::Any),
            &root.join("changed.txt"),
        );

        assert!(signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn git_operation_releases_on_index_rename_without_remove_event() {
        let root = temp_workspace("git-index-rename");
        let lock = root.join(".git/index.lock");
        std::fs::create_dir_all(root.join(".git")).expect("create Git directory");
        std::fs::write(&lock, "").expect("create index lock");

        let signals = FreshnessSignals::new();
        signals.set_watcher_healthy(true);
        let now = Instant::now();
        assert!(signals.begin_scan(FreshnessDomain::Workspace, now));

        send_event(
            &signals,
            &root,
            now,
            EventKind::Create(CreateKind::File),
            &lock,
        );
        send_event(
            &signals,
            &root,
            now,
            EventKind::Modify(ModifyKind::Any),
            &root.join("changed.txt"),
        );
        assert!(!signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));

        std::fs::remove_file(&lock).expect("rename index lock into place");
        send_event(
            &signals,
            &root,
            now,
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            &root.join(".git/index"),
        );

        assert!(signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn orphaned_git_lock_stops_suppressing_after_timeout_recheck() {
        let root = temp_workspace("orphaned-git-lock");
        let lock = root.join(".git/index.lock");
        std::fs::create_dir_all(root.join(".git")).expect("create Git directory");
        std::fs::write(&lock, "").expect("create orphaned index lock");

        let signals = FreshnessSignals::new();
        signals.set_watcher_healthy(true);
        let entered_at = Instant::now();
        assert!(signals.begin_scan(FreshnessDomain::Workspace, entered_at));

        send_event(
            &signals,
            &root,
            entered_at,
            EventKind::Create(CreateKind::File),
            &lock,
        );
        send_event(
            &signals,
            &root,
            entered_at,
            EventKind::Modify(ModifyKind::Any),
            &root.join("during-operation.txt"),
        );
        assert!(!signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));

        let timed_out = entered_at + GIT_OP_MAX_HOLD;
        send_event(
            &signals,
            &root,
            timed_out,
            EventKind::Access(AccessKind::Any),
            &root.join("after-timeout.txt"),
        );
        assert!(signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));
        assert!(signals.begin_scan(FreshnessDomain::Workspace, timed_out));

        send_event(
            &signals,
            &root,
            timed_out,
            EventKind::Modify(ModifyKind::Any),
            &root.join("after-timeout.txt"),
        );
        assert!(signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn workspace_noise_filter_keeps_only_status_relevant_git_metadata() {
        let root = Path::new("/workspace");
        let agents = root.join("agent-store");
        assert!(!workspace_event_affects_status(
            root,
            &agents,
            &root.join(".git/objects/ab/cd")
        ));
        assert!(workspace_event_affects_status(
            root,
            &agents,
            &root.join(".git/index")
        ));
        assert!(workspace_event_affects_status(
            root,
            &agents,
            &root.join(".git/HEAD")
        ));
        assert!(workspace_event_affects_status(
            root,
            &agents,
            &root.join(".git/refs/heads/main")
        ));
        assert!(!workspace_event_affects_status(
            root,
            &agents,
            &root.join("target/debug/zo")
        ));
        assert!(!workspace_event_affects_status(
            root,
            &agents,
            &root.join("target")
        ));
        assert!(!workspace_event_affects_status(
            root,
            &agents,
            &agents.join("agent.json")
        ));
    }

    #[test]
    fn clearing_at_scan_start_preserves_events_during_the_scan() {
        let signals = FreshnessSignals::new();
        signals.set_watcher_healthy(true);
        let now = Instant::now();
        assert!(signals.begin_scan(FreshnessDomain::Workspace, now));
        assert!(!signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));

        signals.mark_dirty(FreshnessDomain::Workspace);
        assert!(signals
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));
        assert!(signals.begin_scan(FreshnessDomain::Workspace, now));
    }

    #[test]
    fn heal_cadence_uses_slow_and_failed_watcher_intervals() {
        let now = Instant::now();
        let healthy_due = now
            .checked_sub(HEALTHY_HEAL_INTERVAL)
            .expect("test instant supports healthy cadence");
        let agents_due = now
            .checked_sub(FAILED_WATCHER_AGENTS_INTERVAL)
            .expect("test instant supports agent cadence");
        let workspace_due = now
            .checked_sub(FAILED_WATCHER_WORKSPACE_INTERVAL)
            .expect("test instant supports workspace cadence");
        assert!(!heal_scan_due(
            Some(
                healthy_due
                    .checked_add(CADENCE_BOUNDARY_MARGIN)
                    .expect("test instant supports boundary margin")
            ),
            now,
            FreshnessDomain::Workspace,
            true,
            0
        ));
        assert!(heal_scan_due(
            Some(healthy_due),
            now,
            FreshnessDomain::Workspace,
            true,
            0
        ));
        assert!(heal_scan_due(
            Some(agents_due),
            now,
            FreshnessDomain::Agents,
            false,
            0
        ));
        assert!(!heal_scan_due(
            Some(
                workspace_due
                    .checked_add(CADENCE_BOUNDARY_MARGIN)
                    .expect("test instant supports boundary margin")
            ),
            now,
            FreshnessDomain::Workspace,
            false,
            0
        ));
    }

    #[test]
    fn test_failed_watcher_interval_backoff() {
        let base = Duration::from_millis(500);
        assert_eq!(failed_watcher_interval(base, 0), base);
        assert_eq!(failed_watcher_interval(base, 1), base * 2);
        assert_eq!(failed_watcher_interval(base, 2), base * 4);
        assert_eq!(failed_watcher_interval(base, 3), base * 8);
        assert_eq!(failed_watcher_interval(base, 4), base * 8);
    }

    #[test]
    #[ignore = "depends on host filesystem notification support"]
    fn watcher_marks_workspace_dirty_after_a_write() {
        let root = std::env::temp_dir().join(format!(
            "zo-freshness-smoke-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let agents = root.join("agents");
        let wakeups = root.join("wakeups");
        std::fs::create_dir_all(&root).expect("temp workspace");
        let watcher = FreshnessWatcher::start_with_paths(&root, Ok(agents), &wakeups);
        if !watcher.is_active() {
            std::fs::remove_dir_all(root).ok();
            return;
        }
        let source = watcher.source();
        assert!(source.begin_scan(FreshnessDomain::Workspace, Instant::now()));

        std::fs::write(root.join("changed.txt"), "changed\n").expect("write watched file");
        let deadline = Instant::now() + WATCHER_SMOKE_TIMEOUT;
        while !source
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire)
            && Instant::now() < deadline
        {
            std::thread::sleep(WATCHER_SMOKE_POLL_INTERVAL);
        }
        assert!(source
            .dirty_flag(FreshnessDomain::Workspace)
            .load(Ordering::Acquire));
        drop(watcher);
        std::fs::remove_dir_all(root).ok();
    }
}
