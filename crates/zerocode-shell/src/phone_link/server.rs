//! The phone listener: 127.0.0.1 only, a short allow-list, a bearer token on
//! every API call.
//!
//! Tailscale Serve hands a tailnet request to this port and it arrives from
//! loopback — so loopback proves nothing here, unlike the hook bridge. Every
//! request is judged on its own: the Host header must name this machine
//! (DNS rebinding), the path must be on the list (everything else is 404),
//! and an API path needs a device token the book knows (else 401). The window's
//! own command surface is not reachable from here at all.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::book::{self, Book, Pairing, PushTarget, Redeem};

/// A request line and headers bigger than this are not a phone.
const MAX_HEAD: usize = 16 * 1024;
/// The largest body any allowed route takes (a push subscription).
const MAX_BODY: usize = 8 * 1024;
/// A phone's `last seen` is written to disk at most this often.
const SEEN_WRITE_MS: i64 = 60 * 1000;

/// The phone app, served without a token: it holds nothing but code.
struct Asset {
    path: &'static str,
    content_type: &'static str,
    body: &'static str,
}

const INDEX: &str = include_str!("../../../../ui/phone/index.html");
const ASSETS: &[Asset] = &[
    Asset {
        path: "/phone.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_str!("../../../../ui/phone/phone.js"),
    },
    Asset {
        path: "/phone-i18n.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_str!("../../../../ui/phone/phone-i18n.js"),
    },
    Asset {
        path: "/phone.css",
        content_type: "text/css; charset=utf-8",
        body: include_str!("../../../../ui/phone/phone.css"),
    },
    Asset {
        path: "/tokens.css",
        content_type: "text/css; charset=utf-8",
        body: include_str!("../../../../ui/tokens.css"),
    },
    Asset {
        path: "/sw.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_str!("../../../../ui/phone/sw.js"),
    },
    Asset {
        path: "/manifest.webmanifest",
        content_type: "application/manifest+json",
        body: include_str!("../../../../ui/phone/manifest.webmanifest"),
    },
    Asset {
        path: "/icon.svg",
        content_type: "image/svg+xml",
        body: include_str!("../../../../ui/phone/icon.svg"),
    },
];

/// Every API path, with the one method it answers. A path not here is 404;
/// a path here asked with another method is 404 too — the list is the whole
/// surface.
const API: &[(&str, &str)] = &[
    ("GET", "/api/status"),
    ("GET", "/api/panes"),
    ("POST", "/api/push/subscribe"),
    ("POST", "/api/push/unsubscribe"),
];
const PAIR: (&str, &str) = ("POST", "/api/pair");

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                   connect-src 'self'; manifest-src 'self'; worker-src 'self'; \
                   base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[derive(Debug, Clone, Default)]
pub(crate) struct Request {
    pub(crate) method: String,
    /// The path without its query string.
    pub(crate) path: String,
    /// Lowercased names.
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl Request {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) content_type: &'static str,
    pub(crate) body: Vec<u8>,
    pub(crate) extra: Vec<(&'static str, String)>,
}

impl Response {
    fn json(status: u16, value: &serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: value.to_string().into_bytes(),
            extra: Vec::new(),
        }
    }

    fn error(status: u16, word: &str) -> Self {
        Self::json(status, &json!({ "error": word }))
    }

    fn text(status: u16, content_type: &'static str, body: &str) -> Self {
        Self {
            status,
            content_type,
            body: body.as_bytes().to_vec(),
            extra: Vec::new(),
        }
    }
}

/// One pane, as a phone may see it: who, which checkout by folder name, what
/// it is waiting on, since when. Never a path, a command line, an environment
/// value, the agent's words or the screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PaneView {
    pub(crate) id: String,
    pub(crate) agent: String,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) task: Option<String>,
    /// `attention`, `working` or `done`.
    pub(crate) group: &'static str,
    /// `question` or `permission` when `group` is `attention`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) waiting: Option<&'static str>,
    pub(crate) since_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusView {
    pub(crate) version: String,
    pub(crate) name: String,
}

/// What the listener reads from the window, behind a seam so the router is
/// tested without one.
pub(crate) trait Window: Send + Sync {
    fn now_ms(&self) -> i64;
    fn status(&self) -> StatusView;
    fn panes(&self) -> Vec<PaneView>;
    /// A phone has just paired: the window tells the person, by the name the
    /// phone gave and the time. Anything on this machine can reach the
    /// listener from loopback and claim the owner's login, so the person is
    /// the one who notices a phone that is not theirs.
    fn paired(&self, _name: &str, _at_ms: i64) {}
}

