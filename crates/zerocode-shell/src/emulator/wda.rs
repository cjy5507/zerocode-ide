//! A real iPhone, driven from inside it (t-90894): the verbs of the simulator's
//! road, carried by WebDriverAgent over the tunnel the Mac keeps to the phone, so
//! no Mac pointer, front app or clipboard is used, and two agents on two devices do
//! not meet.
//!
//! The words, pins and readers are `zerocode_core::agent_emulator::real_phone`'s.
//! Here is what runs them: [`runner`] the process, [`client`] its HTTP, [`supply`]
//! the build, this file the verbs and the person's say.
//!
//! - One phone takes one verb at a time, first come first served: a verb holds its
//!   phone's gate from the lock check to its answer. Nothing else is shared, so
//!   phones, simulators and desktop agents run side by side.
//! - Every verb asks the lock first. A locked phone is refused with the road that
//!   is open now; nothing here unlocks one, and nothing waits for a person.
//! - The road stays off until the person's answer on the window's card. That answer
//!   is sealed with a key in the Keychain, so a line typed into the settings file
//!   (which an agent can reach) does not turn it on.

mod client;
mod runner;
mod supply;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tauri::Manager as _;
use zerocode_core::agent_emulator::real_phone::{
    self as words, CheckItem, IDLE_STOP_MINUTES, LockedRoads, MAC_POINTER_KEY, PHONE_LOCKED,
    PHONE_NOT_ALLOWED, PHONE_NOT_INSTALLED, PHONE_SECURE_FIELD, PHONE_UNREACHABLE, PhoneFacts,
    RESTART_BACKOFF_SECS, ROAD_KEY, ROAD_WDA, RunnerFailure, Team, WDA_PORT,
};
use zerocode_core::computer_use_protocol::{ProviderError, error_code};

use self::client::{CallError, Wda};
use self::runner::{Life, NotUp};
use super::marks::{Pressed, Proof, Snapshot};
use crate::credential_store::VerifiedSecretStore;

/// How long a call waits for a runner that is not up. A start with nothing to build
/// took 14.5 s on a simulator and can take far longer with a phone's install in it.
const START_WAIT: Duration = Duration::from_secs(120);
/// How long a person is given to answer the card; past it nothing is on.
const CARD_WAIT_MS: u64 = 180_000;
/// Longest a build may take; a first build took 226 s at a low priority.
const BUILD_CAP: Duration = Duration::from_secs(20 * 60);
const GIT_CAP: Duration = Duration::from_secs(120);
const TOOL_OUTPUT_BYTES: usize = 256 * 1024;
const SEAL_SERVICE: &str = "dev.zerocode.real-phone";
const SEAL_ACCOUNT: &str = "consent-seal";
/// The folder under the window's data that holds what this road builds.
const DATA_DIR: &str = "real-phone";
/// A signature with fewer days than this left is called bad before the person goes.
const SIGNATURE_DAYS_FLOOR: i64 = 2;

// ---- The person's say -----------------------------------------------------

/// The Keychain key that seals the person's answers. The settings file holds only
/// the seal of each answer; an agent can write that file, and cannot read this key
/// without the Keychain asking the person.
pub(crate) struct Seal {
    store: VerifiedSecretStore,
    key: Mutex<Option<String>>,
}

impl Seal {
    #[cfg(test)]
    pub(super) fn with_store(store: Arc<dyn crate::credential_store::SecretStore>) -> Self {
        Self {
            store: VerifiedSecretStore::with_store(store),
            key: Mutex::new(None),
        }
    }

    fn key(&self, create: bool) -> Option<String> {
        let mut key = self
            .key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if key.is_none() {
            *key = self
                .store
                .read(SEAL_ACCOUNT)
                .ok()
                .map(|read| read.as_str().to_string());
        }
        if key.is_none() && create {
            let fresh = uuid::Uuid::new_v4().to_string();
            if self
                .store
                .write(SEAL_ACCOUNT, zeroize::Zeroizing::new(fresh.clone()))
                .is_ok()
            {
                *key = Some(fresh);
            }
        }
        key.clone()
    }

    fn tag(&self, what: &str, team: &str, create: bool) -> Option<String> {
        let key = self.key(create)?;
        let digest = Sha256::digest(format!("{key}\0{what}\0{team}").as_bytes());
        Some(digest.iter().fold(String::new(), |mut hex, byte| {
            hex.push_str(&format!("{byte:02x}"));
            hex
        }))
    }

    /// What is stored for the person's answer. Only the two roads a person's hand
    /// reaches call this with `create`.
    pub(crate) fn sealed(&self, what: Said, team: &str) -> Option<String> {
        self.tag(what.word(), team, true)
    }

    /// Whether a stored answer is one this window sealed.
    pub(crate) fn holds(&self, what: Said, team: &str, stored: &str) -> bool {
        !stored.is_empty()
            && !team.is_empty()
            && self.tag(what.word(), team, false).as_deref() == Some(stored)
    }
}

/// What a person can say about this road.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Said {
    Allowed,
    Unattended,
}

impl Said {
    const fn word(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Unattended => "unattended",
        }
    }
}

