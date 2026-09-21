//! Android SDK discovery, AVD lifecycle, streams and input commands.

mod accessibility;
mod capabilities;
mod display;
mod snapshots;

#[cfg(test)]
mod sdk_tests;

#[cfg(all(test, unix))]
mod geometry_tests;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::session::{FinishSession, SessionControl, SessionKey, StartClaim, registry};
use super::{
    BinaryChannel, EmitOutcome, EmulatorNoteCode, EmulatorPlatform, EmulatorStream, SlotWait,
    emit_bytes, emit_note, pump, read_bounded, wait_for_child,
};

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const ACTIVE_CAPTURE_INTERVAL: Duration = Duration::from_millis(120);
const IDLE_CAPTURE_INTERVAL: Duration = Duration::from_millis(900);

/// The ladder this pump climbs while a screen stays still.
///
/// Six rungs rather than iOS's eight, and a longer step, because every rung
/// here is a whole `adb screencap` — the same ladder, told once in [`pump`] and
/// given its own numbers, rather than a second copy written inline.
const IDLE_LADDER: pump::IdleLadder = pump::IdleLadder {
    step: ACTIVE_CAPTURE_INTERVAL,
    ceiling: IDLE_CAPTURE_INTERVAL,
    steps: 6,
};
const IDLE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const BOOT_TIMEOUT: Duration = Duration::from_secs(180);
const BOOT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const VIDEO_RESTART_INTERVAL: Duration = Duration::from_millis(100);
const VIDEO_TIME_LIMIT_SECONDS: &str = "180";
const VIDEO_MAX_DIMENSION: u32 = 1_280;
/// The bit rate the video stream is encoded at — scrcpy's own default, which
/// is what the original gets by passing no `video_bit_rate` at all.
/// `screenrecord` defaults to 20 Mbps instead, and 20 megabits of h264 for a
/// phone mirror is two and a half megabytes a second through a bridge that
/// also carries every keystroke, so the recorder is pinned to the same number.
const ANDROID_STREAM_BIT_RATE: &str = "8000000";
/// How long a mirror has to last before it counts as having stood up.
///
/// Starting one costs a push, a tunnel and a process — about a fifth of a
/// second. A server that falls over inside this is one that will fall over
/// again on the next try, and trying it again at once is a loop that spends
/// the pane's whole existence starting things. The recorder takes that turn
/// instead, and the turn after asks the mirror once more.
const SCRCPY_STOOD_UP: Duration = Duration::from_secs(1);
const MISSES_BEFORE_NOTE: u32 = 2;
/// The one spelling of what an emulator serial looks like.
const EMULATOR_SERIAL_PREFIX: &str = "emulator-";
/// How long a pump waits before asking `adb` again about a device that is not
/// on the bridge.
///
/// A pane whose emulator was killed under it used to spend its whole existence
/// starting things for a device that was gone: measured in the field on 1.1.11
/// (2026-09-21 20:09), 149 `scrcpy refused` lines in two minutes — 6.7 tries a
/// second, each one an `adb push` of a 70 KB jar, a `dumpsys input` and an
/// encoder spawn. Resting here instead costs one `adb get-state` a second
/// (11.7 ms median, measured on an idle API 35 emulator).
///
/// A second, and not longer, because this interval is also how long the pane
/// stays dark AFTER the device comes back: the pump can only notice a return
/// when it next looks. One second keeps the whole reattachment — look, push,
/// tunnel, first picture — inside the three the pane is given.
const DEVICE_ABSENT_REST: Duration = Duration::from_secs(1);
/// How long the video pump waits after a turn in which the device, though
/// `adb` lists it, gave back nothing at all.
///
/// Being on the bridge is not the same as being able to show a picture. A
/// restarting AVD answers `adb get-state` with `device` well before its
/// framework is up: measured across three kill-and-revive rounds on an API 35
/// emulator, 1.0 s, 13.5 s and 8.3 s passed between `adb` listing the serial
/// and `scrcpy` being able to open on it. 1.1.11 spent every one of those
/// seconds starting a mirror, a size read and an encoder six times over.
///
/// A turn that carried bytes is respawned on [`VIDEO_RESTART_INTERVAL`]
/// instead — that one is a recorder reaching its time limit, and the mirror
/// must not blink while it is replaced.
const DEVICE_WAKING_REST: Duration = Duration::from_secs(1);
const MANAGED_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(250);
const MANAGED_FILE_NAME: &str = "android-emulators.json";
const MANAGED_FILE_VERSION: u32 = 1;
const MANAGED_FILE_MAX_BYTES: u64 = 64 * 1024;
const MANAGED_DEVICE_MAX: usize = 32;
const MANAGED_PROPERTY: &str = "qemu.zerocode.managed";
/// How long a saved exit is waited for before the signal goes instead.
///
/// `adb emu kill` is the only exit that writes the AVD's `default_boot`
/// snapshot, and that snapshot is the whole of the next pane's first second
/// (D2). Writing it means flushing the guest's RAM to disk: measured at 6.3 s
/// for the first save of a Pixel 6 (670 MB) and 1.4–2.0 s for the saves after
/// it, load ~35. The limit is set several times that, because the cost of
/// waiting a moment too long is a slow exit and the cost of not waiting is the
/// next pane's cold boot — and a device still there when it passes gets the
/// signal it used to get.
const SNAPSHOT_SAVE_LIMIT: Duration = Duration::from_secs(30);
const SNAPSHOT_SAVE_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// The flag that turns off BOTH halves of quick boot, which this window has
/// not passed since D2.
///
/// It is read back off the running process rather than only kept out of the
/// launch, because a device this window ADOPTS was started by some earlier
/// window, and an emulator carrying this flag will never write the snapshot
/// the next pane resumes. What it cannot witness is the SDK launcher's own
/// decision: that binary re-execs with the arguments it was handed and builds
/// QEMU's options in-process — measured 09-21 on emulator 36.4.10, where the
/// verbose log's "QEMU options list" ran to ninety arguments while `ps` showed
/// the four the window passed. The launcher's decision is witnessed by what it
/// leaves in the AVD instead; see [`snapshots`]. It is a constant because the
/// launch itself must stay clean of the literal.
const NO_SNAPSHOT_FLAG: &str = "-no-snapshot";
/// The AVD files this window reads, and the ceiling it reads them under.
///
/// `<name>.ini` points at the AVD directory, `<name>.avd` is where it sits
/// when nothing points elsewhere, and `config.ini` inside it carries the
/// person's own boot preferences. All three are a few kilobytes of
/// `key = value`; the ceiling is there so a reader cannot be made to follow
/// whatever was renamed on top of one.
const AVD_DIRECTORY_SUFFIX: &str = ".avd";
const AVD_POINTER_SUFFIX: &str = ".ini";
const AVD_CONFIG_FILE: &str = "config.ini";
const AVD_PATH_KEY: &str = "path";
const AVD_INI_MAX_BYTES: u64 = 64 * 1024;
/// The `.android` root and the directory of AVDs inside it.
const DOT_ANDROID: &str = ".android";
const AVD_SUBDIRECTORY: &str = "avd";
/// The keys an AVD carries when a cold boot was asked for on purpose — the
/// person's own answer, and the first thing to name when the window is asking
/// why a quick boot did not happen.
const COLD_BOOT_KEYS: [(&str, &str); 2] = [
    ("fastboot.forceColdBoot", "yes"),
    ("fastboot.forceFastBoot", "no"),
];
/// How often the idle reclaimer looks at a fleet nobody has a pane on (D3).
const IDLE_RECLAIM_POLL_INTERVAL: Duration = Duration::from_secs(60);
/// How recently an AVD must have been streamed for the next window to put it
/// up before anybody asks (D4). A device nobody has opened in a week is a
/// device this window would be starting for nothing.
const PREBOOT_RECENT_DAYS: i64 = 7;
const MILLIS_PER_MINUTE: i64 = 60 * 1_000;
const MILLIS_PER_DAY: i64 = 24 * 60 * MILLIS_PER_MINUTE;

static MANAGED_STORE_GATE: Mutex<()> = Mutex::new(());
static MANAGED_SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedEmulatorRecord {
    token: String,
    avd: String,
    serial: Option<String>,
    pid: Option<u32>,
    started: Option<String>,
}

/// The AVD a pane last streamed, and when.
///
/// It outlives every device row: the rows say what is running NOW and are
/// dropped the moment their process is, while this is the only thing a window
/// booting on a machine where nothing is running can read to know which
/// device to put up (D4).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct LastUsedDevice {
    avd: String,
    at_ms: i64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ManagedEmulatorFile {
    version: u32,
    devices: Vec<ManagedEmulatorRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_used: Option<LastUsedDevice>,
}

struct ManagedEmulatorProcess {
    local_data_root: PathBuf,
    record: Mutex<ManagedEmulatorRecord>,
    child: Mutex<Option<std::process::Child>>,
    launching: AtomicBool,
}

impl ManagedEmulatorProcess {
    fn new_launch(local_data_root: PathBuf, avd: String, token: String) -> Arc<Self> {
        Arc::new(Self {
            local_data_root,
            record: Mutex::new(ManagedEmulatorRecord {
                token,
                avd,
                serial: None,
                pid: None,
                started: None,
            }),
            child: Mutex::new(None),
            launching: AtomicBool::new(true),
        })
    }

    fn adopted(local_data_root: PathBuf, record: ManagedEmulatorRecord) -> Arc<Self> {
        Arc::new(Self {
            local_data_root,
            record: Mutex::new(record),
            child: Mutex::new(None),
            launching: AtomicBool::new(false),
        })
    }

    fn record(&self) -> ManagedEmulatorRecord {
        held(&self.record).clone()
    }

    fn poll_child(&self) -> Result<Option<std::process::ExitStatus>, String> {
        let mut child = held(&self.child);
        let Some(running) = child.as_mut() else {
            return Err("Android 에뮬레이터 프로세스가 이미 회수됐습니다".to_string());
        };
        match running.try_wait() {
            Ok(Some(status)) => {
                child.take();
                Ok(Some(status))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                if let Some(mut failed) = child.take() {
                    let _ = failed.kill();
                    let _ = failed.wait();
                }
                Err(error.to_string())
            }
        }
    }

    fn poll_process(&self) -> Result<Option<std::process::ExitStatus>, String> {
        if held(&self.child).is_some() {
            return self.poll_child();
        }
        let record = self.record();
        match (record.pid, record.started.as_deref()) {
            (Some(pid), Some(started))
                if crate::resource_usage::process_start_identity(pid).as_deref() == Ok(started) =>
            {
                Ok(None)
            }
            _ => Err("Android 에뮬레이터 프로세스가 더 이상 실행 중이 아닙니다".to_string()),
        }
    }

    fn has_child(&self) -> bool {
        held(&self.child).is_some()
    }

    /// Whether the emulator this record names is still up.
    ///
    /// A live `Child` is the stronger authority, exactly as [`poll_process`]
    /// reads it; a device adopted from a previous window has only the pid and
    /// its start identity.
    ///
    /// [`poll_process`]: ManagedEmulatorProcess::poll_process
    fn is_running(&self) -> bool {
        matches!(self.poll_process(), Ok(None))
    }

    /// Ask the device to write its snapshot and quit.
    ///
    /// `adb emu kill` is the emulator's own exit: it saves `default_boot` on
    /// the way out, which is what turns the next pane's cold 25 s into a
    /// resume (D2). Everything here is best effort — no adb, no serial, or a
    /// device that does not go inside [`SNAPSHOT_SAVE_LIMIT`] all fall through
    /// to the signal the caller was already sending.
    fn ask_for_a_saved_exit(&self) {
        let Some(serial) = self.record().serial else {
            return;
        };
        let Ok(sdk) = android_sdk() else {
            return;
        };
        if crate::proc::quiet_command(&sdk.adb)
            .args(["-s", &serial, "emu", "kill"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let deadline = Instant::now() + SNAPSHOT_SAVE_LIMIT;
        while self.is_running() && Instant::now() < deadline {
            std::thread::sleep(SNAPSHOT_SAVE_POLL_INTERVAL);
        }
    }

    /// Put this emulator away — the one road every caller takes.
    ///
    /// The pane's power button, the window's exit and the idle reclaimer all
    /// arrive here, so the saved exit is asked for once rather than in three
    /// slightly different copies. `true` means the device is gone and the
    /// caller may forget its row.
    fn stop(&self) -> bool {
        self.launching.store(false, Ordering::Release);
        self.ask_for_a_saved_exit();
        if let Some(mut child) = held(&self.child).take() {
            let _ = child.kill();
            let _ = child.wait();
            return true;
        }
        let record = self.record();
        match (record.pid, record.started.clone()) {
            (Some(pid), Some(started)) => {
                if crate::resource_usage::process_start_identity(pid).as_deref()
                    != Ok(started.as_str())
                {
                    return true;
                }
                let marker = format!("{MANAGED_PROPERTY}={}", record.token);
                match crate::resource_usage::process_has_args(pid, &["-avd", &record.avd, &marker])
                {
                    Ok(true) => crate::resource_usage::terminate_process(pid, &started),
                    Ok(false) => true,
                    Err(_) => false,
                }
            }
            (None, None) => true,
            _ => false,
        }
    }
}

fn managed_devices() -> &'static Mutex<HashMap<String, Arc<ManagedEmulatorProcess>>> {
    static DEVICES: OnceLock<Mutex<HashMap<String, Arc<ManagedEmulatorProcess>>>> = OnceLock::new();
    DEVICES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn managed_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(MANAGED_FILE_NAME)
}

/// The shape an AVD name has to have before this window will put it on a
/// command line — the one judgement both the device rows and the last-used
/// row are read through.
fn valid_avd_name(avd: &str) -> bool {
    !avd.is_empty()
        && avd.len() <= 256
        && !avd
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

fn valid_managed_record(record: &ManagedEmulatorRecord) -> bool {
    record.token.len() == 48
        && record.token.bytes().all(|byte| byte.is_ascii_hexdigit())
        && valid_avd_name(&record.avd)
        && record.serial.as_ref().is_none_or(|serial| {
            serial.len() <= 64
                && serial.strip_prefix("emulator-").is_some_and(|port| {
                    !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
                })
        })
        && match (record.pid, record.started.as_deref()) {
            (Some(pid), Some(started)) => pid > 0 && !started.is_empty() && started.len() <= 128,
            (None, None) => true,
            _ => false,
        }
}

fn read_managed_file(local_data_root: &Path) -> Result<ManagedEmulatorFile, String> {
    let file = managed_file(local_data_root);
    let bytes = match crate::durable_file::read_plain_file(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedEmulatorFile {
                version: MANAGED_FILE_VERSION,
                ..ManagedEmulatorFile::default()
            });
        }
        Err(error) => return Err(error.to_string()),
    };
    if bytes.len() as u64 > MANAGED_FILE_MAX_BYTES {
        return Err("Android 에뮬레이터 소유권 파일이 올바르지 않습니다".to_string());
    }
    let mut parsed: ManagedEmulatorFile =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if parsed.version != MANAGED_FILE_VERSION || parsed.devices.len() > MANAGED_DEVICE_MAX {
        return Err("Android 에뮬레이터 소유권 파일 판번호가 올바르지 않습니다".to_string());
    }
    parsed.devices.retain(valid_managed_record);
    parsed.last_used = parsed
        .last_used
        .filter(|last| valid_avd_name(&last.avd) && last.at_ms > 0);
    Ok(parsed)
}

