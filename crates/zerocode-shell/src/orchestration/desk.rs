//! The coordinator's desk on the task board (t-6588,
//! docs/design/agent-board-round4.md).
//!
//! What a coordinator typed `worker-list`, `check --peek`, `task-list`,
//! `df -g`, `uptime` and `xcrun simctl list` for all day, answered from what
//! this window already holds: the ledger reading the board's beat publishes
//! ([`super::refresh_board_ledger`]) and the machine that ledger lives on.
//! Nothing here reads the ledger a second way, and nothing here judges a fact
//! the ledger already judges — the disk's word is the rule a `--worktree`
//! summons is refused by ([`zerocode_core::orchestration::worktree_room`]).

use std::path::Path;

use serde::Serialize;
use zerocode_core::orchestration::{WorktreeRoom, worktree_room};

/// The machine strip's one answer: what `df -g`, `uptime` and `simctl` told a
/// coordinator.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct MachineLoad {
    /// `None` where the volume could not be measured — unmeasured, never
    /// empty.
    pub(crate) disk: Option<MachineDisk>,
    /// `None` where the platform has no load average.
    pub(crate) load: Option<LoadAverage>,
    pub(crate) devices: BootedDevices,
}

/// The volume the ledger lives on, in the ledger's own words.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct MachineDisk {
    pub(crate) free_bytes: u64,
    /// The ladder the ledger's own disk notices say it in
    /// (`workspace_space::format_bytes`), so the strip and a refusal never
    /// spell one number two ways.
    pub(crate) free: String,
    /// The path measured.
    pub(crate) at: String,
    /// The verdict the next `--worktree` summons would meet.
    pub(crate) room: WorktreeRoom,
    /// The checkouts that verdict was judged beside.
    pub(crate) held_checkouts: usize,
}

/// The one-minute load average against the cores it is shared by.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct LoadAverage {
    pub(crate) one_minute: f64,
    pub(crate) cores: usize,
    /// Busier than its cores: the reading under which the harness's frame
    /// budgets are recorded rather than judged (`ui/tests/machine-load.mjs`)
    /// and the release lane waits for calm — the same line, drawn here.
    pub(crate) loud: bool,
}

/// How many simulators and emulators are up on this machine, whoever booted
/// them. `None` is "this machine cannot say" (no Android SDK, not macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct BootedDevices {
    pub(crate) ios_booted: Option<usize>,
    pub(crate) android_booted: Option<usize>,
}

/// The machine's one-minute load average and its cores — one call, no
/// subprocess. The walk bench reads its load through this too.
pub(crate) fn load_average() -> Option<LoadAverage> {
    #[cfg(unix)]
    {
        let mut samples = [0f64; 3];
        // SAFETY: `getloadavg` writes at most the one element asked for into
        // the buffer it is handed, which outlives the call.
        let answered = unsafe { libc::getloadavg(samples.as_mut_ptr(), 1) };
        if answered < 1 {
            return None;
        }
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        Some(load_reading(samples[0], cores))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn load_reading(one_minute: f64, cores: usize) -> LoadAverage {
    let capacity = f64::from(u32::try_from(cores).unwrap_or(u32::MAX));
    LoadAverage {
        one_minute,
        cores,
        loud: one_minute > capacity,
    }
}

/// Read the strip: the ledger's volume with the summons' own verdict beside
/// the checkouts live workers hold (as the board's beat last published
/// them), the load, and the booted devices. `simctl` and `adb` are processes,
/// so the caller runs this off the main thread.
pub(crate) fn machine_load(ledger_volume: &Path) -> MachineLoad {
    let held = super::board_ledger_snapshot().held_checkouts;
    let disk = super::free_bytes_at(ledger_volume).map(|free_bytes| MachineDisk {
        free_bytes,
        free: zerocode_core::workspace_space::format_bytes(free_bytes),
        at: ledger_volume.display().to_string(),
        room: worktree_room(free_bytes, held),
        held_checkouts: held,
    });
    let (ios_booted, android_booted) = crate::emulator::booted_devices();
    MachineLoad {
        disk,
        load: load_average(),
        devices: BootedDevices {
            ios_booted,
            android_booted,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Loud" is the harness's line: busier than the cores, not at them.
    #[test]
    fn a_machine_is_loud_only_past_its_cores() {
        assert!(!load_reading(12.0, 12).loud);
        assert!(load_reading(12.1, 12).loud);
        assert!(!load_reading(0.0, 1).loud);
    }

    /// The strip's answer carries the ledger's own words: the verdict in
    /// snake case and the free space in the ledger's ladder.
    #[test]
    fn the_strip_speaks_the_ledgers_words() {
        let disk = MachineDisk {
            free_bytes: 21 * 1024 * 1024 * 1024,
            free: zerocode_core::workspace_space::format_bytes(21 * 1024 * 1024 * 1024),
            at: "/ledger".into(),
            room: worktree_room(21 * 1024 * 1024 * 1024, 2),
            held_checkouts: 2,
        };
        let said = serde_json::to_value(&disk).expect("serializes");
        assert_eq!(said["room"], "tight");
        assert_eq!(said["free"], "21.0 GB");
    }
}
