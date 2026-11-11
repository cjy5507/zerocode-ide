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

use std::sync::{Arc, Mutex, OnceLock};
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
use tailscale::{Exposure, Facts, Refusal, Tailnet, Then};
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
    running: Mutex<Option<Listening>>,
    gate: Mutex<Gate>,
    /// The VAPID signing key, read from the OS credential store once.
    vapid: Mutex<Option<p256::SecretKey>>,
    /// What the last look at Tailscale found.
    last: Mutex<Look>,
    /// Bumped by every on/off, so a recheck that started before it stands down.
    epoch: std::sync::atomic::AtomicU64,
}

struct Listening {
    port: u16,
    task: tauri::async_runtime::JoinHandle<()>,
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

/// `serve --https=<port> off` for each port, stopping at the first refusal.
fn take_down(
    cli: &std::path::Path,
    ports: &[u16],
    run: impl Fn(&std::path::Path, &[String]) -> Result<String, String>,
) -> Result<(), String> {
    for https in ports {
        run(cli, &tailscale::serve_off_args(*https))?;
    }
    Ok(())
}

/// The window is closing: the mapping it made comes down with it, so no
/// tailnet name points at a loopback port somebody else may take before the
/// next start. The book keeps the link on and its port, and the next start
/// maps it again.
pub(crate) fn shutdown() {
    let Some(link) = link() else {
        return;
    };
    if link.listening_port().is_none() {
        return;
    }
    link.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let _ = link.take_down_own(tailscale::EXIT_CLI_TIMEOUT);
    link.stop_listener();
}

/// Read the book and, when a person had turned the link on, bring it back up
/// off the boot path.
pub(crate) fn boot(app: &AppHandle, local_data_root: &std::path::Path) {
    let path = book::book_path(local_data_root);
    let held = book::load(&path);
    let enabled = held.enabled;
    let core = Arc::new(Core::new(Some(path), held));
    let made = LINK.set(Link {
        app: app.clone(),
        core,
        running: Mutex::new(None),
        gate: Mutex::new(Gate::default()),
        vapid: Mutex::new(None),
        last: Mutex::new(Look::default()),
        epoch: std::sync::atomic::AtomicU64::new(0),
    });
    if made.is_err() || !enabled {
        return;
    }
    std::thread::spawn(|| {
        if let Some(link) = link()
            && let Err(word) = link.bring_up(false)
            && let Ok(mut look) = link.last.lock()
        {
            look.problem = Some(word);
        }
    });
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
        self.core
            .book
            .lock()
            .map(|book| book.clone())
            .unwrap_or_default()
    }

    fn write_book(&self, change: impl FnOnce(&mut Book)) {
        {
            let Ok(mut book) = self.core.book.lock() else {
                return;
            };
            change(&mut book);
        }
        self.core.save();
    }

