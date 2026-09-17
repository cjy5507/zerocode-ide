//! Process-local fast path for fixed orchestration mail pointers.
//!
//! Routes and outcomes are hints, never ledger state. The durable inbox still
//! decides whether mail exists and `zerocode-orc check` still owns delivery;
//! this hub merely offers the same fixed pointer to a provider route before
//! the existing PTY path is used.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use zerocode_hookd::session_notify::{
    NoticeId, NotificationOutcome, PointerNotice, SessionNotifier,
};

/// Enough admission room for a burst from every visible pane without allowing
/// a stalled provider to turn pointer hints into an unbounded memory queue.
const NOTIFICATION_QUEUE_CAPACITY: usize = 64;

/// Two fixed workers let one slow provider route make progress independently
/// of another while preserving a small, auditable concurrency ceiling.
const NOTIFICATION_WORKERS: usize = 2;

/// Notification workers run no recursive parser or large stack object. A
/// small explicit stack avoids reserving the platform thread default per
/// optional fast-path worker.
const NOTIFICATION_WORKER_STACK_BYTES: usize = 256 * 1024;

/// Provider adapters are required to finish sooner; this outer fence prevents
/// a defective adapter from suppressing the proven-safe PTY fallback forever.
const NOTIFICATION_RESULT_DEADLINE: Duration = Duration::from_secs(10);

/// One incarnation of a provider route attached to a terminal.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RouteGeneration(u64);

impl std::fmt::Debug for RouteGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("RouteGeneration")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone)]
struct RouteEntry {
    generation: RouteGeneration,
    notifier: Arc<dyn SessionNotifier>,
}

impl std::fmt::Debug for RouteEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RouteEntry")
            .field("generation", &self.generation)
            .field("notifier", &"<redacted-provider-route>")
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct AttemptKey {
    term: u32,
    generation: RouteGeneration,
}

enum AttemptState {
    Pending { started_at: Instant },
    Finished(NotificationOutcome),
}

struct AttemptSlot {
    notice: NoticeId,
    state: AttemptState,
}

struct NotificationJob {
    key: AttemptKey,
    notice: PointerNotice,
    notifier: Arc<dyn SessionNotifier>,
}

struct HubInner {
    routes: Mutex<HashMap<u32, RouteEntry>>,
    attempts: Mutex<HashMap<AttemptKey, AttemptSlot>>,
    next_generation: AtomicU64,
    sender: Mutex<Option<SyncSender<NotificationJob>>>,
    result_deadline: Duration,
}

impl HubInner {
    fn next_generation(&self) -> RouteGeneration {
        loop {
            let generation = self
                .next_generation
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
            if generation != 0 {
                return RouteGeneration(generation);
            }
        }
    }

    fn finish_if_current(&self, key: AttemptKey, notice: &NoticeId, outcome: NotificationOutcome) {
        let mut attempts = self
            .attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(slot) = attempts.get_mut(&key) else {
            return;
        };
        if slot.notice == *notice && matches!(slot.state, AttemptState::Pending { .. }) {
            slot.state = AttemptState::Finished(outcome);
        }
    }

    fn remove_route(&self, term: u32, generation: RouteGeneration) {
        let removed = {
            let mut routes = self
                .routes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if routes
                .get(&term)
                .is_some_and(|route| route.generation == generation)
            {
                routes.remove(&term);
                true
            } else {
                false
            }
        };
        if removed {
            self.attempts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .retain(|key, _| key.term != term || key.generation != generation);
        }
    }

    fn forget_term(&self, term: u32) {
        self.routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&term);
        self.attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|key, _| key.term != term);
    }
}

/// What the pointer pass should do after consulting the native fast path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerOffer {
    /// An admitted provider attempt has not reached a final outcome yet.
    NativePending,
    /// The provider confirmed acceptance; no PTY pointer is needed.
    Confirmed,
    /// Use the PTY path. `Unknown` is safe here because the payload is an
    /// idempotent pointer, not mail or a completion receipt.
    PtyFallback(NotificationOutcome),
}

impl PointerOffer {
    pub(crate) const fn needs_pty(self) -> bool {
        matches!(self, Self::PtyFallback(_))
    }
}

/// Ownership of one registered terminal route.
///
/// Drop removes only the generation this lease installed. A replacement may
/// therefore be registered before an old sidecar finishes shutting down
/// without the old lease deleting the new route.
pub(crate) struct RouteLease {
    hub: Weak<HubInner>,
    term: u32,
    generation: RouteGeneration,
}

impl RouteLease {
    #[must_use]
    pub(crate) const fn generation(&self) -> RouteGeneration {
        self.generation
    }
}

impl std::fmt::Debug for RouteLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RouteLease")
            .field("term", &self.term)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Drop for RouteLease {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.remove_route(self.term, self.generation);
        }
    }
}

