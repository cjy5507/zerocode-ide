//! The one road a phone takes in: Tailscale Serve, inside the tailnet.
//!
//! The window never opens a port beyond 127.0.0.1. When a person turns the
//! phone link on, this asks the Tailscale CLI to put ONE HTTPS port of this
//! machine's tailnet name in front of the loopback listener — and turning it
//! off removes exactly that mapping. Any other Serve mapping the person made
//! is read, never written.
//!
//! Before it serves, and every minute while it does, it reads the WHOLE Serve
//! configuration — the background one, every foreground `tailscale serve`
//! session and every Tailscale service — and refuses three states: Funnel on
//! anything that reaches the phone listener (that is the public internet, not
//! the tailnet), anything else that reaches the listener's loopback port, and
//! the agent hook bridge's port anywhere at all — `zerocode-lane` trusts every
//! loopback caller of that port, and a Serve mapping would make the tailnet a
//! loopback caller. The mapping the window's book says it made is the
//! window's to the end: it goes when the link turns off, when it is refused,
//! when the window closes, and when the port it pointed at is somebody
//! else's by the next start ([`decide`]).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// The tailnet HTTPS ports the window will use, in order. Not 443 — a
/// person's own Serve mappings live there — and not 8443 or 10000: with 443
/// those are the three ports Funnel can open to the internet, so a mapping on
/// 10443 or 9443 can never be Funnelled, by anybody, by mistake.
pub(crate) const HTTPS_PORTS: [u16; 2] = [10443, 9443];
/// The phone port an earlier build used. A mapping the book names on it is
/// still the window's to take down; on any other port the book is not
/// believed, so a hand-edited book cannot point the window at a person's own.
const RETIRED_HTTPS_PORTS: [u16; 1] = [8443];
pub(crate) const CLI_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the window's close waits, in all, for the CLI to take its
/// mapping down.
pub(crate) const EXIT_CLI_TIMEOUT: Duration = Duration::from_secs(3);
/// How often a running CLI is asked whether it has finished.
const CLI_POLL: Duration = Duration::from_millis(50);
/// The one loopback address the listener binds and the mapping names.
pub(crate) const LOOPBACK: &str = "127.0.0.1";
/// The terminal the CLI is told it runs in. On macOS the app's own binary is
/// the CLI only when `TERM` is set; without it — a window opened from the
/// Finder or the Dock has none — it tries to start the Tailscale app instead
/// and says "The Tailscale GUI failed to start", or never answers.
#[cfg(target_os = "macos")]
const CLI_TERM: &str = "dumb";

/// The places the Tailscale installers put the CLI. PATH is never searched:
/// opening Settings runs this program with the window's rights, and a PATH
/// folder an agent can write to would let it put its own `tailscale` first.
#[cfg(target_os = "macos")]
const INSTALLED_CLI: &[&str] = &["/Applications/Tailscale.app/Contents/MacOS/Tailscale"];
#[cfg(windows)]
const INSTALLED_CLI: &[&str] = &[];
#[cfg(not(any(target_os = "macos", windows)))]
const INSTALLED_CLI: &[&str] = &["/usr/bin/tailscale", "/usr/sbin/tailscale"];

/// Program Files folders the Windows installer writes `Tailscale\tailscale.exe`
/// under, by the variable that names each.
#[cfg(windows)]
const PROGRAM_FILES: &[&str] = &["ProgramFiles", "ProgramW6432"];

/// Find the CLI in its install place, or nowhere.
pub(crate) fn find_cli() -> Option<PathBuf> {
    #[allow(unused_mut)] // only Windows adds to the fixed list.
    let mut candidates: Vec<PathBuf> = INSTALLED_CLI.iter().map(PathBuf::from).collect();
    #[cfg(windows)]
    for variable in PROGRAM_FILES {
        if let Some(base) = std::env::var_os(variable) {
            candidates.push(PathBuf::from(base).join("Tailscale").join("tailscale.exe"));
        }
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

impl Tailnet {
    /// A CLI is there but its status could not be read.
    pub(crate) fn unread() -> Self {
        Self {
            installed: true,
            ..Self::default()
        }
    }
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

/// Where in the Serve configuration an entry was found. Only the background
/// configuration's entries can be the window's own: it maps with `--bg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Background,
    /// A `tailscale serve` left running in a terminal (`Foreground`).
    Foreground,
    /// A Tailscale service (`Services`), served under its own name.
    Service,
}

/// One Serve web handler: `https://<host>:<port><path>` → `proxy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Handler {
    pub(crate) host_port: String,
    pub(crate) path: String,
    pub(crate) proxy: Option<String>,
    pub(crate) scope: Scope,
}

/// One TCP port Serve listens on, with the forward target when it is a raw
/// TCP forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Forward {
    pub(crate) port: u16,
    pub(crate) target: Option<String>,
    pub(crate) scope: Scope,
}

/// `tailscale serve status --json`, flattened across every scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ServeConfig {
    pub(crate) handlers: Vec<Handler>,
    pub(crate) tcp: Vec<Forward>,
    /// `host:port` keys Funnel is allowed on.
    pub(crate) funnel: Vec<String>,
}

/// What the pane is told when Serve's configuration is not a shape this
/// reads (`ui/shell-remote.js`).
pub(crate) const SERVE_SHAPE: &str = "serve-shape";

