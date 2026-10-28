//! The kill guard's road through the bridge (t-80602): a shell command an
//! agent is about to run that would signal the window or another pane is
//! refused in the reply to the agent's own pre-tool hook, before it runs.
//!
//! The rules are core's ([`zerocode_core::shell_rule::kill`]); the pids are
//! the window's ([`KillWitness`]). A knock that is not a provider's pre-tool
//! event, or whose payload does not hold the letters `kill`, is not read here
//! at all: its reply is the bytes it always was, and the process table is not
//! read for it.

use zerocode_core::shell_rule::kill::{ProcessTable, ProtectedRoot};

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
    pub protected: Vec<ProtectedRoot>,
}
