//! The live side of a lane's terminal boundary.
//!
//! [`crate::host_pty::PtySpawner`] decides where a terminal is spawned. This trait is
//! the smaller, object-safe contract the registry needs after that spawn. A
//! future SSH implementation can own its channel and terminal parser without
//! making the registry branch on local versus remote execution.

use zerocode_core::host::HostError;
use zerocode_pty::{PtyError, PtyLane, Pumped, Terminal};

/// A safe, transport-neutral failure at the live terminal boundary.
#[derive(Debug, thiserror::Error)]
pub enum PtyTransportError {
    /// The current in-process PTY failed.
    #[error(transparent)]
    Local(#[from] PtyError),
    /// A host transport failed. The category carries no endpoint or command
    /// output, so it is safe for a caller to map to user-facing copy.
    #[error("execution host terminal failure: {0}")]
    Host(HostError),
}

impl From<HostError> for PtyTransportError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

/// A running terminal owned by one lane.
///
/// These are exactly the operations [`crate::LaneRegistry`] uses. Process
/// discovery and local handles stay out of the contract; an SSH channel can
/// answer the same lifecycle questions with its own exit status and close
/// request.
pub trait PtyTransport: Send {
    /// Move available output into the terminal without blocking.
    fn pump(&mut self) -> Pumped;

    /// Parsed terminal state.
    fn terminal(&self) -> &Terminal;

    /// Parsed terminal state for view bookkeeping.
    fn terminal_mut(&mut self) -> &mut Terminal;

    /// Send bytes to the hosted program.
    fn write_input(&mut self, bytes: &[u8]) -> Result<(), PtyTransportError>;

    /// Resize both the hosted terminal and its parsed screen.
    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), PtyTransportError>;

    /// Exit code when finished, or `None` while still running.
    fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError>;

    /// Ask the hosted program to terminate.
    fn kill(&mut self) -> Result<(), PtyTransportError>;

    /// Local process id for resource accounting.
    ///
    /// A remote transport has no process on this machine and returns `None`.
    /// This is observation only: lifecycle still goes through [`Self::kill`].
    fn pid(&self) -> Option<u32> {
        None
    }

    /// When the child last wrote anything, for a transport that keeps count
    /// — the one fact that tells a shell still at work from one that has
    /// stopped, for a caller whose wait on it ran out. `None` is "this
    /// transport does not say", never "it has not written".
    fn last_output_at(&self) -> Option<std::time::Instant> {
        None
    }

    /// The same moment as [`Self::last_output_at`] on the wall clock, in
    /// epoch milliseconds, taken once when the bytes arrived — the number a
    /// reader keys a silence by, so every beat reads the same one. `None`
    /// is "this transport does not say".
    fn last_output_epoch_ms(&self) -> Option<i64> {
        None
    }

    /// Whether a local child still owns the foreground process group.
    /// Remote channels have no comparable OS process identity and answer
    /// `None`, which means no opinion rather than false.
    fn foreground_is_child(&self) -> Option<bool> {
        None
    }

    /// Local process id leading the terminal's foreground process group.
    /// Remote channels and platforms without job-control identity return
    /// `None`.
    fn foreground_process_id(&self) -> Option<u32> {
        None
    }

    /// Local foreground program names used only for agent affordances.
    /// A transport that cannot prove process identity returns no names.
    fn foreground_programs(&self) -> Vec<String> {
        Vec::new()
    }
}

/// One owned live terminal, with its concrete transport erased.
///
/// Factories can return this stable type whether they opened a local PTY or an
/// SSH channel. The registry and callers still depend only on
/// [`PtyTransport`]; the box and its concrete type stay private here.
pub struct PtyHandle {
    transport: Box<dyn PtyTransport>,
}

impl PtyHandle {
    #[must_use]
    pub fn new(transport: impl PtyTransport + 'static) -> Self {
        Self {
            transport: Box::new(transport),
        }
    }
}

impl From<PtyLane> for PtyHandle {
    fn from(lane: PtyLane) -> Self {
        Self::new(lane)
    }
}

