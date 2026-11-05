//! What the window does differently on a machine with little memory (low-spec 6).
//!
//! A conversation the boot wakes is an agent process: one Claude Code was 350 to
//! 700 MB resident on 2026-11-05, and a window that had ten running when it went
//! put all ten back behind the workspace in front, one after another, before the
//! person had looked at any of them. On a 16 GB machine that is room to spare; on
//! an 8 GB one it is most of the memory there is. So on such a machine the boot
//! wakes the workspace in front and then only as many conversations behind it as
//! the budget holds. The others stay asleep exactly as an unvisited workspace's
//! do, and wake the moment somebody steps into their workspace.
//!
//! The same machine puts a conversation back to sleep in session: a tab behind
//! the workspace in front whose conversations have all rested for
//! [`IDLE_SLEEP_AFTER_MS`] becomes the stored tab a restart would have left
//! (its record and its last screen kept, its shells closed), and wakes by the
//! same road when somebody looks at it. The window picks the tabs; the panes a
//! close would hurt stay awake ([`stay_awake`]).

/// Physical memory below this is an "8 GB" machine: the operating system
/// reports a little under the number on the box, and the next common size is 16.
pub(crate) const LOW_MEMORY_BYTES: u64 = 12 * 1024 * 1024 * 1024;

/// How many conversations such a machine wakes at the boot, the front's
/// included — three agents and the window come to about half of 8 GB.
pub(crate) const LOW_MEMORY_BOOT_CONVERSATIONS: usize = 3;

/// The boot's budget of conversations on this machine, or `None` for no limit
/// (enough memory, or memory this platform cannot read).
#[must_use]
pub(crate) fn boot_conversation_budget() -> Option<usize> {
    budget_for(physical_memory())
}

/// The budget for a machine of `memory` bytes.
#[must_use]
pub(crate) fn budget_for(memory: Option<u64>) -> Option<usize> {
    memory
        .filter(|bytes| *bytes > 0 && *bytes < LOW_MEMORY_BYTES)
        .map(|_| LOW_MEMORY_BOOT_CONVERSATIONS)
}

/// How long every conversation in a tab behind the front must have rested
/// before such a machine puts the tab to sleep: long enough that a person
/// who stepped away for a coffee comes back to the pane as they left it.
pub(crate) const IDLE_SLEEP_AFTER_MS: u64 = 30 * 60 * 1000;

/// After how long a resting tab behind the front is put to sleep on this
/// machine, or `None` where the memory is enough — the boot budget's line.
#[must_use]
pub(crate) fn idle_sleep_after_ms() -> Option<u64> {
    idle_sleep_for(physical_memory())
}

/// The rest a machine of `memory` bytes allows before a tab goes to sleep.
#[must_use]
pub(crate) fn idle_sleep_for(memory: Option<u64>) -> Option<u64> {
    budget_for(memory).map(|_| IDLE_SLEEP_AFTER_MS)
}

/// Why one pane of a tab the window would put to sleep has to stay awake.
/// Any one of them keeps the whole tab awake: a tab sleeps whole or not at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StayAwake {
    /// The shell is gone already.
    Gone,
    /// A codex the person typed into a shell: its conversation, its commands
    /// and its hooks run in the shared app-server under `~/.codex`, so closing
    /// the pane frees nothing, and a late report from the daemon would find no
    /// pane of its own to land on (run-77627, t-94962).
    TypedCodex,
    /// The agent is running commands a close would cut — a tool's shell, a
    /// background job the conversation is waiting to hear from.
    Commands(Vec<String>),
    /// This platform's process table cannot tell the agent's own helpers from
    /// the commands it runs (no process groups — Windows), so nothing is cut
    /// on a guess.
    Unknown,
}

impl StayAwake {
    /// The window log's words for it.
    pub(crate) fn said(&self) -> String {
        match self {
            Self::Gone => "the shell is gone".to_string(),
            Self::TypedCodex => {
                "a codex typed into a shell runs in the shared app-server".to_string()
            }
            Self::Commands(commands) => format!("running {}", commands.join(", ")),
            Self::Unknown => "this platform cannot tell its commands from its helpers".to_string(),
        }
    }
}

/// What the window knows of one pane it would put to sleep.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneFacts<'a> {
    /// Whether the shell still stands.
    pub(crate) standing: bool,
    /// Whether it is a codex the person typed into a shell.
    pub(crate) typed_codex: bool,
    /// The commands its agent runs, as `ProcessSample::agent_commands`
    /// reads them — `None` when the table cannot say.
    pub(crate) commands: Option<&'a [String]>,
}

