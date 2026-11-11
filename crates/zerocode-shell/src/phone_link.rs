//! The phone link, stage 1: a phone sees this window's panes and is told when
//! one needs the person — over the person's own tailnet, with no server of
//! ours anywhere.
//!
//! ```text
//! phone ──https──▶ Tailscale Serve (tailnet only, :10443)
//!                    └──http──▶ 127.0.0.1:<port>  phone listener (this module)
//!                                 └── reads the panes the board reads
//! window ──Web Push (RFC 8291/8292)──▶ Apple / Google push ──▶ phone
//! ```
//!
//! Off until a person turns it on in Settings › Remote link; while off no port
//! is open. It needs Tailscale: without it the pane shows the state and the
//! steps, and nothing else happens. Read-only — answering from the phone is
//! stage 2.

mod book;
mod server;
mod tailscale;
mod view;
mod web_push;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager as _};
use zerocode_core::notify::Ring;
use zerocode_shell_state::AppState;
use zerocode_shell_state::credential_store::VerifiedSecretStore;
use zeroize::Zeroizing;

use crate::ShellStateExt as _;
use book::{Book, DeviceView};
use server::{Core, PaneView, StatusView, Window};
use tailscale::{Cli, Exposure, Facts, Installed, Refusal, ServeConfig, Tailnet, Then};
use view::{Gate, Notice, RawPane, Verdict};

const CREDENTIAL_SERVICE: &str = "dev.zerocode.shell.phone-link";
const VAPID_ACCOUNT: &str = "vapid-p256";
/// How often an enabled link re-reads the Serve configuration, so Funnel, a
/// second road to the listener or a hook-bridge mapping turned on behind its
/// back closes the listener and takes the window's mapping down.
const RECHECK: Duration = Duration::from_secs(60);
/// Connections one listener serves at once.
const MAX_CONNECTIONS: usize = 32;
const PUSH_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the accept loop rests after a failed accept, so a process out of
/// file handles does not spin a core asking again.
const ACCEPT_REST: Duration = Duration::from_millis(100);
/// Minute looks in a row that may fail to read Tailscale before the listener
/// closes: one is a hiccup, two is a link nobody can vouch for.
const MISSED_LOOKS_BEFORE_CLOSING: u32 = 2;

/// `count` bytes from the operating system's generator.
pub(crate) fn random_bytes<const N: usize>() -> Option<[u8; N]> {
    use rand::TryRng as _;
    let mut bytes = [0u8; N];
    rand::rngs::SysRng.try_fill_bytes(&mut bytes).ok()?;
    Some(bytes)
}

struct Link {
    app: AppHandle,
    core: Arc<Core>,
    /// The listener's loopback port, served or only held.
    seat: Mutex<Seat<tauri::async_runtime::JoinHandle<()>>>,
    gate: Mutex<Gate>,
    /// The VAPID signing key, read from the OS credential store once.
    vapid: Mutex<Option<p256::SecretKey>>,
    /// What the last look at Tailscale found.
    last: Mutex<Look>,
    /// Bumped by every on/off, so a recheck that started before it stands down.
    epoch: AtomicU64,
    /// Held by turning on, turning off, the minute's look, closing and a
    /// start's clean-up, so no two of them change the listener or the
    /// mappings at once.
    turn: Mutex<()>,
    /// The window is closing; a second close does not wait again.
    closing: AtomicBool,
}

#[derive(Debug, Clone, Default)]
struct Look {
    tailnet: Tailnet,
    exposure: Option<Exposure>,
    problem: Option<String>,
}

static LINK: OnceLock<Link> = OnceLock::new();

fn link() -> Option<&'static Link> {
    LINK.get()
}

fn hook_port() -> Option<u16> {
    crate::hooks::bridge().map(|bridge| bridge.port)
}

fn as_cli(cli: Option<&Installed>) -> Option<&dyn Cli> {
    cli.map(|cli| cli as &dyn Cli)
}

/// `serve --https=<port> off` for each port, stopping at the first refusal.
fn take_down(cli: &dyn Cli, ports: &[u16]) -> Result<(), String> {
    for https in ports {
        cli.run(&tailscale::serve_off_args(*https))?;
    }
    Ok(())
}

/// The mapping the book says the window made: HTTPS port, loopback port.
fn pair_of(book: &Book) -> (Option<u16>, Option<u16>) {
    (book.https_port, book.port)
}

/// Take down every Serve mapping of the window's. With nothing to take down
/// nothing is asked. With something to take down, a CLI, status, name or
/// Serve configuration that cannot be read is an error, never "done" — the
/// book keeps naming the mapping until it is known gone.
fn take_down_own(
    cli: Option<&dyn Cli>,
    remembered: (Option<u16>, Option<u16>),
    listening: Option<u16>,
) -> Result<(), String> {
    if remembered.0.is_none() && listening.is_none() {
        return Ok(());
    }
    let cli = cli.ok_or("tailscale")?;
    let name = cli.status()?.dns_name.ok_or("tailscale-name")?;
    let serve = cli.serve()?;
    take_down(
        cli,
        &tailscale::own_mappings(&serve, &name, remembered, listening),
    )
}

/// Forget the pair a take-down was for once its mappings are known gone,
/// and only while the book still names that pair: a press that wrote a new
/// one in between keeps it.
fn forget_when_gone(
    book: &mut Book,
    taken: (Option<u16>, Option<u16>),
    outcome: &Result<(), String>,
) {
    if outcome.is_ok() && pair_of(book) == taken {
        book.https_port = None;
        book.port = None;
    }
}

fn book_of(core: &Core) -> Book {
    core.book
        .lock()
        .map(|book| book.clone())
        .unwrap_or_default()
}

fn write_book_of(core: &Core, change: impl FnOnce(&mut Book)) {
    {
        let Ok(mut book) = core.book.lock() else {
            return;
        };
        change(&mut book);
    }
    core.save();
}

/// The book's half of turning off: the window's mappings taken down, the
/// link marked off, and the pair forgotten only when they went.
fn turn_off(core: &Core, cli: Option<&dyn Cli>, listening: Option<u16>) -> Result<(), String> {
    let pair = pair_of(&book_of(core));
    let outcome = take_down_own(cli, pair, listening);
    write_book_of(core, |book| {
        book.enabled = false;
        forget_when_gone(book, pair, &outcome);
    });
    outcome
}