/// The fields `ipn.ServeConfig` writes, by where they stand. A Serve
/// configuration with anything else, or with a known field of another
/// shape, is not read as "nothing exposed": a newer Tailscale could have put
/// a road to the listener there, so it is an error and the link stays shut.
const SERVE_FIELDS: &[&str] = &["TCP", "Web", "Services", "AllowFunnel", "Foreground"];
const FOREGROUND_FIELDS: &[&str] = &["TCP", "Web", "AllowFunnel"];
const SERVICE_FIELDS: &[&str] = &["TCP", "Web", "Tun"];
const TCP_FIELDS: &[&str] = &[
    "HTTPS",
    "HTTP",
    "TCPForward",
    "TerminateTLS",
    "ProxyProtocol",
];
const SITE_FIELDS: &[&str] = &["Handlers"];
const HANDLER_FIELDS: &[&str] = &["Path", "Proxy", "Text", "AcceptAppCaps", "Redirect"];

/// The object at `json`, after its fields are checked against `known`; an
/// absent or `null` value is an empty one.
fn fields<'a>(
    json: Option<&'a Value>,
    known: &[&str],
    at: &str,
) -> Result<Option<&'a serde_json::Map<String, Value>>, String> {
    match json {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(object)) => {
            match object.keys().find(|field| !known.contains(&field.as_str())) {
                Some(field) => Err(format!(
                    "tailscale serve status: unknown {at} field {field}"
                )),
                None => Ok(Some(object)),
            }
        }
        Some(_) => Err(format!("tailscale serve status: {at} is not an object")),
    }
}

/// A map field (`host:port → …`, `port → …`): any keys, an object.
fn entries<'a>(
    json: Option<&'a Value>,
    at: &str,
) -> Result<Option<&'a serde_json::Map<String, Value>>, String> {
    match json {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(object)) => Ok(Some(object)),
        Some(_) => Err(format!("tailscale serve status: {at} is not an object")),
    }
}

/// A string field, when it is there.
fn text<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<&'a str>, String> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(format!("tailscale serve status: {field} is not a string")),
    }
}

pub(crate) fn parse_serve(json: &Value) -> Result<ServeConfig, String> {
    let mut config = ServeConfig::default();
    let top = fields(Some(json), SERVE_FIELDS, "top-level")?;
    read_scope(top, Scope::Background, &mut config)?;
    for (field, known, scope) in [
        ("Foreground", FOREGROUND_FIELDS, Scope::Foreground),
        ("Services", SERVICE_FIELDS, Scope::Service),
    ] {
        for entry in entries(top.and_then(|top| top.get(field)), field)?
            .into_iter()
            .flat_map(|entries| entries.values())
        {
            let entry = fields(Some(entry), known, field)?;
            if let Some(tun) = entry.and_then(|entry| entry.get("Tun"))
                && !tun.is_boolean()
            {
                return Err("tailscale serve status: Tun is not a boolean".into());
            }
            read_scope(entry, scope, &mut config)?;
        }
    }
    Ok(config)
}

fn read_scope(
    scope_json: Option<&serde_json::Map<String, Value>>,
    scope: Scope,
    config: &mut ServeConfig,
) -> Result<(), String> {
    let Some(json) = scope_json else {
        return Ok(());
    };
    for (host_port, site) in entries(json.get("Web"), "Web")?.into_iter().flatten() {
        let site = fields(Some(site), SITE_FIELDS, "Web site")?;
        let handlers = entries(site.and_then(|site| site.get("Handlers")), "Handlers")?;
        for (path, handler) in handlers.into_iter().flatten() {
            let Some(handler) = fields(Some(handler), HANDLER_FIELDS, "handler")? else {
                continue;
            };
            config.handlers.push(Handler {
                host_port: host_port.to_ascii_lowercase(),
                path: path.clone(),
                proxy: text(handler, "Proxy")?.map(str::to_string),
                scope,
            });
        }
    }
    for (port, entry) in entries(json.get("TCP"), "TCP")?.into_iter().flatten() {
        let port = port
            .parse::<u16>()
            .map_err(|_| format!("tailscale serve status: TCP port {port}"))?;
        let Some(entry) = fields(Some(entry), TCP_FIELDS, "TCP")? else {
            continue;
        };
        config.tcp.push(Forward {
            port,
            target: text(entry, "TCPForward")?.map(str::to_string),
            scope,
        });
    }
    for (host_port, on) in entries(json.get("AllowFunnel"), "AllowFunnel")?
        .into_iter()
        .flatten()
    {
        match on {
            Value::Bool(true) => config.funnel.push(host_port.to_ascii_lowercase()),
            Value::Bool(false) | Value::Null => {}
            _ => return Err("tailscale serve status: AllowFunnel is not a boolean".into()),
        }
    }
    Ok(())
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
    let loopback = matches!(host, LOOPBACK | "localhost" | "[::1]" | "::1" | "0.0.0.0");
    loopback && named.parse::<u16>().ok() == Some(port)
}

/// The proxy URL the window's own mapping uses.
pub(crate) fn our_proxy(port: u16) -> String {
    format!("http://{LOOPBACK}:{port}")
}

