//! Agent completion plumbing — broadcast channel + store + polling helpers.
//!
//! `AGENT_COMPLETION_TX` is the broadcast channel the TUI subscribes to
//! once at startup (status-only summary). `CompletionStore` keeps
//! recent full results indexed by agent id so the `GetAgentCompletion`
//! polling tool can resolve answers after the fact without growing for
//! the full process lifetime.
//!
//! `notify_agent_completion` is the single shared write path used by
//! every `execute_agent_*` variant in [`super`] — it stamps both
//! surfaces atomically so neither can drift relative to the other.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use core_types::helper_run::HelperRun;
use tokio::sync::mpsc as tokio_mpsc;

const COMPLETION_STORE_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_COMPLETION_STORE_ENTRIES: usize = 256;

/// Polling slice when a cooperative cancel flag is supplied to
/// [`CompletionStore::wait_for_all`]. Short enough to observe a foreground
/// Ctrl+C promptly during a long collection window or unbounded wait; only used
/// when a cancel flag is present, so the no-cancel path keeps blocking on the
/// condvar with no busy-poll.
const CANCEL_POLL_SLICE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct AgentCompletion {
    pub agent_id: String,
    pub name: String,
    pub status: String,
    pub result: Option<String>,
    /// Structured result captured from the agent's `StructuredOutput` tool call
    /// when its workflow phase declared a `schema` (8c). `None` for free-text
    /// agents and when the agent never called the tool.
    pub structured: Option<serde_json::Value>,
    pub error: Option<String>,
    /// What this sub-agent's run cost: tools started, output tokens spent, and
    /// wall-clock time. Every field is `0`/`None` when nothing was measured — a
    /// `still_running` placeholder, a spawn that never streamed, or a backend
    /// that does not track usage — and each such part simply drops out of the
    /// rendered summary rather than printing as a zero.
    ///
    /// `run.output_tokens` is the never-lossy total the workflow engine folds
    /// into the optional `max_output_tokens` budget; consumers needing input
    /// tokens or a cost figure must collect those separately (input tokens are
    /// not yet on this surface). `run.tool_calls`/`run.elapsed` come from the
    /// agent's own manifest, which is what actually counted them.
    pub run: HelperRun,
}

const PROVIDER_ERROR_CLASS_FIELD: &str = "providerErrorClass";

#[must_use]
pub fn provider_error_class_metadata(
    provider_error_class: api::ProviderErrorClass,
) -> serde_json::Value {
    let label = match provider_error_class {
        // The scope rides the label so the parent can tell a sub-agent's account
        // throttle (ride it out) from a provider overload (escape to a lighter
        // model). A pre-scope `"rateLimit"` from an older peer still decodes, as
        // the account case — the conservative direction, since reading a 429 as an
        // overload would swap models on a window every model here shares.
        api::ProviderErrorClass::RateLimit {
            scope: api::CapacityScope::Account,
            ..
        } => "rateLimit",
        api::ProviderErrorClass::RateLimit {
            scope: api::CapacityScope::Provider,
            ..
        } => "providerOverloaded",
        api::ProviderErrorClass::Transient => "transient",
        api::ProviderErrorClass::AuthExpired => "authExpired",
        api::ProviderErrorClass::ContextOverflow => "contextOverflow",
        api::ProviderErrorClass::InvalidToolProtocol => "invalidToolProtocol",
        api::ProviderErrorClass::InvalidToolSchema => "invalidToolSchema",
        api::ProviderErrorClass::SafetyBlocked => "safetyBlocked",
        api::ProviderErrorClass::ContentRefused => "contentRefused",
        api::ProviderErrorClass::NonRetryable => "nonRetryable",
    };
    serde_json::json!({ PROVIDER_ERROR_CLASS_FIELD: label })
}

#[must_use]
pub fn provider_error_class_from_completion(
    completion: &AgentCompletion,
) -> Option<api::ProviderErrorClass> {
    let value = completion
        .structured
        .as_ref()?
        .get(PROVIDER_ERROR_CLASS_FIELD)?
        .as_str()?;
    match value {
        "rateLimit" => Some(api::ProviderErrorClass::account_rate_limit(None)),
        "providerOverloaded" => Some(api::ProviderErrorClass::provider_overloaded(None)),
        "transient" => Some(api::ProviderErrorClass::Transient),
        "authExpired" => Some(api::ProviderErrorClass::AuthExpired),
        "contextOverflow" => Some(api::ProviderErrorClass::ContextOverflow),
        "invalidToolProtocol" => Some(api::ProviderErrorClass::InvalidToolProtocol),
        "invalidToolSchema" => Some(api::ProviderErrorClass::InvalidToolSchema),
        "safetyBlocked" => Some(api::ProviderErrorClass::SafetyBlocked),
        "contentRefused" => Some(api::ProviderErrorClass::ContentRefused),
        "nonRetryable" => Some(api::ProviderErrorClass::NonRetryable),
        _ => None,
    }
}

/// The sub-agent → parent wire label must round-trip the capacity **scope**.
///
/// The parent reacts to these two capacity classes in opposite ways (ride the
/// window out vs escape to a lighter model), so collapsing both onto `"rateLimit"`
/// would silently downgrade every sub-agent overload into an account throttle at
/// the process boundary.
#[cfg(test)]
mod provider_error_class_wire_tests {
    use super::{
        provider_error_class_from_completion, provider_error_class_metadata, AgentCompletion,
        HelperRun,
    };