fn read_managed_records(local_data_root: &Path) -> Result<Vec<ManagedEmulatorRecord>, String> {
    read_managed_file(local_data_root).map(|file| file.devices)
}

/// Write this root's device rows together with the last-used device.
///
/// `last_used` of `None` keeps whatever the file already carries: the rows are
/// rewritten on every launch, stop and reconcile, and the last-used device is
/// a fact from BEFORE this window that none of those know anything about.
fn write_managed_file_locked(
    local_data_root: &Path,
    last_used: Option<LastUsedDevice>,
) -> Result<(), String> {
    let mut devices = held(managed_devices())
        .values()
        .filter(|process| process.local_data_root == local_data_root)
        .map(|process| process.record())
        .filter(valid_managed_record)
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.token.cmp(&right.token));
    devices.dedup_by(|left, right| left.token == right.token);
    if devices.len() > MANAGED_DEVICE_MAX {
        return Err("관리할 수 있는 Android 에뮬레이터가 너무 많습니다".to_string());
    }
    let last_used = last_used.or_else(|| read_managed_file(local_data_root).ok()?.last_used);
    let bytes = serde_json::to_vec(&ManagedEmulatorFile {
        version: MANAGED_FILE_VERSION,
        devices,
        last_used,
    })
    .map_err(|error| error.to_string())?;
    crate::durable_file::replace_bytes(&managed_file(local_data_root), &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn write_managed_records_locked(local_data_root: &Path) -> Result<(), String> {
    write_managed_file_locked(local_data_root, None)
}

fn write_managed_records(local_data_root: &Path) -> Result<(), String> {
    let _store = held(&MANAGED_STORE_GATE);
    write_managed_records_locked(local_data_root)
}

/// Remember the AVD a pane just opened, so the next window can put it up
/// before anybody asks (D4).
fn note_last_used_device(local_data_root: &Path, avd: &str, now_ms: i64) {
    if !valid_avd_name(avd) {
        return;
    }
    let _store = held(&MANAGED_STORE_GATE);
    let _ = write_managed_file_locked(
        local_data_root,
        Some(LastUsedDevice {
            avd: avd.to_string(),
            at_ms: now_ms,
        }),
    );
}

/// Whether a device last streamed at `at_ms` is recent enough to preboot.
///
/// Its own function because the window's boot is the one caller that cannot
/// be run twice to see what it decided: the answer is a device either quietly
/// starting or quietly not.
fn within_the_preboot_window(at_ms: i64, now_ms: i64) -> bool {
    (0..=PREBOOT_RECENT_DAYS * MILLIS_PER_DAY).contains(&(now_ms - at_ms))
}

/// The AVD this machine should have up before anybody opens a pane, or
/// nothing when the last one is older than [`PREBOOT_RECENT_DAYS`].
fn preboot_candidate(local_data_root: &Path, now_ms: i64) -> Option<String> {
    read_managed_file(local_data_root)
        .ok()?
        .last_used
        .filter(|last| within_the_preboot_window(last.at_ms, now_ms))
        .map(|last| last.avd)
}

fn forget_managed_process(process: &Arc<ManagedEmulatorProcess>) {
    let token = process.record().token;
    let removed = {
        let mut devices = held(managed_devices());
        if devices
            .get(&token)
            .is_some_and(|current| Arc::ptr_eq(current, process))
        {
            devices.remove(&token);
            true
        } else {
            false
        }
    };
    if removed {
        let _ = write_managed_records(&process.local_data_root);
    }
}

fn begin_managed_launch(
    local_data_root: &Path,
    avd: &str,
) -> Result<Arc<ManagedEmulatorProcess>, String> {
    if MANAGED_SHUTTING_DOWN.load(Ordering::Acquire) {
        return Err("앱이 종료되는 동안 에뮬레이터를 시작할 수 없습니다".to_string());
    }
    let token = crate::hooks::random_token().ok_or("에뮬레이터 소유권 id를 만들 수 없습니다")?;
    let process = ManagedEmulatorProcess::new_launch(
        local_data_root.to_path_buf(),
        avd.to_string(),
        token.clone(),
    );
    held(managed_devices()).insert(token, process.clone());
    if let Err(error) = write_managed_records(local_data_root) {
        forget_managed_process(&process);
        return Err(format!(
            "Android 에뮬레이터 소유권을 기록할 수 없습니다: {error}"
        ));
    }
    Ok(process)
}

fn attach_managed_child(
    process: &Arc<ManagedEmulatorProcess>,
    mut child: std::process::Child,
) -> Result<(), String> {
    let pid = child.id();
    let started = match crate::resource_usage::process_start_identity(pid) {
        Ok(started) => started,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            forget_managed_process(process);
            return Err(format!(
                "Android 에뮬레이터 시작 식별자를 읽을 수 없습니다: {error}"
            ));
        }
    };
    {
        let mut record = held(&process.record);
        record.pid = Some(pid);
        record.started = Some(started);
    }
    *held(&process.child) = Some(child);
    process.launching.store(false, Ordering::Release);
    if MANAGED_SHUTTING_DOWN.load(Ordering::Acquire) {
        process.stop();
        forget_managed_process(process);
        return Err("앱이 종료되는 동안 에뮬레이터 시작이 취소됐습니다".to_string());
    }
    if let Err(error) = write_managed_records(&process.local_data_root) {
        process.stop();
        forget_managed_process(process);
        return Err(format!(
            "Android 에뮬레이터 소유권을 기록할 수 없습니다: {error}"
        ));
    }
    Ok(())
}

fn watch_managed_process(process: &Arc<ManagedEmulatorProcess>) -> Result<(), String> {
    let watched = process.clone();
    let name = process.record().avd;
    std::thread::Builder::new()
        .name(format!("android-avd-{name}"))
        .spawn(move || {
            // `Ok(None)` is the only answer that means "still running", so the
            // poll itself is the loop's condition. An exit (`Ok(Some(_))`) and
            // a reaper that can no longer ask (`Err(_)`) both end the watch the
            // same way: the record is forgotten below either way.
            while let Ok(None) = watched.poll_child() {
                std::thread::sleep(MANAGED_PROCESS_POLL_INTERVAL);
            }
            forget_managed_process(&watched);
        })
        .map(|_| ())
        .map_err(|error| format!("Android 에뮬레이터 회수기를 시작할 수 없습니다: {error}"))
}

fn stop_managed_device(serial: &str) {
    let processes = held(managed_devices())
        .values()
        .filter(|process| process.record().serial.as_deref() == Some(serial))
        .cloned()
        .collect::<Vec<_>>();
    for process in processes {
        if process.stop() {
            forget_managed_process(&process);
        }
    }
}

fn managed_process_for_avd(
    local_data_root: &Path,
    avd: &str,
) -> Option<Arc<ManagedEmulatorProcess>> {
    held(managed_devices())
        .values()
        .find(|process| {
            if process.local_data_root != local_data_root {
                return false;
            }
            let record = process.record();
            record.avd == avd && record.pid.is_some() && record.started.is_some()
        })
        .cloned()
}

fn managed_processes() -> Vec<Arc<ManagedEmulatorProcess>> {
    held(managed_devices()).values().cloned().collect()
}

/// Put away every emulator this window is holding.
///
/// `keep_booted` is the person's `emulator.keepBooted` (D3): the devices stay
/// up and their rows stay on disk, so the NEXT window adopts them through the
/// road that already exists instead of paying a cold boot. Launches stop
/// either way — a window on its way out has nobody to hand a new device to.
pub(super) fn shutdown_all_devices(keep_booted: bool) {
    MANAGED_SHUTTING_DOWN.store(true, Ordering::Release);
    if keep_booted {
        return;
    }
    for process in managed_processes() {
        if process.stop() {
            forget_managed_process(&process);
        }
    }
}

/// Whether a fleet nobody has a pane on has been idle long enough to put away.
///
/// `last_pane_closed_ms` is `None` while a pane is open, which is why this is
/// the whole judgement rather than half of it: "no panes" and "no panes for
/// long enough" are different facts, and only the second one turns a device
/// off. `minutes` of zero is the person saying never.
fn idle_shutdown_is_due(last_pane_closed_ms: Option<i64>, now_ms: i64, minutes: u32) -> bool {
    let Some(closed) = last_pane_closed_ms else {
        return false;
    };
    minutes > 0 && now_ms.saturating_sub(closed) >= i64::from(minutes) * MILLIS_PER_MINUTE
}

/// Turn off the devices this window is holding, through [`ManagedEmulatorProcess::stop`]
/// — the same saved exit the pane's power button takes.
fn reclaim_idle_devices(local_data_root: &Path) -> usize {
    let mut put_away = 0;
    for process in managed_processes() {
        if process.local_data_root != local_data_root {
            continue;
        }
        if process.stop() {
            forget_managed_process(&process);
            put_away += 1;
        }
    }
    put_away
}

/// Watch for a fleet nobody is looking at (D3).
///
/// Devices are kept booted so the next pane opens on a resume, and a device
/// kept booted forever is a phone's worth of RAM nobody asked for — so the
/// reclaimer is the other half of `emulator.keepBooted`, not an extra. It
/// reads the setting each time it has something to decide rather than holding
/// a copy, because the person may turn it off while it sleeps.
pub(super) fn arm_idle_reclaim(app: &AppHandle) {
    static ARMED: std::sync::Once = std::sync::Once::new();
    let app = app.clone();
    ARMED.call_once(move || {
        let _ = std::thread::Builder::new()
            .name("android-emulator-idle".to_string())
            .spawn(move || {
                let mut idle_since: Option<i64> = None;
                loop {
                    std::thread::sleep(IDLE_RECLAIM_POLL_INTERVAL);
                    let local_data_root = app.state::<crate::AppState>().local_data_root().to_path_buf();
                    let mine = managed_processes()
                        .into_iter()
                        .filter(|process| process.local_data_root == local_data_root)
                        .count();
                    if mine == 0 || registry().live_count(EmulatorPlatform::Android) > 0 {
                        idle_since = None;
                        continue;
                    }
                    let now_ms = crate::now_epoch_ms();
                    let closed = *idle_since.get_or_insert(now_ms);
                    let minutes = crate::load_settings_resilient(
                        app.state::<crate::AppState>().settings(),
                    )
                    .document
                    .emulator_idle_shutdown_minutes;
                    if !idle_shutdown_is_due(Some(closed), now_ms, minutes) {
                        continue;
                    }
                    idle_since = None;
                    let put_away = reclaim_idle_devices(&local_data_root);
                    if put_away > 0 {
                        crate::note_window_event(
                            &local_data_root,
                            &format!(
                                "emulator android idle reclaim: {put_away} device(s) after {minutes}m"
                            ),
                        );
                    }
                }
            });
    });
}

/// Put the last-used AVD up before anybody asks for it (D4).
///
/// Only the device: no stream, no pane, no window event beyond the one line
/// that says it happened. A pane opened while this is still booting finds the
/// tokenized launch through [`managed_process_for_avd`] and waits for it
/// rather than starting the same AVD twice.
pub(super) fn preboot_last_used(app: &AppHandle) {
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("android-emulator-preboot".to_string())
        .spawn(move || {
            let local_data_root = app
                .state::<crate::AppState>()
                .local_data_root()
                .to_path_buf();
            reconcile_managed_devices_now(&local_data_root);
            let Some(avd) = preboot_candidate(&local_data_root, crate::now_epoch_ms()) else {
                return;
            };
            let Ok(sdk) = android_sdk() else {
                return;
            };
            let Ok(devices) = list_android_devices() else {
                return;
            };
            let Some(chosen) = devices.into_iter().find(|device| device.avd == avd) else {
                return;
            };
            if chosen.serial.is_some() {
                return;
            }
            let outcome = match boot_android_device(&app, &sdk, &chosen) {
                Ok(serial) => format!("serial {serial}"),
                Err(error) => format!("failed: {error}"),
            };
            crate::note_window_event(
                &local_data_root,
                &format!("emulator android preboot avd {avd} {outcome}"),
            );
        });
}

/// Where one tool lives inside an SDK: the binary's name and the package
/// directory that holds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct AndroidTool {
    binary: &'static str,
    package: &'static str,
}

/// Every road to a device — frames, input, the accessibility tree — is this
/// one binary, so nothing that talks to a running device needs anything else.
const ADB: AndroidTool = AndroidTool {
    binary: "adb",
    package: "platform-tools",
};