/// Bounded, generation-fenced router for provider notification adapters.
pub(crate) struct SessionNotificationHub {
    inner: Arc<HubInner>,
    workers: Vec<JoinHandle<()>>,
}

impl SessionNotificationHub {
    fn new() -> Self {
        Self::with_limits(
            NOTIFICATION_QUEUE_CAPACITY,
            NOTIFICATION_WORKERS,
            NOTIFICATION_RESULT_DEADLINE,
        )
    }

    fn with_limits(capacity: usize, workers: usize, result_deadline: Duration) -> Self {
        assert!(capacity > 0, "notification queue capacity must be positive");
        assert!(workers > 0, "notification worker count must be positive");
        let (sender, receiver) = sync_channel(capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let inner = Arc::new(HubInner {
            routes: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(0),
            sender: Mutex::new(Some(sender)),
            result_deadline,
        });
        let mut joined = Vec::with_capacity(workers);
        for index in 0..workers {
            let receiver = Arc::clone(&receiver);
            let hub = Arc::downgrade(&inner);
            if let Ok(worker) = std::thread::Builder::new()
                .name(format!("zerocode-session-notify-{index}"))
                .stack_size(NOTIFICATION_WORKER_STACK_BYTES)
                .spawn(move || notification_worker(receiver, hub))
            {
                joined.push(worker);
            }
        }
        Self {
            inner,
            workers: joined,
        }
    }

    pub(crate) fn register_route(
        &self,
        term: u32,
        notifier: Arc<dyn SessionNotifier>,
    ) -> RouteLease {
        let generation = self.inner.next_generation();
        self.inner
            .routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                term,
                RouteEntry {
                    generation,
                    notifier,
                },
            );
        self.inner
            .attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|key, _| key.term != term);
        RouteLease {
            hub: Arc::downgrade(&self.inner),
            term,
            generation,
        }
    }

    pub(crate) fn offer(&self, term: u32, notice: PointerNotice) -> PointerOffer {
        let route = self
            .inner
            .routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&term)
            .cloned();
        let Some(route) = route else {
            return PointerOffer::PtyFallback(NotificationOutcome::DefinitelyUnsent);
        };
        let key = AttemptKey {
            term,
            generation: route.generation,
        };
        {
            let mut attempts = self
                .inner
                .attempts
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(slot) = attempts.get_mut(&key)
                && slot.notice == *notice.id()
            {
                return match slot.state {
                    AttemptState::Pending { started_at }
                        if started_at.elapsed() < self.inner.result_deadline =>
                    {
                        PointerOffer::NativePending
                    }
                    AttemptState::Pending { .. } => {
                        slot.state = AttemptState::Finished(NotificationOutcome::Unknown);
                        PointerOffer::PtyFallback(NotificationOutcome::Unknown)
                    }
                    AttemptState::Finished(NotificationOutcome::Confirmed) => {
                        PointerOffer::Confirmed
                    }
                    AttemptState::Finished(outcome) => PointerOffer::PtyFallback(outcome),
                };
            }
            attempts.insert(
                key,
                AttemptSlot {
                    notice: notice.id().clone(),
                    state: AttemptState::Pending {
                        started_at: Instant::now(),
                    },
                },
            );
        }

        let job = NotificationJob {
            key,
            notice,
            notifier: route.notifier,
        };
        let sender = self
            .inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(sender) = sender else {
            self.inner.finish_if_current(
                key,
                job.notice.id(),
                NotificationOutcome::DefinitelyUnsent,
            );
            return PointerOffer::PtyFallback(NotificationOutcome::DefinitelyUnsent);
        };
        match sender.try_send(job) {
            Ok(()) => PointerOffer::NativePending,
            Err(TrySendError::Full(job) | TrySendError::Disconnected(job)) => {
                self.inner.finish_if_current(
                    key,
                    job.notice.id(),
                    NotificationOutcome::DefinitelyUnsent,
                );
                PointerOffer::PtyFallback(NotificationOutcome::DefinitelyUnsent)
            }
        }
    }

    pub(crate) fn forget_term(&self, term: u32) {
        self.inner.forget_term(term);
    }

    fn has_route(&self, term: u32) -> bool {
        self.inner
            .routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&term)
    }
}

impl std::fmt::Debug for SessionNotificationHub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let routes = self
            .inner
            .routes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        let attempts = self
            .inner
            .attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        formatter
            .debug_struct("SessionNotificationHub")
            .field("routes", &routes)
            .field("attempts", &attempts)
            .field("workers", &self.workers.len())
            .finish()
    }
}