/// Which Host headers are this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Hosts {
    pub(crate) dns_name: Option<String>,
    pub(crate) listener_port: u16,
}

impl Hosts {
    pub(crate) fn admits(&self, host: Option<&str>) -> bool {
        let Some(host) = host else {
            return false;
        };
        let host = host.trim().to_ascii_lowercase();
        let (name, port) = match host.rsplit_once(':') {
            Some((name, port)) if !name.ends_with(']') || name.starts_with('[') => {
                (name.to_string(), port.parse::<u16>().ok())
            }
            _ => (host.clone(), None),
        };
        if name == "127.0.0.1" {
            return port.is_none() || port == Some(self.listener_port);
        }
        self.dns_name
            .as_deref()
            .is_some_and(|dns| !dns.is_empty() && dns.eq_ignore_ascii_case(&name))
    }
}

/// The listener's whole state. The router reads and writes it under short
/// locks; nothing here waits on the network.
pub(crate) struct Core {
    book_path: Option<PathBuf>,
    pub(crate) book: Mutex<Book>,
    pub(crate) pairing: Mutex<Pairing>,
    pub(crate) hosts: Mutex<Hosts>,
    /// The tailnet login that owns this machine. Pairing is refused to any
    /// other login, and to a request that names none.
    pub(crate) owner: Mutex<Option<String>>,
    /// The VAPID public key, base64url, for the phone's `subscribe`.
    pub(crate) vapid_public: Mutex<Option<String>>,
    /// Held across the copy and the write of one save, so two saves land in
    /// the order their copies were taken and never share the staging file.
    saving: Mutex<()>,
}

impl Core {
    pub(crate) fn new(book_path: Option<PathBuf>, book: Book) -> Self {
        Self {
            book_path,
            book: Mutex::new(book),
            pairing: Mutex::new(Pairing::default()),
            hosts: Mutex::new(Hosts::default()),
            owner: Mutex::new(None),
            vapid_public: Mutex::new(None),
            saving: Mutex::new(()),
        }
    }

    /// Write the book as it stands now. Callers change the book under its
    /// own lock and let go of it first: the copy is taken here, under the
    /// save lock, so a later save always writes a later book.
    pub(crate) fn save(&self) {
        let Some(path) = &self.book_path else {
            return;
        };
        let Ok(_saving) = self.saving.lock() else {
            return;
        };
        let Some(snapshot) = self.book.lock().ok().map(|book| book.clone()) else {
            return;
        };
        if let Err(error) = book::save(path, &snapshot) {
            eprintln!("zerocode-shell: phone link book not saved: {error}");
        }
    }

    pub(crate) fn handle(&self, window: &dyn Window, request: &Request) -> Response {
        let mut response = self.route(window, request);
        response.extra.push(("Cache-Control", "no-store".into()));
        response
            .extra
            .push(("X-Content-Type-Options", "nosniff".into()));
        response
            .extra
            .push(("Referrer-Policy", "no-referrer".into()));
        response.extra.push(("X-Frame-Options", "DENY".into()));
        response.extra.push(("Content-Security-Policy", CSP.into()));
        response
    }

    fn route(&self, window: &dyn Window, request: &Request) -> Response {
        let admitted = self
            .hosts
            .lock()
            .map(|hosts| hosts.admits(request.header("host")))
            .unwrap_or(false);
        if !admitted {
            return Response::error(421, "host");
        }
        let method = request.method.as_str();
        let path = request.path.as_str();
        if method == "GET" || method == "HEAD" {
            if matches!(path, "/" | "/pair" | "/index.html") {
                return Response::text(200, "text/html; charset=utf-8", INDEX);
            }
            if let Some(asset) = ASSETS.iter().find(|asset| asset.path == path) {
                return Response::text(200, asset.content_type, asset.body);
            }
        }
        if (method, path) == PAIR {
            if !json_body(request) {
                return Response::error(415, "json");
            }
            return self.pair(window, request);
        }
        if !API.iter().any(|(_, known)| *known == path) {
            return Response::error(404, "not found");
        }
        if !API
            .iter()
            .any(|(known_method, known)| *known_method == method && *known == path)
        {
            return Response::error(404, "not found");
        }
        let Some(device) = self.authorize(window, request) else {
            let mut refused = Response::error(401, "token");
            refused.extra.push(("WWW-Authenticate", "Bearer".into()));
            return refused;
        };
        match path {
            "/api/status" => {
                let status = window.status();
                let vapid = self.vapid_public.lock().ok().and_then(|held| held.clone());
                Response::json(
                    200,
                    &json!({
                        "version": status.version,
                        "name": status.name,
                        "vapidPublicKey": vapid,
                    }),
                )
            }
            "/api/panes" => Response::json(
                200,
                &json!({ "now": window.now_ms(), "panes": window.panes() }),
            ),
            "/api/push/subscribe" if !json_body(request) => Response::error(415, "json"),
            "/api/push/subscribe" => self.subscribe(&device, request),
            "/api/push/unsubscribe" => self.unsubscribe(&device),
            _ => Response::error(404, "not found"),
        }
    }