/// A start with the link off still owns the mapping its book names — an off
/// whose take-down failed, a press that failed after mapping — and takes it
/// down without opening anything. `None` when the book names none.
fn sweep_off(core: &Core, cli: Option<&dyn Cli>, held: Option<u16>) -> Option<Result<(), String>> {
    let book = book_of(core);
    if book.enabled || book.https_port.is_none() {
        return None;
    }
    let pair = pair_of(&book);
    let outcome = take_down_own(cli, pair, held);
    write_book_of(core, |book| {
        if !book.enabled {
            forget_when_gone(book, pair, &outcome);
        }
    });
    Some(outcome)
}

/// The window is closing: the mapping it made comes down with it, so no
/// tailnet name points at a loopback port somebody else may take before the
/// next start. The book keeps the link on and its port, and the next start
/// maps it again. The whole close waits [`tailscale::EXIT_CLI_TIMEOUT`] at
/// most: the CLI runs on a thread of its own, and a close that outlives it
/// is finished by the next start.
pub(crate) fn shutdown() {
    let Some(link) = link() else {
        return;
    };
    if link.held_port().is_none() || link.closing.swap(true, Ordering::SeqCst) {
        return;
    }
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        link.close();
        let _ = done.send(());
    });
    let _ = finished.recv_timeout(tailscale::EXIT_CLI_TIMEOUT);
}

/// Read the book and, when a person had turned the link on, bring it back up
/// off the boot path; when they had turned it off and the book still names a
/// mapping, take that down.
pub(crate) fn boot(app: &AppHandle, local_data_root: &std::path::Path) {
    let path = book::book_path(local_data_root);
    let held = book::load(&path);
    let start = start_of(&held);
    // The loopback port the book's mapping points at is bound first, before
    // the hook bridge binds, so nothing this process starts is handed it.
    let reserved = held
        .https_port
        .and(held.port)
        .and_then(|port| std::net::TcpListener::bind((tailscale::LOOPBACK, port)).ok());
    let core = Arc::new(Core::new(Some(path), held));
    let made = LINK.set(Link {
        app: app.clone(),
        core,
        seat: Mutex::new(Seat::new(reserved)),
        gate: Mutex::new(Gate::default()),
        vapid: Mutex::new(None),
        last: Mutex::new(Look::default()),
        epoch: AtomicU64::new(0),
        turn: Mutex::new(()),
        closing: AtomicBool::new(false),
    });
    if made.is_err() || start == Start::Nothing {
        return;
    }
    std::thread::spawn(move || {
        let Some(link) = link() else {
            return;
        };
        if start == Start::Sweep {
            link.sweep();
        } else if let Err(word) = link.bring_up(false)
            && let Ok(mut look) = link.last.lock()
        {
            look.problem = Some(word);
        }
    });
}

/// What a start does with the book it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    Nothing,
    /// A person had turned the link on.
    BringUp,
    /// The link is off, and the book still names a mapping of the window's.
    Sweep,
}

fn start_of(book: &Book) -> Start {
    if book.enabled {
        Start::BringUp
    } else if book.https_port.is_some() {
        Start::Sweep
    } else {
        Start::Nothing
    }
}

/// A task that can be told to stop: the accept loop, or a test's stand-in.
trait Stop {
    fn stop(self);
}

impl Stop for tauri::async_runtime::JoinHandle<()> {
    fn stop(self) {
        self.abort();
    }
}

/// The listener's loopback port, served or only held. Holding keeps it bound
/// and accepts nothing: no byte goes out, and no other program can take it.
/// The port is let go only by [`Seat::release`], once the window knows no
/// mapping of its points there — a mapping that may stand never points at a
/// port somebody else can bind.
struct Seat<T: Stop> {
    serving: Option<Serving<T>>,
    held: Option<std::net::TcpListener>,
}

struct Serving<T> {
    port: u16,
    task: T,
    /// The socket the task accepts on, kept so stopping the task keeps the port.
    socket: std::net::TcpListener,
}

impl<T: Stop> Seat<T> {
    fn new(held: Option<std::net::TcpListener>) -> Self {
        Self {
            serving: None,
            held,
        }
    }

    fn serving_port(&self) -> Option<u16> {
        self.serving.as_ref().map(|serving| serving.port)
    }

    /// The port the window holds, served or not.
    fn port(&self) -> Option<u16> {
        self.serving_port().or_else(|| {
            self.held
                .as_ref()
                .and_then(|socket| socket.local_addr().ok())
                .map(|address| address.port())
        })
    }

    fn take_held(&mut self) -> Option<std::net::TcpListener> {
        self.held.take()
    }

    fn hold(&mut self, socket: std::net::TcpListener) {
        self.held = Some(socket);
    }

    fn serve(&mut self, port: u16, task: T, socket: std::net::TcpListener) {
        self.serving = Some(Serving { port, task, socket });
    }

    /// Stop accepting and keep the port.
    fn pause(&mut self) {
        if let Some(serving) = self.serving.take() {
            serving.task.stop();
            self.held = Some(serving.socket);
        }
    }

    /// Let the port go: no mapping of the window's points at it any more.
    fn release(&mut self) {
        self.pause();
        self.held = None;
    }
}

/// One more minute look that could not read Tailscale; whether the listener
/// now stops accepting.
fn missed_look(missed: &mut u32) -> bool {
    *missed += 1;
    *missed >= MISSED_LOOKS_BEFORE_CLOSING
}

/// The listener's socket: one already serving, or a fresh one that accepts
/// nothing until the judgment says to serve.
enum Socket {
    Serving(u16),
    Bound(std::net::TcpListener, u16),
}

impl Socket {
    fn port(&self) -> u16 {
        match self {
            Self::Serving(port) | Self::Bound(_, port) => *port,
        }
    }
}

