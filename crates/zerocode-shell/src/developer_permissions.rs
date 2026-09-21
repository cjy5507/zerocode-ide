//! macOS privacy controls for terminal-launched developer tools.
//!
//! The renderer receives only a closed permission identifier and a coarse
//! status. System Settings URLs, probes, and network validation stay in this
//! native boundary so the webview cannot turn this surface into an arbitrary
//! process launcher or socket client.
//!
//! Every judgement here is about **this window**. ZeroCode.app is the
//! responsible process for the tools its panes launch, so what macOS answers
//! this process is what those tools inherit. The Computer Use helper opens
//! with `open -n` and carries its own TCC identity; it must never stand in
//! for this one.

use serde::{Deserialize, Serialize};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionId {
    Microphone,
    Camera,
    Screen,
    Accessibility,
    FullDiskAccess,
    Automation,
    LocalNetwork,
    Usb,
    Bluetooth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionStatus {
    Granted,
    Denied,
    Unknown,
    Ready,
    #[cfg_attr(
        target_os = "macos",
        expect(dead_code, reason = "serialized for non-macOS clients")
    )]
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionAction {
    OpenSettings,
    TriggerPrompt,
}

/// The two capture devices, spelled once for both the read and the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Audio,
    Video,
}

impl MediaKind {
    /// The Info.plist key macOS demands before it will raise this dialog.
    ///
    /// Asking without it does not fail — it **terminates the process**. The
    /// keys live in `crates/zerocode-shell/Info.plist`, which tauri-bundler
    /// merges into the app; the source contract keeps the two lists paired.
    #[cfg_attr(
        not(target_os = "macos"),
        expect(dead_code, reason = "Info.plist is a macOS bundle's")
    )]
    const fn usage_description_key(self) -> &'static str {
        match self {
            Self::Audio => "NSMicrophoneUsageDescription",
            Self::Video => "NSCameraUsageDescription",
        }
    }
}

/// The one native call that answers for a judged row.
///
/// Each variant names an entry point, not a permission: the probe is what
/// macOS is actually asked, and two rows asking the same way would name the
/// same variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// `AVCaptureDevice.authorizationStatusForMediaType:`
    CaptureDevice(MediaKind),
    /// `CGPreflightScreenCaptureAccess()`
    ScreenCapture,
    /// `AXIsProcessTrusted()`
    Accessibility,
    /// Opening the user's own `TCC.db`, which only Full Disk Access can read.
    FullDiskAccess,
    /// `AEDeterminePermissionToAutomateTarget(com.apple.systemevents, …)`
    Automation,
    /// `CBCentralManager.authorization` — the class property, so nothing is
    /// instantiated and no scan prompt is raised by the reading.
    Bluetooth,
}

/// How a permission's status is decided — the page's one answer to
/// "where does this number come from?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JudgedBy {
    /// macOS answers; this probe is the call that asks it.
    Api(Probe),
    /// macOS exposes no query for this permission at this layer. The page
    /// stands at the honest constant carried here and the row's own hint
    /// (`settings.permissions.*Hint`) says why there is nothing to read.
    NoApi(PermissionStatus),
}

/// What raises macOS's own dialog for a row, and the call that raises it.
///
/// macOS shows each of these once per process identity. A row whose answer
/// is already in therefore stops offering the trigger — see [`action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prompt {
    /// `AVCaptureDevice.requestAccessForMediaType:completionHandler:`, which
    /// asks in this window's name and calls back with the answer.
    CaptureDevice(MediaKind),
    /// One Apple Event aimed at System Events.
    Automation,
    /// One multicast datagram.
    LocalNetwork,
}

/// One permission: what macOS is asked, what raises its dialog, and the one
/// System Settings pane the row opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Permission {
    id: PermissionId,
    judged_by: JudgedBy,
    prompt: Option<Prompt>,
    settings_url: &'static str,
}