/// Only the AVD list and a cold boot need this one.
const EMULATOR: AndroidTool = AndroidTool {
    binary: "emulator",
    package: "emulator",
};

/// One row of the search table, in the order it is walked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SdkSource {
    Path,
    SdkRoot,
    AndroidHome,
    HostDefault,
}

impl SdkSource {
    /// The table, top to bottom: the binary the person's own shell would run,
    /// then the two roots a toolchain sets, then where the installer put it.
    ///
    /// A window opened from the Dock inherits `PATH=/usr/bin:/bin:/usr/sbin:/sbin`
    /// and no `ANDROID_*` at all (trap 307), so on that machine every row but
    /// the last is empty — which is why the last row exists.
    const TABLE: [Self; 4] = [
        Self::Path,
        Self::SdkRoot,
        Self::AndroidHome,
        Self::HostDefault,
    ];

    /// How this row is named back to the person when nothing held the binary.
    fn label(self) -> &'static str {
        match self {
            Self::Path => "$PATH",
            Self::SdkRoot => "$ANDROID_SDK_ROOT",
            Self::AndroidHome => "$ANDROID_HOME",
            Self::HostDefault => "기본 SDK 자리",
        }
    }
}

/// What the resolver may read. The process environment is read in
/// [`SdkEnvironment::current`] alone, so a test hands it a fake home rather
/// than writing globals every other test in this binary shares.
#[derive(Clone, Debug)]
struct SdkEnvironment {
    path: Option<OsString>,
    sdk_root: Option<OsString>,
    android_home: Option<OsString>,
    /// The three that name where AVDs live rather than where the SDK does.
    /// They are read here for the same reason as the rest: so a test hands
    /// over a fake home instead of writing globals this whole binary shares.
    avd_home: Option<OsString>,
    android_user_home: Option<OsString>,
    sdk_home: Option<OsString>,
    home: Option<PathBuf>,
}

impl SdkEnvironment {
    fn current() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            sdk_root: std::env::var_os("ANDROID_SDK_ROOT"),
            android_home: std::env::var_os("ANDROID_HOME"),
            avd_home: std::env::var_os("ANDROID_AVD_HOME"),
            android_user_home: std::env::var_os("ANDROID_USER_HOME"),
            sdk_home: std::env::var_os("ANDROID_SDK_HOME"),
            home: dirs::home_dir(),
        }
    }

    /// The directories this row offers, in the order they are tried. A row
    /// with nothing configured offers none, and says so in the reason.
    fn roads(&self, source: SdkSource) -> Vec<PathBuf> {
        match source {
            SdkSource::Path => self
                .path
                .iter()
                .flat_map(|path| std::env::split_paths(path))
                .collect(),
            SdkSource::SdkRoot => configured_root(self.sdk_root.as_ref())
                .into_iter()
                .collect(),
            SdkSource::AndroidHome => configured_root(self.android_home.as_ref())
                .into_iter()
                .collect(),
            SdkSource::HostDefault => self
                .home
                .iter()
                .map(|home| host_default_sdk_root(home))
                .collect(),
        }
    }
}

/// The files a tool would sit in on one road, in the order they are tried.
///
/// Every row but `$PATH` names an SDK root the tool lives under. A `$PATH`
/// entry is one package's own directory, so the tool is either in it or in
/// its sibling package under the same root — a shell with `platform-tools`
/// on its path still reaches the emulator beside it.
fn files_in(source: SdkSource, road: &Path, tool: AndroidTool) -> Vec<PathBuf> {
    let file = executable(tool.binary);
    match source {
        SdkSource::Path => std::iter::once(road.join(&file))
            .chain(
                road.parent()
                    .map(|root| root.join(tool.package).join(&file)),
            )
            .collect(),
        _ => vec![road.join(tool.package).join(file)],
    }
}

/// A configured root is walked only when it is absolute: a relative one would
/// be joined against whatever directory the window happened to start in.
fn configured_root(value: Option<&OsString>) -> Option<PathBuf> {
    let root = PathBuf::from(value?);
    root.is_absolute().then_some(root)
}

/// Where this platform's installer puts the SDK, under the person's home.
fn host_default_sdk_root(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    return home.join("Library/Android/sdk");
    #[cfg(target_os = "windows")]
    return home.join("AppData/Local/Android/Sdk");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return home.join("Android/Sdk");
}

/// The tool that was not found, and every road walked looking for it. It is
/// the error itself, so whoever refuses the person's action says where to put
/// the SDK rather than only that it is missing.
#[derive(Clone, Debug)]
struct SdkSearch {
    tool: AndroidTool,
    looked: Vec<(SdkSource, Vec<PathBuf>)>,
}

impl std::fmt::Display for SdkSearch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let roads = self
            .looked
            .iter()
            .map(|(source, roads)| {
                if roads.is_empty() {
                    format!("{}(설정 안 됨)", source.label())
                } else {
                    let walked = roads
                        .iter()
                        .map(|road| road.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}({walked})", source.label())
                }
            })
            .collect::<Vec<_>>()
            .join("; ");
        write!(
            formatter,
            "Android SDK의 {}를 찾지 못했습니다 — 본 자리: {roads}",
            self.tool.binary
        )
    }
}

/// The SDK root a located binary implies: `<root>/<package>/<binary>`.
fn sdk_root_of(binary: &Path) -> PathBuf {
    binary
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf()
}

#[derive(Clone, Debug)]
struct AndroidSdk {
    root: PathBuf,
    adb: PathBuf,
    /// The AVD list and a cold boot need this; nothing else does. A machine
    /// without the package keeps the reason it would give, so the roads that
    /// only need `adb` still run against a device that is already up.
    emulator: Result<PathBuf, SdkSearch>,
}

fn executable(name: &str) -> OsString {
    let mut name = OsString::from(name);
    name.push(std::env::consts::EXE_SUFFIX);
    name
}

/// The file this tool lives in, or every road that did not hold it.
fn locate(environment: &SdkEnvironment, tool: AndroidTool) -> Result<PathBuf, SdkSearch> {
    let mut looked = Vec::with_capacity(SdkSource::TABLE.len());
    for source in SdkSource::TABLE {
        let roads = environment.roads(source);
        if let Some(found) = roads
            .iter()
            .flat_map(|road| files_in(source, road, tool))
            .find(|candidate| candidate.is_file())
        {
            return Ok(found);
        }
        looked.push((source, roads));
    }
    Err(SdkSearch { tool, looked })
}

/// `adb` alone decides whether this machine has an SDK: it is the road every
/// frame, tap and tree takes. A missing `emulator` package only costs the AVD
/// list and a cold boot, so it is carried as its own reason rather than
/// failing the resolve — a window that could screenshot a live device but not
/// touch it was exactly that asymmetry (t-5446).
fn resolve_android_sdk(environment: &SdkEnvironment) -> Result<AndroidSdk, SdkSearch> {
    let adb = locate(environment, ADB)?;
    let emulator = locate(environment, EMULATOR);
    let root = sdk_root_of(emulator.as_deref().unwrap_or(&adb));
    Ok(AndroidSdk {
        root,
        adb,
        emulator,
    })
}

fn android_sdk() -> Result<AndroidSdk, SdkSearch> {
    resolve_android_sdk(&SdkEnvironment::current())
}

