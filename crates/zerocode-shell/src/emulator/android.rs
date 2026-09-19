//! Android SDK discovery, AVD lifecycle, streams and input commands.

mod accessibility;
mod capabilities;
mod display;

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
const MANAGED_PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(250);
const MANAGED_FILE_NAME: &str = "android-emulators.json";
const MANAGED_FILE_VERSION: u32 = 1;
const MANAGED_FILE_MAX_BYTES: u64 = 64 * 1024;
const MANAGED_DEVICE_MAX: usize = 32;
const MANAGED_PROPERTY: &str = "qemu.zerocode.managed";

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

#[derive(Debug, Deserialize, Serialize)]
struct ManagedEmulatorFile {
    version: u32,
    devices: Vec<ManagedEmulatorRecord>,
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

    fn stop(&self) -> bool {
        self.launching.store(false, Ordering::Release);
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

fn valid_managed_record(record: &ManagedEmulatorRecord) -> bool {
    record.token.len() == 48
        && record.token.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !record.avd.is_empty()
        && record.avd.len() <= 256
        && !record
            .avd
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
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

fn read_managed_records(local_data_root: &Path) -> Result<Vec<ManagedEmulatorRecord>, String> {
    let file = managed_file(local_data_root);
    let bytes = match crate::durable_file::read_plain_file(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    if bytes.len() as u64 > MANAGED_FILE_MAX_BYTES {
        return Err("Android 에뮬레이터 소유권 파일이 올바르지 않습니다".to_string());
    }
    let parsed: ManagedEmulatorFile =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if parsed.version != MANAGED_FILE_VERSION || parsed.devices.len() > MANAGED_DEVICE_MAX {
        return Err("Android 에뮬레이터 소유권 파일 판번호가 올바르지 않습니다".to_string());
    }
    Ok(parsed
        .devices
        .into_iter()
        .filter(valid_managed_record)
        .collect())
}

fn write_managed_records_locked(local_data_root: &Path) -> Result<(), String> {
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
    let bytes = serde_json::to_vec(&ManagedEmulatorFile {
        version: MANAGED_FILE_VERSION,
        devices,
    })
    .map_err(|error| error.to_string())?;
    crate::durable_file::replace_bytes(&managed_file(local_data_root), &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn write_managed_records(local_data_root: &Path) -> Result<(), String> {
    let _store = held(&MANAGED_STORE_GATE);
    write_managed_records_locked(local_data_root)
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

pub(super) fn shutdown_all_devices() {
    MANAGED_SHUTTING_DOWN.store(true, Ordering::Release);
    let processes = held(managed_devices())
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for process in processes {
        if process.stop() {
            forget_managed_process(&process);
        }
    }
}

#[derive(Clone, Debug)]
struct AndroidSdk {
    root: PathBuf,
    adb: PathBuf,
    emulator: PathBuf,
}

fn executable(name: &str) -> OsString {
    let mut name = OsString::from(name);
    name.push(std::env::consts::EXE_SUFFIX);
    name
}

fn sdk_at(root: PathBuf) -> Option<AndroidSdk> {
    if !root.is_absolute() {
        return None;
    }
    let adb = root.join("platform-tools").join(executable("adb"));
    let emulator = root.join("emulator").join(executable("emulator"));
    (adb.is_file() && emulator.is_file()).then_some(AndroidSdk {
        root,
        adb,
        emulator,
    })
}

fn path_binary(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(executable(name)))
        .find(|candidate| candidate.is_file())
}

fn android_sdk_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for variable in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(path) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(path));
        }
    }
    if let Some(home) = dirs::home_dir() {
        #[cfg(target_os = "macos")]
        candidates.push(home.join("Library/Android/sdk"));
        #[cfg(target_os = "linux")]
        candidates.push(home.join("Android/Sdk"));
    }
    #[cfg(target_os = "windows")]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Android/Sdk"));
    }
    if let Some(adb) = path_binary("adb")
        && let Some(root) = adb.parent().and_then(Path::parent)
    {
        candidates.push(root.to_path_buf());
    }
    if let Some(emulator) = path_binary("emulator")
        && let Some(root) = emulator.parent().and_then(Path::parent)
    {
        candidates.push(root.to_path_buf());
    }
    let mut seen = HashSet::new();
    candidates.retain(|candidate| seen.insert(candidate.clone()));
    candidates
}

fn android_sdk() -> Option<AndroidSdk> {
    android_sdk_candidates().into_iter().find_map(sdk_at)
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
            if state != "device" || !serial.starts_with("emulator-") {
                return None;
            }
            let avd = crate::proc::quiet_command(adb)
                .args(["-s", serial, "emu", "avd", "name"])
                .output()
                .ok()
                .filter(|answer| answer.status.success())
                .and_then(|answer| {
                    String::from_utf8_lossy(&answer.stdout)
                        .lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty() && *line != "OK")
                        .map(str::to_string)
                })
                .unwrap_or_default();
            Some((serial.to_string(), avd))
        })
        .collect()
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