/// Every permission this page shows, judged in exactly one place.
///
/// One row per [`PermissionId`]. A second list — a `match` over ids for
/// settings URLs, a second inventory in a command — would be a second answer
/// that drifts; the source contract forbids one.
const PERMISSIONS: [Permission; 9] = [
    Permission {
        id: PermissionId::Microphone,
        judged_by: JudgedBy::Api(Probe::CaptureDevice(MediaKind::Audio)),
        prompt: Some(Prompt::CaptureDevice(MediaKind::Audio)),
        settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
    },
    Permission {
        id: PermissionId::Camera,
        judged_by: JudgedBy::Api(Probe::CaptureDevice(MediaKind::Video)),
        prompt: Some(Prompt::CaptureDevice(MediaKind::Video)),
        settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera",
    },
    Permission {
        id: PermissionId::Screen,
        judged_by: JudgedBy::Api(Probe::ScreenCapture),
        prompt: None,
        settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    },
    Permission {
        id: PermissionId::Accessibility,
        judged_by: JudgedBy::Api(Probe::Accessibility),
        prompt: None,
        settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    },
    Permission {
        id: PermissionId::FullDiskAccess,
        judged_by: JudgedBy::Api(Probe::FullDiskAccess),
        prompt: None,
        settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
    },
    Permission {
        id: PermissionId::Automation,
        judged_by: JudgedBy::Api(Probe::Automation),
        prompt: Some(Prompt::Automation),
        settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
    },
    // No query API: macOS keeps the local-network answer to itself, so the
    // card's own connection test is the only evidence this page can show.
    Permission {
        id: PermissionId::LocalNetwork,
        judged_by: JudgedBy::NoApi(PermissionStatus::Unknown),
        prompt: Some(Prompt::LocalNetwork),
        settings_url: "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_LocalNetwork",
    },
    // No query API either, and nothing standing to read: macOS asks per
    // accessory, at the moment one is attached.
    Permission {
        id: PermissionId::Usb,
        judged_by: JudgedBy::NoApi(PermissionStatus::Ready),
        prompt: None,
        settings_url: "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension",
    },
    Permission {
        id: PermissionId::Bluetooth,
        judged_by: JudgedBy::Api(Probe::Bluetooth),
        prompt: None,
        settings_url: "x-apple.systempreferences:com.apple.preference.security?Privacy_Bluetooth",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PermissionState {
    id: PermissionId,
    status: PermissionStatus,
    action: PermissionAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequestResult {
    id: PermissionId,
    status: PermissionStatus,
    opened_system_settings: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalNetworkTestResult {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    tested_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<&'static str>,
}

impl LocalNetworkTestResult {
    fn success(host: String, port: u16) -> Self {
        Self {
            ok: true,
            host: Some(host),
            port: Some(port),
            tested_at: now_in_millis(),
            failure: None,
        }
    }

    /// A refusal is evidence too: the row shows *when* the last test ran
    /// whichever way it went, so a stale success cannot pass for a fresh one.
    fn failed(failure: &'static str) -> Self {
        Self {
            ok: false,
            host: None,
            port: None,
            tested_at: now_in_millis(),
            failure: Some(failure),
        }
    }
}

fn now_in_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Read every permission once. TCC answers over XPC, so callers hold this off
/// the async worker — see `cmd::remote::developer_permission_statuses`.
pub fn statuses() -> Vec<PermissionState> {
    PERMISSIONS
        .iter()
        .map(|row| {
            let status = status(row);
            PermissionState {
                id: row.id,
                status,
                action: action(row, status),
            }
        })
        .collect()
}

fn permission(id: PermissionId) -> &'static Permission {
    PERMISSIONS
        .iter()
        .find(|row| row.id == id)
        .expect("PERMISSIONS carries one row per PermissionId")
}

/// Which button a row offers.
///
/// macOS raises each of its dialogs once per process identity. Once the
/// answer is in — allowed or denied — a trigger button would do nothing at
/// all, so the row points at the pane where the answer can still be changed.
const fn action(row: &Permission, status: PermissionStatus) -> PermissionAction {
    match row.prompt {
        Some(_) if !matches!(status, PermissionStatus::Granted | PermissionStatus::Denied) => {
            PermissionAction::TriggerPrompt
        }
        _ => PermissionAction::OpenSettings,
    }
}

fn status(row: &Permission) -> PermissionStatus {
    match row.judged_by {
        JudgedBy::Api(probe) => probe.read(),
        JudgedBy::NoApi(standing) => without_api(standing),
    }
}

/// Off macOS there is no privacy database to stand in front of, so the rows
/// macOS cannot answer say what every other row says: unsupported.
#[cfg(not(target_os = "macos"))]
const fn without_api(_standing: PermissionStatus) -> PermissionStatus {
    PermissionStatus::Unsupported
}

#[cfg(target_os = "macos")]
const fn without_api(standing: PermissionStatus) -> PermissionStatus {
    standing
}

impl Probe {
    #[cfg(not(target_os = "macos"))]
    const fn read(self) -> PermissionStatus {
        PermissionStatus::Unsupported
    }

    #[cfg(target_os = "macos")]
    fn read(self) -> PermissionStatus {
        match self {
            Self::CaptureDevice(kind) => capture_device_status(kind),
            Self::ScreenCapture => screen_capture_status(),
            Self::Accessibility => accessibility_status(),
            Self::FullDiskAccess => full_disk_access_status(),
            Self::Automation => automation_status(),
            Self::Bluetooth => bluetooth_status(),
        }
    }
}

/// `AVAuthorizationStatus` and `CBManagerAuthorization` are the same four
/// values under two spellings; one mapping keeps them from drifting apart.
///
/// `notDetermined` is the only one that can still be asked, so it is the page's
/// `Ready`. `restricted` joins `denied`: whatever put the answer there, this
/// process cannot use the device.
#[cfg(target_os = "macos")]
const fn tcc_authorization(raw: isize) -> PermissionStatus {
    match raw {
        0 => PermissionStatus::Ready,
        1 | 2 => PermissionStatus::Denied,
        3 => PermissionStatus::Granted,
        _ => PermissionStatus::Unknown,
    }
}

/// `AEDeterminePermissionToAutomateTarget`'s `OSStatus`.
///
/// `procNotFound` (−600) means the target is not running — nothing was asked,
/// so nothing is known; the row's hint says so.
#[cfg(target_os = "macos")]
const fn automation_permission(raw: i32) -> PermissionStatus {
    match raw {
        0 => PermissionStatus::Granted,
        -1743 => PermissionStatus::Denied,
        -1744 => PermissionStatus::Ready,
        _ => PermissionStatus::Unknown,
    }
}

#[cfg(target_os = "macos")]
fn capture_device_class() -> Option<&'static objc2::runtime::AnyClass> {
    objc2::runtime::AnyClass::get(c"AVCaptureDevice")
}

#[cfg(target_os = "macos")]
fn media_type(kind: MediaKind) -> &'static objc2_foundation::NSString {
    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        #[link_name = "AVMediaTypeAudio"]
        static AUDIO: &'static objc2_foundation::NSString;
        #[link_name = "AVMediaTypeVideo"]
        static VIDEO: &'static objc2_foundation::NSString;
    }

    // SAFETY: both are immortal AVFoundation string constants, initialized
    // before any code of ours can run.
    unsafe {
        match kind {
            MediaKind::Audio => AUDIO,
            MediaKind::Video => VIDEO,
        }
    }
}