/// The window's seal.
pub(crate) fn seal() -> &'static Seal {
    static SEAL: OnceLock<Seal> = OnceLock::new();
    SEAL.get_or_init(|| Seal {
        store: VerifiedSecretStore::new(SEAL_SERVICE),
        key: Mutex::new(None),
    })
}

fn document() -> Option<crate::settings_runtime::SettingsDocument> {
    let app = super::WINDOW.get()?;
    Some(
        crate::settings_runtime::load_settings_resilient(app.state::<crate::AppState>().settings())
            .document,
    )
}

/// The team the person allowed, when they have.
fn allowed_team() -> Option<String> {
    let document = document()?;
    seal()
        .holds(
            Said::Allowed,
            &document.emulator_real_phone_team,
            &document.emulator_real_phone_consent,
        )
        .then_some(document.emulator_real_phone_team)
}

fn unattended_now() -> bool {
    document().is_some_and(|document| {
        let team = &document.emulator_real_phone_team;
        seal().holds(Said::Allowed, team, &document.emulator_real_phone_consent)
            && seal().holds(
                Said::Unattended,
                team,
                &document.emulator_real_phone_unattended,
            )
    })
}

fn data_root() -> Option<PathBuf> {
    let app = super::WINDOW.get()?;
    Some(
        app.state::<crate::AppState>()
            .local_data_root()
            .join(DATA_DIR),
    )
}

fn phone_dir(handle: &str) -> Option<PathBuf> {
    Some(data_root()?.join(handle))
}

// ---- One phone ------------------------------------------------------------

/// What a phone's road needs of the machine: real on a real phone, written by a
/// test on a fake runner or a simulator.
trait Road: Send + Sync {
    /// The person's answer and whether a build stands.
    fn allowed(&self) -> bool;
    fn installed(&self) -> bool;
    /// The runner's HTTP base, read now.
    fn base(&self) -> Result<String, String>;
    fn locked_roads(&self) -> LockedRoads;
}

pub(super) struct Phone {
    handle: String,
    road: Arc<dyn Road>,
    life: Arc<Life>,
    gate: Arc<tokio::sync::Mutex<Wda>>,
}

static PHONES: Mutex<Option<HashMap<String, Arc<Phone>>>> = Mutex::new(None);

fn phones() -> std::sync::MutexGuard<'static, Option<HashMap<String, Arc<Phone>>>> {
    PHONES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A phone that is up to its own road, known by its id from now on.
fn register(handle: &str, road: Arc<dyn Road>, life: Arc<Life>) -> Result<(), String> {
    let phone = Arc::new(Phone {
        handle: handle.to_string(),
        road,
        life,
        gate: Arc::new(tokio::sync::Mutex::new(Wda::new()?)),
    });
    phones()
        .get_or_insert_with(HashMap::new)
        .insert(handle.to_string(), phone);
    Ok(())
}

/// A real phone's road: `devicectl` for its address, the settings for the person's
/// answer, the build in the window's data.
struct RealRoad {
    handle: String,
}

impl Road for RealRoad {
    fn allowed(&self) -> bool {
        allowed_team().is_some()
    }

    fn installed(&self) -> bool {
        phone_dir(&self.handle).is_some_and(|dir| supply::xctestrun_in(&dir).is_some())
    }

    fn base(&self) -> Result<String, String> {
        let facts = read_phone(&self.handle)?;
        let address = facts.address.ok_or_else(|| {
            format!(
                "{PHONE_UNREACHABLE}: the Mac has no tunnel to the phone now. Check the cable or the network; the next call tries again."
            )
        })?;
        Ok(words::runner_base(&address, WDA_PORT))
    }

    fn locked_roads(&self) -> LockedRoads {
        locked_roads_now()
    }
}

fn locked_roads_now() -> LockedRoads {
    let shared_desktop = super::WINDOW.get().is_some_and(|app| {
        crate::settings_runtime::computer_input_policy(app.state::<crate::AppState>().settings())
            == zerocode_core::computer_use_protocol::execution::InputPolicy::SharedDesktop
    });
    LockedRoads {
        mirroring_installed: Path::new("/System/Applications/iPhone Mirroring.app").exists(),
        mirroring_running: crate::proc::quiet_command("/usr/bin/pgrep")
            .args(["-x", "iPhone Mirroring"])
            .stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success()),
        shared_desktop,
    }
}

/// Every iPhone and iPad as `devicectl` sees them now.
fn read_phones() -> Result<Vec<PhoneFacts>, String> {
    #[cfg(target_os = "macos")]
    {
        let directory = super::scratch_directory();
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let file = directory.join(format!("devicectl-{}.json", uuid::Uuid::new_v4()));
        let mut command = super::ios::devicectl_command();
        command
            .args([
                "list",
                "devices",
                "--quiet",
                "--timeout",
                "8",
                "--json-output",
            ])
            .arg(&file);
        let ran = super::process::run_bounded(command, GIT_CAP, TOOL_OUTPUT_BYTES)
            .and_then(|out| out.ensure_success("devicectl"));
        let text = std::fs::read_to_string(&file);
        let _ = std::fs::remove_file(&file);
        ran.map_err(|_| "Xcode's devicectl did not answer".to_string())?;
        Ok(words::phones_in_devicectl(
            &text.map_err(|error| error.to_string())?,
        ))
    }
    #[cfg(not(target_os = "macos"))]
    Err("a real iPhone can be driven from a Mac only".to_string())
}

