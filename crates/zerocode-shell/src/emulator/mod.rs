//! Mobile emulator backend.
//!
//! The crate root wires Tauri commands. Platform-specific discovery and
//! control live in [`ios`] and [`android`]; [`session`] is the shared lifetime
//! boundary. Keeping those responsibilities separate is important here:
//! streaming processes are long-lived and a platform probe must never become
//! a second, slightly different owner of their cleanup.

mod android;
mod capability;
pub(crate) mod checks;
mod ios;
#[cfg(target_os = "macos")]
mod ios_hid;
pub(crate) mod marks;
mod prefs;
mod process;
mod pump;
mod session;
#[cfg(all(test, target_os = "macos"))]
mod walk_bench;

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, ipc::InvokeResponseBody};

pub(crate) use android::{
    AvdSnapshot, android_accessibility_tree, android_accessibility_tree_direct,
    android_avd_snapshot, android_button, android_button_direct, android_emulators,
    android_emulators_direct, android_install_app, android_launch_app, android_logs,
    android_rotate, android_rotate_direct, android_screenshot_direct, android_set_permission,
    android_swipe, android_swipe_direct, android_tap, android_tap_direct, android_text,
    android_text_direct, managed_resource_processes, shutdown_android_emulator,
    start_android_stream, start_emulator_video,
};
pub(crate) use ios::{
    ios_accessibility_tree, ios_accessibility_tree_direct, ios_button, ios_button_direct,
    ios_install_app, ios_launch_app, ios_logs, ios_multi_touch, ios_rotate, ios_rotate_direct,
    ios_screenshot_direct, ios_set_permission, ios_swipe, ios_swipe_direct, ios_tap,
    ios_tap_direct, ios_text, ios_text_direct, ios_touch, mobile_emulators,
    mobile_emulators_direct, open_mobile_emulator, shutdown_mobile_emulator, start_emulator_stream,
};
use session::{Loan, SessionControl, StartVerdict, loans, registry};
pub(crate) use session::{LoanEnd, LoanSummary};

const MAX_FRAME_BYTES: u64 = 16 * 1024 * 1024;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(25);
const BINARY_SEQUENCE_BYTES: usize = size_of::<u64>();

type BinaryChannel = tauri::ipc::Channel<InvokeResponseBody>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EmulatorPlatform {
    Ios,
    Android,
}

impl EmulatorPlatform {
    /// The word the window log and the wire say it in.
    const fn word(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }
}