#[cfg(target_os = "macos")]
fn capture_device_status(kind: MediaKind) -> PermissionStatus {
    let Some(class) = capture_device_class() else {
        return PermissionStatus::Unknown;
    };
    let media = media_type(kind);
    // SAFETY: a class method taking one AVMediaType and returning an
    // NSInteger. It reads the standing TCC answer and raises no dialog.
    let raw: isize = unsafe { objc2::msg_send![class, authorizationStatusForMediaType: media] };
    tcc_authorization(raw)
}

#[cfg(target_os = "macos")]
fn screen_capture_status() -> PermissionStatus {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }

    // SAFETY: this parameter-free CoreGraphics query is documented as a
    // process-wide preflight and returns no borrowed state.
    //
    // False is Denied, not Unknown: whether the answer was never asked for or
    // was refused, a tool this window launches cannot read the screen today.
    if unsafe { CGPreflightScreenCaptureAccess() } {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    }
}

#[cfg(target_os = "macos")]
fn accessibility_status() -> PermissionStatus {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    // SAFETY: AXIsProcessTrusted is a parameter-free process status query.
    //
    // False is Denied for the same reason as the screen: untrusted is
    // untrusted, and the tools cannot drive another app's UI either way.
    if unsafe { AXIsProcessTrusted() } {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    }
}

#[cfg(target_os = "macos")]
fn full_disk_access_status() -> PermissionStatus {
    let Some(home) = dirs::home_dir() else {
        return PermissionStatus::Unknown;
    };
    let database = home.join("Library/Application Support/com.apple.TCC/TCC.db");
    match std::fs::File::open(database) {
        Ok(_) => PermissionStatus::Granted,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            PermissionStatus::Denied
        }
        Err(_) => PermissionStatus::Unknown,
    }
}

