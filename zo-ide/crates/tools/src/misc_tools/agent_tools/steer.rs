//! Delivery is a receipt (t-2513 contract 2).
//!
//! `SendMessage{to: <child>}` used to answer `delivered: true` the moment a
//! string was pushed onto a process-global queue — a queue a PANE child never
//! read, because it is another process. The answer was a lie two ways at
//! once: it said nothing about whether the child had read the words, and for
//! a pane child nothing ever would.
//!
//! So a steer now goes down a [`SteerRoute`] chosen from the child's own
//! manifest, and comes back with a [`SteerOutcome`] whose receipt says what
//! actually happened:
//!
//! | route | `consumed` | `queued` | `rejected` |
//! |---|---|---|---|
//! | in-process queue | a boundary of the running turn READ it (the runtime's steering observer stamps the manifest) | pushed; the child is alive and drains at its next boundary | no live queue in this process |
//! | pane channel | the child answered `session.steer` with the turn it landed in | the child was idle and opened its next turn with it | nobody answered the channel, or its door is shut |
//!
//! `delivered` stays the old boolean and means `receipt != rejected`.

use std::path::PathBuf;

use runtime::subagent_panes::{
    steer_over_channel, Limits, SteerOutcome, SteerReceiptRecord, CHANNEL_FILE,
};

use super::AgentOutput;

/// The road one steer takes to one child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SteerRoute {
    /// A thread of this process: the registered steering queue.
    InProcess,
    /// A zo of its own: its events channel, found through its channel file.
    PaneChannel { channel: PathBuf },
}

impl SteerRoute {
    /// The route a manifest names. A pane child whose channel file the parent
    /// has not seen yet (it is still booting) is reached through the queue —
    /// which it never reads — so the receipt comes back honest: `rejected`,
    /// unless a same-id in-process registration exists.
    #[must_use]
    pub(crate) fn for_manifest(manifest: &AgentOutput) -> Self {
        match (
            manifest.lifecycle.execution.as_deref(),
            manifest.lifecycle.channel.clone(),
        ) {
            (Some(super::EXECUTION_PANE), Some(channel)) => Self::PaneChannel { channel },
            _ => Self::InProcess,
        }
    }
}

/// Steer `manifest`'s agent with `text` and say what became of it.
///
/// The receipt is written onto the manifest as `lastReceipt` before it is
/// returned, so the roster frame and the hook reporter say the same thing the
/// tool result did.
pub(crate) fn steer_agent_with_receipt(manifest: &AgentOutput, text: String) -> SteerOutcome {
    let outcome = match SteerRoute::for_manifest(manifest) {
        SteerRoute::InProcess => steer_in_process(manifest, text),
        SteerRoute::PaneChannel { channel } => {
            steer_over_channel(&channel, &text, Limits::load().channel_timeout)
        }
    };
    super::manifest::stamp_agent_receipt(manifest, SteerReceiptRecord::now(&outcome));
    outcome
}

/// The in-process half: a push onto the registered queue is `queued`; the
/// runtime's steering observer turns it into `consumed` when a boundary reads
/// it ([`super::agent_runtime::build_agent_runtime`]). No queue is a rejection
/// that says so.
fn steer_in_process(manifest: &AgentOutput, text: String) -> SteerOutcome {
    if super::steer_agent(&manifest.agent_id, text) {
        SteerOutcome::queued()
    } else {
        SteerOutcome::rejected(
            "the agent is marked running but has no live steering handle in this process (it may \
             be finishing, or owned by another process)",
        )
    }
}

/// The pane children of `registry` whose channels can still be spoken to —
/// the fan-out list for `auth.reload` (t-2513 §2.6). Idle children have
/// terminal manifests, so status is not the filter: a channel file that is
/// there is.
#[must_use]
pub fn live_pane_children(registry: &super::AgentRegistry) -> Vec<PaneChildChannel> {
    registry
        .manifest_paths()
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| serde_json::from_str::<AgentOutput>(&text).ok())
        .filter(|manifest| manifest.lifecycle.execution.as_deref() == Some(super::EXECUTION_PANE))
        .filter_map(|manifest| {
            let channel = manifest
                .lifecycle
                .channel
                .clone()
                .or_else(|| {
                    super::panes::child_directory(&manifest)
                        .ok()
                        .map(|directory| directory.join(CHANNEL_FILE))
                })
                .filter(|channel| channel.is_file())?;
            Some(PaneChildChannel {
                agent_id: manifest.agent_id,
                channel,
            })
        })
        .collect()
}

/// One pane child's address, for a caller that speaks to every live child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneChildChannel {
    pub agent_id: String,
    pub channel: PathBuf,
}