fn list_android_devices() -> Vec<AndroidDevice> {
    let Some(sdk) = android_sdk() else {
        return Vec::new();
    };
    let running = android_running(&sdk.adb);
    let Ok(out) = crate::proc::quiet_command(&sdk.emulator)
        .arg("-list-avds")
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut devices = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let avd = line.trim();
        let prefix = avd.split_whitespace().next();
        if avd.is_empty()
            || avd.starts_with("No AVD")
            || matches!(
                prefix,
                Some("INFO" | "WARNING" | "ERROR" | "DEBUG" | "VERBOSE" | "PANIC")
            )
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
    devices
}

fn android_serial_is_live(adb: &Path, serial: &str) -> bool {
    android_running(adb).iter().any(|(live, _)| live == serial)
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
    .map_err(|error| error.to_string())
}

pub(crate) async fn android_emulators_direct() -> Result<Vec<AndroidDevice>, String> {
    tauri::async_runtime::spawn_blocking(list_android_devices)
        .await
        .map_err(|error| error.to_string())
}

/// One lossless frame from the same adb device the built-in pane mirrors.
/// Bytes stay in-process until the authenticated CLI route has checked and
/// written them, so no partial destination is reported as a screenshot.
pub(crate) async fn android_screenshot_direct(serial: String) -> Result<Vec<u8>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sdk = android_sdk().ok_or("Android SDK(adb)를 찾을 수 없습니다")?;
        if !android_serial_is_live(&sdk.adb, &serial) {
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
        // The intent reaches durable storage before the process exists. Its
        // unguessable token is also placed on the emulator command line, so a
        // restart can recover the pid even if this process dies immediately
        // after `spawn` and before the pid/start pair is committed.
        let managed = begin_managed_launch(&local_data_root, &chosen.avd)?;
        let marker = format!("{MANAGED_PROPERTY}={}", managed.record().token);
        let mut boot = crate::proc::quiet_command(&sdk.emulator);
        boot.args([
            "-avd",
            &chosen.avd,
            "-no-window",
            "-no-snapshot",
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
        let found = list_android_devices()
            .into_iter()
            .find(|device| device.avd == chosen.avd)
            .and_then(|device| device.serial);
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
    let frame = capture_path(&stream);
    if let Some(parent) = frame.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut misses = pump::MissCounter::new(MISSES_BEFORE_NOTE);
    let mut unchanged = 0u32;
    let mut last_fingerprint = None;
    let mut last_emitted = None;
    while control.wait_until_running() {
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
        if !wait_for_child(&control, CAPTURE_TIMEOUT) {
            if control.is_alive() && !control.is_paused() {
                if misses.missed() {
                    emit_note(&app, &stream, EmulatorNoteCode::FrameUnavailable);
                }
                control.rest(IDLE_CAPTURE_INTERVAL);
            }
            continue;
        }
        let Ok(bytes) = read_bounded(&frame) else {
            misses.missed();
            control.rest(IDLE_CAPTURE_INTERVAL);
            continue;
        };
        misses.hit();
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
    tauri::async_runtime::spawn_blocking(move || {
        let sdk = android_sdk()
            .ok_or("Android SDK를 찾지 못했습니다 — Android Studio의 SDK를 설치하세요")?;
        reconcile_managed_devices_now(app.state::<crate::AppState>().local_data_root());
        let chosen = selected_android_device(&list_android_devices(), avd.as_deref())?;
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
    while control.wait_until_running() {
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
        loop {
            let Ok(read) = output.read(&mut bytes) else {
                break;
            };
            if read == 0 || !control.is_alive() || control.is_paused() {
                break;
            }
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
            control.rest(VIDEO_RESTART_INTERVAL);
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
        let sdk = android_sdk().ok_or_else(|| {
            crate::note_window_event(&husk_root, "emulator video refused: no android sdk");
            "Android SDK가 없습니다".to_string()
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

fn android_control(serial: &str) -> Result<(AndroidSdk, Arc<SessionControl>), String> {
    let sdk = android_sdk().ok_or("Android SDK가 없습니다")?;
    let control = registry()
        .target_control(EmulatorPlatform::Android, serial)
        .filter(|control| control.is_alive())
        .ok_or("이 Android 에뮬레이터 스트림은 실행 중이 아닙니다")?;
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
) -> Result<super::marks::Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (sdk, _control) = android_control(&serial)?;
        marks_snapshot(&sdk, &serial).map(|(snapshot, _)| snapshot)
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
        request.perform_in(&input, &snapshot.faces, snapshot.screen, |x, y| {
            tap_at(&sdk, &serial, x, y, size).map_err(backend_error)
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
        let sdk = android_sdk().ok_or("Android SDK가 없습니다")?;
        if !android_serial_is_live(&sdk.adb, &serial) {
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
    fn configured_sdk_paths_come_before_host_defaults() {
        let candidates = android_sdk_candidates();
        if let Some(configured) = std::env::var_os("ANDROID_HOME") {
            assert_eq!(candidates.first(), Some(&PathBuf::from(configured)));
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
    fn a_pre_spawn_intent_round_trips_without_a_pid_or_serial() {
        let root = tempfile::tempdir().expect("local data root");
        let record = managed_record('e');
        let bytes = serde_json::to_vec(&ManagedEmulatorFile {
            version: MANAGED_FILE_VERSION,
            devices: vec![record.clone()],
        })
        .expect("owner json");
        crate::durable_file::replace_bytes(&managed_file(root.path()), &bytes)
            .expect("durable owner");
        assert_eq!(read_managed_records(root.path()).unwrap(), vec![record]);
    }
}