/// The Apple Event descriptor types and the one target this window's
/// automation road actually drives.
#[cfg(target_os = "macos")]
mod apple_events {
    pub(super) const TYPE_APPLICATION_BUNDLE_ID: u32 = u32::from_be_bytes(*b"bund");
    pub(super) const TYPE_WILDCARD: u32 = u32::from_be_bytes(*b"****");
    pub(super) const SYSTEM_EVENTS: &[u8] = b"com.apple.systemevents";

    /// `AEDesc` — an opaque descriptor handle behind its four-character type.
    #[repr(C)]
    pub(super) struct AEDesc {
        pub(super) descriptor_type: u32,
        pub(super) data_handle: *mut std::ffi::c_void,
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        pub(super) fn AECreateDesc(
            type_code: u32,
            data: *const std::ffi::c_void,
            size: isize,
            result: *mut AEDesc,
        ) -> i16;
        pub(super) fn AEDisposeDesc(descriptor: *mut AEDesc) -> i16;
        pub(super) fn AEDeterminePermissionToAutomateTarget(
            target: *const AEDesc,
            event_class: u32,
            event_id: u32,
            ask_user_if_needed: u8,
        ) -> i32;
    }
}

#[cfg(target_os = "macos")]
fn automation_status() -> PermissionStatus {
    use apple_events::{
        AECreateDesc, AEDesc, AEDeterminePermissionToAutomateTarget, AEDisposeDesc, SYSTEM_EVENTS,
        TYPE_APPLICATION_BUNDLE_ID, TYPE_WILDCARD,
    };

    let Ok(length) = isize::try_from(SYSTEM_EVENTS.len()) else {
        return PermissionStatus::Unknown;
    };
    let mut target = AEDesc {
        descriptor_type: 0,
        data_handle: std::ptr::null_mut(),
    };
    // SAFETY: AECreateDesc copies `SYSTEM_EVENTS` into a descriptor it
    // allocates and writes through `target`, which is ours and live.
    let created = unsafe {
        AECreateDesc(
            TYPE_APPLICATION_BUNDLE_ID,
            SYSTEM_EVENTS.as_ptr().cast(),
            length,
            &raw mut target,
        )
    };
    if created != 0 {
        return PermissionStatus::Unknown;
    }
    // SAFETY: `target` is the descriptor just filled. Asking with
    // `ask_user_if_needed: 0` reads the standing answer and raises no dialog.
    let answer = unsafe {
        AEDeterminePermissionToAutomateTarget(&raw const target, TYPE_WILDCARD, TYPE_WILDCARD, 0)
    };
    // SAFETY: disposing the descriptor this function created, exactly once.
    unsafe { AEDisposeDesc(&raw mut target) };
    automation_permission(answer)
}

#[cfg(target_os = "macos")]
fn bluetooth_status() -> PermissionStatus {
    // CoreBluetooth is loaded for the class below and nothing else; the empty
    // block is what puts `-framework CoreBluetooth` on the link line.
    #[link(name = "CoreBluetooth", kind = "framework")]
    unsafe extern "C" {}

    let Some(class) = objc2::runtime::AnyClass::get(c"CBCentralManager") else {
        return PermissionStatus::Unknown;
    };
    // SAFETY: `authorization` is a class property (macOS 10.15+) returning an
    // NSInteger. Reading it instantiates no central manager, so it cannot
    // raise the scan prompt that creating one would.
    let raw: isize = unsafe { objc2::msg_send![class, authorization] };
    tcc_authorization(raw)
}

/// How long the window waits on a dialog the person owns.
///
/// The OS dialog belongs to them, not to us, so the wait is generous; a lost
/// callback must still not hold a blocking worker forever.
#[cfg(target_os = "macos")]
const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

#[cfg(target_os = "macos")]
impl Prompt {
    /// Raise the dialog; `false` means this build cannot, and the caller
    /// should fall back to the row's System Settings pane.
    fn raise(self) -> io::Result<bool> {
        match self {
            Self::CaptureDevice(kind) => Ok(request_capture_device(kind)),
            Self::Automation => trigger_automation_prompt().map(|()| true),
            Self::LocalNetwork => trigger_local_network_prompt().map(|()| true),
        }
    }
}