/// The pane's whole picture.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkStatus {
    enabled: bool,
    listening: bool,
    tailscale: Tailnet,
    #[serde(skip_serializing_if = "Option::is_none")]
    exposure: Option<Exposure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    push_ready: bool,
    devices: Vec<DeviceView>,
    /// The HTTPS ports the link may use, for the pane's words about them.
    phone_ports: [u16; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    code_expires_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairingCode {
    code: String,
    url: String,
    expires_ms: i64,
    /// The QR of `url`, as an SVG drawn in `currentColor`.
    qr_svg: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TestPush {
    sent: usize,
    gone: usize,
    failed: usize,
}

impl Link {
    fn book(&self) -> Book {
        book_of(&self.core)
    }

    fn write_book(&self, change: impl FnOnce(&mut Book)) {
        write_book_of(&self.core, change);
    }

    fn turn(&self) -> MutexGuard<'_, ()> {
        self.turn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn seat(&self) -> MutexGuard<'_, Seat<tauri::async_runtime::JoinHandle<()>>> {
        self.seat.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The port the listener accepts on, while it does.
    fn listening_port(&self) -> Option<u16> {
        self.seat().serving_port()
    }

    /// The port the window holds, accepting or not.
    fn held_port(&self) -> Option<u16> {
        self.seat().port()
    }

    fn status(&self) -> LinkStatus {
        let book = self.book();
        let look = self
            .last
            .lock()
            .map(|look| look.clone())
            .unwrap_or_default();
        let listening = self.listening_port().is_some();
        let url = match (&look.tailnet.dns_name, book.https_port, look.exposure) {
            (Some(name), Some(port), Some(Exposure::Ours)) if listening => {
                Some(format!("https://{name}:{port}/"))
            }
            _ => None,
        };
        let now = crate::usage_runtime::epoch_ms_now();
        LinkStatus {
            enabled: book.enabled,
            listening,
            tailscale: look.tailnet,
            exposure: look.exposure,
            problem: look.problem,
            url,
            push_ready: self.vapid.lock().map(|key| key.is_some()).unwrap_or(false),
            devices: book.views(),
            phone_ports: tailscale::HTTPS_PORTS,
            code_expires_ms: self
                .core
                .pairing
                .lock()
                .ok()
                .and_then(|pairing| pairing.standing_until(now)),
        }
    }

    /// Look at Tailscale without changing anything.
    fn look(&self) -> Look {
        let cli = Installed::find();
        self.read(as_cli(cli.as_ref())).0
    }

    /// One read of Tailscale — status, then Serve — that the pane and a
    /// judgment share: what the pane shows, and this machine's name and the
    /// Serve configuration, or why they could not be read.
    fn read(&self, cli: Option<&dyn Cli>) -> (Look, Result<(String, ServeConfig), String>) {
        let Some(cli) = cli else {
            return (Look::default(), Err("tailscale".into()));
        };
        let tailnet = match cli.status() {
            Ok(tailnet) => tailnet,
            Err(error) => {
                let look = Look {
                    tailnet: Tailnet::unread(),
                    ..Look::default()
                };
                return (look, Err(error));
            }
        };
        let mut look = Look {
            tailnet,
            exposure: None,
            problem: None,
        };
        let (Some(name), true) = (look.tailnet.dns_name.clone(), look.tailnet.running) else {
            return (look, Err("tailscale-stopped".into()));
        };
        match cli.serve() {
            Ok(serve) => {
                let https = self.book().https_port.unwrap_or(tailscale::HTTPS_PORTS[0]);
                look.exposure = Some(tailscale::exposure_of(
                    &serve,
                    &name,
                    https,
                    self.listening_port(),
                    hook_port(),
                ));
                (look, Ok((name, serve)))
            }
            Err(error) => {
                look.problem = Some(error.clone());
                (look, Err(error))
            }
        }
    }

    fn remember(&self, look: Look) {
        if let Ok(mut held) = self.last.lock() {
            *held = look.clone();
        }
        if let Ok(mut hosts) = self.core.hosts.lock() {
            hosts.dns_name = look.tailnet.dns_name.clone();
        }
        if let Ok(mut owner) = self.core.owner.lock() {
            owner.clone_from(&look.tailnet.owner_login);
        }
    }

    /// Turn the link on: listener, then the Serve mapping. `asked` is a
    /// person's press; a start serves only behind the phone port it had.
    /// Whatever stops it stops the listener accepting again.
    fn bring_up(&'static self, asked: bool) -> Result<(), String> {
        let outcome = {
            let _turn = self.turn();
            let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let outcome = self.bring_up_at(asked);
            match outcome {
                Ok(()) => self.watch(epoch),
                Err(_) => self.stop_listener(),
            }
            outcome
        };
        // The keychain and the pane's look need no lock.
        if outcome.is_ok() {
            self.load_vapid(true);
            let look = self.look();
            self.remember(look);
        }
        outcome
    }

    fn bring_up_at(&'static self, asked: bool) -> Result<(), String> {
        let cli = Installed::find().ok_or("tailscale")?;
        let tailnet = cli.status().unwrap_or_else(|_| Tailnet::unread());
        let refused_before = |word: &str| {
            self.remember(Look {
                tailnet: tailnet.clone(),
                ..Look::default()
            });
            Err(word.to_string())
        };
        if !tailnet.running {
            return refused_before("tailscale-stopped");
        }
        let Some(name) = tailnet.dns_name.clone() else {
            return refused_before("tailscale-name");
        };
        if !tailnet.https_ready {
            return refused_before("tailscale-https");
        }
        let book = self.book();
        let socket = self.bind(book.port)?;
        let listener = socket.port();
        let https = match self.judge(&cli, &tailnet, &name, &book, listener, asked) {
            Ok(https) => https,
            Err(word) => {
                // A mapping of the window's may still point here.
                self.hold(socket);
                return Err(word);
            }
        };
        self.serve(socket)?;
        self.write_book(|book| {
            book.enabled = true;
            book.port = Some(listener);
            book.https_port = Some(https);
        });
        Ok(())
    }

    /// Read Serve, take down what the judgment says, and map the phone port
    /// when it says to: the HTTPS port to serve behind, or why not.
    fn judge(
        &self,
        cli: &Installed,
        tailnet: &Tailnet,
        name: &str,
        book: &Book,
        listener: u16,
        asked: bool,
    ) -> Result<u16, String> {
        let serve = cli.serve()?;
        let facts = Facts {
            config: &serve,
            dns_name: name,
            remembered: pair_of(book),
            listener,
            hook: hook_port(),
            asked,
        };
        let mut decision = tailscale::decide(&facts);
        take_down(cli, &decision.remove)?;
        if !decision.remove.is_empty() && matches!(decision.then, Then::Serve { .. }) {
            // What the CLI left is read again rather than assumed.
            let left = cli.serve()?;
            decision = tailscale::decide(&Facts {
                config: &left,
                ..facts
            });
            if !decision.remove.is_empty() {
                decision.then = Then::Refuse(Refusal::Shared);
            }
        }
        match decision.then {
            Then::Serve { https, map } => {
                if map {
                    // The book names the mapping before it exists, so one a
                    // failure leaves behind is still the window's to take down.
                    self.write_book(|book| {
                        book.port = Some(listener);
                        book.https_port = Some(https);
                    });
                    cli.run(&tailscale::serve_on_args(https, listener))?;
                }
                Ok(https)
            }
            Then::Refuse(refusal) => {
                self.remember(Look {
                    tailnet: tailnet.clone(),
                    exposure: Some(refusal.exposure()),
                    problem: None,
                });
                Err(refusal.word().into())
            }
        }
    }

    /// The close's half of [`shutdown`]: the window's mappings, then the
    /// listener. The process ends right after, port and all.
    fn close(&self) {
        let _turn = self.turn();
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let cli = Installed::find_within(tailscale::EXIT_CLI_TIMEOUT);
        let _ = take_down_own(
            as_cli(cli.as_ref()),
            pair_of(&self.book()),
            self.held_port(),
        );
        self.stop_listener();
    }

    /// A start with the link off: [`sweep_off`], and the held port let go
    /// once nothing of the window's points at it.
    fn sweep(&self) {
        let _turn = self.turn();
        let cli = Installed::find();
        if let Some(Ok(())) = sweep_off(&self.core, as_cli(cli.as_ref()), self.held_port()) {
            self.release_port();
        }
    }

    fn release_port(&self) {
        self.seat().release();
    }

    /// Turn the link off: the Serve mappings this window made, then the
    /// port. A mapping that could not be taken down stays named in the book
    /// and its port stays held, and the next start takes it down.
    fn bring_down(&self) -> Result<(), String> {
        let outcome = {
            let _turn = self.turn();
            self.epoch.fetch_add(1, Ordering::SeqCst);
            let cli = Installed::find();
            let outcome = turn_off(&self.core, as_cli(cli.as_ref()), self.held_port());
            if outcome.is_ok() {
                self.release_port();
            } else {
                self.stop_listener();
            }
            outcome
        };
        if let Ok(mut pairing) = self.core.pairing.lock() {
            pairing.clear();
        }
        let look = self.look();
        self.remember(look);
        outcome
    }

    /// The socket a turn-on judges with: the serving one, the port the
    /// window holds, the port the book remembers, or any.
    fn bind(&self, remembered: Option<u16>) -> Result<Socket, String> {
        if let Some(port) = self.listening_port() {
            return Ok(Socket::Serving(port));
        }
        let held = self.seat().take_held();
        let bound = match held {
            Some(socket) => socket,
            None => remembered
                .and_then(|port| std::net::TcpListener::bind((tailscale::LOOPBACK, port)).ok())
                .map_or_else(|| std::net::TcpListener::bind((tailscale::LOOPBACK, 0)), Ok)
                .map_err(|error| format!("listen: {error}"))?,
        };
        let port = bound
            .local_addr()
            .map_err(|error| format!("listen: {error}"))?
            .port();
        if let Err(error) = bound.set_nonblocking(true) {
            self.seat().hold(bound);
            return Err(format!("listen: {error}"));
        }
        Ok(Socket::Bound(bound, port))
    }

    /// Keep a socket a turn-on bound but did not serve.
    fn hold(&self, socket: Socket) {
        if let Socket::Bound(socket, _) = socket {
            self.seat().hold(socket);
        }
    }

    /// Start accepting on a socket the judgment said to serve.
    fn serve(&'static self, socket: Socket) -> Result<(), String> {
        let Socket::Bound(bound, port) = socket else {
            return Ok(());
        };
        let kept = match bound.try_clone() {
            Ok(kept) => kept,
            Err(error) => {
                self.seat().hold(bound);
                return Err(format!("listen: {error}"));
            }
        };
        if let Ok(mut hosts) = self.core.hosts.lock() {
            hosts.listener_port = port;
        }
        let core = Arc::clone(&self.core);
        let window: Arc<dyn Window> = Arc::new(AppWindow {
            app: self.app.clone(),
        });
        let task = tauri::async_runtime::spawn(async move {
            let Ok(listener) = tokio::net::TcpListener::from_std(bound) else {
                return;
            };
            let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    tokio::time::sleep(ACCEPT_REST).await;
                    continue;
                };
                let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let core = Arc::clone(&core);
                let window = Arc::clone(&window);
                tauri::async_runtime::spawn(async move {
                    server::serve_connection(stream, core, window).await;
                    drop(permit);
                });
            }
        });
        self.seat().serve(port, task, kept);
        Ok(())
    }

    /// Stop accepting; the port stays the window's.
    fn stop_listener(&self) {
        self.seat().pause();
    }

    /// Re-read Tailscale every minute while on, once for the pane and the
    /// judgment both, and judge it as a start would: a refusal takes the
    /// window's mappings down and stops the listener. Two looks in a row
    /// that cannot read Tailscale stop it accepting too, port still held,
    /// and the next look that can judges it as a start would and serves it
    /// again.
    fn watch(&'static self, epoch: u64) {
        std::thread::spawn(move || {
            let mut missed = 0;
            loop {
                std::thread::sleep(RECHECK);
                let _turn = self.turn();
                if self.epoch.load(Ordering::SeqCst) != epoch {
                    return;
                }
                let Some(listener) = self.listening_port() else {
                    match self.bring_up_at(false) {
                        Ok(()) => {
                            missed = 0;
                            let look = self.look();
                            self.remember(look);
                        }
                        Err(word) => {
                            self.stop_listener();
                            if let Ok(mut look) = self.last.lock() {
                                look.problem = Some(word);
                            }
                        }
                    }
                    continue;
                };
                let cli = Installed::find();
                let (mut look, read) = self.read(as_cli(cli.as_ref()));
                let refused = match (read, as_cli(cli.as_ref())) {
                    (Ok((name, serve)), Some(cli)) => {
                        missed = 0;
                        let refusal = self.refusal_of(cli, &name, &serve, listener);
                        if let Some(refusal) = refusal {
                            look.exposure = Some(refusal.exposure());
                            look.problem = Some(refusal.word().into());
                        }
                        refusal.is_some()
                    }
                    (Ok(_), None) => false,
                    (Err(error), _) => {
                        if missed_look(&mut missed) {
                            self.stop_listener();
                            look.problem = Some(error);
                        }
                        false
                    }
                };
                self.remember(look);
                if refused {
                    return;
                }
            }
        });
    }

    /// Judge the running link on one read; when refused, take its mappings
    /// down and stop the listener.
    fn refusal_of(
        &self,
        cli: &dyn Cli,
        name: &str,
        serve: &ServeConfig,
        listener: u16,
    ) -> Option<Refusal> {
        let book = self.book();
        let decision = tailscale::decide(&Facts {
            config: serve,
            dns_name: name,
            remembered: pair_of(&book),
            listener,
            hook: hook_port(),
            asked: false,
        });
        let Then::Refuse(refusal) = decision.then else {
            return None;
        };
        let _ = take_down(cli, &decision.remove);
        self.stop_listener();
        Some(refusal)
    }

    /// Read the VAPID key, making one when `create` and none is kept.
    fn load_vapid(&self, create: bool) {
        if self.vapid.lock().map(|key| key.is_some()).unwrap_or(true) {
            return;
        }
        let secrets = VerifiedSecretStore::new(CREDENTIAL_SERVICE);
        let read = secrets
            .read(VAPID_ACCOUNT)
            .ok()
            .and_then(|text| web_push::decode(&text))
            .and_then(|bytes| p256::SecretKey::from_slice(&bytes).ok());
        let key = match read {
            Some(key) => Some(key),
            None if create => web_push::random_secret_key().filter(|key| {
                let text = Zeroizing::new(web_push::encode(key.to_bytes().as_slice()));
                secrets.write(VAPID_ACCOUNT, text).is_ok()
            }),
            None => None,
        };
        let public = key
            .as_ref()
            .map(|key| web_push::encode(&web_push::public_bytes(&key.public_key())));
        if let Ok(mut held) = self.vapid.lock() {
            *held = key;
        }
        if let Ok(mut held) = self.core.vapid_public.lock() {
            *held = public;
        }
    }

    /// Send one notice to every subscribed phone; forget the ones the push
    /// service calls gone.
    async fn deliver(&self, notice: &Notice) -> TestPush {
        let mut outcome = TestPush {
            sent: 0,
            gone: 0,
            failed: 0,
        };
        let key = self.vapid.lock().ok().and_then(|key| key.clone());
        let Some(key) = key else {
            return outcome;
        };
        let targets = self.book().push_targets();
        if targets.is_empty() {
            return outcome;
        }
        let Ok(client) = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(PUSH_TIMEOUT)
            .build()
        else {
            outcome.failed = targets.len();
            return outcome;
        };
        let payload = serde_json::to_vec(notice).unwrap_or_default();
        let now_secs = u64::try_from(crate::usage_runtime::epoch_ms_now() / 1000).unwrap_or(0);
        for target in targets {
            let sealed = web_push::Receiver::from_keys(&target.p256dh, &target.auth)
                .ok()
                .and_then(|receiver| {
                    let sender = web_push::random_secret_key()?;
                    let salt = random_bytes::<16>()?;
                    web_push::encrypt(&payload, &receiver, &sender, salt).ok()
                });
            let authorization = web_push::vapid_authorization(
                &key,
                &target.endpoint,
                zerocode_core::skill::SKILLS_REPOSITORY_URL,
                now_secs,
            );
            let (Some(body), Some(authorization)) = (sealed, authorization) else {
                outcome.failed += 1;
                continue;
            };
            let answer = client
                .post(&target.endpoint)
                .header("TTL", web_push::PUSH_TTL_SECS.to_string())
                .header("Content-Encoding", "aes128gcm")
                .header("Content-Type", "application/octet-stream")
                .header("Urgency", "high")
                .header("Topic", notice.topic())
                .header("Authorization", authorization)
                .body(body)
                .send()
                .await;
            match answer.map(|answer| web_push::delivery_of(answer.status().as_u16())) {
                Ok(web_push::Delivery::Sent) => outcome.sent += 1,
                Ok(web_push::Delivery::Gone) => {
                    outcome.gone += 1;
                    self.write_book(|book| {
                        book.forget_push(&target.endpoint);
                    });
                }
                Ok(web_push::Delivery::Failed) | Err(_) => outcome.failed += 1,
            }
        }
        outcome
    }
}

