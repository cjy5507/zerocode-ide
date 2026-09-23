//! Which simulators this window is answerable for, and for how long.
//!
//! Three facts, one owner:
//!
//! * the device the person last opened in a pane, kept across restarts so the
//!   next window can wake it before anybody asks (D4) — never one an agent's
//!   pane borrowed, which goes down with its task (t-6336);
//! * which devices this window is answerable for at all — the ones a pane here
//!   opened or this window prebooted, and never one the person booted in their
//!   own Simulator;
//! * how long each of those has gone without a pane, which is the only thing
//!   the idle reclaimer decides on (D3).
//!
//! The road that actually shuts a device down is not here: it is
//! [`super::shutdown_simulator`], the one `simctl shutdown` in this crate, so
//! the reclaimer and the person's own 끄기 button take the same road.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::super::prefs;
use super::{boot_simulator, list_ios_simulators, shutdown_simulator};

/// Where the last device used is written down, beside Android's own record.
const USED_FILE_NAME: &str = "ios-simulators.json";
const USED_FILE_VERSION: u32 = 1;
/// A record this size is not one this window wrote.
const USED_FILE_MAX_BYTES: u64 = 64 * 1024;
/// How many devices are remembered. Enough that a person who moves between a
/// phone and a tablet keeps both, small enough that the file stays a glance.
const REMEMBERED_DEVICE_MAX: usize = 8;

/// How recently a device must have been used for the next window to wake it.
///
/// A week: long enough to cover a weekend and a Monday morning, short enough
/// that a device somebody opened once last month does not silently cost a boot
/// (and the disk a simulator's runtime wants) on every window since.
const PREBOOT_RECENT_DAYS: u64 = 7;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;
const PREBOOT_RECENT: Duration = Duration::from_secs(PREBOOT_RECENT_DAYS * SECONDS_PER_DAY);

/// How often the idle reclaimer looks.
///
/// The sweep itself costs a map lookup per device and nothing at all for a
/// window whose person never opened a device, so this is not paced by its own
/// cost — it is the resolution of a decision measured in tens of minutes, and
/// a minute of slack on a thirty-minute wait is slack nobody can feel.
const IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// One remembered device: which, what it is called, and when it was last used.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsedDevice {
    pub(super) udid: String,
    pub(super) name: String,
    /// Epoch milliseconds, the way every other record in this window keeps a
    /// moment: a reader's locale turns it into words, later, somewhere else.
    pub(super) last_used_ms: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct UsedDeviceFile {
    version: u32,
    devices: Vec<UsedDevice>,
}

/// The devices this window is answerable for, and when each was last left with
/// no pane on it (`None` while a pane is still there).
///
/// A simulator the person booted themselves is deliberately absent: this
/// window may keep its own devices warm and may reclaim them, but a device it
/// never opened is not its business to shut down.
static CLOCKS: Mutex<BTreeMap<String, Option<i64>>> = Mutex::new(BTreeMap::new());

fn clocks() -> std::sync::MutexGuard<'static, BTreeMap<String, Option<i64>>> {
    CLOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn used_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(USED_FILE_NAME)
}

fn read_used_devices(local_data_root: &Path) -> Vec<UsedDevice> {
    let file = used_file(local_data_root);
    let Ok(bytes) = crate::durable_file::read_plain_file(&file) else {
        return Vec::new();
    };
    if bytes.len() as u64 > USED_FILE_MAX_BYTES {
        return Vec::new();
    }
    let Ok(parsed) = serde_json::from_slice::<UsedDeviceFile>(&bytes) else {
        return Vec::new();
    };
    if parsed.version != USED_FILE_VERSION {
        return Vec::new();
    }
    parsed
        .devices
        .into_iter()
        .filter(|device| !device.udid.trim().is_empty())
        .take(REMEMBERED_DEVICE_MAX)
        .collect()
}

/// This device, used now — newest first, and the oldest beyond the cap let go.
///
/// Pure so the ordering rule is a test rather than a file: what the next
/// window wakes is whatever this puts at the front.
fn with_this_use(held: Vec<UsedDevice>, used: UsedDevice) -> Vec<UsedDevice> {
    let mut devices = held
        .into_iter()
        .filter(|device| device.udid != used.udid)
        .collect::<Vec<_>>();
    devices.insert(0, used);
    devices.sort_by(|left, right| right.last_used_ms.cmp(&left.last_used_ms));
    devices.truncate(REMEMBERED_DEVICE_MAX);
    devices
}