    fn completion_with(structured: serde_json::Value) -> AgentCompletion {
        AgentCompletion {
            agent_id: "agent-1".to_string(),
            name: "probe".to_string(),
            status: "failed".to_string(),
            result: None,
            structured: Some(structured),
            error: None,
            run: HelperRun::default(),
        }
    }

    #[test]
    fn capacity_scope_round_trips_through_the_completion_metadata() {
        for class in [
            api::ProviderErrorClass::account_rate_limit(None),
            api::ProviderErrorClass::provider_overloaded(None),
            api::ProviderErrorClass::Transient,
            api::ProviderErrorClass::NonRetryable,
        ] {
            let encoded = provider_error_class_metadata(class);
            let decoded = provider_error_class_from_completion(&completion_with(encoded))
                .expect("every emitted label must decode");
            assert_eq!(decoded, class, "round-trip lost information for {class:?}");
        }
    }

    /// Forward compatibility in the safe direction: a peer built before the split
    /// emits the bare `"rateLimit"`, which decodes as the account scope. Guessing
    /// "overload" there would swap models on a window they all share.
    #[test]
    fn legacy_rate_limit_label_decodes_as_the_account_scope() {
        let decoded = provider_error_class_from_completion(&completion_with(
            serde_json::json!({ "providerErrorClass": "rateLimit" }),
        ))
        .expect("legacy label still decodes");
        assert_eq!(decoded, api::ProviderErrorClass::account_rate_limit(None));
    }
}

// `Mutex<Option<..>>` rather than `OnceLock`: registration replaces the sender
// (exactly one interactive loop per process registers it), and tests need to
// install a dead receiver and then restore the channel-less default without
// poisoning sibling tests.
static AGENT_COMPLETION_TX: Mutex<Option<tokio_mpsc::UnboundedSender<AgentCompletion>>> =
    Mutex::new(None);

pub fn register_agent_completion_channel() -> tokio_mpsc::UnboundedReceiver<AgentCompletion> {
    let (tx, rx) = tokio_mpsc::unbounded_channel();
    *AGENT_COMPLETION_TX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);
    rx
}