    /// The device id a request's bearer token names, refreshing its
    /// `last seen`.
    fn authorize(&self, window: &dyn Window, request: &Request) -> Option<String> {
        let token = request
            .header("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|token| !token.is_empty())?;
        let mut book = self.book.lock().ok()?;
        let at = book.device_of_token(token)?;
        let now = window.now_ms();
        let device = &mut book.devices[at];
        let id = device.id.clone();
        let stale = now.saturating_sub(device.last_seen_ms) >= SEEN_WRITE_MS;
        device.last_seen_ms = now;
        if stale {
            drop(book);
            self.save();
        }
        Some(id)
    }

    fn pair(&self, window: &dyn Window, request: &Request) -> Response {
        #[derive(Deserialize)]
        struct Ask {
            code: String,
            #[serde(default)]
            name: String,
        }
        let owner = self.owner.lock().ok().and_then(|held| held.clone());
        let login = request.header("tailscale-user-login");
        let owner_matches = matches!((owner.as_deref(), login), (Some(owner), Some(login)) if owner.eq_ignore_ascii_case(login.trim()));
        if !owner_matches {
            return Response::error(403, "owner");
        }
        let Ok(ask) = serde_json::from_slice::<Ask>(&request.body) else {
            return Response::error(400, "body");
        };
        let now = window.now_ms();
        let verdict = match self.pairing.lock() {
            Ok(mut pairing) => pairing.redeem(&ask.code, now),
            Err(_) => Redeem::NoCode,
        };
        match verdict {
            Redeem::Accepted => {}
            Redeem::Wrong => return Response::error(403, "code"),
            Redeem::Burned => return Response::error(403, "burned"),
            Redeem::Expired => return Response::error(410, "expired"),
            Redeem::NoCode => return Response::error(410, "no code"),
        }
        let Some(token) = super::random_bytes::<32>().map(|bytes| super::web_push::encode(&bytes))
        else {
            return Response::error(500, "random");
        };
        let id = uuid::Uuid::new_v4().to_string();
        let named = {
            let Ok(mut book) = self.book.lock() else {
                return Response::error(500, "book");
            };
            book.pair(&ask.name, &token, id.clone(), now);
            book.devices
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.name.clone())
                .unwrap_or_default()
        };
        self.save();
        window.paired(&named, now);
        let vapid = self.vapid_public.lock().ok().and_then(|held| held.clone());
        Response::json(
            200,
            &json!({ "token": token, "deviceId": id, "vapidPublicKey": vapid }),
        )
    }

    fn subscribe(&self, device: &str, request: &Request) -> Response {
        #[derive(Deserialize)]
        struct Keys {
            p256dh: String,
            auth: String,
        }
        #[derive(Deserialize)]
        struct Subscription {
            endpoint: String,
            keys: Keys,
        }
        let Ok(subscription) = serde_json::from_slice::<Subscription>(&request.body) else {
            return Response::error(400, "body");
        };
        if super::web_push::audience_of(&subscription.endpoint).is_none()
            || super::web_push::Receiver::from_keys(
                &subscription.keys.p256dh,
                &subscription.keys.auth,
            )
            .is_err()
        {
            return Response::error(400, "subscription");
        }
        {
            let Ok(mut book) = self.book.lock() else {
                return Response::error(500, "book");
            };
            let Some(row) = book.devices.iter_mut().find(|row| row.id == device) else {
                return Response::error(401, "token");
            };
            row.push = Some(PushTarget {
                endpoint: subscription.endpoint,
                p256dh: subscription.keys.p256dh,
                auth: subscription.keys.auth,
            });
        }
        self.save();
        Response::json(200, &json!({ "push": true }))
    }