/// A pane opened this device: remember it for the next window, and take
/// responsibility for it here.
pub(super) fn remember_this_device(local_data_root: &Path, udid: &str, name: &str) {
    this_window_owns(udid);
    let devices = with_this_use(
        read_used_devices(local_data_root),
        UsedDevice {
            udid: udid.to_string(),
            name: name.to_string(),
            last_used_ms: crate::now_epoch_ms(),
        },
    );
    let Ok(bytes) = serde_json::to_vec(&UsedDeviceFile {
        version: USED_FILE_VERSION,
        devices,
    }) else {
        return;
    };
    // A record that cannot be written costs the NEXT window one cold boot, so
    // it is worth a line and worth nothing else: the pane in front of the
    // person is already opening.
    if let Err(error) = crate::durable_file::replace_bytes(&used_file(local_data_root), &bytes) {
        crate::note_window_event(
            local_data_root,
            &format!("emulator ios: the last-used record could not be written: {error}"),
        );
    }
}

/// This window is answerable for this device from now on.
pub(super) fn this_window_owns(udid: &str) {
    clocks().insert(udid.to_string(), None);
}

/// And no longer: it was put away, so a person who boots it again in their
/// own Simulator is not asking this window to watch it.
pub(super) fn this_window_lets_go(udid: &str) {
    clocks().remove(udid);
}

/// What the sweep decides for one device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IdleVerdict {
    /// A pane is on it. The clock is not running.
    Watched,
    /// No pane, and not for long enough yet.
    Waiting,
    /// No pane for the whole wait: shut it down.
    Reclaim,
}

/// The verdict for one device, from the only three facts it takes.
///
/// A device is never reclaimed on the sweep that FINDS it unwatched: that
/// sweep starts its clock. Otherwise the wait would be "up to a sweep
/// shorter than the person asked for", which for the shortest settings people
/// choose is most of the wait.
fn judge(watched: bool, idle_since_ms: Option<i64>, now_ms: i64, wait: Duration) -> IdleVerdict {
    if watched {
        return IdleVerdict::Watched;
    }
    let Some(since) = idle_since_ms else {
        return IdleVerdict::Waiting;
    };
    let waited = now_ms.saturating_sub(since).max(0) as u128;
    if waited >= wait.as_millis() {
        IdleVerdict::Reclaim
    } else {
        IdleVerdict::Waiting
    }
}

/// One pass of the reclaimer over the devices this window is answerable for.
///
/// Told the two things it cannot ask a map about — the clock and whether a
/// pane is on a device — so the whole decision is testable without a
/// simulator, a registry or a settings file.
fn sweep_with(
    now_ms: i64,
    wait: Duration,
    watched: impl Fn(&str) -> bool,
    mut reclaim: impl FnMut(&str),
) {
    let looked = clocks()
        .iter()
        .map(|(udid, since)| (udid.clone(), *since))
        .collect::<Vec<_>>();
    for (udid, since) in looked {
        match judge(watched(&udid), since, now_ms, wait) {
            IdleVerdict::Watched => {
                clocks().insert(udid, None);
            }
            IdleVerdict::Waiting => {
                if since.is_none() {
                    clocks().insert(udid, Some(now_ms));
                }
            }
            IdleVerdict::Reclaim => {
                reclaim(&udid);
                // Off is not ours any more: a person who boots it again in
                // their own Simulator is not asking this window to watch it.
                clocks().remove(&udid);
            }
        }
    }
}

/// Whether any device is this window's business at all.
fn nothing_is_ours() -> bool {
    clocks().is_empty()
}

/// Shut down every device this window is answerable for. The exit road when
/// the person has said not to keep them booted.
pub(super) fn shut_down_our_devices(local_data_root: &Path) {
    let ours = clocks().keys().cloned().collect::<Vec<_>>();
    for udid in ours {
        match shutdown_simulator(&udid) {
            Ok(()) => crate::note_window_event(
                local_data_root,
                &format!("emulator ios: shut down {udid} on the way out (keepBooted is off)"),
            ),
            Err(error) => crate::note_window_event(
                local_data_root,
                &format!("emulator ios: {udid} would not shut down on the way out: {error}"),
            ),
        }
        clocks().remove(&udid);
    }
}