/// The window, as the listener reads it.
struct AppWindow {
    app: AppHandle,
}

impl Window for AppWindow {
    fn now_ms(&self) -> i64 {
        crate::usage_runtime::epoch_ms_now()
    }

    fn status(&self) -> StatusView {
        StatusView {
            version: env!("CARGO_PKG_VERSION").to_string(),
            name: machine_name(),
        }
    }

    fn panes(&self) -> Vec<PaneView> {
        panes_of(&self.app)
    }

    fn paired(&self, name: &str, at_ms: i64) {
        use tauri::Emitter as _;
        let _ = self.app.emit_to(
            crate::MAIN_WINDOW_LABEL,
            PAIRED_EVENT,
            Paired {
                name: name.to_string(),
                at_ms,
            },
        );
    }
}

/// What the window hears when a phone pairs (`ui/shell-remote.js`).
const PAIRED_EVENT: &str = "phone-link:paired";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Paired {
    name: String,
    at_ms: i64,
}

fn panes_of(app: &AppHandle) -> Vec<PaneView> {
    let state = app.state::<AppState>();
    let agents = crate::cmd::board::pane_agents(app.state::<AppState>());
    let ledger = crate::orchestration::board_ledger_snapshot();
    // Where each pane stands, from the cwd sweep — the checkout's folder name
    // when the folder is inside one. Only its last segment leaves this file.
    let cwds = state.pane_cwds().clone();
    agents
        .iter()
        .filter_map(|agent| {
            let worktree = crate::pane_runtime::seated_worktree(&state, agent.term).or_else(|| {
                cwds.get(&agent.term).map(|cwd| {
                    crate::pane_cwd_runtime::checkout_of(cwd).unwrap_or_else(|| cwd.clone())
                })
            });
            let task = ledger
                .agents
                .iter()
                .find(|row| row.term == Some(agent.term))
                .map(|row| row.task_id.clone());
            view::project(&RawPane {
                term: agent.term,
                agent: agent.agent,
                state: &agent.state,
                at: agent.at,
                state_started_at: agent.state_started_at,
                worktree: worktree.as_deref(),
                task_id: task.as_deref(),
                approval: agent.approval.is_some(),
            })
        })
        .collect()
}

