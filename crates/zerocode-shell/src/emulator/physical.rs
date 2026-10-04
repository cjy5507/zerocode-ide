//! The real devices `zerocode-emulator list` names and cannot drive (t-36920).
//!
//! The words, the readers and the shape of the answer are
//! `zerocode_core::agent_emulator::physical`'s. This file runs the tools — Xcode's
//! `devicectl` for iPhones and iPads, `adb` for Android phones — without letting
//! them slow the list:
//!
//! - the tools run beside the simulator and emulator readers, not after them, so
//!   a `list` costs the slowest of them and not the sum;
//! - a caller waits for the real-device tools at most [`PROBE_WAIT`]; a tool that
//!   needs longer is left to finish and the next `list` has its answer, and the
//!   answer for now says it is still looking instead of saying "none";
//! - `devicectl` is the slow one (it asks a system service), so its reading is
//!   kept for [`PROBE_TTL`] and taken by one probe at a time, however many
//!   `list` calls arrive; `adb` answers in milliseconds and is asked every time;
//! - a machine without the tool pays one failed spawn per [`PROBE_TTL`] and the
//!   answer names the gap.
//!
//! There is no loop here: a probe runs because a `list` asked for it, on a
//! thread that ends with it.

use std::time::Duration;

use serde_json::Value;
use zerocode_core::agent_emulator::EMULATOR_HOLD_MS;
use zerocode_core::agent_emulator::physical::{PhysicalDevice, PlatformReading, list_answer};
use zerocode_core::computer_use::EmulatorPlatform;

use super::{android_emulators_direct, mobile_emulators_direct};

/// How long one `list` waits for the tools that name real devices: half of what
/// a walk lets a `list` hold (`EMULATOR_HOLD_MS`), so the simulators, the
/// emulators and these tools together stay inside it. `devicectl` answered in
/// 0.04–0.06 s on a warm service (2026-10-04, `time` over three runs); the wait
/// is for the cold start of that service, and for `adb` starting its server.
const PROBE_WAIT: Duration = Duration::from_millis(EMULATOR_HOLD_MS / 2);

/// How long a tool may run before it is killed. A probe that outlives its
/// caller's wait still ends, and ends the thread it runs on.
#[cfg(any(target_os = "macos", test))]
const PROBE_TOOL_CAP: Duration = Duration::from_secs(10);

/// How long `devicectl` is given by its own `--timeout`: less than
/// [`PROBE_TOOL_CAP`], so it ends itself and says why before it is killed.
#[cfg(any(target_os = "macos", test))]
const DEVICECTL_TIMEOUT_SECS: u64 = 8;

/// How long a `devicectl` reading is reused. Shorter than a person needs to
/// plug a cable, say so to an agent and have it call `list`; longer than one
/// agent's polling of `list` while a device boots, which is the call that
/// would otherwise start a `devicectl` each second. An empty reading is kept
/// as long as a full one: "nothing" is the answer a plugged-in phone makes
/// wrong, and five seconds is the most it can be wrong for.
#[cfg(any(target_os = "macos", test))]
const PROBE_TTL: Duration = Duration::from_secs(5);

/// The most a tool's answer is read: a `devicectl` list of a drawer of paired
/// phones is a few kilobytes, so this is a bound on a tool gone wrong.
#[cfg(any(target_os = "macos", test))]
const PROBE_OUTPUT_MAX_BYTES: usize = 256 * 1024;

/// What `list` says while `devicectl` is still working.
#[cfg(any(target_os = "macos", test))]
const STILL_LOOKING: &str = "Xcode's devicectl is still looking for real iPhones and iPads; ask list again in a few seconds";

#[cfg(any(target_os = "macos", test))]
mod kept {
    //! One reading, kept for a short time and taken by one probe at a time.

    use std::sync::{Condvar, Mutex};
    use std::time::{Duration, Instant};

    use zerocode_core::agent_emulator::physical::PhysicalDevice;

    use super::{STILL_LOOKING, held};

    /// What a probe found, or why it found nothing.
    pub(super) type Reading = Result<Vec<PhysicalDevice>, String>;

    struct Taken {
        at: Instant,
        reading: Reading,
    }

    struct Slot {
        taken: Option<Taken>,
        running: bool,
    }

    pub(super) struct Kept {
        slot: Mutex<Slot>,
        arrived: Condvar,
    }

    impl Kept {
        pub(super) const fn new() -> Self {
            Self {
                slot: Mutex::new(Slot {
                    taken: None,
                    running: false,
                }),
                arrived: Condvar::new(),
            }
        }

