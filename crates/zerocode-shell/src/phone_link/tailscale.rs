//! The one road a phone takes in: Tailscale Serve, inside the tailnet.
//!
//! The window never opens a port beyond 127.0.0.1. When a person turns the
//! phone link on, this asks the Tailscale CLI to put ONE HTTPS port of this
//! machine's tailnet name in front of the loopback listener — and turning it
//! off removes exactly that mapping. Any other Serve mapping the person made
//! is read, never written.
//!
//! Before it maps, and every time the settings pane looks, it refuses two
//! states: Funnel on the phone port (that is the public internet, not the
//! tailnet), and the agent hook bridge's port anywhere in the Serve or Funnel
//! configuration — `zerocode-lane` trusts every loopback caller of that port,
//! and a Serve mapping would make the tailnet a loopback caller.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// The tailnet HTTPS ports the window will use, in order. Not 443: a person's
/// own Serve mappings live there first, and this window never takes a port
/// somebody else holds.
pub(crate) const HTTPS_PORTS: [u16; 3] = [8443, 10443, 9443];
const CLI_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the CLI lives when it is not on PATH. A Finder-launched app inherits
/// launchd's PATH, and the Mac app ships its CLI inside its bundle.
#[cfg(target_os = "macos")]
const BUNDLED_CLI: &[&str] = &["/Applications/Tailscale.app/Contents/MacOS/Tailscale"];
#[cfg(windows)]
const BUNDLED_CLI: &[&str] = &[];
#[cfg(not(any(target_os = "macos", windows)))]
const BUNDLED_CLI: &[&str] = &["/usr/bin/tailscale", "/usr/sbin/tailscale"];

#[cfg(windows)]
const CLI_NAME: &str = "tailscale.exe";
#[cfg(not(windows))]
const CLI_NAME: &str = "tailscale";

/// Find the CLI: the platform's install place first, then PATH.
pub(crate) fn find_cli() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = BUNDLED_CLI.iter().map(PathBuf::from).collect();
    #[cfg(windows)]
    for variable in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(base).join("Tailscale").join(CLI_NAME));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join(CLI_NAME)));
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

/// What `tailscale status --json` says, reduced to what the pane needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tailnet {
    pub(crate) installed: bool,
    /// `BackendState == "Running"`.
    pub(crate) running: bool,
    /// This machine's MagicDNS name without the trailing dot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dns_name: Option<String>,
    /// Whether the tailnet can issue this machine an HTTPS certificate.
    pub(crate) https_ready: bool,
    /// The login that owns this machine — the only person who may pair.
    #[serde(skip)]
    pub(crate) owner_login: Option<String>,
}