/// Whether `pane` has to stay awake, and why.
#[must_use]
pub(crate) fn stay_awake(pane: PaneFacts<'_>) -> Option<StayAwake> {
    if !pane.standing {
        return Some(StayAwake::Gone);
    }
    if pane.typed_codex {
        return Some(StayAwake::TypedCodex);
    }
    match pane.commands {
        None => Some(StayAwake::Unknown),
        Some([]) => None,
        Some(commands) => Some(StayAwake::Commands(commands.to_vec())),
    }
}

/// The machine's physical memory in bytes, when the platform says.
#[cfg(unix)]
#[must_use]
pub(crate) fn physical_memory() -> Option<u64> {
    // SAFETY: `sysconf` reads two system constants; nothing is passed in and
    // no memory crosses.
    let (pages, page) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    let pages = u64::try_from(pages).ok()?;
    let page = u64::try_from(page).ok()?;
    pages.checked_mul(page)
}

/// The machine's physical memory in bytes, when the platform says.
#[cfg(windows)]
#[must_use]
pub(crate) fn physical_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: an all-zero MEMORYSTATUSEX is a valid value of a plain C struct.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).ok()?;
    // SAFETY: `status` is a live, writable MEMORYSTATUSEX whose length field
    // names its own size, which is all the call reads before it writes.
    let read = unsafe { GlobalMemoryStatusEx(&mut status) };
    (read != 0).then_some(status.ullTotalPhys)
}

/// The machine's physical memory in bytes, when the platform says.
#[cfg(not(any(unix, windows)))]
#[must_use]
pub(crate) fn physical_memory() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn an_eight_gigabyte_machine_wakes_three_conversations_and_a_larger_one_all() {
        assert_eq!(budget_for(Some(8 * GIB)), Some(3));
        assert_eq!(budget_for(Some(8 * GIB - 300 * 1024 * 1024)), Some(3));
        assert_eq!(budget_for(Some(16 * GIB)), None);
        assert_eq!(budget_for(Some(64 * GIB)), None);
        assert_eq!(
            budget_for(None),
            None,
            "a machine that says nothing is not limited"
        );
        assert_eq!(budget_for(Some(0)), None);
    }

    #[test]
    fn only_a_small_machine_puts_a_resting_tab_to_sleep_and_after_half_an_hour() {
        assert_eq!(idle_sleep_for(Some(8 * GIB)), Some(IDLE_SLEEP_AFTER_MS));
        assert_eq!(IDLE_SLEEP_AFTER_MS, 1_800_000);
        assert_eq!(idle_sleep_for(Some(16 * GIB)), None);
        assert_eq!(
            idle_sleep_for(None),
            None,
            "a machine that says nothing sleeps nothing"
        );
    }

    #[test]
    fn a_pane_stays_awake_while_a_close_would_cut_something() {
        let none: &[String] = &[];
        let quiet = PaneFacts {
            standing: true,
            typed_codex: false,
            commands: Some(none),
        };
        assert_eq!(
            stay_awake(quiet),
            None,
            "a resting agent with nothing running sleeps"
        );
        assert_eq!(
            stay_awake(PaneFacts {
                standing: false,
                ..quiet
            }),
            Some(StayAwake::Gone)
        );
        assert_eq!(
            stay_awake(PaneFacts {
                typed_codex: true,
                ..quiet
            }),
            Some(StayAwake::TypedCodex)
        );
        let running = ["zsh -c sleep 600".to_string()];
        let busy = stay_awake(PaneFacts {
            commands: Some(&running),
            ..quiet
        });
        assert_eq!(busy, Some(StayAwake::Commands(running.to_vec())));
        assert_eq!(
            busy.map(|why| why.said()).as_deref(),
            Some("running zsh -c sleep 600")
        );
        assert_eq!(
            stay_awake(PaneFacts {
                commands: None,
                ..quiet
            }),
            Some(StayAwake::Unknown),
            "a table that cannot tell helpers from commands cuts nothing"
        );
    }

    #[test]
    fn this_machine_says_how_much_memory_it_has() {
        let memory = physical_memory().expect("the platform reads its memory");
        assert!(memory >= GIB, "{memory} bytes is not a machine");
    }
}