/// The reclaimer itself: one thread, one look a minute, nothing at all to do
/// for a window whose person never opened a device.
pub(super) fn start_idle_reclaimer(app: AppHandle) {
    let _ = std::thread::Builder::new()
        .name("ios-idle-reclaimer".to_string())
        .spawn(move || {
            let local_data_root = app
                .state::<crate::AppState>()
                .local_data_root()
                .to_path_buf();
            loop {
                std::thread::sleep(IDLE_SWEEP_INTERVAL);
                // Before the settings are read, because the common window has
                // no device and must pay neither the read nor the look.
                if nothing_is_ours() {
                    continue;
                }
                let Some(wait) = prefs::of(&app).idle_shutdown else {
                    continue;
                };
                sweep_with(
                    crate::now_epoch_ms(),
                    wait,
                    |udid| {
                        super::super::session::registry()
                            .target_control(super::super::EmulatorPlatform::Ios, udid)
                            .is_some()
                    },
                    |udid| match shutdown_simulator(udid) {
                        Ok(()) => crate::note_window_event(
                            &local_data_root,
                            &format!(
                                "emulator ios: {udid} had no pane for {} minutes — shut down",
                                wait.as_secs() / 60
                            ),
                        ),
                        Err(error) => crate::note_window_event(
                            &local_data_root,
                            &format!("emulator ios: idle {udid} would not shut down: {error}"),
                        ),
                    },
                );
            }
        });
}

/// Whether a remembered use is recent enough to be worth a boot nobody has
/// asked for yet.
fn is_recent(last_used_ms: i64, now_ms: i64, window: Duration) -> bool {
    let ago = now_ms.saturating_sub(last_used_ms);
    // A record from the future is a clock that moved, not a use that has not
    // happened yet — the device was used, so it counts.
    ago < 0 || (ago as u128) <= window.as_millis()
}

/// The device the next pane is most likely to ask for, if it is recent enough.
fn worth_waking(devices: Vec<UsedDevice>, now_ms: i64) -> Option<UsedDevice> {
    devices
        .into_iter()
        .max_by_key(|device| device.last_used_ms)
        .filter(|device| is_recent(device.last_used_ms, now_ms, PREBOOT_RECENT))
}