/// Every directory AVDs could live in, in the order the emulator itself walks
/// them: `$ANDROID_AVD_HOME` outright, then the `avd` inside the current and
/// the legacy names for the `.android` root, then the one under the person's
/// home that everything falls back to.
///
/// A row with nothing configured offers nothing, exactly as the SDK table
/// above does — and on the window's own machine only the last row is filled,
/// because a window opened from the Dock inherits no `ANDROID_*` at all
/// (trap 307).
fn avd_homes(environment: &SdkEnvironment) -> Vec<PathBuf> {
    [
        configured_root(environment.avd_home.as_ref()),
        configured_root(environment.android_user_home.as_ref())
            .map(|root| root.join(AVD_SUBDIRECTORY)),
        configured_root(environment.sdk_home.as_ref())
            .map(|root| root.join(DOT_ANDROID).join(AVD_SUBDIRECTORY)),
        environment
            .home
            .as_ref()
            .map(|home| home.join(DOT_ANDROID).join(AVD_SUBDIRECTORY)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The directory one AVD's files sit in: the absolute `path=` its pointer
/// names, or the `<name>.avd` beside that pointer when there is none.
///
/// `None` means this machine has no such AVD on disk — which is not an error
/// anywhere it is asked. Nothing here refuses a launch; it only decides
/// whether there is a snapshot to look at.
fn avd_root(environment: &SdkEnvironment, avd: &str) -> Option<PathBuf> {
    if !valid_avd_name(avd) {
        return None;
    }
    avd_homes(environment).into_iter().find_map(|home| {
        let pointed = ini_values(
            &home.join(format!("{avd}{AVD_POINTER_SUFFIX}")),
            &[AVD_PATH_KEY],
        )
        .into_iter()
        .next()
        .map(|(_, path)| PathBuf::from(path))
        .filter(|path| path.is_absolute());
        pointed
            .into_iter()
            .chain(std::iter::once(
                home.join(format!("{avd}{AVD_DIRECTORY_SUFFIX}")),
            ))
            .find(|root| root.is_dir())
    })
}

/// The values these keys carry in one AVD ini file.
///
/// `<name>.ini`, `config.ini` and `emulator-user.ini` are all the same
/// `key = value` shape, so they are read through one reader rather than three.
/// Keys are answered in the order they were asked for, and a key the file does
/// not carry is simply absent.
fn ini_values(path: &Path, keys: &[&str]) -> Vec<(String, String)> {
    let readable =
        std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() <= AVD_INI_MAX_BYTES);
    if !readable {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if keys.contains(&key) && !found.iter().any(|(seen, _): &(String, _)| seen == key) {
            found.push((key.to_string(), value.to_string()));
        }
    }
    found.sort_by_key(|(key, _)| keys.iter().position(|wanted| wanted == key));
    found
}

/// Today, locally, as `YYYY-MM-DD` — what a moved-aside snapshot is named
/// after. The offset comes from the window's one local clock rather than a
/// second reading of the platform.
fn today_local() -> String {
    let offset_minutes =
        i32::try_from(crate::automation_runtime::local_offset_secs() / 60).unwrap_or_default();
    zerocode_core::civil::iso_date_of(crate::now_epoch_ms(), offset_minutes)
}

/// Move a snapshot the emulator has already refused out of its way, before
/// this launch meets it too (t-5762).
///
/// This is not a tidy-up for disk's sake. Such a record SURVIVES every exit
/// that does not save — a crash, a force quit, an exit past
/// [`SNAPSHOT_SAVE_LIMIT`], the SIGKILL this window sent before D2 — so an
/// AVD that once failed a load pays the cold boot on every launch until
/// something replaces it. `beat_sweep_w012` carried its August one until
/// 09-21, which is exactly what D2 was supposed to have ended. Clearing it
/// here means the launch after this one starts from a snapshot or from
/// nothing, never from a refusal, and the window log says which.
fn clear_a_stale_snapshot(local_data_root: &Path, avd: &str) {
    let Some(root) = avd_root(&SdkEnvironment::current(), avd) else {
        return;
    };
    if let Some(note) = snapshots::tidy(&root, &today_local()) {
        crate::note_window_event(local_data_root, &note);
    }
}

/// The witness for a quick boot that did not happen anyway (t-5762).
///
/// Both rows are read once the device is up, because neither is knowable
/// before it, and nothing is changed here — the boot has already been paid
/// for. The line exists so the next reader of `window-errors.log` starts from
/// the reason rather than from "the pane was slow again".
///
/// * The AVD's `default_boot` as it stands NOW. The launch moved a refused one
///   aside before the launcher looked, so one that is refused HERE was written
///   during this very boot: a failed load rewrites `snapshot.pb` into an
///   11-byte record carrying only a reason code. That is the launcher's own
///   decision, in the only place it is legible.
/// * [`NO_SNAPSHOT_FLAG`] on the process itself — which only ever means the
///   process was GIVEN it, so on an adopted device it names the older window
///   that started it, and on one we started it is a regression in this file.
fn note_a_refused_snapshot(local_data_root: &Path, avd: &str, pid: Option<u32>) {
    let mut reasons = Vec::new();
    let root = avd_root(&SdkEnvironment::current(), avd);
    if let Some(root) = &root
        && let snapshots::Verdict::Broken(reason) =
            snapshots::inspect(&snapshots::default_boot_of(root))
    {
        reasons.push(format!(
            "the emulator refused its own snapshot during this boot and recorded it: {reason}"
        ));
    }
    if pid.is_some_and(|pid| {
        crate::resource_usage::process_has_args(pid, &[NO_SNAPSHOT_FLAG]) == Ok(true)
    }) {
        reasons.push(format!(
            "the running emulator carries {NO_SNAPSHOT_FLAG}, so it will not write one either"
        ));
    }
    if reasons.is_empty() {
        return;
    }
    // Only now, because these cost a file read each and say nothing unless
    // something above already went wrong.
    if let Some(root) = &root {
        reasons.extend(
            ini_values(
                &root.join(AVD_CONFIG_FILE),
                &COLD_BOOT_KEYS.map(|(key, _)| key),
            )
            .into_iter()
            .filter(|(key, value)| {
                COLD_BOOT_KEYS
                    .iter()
                    .any(|(cold, asked)| cold == key && asked.eq_ignore_ascii_case(value))
            })
            .map(|(key, value)| format!("{AVD_CONFIG_FILE} asks for a cold boot: {key}={value}")),
        );
    }
    crate::note_window_event(
        local_data_root,
        &format!(
            "emulator android quick boot did not happen: avd {avd} pid {} — {}",
            pid.map_or_else(|| "unknown".to_string(), |pid| pid.to_string()),
            reasons.join("; ")
        ),
    );
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AndroidDevice {
    avd: String,
    serial: Option<String>,
    booted: bool,
}

fn android_running(adb: &Path) -> Vec<(String, String)> {
    let Ok(out) = crate::proc::quiet_command(adb)
        .args(["devices", "-l"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let serial = columns.next()?;
            let state = columns.next()?;
            if state != "device" || !serial.starts_with(EMULATOR_SERIAL_PREFIX) {
                return None;
            }
            let avd = android_avd_name(adb, serial).unwrap_or_default();
            Some((serial.to_string(), avd))
        })
        .collect()
}

fn android_avd_name(adb: &Path, serial: &str) -> Result<String, String> {
    let bytes = accessibility::run(
        adb,
        &["-s", serial, "emu", "avd", "name"],
        Some(accessibility::MAX_OUTPUT_BYTES),
    )?;
    let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    let mut names = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "OK");
    let name = names.next().ok_or("Android AVD identity is unavailable")?;
    if names.next().is_some() {
        return Err("Android AVD identity is ambiguous".into());
    }
    Ok(name.to_string())
}

fn recovered_managed_record(
    mut record: ManagedEmulatorRecord,
    sample: &crate::resource_usage::ProcessSample,
) -> Option<ManagedEmulatorRecord> {
    let marker = format!("{MANAGED_PROPERTY}={}", record.token);
    let matching_launch = || sample.processes_with_args(&["-avd", &record.avd, &marker]);
    match (record.pid, record.started.as_deref()) {
        (Some(pid), Some(started))
            if sample.matches_start(pid, started)
                && matching_launch().iter().any(|found| found.pid == pid) =>
        {
            Some(record)
        }
        (None, None) => {
            let mut found = matching_launch();
            if found.len() != 1 {
                return None;
            }
            let found = found.pop().expect("one recovered emulator process");
            record.pid = Some(found.pid);
            record.started = Some(found.started);
            Some(record)
        }
        _ => None,
    }
}

fn reconcile_managed_records(
    records: Vec<ManagedEmulatorRecord>,
    runtime_owned: &HashSet<String>,
    sample: &crate::resource_usage::ProcessSample,
    running: Option<&[(String, String)]>,
) -> Vec<ManagedEmulatorRecord> {
    let mut kept = records
        .into_iter()
        .filter(valid_managed_record)
        .filter_map(|mut record| {
            // A live Child handle is stronger authority than a process sample
            // captured a moment before `spawn`. Keeping it closes the race
            // between resource sampling and a launch in this runtime.
            if runtime_owned.contains(&record.token) {
                return Some(record);
            }
            record = recovered_managed_record(record, sample)?;
            if let Some(running) = running {
                record.serial = running
                    .iter()
                    .find(|(_, avd)| avd == &record.avd)
                    .map(|(serial, _)| serial.clone());
            }
            Some(record)
        })
        .collect::<Vec<_>>();
    kept.sort_by(|left, right| left.token.cmp(&right.token));
    kept.dedup_by(|left, right| left.token == right.token);
    kept
}

fn reconcile_managed_devices(
    local_data_root: &Path,
    sample: &crate::resource_usage::ProcessSample,
    running: Option<&[(String, String)]>,
) -> Result<Vec<crate::resource_usage::NativeProcessRoot>, String> {
    let _store = held(&MANAGED_STORE_GATE);
    let had_file = managed_file(local_data_root).is_file();
    let mut records = read_managed_records(local_data_root)?
        .into_iter()
        .map(|record| (record.token.clone(), record))
        .collect::<HashMap<_, _>>();
    let current = held(managed_devices())
        .values()
        .filter(|process| process.local_data_root == local_data_root)
        .cloned()
        .collect::<Vec<_>>();
    let runtime_owned = current
        .iter()
        .filter(|process| process.launching.load(Ordering::Acquire) || process.has_child())
        .map(|process| process.record().token)
        .collect::<HashSet<_>>();
    for process in &current {
        let record = process.record();
        records.insert(record.token.clone(), record);
    }
    let records = reconcile_managed_records(
        records.into_values().collect(),
        &runtime_owned,
        sample,
        running,
    );
    let kept = records
        .iter()
        .map(|record| record.token.clone())
        .collect::<HashSet<_>>();
    {
        let mut devices = held(managed_devices());
        devices.retain(|token, process| {
            process.local_data_root != local_data_root || kept.contains(token)
        });
        for record in &records {
            if let Some(process) = devices.get(&record.token) {
                *held(&process.record) = record.clone();
            } else {
                devices.insert(
                    record.token.clone(),
                    ManagedEmulatorProcess::adopted(local_data_root.to_path_buf(), record.clone()),
                );
            }
        }
    }
    if had_file || !records.is_empty() {
        write_managed_records_locked(local_data_root)?;
    }
    Ok(records
        .into_iter()
        .filter_map(|record| {
            let key = record.serial.as_ref().map_or_else(
                || format!("android-avd:{}", record.avd),
                |serial| format!("android:{serial}"),
            );
            Some(crate::resource_usage::NativeProcessRoot {
                key,
                label: format!("Android · {}", record.avd),
                pid: record.pid?,
                started: record.started?,
            })
        })
        .collect())
}

pub(crate) fn managed_resource_processes(
    local_data_root: &Path,
    sample: &crate::resource_usage::ProcessSample,
) -> Vec<crate::resource_usage::NativeProcessRoot> {
    match reconcile_managed_devices(local_data_root, sample, None) {
        Ok(roots) => roots,
        Err(error) => {
            eprintln!("zerocode-shell: Android emulator reconciliation failed: {error}");
            Vec::new()
        }
    }
}

fn reconcile_managed_devices_now(local_data_root: &Path) {
    if let Ok(sample) = crate::resource_usage::enumerate_processes() {
        let running = android_sdk()
            .map(|sdk| android_running(&sdk.adb))
            .unwrap_or_default();
        let _ = reconcile_managed_devices(local_data_root, &sample, Some(&running));
    }
}

/// Every AVD this machine can show: the ones already up, then the ones the
/// emulator package knows how to start.
///
/// `adb` alone answers the first half, so a device that is already running is
/// listed — and therefore mirrored, tapped and read — on a machine whose SDK
/// has no emulator package at all. Only when that leaves nothing does the
/// missing package become the answer.
fn list_android_devices() -> Result<Vec<AndroidDevice>, String> {
    let sdk = android_sdk().map_err(|search| search.to_string())?;
    let running = android_running(&sdk.adb);
    let mut devices = running
        .iter()
        .filter(|(_, avd)| !avd.is_empty())
        .map(|(serial, avd)| AndroidDevice {
            avd: avd.clone(),
            serial: Some(serial.clone()),
            booted: true,
        })
        .collect::<Vec<_>>();
    let emulator = match &sdk.emulator {
        Ok(emulator) => emulator,
        Err(search) if devices.is_empty() => return Err(search.to_string()),
        Err(_) => return Ok(devices),
    };
    let listed = crate::proc::quiet_command(emulator)
        .arg("-list-avds")
        .output();
    let out = match listed {
        Ok(out) if out.status.success() => out,
        // Listing is the only thing this binary is asked for here. While
        // devices are already up its failure costs nothing; when there are
        // none it is the whole answer, and answering "no AVDs" instead would
        // hide it — one sentence covering two different causes is what sent
        // t-5446 looking at the wrong one.
        answer if devices.is_empty() => {
            let detail = match answer {
                Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
                Err(error) => error.to_string(),
            };
            return Err(format!(
                "{} -list-avds가 실패했습니다: {detail}",
                emulator.display()
            ));
        }
        _ => return Ok(devices),
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let avd = line.trim();
        let prefix = avd.split_whitespace().next();
        if avd.is_empty()
            || avd.starts_with("No AVD")
            || matches!(
                prefix,
                Some("INFO" | "WARNING" | "ERROR" | "DEBUG" | "VERBOSE" | "PANIC")
            )
            || devices.iter().any(|device| device.avd == avd)
        {
            continue;
        }
        let serial = running
            .iter()
            .find(|(_, name)| name == avd)
            .map(|(serial, _)| serial.clone());
        devices.push(AndroidDevice {
            avd: avd.to_string(),
            booted: serial.is_some(),
            serial,
        });
    }
    Ok(devices)
}

/// Whether one serial is on the bridge — the single absence verdict this
/// module has.
///
/// The mirror, the recorder and every input door read it, because "is the
/// device there" is one question and three answers to it is how a pane ends up
/// pushing a jar at a serial `adb` has not had for two minutes while the
/// person is told their tap failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DevicePresence {
    /// `adb` lists it as `device`: it can be pushed to, recorded and typed at.
    OnTheBridge,
    /// Gone from the list, or listed in a state that refuses work — `offline`
    /// while it boots or dies, `unauthorized`, `bootloader`.
    Gone,
}

/// Read the bridge once, for one serial.
///
/// `adb get-state` and not `adb devices -l`: it answers the same question
/// about the serial actually being asked about, in one process that never
/// reaches the device's shell — 11.7 ms median against a live emulator, and
/// the same 10–11 ms when the device is gone, because the adb SERVER refuses
/// it. The listing road costs `devices -l` plus an `emu avd name` per device
/// (15.1 + 16.5 ms each, measured) to learn names nobody asked for.
fn device_presence(adb: &Path, serial: &str) -> DevicePresence {
    // Only emulators are ever streamed, tapped or shut down here, and the
    // listing road has always enforced that. `get-state` would happily answer
    // for a phone on a cable, so the rule travels with the verdict.
    if !serial.starts_with(EMULATOR_SERIAL_PREFIX) {
        return DevicePresence::Gone;
    }
    let said = crate::proc::quiet_command(adb)
        .args(["-s", serial, "get-state"])
        .output()
        .ok()
        .filter(|answer| answer.status.success())
        .map(|answer| String::from_utf8_lossy(&answer.stdout).trim().to_string());
    presence_of(said.as_deref())
}

/// What one `adb get-state` answer means. `None` is the command itself having
/// refused, which is what a serial no longer on the bridge produces.
fn presence_of(said: Option<&str>) -> DevicePresence {
    match said {
        Some("device") => DevicePresence::OnTheBridge,
        _ => DevicePresence::Gone,
    }
}

/// One pump's memory of a device that went away.
///
/// The pane's note and the black box's line are written ONCE per absence
/// rather than once per look, and the return is written with how long it
/// lasted. That is the difference between a log naming an event and a log
/// filling with 149 copies of it.
///
/// It also publishes each reading on the session, where the input door reads
/// it for nothing — a tap turned away in the pane's own words costs no process
/// at all while the device is healthy.
struct AbsenceWatch {
    app: AppHandle,
    stream: String,
    husk_root: PathBuf,
    control: Arc<SessionControl>,
    serial: String,
    gone_since: Option<Instant>,
}

impl AbsenceWatch {
    fn new(
        app: &AppHandle,
        stream: &str,
        husk_root: &Path,
        control: &Arc<SessionControl>,
        serial: &str,
    ) -> Self {
        Self {
            app: app.clone(),
            stream: stream.to_string(),
            husk_root: husk_root.to_path_buf(),
            control: Arc::clone(control),
            serial: serial.to_string(),
            gone_since: None,
        }
    }

    /// Is this watch already holding an absence? A field, not a process —
    /// the road that photographs a device every 120 ms asks this first and
    /// only pays `adb` once it has reason to.
    const fn is_gone(&self) -> bool {
        self.gone_since.is_some()
    }

    /// Ask `adb` itself. `true` means the device is not there — rest, and try
    /// nothing at it.
    fn absent(&mut self, adb: &Path) -> bool {
        let presence = device_presence(adb, &self.serial);
        self.saw(presence);
        presence == DevicePresence::Gone
    }

    /// A picture arrived, which only a live device produces.
    ///
    /// Cheaper evidence than a process, and the road holding it must hand it
    /// over: a watch left holding an absence goes on refusing the taps of a
    /// device that came back, and a pump that asked `adb` on every healthy
    /// frame would pay 11.7 ms for what the frame already proved.
    fn answered(&mut self) {
        self.saw(DevicePresence::OnTheBridge);
    }

    fn saw(&mut self, presence: DevicePresence) {
        self.control
            .note_on_the_bridge(presence == DevicePresence::OnTheBridge);
        let stream = &self.stream;
        let serial = &self.serial;
        match presence {
            DevicePresence::Gone => {
                if self.gone_since.is_none() {
                    self.gone_since = Some(Instant::now());
                    emit_note(&self.app, stream, EmulatorNoteCode::DeviceOffline);
                    crate::note_window_event(
                        &self.husk_root,
                        &format!(
                            "emulator android {stream}: {serial} left the bridge; \
                             looking every {DEVICE_ABSENT_REST:?} until it is back"
                        ),
                    );
                }
            }
            DevicePresence::OnTheBridge => {
                if let Some(since) = self.gone_since.take() {
                    crate::note_window_event(
                        &self.husk_root,
                        &format!(
                            "emulator android {stream}: {serial} back on the bridge after {:?}",
                            since.elapsed()
                        ),
                    );
                }
            }
        }
    }
}

#[tauri::command]
pub(crate) async fn android_emulators(
    webview: tauri::Webview,
) -> Result<Vec<AndroidDevice>, String> {
    crate::from_the_main_webview(&webview)?;
    let local_data_root = webview
        .state::<crate::AppState>()
        .local_data_root()
        .to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        reconcile_managed_devices_now(&local_data_root);
        list_android_devices()
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) async fn android_emulators_direct() -> Result<Vec<AndroidDevice>, String> {
    tauri::async_runtime::spawn_blocking(list_android_devices)
        .await
        .map_err(|error| error.to_string())?
}

/// One lossless frame from the same adb device the built-in pane mirrors.
/// Bytes stay in-process until the authenticated CLI route has checked and
/// written them, so no partial destination is reported as a screenshot.
pub(crate) async fn android_screenshot_direct(serial: String) -> Result<Vec<u8>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sdk = android_sdk().map_err(|search| search.to_string())?;
        if device_presence(&sdk.adb, &serial) == DevicePresence::Gone {
            return Err(format!("Android device `{serial}` is not running"));
        }
        let output = crate::proc::quiet_command(&sdk.adb)
            .args(["-s", &serial, "exec-out", "screencap", "-p"])
            .output()
            .map_err(|error| format!("Android screenshot could not start: {error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if detail.is_empty() {
                "Android screenshot failed".to_string()
            } else {
                format!("Android screenshot failed: {detail}")
            });
        }
        if output.stdout.is_empty() {
            return Err("Android screenshot was empty".to_string());
        }
        if output.stdout.len() as u64 > super::MAX_FRAME_BYTES {
            return Err(format!(
                "Android screenshot exceeds the {} byte limit",
                super::MAX_FRAME_BYTES
            ));
        }
        Ok(output.stdout)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn selected_android_device(
    devices: &[AndroidDevice],
    requested: Option<&str>,
) -> Result<AndroidDevice, String> {
    match requested {
        Some(avd) => devices.iter().find(|device| device.avd == avd),
        None => devices
            .iter()
            .find(|device| device.booted)
            .or_else(|| devices.first()),
    }
    .cloned()
    .ok_or_else(|| "사용 가능한 Android 에뮬레이터(AVD)가 없습니다".to_string())
}