pub(crate) fn parse_status(json: &Value) -> Tailnet {
    let running = json.get("BackendState").and_then(Value::as_str) == Some("Running");
    let own = json.get("Self");
    let dns_name = own
        .and_then(|own| own.get("DNSName"))
        .and_then(Value::as_str)
        .map(|name| name.trim_end_matches('.').to_ascii_lowercase())
        .filter(|name| !name.is_empty());
    let https_ready = dns_name.as_ref().is_some_and(|name| {
        json.get("CertDomains")
            .and_then(Value::as_array)
            .is_some_and(|domains| {
                domains
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|domain| domain.trim_end_matches('.').eq_ignore_ascii_case(name))
            })
    });
    let owner_login = own
        .and_then(|own| own.get("UserID"))
        .map(|id| match id {
            Value::Number(number) => number.to_string(),
            Value::String(text) => text.clone(),
            _ => String::new(),
        })
        .and_then(|id| {
            json.get("User")
                .and_then(|users| users.get(&id))
                .and_then(|user| user.get("LoginName"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    Tailnet {
        installed: true,
        running,
        dns_name,
        https_ready,
        owner_login,
    }
}

/// One Serve web handler: `https://<host>:<port><path>` → `proxy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Handler {
    pub(crate) host_port: String,
    pub(crate) path: String,
    pub(crate) proxy: Option<String>,
}

/// `tailscale serve status --json`, flattened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ServeConfig {
    pub(crate) handlers: Vec<Handler>,
    /// TCP ports Serve listens on, with the forward target when it is a raw
    /// TCP forward.
    pub(crate) tcp: Vec<(u16, Option<String>)>,
    /// `host:port` keys Funnel is allowed on.
    pub(crate) funnel: Vec<String>,
}

pub(crate) fn parse_serve(json: &Value) -> ServeConfig {
    let mut config = ServeConfig::default();
    if let Some(web) = json.get("Web").and_then(Value::as_object) {
        for (host_port, site) in web {
            if let Some(handlers) = site.get("Handlers").and_then(Value::as_object) {
                for (path, handler) in handlers {
                    config.handlers.push(Handler {
                        host_port: host_port.to_ascii_lowercase(),
                        path: path.clone(),
                        proxy: handler
                            .get("Proxy")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    });
                }
            }
        }
    }
    if let Some(tcp) = json.get("TCP").and_then(Value::as_object) {
        for (port, entry) in tcp {
            if let Ok(port) = port.parse::<u16>() {
                let forward = entry
                    .get("TCPForward")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                config.tcp.push((port, forward));
            }
        }
    }
    if let Some(funnel) = json.get("AllowFunnel").and_then(Value::as_object) {
        for (host_port, on) in funnel {
            if on.as_bool() == Some(true) {
                config.funnel.push(host_port.to_ascii_lowercase());
            }
        }
    }
    config
}

/// Whether a proxy or forward target names this loopback port.
fn targets_port(target: &str, port: u16) -> bool {
    let rest = target
        .split_once("://")
        .map_or(target, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some((host, named)) = rest.rsplit_once(':') else {
        // `tailscale serve 3000` stores a full URL; a bare number is a port.
        return rest.parse::<u16>().ok() == Some(port);
    };
    let loopback = matches!(
        host,
        "127.0.0.1" | "localhost" | "[::1]" | "::1" | "0.0.0.0"
    );
    loopback && named.parse::<u16>().ok() == Some(port)
}

/// The proxy URL the window's own mapping uses.
pub(crate) fn our_proxy(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Exposure {
    /// No mapping on the phone port.
    Absent,
    /// The phone port maps to this window's listener, tailnet only.
    Ours,
    /// Someone else's mapping holds the phone port.
    Taken,
    /// Funnel is on for the phone port — public internet. Refused.
    Funnel,
    /// The hook bridge's port is reachable through Serve or Funnel. Refused.
    HookExposed,
}

/// Judge one Serve configuration for one phone port.
pub(crate) fn judge(
    config: &ServeConfig,
    dns_name: &str,
    https_port: u16,
    listener_port: Option<u16>,
    hook_port: Option<u16>,
) -> Exposure {
    if let Some(hook) = hook_port {
        let through_web = config
            .handlers
            .iter()
            .filter_map(|handler| handler.proxy.as_deref())
            .any(|proxy| targets_port(proxy, hook));
        let through_tcp = config
            .tcp
            .iter()
            .filter_map(|(_, forward)| forward.as_deref())
            .any(|forward| targets_port(forward, hook));
        if through_web || through_tcp {
            return Exposure::HookExposed;
        }
    }
    let key = format!("{}:{https_port}", dns_name.to_ascii_lowercase());
    if config.funnel.contains(&key) {
        return Exposure::Funnel;
    }
    let on_port: Vec<&Handler> = config
        .handlers
        .iter()
        .filter(|handler| handler.host_port == key)
        .collect();
    let raw_tcp = config
        .tcp
        .iter()
        .any(|(port, forward)| *port == https_port && forward.is_some());
    if on_port.is_empty() && !raw_tcp {
        return Exposure::Absent;
    }
    let ours = listener_port.is_some_and(|listener| {
        !raw_tcp
            && on_port.len() == 1
            && on_port[0].path == "/"
            && on_port[0]
                .proxy
                .as_deref()
                .is_some_and(|proxy| targets_port(proxy, listener))
    });
    if ours {
        Exposure::Ours
    } else {
        Exposure::Taken
    }
}

/// The first phone port nobody else holds — or the one already ours.
pub(crate) fn pick_port(
    config: &ServeConfig,
    dns_name: &str,
    listener_port: u16,
    remembered: Option<u16>,
) -> Option<u16> {
    let ordered = remembered
        .into_iter()
        .chain(HTTPS_PORTS)
        .collect::<Vec<u16>>();
    ordered.into_iter().find(|port| {
        matches!(
            judge(config, dns_name, *port, Some(listener_port), None),
            Exposure::Absent | Exposure::Ours
        )
    })
}

/// The argv that maps `https_port` to the loopback listener, in the
/// background, tailnet only.
pub(crate) fn serve_on_args(https_port: u16, listener_port: u16) -> Vec<String> {
    vec![
        "serve".into(),
        "--bg".into(),
        "--yes".into(),
        format!("--https={https_port}"),
        our_proxy(listener_port),
    ]
}

/// The argv that removes that one mapping and nothing else.
pub(crate) fn serve_off_args(https_port: u16) -> Vec<String> {
    vec![
        "serve".into(),
        format!("--https={https_port}"),
        "off".into(),
    ]
}

/// Run the CLI with a time limit; stdout on a zero exit, the CLI's own
/// words otherwise. Both pipes are read while the child runs: `status
/// --json` on a large tailnet is more than a pipe holds, and a child whose
/// pipe is full never exits.
pub(crate) fn run(cli: &Path, args: &[String]) -> Result<String, String> {
    let mut child = crate::proc::quiet_command(cli)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("tailscale could not start: {error}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
    );
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < CLI_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("tailscale did not answer in time".into());
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        let said = String::from_utf8_lossy(&stderr);
        let said = said
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        Err(format!("tailscale: {}", said.trim()))
    }
}

pub(crate) fn read_status(cli: &Path) -> Tailnet {
    run(cli, &["status".into(), "--json".into()])
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map_or(
            Tailnet {
                installed: true,
                ..Tailnet::default()
            },
            |json| parse_status(&json),
        )
}

pub(crate) fn read_serve(cli: &Path) -> Result<ServeConfig, String> {
    let text = run(cli, &["serve".into(), "status".into(), "--json".into()])?;
    if text.trim().is_empty() {
        return Ok(ServeConfig::default());
    }
    serde_json::from_str::<Value>(&text)
        .map(|json| parse_serve(&json))
        .map_err(|error| format!("tailscale serve status: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HOST: &str = "host.example.test";

    fn config(value: Value) -> ServeConfig {
        parse_serve(&value)
    }

    #[test]
    fn status_reads_running_name_certificate_and_owner() {
        let status = parse_status(&json!({
            "BackendState": "Running",
            "CertDomains": ["host.example.test"],
            "Self": { "DNSName": "Host.Example.Test.", "UserID": 42 },
            "User": { "42": { "LoginName": "owner@example.test" } }
        }));
        assert!(status.running);
        assert_eq!(status.dns_name.as_deref(), Some(HOST));
        assert!(status.https_ready);
        assert_eq!(status.owner_login.as_deref(), Some("owner@example.test"));
        let serialized = serde_json::to_string(&status).expect("json");
        assert!(
            !serialized.contains("owner@"),
            "the login never leaves for the pane"
        );
    }

    #[test]
    fn a_stopped_tailnet_or_one_without_certificates_is_not_ready() {
        let stopped = parse_status(&json!({ "BackendState": "Stopped", "Self": {} }));
        assert!(!stopped.running);
        assert!(!stopped.https_ready);
        let no_cert = parse_status(&json!({
            "BackendState": "Running",
            "Self": { "DNSName": "host.example.test." }
        }));
        assert!(no_cert.running);
        assert!(!no_cert.https_ready);
    }

    #[test]
    fn someone_elses_mapping_on_443_leaves_8443_free() {
        let serve = config(json!({
            "TCP": { "443": { "HTTPS": true } },
            "Web": { "host.example.test:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:8788" } } } }
        }));
        assert_eq!(judge(&serve, HOST, 443, Some(5000), None), Exposure::Taken);
        assert_eq!(
            judge(&serve, HOST, 8443, Some(5000), None),
            Exposure::Absent
        );
        assert_eq!(pick_port(&serve, HOST, 5000, None), Some(8443));
    }

    #[test]
    fn our_own_mapping_is_recognised_and_kept() {
        let serve = config(json!({
            "TCP": { "8443": { "HTTPS": true } },
            "Web": { "host.example.test:8443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5000" } } } }
        }));
        assert_eq!(judge(&serve, HOST, 8443, Some(5000), None), Exposure::Ours);
        assert_eq!(judge(&serve, HOST, 8443, Some(5001), None), Exposure::Taken);
        assert_eq!(pick_port(&serve, HOST, 5000, Some(8443)), Some(8443));
        assert_eq!(pick_port(&serve, HOST, 5001, None), Some(10443));
    }

    #[test]
    fn funnel_on_the_phone_port_is_refused() {
        let serve = config(json!({
            "Web": { "host.example.test:8443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5000" } } } },
            "AllowFunnel": { "host.example.test:8443": true }
        }));
        assert_eq!(
            judge(&serve, HOST, 8443, Some(5000), None),
            Exposure::Funnel
        );
        assert_eq!(pick_port(&serve, HOST, 5000, Some(8443)), Some(10443));
    }

    #[test]
    fn the_hook_bridge_reachable_through_serve_refuses_the_phone_link() {
        let web = config(json!({
            "Web": { "host.example.test:443": { "Handlers": { "/hooks": { "Proxy": "http://localhost:6100" } } } }
        }));
        assert_eq!(
            judge(&web, HOST, 8443, Some(5000), Some(6100)),
            Exposure::HookExposed
        );
        let tcp = config(json!({ "TCP": { "7000": { "TCPForward": "127.0.0.1:6100" } } }));
        assert_eq!(
            judge(&tcp, HOST, 8443, Some(5000), Some(6100)),
            Exposure::HookExposed
        );
        let elsewhere = config(json!({
            "Web": { "host.example.test:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:61000" } } } }
        }));
        assert_eq!(
            judge(&elsewhere, HOST, 8443, Some(5000), Some(6100)),
            Exposure::Absent,
            "a port that merely starts with the hook's digits is not the hook"
        );
    }

    #[test]
    fn a_path_mapping_on_the_phone_port_is_someone_elses() {
        let serve = config(json!({
            "Web": { "host.example.test:8443": { "Handlers": {
                "/": { "Proxy": "http://127.0.0.1:5000" },
                "/other": { "Proxy": "http://127.0.0.1:9000" }
            } } }
        }));
        assert_eq!(judge(&serve, HOST, 8443, Some(5000), None), Exposure::Taken);
    }

    #[test]
    fn on_and_off_name_one_port_and_never_reset() {
        assert_eq!(
            serve_on_args(8443, 5000),
            [
                "serve",
                "--bg",
                "--yes",
                "--https=8443",
                "http://127.0.0.1:5000"
            ]
        );
        assert_eq!(serve_off_args(8443), ["serve", "--https=8443", "off"]);
        assert!(!serve_off_args(8443).iter().any(|arg| arg == "reset"));
    }

    #[test]
    fn an_empty_serve_status_is_an_empty_configuration() {
        assert_eq!(config(json!({})), ServeConfig::default());
    }
}