/// This computer's short name, for the phone's header.
fn machine_name() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its length and gethostname writes at
        // most that many bytes.
        let named = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
        if named {
            let end = buffer
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(buffer.len());
            let name = String::from_utf8_lossy(&buffer[..end]);
            let short = name.split('.').next().unwrap_or_default().trim();
            if !short.is_empty() {
                return short.to_string();
            }
        }
    }
    #[cfg(windows)]
    {
        if let Some(name) = std::env::var_os("COMPUTERNAME") {
            return name.to_string_lossy().into_owned();
        }
    }
    "ZeroCode".to_string()
}

/// A bell rang (`pane_runtime::ring_now`). When it asks for the person and
/// the person is not at the window, the paired phones are told.
pub(crate) fn ring(app: &AppHandle, term: Option<u32>, ring: Ring) {
    let Some(link) = link() else {
        return;
    };
    if !matches!(ring, Ring::Attention) {
        return;
    }
    let Some(term) = term else {
        return;
    };
    if link.listening_port().is_none() || link.book().push_targets().is_empty() {
        return;
    }
    let now = crate::usage_runtime::epoch_ms_now();
    let focused = app
        .get_webview_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    let last_hand = app.state::<AppState>().notify_book().last_hand_ms();
    let present = zerocode_core::notify_call::Attendance::of(focused, last_hand, now)
        == zerocode_core::notify_call::Attendance::Present;
    let verdict = match link.gate.lock() {
        Ok(mut gate) => gate.judge(term, present, now),
        Err(_) => return,
    };
    let Verdict::Send { more } = verdict else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let pane = panes_of(&app)
            .into_iter()
            .find(|pane| pane.id == format!("term-{term}"));
        let fallback = PaneView {
            id: format!("term-{term}"),
            agent: String::new(),
            name: String::new(),
            task: None,
            group: "attention",
            waiting: Some("question"),
            since_ms: 0,
        };
        let notice = Notice::for_pane(pane.as_ref().unwrap_or(&fallback), more);
        let _ = link.deliver(&notice).await;
    });
}