fn read_phone(handle: &str) -> Result<PhoneFacts, String> {
    read_phones()?
        .into_iter()
        .find(|phone| phone.handle == handle)
        .ok_or_else(|| format!("no phone {handle} is known to this Mac; run list"))
}

/// The phone with this id, made known on first use.
fn phone(handle: &str) -> Result<Arc<Phone>, ProviderError> {
    if let Some(known) = phones()
        .as_ref()
        .and_then(|known| known.get(handle))
        .cloned()
    {
        return Ok(known);
    }
    let facts = read_phone(handle).map_err(ProviderError::invalid_argument)?;
    let udid = facts.udid;
    let dir_handle = handle.to_string();
    let spawn: runner::Spawn = Arc::new(move || {
        let plan = phone_dir(&dir_handle)
            .and_then(|dir| supply::xctestrun_in(&dir))
            .ok_or_else(|| std::io::Error::other("no build stands"))?;
        runner::watched("/usr/bin/xcrun", &{
            let mut args = vec!["xcodebuild".to_string()];
            args.extend(supply::run_args(&plan, &udid));
            args
        })
        .spawn()
    });
    let life = Life::new(
        spawn,
        Arc::new(unattended_now),
        Duration::from_secs(IDLE_STOP_MINUTES * 60),
        RESTART_BACKOFF_SECS
            .iter()
            .map(|secs| Duration::from_secs(*secs))
            .collect(),
    );
    let road = Arc::new(RealRoad {
        handle: handle.to_string(),
    });
    register(handle, road, life).map_err(super::marks::backend_error)?;
    phones()
        .as_ref()
        .and_then(|known| known.get(handle))
        .cloned()
        .ok_or_else(|| ProviderError::invalid_argument("the phone did not stay known"))
}

/// Everything standing ends with the window: its pipes close, each shell ends its
/// `xcodebuild`, and nothing here waits for that.
pub(super) fn stop_all() {
    if let Some(known) = phones().as_ref() {
        for phone in known.values() {
            phone.life.stop();
        }
    }
}

// ---- Before any verb --------------------------------------------------------

fn phone_error(code: &str, sentence: impl Into<String>) -> ProviderError {
    ProviderError::new(code, sentence.into())
}

async fn on_a_worker<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ProviderError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(super::marks::backend_error)
}

fn call_error(error: CallError) -> ProviderError {
    match error {
        CallError::Transport(_) => phone_error(
            PHONE_UNREACHABLE,
            format!(
                "{PHONE_UNREACHABLE}: the phone's runner stopped answering; the next call starts it again. If this was a press, it may or may not have landed: look before pressing again."
            ),
        ),
        CallError::NoSession => phone_error(PHONE_UNREACHABLE, "the runner has no session"),
        CallError::Refused { status, word } => ProviderError::new(
            "emulator_error",
            format!("the phone's runner refused this ({status} {word})"),
        ),
    }
}

async fn locked_error(phone: &Phone) -> ProviderError {
    let road = Arc::clone(&phone.road);
    let roads = on_a_worker(move || road.locked_roads()).await;
    phone_error(
        PHONE_LOCKED,
        words::locked_sentence(roads.unwrap_or(LockedRoads {
            mirroring_installed: false,
            mirroring_running: false,
            shared_desktop: false,
        })),
    )
}

async fn up(phone: &Phone) -> Result<(), ProviderError> {
    let life = Arc::clone(&phone.life);
    match on_a_worker(move || life.ensure_up(START_WAIT)).await? {
        Ok(()) => Ok(()),
        Err(NotUp::Failed(RunnerFailure::Locked)) => Err(locked_error(phone).await),
        Err(NotUp::Failed(failure)) => Err(phone_error(
            PHONE_UNREACHABLE,
            format!(
                "{PHONE_UNREACHABLE}: {}.",
                words::runner_failure_said(failure)
            ),
        )),
        Err(NotUp::TimedOut) => Err(phone_error(
            PHONE_UNREACHABLE,
            format!(
                "{PHONE_UNREACHABLE}: the runner did not come up in {} s; ask again.",
                START_WAIT.as_secs()
            ),
        )),
        Err(NotUp::CouldNotStart) => Err(phone_error(
            PHONE_NOT_INSTALLED,
            format!(
                "{PHONE_NOT_INSTALLED}: no build stands for this phone; run phone-install --device {}.",
                phone.handle
            ),
        )),
    }
}