fn agent_completion_sender() -> Option<tokio_mpsc::UnboundedSender<AgentCompletion>> {
    AGENT_COMPLETION_TX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[cfg(test)]
pub(crate) fn clear_agent_completion_channel_for_tests() {
    *AGENT_COMPLETION_TX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// Process-global map of agent ids the model launched in **background** mode
/// (`AgentInput::background`). The `Agent` tool detaches these and returns at
/// spawn time, so the foreground REPL needs a side channel to tell a
/// background agent's completion (which it must re-inject into the conversation)
/// apart from a synchronous agent's completion (already returned inline) and
/// internal fan-out plumbing. Background bash tasks additionally record their
/// launch session so a process-global completion cannot be consumed by the
/// wrong session.
#[derive(Clone, Debug)]
enum BackgroundCompletionSource {
    Agent,
    Task(Option<String>),
}

pub(crate) enum BackgroundTaskSession {
    NotTask,
    Unstamped,
    Session(String),
}

static BACKGROUND_AGENT_IDS: OnceLock<Mutex<HashMap<String, BackgroundCompletionSource>>> =
    OnceLock::new();

fn background_agent_ids() -> &'static Mutex<HashMap<String, BackgroundCompletionSource>> {
    BACKGROUND_AGENT_IDS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record that `agent_id` was launched in background mode. Called by the `Agent`
/// tool's background branch at spawn time.
pub fn mark_background_agent(agent_id: String) {
    background_agent_ids()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(agent_id, BackgroundCompletionSource::Agent);
}

fn mark_background_task(task_id: String, session_id: Option<String>) {
    background_agent_ids()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(task_id, BackgroundCompletionSource::Task(session_id));
}

/// Whether a marked completion is safe to consume in `active_session_id`.
/// Ordinary background agents retain their manifest-based routing; stamped
/// background tasks require an exact session match, and unstamped tasks fail
/// closed.
#[must_use]
pub fn background_completion_matches_session(
    agent_id: &str,
    active_session_id: &str,
) -> bool {
    match background_agent_ids()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(agent_id)
    {
        Some(BackgroundCompletionSource::Task(Some(session_id))) => {
            session_id == active_session_id
        }
        Some(BackgroundCompletionSource::Task(None)) => false,
        Some(BackgroundCompletionSource::Agent) | None => true,
    }
}

pub(crate) fn background_task_session_id(agent_id: &str) -> BackgroundTaskSession {
    match background_agent_ids()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(agent_id)
    {
        Some(BackgroundCompletionSource::Task(Some(session_id))) => {
            BackgroundTaskSession::Session(session_id.clone())
        }
        Some(BackgroundCompletionSource::Task(None)) => BackgroundTaskSession::Unstamped,
        Some(BackgroundCompletionSource::Agent) | None => BackgroundTaskSession::NotTask,
    }
}

/// Whether `agent_id` was launched in background mode (its completion should be
/// re-injected into the conversation rather than merely displayed).
#[must_use]
pub fn is_background_agent(agent_id: &str) -> bool {
    background_agent_ids()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_key(agent_id)
}

/// Forget a background agent id once its completion has been consumed, so the
/// set never grows without bound across a long session.
pub fn clear_background_agent(agent_id: &str) {
    background_agent_ids()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(agent_id);
}

/// Snapshot of the currently-marked background agent ids. Used by hosts
/// without an idle re-injection pump (serve) to sweep for completions at the
/// next turn boundary. Cheap — the set only holds in-flight detached agents.
#[must_use]
pub fn background_agent_ids_snapshot() -> Vec<String> {
    background_agent_ids()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .keys()
        .cloned()
        .collect()
}

/// `status` marker for the W9-3 starvation notice. Shared with the TUI's
/// display boundary so the notice renders as a one-shot warning instead of
/// being mistaken for a terminal rate-limit *failure* (the agent keeps
/// retrying after sending it).
pub const AGENT_STARVED_STATUS: &str = "starved";

/// Build the W9-3 starvation notice. Pure so the shape is testable: the
/// `#starved` id suffix keeps it from ever matching an awaited agent id, and
/// the message rides in `error` (the field the TUI renders for non-completed
/// statuses).
pub(super) fn starvation_notice(agent_id: &str, name: &str, message: String) -> AgentCompletion {
    AgentCompletion {
        agent_id: format!("{agent_id}#starved"),
        name: name.to_string(),
        status: AGENT_STARVED_STATUS.to_string(),
        result: None,
        structured: None,
        error: Some(message),
        run: HelperRun::default(),
    }
}

/// W9-3: deliver a starvation notice to the parent transcript. Channel-only
/// by design — the polling store stays clean (this is not a terminal
/// completion; the agent keeps retrying), so the workflow engine's
/// store-based waits and the `AgentOutput` tool never see it.
pub(super) fn notify_agent_starvation(notice: AgentCompletion) {
    if let Some(tx) = agent_completion_sender() {
        let _ = tx.send(notice);
    }
}

/// `status` marker for a MID-RUN agent→main message (the upstream edge a
/// sub-agent uses to push a finding/question to the parent conversation while
/// it is still working).
///
/// It rides the SAME transport as completions (`AGENT_COMPLETION_TX`) so
/// ordering between "the agent said something" and "the agent finished" is
/// preserved by construction. The transport's payload is a struct, not an
/// enum, and every consumer already branches on `status`; the established
/// extension idiom here is therefore a reserved status plus a `#`-suffixed id
/// (see [`AGENT_STARVED_STATUS`]), which also guarantees the notice can never
/// be mistaken for — or collide with — an awaited terminal completion.
pub const AGENT_MESSAGE_STATUS: &str = "message";

const AGENT_MESSAGE_ID_SUFFIX: &str = "#message";

/// Build the mid-run agent→main message notice. Pure so its shape is testable:
/// the body rides in `result` (the field the host reads for model-facing text),
/// and the id suffix keeps it off every awaited-id path.
pub(super) fn agent_message_notice(agent_id: &str, name: &str, text: String) -> AgentCompletion {
    AgentCompletion {
        agent_id: format!("{agent_id}{AGENT_MESSAGE_ID_SUFFIX}"),
        name: name.to_string(),
        status: AGENT_MESSAGE_STATUS.to_string(),
        result: Some(text),
        structured: None,
        error: None,
        run: HelperRun::default(),
    }
}

/// The sending agent's REAL id for a notice built by `agent_message_notice`
/// — what the host puts in the `SendMessage(to: …)` reply pointer. Idempotent
/// on an unsuffixed id.
#[must_use]
pub fn agent_message_source_id(agent_id: &str) -> &str {
    agent_id
        .strip_suffix(AGENT_MESSAGE_ID_SUFFIX)
        .unwrap_or(agent_id)
}

/// Deliver a mid-run agent→main message to the parent host. Channel-only for
/// the same reason as [`notify_agent_starvation`]: the polling store holds
/// TERMINAL completions, and a mid-run message is not one — publishing it there
/// would satisfy a workflow barrier waiting on this agent.
///
/// Returns `false` when this process has no interactive consumer registered
/// (headless / serve): the caller reports that honestly to the sub-agent rather
/// than claiming a delivery that nobody will ever read.
pub(super) fn notify_agent_message(notice: AgentCompletion) -> bool {
    match agent_completion_sender() {
        Some(tx) => tx.send(notice).is_ok(),
        None => false,
    }
}

// ---- Completion store (single-responsibility wait/poll primitive) -----------
//
// 모델이 `GetAgentCompletion(agent_ids, wait_ms)` 도구로 sub-agent 완료를
// **동기 sync 함수** 안에서 회수하기 위한 store. 채널과 분리한 이유:
//
//   * 채널 (`AGENT_COMPLETION_TX`) = TUI display 용 — receiver 가 한 곳
//     (`tui_loop`) 에서만 소비.
//   * 스토어 (`CompletionStore`) = 도구 polling 용 — 여러 caller 가
//     같은 `agent_id` 를 시간차로 조회 가능, 최근 full result 를 보관.
//
// `std::sync::Condvar` 기반: tokio 의존 없이 sync 도구 함수에서 직접
// `wait_timeout` 가능. `notify_all` 후 단일 lock 안에서 condition 재검사
// → spurious wake / lost-wakeup 안전.

struct StoredCompletion {
    completion: AgentCompletion,
    recorded_at: Instant,
    sequence: u64,
}

#[derive(Default)]
struct CompletionState {
    entries: HashMap<String, StoredCompletion>,
    next_sequence: u64,
}

struct CompletionStore {
    state: Mutex<CompletionState>,
    cond: Condvar,
}

/// Keep whichever half of a re-published run actually measured something.
///
/// Two publishers can reach the same agent id — a watchdog's `stopped` and the
/// worker's own terminal payload — and each may know a different part of the
/// cost. Taking the larger count/total and the first known clock keeps the
/// richer of the two without ever inventing a number neither reported.
fn merge_run(existing: HelperRun, incoming: HelperRun) -> HelperRun {
    HelperRun {
        tool_calls: existing.tool_calls.max(incoming.tool_calls),
        output_tokens: existing.output_tokens.max(incoming.output_tokens),
        elapsed: existing.elapsed.or(incoming.elapsed),
    }
}

impl CompletionStore {
    fn new() -> Self {
        Self {
            state: Mutex::new(CompletionState::default()),
            cond: Condvar::new(),
        }
    }

    fn remove(&self, agent_id: &str) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.entries.remove(agent_id).is_some() {
            self.cond.notify_all();
        }
    }

    fn contains(&self, agent_id: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .contains_key(agent_id)
    }

    fn publish(&self, completion: AgentCompletion) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        if let Some(stored) = state.entries.get_mut(&completion.agent_id) {
            // A watchdog may have published `stopped` while a blocked worker
            // was unwinding. Preserve that terminal ownership, but retain any
            // richer worker payload for polling and later orchestration.
            let existing = &mut stored.completion;
            if completion.result.is_some() {
                existing.result = completion.result;
            }
            if completion.structured.is_some() {
                existing.structured = completion.structured;
            }
            if existing.error.is_none() && completion.error.is_some() {
                existing.error = completion.error;
            }
            existing.run = merge_run(existing.run, completion.run);
            stored.recorded_at = now;
            self.cond.notify_all();
            return false;
        }
        state.next_sequence = state.next_sequence.saturating_add(1);
        let sequence = state.next_sequence;
        state.entries.insert(
            completion.agent_id.clone(),
            StoredCompletion {
                completion,
                recorded_at: now,
                sequence,
            },
        );
        Self::prune_expired_locked(&mut state, now);
        Self::prune_to_max_entries_locked(&mut state);
        self.cond.notify_all();
        true
    }

    #[cfg(test)]
    fn record(&self, completion: AgentCompletion) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        state.next_sequence = state.next_sequence.saturating_add(1);
        let sequence = state.next_sequence;
        state.entries.insert(
            completion.agent_id.clone(),
            StoredCompletion {
                completion,
                recorded_at: now,
                sequence,
            },
        );
        Self::prune_expired_locked(&mut state, now);
        Self::prune_to_max_entries_locked(&mut state);
        self.cond.notify_all();
    }

    /// `ids` 모두 완료될 때까지 (또는 `timeout` 만료까지) 대기.
    ///
    /// 반환 벡터는 입력 `ids` 와 같은 순서. 시간 초과로 도착 못 한 항목은
    /// `status = "still_running"` placeholder.
    ///
    /// `cancel` 이 `Some` 이고 도중에 `true` 가 되면 즉시 (미도착 항목은
    /// `still_running` 으로) 반환한다. 폴링은 `cancel` 이 있을 때만 짧은
    /// 슬라이스로 일어나므로, cancel 없는 경로는 기존처럼 condvar 에 블록한다.
    fn wait_for_all(
        &self,
        ids: &[String],
        timeout: Option<Duration>,
        cancel: Option<&AtomicBool>,
        on_done: Option<&mut dyn FnMut(&AgentCompletion)>,
    ) -> Vec<AgentCompletion> {
        self.wait(ids, timeout, cancel, on_done, false)
    }

    fn wait(
        &self,
        ids: &[String],
        timeout: Option<Duration>,
        cancel: Option<&AtomicBool>,
        mut on_done: Option<&mut dyn FnMut(&AgentCompletion)>,
        return_on_completion: bool,
    ) -> Vec<AgentCompletion> {
        let deadline = timeout.map(|timeout| {
            let now = Instant::now();
            // A finite-but-overflowing timeout must NOT collapse into the
            // `None` sentinel (which means "wait forever" below) — saturate to
            // a far-future bounded deadline instead. `Instant` has no
            // `saturating_add`; this is unreachable for realistic durations.
            now.checked_add(timeout)
                .unwrap_or_else(|| now + Duration::from_secs(86_400 * 365))
        });
        let is_cancelled = || cancel.is_some_and(|flag| flag.load(Ordering::Relaxed));
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut collected: HashMap<String, AgentCompletion> = HashMap::new();
        Self::prune_expired_locked(&mut guard, Instant::now());
        loop {
            if is_cancelled() {
                break;
            }
            // Completion-order observer: each id is reported the moment it
            // first lands in `collected`. Callbacks run *outside* the lock so a
            // slow consumer (progress-file write) cannot stall `record()`.
            let mut newly: Vec<AgentCompletion> = Vec::new();
            for id in ids {
                if collected.contains_key(id) {
                    continue;
                }
                if let Some(entry) = guard.entries.get(id) {
                    if return_on_completion && !super::agent_output_status_is_terminal(&entry.completion.status) {
                        continue;
                    }
                    collected.insert(id.clone(), entry.completion.clone());
                    if on_done.is_some() {
                        newly.push(entry.completion.clone());
                    }
                }
            }
            if !newly.is_empty() {
                if let Some(observer) = on_done.as_mut() {
                    drop(guard);
                    for completion in &newly {
                        observer(completion);
                    }
                    guard = self
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
            let all_present = ids.iter().all(|id| collected.contains_key(id));
            if all_present || (return_on_completion && !collected.is_empty()) {
                break;
            }
            match (deadline, cancel.is_some()) {
                (Some(deadline), observe_cancel) => {
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                        break;
                    };
                    // Cap each wait to a poll slice only when a cancel flag is
                    // present, so cancellation is seen without busy-polling the
                    // non-cancellable path.
                    let wait = if observe_cancel {
                        remaining.min(CANCEL_POLL_SLICE)
                    } else {
                        remaining
                    };
                    let (next_guard, _wait_result) = self
                        .cond
                        .wait_timeout(guard, wait)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard = next_guard;
                }
                (None, true) => {
                    // Unbounded wait that still observes cancel: block in slices.
                    let (next_guard, _wait_result) = self
                        .cond
                        .wait_timeout(guard, CANCEL_POLL_SLICE)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard = next_guard;
                }
                (None, false) => {
                    guard = self
                        .cond
                        .wait(guard)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
            Self::prune_expired_locked(&mut guard, Instant::now());
        }
        ids.iter()
            .map(|id| {
                collected
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| AgentCompletion {
                        agent_id: id.clone(),
                        name: String::new(),
                        status: String::from("still_running"),
                        result: None,
                        structured: None,
                        error: None,
                        run: HelperRun::default(),
                    })
            })
            .collect()
    }

    fn prune_expired_locked(state: &mut CompletionState, now: Instant) {
        state.entries.retain(|_, entry| {
            now.saturating_duration_since(entry.recorded_at) <= COMPLETION_STORE_TTL
        });
    }

    fn prune_to_max_entries_locked(state: &mut CompletionState) {
        while state.entries.len() > MAX_COMPLETION_STORE_ENTRIES {
            let Some(oldest_id) = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.sequence)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            state.entries.remove(&oldest_id);
        }
    }
}