/// Whether this build carries the usage description a capture dialog needs.
///
/// A `cargo run` window has no bundle at all, and a bundle that lost the
/// merge would have no key either. Either way macOS would kill us for asking,
/// so the row quietly falls back to its System Settings pane instead.
#[cfg(target_os = "macos")]
fn can_ask_for(kind: MediaKind) -> bool {
    use objc2::runtime::{AnyClass, AnyObject};

    let Some(class) = AnyClass::get(c"NSBundle") else {
        return false;
    };
    // SAFETY: `mainBundle` is a class method returning an autoreleased
    // NSBundle, and `objectForInfoDictionaryKey:` reads one key out of the
    // already-parsed Info.plist. Both are nil-returning, never throwing.
    unsafe {
        let bundle: *mut AnyObject = objc2::msg_send![class, mainBundle];
        if bundle.is_null() {
            return false;
        }
        let key = objc2_foundation::NSString::from_str(kind.usage_description_key());
        let value: *mut AnyObject = objc2::msg_send![bundle, objectForInfoDictionaryKey: &*key];
        !value.is_null()
    }
}

#[cfg(target_os = "macos")]
fn request_capture_device(kind: MediaKind) -> bool {
    if !can_ask_for(kind) {
        return false;
    }
    let Some(class) = capture_device_class() else {
        return false;
    };
    let media = media_type(kind);
    let (answered, answer) = std::sync::mpsc::sync_channel::<()>(1);
    let handler = block2::RcBlock::new(move |_granted: objc2::runtime::Bool| {
        let _ = answered.try_send(());
    });
    // SAFETY: the class method takes an AVMediaType and a `void(^)(BOOL)`
    // block, which AVFoundation copies before returning. `handler` outlives
    // the call and the framework owns its copy afterwards.
    unsafe {
        let _: () = objc2::msg_send![
            class,
            requestAccessForMediaType: media,
            completionHandler: &*handler,
        ];
    }
    let _ = answer.recv_timeout(PROMPT_TIMEOUT);
    true
}

#[cfg(target_os = "macos")]
fn trigger_automation_prompt() -> io::Result<()> {
    use std::process::Stdio;
    use std::thread;
    use std::time::Instant;

    let mut child = crate::proc::quiet_command("osascript")
        .args(["-e", "tell application \"System Events\" to return 1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

#[cfg(target_os = "macos")]
fn trigger_local_network_prompt() -> io::Result<()> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket.send_to(&[0], "224.0.0.251:5353")?;
    Ok(())
}

pub async fn request(id: PermissionId) -> io::Result<PermissionRequestResult> {
    #[cfg(not(target_os = "macos"))]
    {
        Ok(PermissionRequestResult {
            id,
            status: PermissionStatus::Unsupported,
            opened_system_settings: false,
        })
    }

    #[cfg(target_os = "macos")]
    {
        let row = permission(id);
        let raised = match row.prompt {
            Some(prompt) => tokio::task::spawn_blocking(move || prompt.raise())
                .await
                .map_err(io::Error::other)??,
            None => false,
        };
        let opened_system_settings = !raised;
        if opened_system_settings {
            open_settings(id)?;
        }
        // Read the answer the person just gave, off the async worker for the
        // same reason `statuses` is.
        let status = tokio::task::spawn_blocking(move || status(row))
            .await
            .map_err(io::Error::other)?;
        Ok(PermissionRequestResult {
            id,
            status,
            opened_system_settings,
        })
    }
}

#[cfg(not(target_os = "macos"))]
pub fn open_settings(id: PermissionId) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "{} is a macOS System Settings pane",
            permission(id).settings_url
        ),
    ))
}

#[cfg(target_os = "macos")]
pub fn open_settings(id: PermissionId) -> io::Result<()> {
    let mut opener = crate::proc::quiet_command("open");
    opener.arg(permission(id).settings_url);
    zerocode_core::reap::spawn_forgotten(opener).map(|_| ())
}