/// A verb's first steps, in this order: the person's answer, a build, the phone's
/// own gate, the runner, the lock. A tunnel that moved is read again, and a runner
/// that does not answer is started again, once each.
async fn driven(
    handle: &str,
) -> Result<(Arc<Phone>, tokio::sync::OwnedMutexGuard<Wda>), ProviderError> {
    let phone = phone(handle)?;
    let road = Arc::clone(&phone.road);
    let (allowed, installed) = on_a_worker(move || (road.allowed(), road.installed())).await?;
    if !allowed {
        return Err(phone_error(
            PHONE_NOT_ALLOWED,
            words::not_allowed_sentence(handle),
        ));
    }
    if !installed {
        return Err(phone_error(
            PHONE_NOT_INSTALLED,
            format!(
                "{PHONE_NOT_INSTALLED}: the person allowed this road and no build stands; run phone-install --device {handle}."
            ),
        ));
    }
    let mut wda = Arc::clone(&phone.gate).lock_owned().await;
    phone.life.touch();
    for attempt in 0..3 {
        up(&phone).await?;
        if wda.base.is_none() {
            let road = Arc::clone(&phone.road);
            match on_a_worker(move || road.base()).await? {
                Ok(base) => wda.base = Some(base),
                Err(why) => return Err(phone_error(PHONE_UNREACHABLE, why)),
            }
        }
        match wda.locked().await {
            Ok(true) => return Err(locked_error(&phone).await),
            Ok(false) => return Ok((phone, wda)),
            Err(CallError::Transport(_)) if attempt == 0 => wda.forget(),
            Err(CallError::Transport(_)) if attempt == 1 => {
                wda.forget();
                let life = Arc::clone(&phone.life);
                on_a_worker(move || life.stop_and_wait()).await?;
            }
            Err(other) => return Err(call_error(other)),
        }
    }
    Err(phone_error(
        PHONE_UNREACHABLE,
        format!(
            "{PHONE_UNREACHABLE}: the phone's runner would not answer; check the cable or the network."
        ),
    ))
}

/// The screen as the marks read it, the front app's id on its root.
async fn snapshot_on(wda: &mut Wda) -> Result<Snapshot, ProviderError> {
    let bundle = wda.active_app().await.map_err(call_error)?;
    let source = wda.source().await.map_err(call_error)?;
    let tree = words::tree_from_source(&source, bundle.as_deref());
    Snapshot::ios(&tree).map_err(super::marks::backend_error)
}

/// A gesture failed: a phone that locked meanwhile says so, and the next verb reads
/// the runner again from nothing.
async fn after_failed_act(phone: &Phone, wda: &mut Wda, error: CallError) -> ProviderError {
    if matches!(wda.locked().await, Ok(true)) {
        return locked_error(phone).await;
    }
    if matches!(error, CallError::Transport(_)) {
        wda.forget();
    }
    call_error(error)
}

fn acted() -> Value {
    json!({ "performed": true, ROAD_KEY: ROAD_WDA, MAC_POINTER_KEY: false })
}

// ---- The verbs ----------------------------------------------------------------

pub(super) async fn marks_snapshot(handle: &str) -> Result<Snapshot, ProviderError> {
    let (_phone, mut wda) = driven(handle).await?;
    snapshot_on(&mut wda).await
}

/// A press by number: the screen is read again and must still hold the mark, its
/// app and its words; a payment, a transfer or a delete stops here by the person's
/// policy, with nobody asked. The tree is read for each press, because a person's
/// finger on the same phone is what no look can foresee.
pub(super) async fn click_mark(
    handle: &str,
    request: super::marks::PinnedTap,
) -> Result<Pressed, ProviderError> {
    let (phone, mut wda) = driven(handle).await?;
    let snapshot = snapshot_on(&mut wda).await?;
    let mut point = None;
    request.perform_on_tree(&snapshot.faces, snapshot.screen, |x, y| {
        point = Some((x, y));
        Ok(())
    })?;
    let (x, y) = point.ok_or_else(|| super::marks::backend_error("no point was proven"))?;
    let (at_x, at_y) = (
        snapshot.screen.x + x * snapshot.screen.width,
        snapshot.screen.y + y * snapshot.screen.height,
    );
    if let Err(error) = wda.actions(&words::tap_actions(at_x, at_y)).await {
        return Err(after_failed_act(&phone, &mut wda, error).await);
    }
    Ok(Pressed {
        proof: Proof::Tree,
        settled: None,
    })
}

pub(crate) async fn tree(handle: &str) -> Result<Value, ProviderError> {
    let (_phone, mut wda) = driven(handle).await?;
    let bundle = wda.active_app().await.map_err(call_error)?;
    let source = wda.source().await.map_err(call_error)?;
    Ok(json!({
        "tree": words::tree_from_source(&source, bundle.as_deref()),
        "app": { "package": bundle },
    }))
}

/// A tap by position. A number is proven; a position is not, so the screen is read
/// and a guarded control under the point stops it as it stops a press by number.
pub(crate) async fn tap(handle: &str, x: f64, y: f64) -> Result<Value, ProviderError> {
    let (phone, mut wda) = driven(handle).await?;
    let snapshot = snapshot_on(&mut wda).await?;
    let (at_x, at_y) = (x * snapshot.screen.width, y * snapshot.screen.height);
    super::marks::refuse_guarded_at(&snapshot.faces, snapshot.screen, at_x, at_y)?;
    if let Err(error) = wda
        .actions(&words::tap_actions(
            snapshot.screen.x + at_x,
            snapshot.screen.y + at_y,
        ))
        .await
    {
        return Err(after_failed_act(&phone, &mut wda, error).await);
    }
    Ok(acted())
}