static AGENT_COMPLETION_STORE: OnceLock<CompletionStore> = OnceLock::new();

#[cfg(test)]
static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn lock_completion_store_for_tests() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn completion_store() -> &'static CompletionStore {
    AGENT_COMPLETION_STORE.get_or_init(CompletionStore::new)
}

pub(super) fn agent_completion_is_published(agent_id: &str) -> bool {
    completion_store().contains(agent_id)
}

/// 모델 polling 도구(`GetAgentCompletion`)가 호출하는 동기 wait.
#[must_use]
pub fn wait_for_agent_completions(ids: &[String], timeout: Duration) -> Vec<AgentCompletion> {
    completion_store().wait_for_all(ids, Some(timeout), None, None)
}

/// Like [`wait_for_agent_completions`] but observes a cooperative cancel flag:
/// a foreground Ctrl+C during the collection window returns promptly with
/// `still_running` placeholders for the agents that had not completed, instead
/// of blocking a worker thread for the full window (WI-G).
pub fn wait_for_agent_completions_cancellable(
    ids: &[String],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Vec<AgentCompletion> {
    completion_store().wait_for_all(ids, Some(timeout), cancel, None)
}

/// [`wait_for_agent_completions_cancellable`] plus a completion-order observer:
/// `on_done` fires once per agent the moment its terminal completion lands —
/// while the barrier is still collecting — so orchestration layers (the
/// workflow engine's per-agent progress events) can move live progress instead
/// of staying frozen until the whole batch resolves.
#[cfg(test)]
fn wait_for_agent_completions_observed(
    ids: &[String],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    on_done: &mut dyn FnMut(&AgentCompletion),
) -> Vec<AgentCompletion> {
    completion_store().wait_for_all(ids, Some(timeout), cancel, Some(on_done))
}

/// Observe the next available completions without cancelling unfinished agents.
pub(crate) fn wait_for_next_agent_completions(
    ids: &[String],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    on_done: &mut dyn FnMut(&AgentCompletion),
) -> Vec<AgentCompletion> {
    completion_store().wait(ids, Some(timeout), cancel, Some(on_done), true)
}

/// Internal orchestration wait: block until every spawned agent has produced its
/// terminal completion. Unlike the polling tool path, this never fabricates a
/// `still_running` result just because a wall-clock collection window elapsed.
// Retained seam: the only entry point to `wait_for_all`'s unbounded-wait
// (timeout = None) contract, which the store tests pin down. Orchestration
// callers currently prefer the observed/timeout variants.
#[allow(dead_code)]
pub fn wait_for_agent_completions_until_done(ids: &[String]) -> Vec<AgentCompletion> {
    completion_store().wait_for_all(ids, None, None, None)
}

/// Forget a terminal completion before reusing its agent id for a resumed run.
/// This is one mutex-protected removal, so a subsequent publication creates a
/// fresh store generation and emits a fresh channel event.
pub(super) fn reset_agent_completion(agent_id: &str) {
    completion_store().remove(agent_id);
}

#[cfg(test)]
pub(crate) fn reset_completion_store_for_tests() {
    if let Some(store) = AGENT_COMPLETION_STORE.get() {
        let mut state = store
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.entries.clear();
        state.next_sequence = 0;
    }
}

#[cfg(test)]
pub(crate) fn inject_completion_for_tests(completion: AgentCompletion) {
    completion_store().record(completion);
}

#[cfg(test)]
pub(crate) fn publish_agent_completion_for_tests(completion: AgentCompletion) -> bool {
    notify_agent_completion(completion, None, None)
}

#[cfg(test)]
pub(crate) fn completion_store_len_for_tests() -> usize {
    completion_store()
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entries
        .len()
}

/// Publish exactly one terminal completion notification per agent. A later
/// worker result enriches the store for polling but deliberately does not send
/// another channel event. `manifest_generation` is the publishing run's
/// durable generation when known: the delivery marker refuses to stamp a
/// mismatched (e.g. concurrently resumed) generation.
pub(super) fn notify_agent_completion(
    completion: AgentCompletion,
    manifest_generation: Option<u64>,
    manifest_file: Option<std::path::PathBuf>,
) -> bool {
    let published = completion_store().publish(completion.clone());
    if published {
        let agent_id = completion.agent_id.clone();
        // Delivery edge per host: an interactive host's re-injection is driven
        // by this channel (the TUI loop consumes it and re-injects the result
        // as a fresh parent turn), so there the wakeup send must SUCCEED to
        // count as delivered. Channel-less hosts (serve, headless) sweep the
        // polling store directly, so for them store publication IS delivery.
        let delivered = match agent_completion_sender() {
            Some(tx) => {
                let compact = AgentCompletion {
                    result: None,
                    ..completion
                };
                tx.send(compact).is_ok()
            }
            None => true,
        };
        // Post-mortem delivery marker, stamped LAST so it follows every
        // publication side effect — and only when this host's delivery edge
        // was actually crossed. Safe under a held per-agent manifest lock
        // (same-thread reentry); a non-agent id (background task) is a no-op.
        if let (true, Some(manifest_file)) = (delivered, manifest_file) {
            super::manifest::stamp_completion_published(
                &agent_id,
                &manifest_file,
                manifest_generation,
            );
        }
    }
    published
}

/// Funnel a finished background bash task into the SAME push path a background
/// agent uses: mark its id so the reinject gate accepts it, then record + signal
/// completion. Called from the session layer because the `runtime` crate (where
/// the task watcher lives) cannot depend on `tools`. `status` is "completed" or
/// "failed"; `output` becomes the re-injected result.
pub fn notify_background_task_completion(
    task_id: String,
    status: &str,
    output: Option<String>,
    session_id: Option<String>,
) {
    notify_session_fact(background_task_completion(task_id, status, output), session_id);
}

/// Deliver a host-authored terminal fact that is no agent's own completion —
/// a finished background process, the verify-by-default loop's stop receipt —
/// to the session that owns it, by the one push path both use: stamp the id
/// with that session (a foreign or unstamped session fails closed), then
/// publish, so the interactive pump re-injects it and channel-less hosts find
/// it in their sweep. The id must be one no agent owns.
pub(super) fn notify_session_fact(fact: AgentCompletion, session_id: Option<String>) {
    mark_background_task(fact.agent_id.clone(), session_id);
    notify_agent_completion(fact, None, None);
}

/// Build the same background-process fact for interactive and headless hosts.
#[must_use]
pub fn background_task_completion(task_id: String, status: &str, output: Option<String>) -> AgentCompletion {
    let log = output.unwrap_or_default();
    let summary = runtime::background_log::completion_summary(&task_id, &log);
    AgentCompletion {
        agent_id: task_id,
        name: runtime::background_log::BACKGROUND_BASH.to_string(),
        status: status.to_string(),
        result: Some(format!("{log}\n{summary}\n")),
        structured: None,
        error: None,
        run: HelperRun::default(),
    }
}


#[cfg(test)]
mod completion_store_tests {
    use super::*;
    use std::thread;

    fn completion(id: &str, status: &str, result: Option<&str>) -> AgentCompletion {
        AgentCompletion {
            agent_id: id.to_string(),
            name: format!("agent-{id}"),
            status: status.to_string(),
            result: result.map(str::to_string),
            structured: None,
            error: None,
            run: HelperRun::default(),
        }
    }

    #[test]
    fn next_completion_does_not_cancel_or_consume_pending_work() {
        let store = CompletionStore::new();
        store.record(completion("a", "completed", Some("first")));
        let ids = vec!["b".to_owned(), "a".to_owned()];
        let mut observed = Vec::new();
        let first = store.wait(&ids, Some(Duration::from_secs(5)), None, Some(&mut |done| {
            observed.push(done.agent_id.clone());
        }), true);
        assert_eq!(first[0].status, "still_running");
        assert_eq!(first[1].result.as_deref(), Some("first"));
        assert_eq!(observed, vec!["a"]);
        store.record(completion("b", "completed", Some("later")));
        let all = store.wait_for_all(&ids, Some(Duration::ZERO), None, None);
        assert_eq!(all[0].result.as_deref(), Some("later"));
        assert_eq!(all[1].result.as_deref(), Some("first"));
    }

    #[test]
    fn next_completion_waits_past_nonterminal_store_entries() {
        let store = CompletionStore::new();
        store.record(completion("a", "still_running", None));
        let ids = vec!["a".to_owned()];
        let started = Instant::now();
        let timeout = Duration::from_millis(10);
        let mut seen = Vec::new();
        let result = store.wait(&ids, Some(timeout), None, Some(&mut |done| seen.push(done.agent_id.clone())), true);
        assert!(started.elapsed() >= timeout);
        assert!(seen.is_empty());
        assert_eq!(result[0].status, "still_running");
        store.record(completion("a", "completed", Some("done")));
        assert_eq!(store.wait(&ids, Some(Duration::ZERO), None, None, true)[0].status, "completed");
    }

    #[test]
    fn next_completion_timeout_and_cancel_preserve_store() {
        let store = CompletionStore::new();
        let ids = vec!["a".to_owned()];
        let cancel = AtomicBool::new(true);
        let absent = store.wait(&ids, Some(Duration::ZERO), None, None, true);
        assert_eq!(absent[0].status, "still_running");
        store.record(completion("a", "completed", Some("done")));
        let cancelled = store.wait(&ids, None, Some(&cancel), None, true);
        assert_eq!(cancelled[0].status, "still_running");
        cancel.store(false, Ordering::Relaxed);
        assert_eq!(store.wait(&ids, None, Some(&cancel), None, true)[0].status, "completed");
    }

    #[test]
    fn wait_returns_immediately_when_all_present() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        inject_completion_for_tests(completion("a", "completed", Some("hello")));
        let start = Instant::now();
        let result = wait_for_agent_completions(&["a".to_string()], Duration::from_secs(5));
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "should not block"
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, "completed");
        assert_eq!(result[0].result.as_deref(), Some("hello"));
    }

    #[test]
    fn background_marker_roundtrips_and_full_result_reads_back() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        let id = "bg-marker-1";
        // The marker set the foreground REPL uses to tell a backgrounded agent's
        // completion (re-inject) apart from a synchronous one (already returned).
        assert!(!is_background_agent(id));
        mark_background_agent(id.to_string());
        assert!(is_background_agent(id));
        // The broadcast channel strips `result`, so re-injection reads the full
        // answer back from the store by id with a zero timeout — this is exactly
        // that read-back, and it must return the complete result.
        inject_completion_for_tests(completion(id, "completed", Some("the answer")));
        let read = wait_for_agent_completions(&[id.to_string()], Duration::ZERO);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].result.as_deref(), Some("the answer"));
        // Cleared after consumption so the id set never grows without bound.
        clear_background_agent(id);
        assert!(!is_background_agent(id));
    }

    #[test]
    fn wait_returns_still_running_after_timeout() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        let start = Instant::now();
        let result =
            wait_for_agent_completions(&["missing".to_string()], Duration::from_millis(80));
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(70),
            "should wait approximately the timeout, got {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(300),
            "should not over-wait, got {elapsed:?}"
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, "still_running");
    }

    #[test]
    fn wait_wakes_on_late_completion() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        let id = "late".to_string();
        thread::spawn({
            let id = id.clone();
            move || {
                thread::sleep(Duration::from_millis(50));
                inject_completion_for_tests(completion(&id, "completed", Some("done")));
            }
        });
        let start = Instant::now();
        let result = wait_for_agent_completions(std::slice::from_ref(&id), Duration::from_secs(5));
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "should wake on notify_all, not poll, got {elapsed:?}"
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, "completed");
    }

    #[test]
    fn unbounded_wait_returns_the_actual_late_completion() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        let id = "eventual".to_string();
        thread::spawn({
            let id = id.clone();
            move || {
                thread::sleep(Duration::from_millis(50));
                inject_completion_for_tests(completion(&id, "completed", Some("actual result")));
            }
        });
        let start = Instant::now();
        let result = wait_for_agent_completions_until_done(std::slice::from_ref(&id));
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "should wake from condvar when the actual result arrives"
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, "completed");
        assert_eq!(result[0].result.as_deref(), Some("actual result"));
    }

    #[test]
    fn wait_keeps_collected_results_even_if_store_later_evicts_them() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        let first = "first".to_string();
        let second = "second".to_string();
        // The property under test is "a result the waiter already collected
        // survives later store eviction" — which needs collect-BEFORE-evict
        // ordering. The old shape raced the scheduler for it (the eviction
        // flood reacquires the store lock in a tight loop, starving the
        // waiter under parallel-test load → intermittent `still_running`).
        // Enforce the ordering by construction instead: the injector floods
        // only after the waiter's completion observer reports `first`.
        let first_collected = std::sync::Arc::new(AtomicBool::new(false));
        let injector = thread::spawn({
            let first = first.clone();
            let second = second.clone();
            let first_collected = std::sync::Arc::clone(&first_collected);
            move || {
                inject_completion_for_tests(completion(&first, "completed", Some("first result")));
                while !first_collected.load(Ordering::Acquire) {
                    thread::yield_now();
                }
                for index in 0..(MAX_COMPLETION_STORE_ENTRIES + 10) {
                    inject_completion_for_tests(completion(
                        &format!("noise-{index}"),
                        "completed",
                        Some("noise"),
                    ));
                }
                inject_completion_for_tests(completion(
                    &second,
                    "completed",
                    Some("second result"),
                ));
            }
        });

        let mut on_done = |done: &AgentCompletion| {
            if done.agent_id == "first" {
                first_collected.store(true, Ordering::Release);
            }
        };
        let result = wait_for_agent_completions_observed(
            &[first, second],
            Duration::from_secs(5),
            None,
            &mut on_done,
        );
        injector.join().expect("injector thread");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].status, "completed");
        assert_eq!(result[0].result.as_deref(), Some("first result"));
        assert_eq!(result[1].status, "completed");
        assert_eq!(result[1].result.as_deref(), Some("second result"));
    }

    #[test]
    fn wait_for_all_observes_cancel() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        // No completion for "pending"; with a pre-set cancel flag the wait must
        // return promptly instead of blocking for the full (long) timeout.
        let cancel = AtomicBool::new(true);
        let start = Instant::now();
        let result = wait_for_agent_completions_cancellable(
            &["pending".to_string()],
            Duration::from_secs(30),
            Some(&cancel),
        );
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "cancel must short-circuit the collection window, got {:?}",
            start.elapsed()
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].status, "still_running");
    }

    #[test]
    fn wait_for_all_wakes_when_cancel_flips_late() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        thread::spawn({
            let cancel = std::sync::Arc::clone(&cancel);
            move || {
                thread::sleep(Duration::from_millis(80));
                cancel.store(true, Ordering::Relaxed);
            }
        });
        let start = Instant::now();
        let result = wait_for_agent_completions_cancellable(
            &["pending".to_string()],
            Duration::from_secs(30),
            Some(&cancel),
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed >= Duration::from_millis(70) && elapsed < Duration::from_secs(2),
            "should wake shortly after the late cancel, got {elapsed:?}"
        );
        assert_eq!(result[0].status, "still_running");
    }

    /// 옵저버는 입력 `ids` 순서가 아니라 **완료가 도착한 순서**대로,
    /// 배리어가 끝나기 전에 한 번씩 발화한다.
    #[test]
    fn observed_wait_fires_in_completion_order_mid_barrier() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        let first = "obs-late".to_string();
        let second = "obs-early".to_string();
        thread::spawn({
            let first = first.clone();
            let second = second.clone();
            move || {
                // `ids` 순서(첫번째=obs-late)와 반대로 도착시킨다.
                thread::sleep(Duration::from_millis(30));
                inject_completion_for_tests(completion(&second, "completed", Some("early")));
                thread::sleep(Duration::from_millis(40));
                inject_completion_for_tests(completion(&first, "completed", Some("late")));
            }
        });
        let mut seen: Vec<String> = Vec::new();
        let result = wait_for_agent_completions_observed(
            &[first.clone(), second.clone()],
            Duration::from_secs(5),
            None,
            &mut |completion| seen.push(completion.agent_id.clone()),
        );
        assert_eq!(
            seen,
            vec![second.clone(), first.clone()],
            "observer must fire in completion order, not ids order"
        );
        // 반환 벡터는 기존 계약 그대로 ids 순서.
        assert_eq!(result[0].agent_id, first);
        assert_eq!(result[1].agent_id, second);
        assert!(result.iter().all(|c| c.status == "completed"));
    }

    #[test]
    fn wait_collects_mixed_states() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();
        inject_completion_for_tests(completion("done", "completed", Some("ok")));
        let result = wait_for_agent_completions(
            &["done".to_string(), "pending".to_string()],
            Duration::from_millis(30),
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].agent_id, "done");
        assert_eq!(result[0].status, "completed");
        assert_eq!(result[1].agent_id, "pending");
        assert_eq!(result[1].status, "still_running");
    }

    #[test]
    fn store_evicts_oldest_completion_above_entry_cap() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reset_completion_store_for_tests();

        for index in 0..(MAX_COMPLETION_STORE_ENTRIES + 10) {
            inject_completion_for_tests(completion(
                &format!("agent-{index}"),
                "completed",
                Some("done"),
            ));
        }

        assert_eq!(
            completion_store_len_for_tests(),
            MAX_COMPLETION_STORE_ENTRIES
        );

        let evicted =
            wait_for_agent_completions(&["agent-0".to_string()], Duration::from_millis(0));
        assert_eq!(evicted[0].status, "still_running");

        let retained = wait_for_agent_completions(
            &[format!("agent-{}", MAX_COMPLETION_STORE_ENTRIES + 9)],
            Duration::from_millis(0),
        );
        assert_eq!(retained[0].status, "completed");
        assert_eq!(retained[0].result.as_deref(), Some("done"));
    }

    #[test]
    fn late_completion_enriches_store_without_republishing_terminal_status() {
        let store = CompletionStore::new();
        assert!(store.publish(completion("race", "stopped", None)));

        let mut rich = completion("race", "completed", Some("partial result"));
        rich.structured = Some(serde_json::json!({"verdict": "pass"}));
        rich.run.output_tokens = 42;
        assert!(
            !store.publish(rich),
            "the worker may enrich the stored completion but cannot emit a second terminal event"
        );

        let state = store
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let completion = &state.entries["race"].completion;
        assert_eq!(completion.status, "stopped");
        assert_eq!(completion.result.as_deref(), Some("partial result"));
        assert_eq!(completion.structured, Some(serde_json::json!({"verdict": "pass"})));
        assert_eq!(completion.run.output_tokens, 42);
    }

    /// W9-3: 통지 형태 — `#starved` 접미사로 어떤 대기 중 `agent_id`와도 충돌하지
    /// 않고, 메시지는 TUI가 비종결 상태에 렌더하는 `error` 필드에 실린다.
    #[test]
    fn starvation_notice_shape_never_collides_with_awaited_ids() {
        let notice = super::starvation_notice("agent-12ab", "explorer", "starved 5m".to_string());
        assert_eq!(notice.agent_id, "agent-12ab#starved");
        assert_eq!(notice.status, super::AGENT_STARVED_STATUS);
        assert_eq!(notice.name, "explorer");
        assert_eq!(notice.error.as_deref(), Some("starved 5m"));
        assert!(notice.result.is_none());
        assert_eq!(notice.run.output_tokens, 0);
    }
}