/// Wake the simulator this machine last used, before anybody asks for it.
///
/// Whether this happens at all is [`super::super::preboot_last_used`]'s
/// question — the setting covers both platforms, so it is read once, there.
///
/// The DEVICE only: no stream, no pane, no Simulator window. A pane opened
/// while this boot is still in flight attaches to it rather than starting a
/// second one — `simctl boot` answers an already-waking device by saying so,
/// and [`boot_simulator`] reads that as the yes it is.
pub(super) fn preboot_last_used(app: &AppHandle) {
    let local_data_root = app
        .state::<crate::AppState>()
        .local_data_root()
        .to_path_buf();
    let Some(wanted) = worth_waking(read_used_devices(&local_data_root), crate::now_epoch_ms())
    else {
        return;
    };
    // The listing is what says the device still exists on this machine and
    // whether it is already up; a `simctl boot` for a device Xcode has since
    // deleted is an error nobody asked for.
    let devices = list_ios_simulators();
    let Some(device) = devices.iter().find(|device| device.udid == wanted.udid) else {
        return;
    };
    if device.booted {
        this_window_owns(&device.udid);
        return;
    }
    let started = std::time::Instant::now();
    match boot_simulator(device) {
        Ok(()) => {
            this_window_owns(&device.udid);
            crate::note_window_event(
                &local_data_root,
                &format!(
                    "emulator ios: prebooted {} ({}) in {}ms — last used {} minutes ago",
                    device.name,
                    device.udid,
                    started.elapsed().as_millis(),
                    crate::now_epoch_ms().saturating_sub(wanted.last_used_ms) / 60_000
                ),
            );
        }
        Err(error) => crate::note_window_event(
            &local_data_root,
            &format!(
                "emulator ios: {} could not be prebooted: {error}",
                device.name
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn used(udid: &str, at: i64) -> UsedDevice {
        UsedDevice {
            udid: udid.to_string(),
            name: format!("iPhone {udid}"),
            last_used_ms: at,
        }
    }

    /// The newest use is the front of the record, whatever order it arrived
    /// in, and one device is one row however many times it is used.
    #[test]
    fn the_record_keeps_the_newest_use_first_and_one_row_per_device() {
        let held = vec![used("a", 300), used("b", 200)];
        let after = with_this_use(held, used("b", 400));
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].udid, "b");
        assert_eq!(after[0].last_used_ms, 400);
        assert_eq!(after[1].udid, "a");
    }

    /// The record is a glance, not a history: the oldest rows fall off.
    #[test]
    fn the_record_stops_at_the_cap() {
        let mut held = Vec::new();
        for index in 0..REMEMBERED_DEVICE_MAX as i64 + 4 {
            held = with_this_use(held, used(&format!("device-{index}"), index * 10));
        }
        assert_eq!(held.len(), REMEMBERED_DEVICE_MAX);
        // And what survived is the newest, which is what a preboot reads.
        assert_eq!(
            held[0].udid,
            format!("device-{}", REMEMBERED_DEVICE_MAX + 3)
        );
    }

    /// A week is the window; a device older than that is left alone, and one
    /// whose record is from the future is a clock that moved, not a lie.
    #[test]
    fn only_a_recently_used_device_is_worth_waking() {
        let now = 1_000 * 60 * 60 * 24 * 30;
        let day = 1_000 * 60 * 60 * 24;
        assert_eq!(
            worth_waking(
                vec![used("old", now - day * 8), used("fresh", now - day)],
                now
            )
            .map(|device| device.udid),
            Some("fresh".to_string())
        );
        assert_eq!(worth_waking(vec![used("old", now - day * 8)], now), None);
        assert!(is_recent(now + day, now, PREBOOT_RECENT));
        // The edge itself is inside the window rather than outside it.
        assert!(is_recent(
            now - PREBOOT_RECENT.as_millis() as i64,
            now,
            PREBOOT_RECENT
        ));
    }

    /// A device with a pane on it is never reclaimed, however long the window
    /// has been open.
    #[test]
    fn a_watched_device_is_never_reclaimed() {
        assert_eq!(
            judge(true, Some(0), 1_000_000_000, Duration::from_secs(1)),
            IdleVerdict::Watched
        );
    }

    /// The sweep that notices a device went quiet starts its clock; the one
    /// after the wait shuts it down.
    #[test]
    fn the_clock_starts_before_it_runs_out() {
        let wait = Duration::from_secs(30 * 60);
        assert_eq!(judge(false, None, 1_000, wait), IdleVerdict::Waiting);
        assert_eq!(judge(false, Some(1_000), 1_000, wait), IdleVerdict::Waiting);
        assert_eq!(
            judge(
                false,
                Some(1_000),
                1_000 + wait.as_millis() as i64 - 1,
                wait
            ),
            IdleVerdict::Waiting
        );
        assert_eq!(
            judge(false, Some(1_000), 1_000 + wait.as_millis() as i64, wait),
            IdleVerdict::Reclaim
        );
    }

    /// The sweep as a whole: a pane clears the clock, an empty device starts
    /// one, and the wait's end is one shutdown and one forgotten device.
    #[test]
    fn a_sweep_starts_clocks_clears_them_and_reclaims_exactly_once() {
        let wait = Duration::from_secs(600);
        clocks().clear();
        this_window_owns("watched");
        this_window_owns("quiet");
        // First look: the quiet one's clock starts, the watched one has none.
        sweep_with(
            10_000,
            wait,
            |udid| udid == "watched",
            |udid| panic!("nothing is reclaimed on the sweep that finds it: {udid}"),
        );
        assert_eq!(clocks().get("quiet"), Some(&Some(10_000)));
        assert_eq!(clocks().get("watched"), Some(&None));
        // Still inside the wait: nothing happens and the clock is not reset.
        sweep_with(
            10_000 + 599_000,
            wait,
            |_| false,
            |udid| panic!("reclaimed before the wait was over: {udid}"),
        );
        assert_eq!(clocks().get("quiet"), Some(&Some(10_000)));
        // Past it: one shutdown, and that device stops being ours. The one
        // whose clock started later is still waiting out its own wait.
        let mut reclaimed = Vec::new();
        sweep_with(
            10_000 + 600_000,
            wait,
            |_| false,
            |udid| reclaimed.push(udid.to_string()),
        );
        assert_eq!(reclaimed, vec!["quiet".to_string()]);
        assert!(clocks().get("quiet").is_none());
        assert_eq!(clocks().get("watched"), Some(&Some(10_000 + 599_000)));
        clocks().clear();
        assert!(nothing_is_ours());
    }
}
