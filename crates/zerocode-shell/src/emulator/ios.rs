//! iOS Simulator discovery, lifecycle, frame capture and input commands.

mod capabilities;

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

fn boot_simulator(device: &SimulatorDevice) -> Result<(), String> {
    if device.booted {
        return Ok(());
    }
    let boot = simctl_command()
        .args(["boot", &device.udid])
        .output()
        .map_err(|error| error.to_string())?;
    let said = String::from_utf8_lossy(&boot.stderr);
    if !boot.status.success() && !said.contains("current state: Booted") {
        return Err(format!("시뮬레이터를 부팅할 수 없습니다: {}", said.trim()));
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

/// Hold the pane's first picture until the device can draw one.
///
/// `simctl boot` returns when CoreSimulator has ACCEPTED the boot, and every
/// road to a picture works from that instant — so without this the pane's
/// first frame is the boot screen, measured at 6.6KB of black with a spinner
/// four pixels across. Everything downstream is then correct and useless: the
/// pane paints it, calls itself connected, takes down the note that said the
/// device was waking, and offers the same black until the device finishes —
/// 9.6s on a warm boot here, a minute and a half on a first one. The note the
/// pane already has is a better answer than that picture, so this simply does
/// not produce the picture.
///
/// `bootstatus` is the platform's own answer to the question and not a
/// formality: measured, it holds until the device's system application is up,
/// which is the same moment the framebuffer stops being black.
///
/// Asked only for a device THIS pane booted. One that was already running has
/// answered it long ago, and paying even 0.18s of it on the way to the first
/// frame would be paying for nothing.
#[cfg(target_os = "macos")]
fn wait_until_the_device_can_draw(
    udid: &str,
    control: &SessionControl,
    husk_root: &Path,
    stream: &str,
) {
    let Ok(child) = simctl_command()
        .args(["bootstatus", udid])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        // A machine that cannot even spawn it keeps the behaviour it had:
        // frames from whenever, boot screen included.
        return;
    };
    if !control.install_child(child) {
        return;
    }
    let started = Instant::now();
    let ready = wait_for_child(control, BOOT_WAIT);
    // The pane is blank for the whole of this, so the one thing a person
    // reading the log will want is how long it was blank and why it stopped.
    crate::note_window_event(
        husk_root,
        &format!(
            "emulator ios pump {stream}: waited {}ms for the device to finish booting ({})",
            started.elapsed().as_millis(),
            if ready { "ready" } else { "gave up" }
        ),
    );
}

#[cfg(not(target_os = "macos"))]
fn wait_until_the_device_can_draw(
    _udid: &str,
    _control: &SessionControl,
    _husk_root: &Path,
    _stream: &str,
) {
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
    misses: u32,
    /// When the road was put down. Tried as if new once the retry has passed.
    resting_since: Option<Instant>,
    /// Whether this road has already said in the log that it is the one
    /// carrying the pane. A flag rather than "was this the first try", because
    /// the first try is exactly the one a cold start can lose and the answer
    /// would then be silence for the life of the stream.
    told: bool,
}

impl HelperRoad {
    fn new() -> Self {
        Self {
            streaming_at: None,
            misses: 0,
            resting_since: None,
            told: false,
        }
    }

    /// Count one failure, and put the road down once they stop being hiccups.
    fn stumbled(&mut self, error: &str, husk_root: &Path, stream: &str) -> HelperFrameAnswer {
        // Whatever failed, the helper is no longer known to be pushing at any
        // size — so the next healthy turn re-negotiates rather than assuming.
        self.streaming_at = None;
        self.misses = self.misses.saturating_add(1);
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
        long_edge: u32,
        max_fps: u32,
        refresh: bool,
        husk_root: &Path,
        stream: &str,
    ) -> HelperFrameAnswer {
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
        if !super::ios_hid::retained(udid) {
            return HelperFrameAnswer::Absent;
        }
        // Told once per size, not once per frame. This is the entire saving:
        // after this message the request lane is free for touches, and pictures
        // arrive on their own socket without anyone asking for them.
        if self.streaming_at != Some((long_edge, max_fps)) {
            if let Err(error) =
                super::ios_hid::stream_frames(udid, long_edge, FRAME_JPEG_QUALITY, max_fps)
            {
                return self.stumbled(&error, husk_root, stream);
            }
            self.streaming_at = Some((long_edge, max_fps));
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
                    Err(error) => self.stumbled(&error, husk_root, stream),
                }
            }
            Err(error) => self.stumbled(&error, husk_root, stream),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn take_frame(
        &mut self,
        _udid: &str,
        _long_edge: u32,
        _max_fps: u32,
        _refresh: bool,
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
    waking: bool,
    control: Arc<SessionControl>,
) {
    let _finished = FinishSession::new(stream.clone());
    let husk_root = app
        .state::<crate::AppState>()
        .local_data_root()
        .to_path_buf();
    // Before anything is captured, because every road can capture a boot
    // screen and none of them can tell one from a device.
    if waking {
        wait_until_the_device_can_draw(&udid, &control, &husk_root, &stream);
    }
    let frame = capture_path(&stream);
    if let Some(parent) = frame.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
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
        let max_fps =
            frame_rate_for(control.is_engaged() || control.input_within(INPUT_BOOST_WINDOW));
        match helper.take_frame(&udid, long_edge, max_fps, refresh, &husk_root, &stream) {
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
                if misses.missed() {
                    crate::note_window_event(
                        &husk_root,
                        &format!(
                            "emulator ios pump {stream} udid {udid}: screenshots not answering"
                        ),
                    );
                    emit_note(&app, &stream, EmulatorNoteCode::FrameUnavailable);
                }
                control.rest(FALLBACK_MISS_REST);
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

#[tauri::command]
pub(crate) async fn start_emulator_stream(
    app: AppHandle,
    webview: tauri::Webview,
    udid: Option<String>,
    viewport: Option<Viewport>,
    on_frame: BinaryChannel,
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
                return Ok(stream);
            }
            StartClaim::Acquired(lease) => lease,
        };
        // Whether the boot above is OURS is the only thing that decides
        // whether the pump waits for one, and it has to be read before the
        // boot makes the answer yes.
        let waking = !chosen.booted;
        boot_simulator(&chosen)?;
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
        crate::note_window_event(
            app.state::<crate::AppState>().local_data_root(),
            &format!(
                "emulator ios stream {stream_id} picked {} ({})",
                chosen.name, chosen.udid
            ),
        );
        let descriptor = lease.activate(descriptor, control.clone());
        let pump_app = app.clone();
        let pump_stream = stream_id.clone();
        let pump_udid = chosen.udid.clone();
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
                    waking,
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
                let mut rest = Duration::from_secs(3);
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
                    rest = (rest * 2).min(Duration::from_secs(60));
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
        let out = simctl_command()
            .args(["shutdown", &chosen.udid])
            .output()
            .map_err(|error| error.to_string())?;
        let said = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() && !said.contains("current state: Shutdown") {
            return Err(format!("시뮬레이터를 끌 수 없습니다: {}", said.trim()));
        }
        Ok(())
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
) -> Result<(), zerocode_core::computer_use_protocol::ProviderError> {
    use super::marks::backend_error;
    tauri::async_runtime::spawn_blocking(move || {
        let control = ios_control(&udid).map_err(backend_error)?;
        let input = control.input().map_err(backend_error)?;
        let snapshot = marks_snapshot(&udid).map_err(backend_error)?;
        request.on_device(&udid, || {
            request.perform_in(&input, &snapshot.faces, snapshot.screen, |x, y| {
                #[cfg(target_os = "macos")]
                super::ios_hid::send(&udid, super::ios_hid::InputRequest::Tap { x, y })
                    .map_err(backend_error)?;
                #[cfg(not(target_os = "macos"))]
                let _ = (x, y, run_ios_input(&udid, ()).map_err(backend_error)?);
                control.notify();
                Ok(())
            })
        })
    })
    .await
    .map_err(backend_error)?
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

#[cfg(test)]
mod tests {
    use super::*;

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