/// Whether an HTTPS port can hold a mapping of the window's at all.
fn window_port(https: u16) -> bool {
    HTTPS_PORTS.contains(&https) || RETIRED_HTTPS_PORTS.contains(&https)
}

fn key(dns_name: &str, https_port: u16) -> String {
    format!("{}:{https_port}", dns_name.to_ascii_lowercase())
}

/// The port of a `host:port` key.
fn port_of_key(host_port: &str) -> Option<u16> {
    host_port.rsplit_once(':')?.1.parse().ok()
}

impl ServeConfig {
    fn handlers_to(&self, port: u16) -> impl Iterator<Item = &Handler> {
        self.handlers.iter().filter(move |handler| {
            handler
                .proxy
                .as_deref()
                .is_some_and(|proxy| targets_port(proxy, port))
        })
    }

    fn forwards_to(&self, port: u16) -> impl Iterator<Item = &Forward> {
        self.tcp.iter().filter(move |forward| {
            forward
                .target
                .as_deref()
                .is_some_and(|target| targets_port(target, port))
        })
    }

    /// Whether anything in any scope sends to this loopback port.
    fn reaches(&self, port: u16) -> bool {
        self.handlers_to(port).next().is_some() || self.forwards_to(port).next().is_some()
    }

    /// The configuration as it stands once the window's own mappings on
    /// these HTTPS ports are gone. Funnel keys stay: whether `serve … off`
    /// also takes Funnel away is the CLI's business, so it is not assumed.
    fn without(&self, dns_name: &str, removed: &[u16]) -> Self {
        let keys: Vec<String> = removed.iter().map(|port| key(dns_name, *port)).collect();
        let gone = |host_port: &str, scope: Scope| {
            scope == Scope::Background && keys.iter().any(|one| one.as_str() == host_port)
        };
        let mut config = self.clone();
        config
            .handlers
            .retain(|handler| !(gone(&handler.host_port, handler.scope) && handler.path == "/"));
        config.tcp.retain(|forward| {
            forward.scope != Scope::Background
                || !removed.contains(&forward.port)
                || config.handlers.iter().any(|handler| {
                    handler.scope == Scope::Background
                        && port_of_key(&handler.host_port) == Some(forward.port)
                })
        });
        config
    }
}

/// Whether the window's own mapping stands: the background `/` handler of
/// `https_port` on this machine's name sends to `listener_port`. Funnelled or
/// not, with other paths beside it or not — what the window made, the window
/// takes down, and `serve --https=<port> off` takes down only that `/`.
pub(crate) fn ours_stands(
    config: &ServeConfig,
    dns_name: &str,
    https_port: u16,
    listener_port: u16,
) -> bool {
    let key = key(dns_name, https_port);
    config.handlers_to(listener_port).any(|handler| {
        handler.scope == Scope::Background && handler.host_port == key && handler.path == "/"
    })
}

/// The mapping the book names, while it stands on a port the window uses.
fn named_mapping(
    config: &ServeConfig,
    dns_name: &str,
    (https, port): (Option<u16>, Option<u16>),
) -> Option<u16> {
    let (Some(https), Some(port)) = (https, port) else {
        return None;
    };
    (window_port(https) && ours_stands(config, dns_name, https, port)).then_some(https)
}

/// Add every phone port whose `/` sends to the listener.
fn add_phone_ports_to(config: &ServeConfig, dns_name: &str, listener: u16, ports: &mut Vec<u16>) {
    for https in HTTPS_PORTS {
        if !ports.contains(&https) && ours_stands(config, dns_name, https, listener) {
            ports.push(https);
        }
    }
}

/// Every mapping of the window's: the one the book names, wherever it now
/// points, and each phone port sending to the listener — what turning off,
/// closing and a start with the link off take down.
pub(crate) fn own_mappings(
    config: &ServeConfig,
    dns_name: &str,
    remembered: (Option<u16>, Option<u16>),
    listener: Option<u16>,
) -> Vec<u16> {
    let mut ports: Vec<u16> = named_mapping(config, dns_name, remembered)
        .into_iter()
        .collect();
    if let Some(listener) = listener {
        add_phone_ports_to(config, dns_name, listener, &mut ports);
    }
    ports
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Exposure {
    /// No mapping on the phone port.
    Absent,
    /// The phone port maps to this window's listener, tailnet only, alone.
    Ours,
    /// Someone else's mapping holds the phone port, or reaches the listener.
    Taken,
    /// Funnel — the public internet — is on the phone port or on something
    /// that reaches the listener. Refused.
    Funnel,
    /// The hook bridge's port is reachable through Serve or Funnel. Refused.
    HookExposed,
}

/// What holds one phone port on this machine's name, whoever it points at.
pub(crate) fn port_state(config: &ServeConfig, dns_name: &str, https_port: u16) -> Exposure {
    let key = key(dns_name, https_port);
    if config.funnel.contains(&key) {
        return Exposure::Funnel;
    }
    let held = config
        .handlers
        .iter()
        .any(|handler| handler.host_port == key)
        || config.tcp.iter().any(|forward| forward.port == https_port);
    if held {
        Exposure::Taken
    } else {
        Exposure::Absent
    }
}

/// Everything Serve sends to the listener's loopback port, in every scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// Nothing reaches it.
    Nothing,
    /// Exactly the window's own mapping on this phone port, and nothing else
    /// on that port or toward the listener.
    Ours(u16),
    /// Something reaching it is Funnelled.
    Funnel,
    /// Something besides the window's one mapping reaches it.
    Shared,
}