fn the_link() -> Result<&'static Link, String> {
    link().ok_or_else(|| "phone link is not ready".to_string())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn phone_link_status() -> Result<LinkStatus, String> {
    let _crumb = crate::crumbs::Command::enter("phone_link_status");
    blocking(|| {
        let link = the_link()?;
        let look = link.look();
        link.remember(look);
        Ok(link.status())
    })
    .await
}

#[tauri::command]
pub(crate) async fn phone_link_enable() -> Result<LinkStatus, String> {
    let _crumb = crate::crumbs::Command::enter("phone_link_enable");
    blocking(|| {
        let link = the_link()?;
        if let Err(word) = link.bring_up(true)
            && let Ok(mut look) = link.last.lock()
        {
            look.problem = Some(word);
        }
        Ok(link.status())
    })
    .await
}

#[tauri::command]
pub(crate) async fn phone_link_disable() -> Result<LinkStatus, String> {
    let _crumb = crate::crumbs::Command::enter("phone_link_disable");
    blocking(|| {
        let link = the_link()?;
        if let Err(word) = link.bring_down()
            && let Ok(mut look) = link.last.lock()
        {
            look.problem = Some(word);
        }
        Ok(link.status())
    })
    .await
}

#[tauri::command]
pub(crate) async fn phone_link_new_code() -> Result<PairingCode, String> {
    let _crumb = crate::crumbs::Command::enter("phone_link_new_code");
    blocking(|| {
        let link = the_link()?;
        let url = link
            .status()
            .url
            .ok_or_else(|| "phone link is not on".to_string())?;
        let now = crate::usage_runtime::epoch_ms_now();
        let (code, expires_ms) = link
            .core
            .pairing
            .lock()
            .map_err(|error| error.to_string())?
            .issue(now)
            .ok_or_else(|| "no random bytes".to_string())?;
        let url = format!("{url}pair#{code}");
        let qr_svg = qr_svg(&url).ok_or_else(|| "qr".to_string())?;
        Ok(PairingCode {
            code,
            url,
            expires_ms,
            qr_svg,
        })
    })
    .await
}

#[tauri::command]
pub(crate) async fn phone_link_revoke(id: String) -> Result<LinkStatus, String> {
    let _crumb = crate::crumbs::Command::enter("phone_link_revoke");
    blocking(move || {
        let link = the_link()?;
        link.write_book(|book| {
            book.revoke(&id);
        });
        Ok(link.status())
    })
    .await
}

#[tauri::command]
pub(crate) async fn phone_link_test_push() -> Result<TestPush, String> {
    let _crumb = crate::crumbs::Command::enter("phone_link_test_push");
    let link = the_link()?;
    link.load_vapid(false);
    Ok(link.deliver(&Notice::test()).await)
}

