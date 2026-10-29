//! The kill guard's road through the bridge (t-80602): a shell command an
//! agent is about to run that would signal the window or another pane is
//! refused in the reply to the agent's own pre-tool hook, before it runs.
//!
//! The rules are core's ([`zerocode_core::shell_rule::kill`]); the pids are
//! the window's ([`KillWitness`]). A knock that is not a provider's pre-tool
//! event, or whose payload does not hold the letters `kill`, is not read here
//! at all: its reply is the bytes it always was, and the process table is not
//! read for it.

use std::sync::Arc;
use std::time::Duration;

use zerocode_core::AgentKind;
use zerocode_core::hook_continuation::{self, PreToolDecision};
use zerocode_core::shell_rule::kill::{
    self, KillCall, KillVerdict, PaneMark, ProcessTable, ProtectedRoot,
};

/// What the window knows of its panes' processes, asked only for a command
/// that would signal something.
pub trait KillWitness: Send + Sync {
    /// The calling pane's own process and the trees no pane but their own
    /// may signal — the window itself, every other pane.
    fn roots(&self, pane_key: &str) -> KillRoots;
    /// The process table, read now.
    fn table(&self) -> ProcessTable;
}

/// [`KillWitness::roots`]'s answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KillRoots {
    /// The pane's own process, `None` when the window does not know it.
    pub caller: Option<u32>,
    /// The mark the window wrote into the calling pane's environment when it
    /// started it (t-81714) — what a job the pane set loose names. The window
    /// knows it whatever the pane runs: the pane's own process may be a
    /// program whose environment does not read (macOS's `/bin/zsh`).
    pub caller_mark: Option<PaneMark>,
    pub protected: Vec<ProtectedRoot>,
}

/// A knock the guard has something to say about.
pub(crate) struct Pending {
    decision: PreToolDecision,
    calls: Vec<KillCall>,
}

/// The kill calls a knock carries, when it is `agent`'s pre-tool event, its
/// hook can refuse, and its command would signal something.
pub(crate) fn pending(agent: AgentKind, event: &str, payload: &str) -> Option<Pending> {
    let decision = hook_continuation::pre_tool_decision(agent)?;
    if super::normalized_hook_event(event) != super::normalized_hook_event(decision.event)
        || !kill::might_signal(payload)
    {
        return None;
    }
    let command = zerocode_core::hook_guard::command_about(event, payload)?;
    let calls = kill::kill_calls(&command);
    (!calls.is_empty()).then_some(Pending { decision, calls })
}

/// The reply for `pending`: judged on a blocking thread — the table is a
/// process listing — within `wall`, and refused when it is not.
pub(crate) async fn reply(
    witness: Arc<dyn KillWitness>,
    pane_key: String,
    pending: Pending,
    wall: Duration,
) -> Option<serde_json::Value> {
    let Pending { decision, calls } = pending;
    let written = calls
        .iter()
        .map(|call| call.written.clone())
        .collect::<Vec<_>>()
        .join("; ");
    let judging = tokio::task::spawn_blocking(move || {
        let roots = witness.roots(&pane_key);
        let table = if calls.iter().any(KillCall::needs_table) {
            match (roots.caller, roots.caller_mark) {
                // The window's word for the pane it started the caller in:
                // the caller's own environment may not read (t-81714).
                (Some(caller), Some(mark)) => witness.table().with_pane_mark(caller, mark),
                _ => witness.table(),
            }
        } else {
            ProcessTable::new(Vec::new())
        };
        kill::judge_kill(&calls, &table, roots.caller, &roots.protected)
    });
    let verdict = match tokio::time::timeout(wall, judging).await {
        Ok(Ok(verdict)) => verdict,
        _ => KillVerdict::Ask {
            reason: format!(
                "`{written}` could not be checked within {} ms; a person should say yes first",
                wall.as_millis()
            ),
        },
    };
    answer(decision, &verdict)
}

/// What a pane can do instead of a kill it was refused for want of a person
/// to ask: the pids it learns are its own to end by number, and a process it
/// did not start is not its own to end.
const WAY_OUT: &str = "nobody in a ZeroCode pane can answer a question, so it was refused. \
To end what this pane started, name it so it can be checked before it runs: \
its pid as a number (`echo $!` or `pgrep -f <name>`, then `kill 4242`), \
`kill $(pgrep -f <name>)`, or its job (`kill %1`, `kill $!` in the same command). \
A process this pane did not start is the person's or the coordinator's to end, \
not this pane's";

/// The hook reply a verdict takes: nothing for a call that may run, so its
/// reply stays what it was, and a refusal otherwise. A pane is a host with
/// nobody to ask ([`KillVerdict::Ask`]) — a worker's pane has no person at
/// it, and one sitting on a question waits forever — so a question is a
/// refusal that says what the pane can do instead.
pub(crate) fn answer(
    decision: PreToolDecision,
    verdict: &KillVerdict,
) -> Option<serde_json::Value> {
    let reason = match verdict {
        KillVerdict::Allow => return None,
        KillVerdict::Ask { reason } => format!("{reason}; {WAY_OUT}"),
        KillVerdict::Deny { reason } => reason.clone(),
    };
    Some(serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": decision.event,
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    }))
}