    fn unsubscribe(&self, device: &str) -> Response {
        {
            let Ok(mut book) = self.book.lock() else {
                return Response::error(500, "book");
            };
            if let Some(row) = book.devices.iter_mut().find(|row| row.id == device) {
                row.push = None;
            }
        }
        self.save();
        Response::json(200, &json!({ "push": false }))
    }
}

/// A body declared as JSON. A page on another origin can send a plain-text
/// POST without asking first; it cannot send this one without a CORS
/// preflight, which this listener never answers.
fn json_body(request: &Request) -> bool {
    request.header("content-type").is_some_and(|value| {
        value
            .trim()
            .to_ascii_lowercase()
            .starts_with("application/json")
    })
}

/// Parse one request head. `None` for anything that is not a plain
/// HTTP/1.x request line with headers.
pub(crate) fn parse_head(head: &[u8]) -> Option<Request> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    let version = first.next()?;
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return None;
    }
    let path = target.split(['?', '#']).next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    Some(Request {
        method,
        path,
        headers,
        body: Vec::new(),
    })
}

pub(crate) fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        410 => "Gone",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        421 => "Misdirected Request",
        _ => "Error",
    }
}

pub(crate) fn encode_response(response: &Response, head_only: bool) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason(response.status),
        response.content_type,
        response.body.len()
    );
    for (name, value) in &response.extra {
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    if !head_only {
        bytes.extend_from_slice(&response.body);
    }
    bytes
}

