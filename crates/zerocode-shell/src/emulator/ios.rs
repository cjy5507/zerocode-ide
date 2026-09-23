//! iOS Simulator discovery, lifecycle, frame capture and input commands.

mod capabilities;
mod keeping;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::session::{FinishSession, SessionControl, SessionKey, StartClaim, registry};
use super::{
    BinaryChannel, EmitOutcome, EmulatorNoteCode, EmulatorPlatform, EmulatorStream, SlotWait,
    Viewport, emit_bytes, emit_note, pump, read_bounded, wait_for_child,
};

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(4);
const IDLE_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const MISSES_BEFORE_NOTE: u32 = 2;

/// The ladder the POLLING roads climb while a screen stays still.
///
/// Only the two fallback roads below ever climb it. The helper's push road does
/// not poll at all — it is TOLD when the framebuffer moves — so neither the
/// reason for the ladder (a still screen must not cost sixty captures a second)
/// nor its price (a change arriving mid-rest waits out the rest, up to 675ms)
/// applies there. That price is exactly what a person felt as "화면이 늦게
/// 바뀜" whenever the screen moved without their finger causing it.
const FALLBACK_IDLE_LADDER: pump::IdleLadder = pump::IdleLadder {
    step: Duration::from_millis(75),
    ceiling: Duration::from_millis(900),
    steps: 8,
};

/// How long a polling road rests after a capture that answered nothing at all.
const FALLBACK_MISS_REST: Duration = Duration::from_millis(900);

/// The same rest, while the device is still coming up.
///
/// The full rest is for a road that answered nothing about a device that CAN
/// draw — a wedged simulator, a screenshot that timed out — where trying again
/// at once would spend the pane's whole existence on failures that are not
/// going to stop. A booting device is a different question with a different
/// answer already on its way: measured on this machine, the first picture a
/// shut-down device can give arrives within a second of `simctl boot` being
/// accepted, so a 900ms rest is up to 900ms of blank pane AFTER the boot logo
/// exists.
const BOOTING_MISS_REST: Duration = Duration::from_millis(250);

/// The gap the POLLING roads leave between frames when the work itself was
/// quick.
///
/// This was once the only thing deciding the pane's rate, back when every road
/// was a road the pump had to walk itself: a CoreSimulator still picture costs
/// 130-231ms whatever format it is asked for, and reading the Simulator's own
/// window costs 27-28ms for a phone-shaped window. Neither can reach 16ms, so
/// for them this is a ceiling in name only — it is the floor under a road that
/// is already slower than it.
///
/// It no longer paces the road the pane actually uses. The helper pushes when
/// the framebuffer moves and the pump waits on a socket rather than a clock, so
/// what limits the fast road now is `FRAME_MAX_FPS` at the helper's end and the
/// renderer's own appetite at ours.
const FALLBACK_FRAME_CEILING: Duration = Duration::from_millis(16);

/// The quality the helper's JPEG encoder is set to, from 0 to 1.
///
/// This was 0.5, chosen when the capture was the expensive half and every byte
/// saved was a millisecond saved. It is not any more — the helper encodes from a
/// framebuffer it already holds — and 0.5 is visibly lossy on exactly the
/// content a phone screen is made of: text, icon edges, thin strokes. The bytes
/// this costs are bytes the push socket and the raw IPC channel were already
/// built to carry.
const FRAME_JPEG_QUALITY: f64 = 0.82;

/// The most pictures a second the helper is allowed to push while the pane is
/// being USED — somebody's pointer over the screen or keyboard focus in it,
/// or a device input (a person's touch, an agent's `zerocode-emulator tap`)
/// within the last [`INPUT_BOOST_WINDOW`].
///
/// Above the 30 Orca's own stream tops out at (`MAX_FPS = 30`,
/// mjpeg-frame-stream.ts), because the road can afford it while it is
/// wanted: the helper's encode is 1.92ms, and a person driving the device
/// should see it as smoothly as the Simulator itself shows it.
const FRAME_MAX_FPS_ACTIVE: u32 = 60;
/// The rate for a pane merely in view — the mirror glanced at while the
/// terminal beside it is read.
///
/// Measured on 2026-09-02 with an iPhone Air at 1122x2436 and iOS 26's
/// animated wallpaper (16% of pixels change every 150ms, so nothing is ever
/// "still" and the seed road never rests): at the full rate the helper
/// delivered 42-50fps and the window paid WebKit.GPU 31%, WindowServer 40%,
/// WebContent 18% for a mirror nobody was touching — typing in the terminal
/// beside it stuttered. Motion is still seen at this rate, at a fifth of that
/// bill; the moment the pointer enters or the device is poked, the active
/// rate is back within one wait.
const FRAME_MAX_FPS_GLANCE: u32 = 12;
/// How long after a device input the pane stays at the active rate: long
/// enough for the transition the input started, and for the next input of a
/// sequence — a typed word, a swipe through a list — to keep it there.
const INPUT_BOOST_WINDOW: Duration = Duration::from_millis(2500);

/// The rate the helper is told, from whether the pane is being used.
const fn frame_rate_for(active: bool) -> u32 {
    if active {
        FRAME_MAX_FPS_ACTIVE
    } else {
        FRAME_MAX_FPS_GLANCE
    }
}

/// How long the pump waits on the push socket before it looks around.
///
/// Not a frame interval — a still screen legitimately pushes nothing for
/// minutes. It is how often a pump on the fast road re-reads the things only it
/// can act on: a pause, a resize, a stream that has died.
const FRAME_WAIT_TIMEOUT: Duration = Duration::from_millis(250);

/// The long edge a pane gets before it has said how big it is.
///
/// A first picture that is slightly too small is a pane that sharpens a
/// heartbeat later; a first picture sized for the largest possible pane is
/// bytes every pane pays for. The window reports its real size as soon as it
/// has laid the screen out.
const DEFAULT_LONG_EDGE_PX: u32 = 1024;

/// The smallest long edge worth encoding at, whatever a pane claims.
///
/// A pane can report absurdly small numbers mid-layout — a split being dragged
/// shut, a tab that has not been measured yet — and encoding a 12-pixel picture
/// for one of those frames makes the pane visibly flash.
const MIN_LONG_EDGE_PX: u32 = 320;

/// The largest long edge asked for, however big the pane grows.
///
/// The tallest iPhone framebuffer measured here is 2736 and the helper never
/// upscales past the device's own size, so this is not about the device — it is
/// the point past which more pixels cost real bytes for detail a mirror of a
/// phone does not need.
const MAX_LONG_EDGE_PX: u32 = 2436;

/// Long edges are rounded up to a multiple of this before the helper is told.
///
/// The helper's `VTCompressionSession` is built for one exact size and thrown
/// away when that size changes, so a pane being dragged would otherwise destroy
/// and rebuild the encoder on every mouse move. Quantising means a drag crosses
/// a handful of sizes instead of hundreds.
const LONG_EDGE_QUANTUM_PX: u32 = 64;

/// How many moving frames one rate line covers.
///
/// Long enough that a single slow frame does not become a headline, short
/// enough that a pane that went bad is written down within a couple of
/// seconds of continuous motion.
const FRAMES_PER_RATE_WORD: u32 = 120;

/// How often the pump looks for the Simulator's own window while it does not
/// have one.
///
/// A window listing costs ~50ms on this machine and its answer only changes
/// when somebody opens, closes, hides or shows a window — so asking every
/// frame would spend a fifth of the slow road's own budget re-learning the
/// same "no". Once a second is fast enough that unhiding the Simulator is
/// felt as an immediate change.
const WINDOW_LOOK_EVERY: Duration = Duration::from_secs(1);

/// How long a boot this pane asked for is waited on before the pane stops
/// waiting and shows whatever the device has.
///
/// The cap is for the boot that is not coming, not for the slow one: measured
/// on this machine, an already-booted device answers `bootstatus` in 0.18s and
/// a warm cold boot in 9.6s, while a device meeting its setup assistant for
/// the first time spends minutes — and minutes of "the device is waking" is a
/// true sentence, where five of them is a pane nobody should still be looking
/// at.
const BOOT_WAIT: Duration = Duration::from_secs(300);

/// How often the helper is asked for again while the device is still coming
/// up.
///
/// The pane no longer holds its first picture until the boot is over (D5), so
/// the roads below draw the Simulator's own boot logo from the moment
/// CoreSimulator accepts the boot. What a booting device cannot yet give is
/// the HELPER — it loads the active Xcode's SimulatorKit against a framebuffer
/// that does not exist yet — and the doubling backoff that covers a machine
/// with no helper at all would answer a nine-second boot by first asking again
/// at three seconds, then at nine, then at twenty-one. This is the beat for
/// the one case where the next try is genuinely likely to be the one that
/// works, at one cold start (247-495ms) per try.
const HELPER_ATTACH_RETRY: Duration = Duration::from_millis(500);

/// The longest the capability thread ever waits between cold starts, once the
/// device is up and the helper still has not answered.
const HELPER_ATTACH_CEILING: Duration = Duration::from_secs(60);