impl From<zerocode_core::computer_use::EmulatorPlatform> for EmulatorPlatform {
    fn from(asked: zerocode_core::computer_use::EmulatorPlatform) -> Self {
        match asked {
            zerocode_core::computer_use::EmulatorPlatform::Ios => Self::Ios,
            zerocode_core::computer_use::EmulatorPlatform::Android => Self::Android,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmulatorStream {
    pub stream: String,
    pub udid: String,
    pub name: String,
    pub platform: EmulatorPlatform,
    pub interactive: bool,
    pub reused: bool,
}

/// What the window hears when an agent asks for a device mirror
/// (`zerocode-emulator open`, `emulator:agent-open`): the platform, the device
/// when one was named, and the terminal whose agent asked, read off the pane
/// key its door presented. The window seats the mirror in that terminal's
/// checkout — the way `BrowserPopup.term` seats a browser tab — and a shell
/// whose door named no pane carries no `term`, so the window opens the mirror
/// where the person is looking and says why (t-6379).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct AgentOpen {
    platform: zerocode_core::computer_use::EmulatorPlatform,
    device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    term: Option<u32>,
}

impl AgentOpen {
    pub(crate) fn asked(
        platform: zerocode_core::computer_use::EmulatorPlatform,
        device: Option<String>,
        pane: Option<&str>,
    ) -> Self {
        Self {
            platform,
            device,
            term: pane.and_then(crate::hooks::term_of_pane_key),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmulatorPayload {
    stream: String,
    sequence: u64,
    bytes: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime: Option<&'static str>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum EmulatorNoteCode {
    FrameUnavailable,
    StreamEnded,
    VideoStartFailed,
    /// The device itself is not on the bridge — killed, still booting, or
    /// `offline`. Not a sick stream: the pump is resting beside it and will
    /// carry on the moment `adb` lists it again, which is why this says
    /// "it will resume" rather than asking anybody to restart anything.
    DeviceOffline,
}

impl EmulatorNoteCode {
    /// The word this note travels as.
    ///
    /// The same spelling `serde` writes, said once here so a REFUSAL can
    /// carry it too — a tap turned away because the device is gone and the
    /// note under the pane are the same fact, and must not become two
    /// sentences that drift apart. A test pins these spellings to the ones
    /// `serde` puts on the wire.
    const fn code(self) -> &'static str {
        match self {
            Self::FrameUnavailable => "frame-unavailable",
            Self::StreamEnded => "stream-ended",
            Self::VideoStartFailed => "video-start-failed",
            Self::DeviceOffline => "device-offline",
        }
    }
}

#[derive(Clone, Serialize)]
struct EmulatorNote {
    stream: String,
    code: EmulatorNoteCode,
}

/// How big a picture one pane can actually paint.
///
/// The long edge is in the DEVICE pixels the pane will paint with — its CSS box
/// times the display's scale — because that is the only number at which a
/// picture is neither blurred by being stretched nor paid for in bytes nobody
/// can see. A pane that never says gets its platform's own default size.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Viewport {
    pub long_edge_px: u32,
}

/// Whether a pump would rather wait for the renderer or skip this picture.
#[derive(Clone, Copy)]
enum SlotWait {
    /// Wait until the renderer has room. What a pump that ASKS for each
    /// picture wants: the frame it would take instead does not exist yet, and
    /// taking it is the expensive half.
    Block,
    /// Skip this picture if the renderer has no room. What a pump being PUSHED
    /// to wants: a newer frame is already on its way, so queuing this one only
    /// guarantees the person sees the past.
    Skip,
}

/// What became of one picture handed to [`emit_bytes`].
enum EmitOutcome {
    Sent,
    /// The renderer was full and this picture was let go. The stream is fine.
    Skipped,
    /// The door is gone, or the picture was unusable. The pump is done.
    Closed,
}

/// Emit one bounded payload after taking one of the stream's in-flight slots.
fn emit_bytes(
    app: &AppHandle,
    control: &Arc<SessionControl>,
    event: &'static str,
    stream: &str,
    bytes: &[u8],
    mime: Option<&'static str>,
    slot: SlotWait,
) -> EmitOutcome {
    if bytes.is_empty() || bytes.len() as u64 > MAX_FRAME_BYTES {
        return EmitOutcome::Closed;
    }
    // Read the door for THIS frame rather than holding the one the pump was
    // started with: a pane re-opening the same device is handed this session
    // and brings a channel of its own, and the pump must follow it there.
    let channel = control.frame_door();
    let reserved = match slot {
        SlotWait::Block => control.reserve_payload(),
        SlotWait::Skip => control.try_reserve_payload(),
    };
    let Some(sequence) = reserved else {
        return match slot {
            // The waiting road only comes back empty-handed when the stream
            // itself went away under it, which is the same fact it has always
            // reported here.
            SlotWait::Block => EmitOutcome::Closed,
            SlotWait::Skip => EmitOutcome::Skipped,
        };
    };
    if let Some(channel) = channel {
        let payload = binary_payload(sequence, bytes);
        if channel.send(InvokeResponseBody::Raw(payload)).is_err() {
            control.acknowledge(sequence);
            return EmitOutcome::Closed;
        }
        return EmitOutcome::Sent;
    }
    let payload = EmulatorPayload {
        stream: stream.to_string(),
        sequence,
        bytes: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime,
    };
    if app.emit_to("main", event, payload).is_err() {
        control.acknowledge(sequence);
        return EmitOutcome::Closed;
    }
    EmitOutcome::Sent
}

fn binary_payload(sequence: u64, bytes: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(BINARY_SEQUENCE_BYTES + bytes.len());
    payload.extend_from_slice(&sequence.to_be_bytes());
    payload.extend_from_slice(bytes);
    payload
}

/// One stream's input capability, told as its own word.
///
/// It is a word rather than a field of the start answer because finding it out
/// costs a helper's cold start — and the first picture does not depend on it.
/// A pane that is drawing but not yet touchable is a true state; a pane that is
/// blank because we are still asking is not.
#[derive(Clone, Serialize)]
struct EmulatorCapability {
    stream: String,
    interactive: bool,
}

fn emit_capability(app: &AppHandle, stream: &str, interactive: bool) {
    let _ = app.emit_to(
        "main",
        "emulator:capability",
        EmulatorCapability {
            stream: stream.to_string(),
            interactive,
        },
    );
}

fn emit_note(app: &AppHandle, stream: &str, code: EmulatorNoteCode) {
    let _ = app.emit_to(
        "main",
        "emulator:note",
        EmulatorNote {
            stream: stream.to_string(),
            code,
        },
    );
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let size = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    if size == 0 {
        return Err("빈 에뮬레이터 프레임입니다".to_string());
    }
    if size > MAX_FRAME_BYTES {
        return Err(format!(
            "에뮬레이터 프레임이 제한({MAX_FRAME_BYTES}바이트)을 넘었습니다"
        ));
    }
    std::fs::read(path).map_err(|error| error.to_string())
}

/// Wait for the child currently installed in `control`, killing and collecting
/// it on timeout. `false` also covers a pause/stop that deliberately removed it.
fn wait_for_child(control: &SessionControl, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match control.child_status() {
            Some(Ok(Some(status))) => {
                if let Some(mut child) = control.take_child() {
                    let _ = child.wait();
                }
                return status.success();
            }
            Some(Ok(None)) if Instant::now() < deadline => {
                std::thread::sleep(PROCESS_POLL_INTERVAL);
            }
            Some(Ok(None) | Err(_)) => {
                control.kill_child();
                return false;
            }
            None => return false,
        }
    }
}

#[tauri::command]
pub(crate) fn stop_emulator_stream(webview: tauri::Webview, stream: String) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    registry().stop(&stream);
    Ok(())
}

#[tauri::command]
pub(crate) fn acknowledge_emulator_payload(
    webview: tauri::Webview,
    stream: String,
    sequence: u64,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    registry().acknowledge(&stream, sequence);
    Ok(())
}

#[tauri::command]
pub(crate) fn set_emulator_stream_paused(
    webview: tauri::Webview,
    stream: String,
    paused: bool,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    registry().set_paused(&stream, paused);
    Ok(())
}

/// Whether somebody is attending to the pane. Not a pause: the picture keeps
/// coming, at the rate a glance needs rather than the rate a touch needs.
#[tauri::command]
pub(crate) fn set_emulator_stream_engaged(
    webview: tauri::Webview,
    stream: String,
    engaged: bool,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    registry().set_engaged(&stream, engaged);
    Ok(())
}

/// Tell a live stream how big its pane became.
///
/// Its own command rather than a field of the start answer because a pane is
/// resized far more often than it is opened — dragging a split, closing the
/// sidebar, zooming the window — and every one of those changes the only
/// number that decides whether the picture is sharp.
#[tauri::command]
pub(crate) fn set_emulator_stream_viewport(
    webview: tauri::Webview,
    stream: String,
    viewport: Viewport,
) -> Result<(), String> {
    crate::from_the_main_webview(&webview)?;
    registry().set_viewport(&stream, viewport.long_edge_px);
    Ok(())
}

/// Pick a platform-appropriate app artifact with the native panel — in the
/// helper's process, through the one seat every picker rides
/// (`pick_paths`). Validation happens again in the installer; doing it here
/// gives an immediate answer for a package/file mismatch without exposing a
/// general filesystem picker.
#[tauri::command]
pub(crate) async fn choose_emulator_app(
    webview: tauri::Webview,
    app: AppHandle,
    platform: EmulatorPlatform,
) -> Result<Option<String>, String> {
    crate::from_the_main_webview(&webview)?;
    let request = zerocode_core::pick::PickRequest::new(zerocode_core::pick::PickKind::File);
    let request = match platform {
        EmulatorPlatform::Ios => request.filtered("iOS Simulator app", &["app"]),
        EmulatorPlatform::Android => request.filtered("Android APK", &["apk"]),
    };
    let mut picked = crate::pick_paths(&app, request).await?;
    let Some(path) = picked.pop() else {
        return Ok(None);
    };
    let path = path.to_string_lossy().into_owned();
    let checked = match platform {
        EmulatorPlatform::Ios => capability::app_path(&path, "app", true),
        EmulatorPlatform::Android => capability::app_path(&path, "apk", false),
    }?;
    Ok(Some(checked.to_string_lossy().into_owned()))
}

/// What the window does for the emulators when it starts.
///
/// Its own door rather than three calls from the boot, because all of it is
/// the same sentence — the device somebody will ask for next should already
/// be awake, and the one nobody asks for should not stay awake forever — and
/// none of it may hold the boot up: the work runs on its own thread, and the
/// reclaimers each start theirs.
pub(crate) fn on_window_boot(app: &AppHandle) {
    let _ = WINDOW.set(app.clone());
    let app = app.clone();
    // One thread for all of it: the settings read, a device listing and a
    // boot are file and process work, and a window that waits on a simulator
    // before it paints has made a convenience into a launch delay.
    let _ = std::thread::Builder::new()
        .name("emulator-boot".to_string())
        .spawn(move || {
            preboot_last_used(&app);
            android::arm_idle_reclaim(&app);
            ios::start_idle_reclaimer(app);
        });
}

/// Put the last-used device of each platform up while the window is still
/// painting (D4).
///
/// One entry point rather than one per platform, because "the device the next
/// pane will want" is one question with a per-platform answer, and the window
/// boot has no business knowing how many platforms there are — nor whether the
/// person wants it at all: `emulator.prebootLastUsed` is read here, once, for
/// both. Nothing here opens a stream: prebooting is the device only, and the
/// pane that arrives later joins the launch already in flight.
pub(crate) fn preboot_last_used(app: &AppHandle) {
    if !prefs::of(app).preboot_last_used {
        return;
    }
    android::preboot_last_used(app);
    ios::preboot_last_used(app);
}

/// Close every pane, and put away the devices the window is not keeping.
///
/// `emulator.keepBooted` (D3) is read here, at exit, because the switch may
/// have been turned in this session: the panes always go — nothing is left
/// pumping into a window that is gone — and the devices stay up so the next
/// window's first pane opens on a resume rather than a cold boot. A lent
/// device is not the person's to keep: its borrowers are panes, and they go
/// with this window, so it goes down with them whatever the switch says
/// (t-6336) — nothing would be left to remember whose it was.
pub(crate) fn shutdown_all(app: &AppHandle) {
    let prefs = prefs::of(app);
    put_away(loans().take_all(), LoanEnd::WindowExit);
    registry().shutdown_all();
    android::shutdown_all_devices(prefs.keep_booted);
    ios::devices_at_exit(
        prefs.keep_booted,
        app.state::<crate::AppState>().local_data_root(),
    );
    #[cfg(target_os = "macos")]
    ios_hid::shutdown_all();
}

/// The window the loan roads speak to: set once at boot, so the roads that
/// end a loan — a pane closing, a worker reporting done, the idle reclaimer —
/// need no handle of their own.
static WINDOW: OnceLock<AppHandle> = OnceLock::new();

/// Tell the window how many devices are lent (the status bar's one line).
fn announce_loans() {
    if let Some(app) = WINDOW.get() {
        let _ = app.emit_to("main", "emulator:loans", loans().summary());
    }
}

/// The booted simulators and emulators on this machine, for the task board's
/// machine strip (t-6588): iOS off `simctl` (macOS only — elsewhere nobody
/// can say), Android off `adb`. Processes both, so never on the main thread.
pub(crate) fn booted_devices() -> (Option<usize>, Option<usize>) {
    (
        cfg!(target_os = "macos").then(ios::booted_simulators),
        android::booted_emulators(),
    )
}

/// The loan book's line, for a window that opens — or reloads — while
/// devices are lent.
#[tauri::command]
pub(crate) fn emulator_loans(webview: tauri::Webview) -> Result<LoanSummary, String> {
    crate::from_the_main_webview(&webview)?;
    Ok(loans().summary())
}

/// A stream start, judged for the loan book and said in the window log —
/// both platforms' starts call this once they know whether they booted the
/// device (`booted_here`) and which pane asked (`borrower`, none for the
/// person).
fn note_start(
    app: &AppHandle,
    platform: EmulatorPlatform,
    device: &str,
    name: &str,
    borrower: Option<u32>,
    booted_here: bool,
) {
    let said = match loans().note_start(
        platform,
        device,
        borrower,
        booted_here,
        crate::now_epoch_ms(),
    ) {
        StartVerdict::Lend(term) => format!("lent to term {term}"),
        StartVerdict::Join(term) => format!("also lent to term {term}"),
        StartVerdict::Keep => "kept: the person opened it, so it is theirs now".to_string(),
        StartVerdict::Nobody => return,
    };
    crate::note_window_event(
        app.state::<crate::AppState>().local_data_root(),
        &format!("emulator {}: {name} ({device}) {said}", platform.word()),
    );
    announce_loans();
}

/// The door used this device (any verb that named it): a lent one's last use
/// moves.
pub(crate) fn used_through_the_door(
    platform: zerocode_core::computer_use::EmulatorPlatform,
    device: &str,
) {
    loans().touch(platform.into(), device, crate::now_epoch_ms());
}

/// One of the window's own roads put this device down — its 끄기 button, the
/// idle reclaimer, a device that left on its own: nobody's loan any more.
fn loan_put_down(platform: EmulatorPlatform, device: &str) {
    if loans().forget(platform, device) {
        announce_loans();
    }
}

/// `term`'s work ended — its pane closed, its worker reported
/// `worker_done`, its agent said its session ended (t-6336). The devices the
/// door put up for it and for nobody else go down, on a thread of their own:
/// a simulator takes a second to shut down and an AVD's saved exit up to
/// thirty, and none of the three roads that call this may wait on either.
/// The common pane has lent nothing, and its end costs one lock.
pub(crate) fn borrower_gone(term: u32, end: LoanEnd) {
    let returned = loans().returned_by(term);
    if returned.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name(format!("emulator-return-{term}"))
        .spawn(move || put_away(returned, end));
}

/// What the window hears when a loan is returned: which device, so it takes
/// the borrowers' mirrors of it off the strip.
#[derive(Clone, Serialize)]
struct LoanReturned {
    platform: EmulatorPlatform,
    device: String,
}

/// Put returned loans away, each in the one order that leaves nothing
/// pointing at a device that is gone: its streams stop, the device goes down
/// by its platform's own power road, and then the window hears — it takes the
/// borrowers' mirrors off the strip.
fn put_away(returned: Vec<Loan>, end: LoanEnd) {
    if returned.is_empty() {
        return;
    }
    put_away_with(
        returned,
        |loan| registry().stop_device(loan.platform, &loan.device),
        |loan| match loan.platform {
            EmulatorPlatform::Ios => ios::put_away(&loan.device),
            EmulatorPlatform::Android => WINDOW.get().map_or(Ok(()), |app| {
                android::put_away(
                    app.state::<crate::AppState>().local_data_root(),
                    &loan.device,
                )
            }),
        },
        |loan, stopped, outcome, took| {
            let Some(app) = WINDOW.get() else {
                return;
            };
            let said = match outcome {
                Ok(()) => format!("shut down in {} ms", took.as_millis()),
                Err(error) => format!("would not shut down: {error}"),
            };
            crate::note_window_event(
                app.state::<crate::AppState>().local_data_root(),
                &format!(
                    "emulator {}: lent {} returned ({}) — {stopped} stream(s) stopped, {said}",
                    loan.platform.word(),
                    loan.device,
                    end.word(),
                ),
            );
            let _ = app.emit_to(
                "main",
                "emulator:loan-returned",
                LoanReturned {
                    platform: loan.platform,
                    device: loan.device.clone(),
                },
            );
        },
    );
    announce_loans();
}

/// [`put_away`]'s order, with the three hands passed in so the order is a
/// test rather than a hope.
fn put_away_with(
    returned: Vec<Loan>,
    mut stop_streams: impl FnMut(&Loan) -> usize,
    mut shut_down: impl FnMut(&Loan) -> Result<(), String>,
    mut tell: impl FnMut(&Loan, usize, &Result<(), String>, Duration),
) {
    for loan in &returned {
        let began = Instant::now();
        let stopped = stop_streams(loan);
        let outcome = shut_down(loan);
        tell(loan, stopped, &outcome, began.elapsed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A return puts each device away in the one order that leaves nothing
    /// pointing at a device that is gone (t-6336): its streams stop, then the
    /// device goes down by its platform's road, then the window hears — and a
    /// device that would not go down is still reported, with why, and does
    /// not stop the next one.
    #[test]
    fn a_return_stops_the_streams_then_the_device_then_tells_the_window() {
        let book = session::LoanBook::default();
        book.note_start(EmulatorPlatform::Ios, "lent-sim", Some(5), true, 1);
        book.note_start(EmulatorPlatform::Android, "lent_avd", Some(5), true, 2);
        book.note_start(EmulatorPlatform::Ios, "persons-sim", None, true, 3);
        let steps = std::cell::RefCell::new(Vec::new());
        put_away_with(
            book.returned_by(5),
            |loan| {
                steps.borrow_mut().push(format!("stop {}", loan.device));
                1
            },
            |loan| {
                steps.borrow_mut().push(format!("down {}", loan.device));
                if loan.platform == EmulatorPlatform::Android {
                    Err("the emulator process would not stop".to_string())
                } else {
                    Ok(())
                }
            },
            |loan, stopped, outcome, _| {
                steps.borrow_mut().push(format!(
                    "tell {} {stopped} {}",
                    loan.device,
                    outcome
                        .as_ref()
                        .map_or_else(Clone::clone, |()| "down".to_string())
                ));
            },
        );
        assert_eq!(
            steps.into_inner(),
            [
                "stop lent-sim",
                "down lent-sim",
                "tell lent-sim 1 down",
                "stop lent_avd",
                "down lent_avd",
                "tell lent_avd 1 the emulator process would not stop",
            ]
        );
    }

    #[test]
    fn raw_channel_payload_prefixes_one_big_endian_sequence_without_reencoding_bytes() {
        let payload = binary_payload(42, &[0, 1, 0xff]);
        assert_eq!(&payload[..BINARY_SEQUENCE_BYTES], &42u64.to_be_bytes());
        assert_eq!(&payload[BINARY_SEQUENCE_BYTES..], &[0, 1, 0xff]);
    }

    #[test]
    fn native_notes_cross_the_wire_as_locale_independent_codes() {
        assert_eq!(
            serde_json::to_value(EmulatorNoteCode::FrameUnavailable).unwrap(),
            "frame-unavailable"
        );
    }

    /// An agent's `open` names the terminal that asked, read off the pane key
    /// its door presented (t-6379) — and nothing it cannot read: a shell whose
    /// door named no pane, or named something that is not a pane key, asks
    /// from nowhere the window knows, and the payload says so by carrying no
    /// `term` at all. The rest of the wire is what it always was.
    #[test]
    fn an_agents_open_names_the_terminal_that_asked_and_nothing_it_cannot_read() {
        use zerocode_core::computer_use::EmulatorPlatform as Asked;
        let wire = |open: AgentOpen| serde_json::to_value(open).expect("serializes");
        assert_eq!(
            wire(AgentOpen::asked(
                Asked::Ios,
                Some("U1".to_string()),
                Some(&crate::hooks::pane_key_of(7)),
            )),
            serde_json::json!({ "platform": "ios", "device": "U1", "term": 7 })
        );
        assert_eq!(
            wire(AgentOpen::asked(Asked::Android, None, None)),
            serde_json::json!({ "platform": "android", "device": null })
        );
        assert_eq!(
            wire(AgentOpen::asked(Asked::Ios, None, Some("tab-1/leaf-2"))),
            serde_json::json!({ "platform": "ios", "device": null })
        );
    }

    /// The word a note travels as and the word a refusal carries are the same
    /// word.
    ///
    /// A device that is gone says so twice — under the pane, from the pump,
    /// and in the error a refused tap hands back — and the window matches
    /// them by this spelling. Two spellings is a tap that reports "could not
    /// send input" while the pane beside it says the device is off.
    #[test]
    fn the_note_codes_travel_as_their_own_words() {
        for code in [
            EmulatorNoteCode::FrameUnavailable,
            EmulatorNoteCode::StreamEnded,
            EmulatorNoteCode::VideoStartFailed,
            EmulatorNoteCode::DeviceOffline,
        ] {
            assert_eq!(
                serde_json::to_value(code).unwrap(),
                code.code(),
                "the wire and the refusal spell this note differently"
            );
        }
    }
}