        /// The reading if it is fresh; otherwise the one a probe started now
        /// brings, waited for at most `wait`. A probe still running when the
        /// wait ends is left to finish — its answer is the next call's — and
        /// the reading handed back says it is still looking.
        pub(super) fn read<F>(&'static self, ttl: Duration, wait: Duration, probe: F) -> Reading
        where
            F: FnOnce() -> Reading + Send + 'static,
        {
            let began = Instant::now();
            // A reading taken since this call began is fresh for it whatever
            // the time to live: the call that waited for a probe gets that
            // probe's answer, even when the answer is already old by the time
            // the call wakes.
            let usable = |taken: &Taken| taken.at >= began || taken.at.elapsed() < ttl;
            let mut probe = Some(probe);
            let mut slot = held(&self.slot);
            loop {
                if let Some(taken) = slot.taken.as_ref().filter(|taken| usable(taken)) {
                    return taken.reading.clone();
                }
                if !slot.running
                    && let Some(probe) = probe.take()
                {
                    slot.running = true;
                    if let Err(error) = self.start(probe) {
                        slot.running = false;
                        return Err(format!("the probe could not start: {error}"));
                    }
                }
                let Some(left) = wait.checked_sub(began.elapsed()) else {
                    return Err(STILL_LOOKING.to_string());
                };
                let (waited, timeout) = self
                    .arrived
                    .wait_timeout(slot, left)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                slot = waited;
                if timeout.timed_out() {
                    return slot
                        .taken
                        .as_ref()
                        .filter(|taken| usable(taken))
                        .map_or_else(
                            || Err(STILL_LOOKING.to_string()),
                            |taken| taken.reading.clone(),
                        );
                }
            }
        }

        /// Run `probe` on a thread of its own and put what it finds in the
        /// slot. A probe that panics leaves a reason in the slot, not a slot
        /// that is running forever.
        fn start<F>(&'static self, probe: F) -> std::io::Result<()>
        where
            F: FnOnce() -> Reading + Send + 'static,
        {
            std::thread::Builder::new()
                .name("physical-devices-probe".to_string())
                .spawn(move || {
                    let reading = std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe))
                        .unwrap_or_else(|_| Err("the probe stopped unexpectedly".to_string()));
                    let mut slot = held(&self.slot);
                    slot.taken = Some(Taken {
                        at: Instant::now(),
                        reading,
                    });
                    slot.running = false;
                    drop(slot);
                    self.arrived.notify_all();
                })
                .map(drop)
        }

        /// Forget the reading, for a measurement that wants a cold start.
        #[cfg(test)]
        pub(super) fn forget(&self) {
            held(&self.slot).taken = None;
        }
    }
}

#[cfg(any(target_os = "macos", test))]
use super::session::held;
#[cfg(any(target_os = "macos", test))]
use kept::{Kept, Reading};

/// The iPhones and iPads Xcode's `devicectl` lists, run now.
#[cfg(any(target_os = "macos", test))]
fn ios_devices_with(mut command: std::process::Command, cap: Duration) -> Reading {
    const FAILED: &str = "Xcode's devicectl did not answer";
    let directory = super::scratch_directory();
    std::fs::create_dir_all(&directory).map_err(|error| format!("{FAILED}: {error}"))?;
    let file = directory.join(format!("devicectl-{}.json", uuid::Uuid::new_v4()));
    command
        .args(["list", "devices", "--quiet", "--timeout"])
        .arg(DEVICECTL_TIMEOUT_SECS.to_string())
        .arg("--json-output")
        .arg(&file);
    let written = super::process::run_bounded(command, cap, PROBE_OUTPUT_MAX_BYTES)
        .and_then(|out| out.ensure_success("devicectl"))
        .map_err(|error| format!("{FAILED}: {error}"))
        .and_then(|_| read_json(&file));
    let _ = std::fs::remove_file(&file);
    zerocode_core::agent_emulator::physical::parse_devicectl(&written?)
}

/// What `devicectl` wrote, bounded: a file is read whole only when it is small.
#[cfg(any(target_os = "macos", test))]
fn read_json(file: &std::path::Path) -> Result<String, String> {
    use std::io::Read as _;
    let mut text = String::new();
    std::fs::File::open(file)
        .map_err(|error| format!("devicectl wrote no answer file: {error}"))?
        .take(PROBE_OUTPUT_MAX_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("devicectl's answer could not be read: {error}"))?;
    if text.len() > PROBE_OUTPUT_MAX_BYTES {
        return Err("devicectl's answer is larger than this window reads".to_string());
    }
    Ok(text)
}

#[cfg(target_os = "macos")]
static IOS_DEVICES: Kept = Kept::new();

/// The real iPhones and iPads, from the kept reading or one probe.
#[cfg(target_os = "macos")]
async fn physical_ios() -> Reading {
    tauri::async_runtime::spawn_blocking(|| {
        IOS_DEVICES.read(PROBE_TTL, PROBE_WAIT, || {
            ios_devices_with(super::ios::devicectl_command(), PROBE_TOOL_CAP)
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// There is no Xcode off a Mac: no real iPhone to name, and no gap to report.
#[cfg(not(target_os = "macos"))]
async fn physical_ios() -> Result<Vec<PhysicalDevice>, String> {
    Ok(Vec::new())
}

/// The real Android phones `adb` lists, asked every time: it answers in
/// milliseconds, and a stale "none" is what a freshly plugged phone makes wrong.
async fn physical_android() -> Result<Vec<PhysicalDevice>, String> {
    tauri::async_runtime::spawn_blocking(|| super::android::physical_android_devices(PROBE_WAIT))
        .await
        .map_err(|error| error.to_string())?
}

/// What `zerocode-emulator list` answers, read now.
pub(crate) async fn list_answer_now() -> Result<Value, String> {
    let (simulators, emulators, ios_physical, android_physical) = tokio::join!(
        mobile_emulators_direct(),
        android_emulators_direct(),
        physical_ios(),
        physical_android(),
    );
    let to_rows = |rows: Result<Value, serde_json::Error>| rows.map_err(|error| error.to_string());
    list_answer(
        PlatformReading {
            platform: EmulatorPlatform::Ios,
            simulated: simulators.and_then(|found| to_rows(serde_json::to_value(found))),
            physical: ios_physical,
        },
        PlatformReading {
            platform: EmulatorPlatform::Android,
            simulated: emulators.and_then(|found| to_rows(serde_json::to_value(found))),
            physical: android_physical,
        },
    )
}

#[cfg(test)]
mod tests;