fn boot_android_device(
    app: &AppHandle,
    sdk: &AndroidSdk,
    chosen: &AndroidDevice,
) -> Result<String, String> {
    if let Some(serial) = &chosen.serial {
        return Ok(serial.clone());
    }
    let local_data_root = app
        .state::<crate::AppState>()
        .local_data_root()
        .to_path_buf();
    let managed = if let Some(adopted) = managed_process_for_avd(&local_data_root, &chosen.avd) {
        // A crash during boot leaves no adb serial yet. The tokenized process
        // is still the launch already in flight, so wait for it rather than
        // starting a second instance of the same AVD.
        adopted
    } else {
        // Before the launcher reads it: a `default_boot` the emulator has
        // already refused is a cold boot on every launch until something
        // replaces it (t-5762).
        clear_a_stale_snapshot(&local_data_root, &chosen.avd);
        // The intent reaches durable storage before the process exists. Its
        // unguessable token is also placed on the emulator command line, so a
        // restart can recover the pid even if this process dies immediately
        // after `spawn` and before the pid/start pair is committed.
        let managed = begin_managed_launch(&local_data_root, &chosen.avd)?;
        let marker = format!("{MANAGED_PROPERTY}={}", managed.record().token);
        let emulator = sdk.emulator.as_ref().map_err(SdkSearch::to_string)?;
        let mut boot = crate::proc::quiet_command(emulator);
        // No `-no-snapshot` (D2). That flag turns off BOTH halves of quick
        // boot — the resume on the way in and the save on the way out — so an
        // AVD carrying `default_boot` and `fastboot.forceFastBoot=yes` was
        // still paying a full cold boot every single time. Measured on one
        // Pixel 6 AVD, load ~33: 42.3 s to `sys.boot_completed` cold against
        // 7.3 s and 7.2 s resuming the snapshot the exit below writes.
        boot.args([
            "-avd",
            &chosen.avd,
            "-no-window",
            "-no-boot-anim",
            "-prop",
            &marker,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        let process = match boot.spawn() {
            Ok(process) => process,
            Err(error) => {
                forget_managed_process(&managed);
                return Err(format!("에뮬레이터를 시작할 수 없습니다: {error}"));
            }
        };
        attach_managed_child(&managed, process)?;
        managed
    };

    let deadline = Instant::now() + BOOT_TIMEOUT;
    loop {
        match managed.poll_process() {
            Ok(Some(status)) => {
                forget_managed_process(&managed);
                return Err(format!(
                    "Android 에뮬레이터가 부팅 전에 종료됐습니다: {status}"
                ));
            }
            Err(error) => {
                managed.stop();
                forget_managed_process(&managed);
                return Err(format!(
                    "Android 에뮬레이터 상태를 읽지 못했습니다: {error}"
                ));
            }
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            managed.stop();
            forget_managed_process(&managed);
            crate::note_window_event(
                &local_data_root,
                &format!("emulator android boot timed out: avd {}", chosen.avd),
            );
            return Err("에뮬레이터 부팅이 시간을 초과했습니다".to_string());
        }
        std::thread::sleep(BOOT_POLL_INTERVAL);
        // The serial of a booting AVD comes from adb, which is already the
        // next question asked of it — polling the AVD list here would start
        // the emulator binary once a second to learn nothing new.
        let found = android_running(&sdk.adb)
            .into_iter()
            .find(|(_, avd)| avd == &chosen.avd)
            .map(|(serial, _)| serial);
        let Some(serial) = found else {
            continue;
        };
        let ready = crate::proc::quiet_command(&sdk.adb)
            .args(["-s", &serial, "shell", "getprop", "sys.boot_completed"])
            .output()
            .ok()
            .filter(|answer| answer.status.success())
            .is_some_and(|answer| String::from_utf8_lossy(&answer.stdout).trim() == "1");
        if ready {
            // The AVD's snapshot directory is only final once the emulator
            // has read it, which is now.
            note_a_refused_snapshot(&local_data_root, &chosen.avd, managed.record().pid);
            held(&managed.record).serial = Some(serial.clone());
            if let Err(error) = write_managed_records(&local_data_root) {
                managed.stop();
                forget_managed_process(&managed);
                return Err(format!(
                    "Android 에뮬레이터 소유권을 기록할 수 없습니다: {error}"
                ));
            }
            if managed.has_child()
                && let Err(error) = watch_managed_process(&managed)
            {
                managed.stop();
                forget_managed_process(&managed);
                return Err(error);
            }
            return Ok(serial);
        }
    }
}

fn capture_path(stream: &str) -> PathBuf {
    std::env::temp_dir()
        .join("zerocode-emulator")
        .join(format!("{stream}.png"))
}

fn pump_android_frames(
    app: AppHandle,
    stream: String,
    adb: PathBuf,
    serial: String,
    control: Arc<SessionControl>,
) {
    let _finished = FinishSession::new(stream.clone());
    let husk_root = app
        .state::<crate::AppState>()
        .local_data_root()
        .to_path_buf();
    let frame = capture_path(&stream);
    if let Some(parent) = frame.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut misses = pump::MissCounter::new(MISSES_BEFORE_NOTE);
    let mut absence = AbsenceWatch::new(&app, &stream, &husk_root, &control, &serial);
    let mut unchanged = 0u32;
    let mut last_fingerprint = None;
    let mut last_emitted = None;
    while control.wait_until_running() {
        // A device already known to be gone is not photographed on the chance
        // that it came back. The watch's own memory is free; only when it
        // holds an absence is `adb` asked, and nothing at all is spawned at
        // the device until it answers.
        if absence.is_gone() && absence.absent(&adb) {
            control.rest(DEVICE_ABSENT_REST);
            continue;
        }
        if !control.wait_for_payload_slot() {
            continue;
        }
        let output = match std::fs::File::create(&frame) {
            Ok(output) => output,
            Err(_) => break,
        };
        let child = crate::proc::quiet_command(&adb)
            .args(["-s", &serial, "exec-out", "screencap", "-p"])
            .stdout(Stdio::from(output))
            .stderr(Stdio::null())
            .spawn();
        let Ok(child) = child else {
            break;
        };
        if !control.install_child(child) {
            continue;
        }
        let captured = wait_for_child(&control, CAPTURE_TIMEOUT)
            .then(|| read_bounded(&frame).ok())
            .flatten();
        let Some(bytes) = captured else {
            if control.is_alive() && !control.is_paused() {
                // A capture that came back with nothing is the FIRST place a
                // device's absence shows, and the only place worth asking adb
                // about it: a healthy pane takes a picture every 120 ms and
                // must not pay a process each time to be told what the
                // picture already proved.
                if absence.absent(&adb) {
                    control.rest(DEVICE_ABSENT_REST);
                } else {
                    if misses.missed() {
                        emit_note(&app, &stream, EmulatorNoteCode::FrameUnavailable);
                    }
                    control.rest(IDLE_CAPTURE_INTERVAL);
                }
            }
            continue;
        };
        misses.hit();
        absence.answered();
        let fingerprint = pump::frame_fingerprint(&bytes);
        let changed = last_fingerprint != Some(fingerprint);
        let refresh_due =
            last_emitted.is_none_or(|at: Instant| at.elapsed() >= IDLE_REFRESH_INTERVAL);
        if changed || refresh_due {
            match emit_bytes(
                &app,
                &control,
                "emulator:frame",
                &stream,
                &bytes,
                Some("image/png"),
                // This road PAYS for the picture it is holding — a `screencap`
                // per frame — so waiting for the renderer is cheaper than
                // throwing the picture away and taking another.
                SlotWait::Block,
            ) {
                EmitOutcome::Sent => last_emitted = Some(Instant::now()),
                EmitOutcome::Skipped | EmitOutcome::Closed => {
                    if control.is_alive() && !control.is_paused() {
                        emit_note(&app, &stream, EmulatorNoteCode::StreamEnded);
                        break;
                    }
                }
            }
        }
        if changed {
            unchanged = 0;
            last_fingerprint = Some(fingerprint);
        } else {
            unchanged = unchanged.saturating_add(1);
        }
        control.rest(IDLE_LADDER.delay(unchanged));
    }
    control.kill_child();
    let _ = std::fs::remove_file(frame);
}

#[tauri::command]
pub(crate) async fn start_android_stream(
    app: AppHandle,
    webview: tauri::Webview,
    avd: Option<String>,
    on_frame: BinaryChannel,
) -> Result<EmulatorStream, String> {
    crate::from_the_main_webview(&webview)?;
    // The same door as iOS's, warmed for the same reason (t-5535): a walk on
    // this device asks its first question a moment after the first frame.
    crate::systemone::warm_for_walks();
    tauri::async_runtime::spawn_blocking(move || {
        let sdk = android_sdk().map_err(|search| search.to_string())?;
        reconcile_managed_devices_now(app.state::<crate::AppState>().local_data_root());
        let chosen = selected_android_device(&list_android_devices()?, avd.as_deref())?;
        // Before the claim, because a pane handed an existing session returns
        // from inside it — and a device somebody just opened a second pane on
        // is exactly the one the next window should put up (D4).
        note_last_used_device(
            app.state::<crate::AppState>().local_data_root(),
            &chosen.avd,
            crate::now_epoch_ms(),
        );
        let lease = match registry().claim(SessionKey::frames(
            EmulatorPlatform::Android,
            chosen.avd.clone(),
        ))? {
            StartClaim::Existing(stream) => {
                // Same rule as iOS: the reused session adopts the newest
                // caller's door, or it posts into one that is already closed.
                registry().hand_frames_to(&stream.stream, on_frame);
                return Ok(stream);
            }
            StartClaim::Acquired(lease) => lease,
        };
        let serial = boot_android_device(&app, &sdk, &chosen)?;
        let stream_id = crate::hooks::random_token().ok_or("스트림 id를 만들 수 없습니다")?;
        let descriptor = EmulatorStream {
            stream: stream_id.clone(),
            udid: serial.clone(),
            name: chosen.avd.clone(),
            platform: EmulatorPlatform::Android,
            interactive: true,
            reused: false,
        };
        let control = registry().new_control(&descriptor);
        control.hand_frames_to(on_frame);
        let descriptor = lease.activate(descriptor, control.clone());
        crate::note_window_event(
            app.state::<crate::AppState>().local_data_root(),
            &format!(
                "emulator android stream {stream_id} avd {} serial {serial} sdk {}",
                chosen.avd,
                sdk.root.display()
            ),
        );
        let pump_app = app.clone();
        let pump_stream = stream_id.clone();
        let pump_adb = sdk.adb;
        if let Err(error) = std::thread::Builder::new()
            .name(format!("android-emulator-{stream_id}"))
            .spawn(move || pump_android_frames(pump_app, pump_stream, pump_adb, serial, control))
        {
            registry().stop(&stream_id);
            return Err(format!("Android 화면 스트림을 시작할 수 없습니다: {error}"));
        }
        Ok(descriptor)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// One scrcpy mirror session, from its handshake to whatever ends it.
///
/// The device-side encoder pushes h264 the moment pixels change — no polling,
/// no per-frame process. The server's own framing (device name, codec header,
/// per-packet PTS) is read, used, and left on this side; only Annex-B bytes
/// cross the bridge, which the window's decoder already splits.
///
/// Ends on a closed socket, a stream that lost its place, or the pane being
/// closed or paused. All of these want the same thing from the caller — the
/// outer loop, which decides between mirroring again and the recorder.
fn pump_scrcpy_session(
    app: &AppHandle,
    stream: &str,
    mut session: crate::scrcpy::Session,
    control: &Arc<SessionControl>,
) {
    let mut held: Vec<u8> = Vec::new();
    // Two marks, not one. Neither opening piece is promised in a single read,
    // and a single mark would have the handshake taken TWICE when the codec
    // header lands in the next read — sixty-five bytes of somebody's first
    // picture, eaten as a name.
    let mut greeted = false;
    let mut opened = false;
    while control.is_alive() && !control.is_paused() {
        match session.read_into(&mut held) {
            Ok(_) => {}
            // A read that timed out is a screen nobody touched, not a mirror
            // that died: the deadline is short so a closed pane is noticed
            // quickly, and reaching it means going back to ask again.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(_) => return,
        }
        // The opening bytes, in the order the server writes them, each
        // remembered once it has been taken.
        if !greeted {
            if crate::scrcpy_video::take_handshake(&mut held).is_none() {
                continue;
            }
            greeted = true;
        }
        if !opened {
            if crate::scrcpy_video::take_codec_meta(&mut held).is_none() {
                continue;
            }
            opened = true;
        }
        let Ok(packets) = crate::scrcpy_video::take_packets(&mut held) else {
            return;
        };
        if packets.is_empty() {
            continue;
        }
        // One event for one read rather than one per packet: the window wants
        // a byte stream, and three events carrying a third of a frame each
        // cost three crossings for the same picture.
        let mut carried = Vec::new();
        for packet in packets {
            carried.extend_from_slice(&packet.data);
        }
        if !matches!(
            emit_bytes(
                app,
                control,
                "emulator:video",
                stream,
                &carried,
                None,
                SlotWait::Block,
            ),
            EmitOutcome::Sent
        ) {
            return;
        }
    }
}

fn pump_android_video(
    app: AppHandle,
    stream: String,
    sdk: AndroidSdk,
    serial: String,
    jar: Option<PathBuf>,
    control: Arc<SessionControl>,
) {
    let _finished = FinishSession::new(stream.clone());
    let husk_root = app
        .state::<crate::AppState>()
        .local_data_root()
        .to_path_buf();
    let mut absence = AbsenceWatch::new(&app, &stream, &husk_root, &control, &serial);
    while control.wait_until_running() {
        // The device itself first, once, before anything is pushed to it.
        // Both roads below end at the same device, so both are the same waste
        // when it is gone: measured, a killed emulator had this loop pushing
        // the scrcpy jar 6.4 times a second for as long as the pane stayed
        // open. A turn here is a whole mirror session, so the one reading
        // costs 11.7 ms against the 226 ms the session start costs anyway.
        if absence.absent(&sdk.adb) {
            control.rest(DEVICE_ABSENT_REST);
            continue;
        }
        // The mirror first, when this machine has the helper for it. scrcpy
        // does its own scaling, so it is handed the long edge rather than a
        // shape — and it needs no `--size` arithmetic from this side at all.
        if let Some(jar) = jar.as_deref() {
            match crate::scrcpy::start(
                &sdk.adb,
                &serial,
                jar,
                VIDEO_MAX_DIMENSION,
                ANDROID_STREAM_BIT_RATE,
            ) {
                Ok(session) => {
                    crate::note_window_event(
                        &husk_root,
                        &format!("emulator android {stream}: mirroring {serial} through scrcpy"),
                    );
                    let stood = Instant::now();
                    pump_scrcpy_session(&app, &stream, session, &control);
                    // A session ended by the pane (closed or paused) says
                    // nothing about the mirror's health — only one that fell
                    // over on its own, fast, yields its next turn.
                    if stood.elapsed() >= SCRCPY_STOOD_UP
                        || !control.is_alive()
                        || control.is_paused()
                    {
                        continue;
                    }
                    crate::note_window_event(
                        &husk_root,
                        &format!(
                            "emulator android {stream}: scrcpy fell over in {:?}; recording instead",
                            stood.elapsed()
                        ),
                    );
                }
                Err(said) => crate::note_window_event(
                    &husk_root,
                    &format!(
                        "emulator android {stream}: scrcpy refused ({said}); recording instead"
                    ),
                ),
            }
        }
        // The recorder road. Its size is asked fresh each time the encoder is
        // respawned rather than once: a device rotated or resized between two
        // spawns is a different shape, and the frame that comes back has to
        // match the pane that draws it.
        let video_size = bounded_video_size(&sdk, &serial);
        let mut encoder = crate::proc::quiet_command(&sdk.adb);
        encoder.args([
            "-s",
            &serial,
            "exec-out",
            "screenrecord",
            "--output-format=h264",
            "--time-limit",
            VIDEO_TIME_LIMIT_SECONDS,
            "--bit-rate",
            ANDROID_STREAM_BIT_RATE,
        ]);
        if let Some(size) = &video_size {
            encoder.args(["--size", size]);
        }
        let born = encoder
            .arg("-")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = born else {
            emit_note(&app, &stream, EmulatorNoteCode::VideoStartFailed);
            break;
        };
        let Some(mut output) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            break;
        };
        if !control.install_child(child) {
            continue;
        }
        let mut bytes = [0u8; 64 * 1024];
        let mut carried = false;
        loop {
            let Ok(read) = output.read(&mut bytes) else {
                break;
            };
            if read == 0 || !control.is_alive() || control.is_paused() {
                break;
            }
            carried = true;
            if !matches!(
                emit_bytes(
                    &app,
                    &control,
                    "emulator:video",
                    &stream,
                    &bytes[..read],
                    None,
                    SlotWait::Block,
                ),
                EmitOutcome::Sent
            ) {
                break;
            }
        }
        control.kill_child();
        if control.is_alive() && !control.is_paused() {
            // A turn that carried pictures and ended is a recorder reaching
            // its time limit: replace it at once, or the mirror blinks. A
            // turn that carried NOTHING — no mirror, no frame — is a device
            // that is listed but not yet showing anything, and trying it
            // again a tenth of a second later is the loop this task came
            // from, only with the serial present.
            control.rest(if carried {
                VIDEO_RESTART_INTERVAL
            } else {
                DEVICE_WAKING_REST
            });
        }
    }
    control.kill_child();
}

#[tauri::command]
pub(crate) async fn start_emulator_video(
    app: AppHandle,
    webview: tauri::Webview,
    serial: String,
    on_video: BinaryChannel,
) -> Result<EmulatorStream, String> {
    crate::from_the_main_webview(&webview)?;
    // The mirror's fast road needs a helper this machine may not have yet.
    // Fetched once into the cache, checked against the vendor's own hash, and
    // never fatal — a machine with no network keeps the recorder road, which
    // is why this is an `Option` rather than a `?`.
    let cache_root = app
        .state::<crate::AppState>()
        .local_data_root()
        .to_path_buf();
    let jar = crate::scrcpy::server_jar(&cache_root).await;
    tauri::async_runtime::spawn_blocking(move || {
        let husk_root = cache_root;
        let sdk = android_sdk().map_err(|search| {
            crate::note_window_event(&husk_root, "emulator video refused: no android sdk");
            search.to_string()
        })?;
        if registry()
            .target_control(EmulatorPlatform::Android, &serial)
            .is_none()
        {
            crate::note_window_event(
                &husk_root,
                &format!("emulator video refused: serial {serial} not active"),
            );
            return Err("이 기기 스트림은 실행 중이 아닙니다".to_string());
        }
        let lease =
            match registry().claim(SessionKey::video(EmulatorPlatform::Android, serial.clone()))? {
                StartClaim::Existing(stream) => {
                    registry().hand_frames_to(&stream.stream, on_video);
                    return Ok(stream);
                }
                StartClaim::Acquired(lease) => lease,
            };
        let stream_id = crate::hooks::random_token().ok_or("스트림 id를 만들 수 없습니다")?;
        let descriptor = EmulatorStream {
            stream: stream_id.clone(),
            udid: serial.clone(),
            name: serial.clone(),
            platform: EmulatorPlatform::Android,
            interactive: true,
            reused: false,
        };
        let control = registry().new_control(&descriptor);
        control.hand_frames_to(on_video);
        let descriptor = lease.activate(descriptor, control.clone());
        let pump_app = app.clone();
        let pump_stream = stream_id.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("android-video-{stream_id}"))
            .spawn(move || pump_android_video(pump_app, pump_stream, sdk, serial, jar, control))
        {
            registry().stop(&stream_id);
            return Err(format!(
                "Android 비디오 스트림을 시작할 수 없습니다: {error}"
            ));
        }
        Ok(descriptor)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The one door every tap, keystroke, tree read and log tail comes through.
///
/// It is also where a device that is gone turns them away — in the pane's own
/// words rather than in `adb`'s, and without spending a process to say so: the
/// pump leaves its last reading on the session, and only a reading that says
/// "gone" is worth the [`device_presence`] call that confirms it.
fn android_control(serial: &str) -> Result<(AndroidSdk, Arc<SessionControl>), String> {
    let sdk = android_sdk().map_err(|search| search.to_string())?;
    let control = registry()
        .target_control(EmulatorPlatform::Android, serial)
        .filter(|control| control.is_alive())
        .ok_or("이 Android 에뮬레이터 스트림은 실행 중이 아닙니다")?;
    if !control.was_on_the_bridge() && device_presence(&sdk.adb, serial) == DevicePresence::Gone {
        return Err(EmulatorNoteCode::DeviceOffline.code().to_string());
    }
    Ok((sdk, control))
}

fn read_android_screen_size(sdk: &AndroidSdk, serial: &str) -> Result<(u32, u32), String> {
    read_android_display(sdk, serial).map(|display| display.size)
}

fn read_android_display(sdk: &AndroidSdk, serial: &str) -> Result<display::Geometry, String> {
    let bytes = accessibility::run(
        &sdk.adb,
        &["-s", serial, "shell", "dumpsys", "input"],
        Some(accessibility::MAX_OUTPUT_BYTES),
    )?;
    display::parse(&String::from_utf8_lossy(&bytes))
}

fn bounded_video_size(sdk: &AndroidSdk, serial: &str) -> Option<String> {
    let (width, height) = read_android_screen_size(sdk, serial).ok()?;
    bounded_video_dimensions(width, height).map(|(width, height)| format!("{width}x{height}"))
}

fn bounded_video_dimensions(width: u32, height: u32) -> Option<(u32, u32)> {
    let longest = width.max(height);
    if longest <= VIDEO_MAX_DIMENSION {
        return None;
    }
    let scale = f64::from(VIDEO_MAX_DIMENSION) / f64::from(longest);
    let even = |dimension: u32| {
        let scaled = (f64::from(dimension) * scale).round() as u32;
        scaled.max(2) & !1
    };
    Some((even(width), even(height)))
}

pub(super) fn device_point(x: f64, y: f64, size: (u32, u32)) -> (u32, u32) {
    let (width, height) = size;
    (
        (x.clamp(0.0, 1.0) * f64::from(width.saturating_sub(1))).round() as u32,
        (y.clamp(0.0, 1.0) * f64::from(height.saturating_sub(1))).round() as u32,
    )
}

fn android_input(sdk: &AndroidSdk, serial: &str, args: &[&str]) -> Result<(), String> {
    let (_, control) = android_control(serial)?;
    let _input = control.input()?;
    android_input_unlocked(sdk, serial, args)
}

fn android_input_unlocked(sdk: &AndroidSdk, serial: &str, args: &[&str]) -> Result<(), String> {
    let mut command = vec!["-s", serial, "shell", "input"];
    command.extend_from_slice(args);
    let out = crate::proc::quiet_command(&sdk.adb)
        .args(command)
        .output()
        .map_err(|error| error.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[tauri::command]
pub(crate) async fn android_tap(
    webview: tauri::Webview,
    serial: String,
    x: f64,
    y: f64,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    android_tap_direct(serial, x, y).await
}

pub(crate) async fn android_tap_direct(serial: String, x: f64, y: f64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, control) = android_control(&serial)?;
        let _input = control.input()?;
        tap_normalized(&sdk, &serial, x, y)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn tap_normalized(sdk: &AndroidSdk, serial: &str, x: f64, y: f64) -> Result<(), String> {
    tap_at(sdk, serial, x, y, read_android_screen_size(sdk, serial)?)
}

fn tap_at(sdk: &AndroidSdk, serial: &str, x: f64, y: f64, size: (u32, u32)) -> Result<(), String> {
    let (x, y) = device_point(x, y, size);
    android_input_unlocked(sdk, serial, &["tap", &x.to_string(), &y.to_string()])?;
    registry().nudge(EmulatorPlatform::Android, serial);
    Ok(())
}

fn marks_snapshot(
    sdk: &AndroidSdk,
    serial: &str,
) -> Result<(super::marks::Snapshot, (u32, u32)), String> {
    let display = read_android_display(sdk, serial)?;
    let tree = serde_json::to_value(accessibility::snapshot(&sdk.adb, serial)?)
        .map_err(|error| error.to_string())?;
    if read_android_display(sdk, serial)? != display {
        return Err("Android display changed while reading the tree; run marks again".into());
    }
    let size = display.size;
    let screen = zerocode_core::computer_use_protocol::render::Rect::new(
        0.0,
        0.0,
        f64::from(size.0),
        f64::from(size.1),
    );
    super::marks::Snapshot::new(
        zerocode_core::computer_use::EmulatorPlatform::Android,
        &tree,
        screen,
    )
    .map(|snapshot| (snapshot, size))
}

pub(super) async fn marks_snapshot_direct(
    serial: String,
    identity: String,
) -> Result<super::marks::Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, _control) = android_control(&serial)?;
        let (snapshot, _) = marks_snapshot(&sdk, &serial)?;
        if android_avd_name(&sdk.adb, &serial)? != identity {
            return Err("Android device changed while reading the tree; run marks again".into());
        }
        Ok(snapshot)
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) async fn click_mark_direct(
    serial: String,
    request: super::marks::PinnedTap,
) -> Result<(), zerocode_core::computer_use_protocol::ProviderError> {
    use super::marks::backend_error;
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, control) = android_control(&serial).map_err(backend_error)?;
        let input = control.input().map_err(backend_error)?;
        let (snapshot, size) = marks_snapshot(&sdk, &serial).map_err(backend_error)?;
        let identity = android_avd_name(&sdk.adb, &serial).map_err(backend_error)?;
        request.on_device(&identity, || {
            request.perform_in(&input, &snapshot.faces, snapshot.screen, |x, y| {
                tap_at(&sdk, &serial, x, y, size).map_err(backend_error)
            })
        })
    })
    .await
    .map_err(backend_error)?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn android_swipe(
    webview: tauri::Webview,
    serial: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    ms: Option<u32>,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    android_swipe_direct(serial, x1, y1, x2, y2, ms).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn android_swipe_direct(
    serial: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    ms: Option<u32>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, control) = android_control(&serial)?;
        let _input = control.input()?;
        swipe_normalized(&sdk, &serial, (x1, y1), (x2, y2), ms)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn swipe_normalized(
    sdk: &AndroidSdk,
    serial: &str,
    from: (f64, f64),
    to: (f64, f64),
    ms: Option<u32>,
) -> Result<(), String> {
    let size = read_android_screen_size(sdk, serial)?;
    let (x1, y1) = device_point(from.0, from.1, size);
    let (x2, y2) = device_point(to.0, to.1, size);
    let duration = ms.unwrap_or(300).clamp(50, 3_000).to_string();
    android_input_unlocked(
        sdk,
        serial,
        &[
            "swipe",
            &x1.to_string(),
            &y1.to_string(),
            &x2.to_string(),
            &y2.to_string(),
            &duration,
        ],
    )?;
    registry().nudge(EmulatorPlatform::Android, serial);
    Ok(())
}

#[tauri::command]
pub(crate) async fn android_text(
    webview: tauri::Webview,
    serial: String,
    text: String,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    android_text_direct(serial, text).await
}

pub(crate) async fn android_text_direct(serial: String, text: String) -> Result<(), String> {
    if text.chars().any(|character| character.is_control()) || text.len() > 4096 {
        return Err("보낼 수 없는 문자가 있습니다".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, _) = android_control(&serial)?;
        let escaped = text.replace(' ', "%s");
        android_input(&sdk, &serial, &["text", &escaped])?;
        registry().nudge(EmulatorPlatform::Android, &serial);
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn android_keycode(name: &str) -> Option<&'static str> {
    Some(match name {
        "home" => "3",
        "back" => "4",
        "recents" | "recent" | "app_switch" | "overview" => "187",
        "power" | "lock" => "26",
        "volume-up" | "volume_up" | "volup" => "24",
        "volume-down" | "volume_down" | "voldown" => "25",
        "enter" => "66",
        "del" => "67",
        "forward_del" => "112",
        "escape" => "111",
        "tab" => "61",
        "up" => "19",
        "down" => "20",
        "left" => "21",
        "right" => "22",
        _ => return None,
    })
}

#[tauri::command]
pub(crate) async fn android_button(
    webview: tauri::Webview,
    serial: String,
    name: String,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    android_button_direct(serial, name).await
}

pub(crate) async fn android_button_direct(serial: String, name: String) -> Result<(), String> {
    let code = android_keycode(&name).ok_or("알 수 없는 버튼입니다")?;
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, _) = android_control(&serial)?;
        android_input(&sdk, &serial, &["keyevent", code])?;
        registry().nudge(EmulatorPlatform::Android, &serial);
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn android_rotate(
    webview: tauri::Webview,
    serial: String,
    rotation: u32,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    android_rotate_direct(serial, rotation).await
}

pub(crate) async fn android_rotate_direct(serial: String, rotation: u32) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, control) = android_control(&serial)?;
        let _input = control.input()?;
        let put = |key: &str, value: &str| -> Result<(), String> {
            let out = crate::proc::quiet_command(&sdk.adb)
                .args([
                    "-s", &serial, "shell", "settings", "put", "system", key, value,
                ])
                .output()
                .map_err(|error| error.to_string())?;
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        };
        put("accelerometer_rotation", "0")?;
        put("user_rotation", &(rotation % 4).to_string())?;
        registry().nudge(EmulatorPlatform::Android, &serial);
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn shutdown_android_emulator(
    webview: tauri::Webview,
    serial: String,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let sdk = android_sdk().map_err(|search| search.to_string())?;
        if device_presence(&sdk.adb, &serial) == DevicePresence::Gone {
            stop_managed_device(&serial);
            return Ok(());
        }
        let out = crate::proc::quiet_command(&sdk.adb)
            .args(["-s", &serial, "emu", "kill"])
            .output()
            .map_err(|error| error.to_string())?;
        if out.status.success() {
            stop_managed_device(&serial);
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn android_accessibility_tree(
    webview: tauri::Webview,
    serial: String,
) -> Result<serde_json::Value, String> {
    crate::from_the_main_webview(&webview)?;
    android_accessibility_tree_direct(serial).await
}

pub(crate) async fn android_accessibility_tree_direct(
    serial: String,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, _control) = android_control(&serial)?;
        let tree = accessibility::snapshot(&sdk.adb, &serial)?;
        serde_json::to_value(tree).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn android_install_app(
    webview: tauri::Webview,
    serial: String,
    path: String,
    reinstall: Option<bool>,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, _control) = android_control(&serial)?;
        capabilities::install_app(&sdk, &serial, &path, reinstall.unwrap_or(false))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn android_launch_app(
    webview: tauri::Webview,
    serial: String,
    package: String,
    activity: Option<String>,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, control) = android_control(&serial)?;
        capabilities::launch_app(&sdk, &serial, &package, activity)?;
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn android_set_permission(
    webview: tauri::Webview,
    serial: String,
    operation: String,
    package: String,
    permission: Option<String>,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, _control) = android_control(&serial)?;
        capabilities::set_permission(&sdk, &serial, &operation, &package, permission)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn android_logs(
    webview: tauri::Webview,
    serial: String,
    lines: Option<usize>,
    filters: Option<Vec<String>>,
) -> Result<super::capability::EmulatorLogBatch, String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, _control) = android_control(&serial)?;
        capabilities::logs(&sdk, &serial, lines, filters)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn held<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_are_clamped_to_the_last_device_pixel() {
        assert_eq!(device_point(-1.0, 2.0, (100, 200)), (0, 199));
        assert_eq!(device_point(0.5, 0.5, (101, 201)), (50, 100));
    }

    #[test]
    fn video_is_bounded_to_orcas_long_edge_and_encoder_safe_even_dimensions() {
        assert_eq!(bounded_video_dimensions(1080, 2400), Some((576, 1280)));
        assert_eq!(bounded_video_dimensions(1280, 720), None);
        let (width, height) = bounded_video_dimensions(1179, 2557).unwrap();
        assert_eq!(height, VIDEO_MAX_DIMENSION);
        assert_eq!(width % 2, 0);
        assert_eq!(height % 2, 0);
    }

    #[test]
    fn every_orca_button_alias_has_one_keycode() {
        for alias in [
            "home",
            "back",
            "recents",
            "recent",
            "app_switch",
            "overview",
            "power",
            "lock",
            "volume-up",
            "volume_up",
            "volup",
            "volume-down",
            "volume_down",
            "voldown",
            "enter",
            "del",
            "forward_del",
            "escape",
            "tab",
            "up",
            "down",
            "left",
            "right",
        ] {
            assert!(android_keycode(alias).is_some(), "missing alias {alias}");
        }
    }

    #[test]
    fn a_managed_emulator_child_is_collected_after_it_exits() {
        let mut exits = if cfg!(windows) {
            let mut command = crate::proc::quiet_command("cmd");
            command.args(["/C", "exit", "0"]);
            command
        } else {
            crate::proc::quiet_command("true")
        };
        let child = exits.spawn().expect("short-lived child");
        let root = tempfile::tempdir().expect("local data root");
        let process = ManagedEmulatorProcess::new_launch(
            root.path().to_path_buf(),
            "Pixel".to_string(),
            "a".repeat(48),
        );
        *held(&process.child) = Some(child);
        process.launching.store(false, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match process.poll_child() {
                Ok(Some(status)) => {
                    assert!(status.success());
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                answer => panic!("child was not collected: {answer:?}"),
            }
        }
        assert!(held(&process.child).is_none());
    }

    fn managed_record(token: char) -> ManagedEmulatorRecord {
        ManagedEmulatorRecord {
            token: token.to_string().repeat(48),
            avd: "Pixel_9".to_string(),
            serial: None,
            pid: None,
            started: None,
        }
    }

    #[test]
    fn a_pre_spawn_journal_intent_recovers_the_tokenized_process_after_a_crash() {
        let record = managed_record('b');
        let marker = format!("{MANAGED_PROPERTY}={}", record.token);
        let command = format!("/sdk/emulator -avd Pixel_9 -no-window -prop {marker}");
        let sample = crate::resource_usage::ProcessSample::fixture(&[(
            700,
            1,
            42.0,
            4096,
            "Thu Aug 27 13:14:15 2026",
            &command,
        )]);
        let kept = reconcile_managed_records(
            vec![record],
            &HashSet::new(),
            &sample,
            Some(&[("emulator-5554".to_string(), "Pixel_9".to_string())]),
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].pid, Some(700));
        assert_eq!(kept[0].started.as_deref(), Some("Thu Aug 27 13:14:15 2026"));
        assert_eq!(kept[0].serial.as_deref(), Some("emulator-5554"));
    }

    #[test]
    fn a_recycled_pid_is_not_recovered_from_the_old_command_token() {
        let mut record = managed_record('c');
        record.pid = Some(700);
        record.started = Some("old-start".to_string());
        let marker = format!("{MANAGED_PROPERTY}={}", record.token);
        let command = format!("/sdk/emulator -avd Pixel_9 -prop {marker}");
        let sample = crate::resource_usage::ProcessSample::fixture(&[(
            700,
            1,
            1.0,
            4096,
            "new-start",
            &command,
        )]);
        assert!(
            reconcile_managed_records(vec![record], &HashSet::new(), &sample, Some(&[])).is_empty()
        );

        let mut forged = managed_record('f');
        forged.pid = Some(701);
        forged.started = Some("same-start".to_string());
        let sample = crate::resource_usage::ProcessSample::fixture(&[(
            701,
            1,
            1.0,
            4096,
            "same-start",
            "/renderer --claimed-pid 701",
        )]);
        assert!(
            reconcile_managed_records(vec![forged], &HashSet::new(), &sample, Some(&[])).is_empty()
        );
    }

    #[test]
    fn the_durable_owner_rejects_partial_pid_identities_and_forged_serials() {
        let mut record = managed_record('d');
        assert!(valid_managed_record(&record));
        record.pid = Some(7);
        assert!(!valid_managed_record(&record));
        record.started = Some("born".to_string());
        assert!(valid_managed_record(&record));
        record.serial = Some("renderer-7".to_string());
        assert!(!valid_managed_record(&record));
    }

    #[test]
    fn an_idle_fleet_is_only_put_away_after_the_persons_own_minutes() {
        let now = 1_700_000_000_000;
        // A pane is open: there is nothing to decide.
        assert!(!idle_shutdown_is_due(None, now, 30));
        // Closed, but not for long enough — and not for a second too few.
        assert!(!idle_shutdown_is_due(
            Some(now - 29 * MILLIS_PER_MINUTE),
            now,
            30
        ));
        assert!(!idle_shutdown_is_due(
            Some(now - 30 * MILLIS_PER_MINUTE + 1),
            now,
            30
        ));
        assert!(idle_shutdown_is_due(
            Some(now - 30 * MILLIS_PER_MINUTE),
            now,
            30
        ));
        // Zero minutes is the person saying never, however long it has been.
        assert!(!idle_shutdown_is_due(Some(0), now, 0));
        // A clock that walked backwards is not a reason to turn a device off.
        assert!(!idle_shutdown_is_due(
            Some(now + MILLIS_PER_MINUTE),
            now,
            30
        ));
    }

    #[test]
    fn only_a_device_used_inside_the_preboot_window_is_put_up_again() {
        let now = 1_700_000_000_000;
        assert!(within_the_preboot_window(now, now));
        assert!(within_the_preboot_window(
            now - PREBOOT_RECENT_DAYS * MILLIS_PER_DAY,
            now
        ));
        assert!(!within_the_preboot_window(
            now - PREBOOT_RECENT_DAYS * MILLIS_PER_DAY - 1,
            now
        ));
        // A row written by a clock ahead of this one is not "just used".
        assert!(!within_the_preboot_window(now + 1, now));
    }

    #[test]
    fn the_last_used_device_outlives_the_rows_and_the_rewrites_that_drop_them() {
        let root = tempfile::tempdir().expect("local data root");
        let now = 1_700_000_000_000;
        note_last_used_device(root.path(), "Pixel_9", now);
        assert_eq!(
            preboot_candidate(root.path(), now).as_deref(),
            Some("Pixel_9")
        );
        // The rewrite every launch and every reconcile performs names no
        // last-used device, and must not take this one away with it.
        write_managed_records(root.path()).expect("rewrite the rows");
        assert_eq!(
            preboot_candidate(root.path(), now).as_deref(),
            Some("Pixel_9")
        );
        // A week later it is still readable and no longer worth booting.
        assert_eq!(
            preboot_candidate(root.path(), now + PREBOOT_RECENT_DAYS * MILLIS_PER_DAY + 1),
            None
        );
        // A name this window would refuse to put on a command line is not
        // remembered at all.
        note_last_used_device(root.path(), "Pixel 9; rm -rf /", now);
        assert_eq!(
            preboot_candidate(root.path(), now).as_deref(),
            Some("Pixel_9")
        );
    }

    /// Only `device` is a device. Everything `adb` can say instead — a state
    /// that refuses work, a failed command, a blank line — is absence, and it
    /// is absence for the mirror, the recorder and the input door alike.
    #[test]
    fn only_a_serial_adb_calls_device_counts_as_being_there() {
        assert_eq!(presence_of(Some("device")), DevicePresence::OnTheBridge);
        for said in [
            "offline",
            "unauthorized",
            "bootloader",
            "recovery",
            "",
            "unknown",
        ] {
            assert_eq!(
                presence_of(Some(said)),
                DevicePresence::Gone,
                "`adb get-state` said {said:?} and that was taken for a device"
            );
        }
        // The command itself refusing — `error: device 'emulator-5554' not
        // found`, which is what a killed emulator produces — is the ordinary
        // way this question is answered, not an exception to it.
        assert_eq!(presence_of(None), DevicePresence::Gone);
    }

    /// A serial that is not an emulator's is never asked about.
    ///
    /// The listing road has always dropped a phone on a cable, and the
    /// verdict the three roads now share has to carry that rule with it —
    /// `adb get-state` would answer `device` for one happily. The path is a
    /// name nothing can run, so a verdict of anything but `Gone` would have
    /// meant the rule was read after the process was spawned.
    #[test]
    fn a_serial_that_is_not_an_emulators_is_gone_without_asking_adb() {
        assert_eq!(
            device_presence(Path::new("/nonexistent/adb"), "R5CT21ABCDE"),
            DevicePresence::Gone
        );
        assert!(EMULATOR_SERIAL_PREFIX.ends_with('-'));
    }

    /// The rest is a named interval, long enough to be a rest and short
    /// enough that the pane it darkens comes back inside its three seconds.
    ///
    /// The ceiling is the whole reattachment: this look, then the push,
    /// tunnel and handshake that follow it (945 ms to the readiness byte,
    /// measured in [`crate::scrcpy`]).
    #[test]
    fn the_absent_rest_is_a_second_not_a_hot_loop() {
        assert!(
            DEVICE_ABSENT_REST >= Duration::from_secs(1),
            "resting less than a second is the loop this replaced"
        );
        assert!(
            DEVICE_ABSENT_REST + Duration::from_millis(945) <= Duration::from_secs(3),
            "the pane cannot come back inside three seconds from this rest"
        );
    }

    /// What a killed emulator costs a pane, measured rather than argued.
    ///
    /// Ignored by default: it needs a real device, and it KILLS the one it is
    /// given twice over, which is why nothing is assumed and everything is
    /// named. Give it the COPY of an AVD on a port of its own — never the
    /// device a window is holding.
    ///
    /// ```text
    /// ZEROCODE_LIVE_ADB=$ANDROID_HOME/platform-tools/adb \
    /// ZEROCODE_LIVE_EMULATOR=$ANDROID_HOME/emulator/emulator \
    /// ZEROCODE_LIVE_SERIAL=emulator-5560 \
    /// ZEROCODE_LIVE_AVD=t5761_probe \
    /// ZEROCODE_LIVE_SCRCPY_JAR=~/Library/Application\ Support/dev.zerocode.app/scrcpy/scrcpy-server-v3.3.4.jar \
    ///   cargo test -p zerocode-shell -- --ignored --nocapture a_killed_emulator
    /// ```
    ///
    /// Both roads are run against the same device under the same conditions:
    /// the one 1.1.11 shipped, written out here because it is the CONTROL and
    /// no longer exists in the pump, and the one this lands. Each pass kills
    /// the device, measures what the pump does with the gap, stands the AVD
    /// back up and measures how long the pane stays dark AFTER `adb` lists the
    /// serial again — the boot itself is the device's own and belongs to
    /// neither road.
    #[test]
    #[ignore = "needs a real Android device it is allowed to kill"]
    fn a_killed_emulator_is_rested_on_by_one_road_and_hammered_by_the_other() {
        let (Ok(adb), Ok(emulator), Ok(serial), Ok(avd), Ok(jar)) = (
            std::env::var("ZEROCODE_LIVE_ADB"),
            std::env::var("ZEROCODE_LIVE_EMULATOR"),
            std::env::var("ZEROCODE_LIVE_SERIAL"),
            std::env::var("ZEROCODE_LIVE_AVD"),
            std::env::var("ZEROCODE_LIVE_SCRCPY_JAR"),
        ) else {
            println!("LIVE: no device named; nothing measured");
            return;
        };
        let adb = PathBuf::from(adb);
        let jar = PathBuf::from(jar);
        let emulator = PathBuf::from(emulator);
        let port = serial
            .strip_prefix(EMULATOR_SERIAL_PREFIX)
            .expect("an emulator serial")
            .to_string();
        let sdk = AndroidSdk {
            root: emulator.clone(),
            adb: adb.clone(),
            emulator: Ok(emulator.clone()),
        };
        /// How long each road is watched while the device is away.
        const WATCHED: Duration = Duration::from_secs(10);
        /// How closely the witness stamps the device's return. The error in
        /// every reattachment below is bounded by this, and it is the same
        /// for both roads.
        const WITNESS_TICK: Duration = Duration::from_millis(100);

        let kill = |adb: &Path| {
            let asked = Instant::now();
            let _ = crate::proc::quiet_command(adb)
                .args(["-s", &serial, "emu", "kill"])
                .status();
            while device_presence(adb, &serial) == DevicePresence::OnTheBridge {
                assert!(
                    asked.elapsed() < Duration::from_secs(60),
                    "it would not die"
                );
                std::thread::sleep(WITNESS_TICK);
            }
            asked.elapsed()
        };
        // An independent witness, so the two moments that matter are stamped
        // by neither road under test: when `adb` LISTS the serial again, and
        // when the device can actually show something. They are not the same
        // moment, which is the whole finding here.
        let stand_up = |adb: PathBuf| {
            let standing = crate::proc::quiet_command(&emulator)
                .args([
                    "-avd",
                    &avd,
                    "-no-window",
                    "-no-boot-anim",
                    // The probe copy is cold-booted on purpose: both rounds
                    // must stand the device up the same way, and a snapshot
                    // the first round wrote would hand the second a head
                    // start. Spelled as the one constant the launch reads.
                    NO_SNAPSHOT_FLAG,
                    "-port",
                    &port,
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the AVD stands back up");
            let seen = Arc::new(Mutex::new((None, None)));
            let stamp = Arc::clone(&seen);
            let serial = serial.clone();
            let watching = std::thread::spawn(move || {
                let asked = Instant::now();
                while device_presence(&adb, &serial) == DevicePresence::Gone {
                    assert!(asked.elapsed() < BOOT_TIMEOUT, "it never came back");
                    std::thread::sleep(WITNESS_TICK);
                }
                held(&stamp).0 = Some(Instant::now());
                // "Able to show something", asked the way the FRAME road
                // asks it, because it is the one readiness probe that does
                // not disturb the mirror: a second `scrcpy` on one device
                // pushes over the jar the road under test is using.
                // `sys.boot_completed` was tried first and is the wrong
                // witness — measured, a mirror opens BEFORE the framework
                // declares the boot done, so every gap came out as zero.
                loop {
                    let showing = crate::proc::quiet_command(&adb)
                        .args(["-s", &serial, "exec-out", "screencap", "-p"])
                        .output()
                        .ok()
                        .filter(|answer| answer.status.success())
                        .is_some_and(|answer| !answer.stdout.is_empty());
                    if showing {
                        held(&stamp).1 = Some(Instant::now());
                        return;
                    }
                    assert!(asked.elapsed() < BOOT_TIMEOUT, "it never woke up");
                    std::thread::sleep(WITNESS_TICK);
                }
            });
            (standing, seen, watching)
        };

        // ---- the road 1.1.11 shipped: push, size, encoder, a tenth of a second
        assert_eq!(
            device_presence(&adb, &serial),
            DevicePresence::OnTheBridge,
            "the device to be killed is not there to begin with"
        );
        // One turn of it, answering whether the mirror opened. The recorder
        // half is spawned and then collected rather than read: a LIVE
        // `screenrecord` fills its pipe and blocks for its whole time limit,
        // which the pump avoids by reading the pipe and which a measurement
        // has no use for.
        let hammer = || {
            let mirrored = crate::scrcpy::start(
                &adb,
                &serial,
                &jar,
                VIDEO_MAX_DIMENSION,
                ANDROID_STREAM_BIT_RATE,
            )
            .is_ok()
            // Stamped where the pump would write its "mirroring" line, not at
            // the end of the turn: the recorder half and the rest below it
            // are what this road does INSTEAD, and charging them to the
            // reattachment would flatter the road that replaces it.
            .then(Instant::now);
            let _ = bounded_video_size(&sdk, &serial);
            let born = crate::proc::quiet_command(&adb)
                .args([
                    "-s",
                    &serial,
                    "exec-out",
                    "screenrecord",
                    "--output-format=h264",
                    "--time-limit",
                    VIDEO_TIME_LIMIT_SECONDS,
                    "-",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = born {
                let until = Instant::now() + VIDEO_RESTART_INTERVAL;
                while Instant::now() < until && matches!(child.try_wait(), Ok(None)) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let _ = child.kill();
                let _ = child.wait();
            }
            std::thread::sleep(VIDEO_RESTART_INTERVAL);
            mirrored
        };
        println!("LIVE: {serial} died in {:?}", kill(&adb));
        // What the pane cannot do while the device is away, measured on the
        // input door rather than argued about.
        let refused = Instant::now();
        let said = read_android_screen_size(&sdk, &serial).expect_err("a gone device has no size");
        let old_refusal = refused.elapsed();
        let mut hammered = 0u32;
        let until = Instant::now() + WATCHED;
        while Instant::now() < until {
            assert!(hammer().is_none(), "the device came back mid-measurement");
            hammered += 1;
        }
        // The frame road's own version of the same waste: a `screencap` at a
        // serial that is gone, once an idle interval, forever. Its rested
        // replacement is the loop measured below — the two pumps share one
        // watch and one rest, so it is counted once.
        let mut photographed = 0u32;
        let until = Instant::now() + WATCHED;
        while Instant::now() < until {
            let born = crate::proc::quiet_command(&adb)
                .args(["-s", &serial, "exec-out", "screencap", "-p"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(mut child) = born {
                let _ = child.wait();
            }
            photographed += 1;
            std::thread::sleep(IDLE_CAPTURE_INTERVAL);
        }
        let (mut standing, seen, watching) = stand_up(adb.clone());
        let mut mirror = None;
        while mirror.is_none() {
            mirror = hammer();
        }
        watching.join().expect("the witness");
        let mirror = mirror.expect("a mirror");
        let (listed, ready) = *held(&seen);
        let old_dark = (
            mirror - listed.expect("a return"),
            mirror.saturating_duration_since(ready.expect("a boot")),
        );
        let _ = standing.kill();
        let _ = standing.wait();

        // ---- the road this lands: one reading, then a rest
        println!("LIVE: {serial} died in {:?}", kill(&adb));
        let refused = Instant::now();
        let new_refusal_says = (device_presence(&adb, &serial) == DevicePresence::Gone)
            .then(|| EmulatorNoteCode::DeviceOffline.code());
        let new_refusal = refused.elapsed();
        let mut looked = 0u32;
        let until = Instant::now() + WATCHED;
        while Instant::now() < until {
            assert_eq!(
                device_presence(&adb, &serial),
                DevicePresence::Gone,
                "the device came back mid-measurement"
            );
            looked += 1;
            std::thread::sleep(DEVICE_ABSENT_REST);
        }
        let (mut standing, seen, watching) = stand_up(adb.clone());
        // The pump's own shape: a device that is not there is rested on, and
        // one that is there but gives nothing back is rested on too — for a
        // named interval each, rather than a tenth of a second.
        let mirror = loop {
            if device_presence(&adb, &serial) == DevicePresence::Gone {
                std::thread::sleep(DEVICE_ABSENT_REST);
                continue;
            }
            if crate::scrcpy::start(
                &adb,
                &serial,
                &jar,
                VIDEO_MAX_DIMENSION,
                ANDROID_STREAM_BIT_RATE,
            )
            .is_ok()
            {
                break Instant::now();
            }
            std::thread::sleep(DEVICE_WAKING_REST);
        };
        watching.join().expect("the witness");
        let (listed, ready) = *held(&seen);
        let new_dark = (
            mirror - listed.expect("a return"),
            mirror.saturating_duration_since(ready.expect("a boot")),
        );
        let _ = standing.kill();
        let _ = standing.wait();

        let rate = |count: u32| f64::from(count) / WATCHED.as_secs_f64();
        println!("LIVE: ---- t-5761, {serial}, {WATCHED:?} absent, witness every {WITNESS_TICK:?}");
        println!(
            "LIVE: mirror tries a second        1.1.11 {:.1}   rested {:.1}",
            rate(hammered),
            rate(looked)
        );
        println!(
            "LIVE: frame tries a second         1.1.11 {:.1}   rested {:.1} (the same one look)",
            rate(photographed),
            rate(looked)
        );
        let ms = |gap: Duration| gap.as_secs_f64() * 1000.0;
        println!(
            "LIVE: dark after adb lists it      1.1.11 {:.0} ms   rested {:.0} ms",
            ms(old_dark.0),
            ms(new_dark.0)
        );
        // Zero here is the answer, not a missing one: the mirror was already
        // up by the time an independent probe got its first picture out of
        // the device. Anything else is the pane waiting on its own rests
        // after the device was ready to serve, which is what the three
        // seconds are a ceiling on.
        println!(
            "LIVE: behind the first picture     1.1.11 {:.0} ms   rested {:.0} ms",
            ms(old_dark.1),
            ms(new_dark.1)
        );
        println!(
            "LIVE: an input while absent         1.1.11 {:.0} ms {said:?}   rested {:.0} ms {new_refusal_says:?}",
            old_refusal.as_secs_f64() * 1000.0,
            new_refusal.as_secs_f64() * 1000.0
        );
        assert!(
            rate(looked) <= 1.0,
            "the rested road is looking more than once a second"
        );
        assert!(
            new_dark.1 <= Duration::from_secs(3),
            "the pane stays dark longer than the three seconds it is given \
             after the device can show something"
        );
        assert_eq!(new_refusal_says, Some("device-offline"));
    }

    #[test]
    fn a_pre_spawn_intent_round_trips_without_a_pid_or_serial() {
        let root = tempfile::tempdir().expect("local data root");
        let record = managed_record('e');
        let bytes = serde_json::to_vec(&ManagedEmulatorFile {
            version: MANAGED_FILE_VERSION,
            devices: vec![record.clone()],
            last_used: None,
        })
        .expect("owner json");
        crate::durable_file::replace_bytes(&managed_file(root.path()), &bytes)
            .expect("durable owner");
        assert_eq!(read_managed_records(root.path()).unwrap(), vec![record]);
    }
}
