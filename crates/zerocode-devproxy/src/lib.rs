//! The socket half of the localhost worktree labels.
//!
//! `zerocode_core::localhost_label` decides what a served port is called;
//! this crate is the small loopback server that makes the name resolve. A
//! request arriving as `ui-auth.zerocode.localhost:<port>` is matched to its
//! route by the `Host` header alone, forwarded to the dev server that owns
//! that label, and answered with whatever came back — headers and body
//! untouched, because the point of the hop is the name, not the content.
//!
//! Ported from `main/localhost-worktree-label-proxy.ts`. Two things are
//! deliberately not ported. The original copies every header across both
//! directions, which is safe in Node only because its own client re-derives
//! framing; hyper honours what it is handed, so [`headers`] filters
//! hop-by-hop fields by hand. And the original validates a target once at
//! registration and then forwards to it forever, so a label keeps proxying to
//! whatever process later takes that port — here every request asks the
//! [`TargetGate`] again.
//!
//! What this crate does NOT do: it is not wired into the window yet, it never
//! resolves DNS (a label is matched, never looked up), and it holds no policy
//! of its own about which ports may be labeled — that is the gate's, injected
//! by whoever knows the port scan.

pub mod guard;
pub mod headers;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HOST, HeaderValue};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode, Uri};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use url::Url;
use zerocode_core::localhost_label::{
    LabelRegistry, LabeledRoute, RegisterError, RouteSpec, connectable_loopback_host,
    label_from_host_header,
};

/// How long a connection may take to finish sending its header block.
///
/// Node gives this away free (`headersTimeout`, 60s by default) and the
/// original therefore never writes it down; hyper's own default resolves to
/// no timeout at all unless a timer is installed, which turns one stalled tab
/// into a connection this process holds forever.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the hop waits for the dev server to accept a connection. A dev
/// server that is starting up should answer late, not never.
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The body this proxy answers with: either the upstream's own stream, passed
/// through, or a short sentence of our own.
pub type ProxyBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

/// Whether a target may be proxied to.
///
/// Injected rather than decided here: the only honest answer comes from the
/// port scan, which lives in the shell. The original asks this question once,
/// at registration, and exempts every loopback address from it — two lines
/// below a comment explaining that an unguarded proxy is an open door. Here
/// it is asked at registration AND on every request, and loopback is a
/// precondition rather than an exemption.
pub trait TargetGate: Send + Sync + 'static {
    fn allows(&self, target: &Url) -> bool;
}

/// A gate that knows a fixed set of host-and-port pairs — the shape the port
/// scan will fill in. Useful on its own for a window that has not scanned yet:
/// an empty set refuses everything, which is the safe direction.
#[derive(Debug, Default)]
pub struct AllowedPorts {
    allowed: Mutex<Vec<(String, u16)>>,
}

impl AllowedPorts {
    pub fn new(ports: impl IntoIterator<Item = (String, u16)>) -> Self {
        Self {
            allowed: Mutex::new(ports.into_iter().collect()),
        }
    }

    /// Replace what the gate knows — the scan answering again.
    pub fn replace(&self, ports: impl IntoIterator<Item = (String, u16)>) {
        let mut held = self.allowed.lock().expect("allowed ports lock");
        *held = ports.into_iter().collect();
    }
}

impl TargetGate for AllowedPorts {
    fn allows(&self, target: &Url) -> bool {
        let (Some(host), Some(port)) = (target.host_str(), target.port()) else {
            return false;
        };
        let host = zerocode_core::localhost_label::normalize_loopback_hostname(host);
        self.allowed
            .lock()
            .expect("allowed ports lock")
            .iter()
            .any(|(known_host, known_port)| {
                *known_port == port
                    && zerocode_core::localhost_label::normalize_loopback_hostname(known_host)
                        == host
            })
    }
}

/// Why a route could not be registered.
#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("{0}")]
    Registry(#[from] RegisterError),
    #[error("the label proxy is not listening yet")]
    NotListening,
    #[error("that address is not a workspace port this window scanned")]
    TargetRefused,
}

/// The registry, the gate, and the port the server bound.
#[derive(Clone)]
pub struct ProxyState {
    inner: Arc<Inner>,
}

struct Inner {
    registry: Mutex<LabelRegistry>,
    gate: Arc<dyn TargetGate>,
    port: OnceLock<u16>,
}

impl ProxyState {
    pub fn new(gate: Arc<dyn TargetGate>) -> Self {
        Self {
            inner: Arc::new(Inner {
                registry: Mutex::new(LabelRegistry::new()),
                gate,
                port: OnceLock::new(),
            }),
        }
    }

    /// The port [`serve`] bound, once it has.
    pub fn port(&self) -> Option<u16> {
        self.inner.port.get().copied()
    }