pub async fn test_local_network(host: String, port: u32) -> LocalNetworkTestResult {
    let host = host.trim();
    let Ok(port) = u16::try_from(port) else {
        return LocalNetworkTestResult::failed("invalid-target");
    };
    if port == 0 || !valid_host(host) {
        return LocalNetworkTestResult::failed("invalid-target");
    }

    let lookup = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port))).await;
    let addresses = match lookup {
        Err(_) => return LocalNetworkTestResult::failed("timeout"),
        Ok(Err(_)) => return LocalNetworkTestResult::failed("unresolved"),
        Ok(Ok(addresses)) => addresses
            .filter(|address| is_local_address(*address))
            .collect::<Vec<_>>(),
    };
    if addresses.is_empty() {
        return LocalNetworkTestResult::failed("unreachable");
    }

    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    let mut refused = false;
    for address in addresses {
        match tokio::time::timeout_at(deadline, tokio::net::TcpStream::connect(address)).await {
            Ok(Ok(_)) => return LocalNetworkTestResult::success(host.to_string(), port),
            Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => refused = true,
            Ok(Err(_)) => {}
            Err(_) => return LocalNetworkTestResult::failed("timeout"),
        }
    }
    LocalNetworkTestResult::failed(if refused { "refused" } else { "unreachable" })
}