/// Serve one connection: one request, one answer, then close.
pub(crate) async fn serve_connection(
    mut stream: tokio::net::TcpStream,
    core: std::sync::Arc<Core>,
    window: std::sync::Arc<dyn Window>,
) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let read = async {
        let mut buffer = Vec::with_capacity(2048);
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                break at + 4;
            }
            if buffer.len() > MAX_HEAD {
                return Err(413_u16);
            }
            let read = stream.read(&mut chunk).await.map_err(|_| 400_u16)?;
            if read == 0 {
                return Err(400_u16);
            }
            buffer.extend_from_slice(&chunk[..read]);
        };
        let mut request = parse_head(&buffer[..head_end]).ok_or(400_u16)?;
        let length = request
            .header("content-length")
            .map(|value| value.parse::<usize>().map_err(|_| 400_u16))
            .transpose()?
            .unwrap_or(0);
        if length > MAX_BODY || request.header("transfer-encoding").is_some() {
            return Err(413_u16);
        }
        let mut body = buffer[head_end..].to_vec();
        while body.len() < length {
            let read = stream.read(&mut chunk).await.map_err(|_| 400_u16)?;
            if read == 0 {
                return Err(400_u16);
            }
            body.extend_from_slice(&chunk[..read]);
        }
        body.truncate(length);
        request.body = body;
        Ok::<Request, u16>(request)
    };
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), read).await;
    let (response, head_only) = match outcome {
        Ok(Ok(request)) => {
            let head_only = request.method == "HEAD";
            (core.handle(window.as_ref(), &request), head_only)
        }
        Ok(Err(status)) => (Response::error(status, "request"), false),
        Err(_) => return,
    };
    let bytes = encode_response(&response, head_only);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let _ = stream.write_all(&bytes).await;
        let _ = stream.shutdown().await;
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "host.example.test";
    const OWNER: &str = "owner@example.test";

    struct FakeWindow {
        now: i64,
        panes: Vec<PaneView>,
        /// What the window was told of new phones.
        told: Mutex<Vec<(String, i64)>>,
    }

    impl Window for FakeWindow {
        fn now_ms(&self) -> i64 {
            self.now
        }
        fn status(&self) -> StatusView {
            StatusView {
                version: "9.9.9".into(),
                name: "Fixture".into(),
            }
        }
        fn panes(&self) -> Vec<PaneView> {
            self.panes.clone()
        }
        fn paired(&self, name: &str, at_ms: i64) {
            self.told.lock().unwrap().push((name.to_string(), at_ms));
        }
    }

    fn window() -> FakeWindow {
        FakeWindow {
            now: 1_000,
            panes: vec![PaneView {
                id: "term-3".into(),
                agent: "claude".into(),
                name: "checkout-a".into(),
                task: None,
                group: "attention",
                waiting: Some("permission"),
                since_ms: 900,
            }],
            told: Mutex::new(Vec::new()),
        }
    }

    fn core() -> Core {
        let core = Core::new(None, Book::default());
        *core.hosts.lock().unwrap() = Hosts {
            dns_name: Some(HOST.into()),
            listener_port: 5000,
        };
        *core.owner.lock().unwrap() = Some(OWNER.into());
        *core.vapid_public.lock().unwrap() = Some("fixture-vapid".into());
        core
    }

    fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            headers: headers
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), (*value).to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
        }
    }

    fn from_tailnet(method: &str, path: &str, extra: &[(&str, &str)], body: &str) -> Request {
        let mut headers = vec![("host", "host.example.test:8443")];
        if method == "POST" && !extra.iter().any(|(name, _)| *name == "content-type") {
            headers.push(("content-type", "application/json"));
        }
        headers.extend_from_slice(extra);
        request(method, path, &headers, body)
    }

    fn paired(core: &Core, window: &FakeWindow) -> String {
        core.pairing
            .lock()
            .unwrap()
            .issue_exact("ABCD2345", window.now);
        let answer = core.handle(
            window,
            &from_tailnet(
                "POST",
                "/api/pair",
                &[
                    ("tailscale-user-login", OWNER),
                    ("content-type", "application/json"),
                ],
                r#"{"code":"ABCD2345","name":"Fixture phone"}"#,
            ),
        );
        assert_eq!(
            answer.status,
            200,
            "{}",
            String::from_utf8_lossy(&answer.body)
        );
        let value: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        assert_eq!(value["vapidPublicKey"], "fixture-vapid");
        assert_eq!(
            window.told.lock().unwrap().last(),
            Some(&("Fixture phone".to_string(), window.now)),
            "the window is told of the phone it just paired"
        );
        value["token"].as_str().unwrap().to_string()
    }

    /// Saves from many threads land in order: the file ends as the book does.
    #[test]
    fn concurrent_saves_leave_the_last_book_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = book::book_path(dir.path());
        let core = std::sync::Arc::new(Core::new(Some(path.clone()), Book::default()));
        let writers: Vec<_> = (0..8_u16)
            .map(|round| {
                let core = std::sync::Arc::clone(&core);
                std::thread::spawn(move || {
                    for step in 0..20_u16 {
                        core.book.lock().unwrap().port = Some(round * 100 + step);
                        core.save();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().expect("writer");
        }
        // No save after the joins: the last one any writer made must already
        // hold the book's last state.
        assert_eq!(book::load(&path), *core.book.lock().unwrap());
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn a_refused_pairing_tells_the_window_nothing() {
        let core = core();
        let window = window();
        core.pairing
            .lock()
            .unwrap()
            .issue_exact("ABCD2345", window.now);
        let wrong = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/pair",
                &[("tailscale-user-login", OWNER)],
                r#"{"code":"WRONG234","name":"Fixture phone"}"#,
            ),
        );
        assert_eq!(wrong.status, 403);
        assert!(window.told.lock().unwrap().is_empty());
    }

    #[test]
    fn host_header_must_name_this_machine() {
        let core = core();
        let window = window();
        for host in [
            "host.example.test",
            "HOST.example.test:8443",
            "127.0.0.1",
            "127.0.0.1:5000",
        ] {
            let answer = core.handle(&window, &request("GET", "/", &[("host", host)], ""));
            assert_eq!(answer.status, 200, "{host}");
        }
        for host in ["evil.example.test", "127.0.0.1:5001", "localhost", ""] {
            let answer = core.handle(&window, &request("GET", "/", &[("host", host)], ""));
            assert_eq!(answer.status, 421, "{host}");
        }
        let answer = core.handle(&window, &request("GET", "/", &[], ""));
        assert_eq!(answer.status, 421, "no Host at all");
    }

    #[test]
    fn api_without_a_token_is_401_and_with_one_is_200() {
        let core = core();
        let window = window();
        let refused = core.handle(&window, &from_tailnet("GET", "/api/panes", &[], ""));
        assert_eq!(refused.status, 401);
        assert!(
            refused
                .extra
                .iter()
                .any(|(name, _)| *name == "WWW-Authenticate")
        );
        let wrong = core.handle(
            &window,
            &from_tailnet("GET", "/api/panes", &[("authorization", "Bearer nope")], ""),
        );
        assert_eq!(wrong.status, 401);
        let token = paired(&core, &window);
        let bearer = format!("Bearer {token}");
        let answer = core.handle(
            &window,
            &from_tailnet("GET", "/api/panes", &[("authorization", &bearer)], ""),
        );
        assert_eq!(answer.status, 200);
        let status = core.handle(
            &window,
            &from_tailnet("GET", "/api/status", &[("authorization", &bearer)], ""),
        );
        assert_eq!(status.status, 200);
    }

    #[test]
    fn paths_off_the_list_are_404_with_or_without_a_token() {
        let core = core();
        let window = window();
        let token = paired(&core, &window);
        let bearer = format!("Bearer {token}");
        for (method, path) in [
            ("GET", "/api/term_write"),
            ("POST", "/api/panes"),
            ("GET", "/api/push/subscribe"),
            ("GET", "/../Cargo.toml"),
            ("GET", "/ipc/board_snapshot"),
            ("DELETE", "/api/status"),
        ] {
            for headers in [vec![], vec![("authorization", bearer.as_str())]] {
                let answer = core.handle(&window, &from_tailnet(method, path, &headers, ""));
                assert_eq!(answer.status, 404, "{method} {path}");
            }
        }
    }

    #[test]
    fn a_revoked_phone_is_refused_at_once() {
        let core = core();
        let window = window();
        let token = paired(&core, &window);
        let id = core.book.lock().unwrap().devices[0].id.clone();
        assert!(core.book.lock().unwrap().revoke(&id));
        let bearer = format!("Bearer {token}");
        let answer = core.handle(
            &window,
            &from_tailnet("GET", "/api/panes", &[("authorization", &bearer)], ""),
        );
        assert_eq!(answer.status, 401);
    }

    #[test]
    fn pairing_needs_the_machine_owner_and_a_standing_code() {
        let core = core();
        let window = window();
        core.pairing.lock().unwrap().issue_exact("ABCD2345", 0);
        let body = r#"{"code":"ABCD2345","name":"x"}"#;
        let stranger = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/pair",
                &[("tailscale-user-login", "someone@example.test")],
                body,
            ),
        );
        assert_eq!(stranger.status, 403, "another tailnet user may not pair");
        let anonymous = core.handle(&window, &from_tailnet("POST", "/api/pair", &[], body));
        assert_eq!(
            anonymous.status, 403,
            "a request naming no login may not pair"
        );
        let wrong = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/pair",
                &[("tailscale-user-login", OWNER)],
                r#"{"code":"ZZZZZZZZ"}"#,
            ),
        );
        assert_eq!(wrong.status, 403);
        let right = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/pair",
                &[("tailscale-user-login", OWNER)],
                body,
            ),
        );
        assert_eq!(right.status, 200);
        let again = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/pair",
                &[("tailscale-user-login", OWNER)],
                body,
            ),
        );
        assert_eq!(again.status, 410, "a code works once");
        let book = core.book.lock().unwrap();
        assert_eq!(book.devices.len(), 1);
        let token: serde_json::Value = serde_json::from_slice(&right.body).unwrap();
        let token = token["token"].as_str().unwrap();
        assert!(token.len() >= 43, "32 random bytes");
        assert!(!serde_json::to_string(&*book).unwrap().contains(token));
    }

    #[test]
    fn with_no_owner_known_nobody_pairs() {
        let core = core();
        *core.owner.lock().unwrap() = None;
        let window = window();
        core.pairing.lock().unwrap().issue_exact("ABCD2345", 0);
        let answer = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/pair",
                &[("tailscale-user-login", OWNER)],
                r#"{"code":"ABCD2345"}"#,
            ),
        );
        assert_eq!(answer.status, 403);
    }

    #[test]
    fn answers_carry_no_path_command_line_or_screen_text() {
        let core = core();
        let window = window();
        let token = paired(&core, &window);
        let bearer = format!("Bearer {token}");
        let answer = core.handle(
            &window,
            &from_tailnet("GET", "/api/panes", &[("authorization", &bearer)], ""),
        );
        let text = String::from_utf8(answer.body).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let pane = &value["panes"][0];
        let keys: Vec<&str> = pane
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for key in &keys {
            assert!(
                ["id", "agent", "name", "task", "group", "waiting", "sinceMs"].contains(key),
                "unexpected field {key}"
            );
        }
        assert!(!text.contains('/'), "no path-shaped value: {text}");
    }

    #[test]
    fn a_subscription_is_kept_only_when_it_is_a_real_push_endpoint() {
        let core = core();
        let window = window();
        let token = paired(&core, &window);
        let bearer = format!("Bearer {token}");
        let bad = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/push/subscribe",
                &[("authorization", &bearer)],
                r#"{"endpoint":"http://web.push.apple.com/x","keys":{"p256dh":"a","auth":"b"}}"#,
            ),
        );
        assert_eq!(bad.status, 400);
        let elsewhere = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/push/subscribe",
                &[("authorization", &bearer)],
                r#"{"endpoint":"https://host.example.test:8443/api/pair","keys":{"p256dh":"BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4","auth":"BTBZMqHH6r4Tts7J_aSIgg"}}"#,
            ),
        );
        assert_eq!(
            elsewhere.status, 400,
            "a push service is the only place the window posts to"
        );
        let good = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/push/subscribe",
                &[("authorization", &bearer)],
                r#"{"endpoint":"https://web.push.apple.com/x","keys":{"p256dh":"BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4","auth":"BTBZMqHH6r4Tts7J_aSIgg"}}"#,
            ),
        );
        assert_eq!(good.status, 200);
        assert_eq!(core.book.lock().unwrap().push_targets().len(), 1);
        let off = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/push/unsubscribe",
                &[("authorization", &bearer)],
                "",
            ),
        );
        assert_eq!(off.status, 200);
        assert!(core.book.lock().unwrap().push_targets().is_empty());
    }

    #[test]
    fn the_app_shell_is_served_without_a_token_under_a_strict_policy() {
        let core = core();
        let window = window();
        for path in [
            "/",
            "/pair",
            "/phone.js",
            "/sw.js",
            "/tokens.css",
            "/manifest.webmanifest",
        ] {
            let answer = core.handle(&window, &from_tailnet("GET", path, &[], ""));
            assert_eq!(answer.status, 200, "{path}");
            assert!(
                answer
                    .extra
                    .iter()
                    .any(|(name, value)| *name == "Content-Security-Policy"
                        && value.contains("frame-ancestors 'none'"))
            );
            assert!(
                answer
                    .extra
                    .iter()
                    .any(|(name, value)| *name == "Cache-Control" && value == "no-store")
            );
        }
    }

    #[test]
    fn a_post_that_is_not_json_is_refused_before_it_is_read() {
        let core = core();
        let window = window();
        core.pairing.lock().unwrap().issue_exact("ABCD2345", 0);
        let answer = core.handle(
            &window,
            &from_tailnet(
                "POST",
                "/api/pair",
                &[
                    ("tailscale-user-login", OWNER),
                    ("content-type", "text/plain"),
                ],
                r#"{"code":"ABCD2345"}"#,
            ),
        );
        assert_eq!(answer.status, 415);
        assert_eq!(
            core.pairing.lock().unwrap().standing_until(1),
            Some(book::CODE_LIFETIME_MS),
            "a refused request does not spend the code"
        );
    }

    #[test]
    fn request_heads_parse_and_junk_does_not() {
        let parsed = parse_head(
            b"GET /api/panes?x=1 HTTP/1.1\r\nHost: host.example.test\r\nAuthorization: Bearer t\r\n\r\n",
        )
        .expect("a request");
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.path, "/api/panes");
        assert_eq!(parsed.header("host"), Some("host.example.test"));
        assert_eq!(parsed.header("authorization"), Some("Bearer t"));
        assert!(parse_head(b"GET http://elsewhere/ HTTP/1.1\r\n\r\n").is_none());
        assert!(parse_head(b"\xff\xfe\r\n\r\n").is_none());
        assert!(parse_head(b"GET / SPDY/3\r\n\r\n").is_none());
    }

    #[test]
    fn a_head_request_gets_headers_without_a_body() {
        let response = Response::text(200, "text/plain", "hello");
        let full = encode_response(&response, false);
        let head = encode_response(&response, true);
        assert!(full.ends_with(b"hello"));
        assert!(!head.ends_with(b"hello"));
        assert!(String::from_utf8_lossy(&head).contains("Content-Length: 5"));
    }
}