    /// Name a target, and hand back the address to open.
    ///
    /// The gate is asked before the registry, so a refused target never takes
    /// a label (`ipc/localhost-worktree-labels.ts:21-25`, whose order this
    /// keeps even though its verdict differs).
    pub fn register(&self, spec: &RouteSpec<'_>) -> Result<LabeledRoute, RouteError> {
        let port = self.port().ok_or(RouteError::NotListening)?;
        let target = zerocode_core::localhost_label::parse_proxy_target(spec.target_url)
            .map_err(|error| RouteError::Registry(RegisterError::Target(error)))?;
        if !self.inner.gate.allows(&target) {
            return Err(RouteError::TargetRefused);
        }
        let mut registry = self.inner.registry.lock().expect("label registry lock");
        let label = registry.register(spec)?;
        let url = zerocode_core::localhost_label::labeled_url(&label, &target, port);
        Ok(LabeledRoute {
            url: url.to_string(),
            label: label.to_string(),
        })
    }

    /// Retire every route an owner holds, and say how many there were.
    ///
    /// The label is remembered as retired rather than released, so the same
    /// name is never minted for somebody else while this process lives.
    pub fn retire_owner(&self, owner: &str) -> usize {
        self.inner
            .registry
            .lock()
            .expect("label registry lock")
            .release_owner(owner)
            .len()
    }

    /// Where a label points, if it is one this process minted and still holds.
    fn target_of(&self, label: &str) -> Option<Url> {
        let registry = self.inner.registry.lock().expect("label registry lock");
        registry.route(label).map(|route| route.target.clone())
    }
}

/// Bind the proxy on the loopback and start answering.
///
/// Loopback is written here rather than taken as an argument: a proxy that
/// forwards to whatever a caller names is only safe while it cannot be
/// reached from another machine (`proxy.ts:81`). Port 0 asks the operating
/// system for a free one; the bound address comes back so the window can
/// build labeled URLs with it.
pub async fn serve(
    state: ProxyState,
    port: u16,
) -> std::io::Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener =
        TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)).await?;
    let addr = listener.local_addr()?;
    let _ = state.inner.port.set(addr.port());
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let served = state.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    let served = served.clone();
                    async move { Ok::<_, hyper::Error>(respond(&served, request).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .timer(TokioTimer::new())
                    .header_read_timeout(HEADER_READ_TIMEOUT)
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    });
    Ok((addr, task))
}

/// Answer one request: the whole rule set, with no socket of our own in it, so
/// every rule below can be asserted without a listener.
pub async fn respond(state: &ProxyState, request: Request<Incoming>) -> Response<ProxyBody> {
    if let Err(refusal) = guard::permitted_method(request.method()) {
        return refuse(refusal.status(), "이 주소는 그런 요청을 받지 않습니다");
    }
    let host = match guard::single_host(request.headers()) {
        Ok(host) => host.to_string(),
        Err(refusal) => return refuse(refusal.status(), "요청의 호스트를 읽을 수 없습니다"),
    };
    let path_and_query = match guard::forwardable_target(request.uri()) {
        Ok(target) => target,
        Err(refusal) => return refuse(refusal.status(), "요청의 경로를 받아들일 수 없습니다"),
    };
    // Nothing of the caller's host is echoed back: an unknown label learns only
    // that it is unknown.
    let Some(label) = label_from_host_header(Some(&host)) else {
        return refuse(StatusCode::NOT_FOUND, "이 창이 붙인 이름이 아닙니다");
    };
    let Some(target) = state.target_of(&label) else {
        return refuse(StatusCode::NOT_FOUND, "이 창이 붙인 이름이 아닙니다");
    };
    // Asked again, every time: a port the window no longer trusts stops being
    // proxied the moment it stops being trusted.
    if !state.inner.gate.allows(&target) {
        return refuse(
            StatusCode::BAD_GATEWAY,
            "그 포트는 더 이상 이 창의 것이 아닙니다",
        );
    }
    // The `host:port` the dev server knows itself by — the REGISTERED
    // spelling, not the address the socket goes to, so a server that checks
    // its own `Host` still recognises itself (`proxy.ts:244-248`).
    let (Some(host), Some(port)) = (target.host_str(), target.port()) else {
        return refuse(StatusCode::BAD_GATEWAY, "그 주소로는 연결할 수 없습니다");
    };
    let authority = format!("{host}:{port}");

    let upgrading = request.headers().get(hyper::header::UPGRADE).cloned();
    let mut sent = headers::forwardable_request(request.headers());
    match HeaderValue::from_str(&authority) {
        Ok(value) => {
            sent.insert(HOST, value);
        }
        Err(_) => return refuse(StatusCode::BAD_GATEWAY, "그 주소로는 연결할 수 없습니다"),
    }
    if upgrading.is_some() {
        headers::restore_upgrade_handshake(&mut sent, request.headers());
    }

    // Path and query alone — hyper writes the request line from the shape of
    // this URI, and one carrying an authority would go upstream in
    // absolute-form. Only a proxy the dev server knows it is talking to may
    // speak that form; this hop is meant to be invisible, and where the bytes
    // go is decided by the socket below, never by the line.
    let Ok(uri) = path_and_query.parse::<Uri>() else {
        return refuse(StatusCode::BAD_GATEWAY, "그 주소로는 연결할 수 없습니다");
    };

    // The client's own body is handed over as a stream. Collecting it here
    // would make the proxy the place a large upload lives.
    let (parts, body) = request.into_parts();
    let mut upstream_request = Request::builder()
        .method(parts.method.clone())
        .uri(uri)
        .body(body)
        .expect("a validated method, uri and the client's own body");
    *upstream_request.headers_mut() = sent;

    // A wildcard bind is not a connectable address, and an IPv6 host arrives
    // bracketed from the URL while a socket wants it bare — both answers come
    // from the core vocabulary rather than being spelled again here.
    let host_for_socket = zerocode_core::localhost_label::normalize_loopback_hostname(
        connectable_loopback_host(host),
    );
    let stream = match tokio::time::timeout(
        UPSTREAM_CONNECT_TIMEOUT,
        TcpStream::connect((host_for_socket.as_str(), port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        // The bodies name neither the label nor the operating system's word
        // for the failure: this page is reachable from any tab on the machine.
        _ => return refuse(StatusCode::BAD_GATEWAY, "개발 서버가 응답하지 않습니다"),
    };

    let Ok((mut sender, connection)) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
    else {
        return refuse(StatusCode::BAD_GATEWAY, "개발 서버가 응답하지 않습니다");
    };
    // The connection task must keep running while the body streams, and it is
    // what carries the upgrade through when one is negotiated.
    let upstream_connection = tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });

    let Ok(upstream) = sender.send_request(upstream_request).await else {
        upstream_connection.abort();
        return refuse(StatusCode::BAD_GATEWAY, "개발 서버가 응답하지 않습니다");
    };

    if upgrading.is_some() {
        return bridge_upgrade(request_parts_back(parts), upstream, upstream_connection).await;
    }

    let status = upstream.status();
    let kept = headers::forwardable_response(upstream.headers());
    let mut answer = Response::builder()
        .status(status)
        .body(upstream.into_body().boxed())
        .expect("a status and body that already exist");
    *answer.headers_mut() = kept;
    answer
}