impl PtyTransport for PtyHandle {
    fn pump(&mut self) -> Pumped {
        self.transport.pump()
    }

    fn terminal(&self) -> &Terminal {
        self.transport.terminal()
    }

    fn terminal_mut(&mut self) -> &mut Terminal {
        self.transport.terminal_mut()
    }

    fn write_input(&mut self, bytes: &[u8]) -> Result<(), PtyTransportError> {
        self.transport.write_input(bytes)
    }

    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), PtyTransportError> {
        self.transport.resize(rows, cols)
    }

    fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
        self.transport.try_wait()
    }

    fn kill(&mut self) -> Result<(), PtyTransportError> {
        self.transport.kill()
    }

    fn pid(&self) -> Option<u32> {
        self.transport.pid()
    }

    fn last_output_at(&self) -> Option<std::time::Instant> {
        self.transport.last_output_at()
    }

    fn last_output_epoch_ms(&self) -> Option<i64> {
        self.transport.last_output_epoch_ms()
    }

    fn foreground_is_child(&self) -> Option<bool> {
        self.transport.foreground_is_child()
    }

    fn foreground_process_id(&self) -> Option<u32> {
        self.transport.foreground_process_id()
    }

    fn foreground_programs(&self) -> Vec<String> {
        self.transport.foreground_programs()
    }
}

impl PtyTransport for PtyLane {
    fn pump(&mut self) -> Pumped {
        PtyLane::pump(self)
    }

    fn last_output_at(&self) -> Option<std::time::Instant> {
        PtyLane::last_output_at(self)
    }

    fn last_output_epoch_ms(&self) -> Option<i64> {
        PtyLane::last_output_epoch_ms(self)
    }

    fn terminal(&self) -> &Terminal {
        PtyLane::terminal(self)
    }

    fn terminal_mut(&mut self) -> &mut Terminal {
        PtyLane::terminal_mut(self)
    }

    fn write_input(&mut self, bytes: &[u8]) -> Result<(), PtyTransportError> {
        PtyLane::write_input(self, bytes).map_err(PtyTransportError::from)
    }

    fn resize(&mut self, rows: u16, cols: u16) -> Result<(), PtyTransportError> {
        PtyLane::resize(self, rows, cols).map_err(PtyTransportError::from)
    }

    fn try_wait(&mut self) -> Result<Option<u32>, PtyTransportError> {
        PtyLane::try_wait(self).map_err(PtyTransportError::from)
    }

    fn kill(&mut self) -> Result<(), PtyTransportError> {
        PtyLane::kill(self).map_err(PtyTransportError::from)
    }

    fn pid(&self) -> Option<u32> {
        PtyLane::pid(self)
    }

    fn foreground_is_child(&self) -> Option<bool> {
        #[cfg(windows)]
        if agent_child_programs(self).is_some() {
            return self.pid().map(|_| true);
        }
        PtyLane::foreground_is_child(self)
    }

    fn foreground_process_id(&self) -> Option<u32> {
        #[cfg(windows)]
        if agent_child_programs(self).is_some() {
            return self.pid();
        }
        PtyLane::foreground_process_id(self)
    }

    fn foreground_programs(&self) -> Vec<String> {
        #[cfg(windows)]
        if let Some(programs) = agent_child_programs(self) {
            return programs;
        }
        PtyLane::foreground_programs(self)
    }
}

/// The pty child's own names when that child IS an agent
/// ([`zerocode_core::child_is_the_agent`]) — then it holds the terminal from
/// the first instant to the last, as the unix kernel says by itself, and the
/// tools it runs under it never do. Windows only: a ConPTY's process tree
/// cannot tell a shell running a command from an agent running a tool, and
/// on unix the foreground group already answers.
#[cfg(windows)]
fn agent_child_programs(lane: &PtyLane) -> Option<Vec<String>> {
    let programs = lane.child_programs();
    zerocode_core::child_is_the_agent(&programs).then_some(programs)
}
