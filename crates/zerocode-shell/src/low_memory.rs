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
    fn this_machine_says_how_much_memory_it_has() {
        let memory = physical_memory().expect("the platform reads its memory");
        assert!(memory >= GIB, "{memory} bytes is not a machine");
    }
}