/// A QR code as SVG, dark modules in `currentColor` on nothing, so the pane
/// colours it with its own tokens.
fn qr_svg(text: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M).ok()?;
    Some(
        code.render::<qrcode::render::svg::Color<'_>>()
            .quiet_zone(true)
            .min_dimensions(192, 192)
            .dark_color(qrcode::render::svg::Color("currentColor"))
            .light_color(qrcode::render::svg::Color("transparent"))
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::book::Book;
    use super::server::Core;
    use super::tailscale::{self, Cli, ServeConfig, Tailnet};
    use std::sync::Mutex;

    const HOST: &str = "host.example.test";

    /// A CLI that answers from fixtures and writes down what it was told to
    /// run.
    struct Fixture {
        status: Result<Tailnet, String>,
        serve: Result<ServeConfig, String>,
        ran: Mutex<Vec<Vec<String>>>,
    }

    impl Fixture {
        fn named(serve: serde_json::Value) -> Self {
            Self {
                status: Ok(Tailnet {
                    installed: true,
                    running: true,
                    dns_name: Some(HOST.into()),
                    https_ready: true,
                    owner_login: None,
                }),
                serve: tailscale::parse_serve(&serve),
                ran: Mutex::new(Vec::new()),
            }
        }

        fn ran(&self) -> Vec<Vec<String>> {
            self.ran.lock().expect("ran").clone()
        }
    }

    impl Cli for Fixture {
        fn status(&self) -> Result<Tailnet, String> {
            self.status.clone()
        }
        fn serve(&self) -> Result<ServeConfig, String> {
            self.serve.clone()
        }
        fn run(&self, args: &[String]) -> Result<String, String> {
            self.ran.lock().expect("ran").push(args.to_vec());
            Ok(String::new())
        }
    }

    /// The window's mapping `10443 → 127.0.0.1:5000`, as Serve writes it.
    fn ours() -> serde_json::Value {
        serde_json::json!({
            "TCP": { "10443": { "HTTPS": true } },
            "Web": { "host.example.test:10443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5000" } } } }
        })
    }

    fn core(enabled: bool) -> Core {
        Core::new(
            None,
            Book {
                enabled,
                port: Some(5000),
                https_port: Some(10443),
                ..Book::default()
            },
        )
    }

    fn pair(core: &Core) -> (Option<u16>, Option<u16>) {
        super::pair_of(&super::book_of(core))
    }

    /// The re-check's blocking scene: 「끄기」 pressed while Tailscale's status
    /// cannot be read. The mapping cannot be taken down, so the book keeps
    /// naming it — off, but still the window's to take down.
    #[test]
    fn turning_off_with_tailscale_unread_keeps_the_mapping_named() {
        let core = core(true);
        let mut unread = Fixture::named(ours());
        unread.status = Err("tailscale did not answer in time".into());
        assert!(super::turn_off(&core, Some(&unread), Some(5000)).is_err());
        assert!(!super::book_of(&core).enabled);
        assert_eq!(pair(&core), (Some(10443), Some(5000)));
        assert!(unread.ran().is_empty());

        let mut nameless = Fixture::named(ours());
        nameless.status = Ok(Tailnet::unread());
        assert!(super::turn_off(&core, Some(&nameless), Some(5000)).is_err());
        assert_eq!(
            pair(&core),
            (Some(10443), Some(5000)),
            "no name, no take-down"
        );

        let mut unserved = Fixture::named(ours());
        unserved.serve = Err("tailscale: serve status failed".into());
        assert!(super::turn_off(&core, Some(&unserved), Some(5000)).is_err());
        assert!(super::turn_off(&core, None, Some(5000)).is_err(), "no CLI");
        assert_eq!(pair(&core), (Some(10443), Some(5000)));

        let read = Fixture::named(ours());
        assert!(super::turn_off(&core, Some(&read), Some(5000)).is_ok());
        assert_eq!(read.ran(), [tailscale::serve_off_args(10443)]);
        assert_eq!(pair(&core), (None, None), "gone, then forgotten");
    }

    /// The book is off but still names a mapping: the next start takes it
    /// down without a listener, then forgets it.
    #[test]
    fn a_start_with_the_link_off_takes_the_named_mapping_down() {
        let core = core(false);
        let mut unread = Fixture::named(ours());
        unread.status = Err("tailscale did not answer in time".into());
        assert!(matches!(
            super::sweep_off(&core, Some(&unread), None),
            Some(Err(_))
        ));
        assert_eq!(
            pair(&core),
            (Some(10443), Some(5000)),
            "kept for the next start"
        );

        let read = Fixture::named(ours());
        assert_eq!(super::sweep_off(&core, Some(&read), None), Some(Ok(())));
        assert_eq!(read.ran(), [tailscale::serve_off_args(10443)]);
        assert_eq!(pair(&core), (None, None));
        assert_eq!(
            super::sweep_off(&core, Some(&read), None),
            None,
            "nothing named now"
        );

        let on = self::core(true);
        assert_eq!(
            super::sweep_off(&on, Some(&read), None),
            None,
            "an on link is the start's"
        );
        assert_eq!(pair(&on), (Some(10443), Some(5000)));
    }

    /// A mapping somebody else moved off the window's port is not the
    /// window's: the start forgets the pair and touches nothing.
    #[test]
    fn a_start_with_the_link_off_leaves_a_mapping_it_did_not_make() {
        let core = core(false);
        let moved = Fixture::named(serde_json::json!({
            "TCP": { "10443": { "HTTPS": true } },
            "Web": { "host.example.test:10443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:7777" } } } }
        }));
        assert_eq!(super::sweep_off(&core, Some(&moved), None), Some(Ok(())));
        assert!(moved.ran().is_empty());
        assert_eq!(pair(&core), (None, None));
    }

    /// What a start does is read from the book alone: on brings the link
    /// up, off with a mapping named takes that mapping down.
    #[test]
    fn a_start_reads_what_to_do_from_the_book() {
        use super::{Start, start_of};
        assert_eq!(start_of(&Book::default()), Start::Nothing);
        let on = Book {
            enabled: true,
            ..Book::default()
        };
        assert_eq!(start_of(&on), Start::BringUp);
        let named = Book {
            https_port: Some(10443),
            port: Some(5000),
            ..Book::default()
        };
        assert_eq!(start_of(&named), Start::Sweep);
    }

    /// A stand-in for the accept loop.
    struct Task(std::sync::Arc<std::sync::atomic::AtomicBool>);

    impl super::Stop for Task {
        fn stop(self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn free(port: u16) -> bool {
        std::net::TcpListener::bind((tailscale::LOOPBACK, port)).is_ok()
    }

    /// The delta check's blocking scene: two minute looks in a row cannot
    /// read Tailscale while the window's mapping may stand. The listener
    /// stops accepting, and the port stays the window's — no other program
    /// can bind what the mapping points at — until the window knows the
    /// mapping is gone.
    #[test]
    fn two_missed_looks_stop_accepting_and_keep_the_port() {
        let socket = std::net::TcpListener::bind((tailscale::LOOPBACK, 0)).expect("bind");
        let port = socket.local_addr().expect("address").port();
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut seat = super::Seat::new(None);
        seat.serve(port, Task(std::sync::Arc::clone(&stopped)), socket);
        let mut missed = 0;
        assert!(!super::missed_look(&mut missed), "one miss is a hiccup");
        assert!(super::missed_look(&mut missed), "two stop the listener");
        seat.pause();
        assert!(
            stopped.load(std::sync::atomic::Ordering::SeqCst),
            "nothing is accepted"
        );
        assert_eq!(seat.serving_port(), None);
        assert_eq!(seat.port(), Some(port), "the port is still the window's");
        assert!(!free(port), "no other program can take it");
        // A look that reads again serves the same port.
        let again = seat.take_held().expect("held");
        assert_eq!(again.local_addr().expect("address").port(), port);
        seat.hold(again);
        assert!(!free(port));
        // Turned off with the mapping known gone: the port goes.
        seat.release();
        assert_eq!(seat.port(), None);
        assert!(free(port), "let go once the mapping is gone");
    }

    #[test]
    fn a_qr_code_draws_in_the_panes_own_colour() {
        let svg = super::qr_svg("https://host.example.test:8443/pair#ABCD2345").expect("fits");
        assert!(svg.starts_with("<?xml") || svg.starts_with("<svg"));
        assert!(svg.contains("currentColor"));
        assert!(!svg.contains("#000"));
    }

    #[test]
    fn random_bytes_are_not_all_zero() {
        let bytes = super::random_bytes::<32>().expect("os random");
        assert!(bytes.iter().any(|byte| *byte != 0));
    }

    /// The real road on the machine that runs it, by hand only: this
    /// machine's Tailscale maps one HTTPS port to a real loopback listener
    /// with the window's own router behind it, a request through the tailnet
    /// name is answered, the API is 401 without a token and 200 with one,
    /// and turning off leaves Serve exactly as it was found. Names are
    /// printed masked.
    ///
    /// `ZEROCODE_PHONE_LINK_LIVE=1 cargo test -p zerocode-shell --bin
    /// zerocode-shell live_tailscale -- --ignored --nocapture`
    #[test]
    #[ignore = "drives this machine's real Tailscale Serve; run by hand"]
    fn live_tailscale_round_trip() {
        use super::server::{Core, PaneView, StatusView, Window};
        use super::{book, tailscale};
        use std::sync::Arc;

        struct Fixture;
        impl Window for Fixture {
            fn now_ms(&self) -> i64 {
                crate::usage_runtime::epoch_ms_now()
            }
            fn status(&self) -> StatusView {
                StatusView {
                    version: "live".into(),
                    name: "fixture".into(),
                }
            }
            fn panes(&self) -> Vec<PaneView> {
                Vec::new()
            }
        }

        /// Puts Serve back the way it was found, whatever the test did.
        struct Off(tailscale::Installed, u16);
        impl Drop for Off {
            fn drop(&mut self) {
                let _ = self.0.run(&tailscale::serve_off_args(self.1));
            }
        }

        if std::env::var_os("ZEROCODE_PHONE_LINK_LIVE").is_none() {
            return;
        }
        let cli = tailscale::Installed::find().expect("a Tailscale CLI");
        let tailnet = cli.status().expect("tailscale status");
        assert!(tailnet.running && tailnet.https_ready, "tailnet not ready");
        let name = tailnet.dns_name.clone().expect("a MagicDNS name");
        // This machine's resolver need not answer for its own MagicDNS name
        // (11-12: `getaddrinfo` said no such name while 100.100.100.100
        // answered it), so the requests go to its tailnet address under that
        // name — TLS and Host still carry the name, as a phone's do.
        let own = cli
            .run(&["status".into(), "--json".into()])
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|json| {
                json["Self"]["TailscaleIPs"].as_array().and_then(|ips| {
                    ips.iter()
                        .filter_map(|ip| ip.as_str()?.parse::<std::net::IpAddr>().ok())
                        .find(std::net::IpAddr::is_ipv4)
                })
            })
            .expect("this machine's tailnet address");
        let before = cli.serve().expect("serve status");

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind((tailscale::LOOPBACK, 0)))
            .expect("loopback bind");
        let port = listener.local_addr().expect("addr").port();
        let core = Arc::new(Core::new(None, book::Book::default()));
        core.hosts.lock().unwrap().dns_name = Some(name.clone());
        core.hosts.lock().unwrap().listener_port = port;
        core.owner.lock().unwrap().clone_from(&tailnet.owner_login);
        let window: Arc<dyn Window> = Arc::new(Fixture);
        {
            let core = Arc::clone(&core);
            runtime.spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let core = Arc::clone(&core);
                    let window = Arc::clone(&window);
                    tokio::spawn(super::server::serve_connection(stream, core, window));
                }
            });
        }

        let pressed = tailscale::decide(&tailscale::Facts {
            config: &before,
            dns_name: &name,
            remembered: (None, None),
            listener: port,
            hook: crate::hooks::bridge().map(|bridge| bridge.port),
            asked: true,
        });
        assert!(pressed.remove.is_empty(), "nothing of ours stood before");
        let tailscale::Then::Serve { https, map: true } = pressed.then else {
            panic!("no free phone port: {pressed:?}");
        };
        cli.run(&tailscale::serve_on_args(https, port))
            .expect("serve on");
        let off = Off(
            tailscale::Installed::find().expect("a Tailscale CLI"),
            https,
        );
        let mapped = cli.serve().expect("serve status");
        assert_eq!(
            tailscale::reach_of(&mapped, &name, port),
            tailscale::Reach::Ours(https),
            "the mapping is ours and tailnet-only"
        );
        println!("on: https://<ts.net 이름>:{https}/ -> 127.0.0.1:<port>");

        let base = format!("https://{name}:{https}");
        let code = core
            .pairing
            .lock()
            .unwrap()
            .issue(crate::usage_runtime::epoch_ms_now())
            .expect("a code")
            .0;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .resolve(&name, std::net::SocketAddr::new(own, https))
            .build()
            .expect("client");
        let (shell, no_token, paired, with_token) = runtime.block_on(async {
            let shell = client.get(format!("{base}/")).send().await.expect("GET /");
            let no_token = client
                .get(format!("{base}/api/panes"))
                .send()
                .await
                .expect("GET /api/panes");
            let paired = client
                .post(format!("{base}/api/pair"))
                .header("Content-Type", "application/json")
                .body(format!(r#"{{"code":"{code}","name":"live test"}}"#))
                .send()
                .await
                .expect("POST /api/pair");
            let paired_status = paired.status().as_u16();
            let token = paired
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|body| body["token"].as_str().map(str::to_string))
                .unwrap_or_default();
            let with_token = client
                .get(format!("{base}/api/panes"))
                .bearer_auth(&token)
                .send()
                .await
                .expect("GET /api/panes with token");
            (
                shell.status().as_u16(),
                no_token.status().as_u16(),
                paired_status,
                with_token.status().as_u16(),
            )
        });
        println!("GET / = {shell}");
        println!("GET /api/panes without token = {no_token}");
        println!("POST /api/pair (owner login from Serve) = {paired}");
        println!("GET /api/panes with token = {with_token}");

        drop(off);
        let after = cli.serve().expect("serve status");
        println!(
            "off: phone mapping stands = {}; Serve as found = {}",
            tailscale::ours_stands(&after, &name, https, port),
            after == before
        );
        assert_eq!((shell, no_token, paired, with_token), (200, 401, 200, 200));
        assert_eq!(after, before, "Serve is exactly as it was found");
        runtime.shutdown_background();
    }
}