pub(crate) fn reach_of(config: &ServeConfig, dns_name: &str, listener_port: u16) -> Reach {
    let handlers: Vec<&Handler> = config.handlers_to(listener_port).collect();
    let forwards: Vec<&Forward> = config.forwards_to(listener_port).collect();
    if handlers.is_empty() && forwards.is_empty() {
        return Reach::Nothing;
    }
    let funnelled = handlers
        .iter()
        .any(|handler| config.funnel.contains(&handler.host_port))
        || forwards.iter().any(|forward| {
            config
                .funnel
                .iter()
                .any(|host_port| port_of_key(host_port) == Some(forward.port))
        });
    if funnelled {
        return Reach::Funnel;
    }
    if let ([only], []) = (handlers.as_slice(), forwards.as_slice())
        && let Some(https) = HTTPS_PORTS
            .into_iter()
            .find(|https| only.host_port == key(dns_name, *https))
        && only.scope == Scope::Background
        && only.path == "/"
    {
        let key = key(dns_name, https);
        let alone = config
            .handlers
            .iter()
            .filter(|handler| handler.host_port == key)
            .count()
            == 1
            && !config
                .tcp
                .iter()
                .any(|forward| forward.port == https && forward.target.is_some());
        if alone {
            return Reach::Ours(https);
        }
    }
    Reach::Shared
}

/// Why the listener stays closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Funnel is on what the listener sits behind.
    Funnel,
    /// The hook bridge is reachable through Serve.
    HookExposed,
    /// Something besides the window's mapping reaches the listener.
    Shared,
    /// The phone port the window used is not free, and nobody pressed 「켜기」:
    /// a start never moves the link to a port the person did not see it take.
    PortMoved,
    /// Every phone port is held.
    NoFreePort,
}

impl Refusal {
    /// The word the settings pane says it with (`ui/shell-remote.js`).
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Funnel => "refused",
            Self::HookExposed => "hook-exposed",
            Self::Shared => "listener-shared",
            Self::PortMoved => "port-moved",
            Self::NoFreePort => "no-free-port",
        }
    }

    pub(crate) fn exposure(self) -> Exposure {
        match self {
            Self::Funnel => Exposure::Funnel,
            Self::HookExposed => Exposure::HookExposed,
            Self::Shared | Self::PortMoved | Self::NoFreePort => Exposure::Taken,
        }
    }
}

/// What the window does next with a listener it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Then {
    /// Serve the listener behind `https`; `map` when the CLI must make the
    /// mapping.
    Serve { https: u16, map: bool },
    /// Close the listener and say why.
    Refuse(Refusal),
}

/// The window's own mappings to take down, by HTTPS port, and then what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) remove: Vec<u16>,
    pub(crate) then: Then,
}

/// What [`decide`] reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Facts<'a> {
    pub(crate) config: &'a ServeConfig,
    pub(crate) dns_name: &'a str,
    /// The mapping the book says this window made: HTTPS port, loopback port.
    pub(crate) remembered: (Option<u16>, Option<u16>),
    /// The loopback port the listener holds now.
    pub(crate) listener: u16,
    pub(crate) hook: Option<u16>,
    /// A person pressed 「켜기」: the link may take a new phone port.
    pub(crate) asked: bool,
}

/// The one judgment behind turning on, starting up and the minute's look,
/// with no CLI: which of the window's own mappings go, and whether the
/// listener is served or closed.
///
/// - The mapping the book names is the window's. It goes when it now points
///   at a loopback port the listener does not hold (somebody — the hook
///   bridge, another program — took it while the window was closed), when
///   Funnel is on it, or when its port is no longer a phone port.
/// - With that gone, the listener is refused when the hook bridge is
///   reachable, when Funnel is on anything reaching the listener, or when
///   anything besides one mapping of the window's reaches it. A refusal takes
///   down every mapping of the window's that still points at the listener.
/// - A start (`asked` false) serves only behind the phone port it had, never
///   a new one; a press takes the first free phone port.
pub(crate) fn decide(facts: &Facts<'_>) -> Decision {
    let Facts {
        config,
        dns_name,
        remembered: (remembered_https, remembered_port),
        listener,
        hook,
        asked,
    } = *facts;
    let mut remove = Vec::new();
    let mut funnelled_ours = false;
    if let Some(https) = named_mapping(config, dns_name, (remembered_https, remembered_port)) {
        funnelled_ours = config.funnel.contains(&key(dns_name, https));
        if remembered_port != Some(listener) || funnelled_ours || !HTTPS_PORTS.contains(&https) {
            remove.push(https);
        }
    }
    let after = config.without(dns_name, &remove);
    let refuse = |refusal: Refusal, mut remove: Vec<u16>| {
        add_phone_ports_to(&after, dns_name, listener, &mut remove);
        Decision {
            remove,
            then: Then::Refuse(refusal),
        }
    };
    if hook.is_some_and(|hook| after.reaches(hook)) {
        return refuse(Refusal::HookExposed, remove);
    }
    if funnelled_ours {
        return refuse(Refusal::Funnel, remove);
    }
    match reach_of(&after, dns_name, listener) {
        Reach::Funnel => return refuse(Refusal::Funnel, remove),
        Reach::Shared => return refuse(Refusal::Shared, remove),
        Reach::Ours(https) => {
            return Decision {
                remove,
                then: Then::Serve { https, map: false },
            };
        }
        Reach::Nothing => {}
    }
    let kept = remembered_https.filter(|https| HTTPS_PORTS.contains(https));
    if let Some(https) = kept {
        match port_state(&after, dns_name, https) {
            Exposure::Absent => {
                return Decision {
                    remove,
                    then: Then::Serve { https, map: true },
                };
            }
            Exposure::Funnel if !asked => return refuse(Refusal::Funnel, remove),
            _ if !asked => return refuse(Refusal::PortMoved, remove),
            _ => {}
        }
    } else if !asked {
        return refuse(Refusal::PortMoved, remove);
    }
    match HTTPS_PORTS
        .into_iter()
        .find(|https| port_state(&after, dns_name, *https) == Exposure::Absent)
    {
        Some(https) => Decision {
            remove,
            then: Then::Serve { https, map: true },
        },
        None => refuse(Refusal::NoFreePort, remove),
    }
}

