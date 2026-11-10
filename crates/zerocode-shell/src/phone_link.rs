//! The phone link, stage 1: a phone sees this window's panes and is told when
//! one needs the person — over the person's own tailnet, with no server of
//! ours anywhere.
//!
//! ```text
//! phone ──https──▶ Tailscale Serve (tailnet only, :8443)
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
use tailscale::{Exposure, Tailnet};
use view::{Gate, Notice, RawPane, Verdict};

const CREDENTIAL_SERVICE: &str = "dev.zerocode.shell.phone-link";
const VAPID_ACCOUNT: &str = "vapid-p256";
/// How often an enabled link re-reads the Serve configuration, so Funnel or a
/// hook-bridge mapping turned on behind its back closes the listener.
const RECHECK: Duration = Duration::from_secs(60);
/// Connections one listener serves at once.
const MAX_CONNECTIONS: usize = 32;
const PUSH_TIMEOUT: Duration = Duration::from_secs(15);

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
        if let Some(link) = link() {
            let _ = link.bring_up(false);
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
        let snapshot = {
            let Ok(mut book) = self.core.book.lock() else {
                return;
            };
            change(&mut book);
            book.clone()
        };
        self.core.save(&snapshot);
    }

    fn listening_port(&self) -> Option<u16> {
        self.running
            .lock()
            .ok()
            .and_then(|running| running.as_ref().map(|one| one.port))
    }

    fn status(&self) -> LinkStatus {
        let book = self.book();
        let look = self.last.lock().map(|look| look.clone()).unwrap_or_default();
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
                let hook = crate::hooks::bridge().map(|bridge| bridge.port);
                look.exposure = Some(tailscale::judge(
                    &serve,
                    &name,
                    https,
                    self.listening_port().or(book.port),
                    hook,
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
    /// person's press; a boot bring-up maps only what was mapped before.
    fn bring_up(&'static self, asked: bool) -> Result<(), String> {
        let epoch = self
            .epoch
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let cli = tailscale::find_cli().ok_or("tailscale")?;
        let tailnet = tailscale::read_status(&cli);
        if !tailnet.running {
            self.remember(Look {
                tailnet,
                ..Look::default()
            });
            return Err("tailscale-stopped".into());
        }
        let Some(name) = tailnet.dns_name.clone() else {
            self.remember(Look {
                tailnet,
                ..Look::default()
            });
            return Err("tailscale-name".into());
        };
        if !tailnet.https_ready {
            self.remember(Look {
                tailnet,
                ..Look::default()
            });
            return Err("tailscale-https".into());
        }
        let serve = tailscale::read_serve(&cli)?;
        let hook = crate::hooks::bridge().map(|bridge| bridge.port);
        if let Some(hook) = hook
            && tailscale::judge(&serve, &name, 0, None, Some(hook)) == Exposure::HookExposed
        {
            self.remember(Look {
                tailnet,
                exposure: Some(Exposure::HookExposed),
                problem: None,
            });
            self.stop_listener();
            return Err("hook-exposed".into());
        }
        self.load_vapid(true);
        let book = self.book();
        let port = self.start_listener(book.port)?;
        // The port this window held last was taken while it was closed: the
        // mapping it left still points there, at somebody else's listener.
        // That mapping is this window's own (the book says so), and it goes
        // before anything else is decided.
        let mut serve = serve;
        if let (Some(old), Some(https)) = (book.port, book.https_port)
            && old != port
            && tailscale::judge(&serve, &name, https, Some(old), None) == Exposure::Ours
        {
            tailscale::run(&cli, &tailscale::serve_off_args(https))?;
            serve = tailscale::read_serve(&cli)?;
        }
        let https = tailscale::pick_port(&serve, &name, port, book.https_port)
            .ok_or("no-free-port")?;
        let exposure = tailscale::judge(&serve, &name, https, Some(port), hook);
        match exposure {
            Exposure::Ours => {}
            Exposure::Absent if asked || book.https_port == Some(https) => {
                tailscale::run(&cli, &tailscale::serve_on_args(https, port))?;
            }
            Exposure::Absent => {}
            Exposure::Taken => return Err("port-taken".into()),
            Exposure::Funnel | Exposure::HookExposed => {
                self.stop_listener();
                return Err("refused".into());
            }
        }
        self.write_book(|book| {
            book.enabled = true;
            book.port = Some(port);
            book.https_port = Some(https);
        });
        let look = self.look();
        self.remember(look);
        self.watch(epoch);
        Ok(())
    }

    /// Turn the link off: the Serve mapping this window made, then the port.
    fn bring_down(&self) -> Result<(), String> {
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let book = self.book();
        let mut outcome = Ok(());
        if let (Some(cli), Some(https)) = (tailscale::find_cli(), book.https_port) {
            let mine = tailscale::read_serve(&cli).ok().is_some_and(|serve| {
                let name = self
                    .last
                    .lock()
                    .ok()
                    .and_then(|look| look.tailnet.dns_name.clone())
                    .unwrap_or_default();
                tailscale::judge(&serve, &name, https, self.listening_port().or(book.port), None)
                    == Exposure::Ours
            });
            if mine && let Err(error) = tailscale::run(&cli, &tailscale::serve_off_args(https)) {
                outcome = Err(error);
            }
        }
        self.stop_listener();
        if let Ok(mut pairing) = self.core.pairing.lock() {
            pairing.clear();
        }
        self.write_book(|book| book.enabled = false);
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

    /// Re-read Serve every minute while on; Funnel or an exposed hook bridge
    /// closes the listener.
    fn watch(&'static self, epoch: u64) {
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(RECHECK);
                if self.epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch {
                    return;
                }
                let look = self.look();
                let refused = matches!(
                    look.exposure,
                    Some(Exposure::Funnel | Exposure::HookExposed)
                );
                self.remember(look);
                if refused {
                    self.stop_listener();
                    return;
                }
            }
        });
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
            let worktree =
                crate::pane_runtime::seated_worktree(&state, agent.term).or_else(|| {
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
            let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
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
        if let Some(link) = link() {
            let _ = link.deliver(&notice).await;
        }
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
        if let Err(word) = link.bring_up(true) {
            link.stop_listener();
            if let Ok(mut look) = link.last.lock() {
                look.problem = Some(word);
            }
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
}