impl PaneChildChannel {
    /// One call on this child's channel, bounded by the limits table.
    ///
    /// # Errors
    ///
    /// The child did not answer, refused, or answered nonsense.
    pub fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, runtime::subagent_panes::ChannelCallError> {
        let coordinates = runtime::subagent_panes::ChannelCoordinates::read(&self.channel)
            .map_err(runtime::subagent_panes::ChannelCallError::Unreachable)?;
        coordinates.call(method, params, Limits::load().channel_timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::subagent_panes::SteerReceipt;

    /// An in-process child with no live queue is a rejection that says so;
    /// with one, the push is `queued` — never `consumed`, which only a
    /// boundary that read the words may say.
    #[test]
    fn the_in_process_route_answers_queued_or_rejected_never_consumed() {
        let isolated = crate::misc_tools::agent_tools::tests::IsolatedStore::new("steer-inproc");
        let manifest = isolated.running_manifest("agent-steer-1");
        assert_eq!(SteerRoute::for_manifest(&manifest), SteerRoute::InProcess);

        let rejected = steer_agent_with_receipt(&manifest, "anyone?".to_string());
        assert_eq!(rejected.receipt, SteerReceipt::Rejected);
        assert!(!rejected.delivered());
        assert!(rejected.reason.as_deref().is_some_and(|why| why.contains("no live steering handle")));
        let stored = isolated.read_manifest("agent-steer-1");
        assert_eq!(
            stored.lifecycle.last_receipt.as_ref().map(|record| record.receipt),
            Some(SteerReceipt::Rejected)
        );

        let queue = runtime::SteeringQueue::default();
        super::super::register_agent_steering(
            manifest.agent_id.clone(),
            manifest.run_generation,
            queue.clone(),
        );
        let queued = steer_agent_with_receipt(&manifest, "look again".to_string());
        assert_eq!(queued.receipt, SteerReceipt::Queued);
        assert!(queued.delivered());
        assert_eq!(queue.lock().unwrap().as_slice(), ["look again".to_string()]);
        let stored = isolated.read_manifest("agent-steer-1");
        assert_eq!(
            stored.lifecycle.last_receipt.as_ref().map(|record| record.receipt),
            Some(SteerReceipt::Queued)
        );
        // The observer's stamp — what the inline runtime does when a boundary
        // drains — flips the same field to `consumed`.
        super::super::manifest::stamp_agent_receipt_at_path(
            std::path::Path::new(&manifest.manifest_file),
            SteerReceiptRecord::now(&SteerOutcome::consumed(None)),
        );
        let stored = isolated.read_manifest("agent-steer-1");
        assert_eq!(
            stored.lifecycle.last_receipt.as_ref().map(|record| record.receipt),
            Some(SteerReceipt::Consumed)
        );
        super::super::unregister_agent_steering(&manifest.agent_id, manifest.run_generation);
    }

    /// A pane child is reached through its channel file, and the receipt is
    /// whatever the child answered — including nobody answering.
    #[test]
    fn the_pane_route_reads_the_receipt_off_the_childs_channel() {
        let isolated = crate::misc_tools::agent_tools::tests::IsolatedStore::new("steer-pane");
        let mut manifest = isolated.running_manifest("agent-steer-2");
        let directory = isolated.store.join("agent-steer-2");
        std::fs::create_dir_all(&directory).unwrap();
        let channel = directory.join(CHANNEL_FILE);
        manifest.lifecycle.execution = Some(super::super::EXECUTION_PANE.to_string());
        manifest.lifecycle.channel = Some(channel.clone());
        super::super::manifest::write_agent_manifest(&manifest).unwrap();
        assert_eq!(
            SteerRoute::for_manifest(&manifest),
            SteerRoute::PaneChannel { channel: channel.clone() }
        );

        // A listener that answers `session.steer` with a running turn.
        let (coordinates, served) = fake_child_channel(serde_json::json!({"turn_id": 4}));
        coordinates.write(&channel).unwrap();
        let consumed = steer_agent_with_receipt(&manifest, "look again".to_string());
        assert_eq!(consumed.receipt, SteerReceipt::Consumed);
        assert_eq!(consumed.turn_id, Some(4));
        let asked = served.join().unwrap();
        assert_eq!(asked[0]["params"]["text"], "look again");
        let stored = isolated.read_manifest("agent-steer-2");
        assert_eq!(
            stored.lifecycle.last_receipt.as_ref().map(|record| record.receipt),
            Some(SteerReceipt::Consumed)
        );

        // The listener is gone: rejected, with the reason.
        let rejected = steer_agent_with_receipt(&manifest, "anyone?".to_string());
        assert_eq!(rejected.receipt, SteerReceipt::Rejected);
        assert!(rejected.reason.as_deref().is_some_and(|why| why.contains("unreachable")), "{rejected:?}");

        // And the fan-out list names this child by its channel file.
        let registry = crate::misc_tools::agent_tools::AgentRegistry::at_root_for_tests(
            "session-pane-test",
            &isolated.store,
        );
        let children = live_pane_children(&registry);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].agent_id, "agent-steer-2");
        assert_eq!(children[0].channel, channel);
        std::fs::remove_file(&channel).unwrap();
        assert!(live_pane_children(&registry).is_empty(), "a gone channel file is a gone child");
    }

    /// One accept, one `session.steer` answer, then gone — the shape of a
    /// child that answers once and a port that is closed afterwards.
    fn fake_child_channel(
        answer: serde_json::Value,
    ) -> (
        runtime::subagent_panes::ChannelCoordinates,
        std::thread::JoinHandle<Vec<serde_json::Value>>,
    ) {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let Ok((stream, _)) = listener.accept() else {
                return seen;
            };
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let request: serde_json::Value = serde_json::from_str(line.trim()).expect("json");
            seen.push(request.clone());
            let mut writer = stream;
            let response = serde_json::json!({"jsonrpc": "2.0", "id": request["id"], "result": answer});
            let _ = writeln!(writer, "{response}");
            seen
        });
        (
            runtime::subagent_panes::ChannelCoordinates {
                addr,
                token: None,
                session_id: "child".to_string(),
            },
            handle,
        )
    }
}