pub(crate) async fn swipe(
    handle: &str,
    from: (f64, f64),
    to: (f64, f64),
    ms: Option<u32>,
) -> Result<Value, ProviderError> {
    const SWIPE_MS: u32 = 300;
    let (phone, mut wda) = driven(handle).await?;
    let (width, height) = wda.window_size().await.map_err(call_error)?;
    let gesture = words::swipe_actions(
        (from.0 * width, from.1 * height),
        (to.0 * width, to.1 * height),
        ms.unwrap_or(SWIPE_MS),
    );
    if let Err(error) = wda.actions(&gesture).await {
        return Err(after_failed_act(&phone, &mut wda, error).await);
    }
    Ok(acted())
}

/// Typing: only into a field the phone says has the focus, and never a secret one.
/// A password is not an agent's to type, on a phone with nobody near it least of
/// all, so this refuses at once and the answer says so.
pub(crate) async fn text(handle: &str, text: &str) -> Result<Value, ProviderError> {
    let (phone, mut wda) = driven(handle).await?;
    match wda.focused_type().await.map_err(call_error)? {
        Some(kind) if words::is_secure_type(&kind) => {
            return Err(phone_error(
                PHONE_SECURE_FIELD,
                format!(
                    "{PHONE_SECURE_FIELD}: the focused field is a password field. Nothing was typed. A password, PIN or biometric is the person's; stop and tell them."
                ),
            ));
        }
        Some(_) => {}
        None => {
            return Err(ProviderError::invalid_argument(
                "no field has the focus on the phone; press the field first",
            ));
        }
    }
    if let Err(error) = wda.keys(text).await {
        return Err(after_failed_act(&phone, &mut wda, error).await);
    }
    Ok(acted())
}

pub(crate) async fn button(handle: &str, name: &str) -> Result<Value, ProviderError> {
    let (phone, mut wda) = driven(handle).await?;
    let done = match name {
        "home" => wda.home().await,
        "enter" => wda.keys("\n").await,
        _ => {
            return Err(ProviderError::new(
                error_code::UNSUPPORTED_CAPABILITY,
                format!("a real phone takes `home` and `enter`, not `{name}`"),
            ));
        }
    };
    if let Err(error) = done {
        return Err(after_failed_act(&phone, &mut wda, error).await);
    }
    Ok(acted())
}

pub(crate) async fn foreground(handle: &str, app: &str) -> Result<Value, ProviderError> {
    let (_phone, mut wda) = driven(handle).await?;
    let bundle = wda.active_app().await.map_err(call_error)?;
    Ok(json!({
        super::checks::COUNT_KEY: usize::from(bundle.as_deref() == Some(app)),
        "app": { "package": bundle },
    }))
}

pub(crate) async fn screenshot(handle: &str) -> Result<Vec<u8>, String> {
    let (_phone, wda) = driven(handle).await.map_err(|error| error.message)?;
    wda.screenshot()
        .await
        .map_err(|error| call_error(error).message)
}

// ---- Setup: only after the person's answer -------------------------------------

/// Where a phone's build stands, for `list`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Setup {
    Building,
    Failed(String),
}

static SETUPS: Mutex<Option<HashMap<String, Setup>>> = Mutex::new(None);

fn set_setup(handle: &str, setup: Option<Setup>) {
    let mut held = SETUPS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let map = held.get_or_insert_with(HashMap::new);
    match setup {
        Some(setup) => {
            map.insert(handle.to_string(), setup);
        }
        None => {
            map.remove(handle);
        }
    }
}

fn setup_of(handle: &str) -> Option<Setup> {
    SETUPS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|map| map.get(handle).cloned())
}

/// `list`'s rows, told what stands: an iOS row is drivable once the person allowed
/// the road and a build stands for this phone's id.
pub(super) fn overlay(rows: &mut [zerocode_core::agent_emulator::physical::PhysicalDevice]) {
    use zerocode_core::agent_emulator::physical::{LinkState, REAL_PHONE_REASON};
    let allowed = allowed_team().is_some();
    for row in rows {
        let Some(handle) = row.device.clone() else {
            continue;
        };
        match setup_of(&handle) {
            Some(Setup::Building) => row.setup = Some("building".to_string()),
            Some(Setup::Failed(why)) => row.setup = Some(why),
            None => {}
        }
        let built = phone_dir(&handle).is_some_and(|dir| supply::xctestrun_in(&dir).is_some());
        if allowed && built && row.state == LinkState::Connected {
            row.drivable = true;
            row.reason = REAL_PHONE_REASON;
        }
    }
}

struct MachineTools;