impl Drop for SessionNotificationHub {
    fn drop(&mut self) {
        self.inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn notification_worker(receiver: Arc<Mutex<Receiver<NotificationJob>>>, hub: Weak<HubInner>) {
    loop {
        let job = receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv();
        let Ok(job) = job else {
            return;
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| job.notifier.notify(&job.notice)))
            .unwrap_or(NotificationOutcome::Unknown);
        let Some(hub) = hub.upgrade() else {
            return;
        };
        hub.finish_if_current(job.key, job.notice.id(), outcome);
    }
}

static GLOBAL_HUB: OnceLock<SessionNotificationHub> = OnceLock::new();

fn global_hub() -> &'static SessionNotificationHub {
    GLOBAL_HUB.get_or_init(SessionNotificationHub::new)
}

pub(crate) fn offer(term: u32, notice: PointerNotice) -> PointerOffer {
    GLOBAL_HUB.get().map_or(
        PointerOffer::PtyFallback(NotificationOutcome::DefinitelyUnsent),
        |hub| hub.offer(term, notice),
    )
}

pub(crate) fn has_route(term: u32) -> bool {
    GLOBAL_HUB.get().is_some_and(|hub| hub.has_route(term))
}

pub(crate) fn register_route(term: u32, notifier: Arc<dyn SessionNotifier>) -> RouteLease {
    global_hub().register_route(term, notifier)
}