fn valid_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 || host.chars().any(char::is_whitespace) {
        return false;
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    host.is_ascii()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn is_local_address(address: SocketAddr) -> bool {
    match address.ip() {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_source::window_source;

    #[test]
    fn permission_ids_are_closed_and_have_one_state_each() {
        let states = statuses();
        assert_eq!(states.len(), PERMISSIONS.len());
        for row in PERMISSIONS {
            assert_eq!(
                states.iter().filter(|state| state.id == row.id).count(),
                1,
                "{:?} must have exactly one state",
                row.id
            );
        }
    }

    #[test]
    fn every_permission_is_looked_up_in_the_one_table() {
        for row in PERMISSIONS {
            assert_eq!(permission(row.id), &row);
        }
    }

    #[test]
    fn renderer_copy_and_tauri_commands_follow_the_native_permission_contract() {
        let shell = window_source();
        let definitions = shell
            .split_once("const developerPermissionDefinitions")
            .and_then(|(_, rest)| rest.split_once("const developerPermissionIds"))
            .map(|(definitions, _)| definitions)
            .expect("renderer permission definitions");
        assert_eq!(definitions.matches("\n    id: ").count(), PERMISSIONS.len());
        for row in PERMISSIONS {
            let wire_id = serde_json::to_string(&row.id).expect("serialized permission id");
            assert_eq!(
                definitions.matches(&format!("id: {wire_id}")).count(),
                1,
                "{wire_id} must have one renderer copy owner"
            );
        }

        let main = include_str!("main.rs");
        for command in [
            "developer_permission_statuses",
            "request_developer_permission",
            "open_developer_permission_settings",
            "test_local_network_permission",
        ] {
            assert!(
                shell.contains(&format!("invoke(\"{command}\"")),
                "renderer does not consume {command}"
            );
            assert!(
                main.contains(&format!("            {command},")),
                "Tauri handler does not expose {command}"
            );
        }
    }

    /// The four values `AVAuthorizationStatus` and `CBManagerAuthorization`
    /// share, mapped by the one function both probes call.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_four_tcc_authorization_values_map_to_one_vocabulary() {
        assert_eq!(tcc_authorization(0), PermissionStatus::Ready);
        assert_eq!(tcc_authorization(1), PermissionStatus::Denied);
        assert_eq!(tcc_authorization(2), PermissionStatus::Denied);
        assert_eq!(tcc_authorization(3), PermissionStatus::Granted);
        assert_eq!(tcc_authorization(-1), PermissionStatus::Unknown);
        assert_eq!(tcc_authorization(9), PermissionStatus::Unknown);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_automation_os_statuses_map_to_one_vocabulary() {
        assert_eq!(automation_permission(0), PermissionStatus::Granted);
        assert_eq!(automation_permission(-1743), PermissionStatus::Denied);
        assert_eq!(automation_permission(-1744), PermissionStatus::Ready);
        // procNotFound: System Events is not running, so nothing was asked.
        assert_eq!(automation_permission(-600), PermissionStatus::Unknown);
    }

    /// A prompt row stops offering its trigger once the answer is in, because
    /// macOS will not raise that dialog a second time for this process.
    #[test]
    fn an_answered_prompt_row_offers_the_settings_pane_instead() {
        let microphone = permission(PermissionId::Microphone);
        for answered in [PermissionStatus::Granted, PermissionStatus::Denied] {
            assert_eq!(
                action(microphone, answered),
                PermissionAction::OpenSettings,
                "{answered:?} still offered a dialog macOS will not raise"
            );
        }
        for open in [PermissionStatus::Ready, PermissionStatus::Unknown] {
            assert_eq!(action(microphone, open), PermissionAction::TriggerPrompt);
        }
        let screen = permission(PermissionId::Screen);
        assert_eq!(
            action(screen, PermissionStatus::Ready),
            PermissionAction::OpenSettings,
            "a row with no prompt of its own must not grow one"
        );
    }

    /// The frameworks this file names must actually reach the link line —
    /// an unlinked framework leaves its class unregistered and every row it
    /// judges silently Unknown.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_frameworks_these_probes_call_are_linked_into_this_binary() {
        for class in [c"AVCaptureDevice", c"CBCentralManager"] {
            assert!(
                objc2::runtime::AnyClass::get(class).is_some(),
                "{class:?} is not registered: its framework is not linked"
            );
        }
    }

    /// Live, on whatever identity the test binary carries: every probe has to
    /// answer, and none of them may raise a dialog. The values themselves
    /// belong to the window's own TCC identity, not this binary's, so they
    /// are deliberately not asserted here.
    #[cfg(target_os = "macos")]
    #[test]
    fn every_native_probe_answers_without_raising_a_prompt() {
        for row in PERMISSIONS {
            let answer = status(&row);
            assert!(
                !matches!(answer, PermissionStatus::Unsupported),
                "{:?} answered Unsupported on macOS",
                row.id
            );
        }
    }

    /// The dialog's licence, checked on a binary that could raise it.
    ///
    /// macOS terminates any process that asks for a capture device without
    /// the matching usage description. `tauri-build` merges
    /// `crates/zerocode-shell/Info.plist` into every binary it builds — this
    /// test's own included — so `can_ask_for` answering yes here *is* that
    /// merge, checked. Lose the file and this goes red before a person ever
    /// presses the button; lose it in the bundler and the row quietly falls
    /// back to its System Settings pane instead of killing the window.
    ///
    /// The request itself is never made from a test: this identity has not
    /// been asked yet, so the dialog would really open and really wait.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_capture_dialog_carries_the_usage_description_macos_demands() {
        for kind in [MediaKind::Audio, MediaKind::Video] {
            assert!(
                can_ask_for(kind),
                "{} did not reach this binary's Info.plist",
                kind.usage_description_key()
            );
        }
    }

    #[test]
    fn local_network_targets_are_syntactically_bounded() {
        assert!(valid_host("192.168.1.20"));
        assert!(valid_host("build-box.local"));
        assert!(valid_host("::1"));
        assert!(!valid_host("https://example.com"));
        assert!(!valid_host("bad host"));
        assert!(!valid_host("-bad.local"));
    }

    #[test]
    fn only_loopback_private_and_link_local_addresses_are_connectable() {
        for address in ["127.0.0.1:80", "10.0.0.3:80", "169.254.1.2:80", "[::1]:80"] {
            assert!(is_local_address(address.parse().unwrap()), "{address}");
        }
        for address in ["8.8.8.8:53", "1.1.1.1:443", "[2606:4700:4700::1111]:53"] {
            assert!(!is_local_address(address.parse().unwrap()), "{address}");
        }
    }

    #[tokio::test]
    async fn a_local_connection_test_reaches_only_the_bound_private_endpoint() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = tokio::spawn(async move { listener.accept().await.unwrap() });

        let result = test_local_network("127.0.0.1".into(), u32::from(port)).await;
        assert!(result.ok);
        assert_eq!(result.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(result.port, Some(port));
        assert!(result.tested_at > 0);
        accepted.await.unwrap();
    }

    #[tokio::test]
    async fn a_refused_test_is_stamped_the_same_way_a_successful_one_is() {
        let result = test_local_network("8.8.8.8".into(), 53).await;
        assert!(!result.ok);
        assert_eq!(result.failure, Some("unreachable"));
        assert!(
            result.tested_at > 0,
            "a failed test must still say when it ran"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn every_permission_opens_only_its_fixed_system_settings_pane() {
        for row in PERMISSIONS {
            assert!(row.settings_url.starts_with("x-apple.systempreferences:"));
            assert!(!row.settings_url.contains(['\n', '\r', '\0']));
        }
    }
}