/// Rebuild a request head we still need after taking its body apart — the
/// upgrade path needs the original request to hand hyper for the takeover.
fn request_parts_back(parts: hyper::http::request::Parts) -> Request<Empty<Bytes>> {
    Request::from_parts(parts, Empty::new())
}

/// Join the two sockets once, and only once, the dev server has agreed.
///
/// The original never reads the upstream's answer at all — it opens a raw TCP
/// connection, writes a request line it assembled by hand, and pipes the two
/// sockets together whatever comes back, which makes the upgrade path a way
/// to write arbitrary bytes at any loopback port. Here the bridge is built
/// only after a `101`, and any other answer is relayed as an ordinary
/// response.
async fn bridge_upgrade(
    client_request: Request<Empty<Bytes>>,
    upstream: Response<Incoming>,
    upstream_connection: tokio::task::JoinHandle<()>,
) -> Response<ProxyBody> {
    if upstream.status() != StatusCode::SWITCHING_PROTOCOLS {
        upstream_connection.abort();
        let status = upstream.status();
        let kept = headers::forwardable_response(upstream.headers());
        let mut answer = Response::builder()
            .status(status)
            .body(upstream.into_body().boxed())
            .expect("a status and body that already exist");
        *answer.headers_mut() = kept;
        return answer;
    }
    let status = upstream.status();
    let mut kept = headers::forwardable_response(upstream.headers());
    // The handshake fields are hop-by-hop and were filtered out; the browser
    // needs exactly the ones the dev server just agreed to, including the
    // accept key we deliberately never compute ourselves.
    headers::restore_upgrade_handshake(&mut kept, upstream.headers());

    let mut client_request = client_request;
    let downstream = hyper::upgrade::on(&mut client_request);
    let upstream_upgrade = hyper::upgrade::on(upstream);
    tokio::spawn(async move {
        let (Ok(client), Ok(server)) = tokio::join!(downstream, upstream_upgrade) else {
            upstream_connection.abort();
            return;
        };
        let mut client = TokioIo::new(client);
        let mut server = TokioIo::new(server);
        let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
        upstream_connection.abort();
    });

    let mut answer = Response::builder()
        .status(status)
        .body(
            Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("an empty body for a protocol switch");
    *answer.headers_mut() = kept;
    answer
}

/// A short answer of our own, in the window's voice.
fn refuse(status: StatusCode, said: &str) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .header(
            hyper::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )
        .body(
            Full::new(Bytes::from(said.to_string()))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("a status and a short sentence")
}