impl supply::Tools for MachineTools {
    fn run(&mut self, program: &str, args: &[String], cwd: &Path) -> Result<String, String> {
        let mut command = if program == "xcodebuild" {
            let mut command = crate::proc::quiet_command("/usr/bin/xcrun");
            command.arg("xcodebuild");
            command
        } else {
            crate::proc::quiet_command(program)
        };
        command.args(args).current_dir(cwd);
        if program == "git" {
            command
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0");
        }
        let cap = if program == "xcodebuild" {
            BUILD_CAP
        } else {
            GIT_CAP
        };
        let out = super::process::run_bounded(command, cap, TOOL_OUTPUT_BYTES)?;
        if out.status.success() {
            Ok(out.stdout_text())
        } else if program == "xcodebuild" {
            // Its log names the team, paths and ids; it stays on the Mac.
            let log = cwd.with_file_name("build.log");
            let _ = std::fs::write(&log, out.diagnostic());
            Err(format!(
                "the build failed; its log is {}. Tell the person: signing may need them in Xcode.",
                log.display()
            ))
        } else {
            Err(format!("{program} failed"))
        }
    }
}

fn read_teams() -> Vec<Team> {
    let Some(root) = data_root() else {
        return Vec::new();
    };
    let _ = std::fs::create_dir_all(&root);
    let file = root.join("xcode-prefs.plist");
    let exported = crate::proc::quiet_command("/usr/bin/defaults")
        .args(["export", "com.apple.dt.Xcode"])
        .arg(&file)
        .status()
        .is_ok_and(|status| status.success());
    let teams = exported
        .then(|| {
            crate::proc::quiet_command("/usr/bin/plutil")
                .args([
                    "-extract",
                    "IDEProvisioningTeamByIdentifier",
                    "json",
                    "-o",
                    "-",
                ])
                .arg(&file)
                .output()
                .ok()
        })
        .flatten()
        .map(|out| words::parse_teams(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default();
    let _ = std::fs::remove_file(&file);
    teams
}

/// Write the person's answer, by the only road: their press on the card.
fn commit_allowed(team: &Team) -> Result<(), String> {
    let tag = seal()
        .sealed(Said::Allowed, &team.id)
        .ok_or("the Keychain would not keep the answer, so nothing was turned on")?;
    let app = super::WINDOW.get().ok_or("no window")?;
    crate::settings_runtime::mutate_settings(
        app.state::<crate::AppState>().settings(),
        |document| {
            document.emulator_real_phone_consent = tag;
            document.emulator_real_phone_team = team.id.clone();
            // A new answer is about this team: unattended is asked for again.
            document.emulator_real_phone_unattended = String::new();
            Ok(())
        },
    )
    .map(|snapshot| announce(app, &snapshot))
}

/// The page paints the switches from the settings it holds: tell it they moved.
fn announce(app: &tauri::AppHandle, snapshot: &crate::settings_runtime::SettingsSnapshot) {
    use crate::settings_runtime::setting_key;
    crate::settings_runtime::emit_settings_changed(
        app,
        "",
        snapshot.revision,
        &[
            setting_key::EMULATOR_REAL_PHONE_CONSENT,
            setting_key::EMULATOR_REAL_PHONE_TEAM,
            setting_key::EMULATOR_REAL_PHONE_UNATTENDED,
        ],
    );
}

/// Take the person's answers back. A person's own road, so nothing stands after it.
pub(crate) fn withdraw(document: &mut crate::settings_runtime::SettingsDocument) {
    document.emulator_real_phone_consent = String::new();
    document.emulator_real_phone_team = String::new();
    document.emulator_real_phone_unattended = String::new();
}

/// The road's consent is off: runners end now, however many there are.
pub(crate) fn turned_off() {
    stop_all();
}

/// `phone-install`: one card the first time, then a build and an install in the
/// background; `list` says how it goes. With the person's answer standing for this
/// team there is no card, and with a build standing for it there is no build.
pub(crate) async fn install(handle: &str, team: Option<&str>) -> Result<Value, ProviderError> {
    let asked = team.map(str::to_string);
    let handle_owned = handle.to_string();
    let chosen = on_a_worker(move || -> Result<(PhoneFacts, Team), ProviderError> {
        let facts = read_phone(&handle_owned).map_err(ProviderError::invalid_argument)?;
        let teams = read_teams();
        let team = words::choose_team(&teams, asked.as_deref())
            .map_err(|why| phone_error(words::PHONE_TEAM_REQUIRED, why))?
            .clone();
        Ok((facts, team))
    })
    .await??;
    let (facts, team) = chosen;
    if allowed_team().as_deref() != Some(team.id.as_str()) {
        let sentence = words::consent_sentence(&team, &facts.name);
        let args = json!({
            "phone": facts.name, "team": team.name, "kind": team.kind, "tag": words::WDA_TAG,
        });
        let decision = on_a_worker(move || {
            crate::computer_use::confirm::handoff_said(
                &sentence,
                words::CONSENT_KEY,
                args,
                CARD_WAIT_MS,
            )
        })
        .await?;
        refuse_unless_allowed(decision)?;
        commit_allowed(&team).map_err(super::marks::backend_error)?;
    }
    let dir = phone_dir(handle).ok_or_else(|| super::marks::backend_error("no window"))?;
    std::fs::create_dir_all(&dir).map_err(super::marks::backend_error)?;
    let stamp = words::BuildStamp::new(supply::TAP_NO_WAIT_PATCH, &team.id, &facts.os);
    if supply::built_for(&dir, &stamp).is_some() {
        set_setup(handle, None);
        return Ok(json!({ "installed": true, "built": false }));
    }
    set_setup(handle, Some(Setup::Building));
    let building = handle.to_string();
    let udid = facts.udid;
    let os = facts.os;
    std::thread::Builder::new()
        .name("phone-build".to_string())
        .spawn(move || {
            let built = supply::prepare(&mut MachineTools, &dir, &team, &udid, &os);
            set_setup(&building, built.err().map(Setup::Failed));
        })
        .map_err(super::marks::backend_error)?;
    Ok(
        json!({ "installing": true, "watch": "run list: this phone's row says setup while it builds, and drivable when it stands" }),
    )
}

fn refuse_unless_allowed(
    decision: crate::computer_use::confirm::Decision,
) -> Result<(), ProviderError> {
    use crate::computer_use::confirm::Decision;
    match decision {
        Decision::Allowed => Ok(()),
        Decision::Refused => Err(ProviderError::new(
            error_code::CONFIRMATION_REFUSED,
            "the person said no; nothing changed. Do not ask again.",
        )),
        Decision::TimedOut => Err(ProviderError::new(
            error_code::CONFIRMATION_TIMEOUT,
            "nobody answered the card; nothing changed. Use another road or tell the person.",
        )),
    }
}

/// `phone-remove`: after the person's answer, the runner off the phone, the build
/// off the Mac, and the road off.
pub(crate) async fn remove(handle: &str) -> Result<Value, ProviderError> {
    let handle_owned = handle.to_string();
    let facts = on_a_worker(move || read_phone(&handle_owned))
        .await?
        .map_err(ProviderError::invalid_argument)?;
    let sentence = words::remove_sentence(&facts.name);
    let args = json!({ "phone": facts.name });
    let decision = on_a_worker(move || {
        crate::computer_use::confirm::handoff_said(&sentence, words::REMOVE_KEY, args, CARD_WAIT_MS)
    })
    .await?;
    refuse_unless_allowed(decision)?;
    let team = allowed_team();
    if let Ok(phone) = phone(handle) {
        let life = Arc::clone(&phone.life);
        on_a_worker(move || life.stop_and_wait()).await?;
    }
    let mut phone_note = "the phone's copy was left: no team is on record";
    if let Some(team) = &team {
        #[cfg(target_os = "macos")]
        {
            let mut command = super::ios::devicectl_command();
            command
                .args(["device", "uninstall", "app", "--device"])
                .arg(&facts.udid)
                .arg(format!("{}.xctrunner", words::runner_bundle_id(team)));
            phone_note = match super::process::run_bounded(command, GIT_CAP, TOOL_OUTPUT_BYTES)
                .and_then(|out| out.ensure_success("devicectl"))
            {
                Ok(_) => "removed from the phone",
                Err(_) => {
                    "the phone did not answer; its copy stays until the person deletes the app"
                }
            };
        }
        #[cfg(not(target_os = "macos"))]
        let _ = team;
    }
    if let Some(dir) = phone_dir(handle) {
        let _ = std::fs::remove_dir_all(dir);
    }
    set_setup(handle, None);
    let app = super::WINDOW
        .get()
        .ok_or_else(|| super::marks::backend_error("no window"))?;
    crate::settings_runtime::mutate_settings(
        app.state::<crate::AppState>().settings(),
        |document| {
            withdraw(document);
            Ok(())
        },
    )
    .map(|snapshot| announce(app, &snapshot))
    .map_err(super::marks::backend_error)?;
    Ok(json!({ "removed": true, "phone": phone_note }))
}

// ---- Readiness -----------------------------------------------------------------

/// `phone-check`: everything unattended use needs, read once. It starts the runner
/// when it may, because a runner that comes up is the only proof one will.
pub(crate) async fn check(handle: &str) -> Result<Value, ProviderError> {
    let handle_owned = handle.to_string();
    let mut items = on_a_worker(move || machine_items(&handle_owned)).await??;
    let can_start = items
        .iter()
        .filter(|item| matches!(item.name, "consent" | "build" | "connected"))
        .all(|item| item.ok == Some(true));
    if can_start {
        match driven(handle).await {
            Ok((phone, _wda)) => {
                items.push(CheckItem::new(
                    "runner",
                    Some(true),
                    phone.life.phase().word(),
                ));
                items.push(CheckItem::new("phoneLocked", Some(true), "not locked"));
            }
            Err(error) if error.code == PHONE_LOCKED => {
                items.push(CheckItem::new("runner", None, "a locked phone cannot say"));
                items.push(CheckItem::new(
                    "phoneLocked",
                    Some(false),
                    "locked: every press is refused until the person unlocks it",
                ));
            }
            Err(error) => {
                items.push(CheckItem::new("runner", Some(false), error.message));
                items.push(CheckItem::new(
                    "phoneLocked",
                    None,
                    "unreadable without a runner",
                ));
            }
        }
    } else {
        items.push(CheckItem::new(
            "runner",
            Some(false),
            "not started: consent, build and connection come first",
        ));
        items.push(CheckItem::new(
            "phoneLocked",
            None,
            "unreadable without a runner",
        ));
    }
    Ok(words::check_answer(items))
}

fn output_of(program: &str, args: &[&str]) -> Option<String> {
    crate::proc::quiet_command(program)
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn machine_items(handle: &str) -> Result<Vec<CheckItem>, ProviderError> {
    let facts = read_phone(handle).map_err(ProviderError::invalid_argument)?;
    let mut items = Vec::new();
    let reached = facts
        .tunnel
        .as_deref()
        .is_some_and(|word| word != "unavailable");
    items.push(CheckItem::new(
        "connected",
        Some(reached),
        if reached {
            "the Mac reaches the phone"
        } else {
            "the Mac cannot reach the phone now"
        },
    ));
    let developer = facts.developer_mode.as_deref() == Some("enabled");
    items.push(CheckItem::new(
        "developerMode",
        facts.developer_mode.as_ref().map(|_| developer),
        facts
            .developer_mode
            .clone()
            .unwrap_or_else(|| "the phone did not say".to_string()),
    ));
    let team = allowed_team();
    items.push(CheckItem::new(
        "consent",
        Some(team.is_some()),
        if team.is_some() {
            "the person allowed this road, until they turn it off"
        } else {
            "not allowed: phone-install puts a card to the person"
        },
    ));
    let dir = phone_dir(handle);
    let built = dir
        .as_deref()
        .is_some_and(|dir| supply::xctestrun_in(dir).is_some());
    items.push(CheckItem::new(
        "build",
        Some(built),
        if built { "stands" } else { "none" },
    ));
    let today = crate::now_epoch_ms() / 86_400_000;
    let ends = dir
        .as_deref()
        .filter(|_| built)
        .and_then(|dir| supply::signature_ends(dir, &mut MachineTools));
    let days_left = ends
        .as_deref()
        .and_then(words::iso_days)
        .map(|days| days - today);
    items.push(CheckItem::new(
        "signature",
        days_left.map(|left| left >= SIGNATURE_DAYS_FLOOR),
        days_left.map_or_else(
            || "unread: no build, or its profile could not be read".to_string(),
            |left| format!("{left} days left"),
        ),
    ));
    let unattended = unattended_now();
    items.push(CheckItem::new(
        "unattended",
        Some(unattended),
        if unattended {
            "on: the runner stays up and comes back on its own"
        } else {
            "off: the runner ends after 5 minutes unused; the person turns this on in Settings"
        },
    ));
    let roads = locked_roads_now();
    items.push(CheckItem::new(
        "mirroring",
        Some(roads.mirroring_installed),
        if !roads.mirroring_installed {
            "not on this Mac"
        } else if roads.mirroring_running {
            "the app is running; it works only on a locked phone"
        } else {
            "installed and not running; the person opens it"
        },
    ));
    items.push(CheckItem::new(
        "inputSetting",
        Some(roads.shared_desktop),
        if roads.shared_desktop {
            "shared desktop: mirroring may borrow the Mac's pointer"
        } else {
            "background only: this road does not need more; mirroring cannot press by pointer"
        },
    ));
    let pmset = output_of("/usr/bin/pmset", &["-g"]).unwrap_or_default();
    let sleep = words::sleep_minutes(&pmset, "sleep");
    items.push(CheckItem::new(
        "macSleep",
        sleep.map(|minutes| minutes == 0),
        sleep.map_or_else(
            || "unread".to_string(),
            |minutes| {
                if minutes == 0 {
                    "the Mac does not sleep on its own".to_string()
                } else {
                    format!(
                        "the Mac sleeps after {minutes} minutes idle, and the phone goes with it"
                    )
                }
            },
        ),
    ));
    let display = words::sleep_minutes(&pmset, "displaysleep");
    items.push(CheckItem::new(
        "macDisplay",
        display.map(|_| true),
        display.map_or_else(
            || "unread".to_string(),
            |minutes| format!("the display sleeps after {minutes} minutes (0 is never); this road does not need it"),
        ),
    ));
    let locked = output_of(
        "/bin/sh",
        &[
            "-c",
            "ioreg -n Root -d1 -a | plutil -extract IOConsoleLocked raw -o - -",
        ],
    );
    let locked = locked.as_deref().map(str::trim);
    items.push(CheckItem::new(
        "macScreenLock",
        locked.map(|word| word == "false"),
        match locked {
            Some("false") => "not locked",
            Some(_) => "locked: this road still works; mirroring and other desktop roads do not",
            None => "unread",
        },
    ));
    items.push(CheckItem::new(
        "phoneAutoLock",
        None,
        "this window cannot read the phone's Auto-Lock setting; a phone that locks itself stops this road until the person unlocks it",
    ));
    Ok(items)
}

#[cfg(all(test, target_os = "macos"))]
mod bench;
#[cfg(test)]
mod tests;