/// One `simctl` invocation, however this machine reaches it.
///
/// `xcrun` resolves its tool with a real process walk on every call, and the
/// frame pump used to pay that walk thirty times a second — half of the
/// "Orca는 바로 떠" gap once the nap was gone. Resolved once; a machine where
/// `--find` answers nothing keeps the plain `xcrun simctl` spelling and fails
/// exactly where it always failed.
pub(crate) fn simctl_command() -> Command {
    static FOUND: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    let words = FOUND.get_or_init(|| {
        let found = crate::proc::quiet_command("xcrun")
            .args(["--find", "simctl"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|path| !path.is_empty() && std::path::Path::new(path).is_file());
        match found {
            Some(path) => vec![path],
            None => vec!["xcrun".to_string(), "simctl".to_string()],
        }
    });
    let mut command = crate::proc::quiet_command(&words[0]);
    command.args(&words[1..]);
    command
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SimulatorDevice {
    udid: String,
    name: String,
    booted: bool,
    runtime: String,
}

#[derive(Deserialize)]
struct SimctlList {
    #[serde(default)]
    devices: BTreeMap<String, Vec<SimctlDevice>>,
}

#[derive(Deserialize)]
struct SimctlDevice {
    udid: String,
    name: String,
    #[serde(default)]
    state: String,
    #[serde(default, rename = "isAvailable")]
    is_available: Option<bool>,
}

pub(super) fn list_ios_simulators() -> Vec<SimulatorDevice> {
    let Ok(out) = simctl_command()
        .args(["list", "devices", "available", "--json"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let Ok(doc) = serde_json::from_slice::<SimctlList>(&out.stdout) else {
        return Vec::new();
    };
    doc.devices
        .into_iter()
        .flat_map(|(runtime, devices)| {
            devices.into_iter().filter_map(move |device| {
                if device.is_available == Some(false)
                    || device.udid.trim().is_empty()
                    || device.name.trim().is_empty()
                {
                    return None;
                }
                Some(SimulatorDevice {
                    udid: device.udid,
                    name: device.name,
                    booted: device.state == "Booted",
                    runtime: runtime.clone(),
                })
            })
        })
        .collect()
}

fn pick_default_simulator(devices: &[SimulatorDevice]) -> Option<&SimulatorDevice> {
    let iphone = |device: &&SimulatorDevice| device.name.to_lowercase().contains("iphone");
    devices
        .iter()
        .filter(|device| device.booted)
        .find(iphone)
        .or_else(|| devices.iter().find(|device| device.booted))
        .or_else(|| devices.iter().find(iphone))
        .or_else(|| devices.first())
}

fn selected_simulator(
    devices: &[SimulatorDevice],
    requested: Option<&str>,
) -> Result<SimulatorDevice, String> {
    match requested {
        Some(udid) => devices.iter().find(|device| device.udid == udid),
        None => pick_default_simulator(devices),
    }
    .cloned()
    .ok_or_else(|| {
        "사용 가능한 iOS 시뮬레이터가 없습니다 — Xcode Settings > Platforms에서 추가하세요"
            .to_string()
    })
}

/// Whether `simctl boot`'s refusal is the device saying it is already awake.
///
/// The only way to tell an already-running device from a real failure is the
/// state named in the refusal, and since the window preboots the last device
/// used (D4) that state is `Booting` as often as it is `Booted`: a pane opened
/// while the preboot is still in flight must attach to that boot rather than
/// failing in front of the person.
fn already_awake(said: &str) -> bool {
    ["current state: Booted", "current state: Booting"]
        .into_iter()
        .any(|state| said.contains(state))
}

fn boot_simulator(device: &SimulatorDevice) -> Result<(), String> {
    if device.booted {
        return Ok(());
    }
    let boot = simctl_command()
        .args(["boot", &device.udid])
        .output()
        .map_err(|error| error.to_string())?;
    let said = String::from_utf8_lossy(&boot.stderr);
    if !boot.status.success() && !already_awake(&said) {
        return Err(format!("시뮬레이터를 부팅할 수 없습니다: {}", said.trim()));
    }
    Ok(())
}

/// The one `simctl shutdown` in this window.
///
/// Both roads that end a simulator's day come through here: the person's own
/// 끄기 button ([`shutdown_mobile_emulator`], which checks first that the
/// device is one of this machine's) and the idle reclaimer, which has already
/// decided. A second spelling of this would be a second answer to "what does
/// an already-shut-down device mean", and the reclaimer meets that case every
/// time somebody shuts a device down by hand first.
fn shutdown_simulator(udid: &str) -> Result<(), String> {
    shutdown_simulator_by(simctl_command(), udid)?;
    // Down by one of this window's own roads: whoever it was lent to, it is
    // nobody's loan now.
    super::loan_put_down(EmulatorPlatform::Ios, udid);
    Ok(())
}

/// [`shutdown_simulator`] with its `simctl` handed in — the one place a fake
/// one can answer it.
fn shutdown_simulator_by(mut command: Command, udid: &str) -> Result<(), String> {
    command.args(["shutdown", udid]);
    // Bounded, because one caller is the window on its way out: a wedged
    // device — this module already knows of one that hangs `simctl io
    // screenshot` forever — must cost the exit a deadline rather than the
    // whole of it.
    let out = super::process::run_bounded(
        command,
        super::capability::SHORT_COMMAND_TIMEOUT,
        super::capability::COMMAND_OUTPUT_BYTES,
    )?;
    let said = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() && !said.contains("current state: Shutdown") {
        return Err(format!("시뮬레이터를 끌 수 없습니다: {}", said.trim()));
    }
    Ok(())
}

/// Ask Launch Services for the Simulator, without waiting for it to answer.
///
/// `open` returns when Launch Services has accepted the request, not when the
/// application is ready — so waiting on it buys nothing and costs 64-99ms on
/// this machine, measured. Nothing below reads its exit status: the pump finds
/// the device through CoreSimulator, and the window road looks for the window
/// once a second forever, so a launch that is still in flight is simply a
/// window that is not there yet.
#[cfg(target_os = "macos")]
fn prepare_simulator_services(udid: &str) {
    let _ = crate::proc::quiet_command("open")
        .args([
            "-gj",
            "-a",
            "Simulator",
            "--args",
            "-CurrentDeviceUDID",
            udid,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(not(target_os = "macos"))]
fn prepare_simulator_services(_udid: &str) {}

#[tauri::command]
pub(crate) async fn mobile_emulators(
    webview: tauri::Webview,
) -> Result<Vec<SimulatorDevice>, String> {
    crate::from_the_main_webview(&webview)?;
    mobile_emulators_direct().await
}

pub(crate) async fn mobile_emulators_direct() -> Result<Vec<SimulatorDevice>, String> {
    tauri::async_runtime::spawn_blocking(list_ios_simulators)
        .await
        .map_err(|error| error.to_string())
}

/// One lossless CoreSimulator frame. This is the slow fallback already used
/// by the built-in pane, requested as PNG so the agent receives exact pixels.
pub(crate) async fn ios_screenshot_direct(udid: String) -> Result<Vec<u8>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let directory = std::env::temp_dir().join("zerocode-emulator");
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let frame = directory.join(format!("agent-{}.png", uuid::Uuid::new_v4()));
        let output = simctl_command()
            .args(["io", &udid, "screenshot", "--type=png"])
            .arg(&frame)
            .output()
            .map_err(|error| format!("iOS screenshot could not start: {error}"))?;
        let answer = if output.status.success() {
            super::read_bounded(&frame)
        } else {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if detail.is_empty() {
                "iOS screenshot failed".to_string()
            } else {
                format!("iOS screenshot failed: {detail}")
            })
        };
        let _ = std::fs::remove_file(frame);
        answer
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The size to ask the helper for, from the size the pane says it can paint.
///
/// Deliberately does NOT clamp against the device's own framebuffer: the helper
/// already refuses to upscale (`scale` is 1.0 whenever the ask is at or above
/// the source's longest edge, main.swift), so a second clamp here would be a
/// second place to keep that rule correct and the first one to drift.
fn resolve_long_edge(asked: Option<u32>) -> u32 {
    asked
        .unwrap_or(DEFAULT_LONG_EDGE_PX)
        .clamp(MIN_LONG_EDGE_PX, MAX_LONG_EDGE_PX)
        .div_ceil(LONG_EDGE_QUANTUM_PX)
        .saturating_mul(LONG_EDGE_QUANTUM_PX)
        .min(MAX_LONG_EDGE_PX)
}

fn capture_path(stream: &str) -> PathBuf {
    std::env::temp_dir()
        .join("zerocode-emulator")
        .join(format!("{stream}.jpg"))
}

/// One still picture through CoreSimulator, downscaled for the pane.
///
/// The slow road, and the only one that works everywhere: measured at
/// 130-230ms whatever format is asked for. Full-res frames are half a
/// megabyte and the pane is not; `sips` is the platform's own scaler (~75ms
/// measured) and its failure just sends the full frame. A function of its
/// own so the pump can be about CHOOSING a road rather than being one.
fn screenshot_picture(
    udid: &str,
    frame: &std::path::Path,
    control: &SessionControl,
) -> Result<Option<Vec<u8>>, ()> {
    let _ = std::fs::remove_file(frame);
    let child = simctl_command()
        .args(["io", udid, "screenshot", "--type=jpeg"])
        .arg(frame)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(child) = child else {
        // A road that cannot even be walked ends the pump instead of
        // spinning on a spawn that will fail again immediately.
        return Err(());
    };
    if !control.install_child(child) {
        return Ok(None);
    }
    if !wait_for_child(control, CAPTURE_TIMEOUT) {
        return Ok(None);
    }
    let _ = crate::proc::quiet_command("sips")
        .args(["-Z", "900"])
        .arg(frame)
        .arg("--out")
        .arg(frame)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(read_bounded(frame).ok())
}

/// Whether the device this pane opened is still on its way up.
///
/// Two roads need the answer and neither can ask CoreSimulator cheaply: the
/// pump's framebuffer road, whose failures are only worth resting it for once
/// the device can actually draw, and the capability thread, whose cold start
/// cannot succeed before then either. One watch, two readers, and the
/// `bootstatus` child that answers it runs beside the pictures rather than in
/// front of them.
struct BootWatch {
    finished: std::sync::atomic::AtomicBool,
}

impl BootWatch {
    /// A device that was already running. Nothing is waited for, and no road
    /// gets to blame a boot for failing.
    fn already_up() -> Arc<Self> {
        Arc::new(Self {
            finished: std::sync::atomic::AtomicBool::new(true),
        })
    }

    fn waking() -> Arc<Self> {
        Arc::new(Self {
            finished: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn booting(&self) -> bool {
        !self.finished.load(std::sync::atomic::Ordering::Acquire)
    }

    fn finished(&self) {
        self.finished
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

/// Time the device's boot WITHOUT standing in the first picture's way.
///
/// Until 2026-09-21 this waited: `bootstatus` ran to the end and only then was
/// a road opened, because every road can capture a boot screen and none of
/// them can tell one from a device — measured, that first picture is 6.6KB of
/// black with a four-pixel spinner, and the pane would have called itself
/// connected over it.
///
/// What that traded away is the nine seconds the person spends looking at a
/// pane with nothing in it ("둘 다 1초 때에 바로 부팅되게 할 순 없나"), while
/// the Simulator's own window — the same device, the same CoreSimulator —
/// shows an Apple logo and a progress ring the whole time. That logo IS the
/// honest picture of a device that is booting, so the roads now draw it and
/// this keeps only the part that was never in the way: the measurement, for
/// the one line a person reading the log wants, and the flag the two roads
/// above read to tell "not yet" from "broken".
///
/// The child is owned here rather than handed to [`SessionControl`]: that slot
/// holds ONE child and the still-picture road needs it for every frame it
/// takes, which is exactly the road that draws the boot logo.
#[cfg(target_os = "macos")]
fn watch_the_device_boot(
    udid: &str,
    control: &Arc<SessionControl>,
    husk_root: &Path,
    stream: &str,
) -> Arc<BootWatch> {
    let watch = BootWatch::waking();
    let watching = watch.clone();
    let control = control.clone();
    let husk_root = husk_root.to_path_buf();
    let named = stream.to_string();
    let device = udid.to_string();
    if std::thread::Builder::new()
        .name(format!("ios-boot-{stream}"))
        .spawn(move || {
            let started = Instant::now();
            let ready = match simctl_command()
                .args(["bootstatus", &device])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(mut child) => wait_beside(&mut child, &control),
                // A machine that cannot even spawn it has no second opinion
                // about this boot, and nothing above may keep calling the
                // device "not yet" on the strength of a question nobody asked.
                Err(_) => {
                    watching.finished();
                    return;
                }
            };
            watching.finished();
            crate::note_window_event(
                &husk_root,
                &format!(
                    "emulator ios pump {named}: waited {}ms for the device to finish booting ({})",
                    started.elapsed().as_millis(),
                    if ready { "ready" } else { "gave up" }
                ),
            );
        })
        .is_err()
    {
        watch.finished();
    }
    watch
}

#[cfg(not(target_os = "macos"))]
fn watch_the_device_boot(
    _udid: &str,
    _control: &Arc<SessionControl>,
    _husk_root: &Path,
    _stream: &str,
) -> Arc<BootWatch> {
    BootWatch::already_up()
}

/// Wait for a child this thread owns, giving up at [`BOOT_WAIT`] or as soon as
/// the stream it belongs to ends.
///
/// The cap is for the boot that is not coming, not for the slow one: measured
/// on this machine, an already-booted device answers `bootstatus` in 0.18s and
/// a warm cold boot in 8.8-9.6s, while a device meeting its setup assistant
/// for the first time spends minutes.
#[cfg(target_os = "macos")]
fn wait_beside(child: &mut std::process::Child, control: &SessionControl) -> bool {
    let deadline = Instant::now() + BOOT_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if control.is_alive() && Instant::now() < deadline => {
                std::thread::sleep(super::PROCESS_POLL_INTERVAL);
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// One picture off the helper, with the size it was actually encoded at.
struct HelperPicture {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    /// The framebuffer generation this was taken at. Only ever reported, never
    /// compared — the helper owns the comparison now — but it is the number
    /// that tells a live surface from a cached dead one (main.swift warns that
    /// a surface already held keeps answering its last pixels forever), so the
    /// one line this road writes carries it.
    seed: u32,
}

/// What the helper had to say about the screen this turn.
enum HelperFrameAnswer {
    /// This machine has no helper road — the roads below answer instead.
    Absent,
    /// The push socket is live and the framebuffer has not moved. Nothing to
    /// send, and nothing for the slower roads to do either: they would only
    /// re-capture the screen this road is already watching for free.
    Still,
    Picture(HelperPicture),
}

/// Consecutive helper failures before the pump rests the road.
const HELPER_MISSES_BEFORE_GIVING_UP: u32 = 3;

/// How long a rested helper road waits before it is tried as if new.
const HELPER_ROAD_RETRY: Duration = Duration::from_secs(5);

/// The helper's push road, and what the pump remembers about it.
///
/// A road is rested rather than retried every frame because the two below it
/// work, and a helper that cannot start will not start on the next frame
/// either — but a single failure is a hiccup, not an answer, so it takes a few.
/// A rest, not a verdict: a helper can ARRIVE late — the capability thread
/// retries its cold start behind the pictures — and when this was once a
/// permanent `Gone` it parked the pane on the still-picture road (4-5fps on a
/// machine with the Simulator window hidden) for the life of the stream. That
/// floor is where "iOS 속도가 너무 느리고 화면이동시 파란색" lives: at five
/// frames a second a transition is one or two of the device's own blur frames,
/// each standing for hundreds of milliseconds.
struct HelperRoad {
    /// The long edge and the rate the helper was last told to push at, and
    /// `None` when it is not pushing. Comparing the pane's current ask against
    /// this is what turns a resize — or a pointer entering — into exactly one
    /// re-negotiation instead of one per frame.
    streaming_at: Option<(u32, u32)>,
    /// Where this road stands between one picture and the next.
    seam: Seam,
    misses: u32,
    /// When the road was put down. Tried as if new once the retry has passed.
    resting_since: Option<Instant>,
    /// Whether this road has already said in the log that it is the one
    /// carrying the pane. A flag rather than "was this the first try", because
    /// the first try is exactly the one a cold start can lose and the answer
    /// would then be silence for the life of the stream.
    told: bool,
}

/// What one turn of the pump asks the fast road for.
///
/// One value rather than four more arguments because they arrive together and
/// say one thing: this is the picture this pane wants right now, and this is
/// what the road may conclude from failing to hand one over.
#[derive(Clone, Copy)]
struct Turn {
    /// The long edge the pane can actually paint.
    long_edge: u32,
    /// The rate the helper is allowed to push at.
    max_fps: u32,
    /// Ask outright, whether or not the screen moved.
    refresh: bool,
    /// The device is still coming up, so "no picture" is not a failure.
    booting: bool,
}

/// Why one helper turn came back empty — and therefore whether it counts
/// against the road.
///
/// "Not yet" and "dead" are different facts, and only one of them is worth
/// resting a road for. Two of these three are "not yet":
///
/// * a device still booting has no framebuffer to hand out, and asking it for
///   one is the pane doing its job early rather than the road failing;
/// * a stream the pump itself just replaced, by asking for a new size or rate,
///   ends because it was replaced. The helper retires the pusher mid-stream
///   and that pusher closes its end of the socket (main.swift's `FramePusher`).
///
/// Counting the second of those was the whole of the 2026-09-21 defect: a pane
/// that did nothing worse than being resized had three of them in a row at
/// +26.5s, rested the fast road for five seconds, and only came back at
/// +32.6s.
/// What a re-negotiation is owed, and whether it has been paid.
///
/// The helper answers a new size or rate by retiring the pusher that was
/// mid-stream and starting another, and the retiring one closes its end of the
/// frame socket — so the failure that follows our own ask is ours. Exactly one
/// failure, though, and not one per ask: a stumble puts the road back to
/// "nothing negotiated", so a road that fails, re-negotiates and fails again
/// would otherwise forgive itself forever and never rest. It is a PICTURE that
/// buys the next free pass, because a picture is the only proof the road was
/// healthy in between.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Seam {
    /// Nothing has been asked for that would close a stream.
    Closed,
    /// A size or rate was just asked for; the close it causes is ours.
    Owed,
    /// That close has been forgiven and no picture has arrived since.
    Spent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stumble {
    /// The device is still coming up.
    Booting,
    /// We replaced this stream ourselves, this turn.
    Renegotiated,
    /// Nothing explains it. The one that rests the road.
    Unexplained,
}

impl HelperRoad {
    fn new() -> Self {
        Self {
            streaming_at: None,
            seam: Seam::Closed,
            misses: 0,
            resting_since: None,
            told: false,
        }
    }

    /// What this turn's failure counts as — and where a re-negotiation's one
    /// free pass is spent.
    fn why(&mut self, booting: bool) -> Stumble {
        if booting {
            return Stumble::Booting;
        }
        if self.seam == Seam::Owed {
            self.seam = Seam::Spent;
            return Stumble::Renegotiated;
        }
        Stumble::Unexplained
    }

    /// Answer one failure: count it only when nothing explains it, and put the
    /// road down once the unexplained ones stop being hiccups.
    fn stumbled(
        &mut self,
        why: Stumble,
        error: &str,
        husk_root: &Path,
        stream: &str,
    ) -> HelperFrameAnswer {
        // Whatever failed, the helper is no longer known to be pushing at any
        // size — so the next healthy turn re-negotiates rather than assuming.
        self.streaming_at = None;
        if why != Stumble::Unexplained {
            return HelperFrameAnswer::Absent;
        }
        self.misses = self.misses.saturating_add(1);
        // The first miss of a run says what went wrong; the rest line below
        // says the road gave up. Between them a reader sees whether three
        // misses were one failure or three different ones.
        if self.misses == 1 {
            crate::note_window_event(
                husk_root,
                &format!("emulator ios pump {stream}: framebuffer road stumbled ({error})"),
            );
        }
        if self.misses >= HELPER_MISSES_BEFORE_GIVING_UP {
            crate::note_window_event(
                husk_root,
                &format!(
                    "emulator ios pump {stream}: framebuffer road resting after {} tries \
                     ({error}) — slower roads for the next {}s, then it is tried again",
                    self.misses,
                    HELPER_ROAD_RETRY.as_secs()
                ),
            );
            self.resting_since = Some(Instant::now());
        }
        HelperFrameAnswer::Absent
    }

    /// Take one picture, saying once per stream which road carried it.
    fn arrived(
        &mut self,
        picture: HelperPicture,
        husk_root: &Path,
        stream: &str,
    ) -> HelperFrameAnswer {
        // Which road a pane ended up on is the first question asked whenever it
        // looks slow, and until this line existed only the roads that FAILED
        // left a word behind — so a pane on the fast one and a pane on the slow
        // one read exactly alike in the log.
        if !self.told {
            crate::note_window_event(
                husk_root,
                &format!(
                    "emulator ios pump {stream}: the device's framebuffer is being pushed \
                     ({} bytes at {}x{}, surface {})",
                    picture.bytes.len(),
                    picture.width,
                    picture.height,
                    picture.seed
                ),
            );
            self.told = true;
        }
        self.misses = 0;
        // A picture ends the seam a re-negotiation opened, and is the only
        // thing that buys the next free pass: the next failure is about
        // whatever comes after this picture.
        self.seam = Seam::Closed;
        HelperFrameAnswer::Picture(picture)
    }

    /// Let the helper idle its encoder. Said when the pane stops looking.
    #[cfg(target_os = "macos")]
    fn stop_streaming(&mut self, udid: &str) {
        if self.streaming_at.take().is_some() {
            let _ = super::ios_hid::stop_frames(udid);
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn stop_streaming(&mut self, _udid: &str) {}

    /// One turn of the push road: keep the stream negotiated, then wait on it.
    #[cfg(target_os = "macos")]
    fn take_frame(
        &mut self,
        udid: &str,
        turn: Turn,
        husk_root: &Path,
        stream: &str,
    ) -> HelperFrameAnswer {
        let Turn {
            long_edge,
            max_fps,
            refresh,
            booting,
        } = turn;
        if let Some(since) = self.resting_since {
            if since.elapsed() < HELPER_ROAD_RETRY {
                return HelperFrameAnswer::Absent;
            }
            // Tried as if new — `told` false, so a road that heals says so in
            // the log the way a road that started well does.
            *self = Self::new();
        }
        // A helper nobody has asked for yet is not a helper that failed. The
        // pump is running before the capability thread retains one, so this
        // road's first turns can find an empty map and get the same String
        // back that a dead pipe gives — which used to spend the whole miss
        // budget on a cold start and put the fast road out for the life of
        // the stream. It is asked about rather than assumed because the pump
        // does not know, and must not guess, when the retain lands.
        //
        // A helper that FELL is the opposite case and walks straight on: the
        // capability thread asked for its one helper and went home, so this
        // road is the only one left that wants a picture, and the asks below
        // are what bring a fresh helper. Answered `Absent` here as well —
        // which is what one boolean made this do until 2026-09-22 — a pane
        // whose helper died sat on the slow road for as long as it stayed
        // open, and no miss was ever counted to say so.
        if super::ios_hid::standing(udid) == super::ios_hid::HelperStanding::Unasked {
            return HelperFrameAnswer::Absent;
        }
        // Told once per size, not once per frame. This is the entire saving:
        // after this message the request lane is free for touches, and pictures
        // arrive on their own socket without anyone asking for them.
        if self.streaming_at != Some((long_edge, max_fps)) {
            let was = self.streaming_at;
            if let Err(error) =
                super::ios_hid::stream_frames(udid, long_edge, FRAME_JPEG_QUALITY, max_fps)
            {
                let why = self.why(booting);
                return self.stumbled(why, &error, husk_root, stream);
            }
            crate::note_window_event(
                husk_root,
                &format!(
                    "emulator ios pump {stream}: stream asked at {long_edge}px {max_fps}fps{}",
                    was.map_or(String::new(), |(edge, fps)| format!(
                        " (was {edge}px {fps}fps)"
                    ))
                ),
            );
            self.streaming_at = Some((long_edge, max_fps));
            // The stream the helper was pushing down, if any, has just been
            // retired in favour of this one — so the close that follows is
            // ours and not the road's. Only from `Closed`: an ask that follows
            // a forgiven failure, with no picture in between, buys nothing.
            if self.seam == Seam::Closed {
                self.seam = Seam::Owed;
            }
        }
        let picture = |frame: super::ios_hid::HelperFrame| HelperPicture {
            bytes: frame.bytes,
            width: frame.width,
            height: frame.height,
            seed: frame.seed,
        };
        match super::ios_hid::next_frame(udid, FRAME_WAIT_TIMEOUT) {
            Ok(Some(frame)) => self.arrived(picture(frame), husk_root, stream),
            Ok(None) if !refresh => {
                self.misses = 0;
                HelperFrameAnswer::Still
            }
            // A pane that has been looking at the same picture for a while asks
            // for one outright: a stream that says nothing is otherwise
            // indistinguishable from a stream that has died. Asking with no
            // seed is how "give me one whether or not it moved" is said — the
            // helper compares against what it is told, not what it remembers.
            Ok(None) => {
                match super::ios_hid::still_frame(udid, None, long_edge, FRAME_JPEG_QUALITY) {
                    Ok(Some(frame)) => self.arrived(picture(frame), husk_root, stream),
                    Ok(None) => {
                        self.misses = 0;
                        HelperFrameAnswer::Still
                    }
                    Err(error) => {
                        let why = self.why(booting);
                        self.stumbled(why, &error, husk_root, stream)
                    }
                }
            }
            Err(error) => {
                let why = self.why(booting);
                self.stumbled(why, &error, husk_root, stream)
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn take_frame(
        &mut self,
        _udid: &str,
        _turn: Turn,
        _husk_root: &Path,
        _stream: &str,
    ) -> HelperFrameAnswer {
        HelperFrameAnswer::Absent
    }
}

fn pump_ios_frames(
    app: AppHandle,
    stream: String,
    udid: String,
    device: String,
    boot: Arc<BootWatch>,
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
    // The helper's own story — starts, deaths, replacements, the frame
    // socket's ends — goes in the same log as the pump's, or the pump's
    // "the stream is closed" is the only witness to a helper being replaced
    // every second (2026-09-21).
    #[cfg(target_os = "macos")]
    super::ios_hid::note_helper_events_at(&husk_root);
    let mut misses = pump::MissCounter::new(MISSES_BEFORE_NOTE);
    // What the fast road COSTS, and only while the screen is actually moving:
    // an idle wait or a paused pane between two pictures is not the road's
    // time, and averaging it in would make a healthy pump read like a broken
    // one.
    let mut rate = pump::RateWatch::new(FRAMES_PER_RATE_WORD);
    let mut unchanged = 0u32;
    let mut last_fingerprint = None;
    let mut last_emitted: Option<Instant> = None;
    // The middle road, when this machine offers it: the Simulator's own window,
    // read directly instead of asked of CoreSimulator — a tenth of the
    // screenshot's cost. The search for it carries its own clock so machines
    // that do not offer the road spend ~nothing re-learning that.
    let mut window: Option<u32> = None;
    let mut looked = Instant::now()
        .checked_sub(WINDOW_LOOK_EVERY)
        .unwrap_or_else(Instant::now);
    // The road above both of those, and the only one that is not a poll: the
    // helper holds this device's framebuffer and its encoder, and once it is
    // told to stream it PUSHES a picture the moment the framebuffer moves. The
    // two roads below are what a machine without it has, and they still ask.
    let mut helper = HelperRoad::new();

    loop {
        // A pane nobody is looking at must cost the helper nothing. Said before
        // the wait below rather than after it, because the wait is where a
        // paused pump spends its time and an encoder left running there would
        // burn a core for pictures that are never sent.
        if control.is_paused() {
            helper.stop_streaming(&udid);
            rate.broke();
        }
        if !control.wait_until_running() {
            break;
        }
        let started = Instant::now();
        // A pane that has not been sent anything yet, or has been looking at
        // the same picture for a while, asks for one whether or not the screen
        // moved — a stream that says nothing is indistinguishable from a stream
        // that has died.
        let refresh = last_emitted.is_none_or(|at| at.elapsed() >= IDLE_REFRESH_INTERVAL);
        let long_edge = resolve_long_edge(control.viewport_long_edge());
        let turn = Turn {
            long_edge,
            max_fps: frame_rate_for(
                control.is_engaged() || control.input_within(INPUT_BOOST_WINDOW),
            ),
            refresh,
            // Read per turn rather than once: a boot finishes DURING a pane's
            // first seconds, and the turn after it finishes is the first one
            // whose failures are worth anything.
            booting: boot.booting(),
        };
        match helper.take_frame(&udid, turn, &husk_root, &stream) {
            HelperFrameAnswer::Absent => rate.broke(),
            // Nothing moved. The push road is watching the framebuffer for
            // free, so there is no rest to take and no ladder to climb — the
            // wait already happened inside `take_frame`.
            HelperFrameAnswer::Still => {
                rate.broke();
                continue;
            }
            HelperFrameAnswer::Picture(picture) => {
                misses.hit();
                match emit_bytes(
                    &app,
                    &control,
                    "emulator:frame",
                    &stream,
                    &picture.bytes,
                    Some("image/jpeg"),
                    // A newer picture is already on its way up the socket, so a
                    // frame the renderer has no room for is let go rather than
                    // queued — queuing it would only guarantee the person sees
                    // the past, and would stall the encoder behind the webview.
                    SlotWait::Skip,
                ) {
                    EmitOutcome::Sent => {
                        if let Some(mean) = rate.delivered() {
                            crate::note_window_event(
                                &husk_root,
                                &format!(
                                    "emulator ios pump {stream}: helper road {mean:.1}ms per \
                                     delivered frame ({:.1}fps) at {}x{}",
                                    1000.0 / mean,
                                    picture.width,
                                    picture.height
                                ),
                            );
                        }
                        last_emitted = Some(Instant::now());
                        // The other roads tell pictures apart by hashing them;
                        // this one is told by the framebuffer's own generation,
                        // so the hash of the last one they saw means nothing.
                        last_fingerprint = None;
                    }
                    EmitOutcome::Skipped => rate.broke(),
                    EmitOutcome::Closed => {
                        if control.is_alive() && !control.is_paused() {
                            emit_note(&app, &stream, EmulatorNoteCode::StreamEnded);
                            break;
                        }
                    }
                }
                unchanged = 0;
                continue;
            }
        }
        if window.is_none() && looked.elapsed() >= WINDOW_LOOK_EVERY {
            looked = Instant::now();
            window =
                crate::simulator_window::pick_window(&crate::simulator_window::windows(), &device);
            if window.is_some() {
                crate::note_window_event(
                    &husk_root,
                    &format!("emulator ios pump {stream}: reading {device}'s own window"),
                );
            }
        }
        // The window first when there is one, and the still picture for this
        // frame when it answers nothing — a window can be hidden or closed
        // between two frames, and the pane must not blink for it.
        let taken = window.and_then(crate::simulator_window::capture_jpeg);
        if taken.is_none() && window.take().is_some() {
            crate::note_window_event(
                &husk_root,
                &format!("emulator ios pump {stream}: {device}'s window stopped answering"),
            );
        }
        let bytes = match taken {
            Some(bytes) => Some(bytes),
            None => match screenshot_picture(&udid, &frame, &control) {
                Ok(bytes) => bytes,
                Err(()) => break,
            },
        };
        let Some(bytes) = bytes else {
            if control.is_alive() && !control.is_paused() {
                // A device that is still coming up has no picture to give yet,
                // and telling the pane its screen is unavailable over a boot is
                // the same lie the fast road stopped telling (D5). The budget
                // is not spent on it either: it is for a device that CAN draw,
                // and it has to survive the boot intact.
                if !turn.booting && misses.missed() {
                    crate::note_window_event(
                        &husk_root,
                        &format!(
                            "emulator ios pump {stream} udid {udid}: screenshots not answering"
                        ),
                    );
                    emit_note(&app, &stream, EmulatorNoteCode::FrameUnavailable);
                }
                control.rest(if turn.booting {
                    BOOTING_MISS_REST
                } else {
                    FALLBACK_MISS_REST
                });
            }
            continue;
        };

        misses.hit();
        let fingerprint = pump::frame_fingerprint(&bytes);
        let changed = last_fingerprint != Some(fingerprint);
        // Re-read rather than reusing the answer from the top of the loop: a
        // screenshot costs 130-231ms, so the refresh can fall due DURING the
        // capture that is meant to satisfy it.
        let refresh_due = last_emitted.is_none_or(|at| at.elapsed() >= IDLE_REFRESH_INTERVAL);
        if changed || refresh_due {
            match emit_bytes(
                &app,
                &control,
                "emulator:frame",
                &stream,
                &bytes,
                Some("image/jpeg"),
                // These roads PAY for the picture they are holding — a
                // screenshot cost 130-231ms to take — so waiting for the
                // renderer is cheaper than throwing it away and taking another.
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
        // A moving screen takes only what is left of the polling ceiling — on
        // the still-picture road that is nothing at all, on the window road
        // it is most of the frame. A still screen keeps the idle backoff, so
        // eight unchanged frames later the pump is asking once a second.
        let delay = if changed {
            FALLBACK_FRAME_CEILING.saturating_sub(started.elapsed())
        } else {
            FALLBACK_IDLE_LADDER.delay(unchanged)
        };
        control.rest(delay);
    }
    control.kill_child();
    let _ = std::fs::remove_file(frame);
}

#[cfg(target_os = "macos")]
fn ios_input_available(udid: &str) -> bool {
    super::ios_hid::retain(udid).is_ok()
}

#[cfg(not(target_os = "macos"))]
fn ios_input_available(_udid: &str) -> bool {
    false
}

fn ios_control(udid: &str) -> Result<Arc<SessionControl>, String> {
    registry()
        .target_control(EmulatorPlatform::Ios, udid)
        .filter(|control| control.is_alive())
        .ok_or_else(|| "이 iOS 시뮬레이터 스트림은 실행 중이 아닙니다".to_string())
}

/// `borrower` is the terminal whose agent asked for this device through the
/// emulator door (the window passes it only for an agent's `open`); none is
/// the person. A device this start boots for a borrower is lent to it
/// (t-6336) and goes down when that pane's work ends.
#[tauri::command]
pub(crate) async fn start_emulator_stream(
    app: AppHandle,
    webview: tauri::Webview,
    udid: Option<String>,
    viewport: Option<Viewport>,
    on_frame: BinaryChannel,
    borrower: Option<u32>,
) -> Result<EmulatorStream, String> {
    crate::from_the_main_webview(&webview)?;
    // A device stream is a road a walk starts on, the same as a browser pane
    // (t-5535): the wire is warmed here, before the boot and the first frame,
    // so the walk's first question does not pay a handshake as well.
    crate::systemone::warm_for_walks();
    tauri::async_runtime::spawn_blocking(move || {
        let devices = list_ios_simulators();
        let chosen = selected_simulator(&devices, udid.as_deref())?;
        let key = SessionKey::frames(EmulatorPlatform::Ios, chosen.udid.clone());
        let lease = match registry().claim(key)? {
            StartClaim::Existing(stream) => {
                // The pane in front of us brought a channel; the pump is still
                // posting to the one the previous pane opened and then closed.
                // Without this the reuse is a silent dead pane: send fails, the
                // pump reads that as the stream ending, and it breaks having
                // written nothing anywhere.
                registry().hand_frames_to(&stream.stream, on_frame);
                // And it brought its own size. One device has one pump, so the
                // newest pane to attach is the one whose size the pictures are
                // cut to — which is right, because the window pauses every pane
                // but the visible one, so the newest is the one being looked at.
                if let Some(viewport) = viewport {
                    registry().set_viewport(&stream.stream, viewport.long_edge_px);
                }
                crate::note_window_event(
                    app.state::<crate::AppState>().local_data_root(),
                    &format!(
                        "emulator ios stream {} reused by another pane; frames re-homed",
                        stream.stream
                    ),
                );
                // Up already: another pane joins a loan, or the person keeps it.
                super::note_start(
                    &app,
                    EmulatorPlatform::Ios,
                    &chosen.udid,
                    &chosen.name,
                    borrower,
                    false,
                );
                return Ok(stream);
            }
            StartClaim::Acquired(lease) => lease,
        };
        // Whether the boot above is OURS is the only thing that decides
        // whether the pump waits for one, and it has to be read before the
        // boot makes the answer yes.
        let waking = !chosen.booted;
        boot_simulator(&chosen)?;
        // And whether this start put the device up is who it belongs to.
        super::note_start(
            &app,
            EmulatorPlatform::Ios,
            &chosen.udid,
            &chosen.name,
            borrower,
            waking,
        );
        prepare_simulator_services(&chosen.udid);
        // And then out of sight again. The person asked for a device inside
        // THIS window; a second application appearing on their desktop is not
        // part of that, and until now one always did — the launch above starts
        // the Simulator hidden (`-j`), but `simctl boot` above makes it draw a
        // window for the device anyway, and this line used to be an `unhide`
        // that guaranteed the rest (33578e8).
        //
        // What that unhide bought was the window road, and the road above it
        // has since made the purchase pointless: the helper reads the device's
        // framebuffer through CoreSimulator with no window at all (main.swift
        // 160-163), at 1.92ms a frame against the window's 11-14ms.
        //
        // Measured with the Simulator hidden, and measured the only way that
        // proves anything: identical bytes across a hide would be exactly what
        // a CACHED DEAD surface answers (main.swift 155-158 warns of it), so
        // the screen was MADE to move instead. Safari opened on a hidden
        // device gave ten distinct pictures in eleven polls and a surface seed
        // that climbed 489 to 644 — a live road, not a remembered one. And
        // `simctl io screenshot`, the road below both, never wanted a window
        // either.
        //
        // So the window road keeps its wiring in the pump and stops being fed:
        // it is now reachable only when the person has the Simulator open
        // themselves, which is the one case where reading it costs them
        // nothing they did not already choose.
        crate::simulator_window::hide_simulator();
        // The input helper is NOT asked for here. Its cold start costs
        // 247-495ms on this machine (dlopen of the active Xcode's private
        // frameworks, then a SimServiceContext), and none of that is on the
        // way to a picture — so paying it before the pump exists spends the
        // whole of it as blank pane. `release` is safe on a udid that was
        // never retained, so the cleanup can be attached before anything is.
        let release_udid = chosen.udid.clone();
        let stream_id = crate::hooks::random_token().ok_or("스트림 id를 만들 수 없습니다")?;
        let descriptor = EmulatorStream {
            stream: stream_id.clone(),
            udid: chosen.udid.clone(),
            name: chosen.name.clone(),
            platform: EmulatorPlatform::Ios,
            // Not yet known, and said so rather than guessed. The word comes
            // back on `emulator:capability` when the helper answers.
            interactive: false,
            reused: false,
        };
        let control = registry().new_control(&descriptor);
        control.set_cleanup(move || {
            #[cfg(target_os = "macos")]
            super::ios_hid::release(&release_udid);
        });
        // The door goes to the session, not to the pump: it can be replaced
        // under a running pump when a second pane opens the same device.
        control.hand_frames_to(on_frame);
        // Before the pump exists, so the FIRST picture is already cut to the
        // pane rather than to the default and then re-cut a heartbeat later.
        if let Some(viewport) = viewport {
            control.set_viewport_long_edge(viewport.long_edge_px);
        }
        let husk_root = app
            .state::<crate::AppState>()
            .local_data_root()
            .to_path_buf();
        crate::note_window_event(
            &husk_root,
            &format!(
                "emulator ios stream {stream_id} picked {} ({})",
                chosen.name, chosen.udid
            ),
        );
        // This is the device the next window wakes before anybody asks (D4),
        // and the one this window is answerable for when it goes quiet (D3).
        // The next window wakes the PERSON's device, never an agent's: an
        // audit's simulator written down here came back at the next window's
        // boot (09-23 13:34, "last used 166 minutes ago") after its session
        // had ended (t-6336).
        if borrower.is_none() {
            keeping::remember_this_device(&husk_root, &chosen.udid, &chosen.name);
        } else {
            keeping::this_window_owns(&chosen.udid);
        }
        // The boot is timed beside the pictures rather than in front of them
        // (D5): the roads below draw the Simulator's own boot logo while this
        // watch says the device is still coming up, and both of the roads that
        // can only fail while it does read the same watch.
        let boot = if waking {
            watch_the_device_boot(&chosen.udid, &control, &husk_root, &stream_id)
        } else {
            BootWatch::already_up()
        };
        let descriptor = lease.activate(descriptor, control.clone());
        let pump_app = app.clone();
        let pump_stream = stream_id.clone();
        let pump_udid = chosen.udid.clone();
        let pump_boot = boot.clone();
        // The window road finds the Simulator's window by the DEVICE NAME its
        // title wears — the udid appears nowhere on screen.
        let pump_device = chosen.name;
        let capability_control = control.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("ios-emulator-{stream_id}"))
            .spawn(move || {
                pump_ios_frames(
                    pump_app,
                    pump_stream,
                    pump_udid,
                    pump_device,
                    pump_boot,
                    control,
                )
            })
        {
            registry().stop(&stream_id);
            return Err(format!("iOS 화면 스트림을 시작할 수 없습니다: {error}"));
        }
        // The helper is asked for behind the picture, never in front of it.
        let capability_app = app.clone();
        let capability_stream = stream_id.clone();
        let capability_udid = chosen.udid;
        let _ = std::thread::Builder::new()
            .name(format!("ios-capability-{stream_id}"))
            .spawn(move || {
                // Asked until it answers yes, not once. The cold start is
                // exactly the try a busy machine loses — dlopen of the active
                // Xcode's frameworks plus a SimServiceContext under load —
                // and this thread used to take that first "no" as the answer
                // for the life of the stream: `retained` stayed false, the
                // pump's framebuffer road answered Absent every frame, and
                // the pane sat on the still-picture road at 4-5fps. Each
                // retry costs one cold start, paid behind the pictures the
                // slower roads are already delivering; the backoff doubles so
                // a machine that really cannot start one is asked about ever
                // more rarely, and `rest` returns false the moment the
                // stream dies.
                let mut rest = HELPER_ATTACH_RETRY;
                let mut interactive = ios_input_available(&capability_udid);
                loop {
                    // The stream can end while the helper is still starting,
                    // and the cleanup that would have released it has already
                    // run by then. A reference nobody holds has to be put
                    // down here or it outlives the pane that asked for it.
                    if !capability_control.is_alive() {
                        #[cfg(target_os = "macos")]
                        if interactive {
                            super::ios_hid::release(&capability_udid);
                        }
                        return;
                    }
                    super::emit_capability(&capability_app, &capability_stream, interactive);
                    if interactive || !capability_control.rest(rest) {
                        return;
                    }
                    // While the device is still coming up the next try is
                    // genuinely likely to be the one that works, so it is
                    // asked for on the short beat (D5); once it is up, the
                    // doubling backoff is what covers a machine that will
                    // never have a helper at all.
                    rest = if boot.booting() {
                        HELPER_ATTACH_RETRY
                    } else {
                        (rest * 2).min(HELPER_ATTACH_CEILING)
                    };
                    interactive = ios_input_available(&capability_udid);
                }
            });
        Ok(descriptor)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn shutdown_mobile_emulator(
    webview: tauri::Webview,
    udid: String,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let devices = list_ios_simulators();
        let chosen = devices
            .iter()
            .find(|device| device.udid == udid)
            .ok_or("이 기계의 시뮬레이터가 아닙니다")?;
        shutdown_simulator(&chosen.udid)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn open_mobile_emulator(
    webview: tauri::Webview,
    udid: Option<String>,
) -> Result<String, String> {
    crate::from_the_main_webview(&webview)?;
    open_mobile_emulator_direct(udid).await
}

pub(crate) async fn open_mobile_emulator_direct(udid: Option<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let devices = list_ios_simulators();
        let chosen = selected_simulator(&devices, udid.as_deref())?;
        boot_simulator(&chosen)?;
        let opened = crate::proc::quiet_command("open")
            .args([
                "-a",
                "Simulator",
                "--args",
                "-CurrentDeviceUDID",
                &chosen.udid,
            ])
            .output()
            .map_err(|error| error.to_string())?;
        if !opened.status.success() {
            return Err(format!(
                "Simulator 앱을 열 수 없습니다: {}",
                String::from_utf8_lossy(&opened.stderr).trim()
            ));
        }
        Ok(chosen.name)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(target_os = "macos")]
fn run_ios_input(udid: &str, request: super::ios_hid::InputRequest) -> Result<(), String> {
    let control = ios_control(udid)?;
    let _input = control.input()?;
    super::ios_hid::send(udid, request)
}

#[cfg(not(target_os = "macos"))]
fn run_ios_input(_udid: &str, _request: ()) -> Result<(), String> {
    Err("iOS 입력은 macOS에서만 지원됩니다".to_string())
}

/// One phase of a live touch, streamed as it happens.
///
/// The window sends begin on pointer-down, throttled moves while the hand
/// drags, and end on release — so the device animates WITH the hand. The old
/// road replayed a finished drag as one 260ms swipe: the device heard nothing
/// until the gesture was over, the fixed pacing turned a slow deliberate drag
/// into a fling (which is how Spotlight and the App Library kept getting
/// pulled by accident — "색이 덮어지는것같고"), and the replay held the
/// helper's one request lane, frames included, for its whole duration.
#[tauri::command]
pub(crate) async fn ios_touch(
    webview: tauri::Webview,
    udid: String,
    phase: String,
    x: f64,
    y: f64,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    if !matches!(phase.as_str(), "begin" | "move" | "end") {
        return Err("유효하지 않은 터치 단계입니다".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid)?;
        #[cfg(target_os = "macos")]
        run_ios_input(
            &udid,
            super::ios_hid::InputRequest::Touch {
                phase,
                x: x.clamp(0.0, 1.0),
                y: y.clamp(0.0, 1.0),
            },
        )?;
        #[cfg(not(target_os = "macos"))]
        let _ = (phase, x, y, run_ios_input(&udid, ())?);
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn ios_tap(
    webview: tauri::Webview,
    udid: String,
    x: f64,
    y: f64,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    ios_tap_direct(udid, x, y).await
}

pub(crate) async fn ios_tap_direct(udid: String, x: f64, y: f64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid)?;
        #[cfg(target_os = "macos")]
        run_ios_input(
            &udid,
            super::ios_hid::InputRequest::Tap {
                x: x.clamp(0.0, 1.0),
                y: y.clamp(0.0, 1.0),
            },
        )?;
        #[cfg(not(target_os = "macos"))]
        let _ = (x, y, run_ios_input(&udid, ())?);
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) async fn marks_snapshot_direct(udid: String) -> Result<super::marks::Snapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _control = ios_control(&udid)?;
        marks_snapshot(&udid)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn marks_snapshot(udid: &str) -> Result<super::marks::Snapshot, String> {
    #[cfg(target_os = "macos")]
    {
        let tree = serde_json::Value::Array(super::ios_hid::accessibility_roots(udid)?);
        super::marks::Snapshot::ios(&tree)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = udid;
        Err("iOS 접근성 트리는 macOS에서만 지원됩니다".into())
    }
}

pub(super) async fn click_mark_direct(
    udid: String,
    request: super::marks::PinnedTap,
) -> Result<super::marks::Pressed, zerocode_core::computer_use_protocol::ProviderError> {
    use super::marks::{Pressed, Proof, backend_error};
    use zerocode_core::computer_use_protocol::ProviderError;
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid).map_err(backend_error)?;
        let tap = |x: f64, y: f64| -> Result<(), ProviderError> {
            #[cfg(target_os = "macos")]
            super::ios_hid::send(&udid, super::ios_hid::InputRequest::Tap { x, y })
                .map_err(backend_error)?;
            #[cfg(not(target_os = "macos"))]
            let _ = (x, y, run_ios_input(&udid, ()).map_err(backend_error)?);
            control.notify();
            Ok(())
        };
        let proof = {
            let input = control.input().map_err(backend_error)?;
            request.on_device(&udid, || {
                // The one point the press lands on is asked first — a few
                // milliseconds against the whole tree's 633 (t-6350) — and
                // the tree is still read for every press the point cannot
                // prove.
                #[cfg(target_os = "macos")]
                {
                    let (x, y) = request.centre();
                    if let Some(pressed) = super::ios_hid::element_at(&udid, x, y)
                        .ok()
                        .and_then(|answer| request.perform_at_centre_in(&input, &answer, tap))
                    {
                        return pressed.map(|()| Proof::Point);
                    }
                }
                let snapshot = marks_snapshot(&udid).map_err(backend_error)?;
                request
                    .perform_in(&input, &snapshot.faces, snapshot.screen, tap)
                    .map(|()| Proof::Tree)
            })?
        };
        // The device's input gate is let go of before the wait: what follows
        // only reads, and a person's touch in the pane need not queue
        // behind a screen settling.
        #[cfg(target_os = "macos")]
        let settled = Some(settle(&udid, Instant::now(), |faces| {
            request.stands_in(faces)
        }));
        #[cfg(not(target_os = "macos"))]
        let settled = None;
        Ok(Pressed { proof, settled })
    })
    .await
    .map_err(backend_error)?
}

/// Read the pressed screen until it stops changing (t-6385,
/// [`super::marks::Settling`]): the walk alone, every element's centre
/// asked — the tree a look reads, less the grid — again and again from the
/// tap until two reads after a change agree, nothing changes within the
/// quiet window, or the ceiling passes. A look taken at once reads the screen
/// being left: the next press was refused 5 times of 5 (t-6350). A read
/// without the pressed control where it stood (`stands`) has moved: the
/// screen can change before the first read comes back.
#[cfg(target_os = "macos")]
fn settle(
    udid: &str,
    tapped: Instant,
    stands: impl Fn(&[zerocode_core::computer_use_protocol::marks::ElementFace]) -> bool,
) -> super::marks::Settled {
    use super::marks::{Settle, Settled, Settling, Snapshot, shape};
    use zerocode_core::agent_emulator::{EMULATOR_SETTLE_CEILING_MS, EMULATOR_SETTLE_QUIET_MS};
    let mut settling = Settling::new(
        Duration::from_millis(EMULATOR_SETTLE_QUIET_MS),
        Duration::from_millis(EMULATOR_SETTLE_CEILING_MS),
    );
    let mut last = None;
    loop {
        let read = super::ios_hid::accessibility_walk(udid)
            .ok()
            .and_then(|roots| Snapshot::ios(&serde_json::Value::Array(roots)).ok());
        let seen = read.as_ref().map(|snapshot| shape(&snapshot.faces));
        match read {
            Some(snapshot) => {
                if !stands(&snapshot.faces) {
                    settling.moved();
                }
                last = Some(snapshot);
            }
            // A read that failed — an app switching, a helper replaced — is
            // not asked again at once.
            None => std::thread::sleep(Duration::from_millis(
                zerocode_core::computer_use::COMPUTER_SETTLE_POLL_MS,
            )),
        }
        let settle = settling.read(seen, tapped.elapsed());
        if settle != Settle::Reading {
            return Settled {
                settle,
                reads: settling.reads(),
                ms: u64::try_from(tapped.elapsed().as_millis()).unwrap_or(u64::MAX),
                last,
            };
        }
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ios_swipe(
    webview: tauri::Webview,
    udid: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    ms: Option<u32>,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    ios_swipe_direct(udid, x1, y1, x2, y2, ms).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn ios_swipe_direct(
    udid: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    ms: Option<u32>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid)?;
        #[cfg(target_os = "macos")]
        run_ios_input(
            &udid,
            super::ios_hid::InputRequest::Swipe {
                x1: x1.clamp(0.0, 1.0),
                y1: y1.clamp(0.0, 1.0),
                x2: x2.clamp(0.0, 1.0),
                y2: y2.clamp(0.0, 1.0),
                duration_ms: ms.unwrap_or(300).clamp(50, 3_000),
            },
        )?;
        #[cfg(not(target_os = "macos"))]
        let _ = (x1, y1, x2, y2, ms, run_ios_input(&udid, ())?);
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn ios_text(
    webview: tauri::Webview,
    udid: String,
    text: String,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    ios_text_direct(udid, text).await
}

pub(crate) async fn ios_text_direct(udid: String, text: String) -> Result<(), String> {
    if text.chars().any(|character| character.is_control()) || text.len() > 4096 {
        return Err("보낼 수 없는 문자가 있습니다".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid)?;
        #[cfg(target_os = "macos")]
        run_ios_input(&udid, super::ios_hid::InputRequest::Text { text })?;
        #[cfg(not(target_os = "macos"))]
        let _ = (text, run_ios_input(&udid, ())?);
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn ios_button(
    webview: tauri::Webview,
    udid: String,
    name: String,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    ios_button_direct(udid, name).await
}

pub(crate) async fn ios_button_direct(udid: String, name: String) -> Result<(), String> {
    if !matches!(
        name.as_str(),
        "home"
            | "enter"
            | "del"
            | "forward_del"
            | "escape"
            | "tab"
            | "up"
            | "down"
            | "left"
            | "right"
            | "power"
            | "lock"
            | "volume-up"
            | "volume_up"
            | "volup"
            | "volume-down"
            | "volume_down"
            | "voldown"
            | "action"
    ) {
        return Err("알 수 없는 iOS 버튼입니다".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid)?;
        #[cfg(target_os = "macos")]
        run_ios_input(&udid, super::ios_hid::InputRequest::Button { name })?;
        #[cfg(not(target_os = "macos"))]
        let _ = (name, run_ios_input(&udid, ())?);
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn ios_rotate(
    webview: tauri::Webview,
    udid: String,
    rotation: u32,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    ios_rotate_direct(udid, rotation).await
}

pub(crate) async fn ios_rotate_direct(udid: String, rotation: u32) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid)?;
        #[cfg(target_os = "macos")]
        run_ios_input(
            &udid,
            super::ios_hid::InputRequest::Rotate {
                rotation: rotation % 4,
            },
        )?;
        #[cfg(not(target_os = "macos"))]
        let _ = (rotation, run_ios_input(&udid, ())?);
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn ios_accessibility_tree(
    webview: tauri::Webview,
    udid: String,
) -> Result<serde_json::Value, String> {
    crate::from_the_main_webview(&webview)?;
    ios_accessibility_tree_direct(udid).await
}

pub(crate) async fn ios_accessibility_tree_direct(
    udid: String,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _control = ios_control(&udid)?;
        #[cfg(target_os = "macos")]
        {
            super::ios_hid::accessibility_tree(&udid)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("iOS 접근성 트리는 macOS에서만 지원됩니다".to_string())
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ios_multi_touch(
    webview: tauri::Webview,
    udid: String,
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    x3: f64,
    y3: f64,
    x4: f64,
    y4: f64,
    ms: Option<u32>,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    if ![x1, y1, x2, y2, x3, y3, x4, y4]
        .into_iter()
        .all(f64::is_finite)
    {
        return Err("올바르지 않은 다중 터치 좌표입니다".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid)?;
        #[cfg(target_os = "macos")]
        run_ios_input(
            &udid,
            super::ios_hid::InputRequest::MultiTouch {
                x1,
                y1,
                x2,
                y2,
                x3,
                y3,
                x4,
                y4,
                duration_ms: ms.unwrap_or(300).clamp(50, 3_000),
            },
        )?;
        #[cfg(not(target_os = "macos"))]
        let _ = (
            x1,
            y1,
            x2,
            y2,
            x3,
            y3,
            x4,
            y4,
            ms,
            run_ios_input(&udid, ())?,
        );
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn ios_install_app(
    webview: tauri::Webview,
    udid: String,
    path: String,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid)?;
        capabilities::install_app(&udid, &path)?;
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn ios_launch_app(
    webview: tauri::Webview,
    udid: String,
    bundle: String,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid)?;
        capabilities::launch_app(&udid, &bundle)?;
        control.notify();
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn ios_set_permission(
    webview: tauri::Webview,
    udid: String,
    operation: String,
    bundle: String,
    service: String,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _control = ios_control(&udid)?;
        capabilities::set_permission(&udid, &operation, &bundle, &service)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn ios_logs(
    webview: tauri::Webview,
    udid: String,
    lines: Option<usize>,
    seconds: Option<u32>,
    process: Option<String>,
) -> Result<super::capability::EmulatorLogBatch, String> {
    crate::from_the_main_webview(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _control = ios_control(&udid)?;
        capabilities::logs(&udid, lines, seconds, process)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Wake the simulator this machine last used, before anybody asks for it (D4).
///
/// On the caller's thread: [`super::on_window_boot`] owns the one thread all
/// of this rides, so a platform added beside this one does not bring a third.
pub(super) fn preboot_last_used(app: &AppHandle) {
    keeping::preboot_last_used(app);
}

/// Start the watch that shuts this window's devices down once nothing is
/// looking at them (D3).
pub(super) fn start_idle_reclaimer(app: AppHandle) {
    keeping::start_idle_reclaimer(app);
}

/// The window is closing. A booted simulator is left booted — that is the
/// whole of D3 — unless the person has said otherwise, in which case the
/// devices this window is answerable for go with it.
///
/// Nothing else here shuts a simulator down on the way out, and nothing ever
/// did: `ios_hid::shutdown_all` puts down the helper CLIENTS (a process apiece
/// and a socket apiece), and the registry stops the streams. The device itself
/// has always survived this window; what it did not survive was having nothing
/// to come back to, which is what the preboot and the last-used record are
/// for.
pub(super) fn devices_at_exit(keep_booted: bool, local_data_root: &Path) {
    if keep_booted {
        return;
    }
    keeping::shut_down_our_devices(local_data_root);
}

/// Put a lent simulator away (t-6336): the one `simctl shutdown`, and its
/// idle clock let go — it is off, and nothing here watches it any more.
pub(super) fn put_away(udid: &str) -> Result<(), String> {
    shutdown_simulator(udid)?;
    keeping::this_window_lets_go(udid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one `simctl shutdown`, answered by a fake `simctl` that writes down
    /// what it was asked (t-6336): a booted device goes down, a device already
    /// down is already where it was sent, and a device the machine no longer
    /// has — an agent deleted it by hand — is a refusal the return reports
    /// rather than papers over.
    #[cfg(unix)]
    #[test]
    fn the_one_shutdown_reads_what_simctl_answers() {
        use std::os::unix::fs::PermissionsExt as _;
        let scratch = tempfile::tempdir().expect("scratch");
        let asked = scratch.path().join("asked");
        let simctl = scratch.path().join("simctl");
        std::fs::write(
            &simctl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$2\" in\n  booted-one) exit 0 ;;\n  already-off) echo 'Unable to shutdown device in current state: Shutdown' >&2; exit 149 ;;\n  *) echo \"Invalid device: $2\" >&2; exit 148 ;;\nesac\n",
                asked.display()
            ),
        )
        .expect("fake simctl");
        std::fs::set_permissions(&simctl, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let fake = || crate::proc::quiet_command(&simctl);
        assert_eq!(shutdown_simulator_by(fake(), "booted-one"), Ok(()));
        assert_eq!(shutdown_simulator_by(fake(), "already-off"), Ok(()));
        let refused = shutdown_simulator_by(fake(), "deleted-one").expect_err("a missing device");
        assert!(refused.contains("Invalid device: deleted-one"), "{refused}");
        assert_eq!(
            std::fs::read_to_string(&asked).expect("what simctl was asked"),
            "shutdown booted-one\nshutdown already-off\nshutdown deleted-one\n"
        );
    }

    /// On a real simulator this task owns (t-6336, named by
    /// `ZEROCODE_LIVE_SIMULATOR`): a device up and lent to nobody stays up when
    /// a pane's work ends, and the same device lent to that pane goes down by
    /// the one `simctl shutdown` — the book's verdict and the power road end
    /// to end, with how long the return took. Never name a device another
    /// session is using: the second half shuts it down.
    #[test]
    #[ignore = "boots a real iOS simulator and shuts it down again"]
    fn a_lent_simulator_goes_down_with_its_borrower_and_an_unlent_one_stays_up() {
        use super::super::session::{LoanEnd, loans};
        let Ok(udid) = std::env::var("ZEROCODE_LIVE_SIMULATOR") else {
            println!("LIVE: no simulator named; nothing measured");
            return;
        };
        const TERM: u32 = 4_242_001;
        let named = || {
            list_ios_simulators()
                .into_iter()
                .find(|device| device.udid == udid)
                .expect("the named simulator is on this machine")
        };
        boot_simulator(&named()).expect("the named simulator boots");
        // Up, and a person's start: nobody's loan, so the pane's end leaves it.
        loans().note_start(
            EmulatorPlatform::Ios,
            &udid,
            None,
            true,
            crate::now_epoch_ms(),
        );
        super::super::borrower_gone(TERM, LoanEnd::PaneClosed);
        std::thread::sleep(Duration::from_secs(3));
        let kept = named().booted;
        println!("LIVE: unlent device after its pane's end: booted={kept}");
        assert!(kept, "a device nobody lent went down with a pane");
        // Lent to the pane: its end puts it down.
        loans().note_start(
            EmulatorPlatform::Ios,
            &udid,
            Some(TERM),
            true,
            crate::now_epoch_ms(),
        );
        let began = Instant::now();
        super::super::borrower_gone(TERM, LoanEnd::PaneClosed);
        let deadline = began + Duration::from_secs(60);
        while named().booted && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let down = !named().booted;
        println!(
            "LIVE: lent device after its pane's end: shut down={down} in {} ms",
            began.elapsed().as_millis()
        );
        assert!(down, "a lent device outlived its borrower");
    }

    fn device(name: &str, booted: bool) -> SimulatorDevice {
        SimulatorDevice {
            udid: name.to_string(),
            name: name.to_string(),
            booted,
            runtime: "runtime".to_string(),
        }
    }

    #[test]
    fn default_selection_matches_orcas_order() {
        let devices = [
            device("iPad booted", true),
            device("iPhone stopped", false),
            device("iPhone booted", true),
        ];
        assert_eq!(
            pick_default_simulator(&devices).unwrap().name,
            "iPhone booted"
        );
    }

    #[test]
    fn a_pane_that_has_not_measured_itself_yet_still_gets_a_usable_picture() {
        assert_eq!(resolve_long_edge(None), DEFAULT_LONG_EDGE_PX);
        // The default is already a multiple of the quantum, so the pane that
        // never speaks never costs an encoder rebuild either.
        assert_eq!(DEFAULT_LONG_EDGE_PX % LONG_EDGE_QUANTUM_PX, 0);
    }

    /// Use gets the ceiling; a glance gets a fifth of it.
    #[test]
    fn the_rate_follows_use() {
        assert_eq!(frame_rate_for(true), 60);
        assert_eq!(frame_rate_for(false), 12);
        assert!(frame_rate_for(false) * 5 <= frame_rate_for(true));
    }

    #[test]
    fn a_long_edge_is_rounded_up_so_a_drag_crosses_few_encoder_sizes() {
        // Rounded UP, never down: rounding down would hand the pane fewer
        // pixels than it paints with, which is the blur this whole number
        // exists to stop.
        assert_eq!(resolve_long_edge(Some(1281)), 1344);
        assert_eq!(resolve_long_edge(Some(1344)), 1344);
        // Every answer is a multiple of the quantum, so the encoder is rebuilt
        // once per rung rather than once per mouse move.
        for asked in [700u32, 701, 900, 1000, 1500, 2000] {
            assert_eq!(resolve_long_edge(Some(asked)) % LONG_EDGE_QUANTUM_PX, 0);
            assert!(resolve_long_edge(Some(asked)) >= asked);
        }
    }

    #[test]
    fn absurd_pane_sizes_are_held_between_the_floor_and_the_ceiling() {
        // A split being dragged shut reports single digits mid-layout, and
        // encoding a picture that size makes the pane visibly flash.
        assert_eq!(resolve_long_edge(Some(0)), MIN_LONG_EDGE_PX);
        assert_eq!(resolve_long_edge(Some(12)), MIN_LONG_EDGE_PX);
        // And a full-screen pane on a large display does not get to ask for
        // more pixels than a mirror of a phone can use.
        assert_eq!(resolve_long_edge(Some(u32::MAX)), MAX_LONG_EDGE_PX);
        assert_eq!(
            resolve_long_edge(Some(MAX_LONG_EDGE_PX + 1)),
            MAX_LONG_EDGE_PX
        );
        // The ceiling holds even though rounding up would otherwise cross it.
        assert!(resolve_long_edge(Some(MAX_LONG_EDGE_PX)) <= MAX_LONG_EDGE_PX);
    }

    /// Nowhere at all: [`crate::note_window_event`] opens a file inside the
    /// root it is given and does nothing when that root does not exist, so a
    /// road tested here writes no log anywhere on this machine.
    fn nowhere() -> &'static Path {
        Path::new("/zerocode-tests-have-no-data-root")
    }

    /// A stream the pump itself replaced is a seam in the road, not its death.
    ///
    /// The helper answers a new size by retiring the pusher mid-stream, and
    /// that pusher closes its end — so the very next wait says "the stream is
    /// closed". Counted as misses, three of those rested the fast road for
    /// five seconds and dropped the pane onto the still-picture road, measured
    /// in the window's log on 2026-09-21 at +26.5s after nothing worse than a
    /// resize.
    #[test]
    fn a_stream_we_replaced_ourselves_costs_the_road_nothing() {
        let mut road = HelperRoad::new();
        road.seam = Seam::Owed;
        let why = road.why(false);
        assert_eq!(why, Stumble::Renegotiated);
        road.stumbled(why, "iOS 화면 스트림이 닫혔습니다", nowhere(), "stream");
        assert_eq!(road.misses, 0, "a close we caused was counted against us");
        assert!(
            road.resting_since.is_none(),
            "the road rested over a resize"
        );
        // And the next turn re-negotiates rather than assuming the old size.
        assert!(road.streaming_at.is_none());
        // The free pass is spent, not held: a road that fails again before any
        // picture arrives is failing for its own reasons.
        assert_eq!(road.why(false), Stumble::Unexplained);
    }

    /// A picture closes the seam: whatever fails after one is about what came
    /// after it.
    #[test]
    fn a_picture_ends_the_free_pass() {
        let mut road = HelperRoad::new();
        road.seam = Seam::Owed;
        road.arrived(
            HelperPicture {
                bytes: vec![1, 2, 3],
                width: 412,
                height: 896,
                seed: 518,
            },
            nowhere(),
            "stream",
        );
        assert_eq!(road.seam, Seam::Closed);
        assert_eq!(road.why(false), Stumble::Unexplained);
    }

    /// The free pass is bought by a PICTURE, not by the ask itself — otherwise
    /// a road whose every stream dies would re-negotiate, be forgiven, and
    /// never once be rested or written down.
    #[test]
    fn a_road_that_only_ever_re_negotiates_is_forgiven_once_and_then_rested() {
        let mut road = HelperRoad::new();
        for turn in 0..=HELPER_MISSES_BEFORE_GIVING_UP {
            // What `take_frame` does after an ask the helper accepted.
            if road.seam == Seam::Closed {
                road.seam = Seam::Owed;
            }
            let why = road.why(false);
            assert_eq!(
                why,
                if turn == 0 {
                    Stumble::Renegotiated
                } else {
                    Stumble::Unexplained
                },
                "turn {turn} was judged wrong"
            );
            road.stumbled(why, "iOS 화면 스트림이 닫혔습니다", nowhere(), "stream");
        }
        assert_eq!(road.misses, HELPER_MISSES_BEFORE_GIVING_UP);
        assert!(road.resting_since.is_some());
    }

    /// A device that cannot draw yet is not a road that is broken. The pane
    /// opens its roads before the boot is over now (D5), so the helper's cold
    /// start legitimately fails for as long as the device is coming up.
    #[test]
    fn a_booting_device_never_rests_the_road() {
        let mut road = HelperRoad::new();
        for _ in 0..HELPER_MISSES_BEFORE_GIVING_UP * 3 {
            let why = road.why(true);
            assert_eq!(why, Stumble::Booting);
            road.stumbled(why, "화면 소스를 열 수 없습니다", nowhere(), "stream");
        }
        assert_eq!(road.misses, 0);
        assert!(road.resting_since.is_none());
        // And a boot does not spend the re-negotiation's free pass: the resize
        // that happens during a boot still gets its one forgiveness after it.
        road.seam = Seam::Owed;
        assert_eq!(road.why(true), Stumble::Booting);
        assert_eq!(road.seam, Seam::Owed);
    }

    /// The rest itself is untouched: three failures nothing explains still put
    /// the road down for the retry, which is what a helper that really died
    /// must not cost the pane more than once.
    #[test]
    fn three_failures_nothing_explains_still_rest_the_road() {
        let mut road = HelperRoad::new();
        for turn in 1..=HELPER_MISSES_BEFORE_GIVING_UP {
            let why = road.why(false);
            assert_eq!(why, Stumble::Unexplained);
            road.stumbled(
                why,
                "iOS 입력 헬퍼가 응답하지 않습니다",
                nowhere(),
                "stream",
            );
            assert_eq!(road.misses, turn);
        }
        assert!(road.resting_since.is_some());
    }

    /// `simctl boot` says no to a device that is already awake, and the state
    /// it names is the only way to tell that from a failure. With a preboot in
    /// flight (D4) that state is `Booting` as often as `Booted`.
    #[test]
    fn a_boot_already_in_flight_reads_as_a_yes() {
        assert!(already_awake(
            "Unable to boot device in current state: Booted"
        ));
        assert!(already_awake(
            "Unable to boot device in current state: Booting"
        ));
        assert!(!already_awake(
            "Invalid device: D0707A9F-8352-4989-934B-2392F6B367CF"
        ));
        assert!(!already_awake(
            "Unable to boot device in current state: Shutting Down"
        ));
    }

    /// One pane's first picture, against a real simulator, in the order the
    /// pump asks its roads in.
    ///
    /// The capability thread's beat and the pump's turn are reproduced rather
    /// than the pump itself, because the pump needs a window: a thread that
    /// retries the helper's cold start, and a loop that takes the helper's
    /// picture when there is one and a CoreSimulator still picture when there
    /// is not. That IS the road order the pane sees.
    ///
    /// ```text
    /// ZEROCODE_LIVE_SIMULATOR=<udid> ZEROCODE_LIVE_ORDER=new|old \
    ///   cargo test -p zerocode-shell --bin zerocode-shell -- --ignored \
    ///   --nocapture the_first_picture
    /// ```
    ///
    /// `old` waits for `bootstatus` before opening a road, which is what this
    /// window did until 2026-09-21; `new` opens them at once.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "boots a real iOS simulator and shuts it down again"]
    fn the_first_picture_of_a_live_pane() {
        let Ok(udid) = std::env::var("ZEROCODE_LIVE_SIMULATOR") else {
            println!("LIVE: no simulator named; nothing measured");
            return;
        };
        let old_order = std::env::var("ZEROCODE_LIVE_ORDER").as_deref() == Ok("old");
        let already_booted = list_ios_simulators()
            .into_iter()
            .find(|device| device.udid == udid)
            .map(|device| device.booted)
            .unwrap_or_default();
        // The clock starts where the pane's does: the stream has picked a
        // device and is about to make sure it is awake.
        let started = Instant::now();
        let device = SimulatorDevice {
            udid: udid.clone(),
            name: "live".to_string(),
            booted: already_booted,
            runtime: "live".to_string(),
        };
        boot_simulator(&device).expect("the device boots");
        let booted_at = started.elapsed();
        if old_order {
            let _ = simctl_command()
                .args(["bootstatus", &udid])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            println!(
                "LIVE: bootstatus answered at {}ms",
                started.elapsed().as_millis()
            );
        }
        // The capability thread: one cold start per beat until one stands.
        let attaching = udid.clone();
        let attached = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let standing = attached.clone();
        let asking = std::thread::spawn(move || {
            while !standing.load(std::sync::atomic::Ordering::Acquire) {
                if super::super::ios_hid::retain(&attaching).is_ok() {
                    standing.store(true, std::sync::atomic::Ordering::Release);
                    return;
                }
                std::thread::sleep(HELPER_ATTACH_RETRY);
            }
        });
        let long_edge = resolve_long_edge(None);
        let frame = std::env::temp_dir().join(format!(
            "zc-first-picture-{}.jpg",
            uuid::Uuid::new_v4().simple()
        ));
        let deadline = Instant::now() + BOOT_WAIT;
        let (road, size) = loop {
            assert!(Instant::now() < deadline, "no road ever drew a picture");
            if super::super::ios_hid::standing(&udid)
                == super::super::ios_hid::HelperStanding::Standing
                && super::super::ios_hid::stream_frames(&udid, long_edge, FRAME_JPEG_QUALITY, 60)
                    .is_ok()
            {
                if let Ok(Some(picture)) =
                    super::super::ios_hid::next_frame(&udid, FRAME_WAIT_TIMEOUT)
                {
                    break (
                        "helper push",
                        format!("{}x{}", picture.width, picture.height),
                    );
                }
                if let Ok(Some(picture)) =
                    super::super::ios_hid::still_frame(&udid, None, long_edge, FRAME_JPEG_QUALITY)
                {
                    break (
                        "helper still",
                        format!("{}x{}", picture.width, picture.height),
                    );
                }
            }
            let _ = std::fs::remove_file(&frame);
            let shot = simctl_command()
                .args(["io", &udid, "screenshot", "--type=jpeg"])
                .arg(&frame)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if shot.is_ok_and(|status| status.success())
                && let Ok(bytes) = std::fs::metadata(&frame)
                && bytes.len() > 0
            {
                break ("still picture", format!("{} bytes", bytes.len()));
            }
            // The pump's own rest after a capture that answered nothing —
            // without it this would measure a road nobody walks.
            std::thread::sleep(BOOTING_MISS_REST);
        };
        let first = started.elapsed();
        println!(
            "LIVE: {} device, {} order — boot accepted at {}ms, first picture at {}ms on the {road} road ({size})",
            if already_booted {
                "booted"
            } else {
                "shut-down"
            },
            if old_order { "old" } else { "new" },
            booted_at.as_millis(),
            first.as_millis()
        );
        attached.store(true, std::sync::atomic::Ordering::Release);
        let _ = asking.join();
        let _ = std::fs::remove_file(&frame);
        super::super::ios_hid::release(&udid);
        if !already_booted {
            let _ = shutdown_simulator(&udid);
        }
    }

    /// The push road survives the sizes a pane actually asks for.
    ///
    /// A resize, a pause and resume, a pointer entering: each of those is one
    /// `stream` ask, and the helper answers it by retiring the pusher that was
    /// mid-stream — which closes its end of the frame socket. Served as one
    /// connection, the wait after that ask answered `iOS 화면 스트림이
    /// 닫혔습니다` forever after.
    ///
    /// `Ok(None)` is the pass here: it is the answer of a live socket with a
    /// still screen. `Err` is the failure this test exists for.
    ///
    /// ```text
    /// ZEROCODE_LIVE_SIMULATOR=<udid> \
    ///   cargo test -p zerocode-shell --bin zerocode-shell -- --ignored \
    ///   --nocapture a_re_negotiated_stream
    /// ```
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs a booted iOS simulator"]
    fn a_re_negotiated_stream_keeps_its_push_road() {
        let Ok(udid) = std::env::var("ZEROCODE_LIVE_SIMULATOR") else {
            println!("LIVE: no simulator named; nothing measured");
            return;
        };
        super::super::ios_hid::retain(&udid).expect("the helper stands");
        // The sizes a dragged split crosses, and back again.
        for (turn, asked) in [1024u32, 512, 1024].into_iter().enumerate() {
            let long_edge = resolve_long_edge(Some(asked));
            super::super::ios_hid::stream_frames(&udid, long_edge, FRAME_JPEG_QUALITY, 60)
                .expect("the helper takes the new size");
            let started = Instant::now();
            let mut pictures = 0u32;
            while started.elapsed() < Duration::from_secs(2) {
                match super::super::ios_hid::next_frame(&udid, FRAME_WAIT_TIMEOUT) {
                    Ok(Some(_)) => pictures += 1,
                    Ok(None) => {}
                    Err(error) => panic!(
                        "turn {turn} at {long_edge}: the push road died on a re-negotiation \
                         after {}ms — {error}",
                        started.elapsed().as_millis()
                    ),
                }
            }
            println!("LIVE: {long_edge} long edge — {pictures} pictures, road alive");
        }
        super::super::ios_hid::release(&udid);
    }

    /// The pane's own turns, out of the window, against a real device — the
    /// column the window's log could not show.
    ///
    /// `a_re_negotiated_stream_keeps_its_push_road` asks once per size and
    /// then only waits; this walks the pump's actual road. The pane is born
    /// attended (the active rate), is told a heartbeat later that nobody is
    /// over it (the glance rate), asks outright every `IDLE_REFRESH_INTERVAL`,
    /// and after every failure falls to a slower road that costs a capture and
    /// an idle rest before the fast road is tried again — the shape the window
    /// walked on 2026-09-21 while a helper was started and put down every
    /// second behind it.
    ///
    /// ```text
    /// ZEROCODE_LIVE_SIMULATOR=<udid> [ZEROCODE_LIVE_SECONDS=60] \
    ///   [ZEROCODE_LIVE_BOOT=1] [ZEROCODE_LIVE_SLOW_ROAD=screenshot] \
    ///   [ZEROCODE_LIVE_SIGCHLD=tokio] [ZEROCODE_LIVE_SIMULATOR_APP=1] \
    ///   [ZEROCODE_LIVE_ATTENTION_EVERY_MS=0] [ZEROCODE_LIVE_POKE_EVERY_MS=0] \
    ///   cargo test -p zerocode-shell --bin zerocode-shell -- --ignored \
    ///   --nocapture the_panes_turn_column
    /// ```
    ///
    /// `BOOT` shuts the device down first and opens the roads during its boot,
    /// the way a pane does (D5); `SLOW_ROAD=screenshot` walks the real
    /// still-picture road on every turn the fast road answers nothing (a
    /// `simctl` child per turn, as in the window) instead of sleeping its
    /// cost; `SIGCHLD=tokio` installs the window's own child-reaping signal
    /// handler first; `SIMULATOR_APP=1` asks for the Simulator application
    /// and hides it, as the window does between the boot and the pump.
    /// `ATTENTION_EVERY_MS` brings the pointer back and takes
    /// it away on that beat (0: it never comes back); `POKE_EVERY_MS` swipes
    /// the device on that beat so the screen has something to push (0: the
    /// device's own motion only).
    ///
    /// `LET_GO_AFTER` is the fault, and the reason this test exists: the frame
    /// reader walks away from each connection after that many pictures, which
    /// is the one event the window's log showed the consequences of and could
    /// not be asked to repeat — a bus put down under a live pusher, a read
    /// that hiccupped, a client replaced beneath it. Measured on this device
    /// on 2026-09-22, five minutes at 768px with `LET_GO_AFTER=8`:
    ///
    /// ```text
    ///                              before        after
    ///   rests of the fast road     44            0
    ///   pictures delivered         318 (1.1/s)   4,713 (15.7/s)
    ///   helpers started            45            1
    ///   ask -> first picture       -             (median ms, printed below)
    /// ```
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs an iOS simulator; with ZEROCODE_LIVE_BOOT it shuts that device down first"]
    fn the_panes_turn_column_keeps_the_push_road() {
        let Ok(udid) = std::env::var("ZEROCODE_LIVE_SIMULATOR") else {
            println!("LIVE: no simulator named; nothing measured");
            return;
        };
        let number = |name: &str, or: u64| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(or)
        };
        let flag = |name: &str, value: &str| std::env::var(name).as_deref() == Ok(value);
        let seconds = number("ZEROCODE_LIVE_SECONDS", 60);
        let attention_every = number("ZEROCODE_LIVE_ATTENTION_EVERY_MS", 0);
        let poke_every = number("ZEROCODE_LIVE_POKE_EVERY_MS", 0);
        let from_shutdown = flag("ZEROCODE_LIVE_BOOT", "1");
        let real_slow_road = flag("ZEROCODE_LIVE_SLOW_ROAD", "screenshot");
        let let_go_after = number("ZEROCODE_LIVE_LET_GO_AFTER", 0);
        if let_go_after > 0 {
            super::super::ios_hid::let_go_of_connections_after(let_go_after);
            println!(
                "LIVE: the frame reader lets each connection go after {let_go_after} pictures"
            );
        }
        // The window's control is born engaged and hears that nobody is over
        // the pane once the pane has laid itself out — within its first turns.
        const ATTENTION_REPORT_AFTER: Duration = Duration::from_millis(300);
        // What the slow road costs on every turn the fast one answers Absent
        // when it is not walked for real: one read of the Simulator's window
        // (27-28ms measured).
        const SLOW_ROAD_CAPTURE: Duration = Duration::from_millis(28);
        // The pane in the window's log was 353x768.
        let long_edge = resolve_long_edge(Some(768));
        if flag("ZEROCODE_LIVE_SIGCHLD", "tokio") {
            // One child through tokio is what makes tokio install its
            // SIGCHLD handler for the whole process, as the window's own
            // first tokio child does.
            let runtime = tokio::runtime::Runtime::new().expect("a runtime");
            runtime
                .block_on(async {
                    crate::proc::quiet_tokio_command("/usr/bin/true")
                        .status()
                        .await
                })
                .expect("a child through tokio");
            println!("LIVE: tokio's SIGCHLD handler is installed");
        }
        if from_shutdown {
            let _ = shutdown_simulator(&udid);
            std::thread::sleep(Duration::from_secs(2));
        }
        let started = Instant::now();
        let boot = if from_shutdown {
            let device = SimulatorDevice {
                udid: udid.clone(),
                name: "live".to_string(),
                booted: false,
                runtime: "live".to_string(),
            };
            boot_simulator(&device).expect("the device boots");
            println!("LIVE: +{}ms boot accepted", started.elapsed().as_millis());
            let watch = BootWatch::waking();
            let watching = watch.clone();
            let device = udid.clone();
            std::thread::spawn(move || {
                let began = Instant::now();
                let _ = simctl_command()
                    .args(["bootstatus", &device])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                watching.finished();
                println!(
                    "LIVE: +{}ms the device finished booting",
                    began.elapsed().as_millis()
                );
            });
            watch
        } else {
            BootWatch::already_up()
        };
        if flag("ZEROCODE_LIVE_SIMULATOR_APP", "1") {
            // What the window does between the boot and the pump: the
            // Simulator application is asked for, hidden, so the device has
            // a window of its own — and the application is on the device
            // the whole time the helper reads its framebuffer.
            prepare_simulator_services(&udid);
            crate::simulator_window::hide_simulator();
            println!(
                "LIVE: +{}ms Simulator.app asked for, hidden",
                started.elapsed().as_millis()
            );
        }
        let attaching = udid.clone();
        let attached = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let standing = attached.clone();
        let asking = std::thread::spawn(move || {
            while !standing.load(std::sync::atomic::Ordering::Acquire) {
                if super::super::ios_hid::retain(&attaching).is_ok() {
                    standing.store(true, std::sync::atomic::Ordering::Release);
                    return;
                }
                std::thread::sleep(HELPER_ATTACH_RETRY);
            }
        });
        let poking = (poke_every > 0).then(|| {
            let udid = udid.clone();
            let done = attached.clone();
            let every = Duration::from_millis(poke_every);
            std::thread::spawn(move || {
                let mut down = true;
                while !done.load(std::sync::atomic::Ordering::Acquire) {
                    std::thread::sleep(every);
                    if super::super::ios_hid::standing(&udid)
                        != super::super::ios_hid::HelperStanding::Standing
                    {
                        continue;
                    }
                    let (from, to) = if down { (0.3, 0.7) } else { (0.7, 0.3) };
                    down = !down;
                    let _ = super::super::ios_hid::send(
                        &udid,
                        super::super::ios_hid::InputRequest::Swipe {
                            x1: 0.5,
                            y1: from,
                            x2: 0.5,
                            y2: to,
                            duration_ms: 200,
                        },
                    );
                }
            })
        });
        let frame = std::env::temp_dir().join(format!(
            "zc-turn-column-{}.jpg",
            uuid::Uuid::new_v4().simple()
        ));
        let mut road = HelperRoad::new();
        let mut last_emitted: Option<Instant> = None;
        let (mut turns, mut pictures, mut stills, mut absents, mut rests, mut asks, mut shots) =
            (0u32, 0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
        let mut unchanged = 0u32;
        let mut first_picture: Option<Duration> = None;
        let mut resting_seen = false;
        let mut bytes = 0u64;
        // What a re-negotiation COSTS the pane: from the ask that retires the
        // pusher to the first picture the next one manages to push.
        let mut asked_at: Option<Instant> = None;
        let mut after_ask: Vec<u128> = Vec::new();
        while started.elapsed() < Duration::from_secs(seconds) {
            turns += 1;
            let now = started.elapsed();
            let engaged = if now < ATTENTION_REPORT_AFTER {
                true
            } else if attention_every == 0 {
                false
            } else {
                ((now - ATTENTION_REPORT_AFTER).as_millis() / u128::from(attention_every)) % 2 == 1
            };
            let refresh = last_emitted.is_none_or(|at| at.elapsed() >= IDLE_REFRESH_INTERVAL);
            let turn = Turn {
                long_edge,
                max_fps: frame_rate_for(engaged),
                refresh,
                booting: boot.booting(),
            };
            let was = road.streaming_at;
            let answer = road.take_frame(&udid, turn, nowhere(), "harness");
            if was != road.streaming_at && road.streaming_at.is_some() {
                asks += 1;
                asked_at = Some(Instant::now());
                println!(
                    "LIVE: +{}ms stream asked at {long_edge}px {}fps{}",
                    now.as_millis(),
                    turn.max_fps,
                    if turn.booting { " (booting)" } else { "" }
                );
            }
            match answer {
                HelperFrameAnswer::Picture(picture) => {
                    pictures += 1;
                    bytes += picture.bytes.len() as u64;
                    if let Some(at) = asked_at.take() {
                        after_ask.push(at.elapsed().as_millis());
                    }
                    unchanged = 0;
                    last_emitted = Some(Instant::now());
                    if first_picture.is_none() {
                        first_picture = Some(now);
                        println!(
                            "LIVE: +{}ms first picture {}x{} ({} bytes)",
                            now.as_millis(),
                            picture.width,
                            picture.height,
                            picture.bytes.len()
                        );
                    }
                }
                HelperFrameAnswer::Still => stills += 1,
                HelperFrameAnswer::Absent => {
                    absents += 1;
                    if road.resting_since.is_some() && !resting_seen {
                        rests += 1;
                        resting_seen = true;
                        println!("LIVE: +{}ms the road rested", now.as_millis());
                    }
                    // The slow road: one capture, then the rest the pump takes
                    // after it — the boot's short one while the device is
                    // coming up, the idle ladder once it is not.
                    if real_slow_road {
                        shots += 1;
                        let _ = std::fs::remove_file(&frame);
                        let _ = simctl_command()
                            .args(["io", &udid, "screenshot", "--type=jpeg"])
                            .arg(&frame)
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                        let _ = crate::proc::quiet_command("sips")
                            .args(["-Z", "900"])
                            .arg(&frame)
                            .arg("--out")
                            .arg(&frame)
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    } else {
                        std::thread::sleep(SLOW_ROAD_CAPTURE);
                    }
                    unchanged = unchanged.saturating_add(1);
                    std::thread::sleep(if turn.booting {
                        BOOTING_MISS_REST
                    } else {
                        FALLBACK_IDLE_LADDER.delay(unchanged)
                    });
                }
            }
            if road.resting_since.is_none() {
                resting_seen = false;
            }
        }
        let elapsed = started.elapsed().as_secs_f64();
        after_ask.sort_unstable();
        let at = |part: f64| {
            after_ask
                .get(((after_ask.len() as f64 - 1.0) * part).round() as usize)
                .map_or_else(|| "-".to_string(), |ms| format!("{ms}ms"))
        };
        println!(
            "LIVE: {seconds}s at {long_edge}px — turns {turns}, stream asks {asks}, pictures {pictures} \
             ({:.1}/s, {} KB), stills {stills}, absents {absents}, screenshots {shots}, rests {rests}, \
             misses now {}, first picture {}",
            f64::from(pictures) / elapsed,
            bytes / 1024,
            road.misses,
            first_picture.map_or("never".to_string(), |at| format!("at {}ms", at.as_millis()))
        );
        println!(
            "LIVE: ask -> first picture over {} re-negotiations: median {}, p90 {}, worst {}",
            after_ask.len(),
            at(0.5),
            at(0.9),
            at(1.0)
        );
        attached.store(true, std::sync::atomic::Ordering::Release);
        let _ = asking.join();
        if let Some(poking) = poking {
            let _ = poking.join();
        }
        let _ = std::fs::remove_file(&frame);
        road.stop_streaming(&udid);
        super::super::ios_hid::release(&udid);
        assert_eq!(
            rests, 0,
            "the framebuffer road rested {rests} times in {seconds}s"
        );
    }

    #[test]
    fn typed_simctl_parser_ignores_unavailable_devices() {
        let parsed: SimctlList = serde_json::from_str(
            r#"{"devices":{"runtime":[
                {"udid":"one","name":"iPhone","state":"Booted","isAvailable":true},
                {"udid":"two","name":"Old","state":"Shutdown","isAvailable":false}
            ]}}"#,
        )
        .unwrap();
        assert_eq!(parsed.devices["runtime"].len(), 2);
        assert_eq!(parsed.devices["runtime"][0].state, "Booted");
    }
}