/// Forget without initializing an otherwise-unused optional fast path.
pub(crate) fn forget_term(term: u32) {
    if let Some(hub) = GLOBAL_HUB.get() {
        hub.forget_term(term);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

    use super::*;

    struct ScriptedNotifier {
        outcomes: Mutex<VecDeque<NotificationOutcome>>,
        calls: AtomicUsize,
    }

    impl ScriptedNotifier {
        fn new(outcomes: impl IntoIterator<Item = NotificationOutcome>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl SessionNotifier for ScriptedNotifier {
        fn notify(&self, _notice: &PointerNotice) -> NotificationOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.outcomes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or(NotificationOutcome::Unknown)
        }
    }

    struct BlockingNotifier {
        entered: SyncSender<()>,
        release: Mutex<Receiver<NotificationOutcome>>,
        calls: AtomicUsize,
    }

    impl SessionNotifier for BlockingNotifier {
        fn notify(&self, _notice: &PointerNotice) -> NotificationOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self.entered.send(());
            self.release
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .recv()
                .unwrap_or(NotificationOutcome::Unknown)
        }
    }

    fn notice(newest: &str) -> PointerNotice {
        PointerNotice::new("run-1", "worker:w-2", newest, 1).expect("a notice")
    }

    fn settled(hub: &SessionNotificationHub, term: u32, notice: &PointerNotice) -> PointerOffer {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let offered = hub.offer(term, notice.clone());
            if !matches!(offered, PointerOffer::NativePending) {
                return offered;
            }
            assert!(
                Instant::now() < deadline,
                "native notification did not settle"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn no_route_and_a_closed_queue_fall_back_as_definitely_unsent() {
        let hub = SessionNotificationHub::with_limits(1, 1, Duration::from_secs(1));
        assert_eq!(
            hub.offer(7, notice("m-1")),
            PointerOffer::PtyFallback(NotificationOutcome::DefinitelyUnsent)
        );

        hub.inner
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let _lease = hub.register_route(
            7,
            Arc::new(ScriptedNotifier::new([NotificationOutcome::Confirmed])),
        );
        assert_eq!(
            hub.offer(7, notice("m-1")),
            PointerOffer::PtyFallback(NotificationOutcome::DefinitelyUnsent)
        );
    }

    #[test]
    fn confirmed_unknown_and_new_watermarks_are_each_attempted_once() {
        let hub = SessionNotificationHub::with_limits(4, 1, Duration::from_secs(1));
        let notifier = Arc::new(ScriptedNotifier::new([
            NotificationOutcome::Unknown,
            NotificationOutcome::Confirmed,
        ]));
        let _lease = hub.register_route(8, notifier.clone());
        let first = notice("m-1");
        let second = notice("m-2");

        assert_eq!(
            settled(&hub, 8, &first),
            PointerOffer::PtyFallback(NotificationOutcome::Unknown)
        );
        assert_eq!(
            hub.offer(8, first),
            PointerOffer::PtyFallback(NotificationOutcome::Unknown)
        );
        assert_eq!(settled(&hub, 8, &second), PointerOffer::Confirmed);
        assert_eq!(notifier.calls.load(Ordering::SeqCst), 2);
        assert!(
            format!("{hub:?}").contains("attempts: 1"),
            "watermarks accumulated instead of replacing the route's one slot"
        );
    }

    #[test]
    fn route_replacement_fences_a_late_result_and_a_stale_lease_drop() {
        let hub = SessionNotificationHub::with_limits(4, 2, Duration::from_secs(1));
        let (entered_tx, entered_rx) = sync_channel(1);
        let (release_tx, release_rx) = sync_channel(1);
        let old = Arc::new(BlockingNotifier {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            calls: AtomicUsize::new(0),
        });
        let old_lease = hub.register_route(9, old);
        let pending = notice("m-1");
        assert_eq!(hub.offer(9, pending.clone()), PointerOffer::NativePending);
        entered_rx.recv().expect("the old route entered");

        let fresh = Arc::new(ScriptedNotifier::new([NotificationOutcome::Confirmed]));
        let fresh_lease = hub.register_route(9, fresh.clone());
        assert_ne!(old_lease.generation(), fresh_lease.generation());
        assert_eq!(settled(&hub, 9, &pending), PointerOffer::Confirmed);
        release_tx
            .send(NotificationOutcome::Unknown)
            .expect("release the old route");
        drop(old_lease);

        assert_eq!(hub.offer(9, pending), PointerOffer::Confirmed);
        assert_eq!(fresh.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_expired_attempt_falls_back_and_ignores_its_late_confirmation() {
        let hub = SessionNotificationHub::with_limits(2, 1, Duration::ZERO);
        let (entered_tx, entered_rx) = sync_channel(1);
        let (release_tx, release_rx) = sync_channel(1);
        let notifier = Arc::new(BlockingNotifier {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            calls: AtomicUsize::new(0),
        });
        let _lease = hub.register_route(10, notifier);
        let pending = notice("m-1");

        assert_eq!(hub.offer(10, pending.clone()), PointerOffer::NativePending);
        entered_rx.recv().expect("the route entered");
        assert_eq!(
            hub.offer(10, pending.clone()),
            PointerOffer::PtyFallback(NotificationOutcome::Unknown)
        );
        release_tx
            .send(NotificationOutcome::Confirmed)
            .expect("release the late result");
        assert_eq!(
            hub.offer(10, pending),
            PointerOffer::PtyFallback(NotificationOutcome::Unknown)
        );
    }

    #[test]
    fn bounded_admission_falls_back_without_running_the_overflow_job() {
        let hub = SessionNotificationHub::with_limits(1, 1, Duration::from_secs(2));
        let (entered_tx, entered_rx) = sync_channel(1);
        let (release_tx, release_rx) = sync_channel(1);
        let blocker = Arc::new(BlockingNotifier {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            calls: AtomicUsize::new(0),
        });
        let queued = Arc::new(ScriptedNotifier::new([NotificationOutcome::Confirmed]));
        let overflow = Arc::new(ScriptedNotifier::new([NotificationOutcome::Confirmed]));
        let _one = hub.register_route(11, blocker);
        let _two = hub.register_route(12, queued);
        let _three = hub.register_route(13, overflow.clone());

        assert_eq!(hub.offer(11, notice("m-1")), PointerOffer::NativePending);
        entered_rx.recv().expect("the worker is occupied");
        assert_eq!(hub.offer(12, notice("m-2")), PointerOffer::NativePending);
        assert_eq!(
            hub.offer(13, notice("m-3")),
            PointerOffer::PtyFallback(NotificationOutcome::DefinitelyUnsent)
        );
        assert_eq!(overflow.calls.load(Ordering::SeqCst), 0);
        release_tx
            .send(NotificationOutcome::Confirmed)
            .expect("release the worker");
    }

    #[test]
    fn terminal_cleanup_removes_the_route_and_fences_its_late_result() {
        let hub = SessionNotificationHub::with_limits(2, 1, Duration::from_secs(1));
        let (entered_tx, entered_rx) = sync_channel(1);
        let (release_tx, release_rx) = sync_channel(1);
        let notifier = Arc::new(BlockingNotifier {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            calls: AtomicUsize::new(0),
        });
        let lease = hub.register_route(14, notifier);
        assert_eq!(hub.offer(14, notice("m-1")), PointerOffer::NativePending);
        entered_rx.recv().expect("the route entered");

        hub.forget_term(14);
        release_tx
            .send(NotificationOutcome::Confirmed)
            .expect("release the removed route");
        assert_eq!(
            hub.offer(14, notice("m-1")),
            PointerOffer::PtyFallback(NotificationOutcome::DefinitelyUnsent)
        );
        drop(lease);
    }

    #[test]
    fn debug_surfaces_expose_counts_and_generations_but_no_provider_state() {
        let hub = SessionNotificationHub::with_limits(2, 1, Duration::from_secs(1));
        let provider_secret = "provider-socket-token-secret";
        let notifier = Arc::new(ScriptedNotifier::new([NotificationOutcome::Confirmed]));
        let lease = hub.register_route(15, notifier);
        let rendered = format!("{hub:?} {lease:?}");

        assert!(!rendered.contains(provider_secret));
        assert!(rendered.contains("routes: 1"));
        assert!(rendered.contains("RouteGeneration"));
    }
}
