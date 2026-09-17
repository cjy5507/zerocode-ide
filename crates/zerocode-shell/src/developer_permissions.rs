//! macOS privacy controls for terminal-launched developer tools.
//!
//! The renderer receives only a closed permission identifier and a coarse
//! status. System Settings URLs, probes, and network validation stay in this
//! native boundary so the webview cannot turn this surface into an arbitrary
//! process launcher or socket client.

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

const PERMISSIONS: [PermissionId; 9] = [
    PermissionId::Microphone,
    PermissionId::Camera,
    PermissionId::Screen,
    PermissionId::Accessibility,
    PermissionId::FullDiskAccess,
    PermissionId::Automation,
    PermissionId::LocalNetwork,
    PermissionId::Usb,
    PermissionId::Bluetooth,
];

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
    #[serde(skip_serializing_if = "Option::is_none")]
    tested_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<&'static str>,
}

impl LocalNetworkTestResult {
    fn success(host: String, port: u16) -> Self {
        Self {
            ok: true,
            host: Some(host),
            port: Some(port),
            tested_at: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
            ),
            failure: None,
        }
    }

    const fn failed(failure: &'static str) -> Self {
        Self {
            ok: false,
            host: None,
            port: None,
            tested_at: None,
            failure: Some(failure),
        }
    }
}

pub fn statuses() -> Vec<PermissionState> {
    PERMISSIONS
        .into_iter()
        .map(|id| PermissionState {
            id,
            status: status(id),
            action: action(id),
        })
        .collect()
}

const fn action(id: PermissionId) -> PermissionAction {
    match id {
        PermissionId::Automation | PermissionId::LocalNetwork => PermissionAction::TriggerPrompt,
        PermissionId::Microphone
        | PermissionId::Camera
        | PermissionId::Screen
        | PermissionId::Accessibility
        | PermissionId::FullDiskAccess
        | PermissionId::Usb
        | PermissionId::Bluetooth => PermissionAction::OpenSettings,
    }
}

#[cfg(not(target_os = "macos"))]
const fn status(_id: PermissionId) -> PermissionStatus {
    PermissionStatus::Unsupported
}

#[cfg(target_os = "macos")]
fn status(id: PermissionId) -> PermissionStatus {
    match id {
        PermissionId::Screen => screen_capture_status(),
        PermissionId::Accessibility => accessibility_status(),
        PermissionId::FullDiskAccess => full_disk_access_status(),
        PermissionId::Usb | PermissionId::Bluetooth => PermissionStatus::Ready,
        // macOS does not expose stable synchronous status APIs for these
        // permissions at this layer. Unknown is deliberate; the settings
        // action still opens or triggers the canonical OS prompt.
        PermissionId::Microphone
        | PermissionId::Camera
        | PermissionId::Automation
        | PermissionId::LocalNetwork => PermissionStatus::Unknown,
    }
}

#[cfg(target_os = "macos")]
fn screen_capture_status() -> PermissionStatus {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }

    // SAFETY: this parameter-free CoreGraphics query is documented as a
    // process-wide preflight and returns no borrowed state.
    if unsafe { CGPreflightScreenCaptureAccess() } {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Unknown
    }
}

#[cfg(target_os = "macos")]
fn accessibility_status() -> PermissionStatus {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    // SAFETY: AXIsProcessTrusted is a parameter-free process status query.
    if unsafe { AXIsProcessTrusted() } {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Unknown
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
        match id {
            PermissionId::Automation => {
                tokio::task::spawn_blocking(trigger_automation_prompt)
                    .await
                    .map_err(io::Error::other)??;
                Ok(PermissionRequestResult {
                    id,
                    status: status(id),
                    opened_system_settings: false,
                })
            }
            PermissionId::LocalNetwork => {
                tokio::task::spawn_blocking(trigger_local_network_prompt)
                    .await
                    .map_err(io::Error::other)??;
                Ok(PermissionRequestResult {
                    id,
                    status: status(id),
                    opened_system_settings: false,
                })
            }
            _ => {
                open_settings(id)?;
                Ok(PermissionRequestResult {
                    id,
                    status: status(id),
                    opened_system_settings: true,
                })
            }
        }
    }
}

pub fn open_settings(id: PermissionId) -> io::Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = id;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "developer permissions are available on macOS only",
        ))
    }

    #[cfg(target_os = "macos")]
    {
        let mut opener = crate::proc::quiet_command("open");
        opener.arg(settings_url(id));
        zerocode_core::reap::spawn_forgotten(opener).map(|_| ())
    }
}

#[cfg(target_os = "macos")]
const fn settings_url(id: PermissionId) -> &'static str {
    match id {
        PermissionId::Camera => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
        }
        PermissionId::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        PermissionId::Screen => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        PermissionId::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        PermissionId::FullDiskAccess => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
        }
        PermissionId::Automation => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation"
        }
        PermissionId::LocalNetwork => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_LocalNetwork"
        }
        PermissionId::Bluetooth => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Bluetooth"
        }
        PermissionId::Usb => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension"
        }
    }
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
        for id in PERMISSIONS {
            assert_eq!(states.iter().filter(|state| state.id == id).count(), 1);
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
        for id in PERMISSIONS {
            let wire_id = serde_json::to_string(&id).expect("serialized permission id");
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
        accepted.await.unwrap();
    }

    #[tokio::test]
    async fn a_public_address_is_rejected_before_any_connection() {
        let result = test_local_network("8.8.8.8".into(), 53).await;
        assert!(!result.ok);
        assert_eq!(result.failure, Some("unreachable"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn every_permission_opens_only_its_fixed_system_settings_pane() {
        for id in PERMISSIONS {
            let url = settings_url(id);
            assert!(url.starts_with("x-apple.systempreferences:"));
            assert!(!url.contains(['\n', '\r', '\0']));
        }
    }
}
