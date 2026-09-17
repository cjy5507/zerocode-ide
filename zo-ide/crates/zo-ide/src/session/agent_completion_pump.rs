//! Background-agent completion delivery owned by the interactive session host.
//!
//! The contract tests pin subscription, mid-turn, idle, exactly-once, and
//! shutdown boundaries independently of the TUI event loop.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use runtime::message_stream::{AgentResultStatus, BlockIdGen, RenderBlock};
use runtime::{AgentNotification, AgentNotificationInbox, AgentNotificationKind};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tools::AgentCompletion;

/// Keep a useful head and tail without allowing a full worker transcript to
/// consume the parent model's remaining context.
const MAX_REINJECTED_RESULT_CHARS: usize = 16_000;

/// One completion that must run as a user-role follow-up turn while rendering
/// as an agent-authored result card.
pub(crate) struct AgentFollowup {
    pub(crate) label: String,
    pub(crate) status: AgentResultStatus,
    /// What the helper's run cost, for the card's `Done (…)` line.
    pub(crate) summary: Option<String>,
    pub(crate) text: String,
}

impl AgentFollowup {
    pub(crate) fn render_block(&self, ids: &BlockIdGen) -> RenderBlock {
        RenderBlock::AgentResult {
            id: ids.next(),
            label: self.label.clone(),
            status: self.status,
            summary: self.summary.clone(),
            body: self.text.clone(),
        }
    }
}

impl From<AgentNotification> for AgentFollowup {
    fn from(notification: AgentNotification) -> Self {
        Self {
            label: notification.label,
            status: notification.status,
            summary: notification.summary,
            text: notification.text,
        }
    }
}

/// Register the process-global producer only for an interactive session.
pub(super) fn register_for_surface(
    headless: bool,
) -> Option<mpsc::UnboundedReceiver<AgentCompletion>> {
    (!headless).then(tools::register_agent_completion_channel)
}

enum DeliveryPhase {
    Idle,
    Turning(AgentNotificationInbox),
}

struct DeliveryRoute {
    session_id: String,
    phase: DeliveryPhase,
}

/// Owns the sole completion consumer task and its idle follow-up queue.
pub(crate) struct AgentCompletionPump {
    route: Arc<Mutex<DeliveryRoute>>,
    followup_tx: mpsc::UnboundedSender<AgentFollowup>,
    followup_rx: mpsc::UnboundedReceiver<AgentFollowup>,
    task: Option<JoinHandle<()>>,
}

impl AgentCompletionPump {
    pub(crate) fn spawn(
        receiver: mpsc::UnboundedReceiver<AgentCompletion>,
        session_id: String,
    ) -> Self {
        let route = Arc::new(Mutex::new(DeliveryRoute {
            session_id,
            phase: DeliveryPhase::Idle,
        }));
        let (followup_tx, followup_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(relay_completions(
            receiver,
            Arc::clone(&route),
            followup_tx.clone(),
        ));
        Self {
            route,
            followup_tx,
            followup_rx,
            task: Some(task),
        }
    }

    /// Route new completions to this turn's runtime inbox. The route lock is
    /// the phase hand-off: a completion either lands here or in the idle queue,
    /// never between them.
    pub(crate) fn begin_turn(&self, inbox: AgentNotificationInbox) {
        self.route
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .phase = DeliveryPhase::Turning(inbox);
    }

    /// Close the live route and move whatever the runtime did not fold at a
    /// tool boundary into the idle queue. Holding the route lock through the
    /// drain excludes a late live push, which is the exactly-once edge.
    pub(crate) fn finish_turn(&self) {
        let mut route = self
            .route
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let DeliveryPhase::Turning(inbox) =
            std::mem::replace(&mut route.phase, DeliveryPhase::Idle)
        else {
            return;
        };
        let leftovers = {
            let mut inbox = inbox
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *inbox)
        };
        for notification in leftovers {
            let _ = self.followup_tx.send(notification.into());
        }
    }

    pub(crate) async fn recv_followup(&mut self) -> Option<AgentFollowup> {
        self.followup_rx.recv().await
    }

    pub(crate) fn try_recv_followup(&mut self) -> Option<AgentFollowup> {
        self.followup_rx.try_recv().ok()
    }

    #[cfg(test)]
    async fn shutdown(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }

    #[cfg(test)]
    fn task_finished(&self) -> bool {
        self.task.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for AgentCompletionPump {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn relay_completions(
    mut receiver: mpsc::UnboundedReceiver<AgentCompletion>,
    route: Arc<Mutex<DeliveryRoute>>,
    followup_tx: mpsc::UnboundedSender<AgentFollowup>,
) {
    while let Some(completion) = receiver.recv().await {
        let route = route
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(notification) = build_background_notification(&completion, &route.session_id)
        else {
            continue;
        };
        if let DeliveryPhase::Turning(inbox) = &route.phase {
            if let Ok(mut inbox) = inbox.lock() {
                inbox.push(notification);
                continue;
            }
        }
        let _ = followup_tx.send(notification.into());
    }
}

fn build_background_notification(
    completion: &AgentCompletion,
    active_session_id: &str,
) -> Option<AgentNotification> {
    if !matches!(completion.status.as_str(), "completed" | "failed" | "stopped")
        || !tools::is_background_agent(&completion.agent_id)
    {
        return None;
    }
    if !tools::background_completion_matches_session(&completion.agent_id, active_session_id) {
        // A process-global producer can outlive the session that launched a
        // background task. Claim its marker so it cannot pollute the next one;
        // the full output remains available through TaskOutput.
        tools::clear_background_agent(&completion.agent_id);
        return None;
    }

    // Claim before reading/building: a duplicate event or a second drain site
    // can no longer pass the gate above.
    tools::clear_background_agent(&completion.agent_id);
    let stored = tools::wait_for_agent_completions(
        std::slice::from_ref(&completion.agent_id),
        Duration::ZERO,
    )
    .into_iter()
    .find(|stored| stored.agent_id == completion.agent_id && stored.status != "still_running");
    let non_blank = |text: Option<String>| {
        text.map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };
    let stored_error = stored.as_ref().and_then(|stored| stored.error.clone());
    let stored_result = stored.and_then(|stored| stored.result);
    let error = non_blank(stored_error).or_else(|| non_blank(completion.error.clone()));
    let body = non_blank(stored_result)
        .or_else(|| non_blank(completion.result.clone()))
        .or_else(|| (completion.status != "completed").then(|| error.clone()).flatten())
        .unwrap_or_else(|| tools::AGENT_NOTIFICATION_EMPTY_RESULT.to_string());
    Some(completion_notification(completion, &body, error.as_deref()))
}

/// A headless host has no idle follow-up loop, but a background process that
/// exits during its turn must still enter the same tool-boundary inbox.
pub(super) fn background_task_notification(task_id: String, status: runtime::task_registry::TaskStatus, output: String) -> Option<AgentNotification> {
    if !matches!(status, runtime::task_registry::TaskStatus::Completed | runtime::task_registry::TaskStatus::Failed) { return None; }
    let completion = tools::background_task_completion(task_id, &status.to_string(), Some(output));
    let body = completion.result.as_deref().unwrap_or_default();
    Some(completion_notification(&completion, body, None))
}

fn completion_notification(completion: &AgentCompletion, body: &str, error: Option<&str>) -> AgentNotification {
    let label = completion.name.trim();
    let label = if label.is_empty() {
        completion.agent_id.as_str()
    } else {
        label
    };
    let header = if label == runtime::background_log::BACKGROUND_BASH {
        runtime::background_log::notification_header(&completion.agent_id)
    } else { tools::background_agent_notification_header(
        &completion.agent_id,
        label,
        &completion.status,
        error,
        completion.run,
    ) };
    AgentNotification {
        label: label.to_string(),
        status: if completion.status == "completed" {
            AgentResultStatus::Completed
        } else {
            AgentResultStatus::Failed
        },
        text: format!(
            "{header}\n\n{}",
            core_types::text::elide_middle(body, MAX_REINJECTED_RESULT_CHARS)
        ),
        kind: AgentNotificationKind::Completion,
        summary: completion.run.summary(),
    }
}

#[cfg(test)]
mod tests {
    use core_types::helper_run::HelperRun;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use runtime::message_stream::{AgentResultStatus, BlockIdGen, RenderBlock};
    use runtime::AgentNotificationKind;
    use tokio::sync::mpsc;
    use tokio::time::timeout;
    use tools::AgentCompletion;

    use super::{register_for_surface, AgentCompletionPump};

    struct BackgroundMark(String);

    impl BackgroundMark {
        fn new(agent_id: &str) -> Self {
            tools::mark_background_agent(agent_id.to_string());
            Self(agent_id.to_string())
        }
    }

    impl Drop for BackgroundMark {
        fn drop(&mut self) {
            tools::clear_background_agent(&self.0);
        }
    }

    fn completion(agent_id: &str, result: &str) -> AgentCompletion {
        AgentCompletion {
            agent_id: agent_id.to_string(),
            name: "runtime-scout".to_string(),
            status: "completed".to_string(),
            result: Some(result.to_string()),
            structured: None,
            error: None,
            run: HelperRun {
                tool_calls: 3,
                output_tokens: 17,
                elapsed: Some(Duration::from_secs(80)),
            },
        }
    }

    async fn wait_for_inbox(
        inbox: &Arc<Mutex<Vec<runtime::AgentNotification>>>,
    ) -> runtime::AgentNotification {
        timeout(Duration::from_millis(250), async {
            loop {
                if let Some(notification) = inbox.lock().expect("inbox").pop() {
                    return notification;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completion pump did not stage the notification")
    }

    #[test]
    fn only_an_interactive_surface_registers_the_global_completion_channel() {
        assert!(
            register_for_surface(false).is_some(),
            "an interactive conversation must subscribe exactly once"
        );
        assert!(
            register_for_surface(true).is_none(),
            "headless execution keeps Agent blocking because no REPL can re-inject"
        );
    }

    #[tokio::test]
    async fn a_background_completion_is_pushed_into_the_live_turn_inbox() {
        let agent_id = "zo-pump-mid-turn";
        let _mark = BackgroundMark::new(agent_id);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut pump = AgentCompletionPump::spawn(rx, "session-a".to_string());
        let inbox = Arc::new(Mutex::new(Vec::new()));
        pump.begin_turn(Arc::clone(&inbox));

        tx.send(completion(agent_id, "the flag lives in config.rs"))
            .expect("completion send");
        let notification = wait_for_inbox(&inbox).await;

        assert_eq!(notification.label, "runtime-scout");
        assert_eq!(notification.status, AgentResultStatus::Completed);
        assert!(notification.text.contains("the flag lives in config.rs"));
        assert_eq!(notification.kind, AgentNotificationKind::Completion);
        // The cost reaches BOTH readers: the model, in the header it folds
        // into its turn, and the person, through the card the followup draws.
        assert_eq!(
            notification.summary.as_deref(),
            Some("3 tool uses · 17 tokens · 1m 20s")
        );
        assert!(
            notification
                .text
                .contains("finished (3 tool uses · 17 tokens · 1m 20s)."),
            "{}",
            notification.text
        );
        assert!(
            timeout(Duration::from_millis(20), pump.recv_followup())
                .await
                .is_err(),
            "a live-turn delivery must not also queue an idle turn"
        );
    }

    #[tokio::test]
    async fn an_unmarked_blocking_completion_is_not_delivered_twice() {
        let background_id = "zo-pump-background-only";
        let _mark = BackgroundMark::new(background_id);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut pump = AgentCompletionPump::spawn(rx, "session-a".to_string());

        tx.send(completion("zo-pump-blocking", "already returned inline"))
            .expect("blocking completion send");
        tx.send(completion(background_id, "deliver me"))
            .expect("background completion send");

        let followup = timeout(Duration::from_millis(250), pump.recv_followup())
            .await
            .expect("background completion did not wake the idle host")
            .expect("pump closed");
        assert!(followup.text.contains("deliver me"));
        assert!(!followup.text.contains("already returned inline"));
        assert!(
            timeout(Duration::from_millis(20), pump.recv_followup())
                .await
                .is_err(),
            "blocking completion was re-injected after its inline tool result"
        );
    }

    #[tokio::test]
    async fn an_idle_completion_becomes_an_agent_result_followup_turn() {
        let agent_id = "zo-pump-idle-card";
        let _mark = BackgroundMark::new(agent_id);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut pump = AgentCompletionPump::spawn(rx, "session-a".to_string());

        tx.send(completion(agent_id, "three call sites"))
            .expect("completion send");
        let followup = timeout(Duration::from_millis(250), pump.recv_followup())
            .await
            .expect("completion did not wake the idle host")
            .expect("pump closed");

        assert!(followup.text.starts_with("[task notification"));
        match followup.render_block(&BlockIdGen::default()) {
            RenderBlock::AgentResult {
                label,
                status,
                body,
                ..
            } => {
                assert_eq!(label, "runtime-scout");
                assert_eq!(status, AgentResultStatus::Completed);
                assert_eq!(body, followup.text);
            }
            other => panic!("idle follow-up rendered as {other:?}, not AgentResult"),
        }
    }

    #[tokio::test]
    async fn a_turn_end_requeues_the_inbox_leftover_exactly_once() {
        let agent_id = "zo-pump-turn-tail";
        let _mark = BackgroundMark::new(agent_id);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut pump = AgentCompletionPump::spawn(rx, "session-a".to_string());
        let inbox = Arc::new(Mutex::new(Vec::new()));
        pump.begin_turn(Arc::clone(&inbox));
        tx.send(completion(agent_id, "arrived after the last tool boundary"))
            .expect("completion send");

        timeout(Duration::from_millis(250), async {
            while inbox.lock().expect("inbox").is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completion was not staged");
        pump.finish_turn();

        assert!(inbox.lock().expect("inbox").is_empty());
        let followup = timeout(Duration::from_millis(250), pump.recv_followup())
            .await
            .expect("leftover was not requeued")
            .expect("pump closed");
        assert!(followup.text.contains("arrived after the last tool boundary"));
        assert!(
            timeout(Duration::from_millis(20), pump.recv_followup())
                .await
                .is_err(),
            "the same leftover was queued more than once"
        );
    }

    #[tokio::test]
    async fn shutdown_stops_the_consumer_and_closes_its_receiver() {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut pump = AgentCompletionPump::spawn(rx, "session-a".to_string());

        pump.shutdown().await;

        assert!(pump.task_finished(), "the consumer task survived shutdown");
        assert!(
            tx.send(completion("zo-pump-after-shutdown", "discard me"))
                .is_err(),
            "the process-global receiver survived session shutdown"
        );
    }

    #[tokio::test]
    async fn dropping_the_session_aborts_the_consumer_without_a_late_delivery() {
        let (tx, rx) = mpsc::unbounded_channel();
        let pump = AgentCompletionPump::spawn(rx, "session-a".to_string());

        drop(pump);
        tokio::task::yield_now().await;

        assert!(
            tx.send(completion("zo-pump-after-drop", "discard me"))
                .is_err(),
            "a completion could still enter the dead session's consumer"
        );
    }
}