    fn listening_port(&self) -> Option<u16> {
        self.running
            .lock()
            .ok()
            .and_then(|running| running.as_ref().map(|one| one.port))
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
        let Some(cli) = tailscale::find_cli() else {
            return Look::default();
        };
        let tailnet = tailscale::read_status(&cli);
        let mut look = Look {
            tailnet,
            exposure: None,
            problem: None,
        };
        let (Some(name), true) = (look.tailnet.dns_name.clone(), look.tailnet.running) else {
            return look;
        };
        match tailscale::read_serve(&cli) {
            Ok(serve) => {
                let book = self.book();
                let https = book.https_port.unwrap_or(tailscale::HTTPS_PORTS[0]);
                look.exposure = Some(tailscale::exposure_of(
                    &serve,
                    &name,
                    https,
                    self.listening_port(),
                    hook_port(),
                ));
            }
            Err(error) => look.problem = Some(error),
        }
        look
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
    /// Whatever stops it closes the listener again.
    fn bring_up(&'static self, asked: bool) -> Result<(), String> {
        let epoch = self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let outcome = self.bring_up_at(epoch, asked);
        if outcome.is_err() {
            self.stop_listener();
        }
        outcome
    }

    fn bring_up_at(&'static self, epoch: u64, asked: bool) -> Result<(), String> {
        let cli = tailscale::find_cli().ok_or("tailscale")?;
        let tailnet = tailscale::read_status(&cli);
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
        let listener = self.start_listener(book.port)?;
        let serve = tailscale::read_serve(&cli)?;
        let facts = Facts {
            config: &serve,
            dns_name: &name,
            remembered: (book.https_port, book.port),
            listener,
            hook: hook_port(),
            asked,
        };
        let mut decision = tailscale::decide(&facts);
        take_down(&cli, &decision.remove, tailscale::run)?;
        if !decision.remove.is_empty() && matches!(decision.then, Then::Serve { .. }) {
            // What the CLI left is read again rather than assumed.
            let left = tailscale::read_serve(&cli)?;
            decision = tailscale::decide(&Facts {
                config: &left,
                ..facts
            });
            if !decision.remove.is_empty() {
                decision.then = Then::Refuse(Refusal::Shared);
            }
        }
        let https = match decision.then {
            Then::Serve { https, map } => {
                if map {
                    tailscale::run(&cli, &tailscale::serve_on_args(https, listener))?;
                }
                https
            }
            Then::Refuse(refusal) => {
                self.remember(Look {
                    tailnet,
                    exposure: Some(refusal.exposure()),
                    problem: None,
                });
                return Err(refusal.word().into());
            }
        };
        self.load_vapid(true);
        self.write_book(|book| {
            book.enabled = true;
            book.port = Some(listener);
            book.https_port = Some(https);
        });
        let look = self.look();
        self.remember(look);
        self.watch(epoch);
        Ok(())
    }

    /// Take down every Serve mapping of the window's: the one the book names
    /// wherever it now points, and any phone port sending to the listener.
    fn take_down_own(&self, limit: Duration) -> Result<(), String> {
        let Some(cli) = tailscale::find_cli() else {
            return Ok(());
        };
        let book = self.book();
        let listening = self.listening_port();
        if book.https_port.is_none() && listening.is_none() {
            return Ok(());
        }
        let Some(name) = tailscale::read_status(&cli).dns_name else {
            return Ok(());
        };
        let serve = tailscale::read_serve(&cli)?;
        let mut ports: Vec<u16> = Vec::new();
        if let (Some(https), Some(port)) = (book.https_port, book.port)
            && tailscale::ours_stands(&serve, &name, https, port)
        {
            ports.push(https);
        }
        if let Some(port) = listening {
            for https in tailscale::HTTPS_PORTS {
                if !ports.contains(&https) && tailscale::ours_stands(&serve, &name, https, port) {
                    ports.push(https);
                }
            }
        }
        take_down(&cli, &ports, |cli, args| {
            tailscale::run_within(cli, args, limit)
        })
    }

    /// Turn the link off: the Serve mappings this window made, then the port.
    fn bring_down(&self) -> Result<(), String> {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let outcome = self.take_down_own(tailscale::CLI_TIMEOUT);
        self.stop_listener();
        if let Ok(mut pairing) = self.core.pairing.lock() {
            pairing.clear();
        }
        let gone = outcome.is_ok();
        self.write_book(|book| {
            book.enabled = false;
            // A mapping that could not be taken down stays named, so the next
            // start tries again.
            if gone {
                book.https_port = None;
                book.port = None;
            }
        });
        let look = self.look();
        self.remember(look);
        outcome
    }

    fn start_listener(&'static self, remembered: Option<u16>) -> Result<u16, String> {
        if let Some(port) = self.listening_port() {
            return Ok(port);
        }
        let bound = remembered
            .and_then(|port| std::net::TcpListener::bind(("127.0.0.1", port)).ok())
            .map_or_else(|| std::net::TcpListener::bind(("127.0.0.1", 0)), Ok)
            .map_err(|error| format!("listen: {error}"))?;
        bound
            .set_nonblocking(true)
            .map_err(|error| format!("listen: {error}"))?;
        let port = bound
            .local_addr()
            .map_err(|error| format!("listen: {error}"))?
            .port();
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
        if let Ok(mut running) = self.running.lock() {
            *running = Some(Listening { port, task });
        }
        Ok(port)
    }

    fn stop_listener(&self) {
        if let Some(listening) = self.running.lock().ok().and_then(|mut held| held.take()) {
            listening.task.abort();
        }
    }

    /// Re-read Serve every minute while on, and judge it as a start would:
    /// a refusal takes the window's mappings down and closes the listener.
    fn watch(&'static self, epoch: u64) {
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(RECHECK);
                if self.epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch {
                    return;
                }
                let mut look = self.look();
                if let Some(refusal) = self.refusal_now()
                    && self.epoch.load(std::sync::atomic::Ordering::SeqCst) == epoch
                {
                    look.exposure = Some(refusal.exposure());
                    look.problem = Some(refusal.word().into());
                    self.remember(look);
                    return;
                }
                self.remember(look);
            }
        });
    }

    /// Judge the running link once; when refused, take its mappings down and
    /// close the listener.
    fn refusal_now(&self) -> Option<Refusal> {
        let listener = self.listening_port()?;
        let cli = tailscale::find_cli()?;
        let name = tailscale::read_status(&cli).dns_name?;
        let serve = tailscale::read_serve(&cli).ok()?;
        let book = self.book();
        let decision = tailscale::decide(&Facts {
            config: &serve,
            dns_name: &name,
            remembered: (book.https_port, book.port),
            listener,
            hook: hook_port(),
            asked: false,
        });
        let Then::Refuse(refusal) = decision.then else {
            return None;
        };
        let _ = take_down(&cli, &decision.remove, tailscale::run);
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
        struct Off(std::path::PathBuf, u16);
        impl Drop for Off {
            fn drop(&mut self) {
                let _ = tailscale::run(&self.0, &tailscale::serve_off_args(self.1));
            }
        }

        if std::env::var_os("ZEROCODE_PHONE_LINK_LIVE").is_none() {
            return;
        }
        let cli = tailscale::find_cli().expect("a Tailscale CLI");
        let tailnet = tailscale::read_status(&cli);
        assert!(tailnet.running && tailnet.https_ready, "tailnet not ready");
        let name = tailnet.dns_name.clone().expect("a MagicDNS name");
        let before = tailscale::read_serve(&cli).expect("serve status");

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind(("127.0.0.1", 0)))
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
        tailscale::run(&cli, &tailscale::serve_on_args(https, port)).expect("serve on");
        let off = Off(cli.clone(), https);
        let mapped = tailscale::read_serve(&cli).expect("serve status");
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
        let after = tailscale::read_serve(&cli).expect("serve status");
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