/// What the settings pane shows for the link: the listener's reach while it
/// listens, the phone port's state while it does not.
pub(crate) fn exposure_of(
    config: &ServeConfig,
    dns_name: &str,
    https_port: u16,
    listener: Option<u16>,
    hook: Option<u16>,
) -> Exposure {
    if hook.is_some_and(|hook| config.reaches(hook)) {
        return Exposure::HookExposed;
    }
    match listener.map(|port| reach_of(config, dns_name, port)) {
        Some(Reach::Ours(_)) => Exposure::Ours,
        Some(Reach::Funnel) => Exposure::Funnel,
        Some(Reach::Shared) => Exposure::Taken,
        Some(Reach::Nothing) | None => port_state(config, dns_name, https_port),
    }
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

/// Run the CLI within `limit`; stdout on a zero exit, the CLI's own words
/// otherwise. Both pipes are read while the child
/// runs: `status --json` on a large tailnet is more than a pipe holds, and a
/// child whose pipe is full never exits.
pub(crate) fn run_within(cli: &Path, args: &[String], limit: Duration) -> Result<String, String> {
    #[allow(unused_mut)] // only macOS adds to it.
    let mut command = crate::proc::quiet_command(cli);
    #[cfg(target_os = "macos")]
    command.env("TERM", CLI_TERM);
    let mut child = command
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
            Ok(None) if started.elapsed() < limit => {
                std::thread::sleep(CLI_POLL);
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

/// `tailscale status --json`, or why it could not be read.
fn read_status_within(cli: &Path, limit: Duration) -> Result<Tailnet, String> {
    let text = run_within(cli, &["status".into(), "--json".into()], limit)?;
    serde_json::from_str::<Value>(&text)
        .map(|json| parse_status(&json))
        .map_err(|error| format!("tailscale status: {error}"))
}

/// `tailscale serve status --json`, or why it could not be read or was not
/// a shape this reads.
fn read_serve_within(cli: &Path, limit: Duration) -> Result<ServeConfig, String> {
    let text = run_within(
        cli,
        &["serve".into(), "status".into(), "--json".into()],
        limit,
    )?;
    if text.trim().is_empty() {
        return Ok(ServeConfig::default());
    }
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|json| parse_serve(&json).ok())
        .ok_or_else(|| SERVE_SHAPE.into())
}

/// What the window asks the CLI, so the judgments around it are tried
/// without one.
pub(crate) trait Cli {
    fn status(&self) -> Result<Tailnet, String>;
    fn serve(&self) -> Result<ServeConfig, String>;
    fn run(&self, args: &[String]) -> Result<String, String>;
}

/// The installed CLI, each call within [`CLI_TIMEOUT`] and all of them
/// within one deadline when it has one.
pub(crate) struct Installed {
    path: PathBuf,
    until: Option<std::time::Instant>,
}

impl Installed {
    pub(crate) fn find() -> Option<Self> {
        find_cli().map(|path| Self { path, until: None })
    }

    /// The CLI with every call it makes ending by `budget` from now.
    pub(crate) fn find_within(budget: Duration) -> Option<Self> {
        let until = std::time::Instant::now() + budget;
        find_cli().map(|path| Self {
            path,
            until: Some(until),
        })
    }

    fn limit(&self) -> Result<Duration, String> {
        let Some(until) = self.until else {
            return Ok(CLI_TIMEOUT);
        };
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err("tailscale did not answer in time".into());
        }
        Ok(left.min(CLI_TIMEOUT))
    }
}

impl Cli for Installed {
    fn status(&self) -> Result<Tailnet, String> {
        read_status_within(&self.path, self.limit()?)
    }

    fn serve(&self) -> Result<ServeConfig, String> {
        read_serve_within(&self.path, self.limit()?)
    }

    fn run(&self, args: &[String]) -> Result<String, String> {
        run_within(&self.path, args, self.limit()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HOST: &str = "host.example.test";

    fn config(value: Value) -> ServeConfig {
        parse_serve(&value).expect("a shape Tailscale writes")
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

    fn ours(https: u16, port: u16) -> Value {
        json!({
            "TCP": { https.to_string(): { "HTTPS": true } },
            "Web": { format!("{HOST}:{https}"): { "Handlers": { "/": { "Proxy": format!("http://127.0.0.1:{port}") } } } }
        })
    }

    fn facts<'a>(
        config: &'a ServeConfig,
        remembered: (Option<u16>, Option<u16>),
        listener: u16,
        hook: Option<u16>,
        asked: bool,
    ) -> Facts<'a> {
        Facts {
            config,
            dns_name: HOST,
            remembered,
            listener,
            hook,
            asked,
        }
    }

    #[test]
    fn the_cli_is_found_only_where_tailscale_installs_it() {
        let source = include_str!("tailscale.rs");
        let finder = source
            .split("pub(crate) fn find_cli()")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("find_cli");
        assert!(
            !finder.contains("\"PATH\"") && !finder.contains("split_paths"),
            "find_cli searches PATH:\n{finder}"
        );
        for place in INSTALLED_CLI {
            assert!(Path::new(place).is_absolute(), "{place}");
        }
    }

    #[test]
    fn the_phone_ports_are_none_funnel_can_open() {
        for funnel in [443, 8443, 10000] {
            assert!(!HTTPS_PORTS.contains(&funnel), "{funnel} is a Funnel port");
        }
    }

    #[test]
    fn someone_elses_mapping_on_443_leaves_the_first_phone_port_free() {
        let serve = config(json!({
            "TCP": { "443": { "HTTPS": true } },
            "Web": { "host.example.test:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:8788" } } } }
        }));
        assert_eq!(port_state(&serve, HOST, 443), Exposure::Taken);
        assert_eq!(port_state(&serve, HOST, 10443), Exposure::Absent);
        let pressed = decide(&facts(&serve, (None, None), 5000, None, true));
        assert_eq!(
            pressed,
            Decision {
                remove: vec![],
                then: Then::Serve {
                    https: 10443,
                    map: true
                }
            }
        );
    }

    #[test]
    fn our_own_mapping_is_recognised_and_kept() {
        let serve = config(ours(10443, 5000));
        assert_eq!(reach_of(&serve, HOST, 5000), Reach::Ours(10443));
        assert!(ours_stands(&serve, HOST, 10443, 5000));
        assert!(!ours_stands(&serve, HOST, 10443, 5001));
        let start = decide(&facts(&serve, (Some(10443), Some(5000)), 5000, None, false));
        assert_eq!(
            start.then,
            Then::Serve {
                https: 10443,
                map: false
            }
        );
        assert!(start.remove.is_empty());
        assert_eq!(
            exposure_of(&serve, HOST, 10443, Some(5000), None),
            Exposure::Ours
        );
    }

    /// The review's first scene: a mapping the window made, Funnel turned on
    /// it while the window was closed, the book still naming it. The start
    /// takes the mapping down and does not open the listener anywhere else.
    #[test]
    fn a_start_behind_our_funnelled_mapping_takes_it_down_and_stays_closed() {
        let mut value = ours(8443, 5000);
        value["AllowFunnel"] = json!({ "host.example.test:8443": true });
        let serve = config(value);
        let start = decide(&facts(&serve, (Some(8443), Some(5000)), 5000, None, false));
        assert_eq!(start.remove, vec![8443]);
        assert_eq!(start.then, Then::Refuse(Refusal::Funnel));
        let pressed = decide(&facts(&serve, (Some(8443), Some(5000)), 5000, None, true));
        assert_eq!(pressed.remove, vec![8443], "a press takes it down too");
        assert_eq!(pressed.then, Then::Refuse(Refusal::Funnel));
        // The same on a phone port, should Funnel ever reach one.
        let mut value = ours(10443, 5000);
        value["AllowFunnel"] = json!({ "host.example.test:10443": true });
        let serve = config(value);
        let watch = decide(&facts(&serve, (Some(10443), Some(5000)), 5000, None, false));
        assert_eq!(watch.remove, vec![10443]);
        assert_eq!(watch.then, Then::Refuse(Refusal::Funnel));
    }

    /// Funnel on anything that reaches the listener refuses it, whoever made
    /// that mapping and on whichever port.
    #[test]
    fn funnel_on_anything_reaching_the_listener_refuses_it() {
        let serve = config(json!({
            "Web": { "host.example.test:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5000" } } } },
            "AllowFunnel": { "host.example.test:443": true }
        }));
        assert_eq!(reach_of(&serve, HOST, 5000), Reach::Funnel);
        let start = decide(&facts(&serve, (Some(10443), Some(5000)), 5000, None, false));
        assert!(start.remove.is_empty(), "443 is not the window's");
        assert_eq!(start.then, Then::Refuse(Refusal::Funnel));
        let raw = config(json!({
            "TCP": { "10000": { "TCPForward": "127.0.0.1:5000" } },
            "AllowFunnel": { "host.example.test:10000": true }
        }));
        assert_eq!(reach_of(&raw, HOST, 5000), Reach::Funnel);
    }

    /// The review's second scene: the book names `10443 → P`, the window
    /// starts, the hook bridge took P while it was closed. The window's
    /// mapping goes before anything else is decided, and the listener on its
    /// new port is served behind the port it had.
    #[test]
    fn a_start_whose_old_port_the_hook_bridge_took_takes_the_mapping_down_first() {
        let serve = config(ours(10443, 6100));
        let start = decide(&facts(
            &serve,
            (Some(10443), Some(6100)),
            5000,
            Some(6100),
            false,
        ));
        assert_eq!(start.remove, vec![10443]);
        assert_eq!(
            start.then,
            Then::Serve {
                https: 10443,
                map: true
            }
        );
        // On a port that is no longer a phone port, it goes too, and a start
        // does not move to another.
        let serve = config(ours(8443, 6100));
        let old = decide(&facts(
            &serve,
            (Some(8443), Some(6100)),
            5000,
            Some(6100),
            false,
        ));
        assert_eq!(old.remove, vec![8443]);
        assert_eq!(old.then, Then::Refuse(Refusal::PortMoved));
    }

    #[test]
    fn the_hook_bridge_reachable_anywhere_refuses_and_takes_ours_down() {
        let web = config(json!({
            "TCP": { "10443": { "HTTPS": true } },
            "Web": {
                "host.example.test:443": { "Handlers": { "/hooks": { "Proxy": "http://localhost:6100" } } },
                "host.example.test:10443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5000" } } }
            }
        }));
        let watch = decide(&facts(
            &web,
            (Some(10443), Some(5000)),
            5000,
            Some(6100),
            false,
        ));
        assert_eq!(watch.then, Then::Refuse(Refusal::HookExposed));
        assert_eq!(
            watch.remove,
            vec![10443],
            "a closed listener leaves no mapping"
        );
        let tcp = config(json!({ "TCP": { "7000": { "TCPForward": "127.0.0.1:6100" } } }));
        assert_eq!(
            exposure_of(&tcp, HOST, 10443, None, Some(6100)),
            Exposure::HookExposed
        );
        let foreground = config(json!({
            "Foreground": { "session-1": {
                "Web": { "host.example.test:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:6100" } } } }
            } }
        }));
        assert_eq!(
            exposure_of(&foreground, HOST, 10443, None, Some(6100)),
            Exposure::HookExposed,
            "a terminal's `tailscale serve` without --bg counts"
        );
        let service = config(json!({
            "Services": { "svc:hooks": { "TCP": { "443": { "TCPForward": "127.0.0.1:6100" } } } }
        }));
        assert_eq!(
            exposure_of(&service, HOST, 10443, None, Some(6100)),
            Exposure::HookExposed,
            "a Tailscale service counts"
        );
        let elsewhere = config(json!({
            "Web": { "host.example.test:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:61000" } } } }
        }));
        assert_eq!(
            exposure_of(&elsewhere, HOST, 10443, None, Some(6100)),
            Exposure::Absent,
            "a port that merely starts with the hook's digits is not the hook"
        );
    }

    #[test]
    fn a_second_road_to_the_listener_refuses_it() {
        let mut value = ours(10443, 5000);
        value["Web"]["host.example.test:443"] =
            json!({ "Handlers": { "/": { "Proxy": "http://127.0.0.1:5000" } } });
        let serve = config(value);
        assert_eq!(reach_of(&serve, HOST, 5000), Reach::Shared);
        let watch = decide(&facts(&serve, (Some(10443), Some(5000)), 5000, None, false));
        assert_eq!(watch.then, Then::Refuse(Refusal::Shared));
        assert_eq!(watch.remove, vec![10443]);
        let foreground = config(json!({
            "Foreground": { "s": {
                "Web": { "host.example.test:10443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5000" } } } }
            } }
        }));
        assert_eq!(
            reach_of(&foreground, HOST, 5000),
            Reach::Shared,
            "a foreground mapping is never the window's"
        );
        let path = config(json!({
            "Web": { "host.example.test:10443": { "Handlers": {
                "/": { "Proxy": "http://127.0.0.1:5000" },
                "/other": { "Proxy": "http://127.0.0.1:9000" }
            } } }
        }));
        assert_eq!(reach_of(&path, HOST, 5000), Reach::Shared);
        assert!(
            ours_stands(&path, HOST, 10443, 5000),
            "but its `/` is still ours to take down"
        );
    }

    #[test]
    fn a_start_never_moves_to_a_port_the_person_did_not_see_it_take() {
        let serve = config(json!({
            "TCP": { "10443": { "HTTPS": true } },
            "Web": { "host.example.test:10443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:7777" } } } }
        }));
        let start = decide(&facts(&serve, (Some(10443), Some(5000)), 5000, None, false));
        assert_eq!(start.then, Then::Refuse(Refusal::PortMoved));
        assert!(
            start.remove.is_empty(),
            "somebody else's mapping is not touched"
        );
        let pressed = decide(&facts(&serve, (Some(10443), Some(5000)), 5000, None, true));
        assert_eq!(
            pressed.then,
            Then::Serve {
                https: 9443,
                map: true
            }
        );
        let none = decide(&facts(&serve, (None, None), 5000, None, false));
        assert_eq!(none.then, Then::Refuse(Refusal::PortMoved));
    }

    #[test]
    fn every_phone_port_held_is_no_free_port() {
        let serve = config(json!({
            "TCP": { "10443": { "HTTPS": true }, "9443": { "HTTPS": true } },
            "Web": {
                "host.example.test:10443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:7777" } } },
                "host.example.test:9443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:7778" } } }
            }
        }));
        let pressed = decide(&facts(&serve, (None, None), 5000, None, true));
        assert_eq!(pressed.then, Then::Refuse(Refusal::NoFreePort));
    }

    #[test]
    fn refusals_say_the_words_the_pane_knows() {
        let pane = include_str!("../../../../ui/shell-remote.js");
        for word in [
            Refusal::Funnel,
            Refusal::HookExposed,
            Refusal::Shared,
            Refusal::PortMoved,
            Refusal::NoFreePort,
        ]
        .map(Refusal::word)
        .into_iter()
        .chain([SERVE_SHAPE])
        {
            assert!(
                pane.contains(&format!("case \"{word}\":")),
                "the settings pane has no words for {word}"
            );
        }
    }

    /// The ports are said from [`HTTPS_PORTS`] alone: the pane reads them
    /// from the status, and no word of it names one.
    #[test]
    fn the_pane_says_the_phone_ports_the_window_sends() {
        let pane = include_str!("../../../../ui/shell-remote.js");
        let words = include_str!("../../../../ui/shell-i18n.js");
        assert!(pane.contains("status?.phonePorts"));
        for https in HTTPS_PORTS {
            let port = https.to_string();
            assert!(!pane.contains(&port), "shell-remote.js names {port}");
            assert!(
                !words
                    .lines()
                    .any(|line| line.contains("remoteLink.") && line.contains(&port)),
                "shell-i18n.js names {port}"
            );
        }
    }

    /// A window opened from the Finder has no `TERM`, and the macOS app's
    /// binary without one is not the CLI: the CLI is always told one.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_cli_is_told_a_terminal_the_window_does_not_have() {
        let said = run_within(
            Path::new("/usr/bin/printenv"),
            &["TERM".into()],
            CLI_TIMEOUT,
        )
        .expect("printenv");
        assert_eq!(said.trim(), CLI_TERM);
    }

    #[test]
    fn on_and_off_name_one_port_and_never_reset() {
        assert_eq!(
            serve_on_args(10443, 5000),
            [
                "serve",
                "--bg",
                "--yes",
                "--https=10443",
                "http://127.0.0.1:5000"
            ]
        );
        assert_eq!(serve_off_args(10443), ["serve", "--https=10443", "off"]);
        assert!(!serve_off_args(10443).iter().any(|arg| arg == "reset"));
    }

    #[test]
    fn an_empty_serve_status_is_an_empty_configuration() {
        assert_eq!(config(json!({})), ServeConfig::default());
    }

    /// What the window takes down is one picker for every road: the book's
    /// mapping on a port the window uses, and phone ports sending to the
    /// listener — never a port a hand-edited book names.
    #[test]
    fn a_book_naming_a_port_the_window_never_uses_is_not_believed() {
        let person = config(json!({
            "TCP": { "443": { "HTTPS": true } },
            "Web": { "host.example.test:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5000" } } } }
        }));
        assert!(own_mappings(&person, HOST, (Some(443), Some(5000)), None).is_empty());
        let start = decide(&facts(&person, (Some(443), Some(5000)), 6000, None, false));
        assert!(start.remove.is_empty(), "443 is the person's");
        let window = config(ours(10443, 5000));
        assert_eq!(
            own_mappings(&window, HOST, (Some(10443), Some(5000)), None),
            vec![10443]
        );
        assert_eq!(
            own_mappings(&window, HOST, (None, None), Some(5000)),
            vec![10443]
        );
        assert!(own_mappings(&window, HOST, (Some(10443), Some(5001)), Some(5001)).is_empty());
    }

    /// A shape this does not know is never read as "nothing exposed".
    #[test]
    fn a_serve_status_of_an_unknown_shape_is_refused() {
        for unknown in [
            json!({ "Tunnels": { "x": {} } }),
            json!({ "Web": [] }),
            json!({ "TCP": { "443": { "HTTPS": true, "Upstream": "127.0.0.1:5000" } } }),
            json!({ "TCP": { "https": { "HTTPS": true } } }),
            json!({ "TCP": { "7000": { "TCPForward": 5000 } } }),
            json!({ "Web": { "host.example.test:443": { "Handlers": { "/": { "Proxy": 5000 } } } } }),
            json!({ "Web": { "host.example.test:443": { "Handlers": { "/": { "Upstream": "x" } } } } }),
            json!({ "AllowFunnel": { "host.example.test:443": "yes" } }),
            json!({ "Services": { "svc:a": { "Tun": "yes" } } }),
            json!({ "Foreground": { "s": { "Foreground": {} } } }),
        ] {
            assert!(parse_serve(&unknown).is_err(), "{unknown}");
        }
        // Every field Tailscale writes is read without complaint.
        let known = config(json!({
            "TCP": { "443": { "HTTPS": true, "HTTP": false, "TerminateTLS": "", "ProxyProtocol": 0 } },
            "Web": { "host.example.test:443": { "Handlers": {
                "/": { "Path": "/srv", "Text": "hi", "AcceptAppCaps": [], "Redirect": "" }
            } } },
            "Services": { "svc:a": { "Tun": true } },
            "Foreground": { "s": { "AllowFunnel": {} } }
        }));
        assert_eq!(known.handlers.len(), 1);
        assert_eq!(known.handlers[0].proxy, None);
    }
}
