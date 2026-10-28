//! TypeSafe (Jev) in the window's settings: the key Jev is asked with, and the
//! one switch a person turns it on and off with (2026-09-23,
//! docs/design/jev-settings-20260917.md §6.1).
//!
//! The key is one keychain item — `dev.zerocode.key.TYPESAFE_API_KEY`, both
//! halves spelled by `zerocode_harness` — and no launch hands it to anyone:
//! every zo, started by this window or typed into a shell, reads that item
//! itself when it first needs the key (`api::find_service_keys_in_keychain`),
//! so nothing zo spawns inherits it. The switch is `smart.jev.enabled` in zo's
//! global settings file, the key both programs' door reads; what a press
//! writes beside it is the core's (`door::switch_on`, `door::switch_off`), and
//! nothing else of that file moves. Whether a saved key works is zo's answer,
//! not this window's:
//! the check execs `zo decision-shadow check --json`, which puts the shadow's
//! own question through the same key road, request and answer check.

use std::collections::{BTreeMap, BTreeSet, btree_map};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zerocode_core::jev::custom;
use zerocode_core::jev::door::{self, JevSettings, NotAnObject};
use zerocode_core::jev::{
    AGENT_TOOL_ASK_OPTIONS, CLASSIFIER_SETTING, ClassifierMode, DEFAULT_MODEL, JEV_USES, JevMode,
    JevUse, MODEL_SETTING, ROUTING, SMART_SETTINGS_KEY, choice, count, model_in, noul, pin_in,
    pinned_model, promote, score,
};
use zerocode_harness::{SERVICE_KEYCHAIN_SERVICE_PREFIX, TYPESAFE_API_KEY_ENV};

use crate::api_routers::{RouterKeys, RouterRefusal};

/// The zo command line that checks the saved key (`zo decision-shadow check`).
pub const ZO_KEY_CHECK_ARGS: [&str; 3] = ["decision-shadow", "check", "--json"];
pub const ZO_CHECK_PROVIDER_FLAG: &str = "--judgment-provider";

#[must_use]
pub fn key_check_args(provider: &judgment_provider::Provider) -> Vec<String> {
    let mut args = ZO_KEY_CHECK_ARGS.map(str::to_string).to_vec();
    if provider.kind != judgment_provider::ProviderKind::Jev {
        args.extend([ZO_CHECK_PROVIDER_FLAG.to_string(), provider.identity()]);
    }
    args
}

/// The zo command line that counts every seat's ledger (`zo jev summary`).
///
/// The window asks rather than counting: these files are zo's to read, the
/// judge that promotes a seat reads them the same way (§4), and a card that
/// counted for itself would be free to disagree with the seat it is drawing.
pub const ZO_JEV_SUMMARY_ARGS: [&str; 3] = ["jev", "summary", "--json"];
/// The flag that hands zo the window's Computer Use sessions folder, where the
/// screen seats (browser, desktop, emulator) append beside each walk's
/// evidence — without it the card read all three as "never asked" with rows
/// on disk (2026-09-20).
pub const ZO_JEV_SUMMARY_SESSIONS_FLAG: &str = "--computer-use";
/// The flag that names the project whose ledgers zo counts. Without it zo
/// counted the window process's own working directory — never a project —
/// and the card said "nothing asked yet" of a routing seat with 28 rows
/// (2026-09-20, the person's screenshot).
pub const ZO_JEV_SUMMARY_CWD_FLAG: &str = "--cwd";
/// The flag that asks zo to list each seat's last requests under its
/// numbers — what the dashboard shows beside the table
/// (docs/design/jev-dashboard-and-perfection-20260921.md §2 (b)). The card
/// never passes it: it draws the numbers, and a list it would not draw is
/// bytes across the exec boundary for nothing.
pub const ZO_JEV_SUMMARY_RECENT_FLAG: &str = "--recent";

/// The keychain item the key lives in.
#[must_use]
pub fn typesafe_keychain_service() -> String {
    format!("{SERVICE_KEYCHAIN_SERVICE_PREFIX}{TYPESAFE_API_KEY_ENV}")
}

/// One mode a switch offers, as the pane lists it: the word it writes and what
/// the mode does. The pane words a choice by what it does, so no mode word is
/// spelled anywhere but the Jev use table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeChoice {
    pub mode: &'static str,
    pub asks: bool,
    pub applies: bool,
    pub automatic: bool,
}

impl ModeChoice {
    fn of(mode: JevMode) -> Self {
        Self {
            mode: mode.key(),
            asks: mode.asks(),
            applies: mode.applies(),
            automatic: mode.automatic(),
        }
    }
}

/// One feature Jev judges for, as the dashboard paints it: the use's own name
/// — which is also the key its words are looked up under — the settings key
/// a person may write by hand, where it stands now, and the modes it offers,
/// each by what it does.
///
/// There is one of these per row of [`JEV_USES`] and no field named after any
/// single use, so a feature added to the table arrives on the dashboard with
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchRow {
    pub id: &'static str,
    #[serde(skip_serializing_if = "zerocode_core::jev::builtin::Decision::is_empty")]
    pub decision: zerocode_core::jev::builtin::Decision,
    pub setting: &'static str,
    pub mode: &'static str,
    pub modes: Vec<ModeChoice>,
    /// Whether a word a person wrote governs this feature
    /// ([`JevUse::word_in`]): its own (`smart.<setting>`), or — for a feature
    /// split off another, while it has none of its own — the one written for
    /// that other (t-6877, [`JevUse::follows`]). A hand-written choice, which
    /// the switch does not govern until the next press on (§6.1).
    pub written: bool,
    /// How many compared marks the judge wants before the seat's accuracy
    /// may speak ([`JevUse::agreement_rows_wanted`]) — the sample the
    /// dashboard fills its bar toward, and under half of which it says the
    /// sample rather than a share (t-6243 D2/D3). `None` for a seat that
    /// never rises.
    pub agreement_rows_wanted: Option<usize>,
    /// The wall the feature's own words name, in milliseconds
    /// ([`zerocode_core::jev::worded_wall_ms`]) — what a `{{seconds}}` in its
    /// paragraph reads, so no catalog writes the number down (t-45961).
    /// `None` for a feature whose words name none.
    pub wall_ms: Option<u64>,
}

/// One word the routing classifier may hold, as the pane lists it: the word it
/// writes and what that word DOES. The pane words a choice by what it does, so
/// no classifier word is spelled anywhere but the core table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierChoice {
    pub mode: &'static str,
    pub runs: bool,
    pub markers: bool,
    pub probes: bool,
    /// Whether the routing seat can be asked under this word (t-6346).
    pub reaches: bool,
}

impl ClassifierChoice {
    fn of(mode: ClassifierMode) -> Self {
        Self {
            mode: mode.key(),
            runs: mode.runs(),
            markers: mode.markers(),
            probes: mode.probes(),
            reaches: mode.reaches(),
        }
    }
}

/// The gate in front of the routing seat, as the pane paints it: the settings
/// key it writes, where it stands, whether where it stands calls the chat
/// probe and whether it reaches the seat at all (every word but `off`,
/// t-6346), and the words it offers.
///
/// It is not a Jev use and has no row in that table — it is zo's own routing
/// setting. It is answered here because it is the one fact the routing row
/// cannot tell the truth without: `smart.decisionShadow` may say the seat
/// applies while this says nothing is ever asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierRow {
    pub setting: &'static str,
    pub mode: &'static str,
    pub probes: bool,
    /// Whether the seat this gate stands in front of can be asked at all.
    pub reaches: bool,
    /// The seat this gate stands in front of, by that use's own name — so the
    /// card reads which row to warn on rather than carrying a second copy of
    /// the coupling.
    pub gates: &'static str,
    pub modes: Vec<ClassifierChoice>,
}

/// The model every request names, as the pane paints it (t-6187): the
/// settings key it writes, the model the door asks for now, whether a person
/// pinned it, and the alias an unpinned door asks — so the card never spells
/// the alias itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub setting: &'static str,
    pub model: String,
    pub pinned: bool,
    pub alias: &'static str,
}

impl ModelRow {
    fn of(root: &Map<String, Value>) -> Self {
        // Read by the core's own readers, as the door reads it: a value the
        // door would ignore is shown as no pin.
        let root = Value::Object(root.clone());
        Self {
            setting: MODEL_SETTING,
            model: model_in(&root).to_string(),
            pinned: pin_in(&root).is_some(),
            alias: DEFAULT_MODEL,
        }
    }
}

/// The one switch (§6.1), as the card and the dashboard paint it: whether Jev
/// is in use here, and where it may send from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JevSwitch {
    /// Something is sent: the door lets a request through
    /// ([`JevSettings::enabled`]), some feature asks, and some folder is
    /// consented. Read off what each program will do rather than off
    /// `smart.jev.enabled` alone, because a machine whose features were set
    /// by hand before the switch existed never wrote it and does send — and
    /// a switch that said otherwise would be one a person cannot trust.
    pub on: bool,
    /// Every folder is consented — the switch's own consent.
    pub everywhere: bool,
    /// Folders consented by name.
    pub folders: usize,
}

impl JevSwitch {
    pub(crate) fn of(document: &Value) -> Self {
        let door = JevSettings::from_root(document);
        let folders = door.folders().count();
        let consented = door.everywhere() || folders > 0;
        let asks = JEV_USES.iter().any(|row| row.mode_in(document).asks());
        Self {
            on: door.enabled && asks && consented,
            everywhere: door.everywhere(),
            folders,
        }
    }
}

/// What the pane paints: whether a key could be kept here, whether one is
/// saved — never the key itself — the one switch, and every feature as its
/// reader reads it, with the modes it offers in the table's order.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeSafeSettings {
    pub keys_kept_here: bool,
    pub key_saved: bool,
    pub provider: judgment_provider::Provider,
    pub provider_identity: String,
    pub provider_connection: Option<judgment_provider::connections::Reference>,
    /// The name the person gave that connection, so the screens say which
    /// connection judges instead of the provider's identity (t-76598).
    pub provider_connection_name: Option<String>,
    pub jev_provider: judgment_provider::Provider,
    /// The one switch: whether Jev is in use here.
    pub jev: JevSwitch,
    /// The model every seat's request names — the pin, or the alias.
    pub model: ModelRow,
    /// The launch rows a person wrote, and nothing else (t-14437): a field
    /// left empty follows today's lineup.
    pub summon_profiles: Value,
    /// What each agent's summons runs at each difficulty today — the
    /// person's row, else the lineup's, else the shipped table's — and what
    /// else the lineup offers there.
    pub summon_rows:
        std::collections::BTreeMap<String, Vec<zerocode_core::summon_difficulty::lineup::Row>>,
    /// Every model each agent's lineup offers today, the newly arrived
    /// marked: what a written row's model is picked from.
    pub summon_lineup:
        std::collections::BTreeMap<String, Vec<zerocode_core::summon_difficulty::lineup::Pick>>,
    /// Whether each agent with rows could be summoned now — its CLI on the
    /// launch PATH, its login — as the readiness table last saw it.
    pub summon_agents: std::collections::BTreeMap<String, SummonAgent>,
    /// When the lineup was read and what each source answered, and the
    /// person's connections, usable or not.
    pub summon_sources: crate::summon_lineup::Sources,
    /// Every row of the use table, in the table's order — what the dashboard
    /// reads each feature's standing off.
    pub switches: Vec<SwitchRow>,
    /// The routing classifier, which decides whether the routing seat is asked
    /// anything at all.
    pub classifier: ClassifierRow,
    /// What the editor of the person's own decisions offers and bounds.
    pub custom_decisions: CustomDecisionRules,
}

/// One agent as the readiness table last saw it (t-3996); `None` fields
/// when nobody has looked yet — the screen then says nothing, which
/// accuses nobody.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummonAgent {
    /// Its CLI was on the PATH a launch would use.
    pub installed: Option<bool>,
    /// The login witnesses' word: `authorized`, `unauthorized`, `unknown`.
    pub auth: Option<&'static str>,
    /// What the witnesses saw, for a hover.
    pub evidence: Option<String>,
    /// It runs any provider's model (zo), so the person's connections are
    /// its to pick from; an agent tied to one provider's CLI takes only that.
    pub any_provider: bool,
}

impl SummonAgent {
    fn of(agent: &str, snapshot: Option<zerocode_core::readiness::AgentReadinessSnapshot>) -> Self {
        Self {
            any_provider: zerocode_core::agent::agent_voice(agent)
                .models_provider
                .is_none(),
            installed: snapshot.as_ref().map(|seen| {
                matches!(
                    seen.binary,
                    zerocode_core::readiness::BinaryState::Present { .. }
                )
            }),
            auth: snapshot.as_ref().map(|seen| seen.auth.as_str()),
            evidence: snapshot
                .map(|seen| seen.evidence)
                .filter(|evidence| !evidence.is_empty()),
        }
    }
}

/// Every agent's launch rows against `lineup`; an agent with none is left
/// out, and a written row that cannot launch leaves its agent out too — the
/// summons refuses it by name, and the screen shows the field as written.
fn summon_rows(
    document: &Value,
    lineup: &crate::summon_lineup::Snapshot,
    now_ms: i64,
) -> std::collections::BTreeMap<String, Vec<zerocode_core::summon_difficulty::lineup::Row>> {
    zerocode_core::agent::AGENT_SPECS
        .iter()
        .filter_map(|spec| {
            let rows = zerocode_core::summon_difficulty::lineup::rows(
                document,
                spec.id,
                lineup.lineups.get(spec.id),
                Some(&lineup.seen),
                now_ms,
            )
            .ok()?;
            (!rows.is_empty()).then(|| (spec.id.to_string(), rows))
        })
        .collect()
}

/// The pane's state, read from the keychain and zo's settings file.
///
/// # Errors
/// An unreadable keychain or settings file.
pub fn read_settings(
    path: &Path,
    keys: &dyn RouterKeys,
    keys_kept_here: bool,
) -> Result<TypeSafeSettings, RouterRefusal> {
    let root = crate::api_routers::read_zo_settings_root(path)?;
    let classifier = classifier_in(&root);
    // Read by the core's own readers, as zo and the door read it: a seat
    // with no word of its own follows the switch (§6.1).
    let document = Value::Object(root.clone());
    let provider = judgment_provider::Provider::from_root(&document)
        .map_err(|reason| RouterRefusal::from(reason.token().to_string()))?;
    let key_saved = provider.requires_auth
        && if provider.connection.is_some() {
            provider_key(&provider, keys)?.is_some_and(|key| !key.trim().is_empty())
        } else {
            key_saved_at(&provider_keychain_service(&provider), keys)?
        };
    let lineup = crate::summon_lineup::snapshot();
    let now_ms = crate::usage_runtime::epoch_ms_now();
    let summon_rows = summon_rows(&document, &lineup, now_ms);
    Ok(TypeSafeSettings {
        keys_kept_here,
        key_saved,
        provider_identity: provider.identity(),
        provider_connection: provider
            .connection
            .as_ref()
            .map(|connection| connection.reference.clone()),
        provider_connection_name: provider.connection.as_ref().and_then(|connection| {
            judgment_provider::connections::row(&document, &connection.reference.id)
                .ok()?
                .get("name")?
                .as_str()
                .map(str::to_string)
        }),
        provider,
        jev_provider: judgment_provider::Provider::jev(model_in(&document)),
        jev: JevSwitch::of(&document),
        model: ModelRow::of(&root),
        summon_profiles: zerocode_core::summon_difficulty::written_profiles(&document),
        // A peek: the readiness table's door for a reader that may not
        // spawn a witness (the window's settings command looks first).
        summon_agents: summon_rows
            .keys()
            .map(|agent| {
                (
                    agent.clone(),
                    SummonAgent::of(agent, crate::readiness_runtime::observe(agent)),
                )
            })
            .collect(),
        summon_sources: crate::summon_lineup::sources(),
        summon_rows,
        summon_lineup: lineup
            .lineups
            .iter()
            .map(|(agent, held)| (agent.clone(), held.picks(agent, Some(&lineup.seen), now_ms)))
            .collect(),
        switches: JEV_USES
            .iter()
            .map(|row| SwitchRow {
                id: row.id,
                decision: zerocode_core::jev::builtin::in_root(&document, row.id)
                    .unwrap_or_default(),
                setting: row.setting,
                mode: row.mode_in(&document).key(),
                modes: choices(row),
                written: row.word_in(&document).is_some(),
                agreement_rows_wanted: row.agreement_rows_wanted,
                wall_ms: zerocode_core::jev::worded_wall_ms(row),
            })
            .collect(),
        classifier: ClassifierRow {
            setting: CLASSIFIER_SETTING,
            mode: classifier.key(),
            probes: classifier.probes(),
            reaches: classifier.reaches(),
            gates: ROUTING.id,
            modes: ClassifierMode::ALL.map(ClassifierChoice::of).to_vec(),
        },
        custom_decisions: CustomDecisionRules::of(&document),
    })
}

/// The classifier's word in a settings document, read by the core table's own
/// parser — including its absence, which is not the same as its default.
fn classifier_in(root: &Map<String, Value>) -> ClassifierMode {
    ClassifierMode::of(
        root.get(SMART_SETTINGS_KEY)
            .and_then(|smart| smart.get(CLASSIFIER_SETTING)),
    )
}

/// The modes a use's switch offers, in the table's order.
fn choices(row: &JevUse) -> Vec<ModeChoice> {
    row.modes.iter().copied().map(ModeChoice::of).collect()
}

/// Whether the keychain item `service` holds a key — read, and the key itself
/// dropped where it was read, so a pane is only ever told yes or no. The one
/// reading of a saved key for every card that keeps one: this one's, and the
/// Computer Use pane's (t-9537).
///
/// # Errors
/// An unreadable keychain.
pub fn key_saved_at(service: &str, keys: &dyn RouterKeys) -> Result<bool, RouterRefusal> {
    Ok(keys
        .read(service)?
        .is_some_and(|key| !key.trim().is_empty()))
}

/// The window's one guard over the keychain items it keeps keys in. Every
/// save and removal of a key takes it, and so does a move of a plain-text
/// secret (`settings_secrets`) across its look at the item and its write — so
/// no save from the settings, another page or another window lands between a
/// look that found the item empty and a write that would cover it (t-76698).
/// The keychain's own write is two `security` runs and has its own lock
/// (`accounts::KEYCHAIN_WRITES`); this one spans the look before it.
static KEY_ITEMS: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    /// Whether this thread holds [`KEY_ITEMS`]: a second take on the same
    /// thread would wait on itself.
    static KEY_ITEMS_HERE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`KEY_ITEMS`], held.
struct KeyItems {
    _held: std::sync::MutexGuard<'static, ()>,
}

impl KeyItems {
    fn take() -> Self {
        debug_assert!(
            !KEY_ITEMS_HERE.with(std::cell::Cell::get),
            "the key guard is taken once per thread"
        );
        let held = KEY_ITEMS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        KEY_ITEMS_HERE.with(|here| here.set(true));
        Self { _held: held }
    }
}

impl Drop for KeyItems {
    fn drop(&mut self) {
        KEY_ITEMS_HERE.with(|here| here.set(false));
    }
}

/// Whether this thread holds the window's guard over keychain items — asked
/// by the secret move's tests, which run where a keychain keeps keys.
#[cfg(all(test, unix))]
pub(crate) fn key_items_held_here() -> bool {
    KEY_ITEMS_HERE.with(std::cell::Cell::get)
}

/// Keep `key` in the keychain item `service`, trimmed. An empty key is refused
/// in `empty`, the card's own sentence, rather than saved as "configured,
/// empty".
///
/// # Errors
/// An empty key, a machine with no keychain, or a refused keychain write.
pub fn save_key_at(
    service: &str,
    key: &str,
    empty: &str,
    keys: &dyn RouterKeys,
) -> Result<(), RouterRefusal> {
    let _held = KeyItems::take();
    save_key_held(service, key, empty, keys)
}

/// [`save_key_at`]'s write, for a caller holding [`KeyItems`].
fn save_key_held(
    service: &str,
    key: &str,
    empty: &str,
    keys: &dyn RouterKeys,
) -> Result<(), RouterRefusal> {
    let key = key.trim();
    if key.is_empty() {
        return Err(empty.to_string().into());
    }
    keys.write(service, key)
}

/// Why [`save_key_unless_another_at`] kept nothing.
#[derive(Debug)]
pub(crate) enum KeptNot<E> {
    /// The item already holds another value; it is left as it is.
    Another,
    /// The caller's last look, just before the write, refused.
    Changed(E),
    /// The keychain refused the look or the write.
    Refused(RouterRefusal),
}

/// [`save_key_at`], unless the item already holds another value: the look
/// and the write are one step under the guard every key save takes, so a
/// value saved in between is never covered. An item already holding `key` is
/// left as it is. `still` is the caller's last look, asked under the guard
/// just before the write (or before answering that the item holds `key`).
///
/// # Errors
/// [`KeptNot`]: another value held, `still` refusing, or the keychain.
pub(crate) fn save_key_unless_another_at<E>(
    service: &str,
    key: &str,
    empty: &str,
    keys: &dyn RouterKeys,
    still: impl FnOnce() -> Result<(), E>,
) -> Result<(), KeptNot<E>> {
    let _held = KeyItems::take();
    let held = keys.read(service).map_err(KeptNot::Refused)?;
    if held.as_deref().is_some_and(|held| held != key) {
        return Err(KeptNot::Another);
    }
    still().map_err(KeptNot::Changed)?;
    if held.is_some() {
        return Ok(());
    }
    save_key_held(service, key, empty, keys).map_err(KeptNot::Refused)
}

/// Forget the key kept in the keychain item `service`. Nothing saved is not
/// an error.
///
/// # Errors
/// A refused keychain deletion.
pub fn remove_key_at(service: &str, keys: &dyn RouterKeys) -> Result<(), RouterRefusal> {
    let _held = KeyItems::take();
    keys.delete(service)
}

/// Keep `key` in the keychain item zo reads, trimmed. An empty key is refused
/// rather than saved as "configured, empty".
///
/// # Errors
/// An empty key, a machine with no keychain, or a refused keychain write.
pub fn save_key(key: &str, keys: &dyn RouterKeys) -> Result<(), RouterRefusal> {
    save_key_at(
        &typesafe_keychain_service(),
        key,
        "TypeSafe API 키가 비어 있습니다",
        keys,
    )
}

/// Forget the saved key. Nothing saved is not an error.
///
/// # Errors
/// A refused keychain deletion.
pub fn remove_key(keys: &dyn RouterKeys) -> Result<(), RouterRefusal> {
    remove_key_at(&typesafe_keychain_service(), keys)
}

#[must_use]
pub fn provider_keychain_service(provider: &judgment_provider::Provider) -> String {
    let prefix = if provider.connection.is_some() {
        zerocode_harness::ROUTER_KEYCHAIN_SERVICE_PREFIX
    } else {
        SERVICE_KEYCHAIN_SERVICE_PREFIX
    };
    format!("{prefix}{}", provider.key_env())
}

/// A linked connection reads its existing reference. No legacy hash-key fallback.
pub(crate) fn provider_key(
    provider: &judgment_provider::Provider,
    keys: &dyn RouterKeys,
) -> Result<Option<String>, RouterRefusal> {
    if let Some(connection) = &provider.connection {
        return crate::api_routers::read_connection_key(&connection.auth_env, keys);
    }
    keys.read(&provider_keychain_service(provider))
}

/// Capture a linked row and its key under the same lock used by saves.
/// An edited row cannot supply its new credential to an older request endpoint.
pub(crate) fn provider_key_at(
    path: &Path,
    provider: &judgment_provider::Provider,
    keys: &dyn RouterKeys,
) -> Result<Option<String>, RouterRefusal> {
    let Some(connection) = &provider.connection else {
        return provider_key(provider, keys);
    };
    crate::api_routers::with_zo_settings_root(path, |root| {
        let current = judgment_provider::connections::candidate(
            &Value::Object(root.clone()),
            &connection.reference,
        )
        .map_err(|error| error.to_string())?;
        if current.identity() != provider.identity() {
            return Err("settings_changed".to_string());
        }
        provider_key(provider, keys).map_err(|error| error.message)
    })
    .map_err(RouterRefusal::from)
}

pub fn selected_provider(path: &Path) -> Result<judgment_provider::Provider, RouterRefusal> {
    let root = crate::api_routers::read_zo_settings_root(path)?;
    judgment_provider::Provider::from_root(&Value::Object(root))
        .map_err(|reason| RouterRefusal::from(reason.token().to_string()))
}

fn selected_key_service(path: &Path, expected: Option<&str>) -> Result<String, RouterRefusal> {
    let provider = selected_provider(path)?;
    require_provider(&provider, expected)?;
    if provider.connection.is_some() {
        return Err("connection_in_use".to_string().into());
    }
    Ok(provider_keychain_service(&provider))
}

pub fn require_provider(
    provider: &judgment_provider::Provider,
    expected: Option<&str>,
) -> Result<(), String> {
    if expected.is_some_and(|identity| identity != provider.identity())
        || (expected.is_none() && provider.kind != judgment_provider::ProviderKind::Jev)
    {
        return Err("settings_changed".to_string());
    }
    Ok(())
}

pub fn save_selected_key(
    path: &Path,
    expected: Option<&str>,
    key: &str,
    keys: &dyn RouterKeys,
) -> Result<(), RouterRefusal> {
    with_provider_locked(path, || {
        Ok(selected_key_service(path, expected).and_then(|service| {
            if service == typesafe_keychain_service() {
                save_key(key, keys)
            } else {
                save_key_at(&service, key, "API 키가 비어 있습니다", keys)
            }
        }))
    })
    .map_err(RouterRefusal::from)?
}

pub(crate) fn save_candidate_key(
    path: &Path,
    provider: &judgment_provider::Provider,
    expected: Option<&str>,
    key: &str,
    keys: &dyn RouterKeys,
) -> Result<(), RouterRefusal> {
    with_provider_locked(path, || {
        Ok(selected_provider(path).and_then(|current| {
            if current.connection.is_some()
                && serde_json::to_value(&current).ok() == serde_json::to_value(provider).ok()
            {
                return Err("connection_in_use".to_string().into());
            }
            if let Some(expected) = expected {
                require_provider(&current, Some(expected))?;
            }
            save_key_at(
                &provider_keychain_service(provider),
                key,
                "API 키가 비어 있습니다",
                keys,
            )
        }))
    })
    .map_err(RouterRefusal::from)?
}

pub fn remove_selected_key(
    path: &Path,
    expected: Option<&str>,
    keys: &dyn RouterKeys,
) -> Result<(), RouterRefusal> {
    with_provider_locked(path, || {
        Ok(selected_key_service(path, expected).and_then(|service| {
            if service == typesafe_keychain_service() {
                remove_key(keys)
            } else {
                remove_key_at(&service, keys)
            }
        }))
    })
    .map_err(RouterRefusal::from)?
}

pub fn set_provider(path: &Path, provider: judgment_provider::Provider) -> Result<(), String> {
    set_provider_checked(path, provider, None, None)
}

pub(crate) fn set_provider_if_identity(
    path: &Path,
    provider: judgment_provider::Provider,
    expected: &str,
    binding: Option<&local_judgment::Binding>,
) -> Result<(), String> {
    set_provider_checked(path, provider, Some(expected), binding)
}

/// Commit an explicitly applied connection and its tested key together.
/// The active-provider check runs before the key is touched, under the same
/// settings lock the router uses, and a failed file write restores the key.
pub(crate) fn set_provider_with_key_if_identity(
    path: &Path,
    provider: judgment_provider::Provider,
    expected: &str,
    key: &str,
    keys: &dyn RouterKeys,
) -> Result<(), RouterRefusal> {
    let provider = applicable(provider)?;
    let key = key.trim();
    if !provider.requires_auth || key.is_empty() {
        return Err("provider_auth_not_required".to_string().into());
    }
    crate::api_routers::update_zo_settings_root_with_key(
        path,
        &provider_keychain_service(&provider),
        key,
        keys,
        |root| write_provider_to_root(root, &provider, Some(expected), None),
    )
}

pub(crate) fn with_provider_locked<R>(
    path: &Path,
    read: impl FnOnce() -> Result<R, String>,
) -> Result<R, String> {
    crate::api_routers::with_zo_settings_root(path, |_| read())
}

/// `provider` as an apply would write it, or the token it is refused with —
/// one reading for the apply and the preflight, so a candidate the preflight
/// asked is one the apply accepts.
fn applicable(
    provider: judgment_provider::Provider,
) -> Result<judgment_provider::Provider, String> {
    let provider = provider.validated().map_err(|reason| reason.to_string())?;
    if provider.kind == judgment_provider::ProviderKind::Jev
        && zerocode_core::jev::pinned_model(&provider.model).is_none()
    {
        return Err(judgment_provider::Refusal::Model.token().to_string());
    }
    Ok(provider)
}

fn set_provider_checked(
    path: &Path,
    provider: judgment_provider::Provider,
    expected: Option<&str>,
    binding: Option<&local_judgment::Binding>,
) -> Result<(), String> {
    let provider = applicable(provider)?;
    if binding.is_some_and(|binding| !binding.matches(&provider)) {
        return Err("managed_model_binding".to_string());
    }
    crate::api_routers::update_zo_settings_root(path, |root| {
        write_provider_to_root(root, &provider, expected, binding)
    })
}

fn write_provider_to_root(
    root: &mut Map<String, Value>,
    provider: &judgment_provider::Provider,
    expected: Option<&str>,
    binding: Option<&local_judgment::Binding>,
) -> Result<(), String> {
    if let Some(expected) = expected {
        let current = judgment_provider::Provider::from_root(&Value::Object(root.clone()))
            .map_err(|reason| reason.to_string())?;
        if current.identity() != expected {
            return Err("settings_changed".to_string());
        }
    }
    let smart = root
        .entry(SMART_SETTINGS_KEY)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("provider_settings")?;
    smart.remove(judgment_provider::connections::JUDGMENT_CONNECTION_KEY);
    smart.remove(judgment_provider::connections::PREVIOUS_JUDGMENT_KEY);
    if provider.kind == judgment_provider::ProviderKind::Jev {
        if provider.model == DEFAULT_MODEL {
            smart.remove(MODEL_SETTING);
        } else {
            smart.insert(
                MODEL_SETTING.to_string(),
                Value::String(provider.model.clone()),
            );
        }
    }
    smart.insert(
        judgment_provider::SETTINGS_KEY.to_string(),
        serde_json::to_value(provider).map_err(|error| error.to_string())?,
    );
    if let Some(binding) = binding {
        smart.insert(
            local_judgment::MANAGED_MODEL_KEY.to_string(),
            serde_json::to_value(binding).map_err(|error| error.to_string())?,
        );
    } else {
        smart.remove(local_judgment::MANAGED_MODEL_KEY);
    }
    Ok(())
}

/// Turn Jev on or off (§6.1) in zo's settings file, under the window's
/// settings lock and by one atomic replace, leaving every key the press does
/// not own as it stood. What a press writes is the core's
/// ([`door::switch_on`], [`door::switch_off`]), where the readers of it live.
///
/// # Errors
/// A `smart` or `smart.jev` that is not an object (refused rather than
/// overwritten), or an unreadable or unwritable file.
pub fn set_enabled(path: &Path, on: bool) -> Result<(), String> {
    update_smart(path, |smart| {
        if on {
            door::switch_on(smart)
        } else {
            door::switch_off(smart)
        }
        .map_err(|NotAnObject(key)| {
            format!(
                "settings.json의 {SMART_SETTINGS_KEY}.{key}가 JSON 객체가 아니라 바꾸지 않았습니다"
            )
        })
    })
}

/// Change `smart` in zo's settings file and nothing else — the one door the
/// switch, the classifier and the model pin write through. A missing `smart`
/// is made; one that is not an object is refused rather than overwritten, and
/// a change that refuses writes nothing.
fn update_smart(
    path: &Path,
    change: impl FnOnce(&mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), String> {
    crate::api_routers::update_zo_settings_root(path, |root| {
        let smart = root
            .entry(SMART_SETTINGS_KEY)
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(smart) = smart else {
            return Err(format!(
                "settings.json의 {SMART_SETTINGS_KEY}가 JSON 객체가 아니라 바꾸지 않았습니다"
            ));
        };
        change(smart)
    })
}

/// The editor keeps question structure and saved overrides separate.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinDecisionEditor {
    pub id: String,
    pub saved: zerocode_core::jev::builtin::Decision,
    pub questions: Vec<zerocode_core::jev::builtin::QuestionDefinition>,
    pub can_edit_prompt: bool,
    pub dynamic: bool,
    pub mode: String,
    pub default_mode: String,
    pub modes: Vec<ModeChoice>,
    pub name_max: usize,
    pub question_max: usize,
    pub legacy_question_max: usize,
    pub criteria_max: usize,
}

pub fn builtin_decision_editor(
    path: &Path,
    id: &str,
    questions: Vec<zerocode_core::jev::builtin::QuestionDefinition>,
) -> Result<BuiltinDecisionEditor, String> {
    use zerocode_core::jev::builtin;
    let row = builtin::seat(id).ok_or("builtin_decision_missing")?;
    let document = Value::Object(crate::api_routers::read_zo_settings_root(path)?);
    let mut defaults = document.clone();
    if let Some(choices) = defaults
        .get_mut(SMART_SETTINGS_KEY)
        .and_then(|smart| smart.get_mut(builtin::SETTING))
        .and_then(Value::as_object_mut)
    {
        choices.remove(id);
    }
    if let Some(smart) = defaults
        .get_mut(SMART_SETTINGS_KEY)
        .and_then(Value::as_object_mut)
    {
        smart.remove(row.setting);
    }
    let dynamic = questions.is_empty();
    Ok(BuiltinDecisionEditor {
        id: id.to_string(),
        saved: builtin::in_root(&document, id).unwrap_or_default(),
        questions,
        can_edit_prompt: id != zerocode_core::jev::JUDGMENT_CACHE.id,
        dynamic,
        mode: row.mode_in(&document).key().to_string(),
        default_mode: row.mode_in(&defaults).key().to_string(),
        modes: choices(row),
        name_max: builtin::NAME_MAX,
        question_max: builtin::QUESTION_MAX,
        legacy_question_max: builtin::LEGACY_QUESTION_MAX,
        criteria_max: builtin::CRITERIA_MAX,
    })
}

/// One settings lock covers the stale-editor check and the whole edit.
/// Deletion is a persistent tombstone; reset alone forgets the override.
pub fn update_builtin_decision(
    path: &Path,
    id: &str,
    action: &str,
    expected: &zerocode_core::jev::builtin::Decision,
    draft: Option<zerocode_core::jev::builtin::Decision>,
) -> Result<(), String> {
    use zerocode_core::jev::builtin;
    let row = builtin::seat(id).ok_or("builtin_decision_missing")?;
    update_smart(path, |smart| {
        let root = serde_json::json!({(SMART_SETTINGS_KEY): &*smart});
        let current = builtin::in_root(&root, id).unwrap_or_default();
        if &current != expected {
            return Err("builtin_decision_changed".to_string());
        }
        let mut next = match action {
            "save" => draft.ok_or("builtin_decision_draft")?,
            "delete" => builtin::Decision {
                deleted: true,
                ..current.clone()
            },
            "restore" => builtin::Decision {
                deleted: false,
                ..current.clone()
            },
            "reset" => builtin::Decision::default(),
            _ => return Err("builtin_decision_action".to_string()),
        };
        if action == "delete" && next.mode.is_none() {
            next.mode = row
                .word_in(&root)
                .map(|word| row.mode_of(Some(word)).key().to_string());
        }
        if action == "save" {
            next.deleted = false;
            for text in [&mut next.name, &mut next.question, &mut next.criteria] {
                *text = text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string);
            }
            // A changed question starts recording. Its previous automatic
            // promotion says nothing about these new evaluation criteria.
            if (next.question != current.question
                || next.criteria != current.criteria
                || next.questions != current.questions)
                && next.edited()
            {
                next.mode = Some(
                    if row.modes.contains(&JevMode::Shadow) {
                        JevMode::Shadow
                    } else {
                        JevMode::Off
                    }
                    .key()
                    .to_string(),
                );
            }
        }
        builtin::validate(id, &next).map_err(str::to_string)?;
        let legacy = next.legacy_mode(row);
        let decisions = smart
            .entry(builtin::SETTING)
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or("builtin_decision_settings")?;
        if next.is_empty() {
            decisions.remove(id);
        } else {
            decisions.insert(
                id.to_string(),
                builtin::storage_value(&next).map_err(|error| error.to_string())?,
            );
        }
        if action == "reset" || legacy.is_none() {
            smart.remove(row.setting);
        } else if let Some(mode) = legacy {
            smart.insert(
                row.setting.to_string(),
                Value::String(mode.key().to_string()),
            );
        }
        Ok(())
    })
}

/// Set the routing classifier to one of its four words, leaving every other
/// key as it stood.
///
/// Its own door, beside the switch's ([`set_enabled`]), because it is not a
/// feature and writes a different key with a different vocabulary. A word the table does not offer is refused rather than written:
/// zo reads an unknown word as the provider-free verdict, so a typo here would
/// quietly stop the routing seat being asked anything.
///
/// # Errors
/// A word the classifier does not offer, a `smart` value that is not an object,
/// or an unreadable or unwritable file.
pub fn set_classifier(path: &Path, mode: &str) -> Result<(), String> {
    let word = ClassifierMode::offered(mode)
        .ok_or_else(|| format!("알 수 없는 분류기 모드입니다: {mode}"))?
        .key();
    update_smart(path, |smart| {
        smart.insert(
            CLASSIFIER_SETTING.to_string(),
            Value::String(word.to_string()),
        );
        Ok(())
    })
}

/// Save the editable launch table without changing any other settings.
/// Why one difficulty's row was not saved: the lineup's refusal by name
/// (`refusal`), or the settings file's own sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SummonRowRefusal {
    pub kind: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<zerocode_core::summon_difficulty::lineup::Refusal>,
}

impl SummonRowRefusal {
    const ROW: &'static str = "summon-row";

    fn file(message: String) -> Self {
        Self {
            kind: "failed",
            message,
            refusal: None,
        }
    }
}

impl From<RouterRefusal> for SummonRowRefusal {
    fn from(refusal: RouterRefusal) -> Self {
        Self::file(refusal.message)
    }
}

/// Write `agent`'s row at `difficulty` — or clear it back to the lineup when
/// `written` names nothing — leaving every other row and key as it stood.
/// The row is checked against `lineup` first (the core's
/// [`check_written`](zerocode_core::summon_difficulty::lineup::check_written)):
/// a model the lineup does not offer, or an effort the row's model does not
/// take, is refused by name and nothing is written.
///
/// # Errors
/// The lineup's refusal, an unknown difficulty, a `smart` value that is not
/// an object, or an unreadable or unwritable file.
pub fn set_summon_profile(
    path: &Path,
    agent: &str,
    difficulty: &str,
    written: &zerocode_core::summon_difficulty::Written,
    lineup: Option<&zerocode_core::summon_difficulty::lineup::Lineup>,
) -> Result<(), SummonRowRefusal> {
    use zerocode_core::summon_difficulty::{self as summon, lineup::Refusal};
    if !summon::LADDER.iter().any(|(key, _)| *key == difficulty) {
        return Err(SummonRowRefusal::file(format!(
            "{agent}: unknown difficulty {difficulty}"
        )));
    }
    let mut refused: Option<Refusal> = None;
    crate::api_routers::update_zo_settings_root(path, |root| {
        let document = Value::Object(root.clone());
        if let Err(refusal) =
            summon::lineup::check_written(&document, agent, difficulty, written, lineup)
        {
            let message = refusal.to_string();
            refused = Some(refusal);
            return Err(message);
        }
        let mut table = summon::written_profiles(&document);
        let rows = table
            .as_object_mut()
            .ok_or_else(|| format!("{}: expected an object", summon::PROFILES_SETTING))?
            .entry(agent)
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(rows) = rows else {
            return Err(format!("{agent}: expected an object of difficulties"));
        };
        if written.is_empty() {
            rows.remove(difficulty);
        } else {
            rows.insert(
                difficulty.to_string(),
                serde_json::to_value(written).map_err(|err| err.to_string())?,
            );
        }
        if rows.is_empty()
            && let Some(table) = table.as_object_mut()
        {
            table.remove(agent);
        }
        summon::validate_profiles(&table)?;
        let smart = root
            .entry(SMART_SETTINGS_KEY)
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(smart) = smart else {
            return Err(format!(
                "settings.json의 {SMART_SETTINGS_KEY}가 JSON 객체가 아니라 바꾸지 않았습니다"
            ));
        };
        if table.as_object().is_some_and(Map::is_empty) {
            smart.remove(summon::PROFILES_SETTING);
        } else {
            smart.insert(summon::PROFILES_SETTING.to_string(), table);
        }
        Ok(())
    })
    .map_err(|message| match refused {
        Some(refusal) => SummonRowRefusal {
            kind: SummonRowRefusal::ROW,
            message,
            refusal: Some(refusal),
        },
        None => SummonRowRefusal::file(message),
    })
}

/// Pin the model every Jev request names (`smart.jevModel`, t-6187), or
/// unpin it with an empty word, leaving every other key as it stood — and
/// answer the row as the card paints it.
///
/// Its own door, like the classifier's: it is no feature's switch, and its word
/// is a model id rather than a mode. Unpinned is no key at all, not the alias
/// written down, so an unpinned file follows the alias wherever the vendor
/// moves it. A word the door would not read as a pin — two words, a control
/// character — is refused rather than written.
///
/// # Errors
/// A pin that is not one word, a `smart` value that is not an object, or an
/// unreadable or unwritable file.
pub fn set_model(path: &Path, word: &str) -> Result<ModelRow, String> {
    let pin = if word.trim().is_empty() {
        None
    } else {
        Some(pinned_model(word).ok_or_else(|| format!("알 수 없는 모델 이름입니다: {word}"))?)
    };
    update_smart(path, |smart| {
        match pin {
            Some(pin) => {
                smart.insert(MODEL_SETTING.to_string(), Value::String(pin.to_string()));
            }
            None => {
                smart.remove(MODEL_SETTING);
            }
        }
        Ok(())
    })?;
    Ok(ModelRow::of(&crate::api_routers::read_zo_settings_root(
        path,
    )?))
}

/// Change the existing local label-draft preference without changing consent or feature modes.
pub fn set_label_drafts(path: &Path, on: bool) -> Result<(), String> {
    update_smart(path, |smart| {
        let jev = smart
            .entry(zerocode_core::jev::door::JEV_SETTINGS_KEY)
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or_else(|| "Jev settings must be an object".to_string())?;
        jev.insert(
            zerocode_core::jev::promote::LABEL_DRAFTS_KEY.to_string(),
            Value::Bool(on),
        );
        Ok(())
    })
}

/// What `zo decision-shadow check --json` said: the model that answered and
/// how long it took, or the failure token of why none did (`no_key`,
/// `unauthorized`, `timeout`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeSafeCheck {
    pub answered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    pub elapsed_ms: u64,
}

pub fn checked_provider(
    provider: &judgment_provider::Provider,
    check: TypeSafeCheck,
) -> Result<TypeSafeCheck, String> {
    if check.answered
        && provider.kind != judgment_provider::ProviderKind::Jev
        && !check
            .model
            .as_deref()
            .is_some_and(|model| model.starts_with(&format!("{}:", provider.identity())))
    {
        return Err("provider_client_outdated".to_string());
    }
    Ok(check)
}

/// The check's answer from zo's stdout — the one JSON object it prints, whether
/// or not anything answered (zo exits 1 when nothing did). `None` when stdout
/// holds no such object: a zo too old to know the verb, or one that crashed.
#[must_use]
pub fn read_check(stdout: &[u8]) -> Option<TypeSafeCheck> {
    String::from_utf8_lossy(stdout)
        .lines()
        .find_map(|line| serde_json::from_str::<TypeSafeCheck>(line.trim()).ok())
}

/// How long a preflight waits for a Jev or remote candidate: zo's own key
/// check's deadline (`DECISION_SHADOW_DEADLINE`).
pub const PREFLIGHT_DEADLINE: Duration = Duration::from_secs(8);
/// How long it waits for a loopback candidate, which may load its model on
/// the first request: the managed runtime's own native check's.
pub const LOCAL_PREFLIGHT_DEADLINE: Duration = Duration::from_secs(35);

/// The preflight's three questions, one of each kind a provider must answer,
/// about one number nobody's work holds.
const PREFLIGHT_CHOICE: &str = "parity";
const PREFLIGHT_SCORE: &str = "size";
const PREFLIGHT_NOUL: &str = "positive";
const PREFLIGHT_PARITY: [(&str, &str); 2] = [
    ("even", "The number divides by two with nothing left over."),
    ("odd", "The number leaves one over when divided by two."),
];
const PREFLIGHT_LEVELS: [&str; 2] = [
    "The number is smaller than ten.",
    "The number is ten or larger.",
];

/// The preflight's request: [`PREFLIGHT_CHOICE`], [`PREFLIGHT_SCORE`] and
/// [`PREFLIGHT_NOUL`] about a fixed number, built by the same builders every
/// question the product asks is.
#[must_use]
pub fn preflight_body() -> Value {
    crate::systemone::request_body(
        &serde_json::json!({"number": 4}),
        &serde_json::json!({
            (PREFLIGHT_CHOICE): choice::question("Is `number` even or odd?", &PREFLIGHT_PARITY),
            (PREFLIGHT_SCORE): {
                "type": "score",
                "instructions": "How large is `number`?",
                "criteria": PREFLIGHT_LEVELS,
            },
            (PREFLIGHT_NOUL): noul::question(
                "`number` is greater than zero.",
                "The number is greater than zero.",
                "The number is zero or less than zero.",
            ),
        }),
    )
}

/// The version an answer to [`preflight_body`] names, once every one of its
/// three answers has passed the reader each seat reads that kind with — or
/// the `schema` token of the first that did not.
///
/// # Errors
/// A body that is not an answer, or an answer that breaks a reader's rule.
pub fn read_preflight(body: &str) -> Result<Option<String>, String> {
    let response: Value = serde_json::from_str(body).map_err(|_| promote::SCHEMA.to_string())?;
    let answers = response
        .get("answers")
        .ok_or_else(|| promote::SCHEMA.to_string())?;
    let offered = PREFLIGHT_PARITY
        .iter()
        .map(|(word, _)| (*word).to_string())
        .collect();
    choice::read(answers, PREFLIGHT_CHOICE, &offered)
        .map_err(|refusal| refusal.token().to_string())?;
    noul::read(answers, PREFLIGHT_NOUL).map_err(|refusal| refusal.token().to_string())?;
    score::read_value(
        answers.get(PREFLIGHT_SCORE).unwrap_or(&Value::Null),
        PREFLIGHT_LEVELS.len(),
    )
    .map_err(|rule| format!("{}_{}", promote::SCHEMA, rule.word()))?;
    Ok(crate::systemone::answered_by(body))
}

/// What a candidate provider said to [`preflight_body`] before anything
/// selected it: whether it answered all three questions in the native
/// System One schema — a connection, not a measure of whether its judgments
/// are right — the version that answered or the token of why none did, and
/// the two identities the result is bound to. An apply that names
/// `active_provider_identity` as the one it expects to replace cannot land
/// after the settings moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreflight {
    pub candidate_identity: String,
    pub active_provider_identity: String,
    pub answered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    pub elapsed_ms: u64,
}

/// Put [`preflight_body`] to `candidate` through `wire` while the settings
/// at `path` select the provider named `expected`, and write nothing: no
/// setting, no key and no managed model. The key is the one the
/// candidate's own endpoint keeps, read only when it asks for one.
///
/// # Errors
/// A candidate an apply would refuse (nothing is sent), `settings_changed`
/// when the selected provider is not `expected` before the request or after
/// its answer, or an unreadable settings file or keychain.
pub fn preflight_provider(
    path: &Path,
    candidate: judgment_provider::Provider,
    expected: &str,
    keys: &dyn RouterKeys,
    wire: &crate::systemone::Wire,
) -> Result<ProviderPreflight, String> {
    preflight_provider_with_key(path, candidate, expected, keys, wire, None)
}

/// An entered key is temporary evidence for this check. It is never written
/// to the credential store; applying a successful connection owns that write.
pub fn preflight_provider_with_key(
    path: &Path,
    candidate: judgment_provider::Provider,
    expected: &str,
    keys: &dyn RouterKeys,
    wire: &crate::systemone::Wire,
    entered_key: Option<&str>,
) -> Result<ProviderPreflight, String> {
    let candidate = applicable(candidate)?;
    let still_selected = || {
        require_provider(
            &selected_provider(path).map_err(|refusal| refusal.message)?,
            Some(expected),
        )
    };
    still_selected()?;
    let key = if candidate.requires_auth {
        if let Some(key) = entered_key.map(str::trim).filter(|key| !key.is_empty()) {
            Some(key.to_string())
        } else {
            provider_key_at(path, &candidate, keys)
                .map_err(|refusal| refusal.message)?
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
        }
    } else {
        None
    };
    let deadline = if candidate.kind == judgment_provider::ProviderKind::Local {
        LOCAL_PREFLIGHT_DEADLINE
    } else {
        PREFLIGHT_DEADLINE
    };
    let began = Instant::now();
    let outcome = wire
        .ask_candidate(&candidate, key.as_deref(), &preflight_body(), deadline)
        .and_then(|body| read_preflight(&body));
    let elapsed_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    still_selected()?;
    let (model, failure) = match outcome {
        Ok(model) => (model, None),
        Err(failure) => (None, Some(failure)),
    };
    Ok(ProviderPreflight {
        candidate_identity: candidate.identity(),
        active_provider_identity: expected.to_string(),
        answered: failure.is_none(),
        model,
        failure,
        elapsed_ms,
    })
}

/// How many Jev requests this machine has sent today, and the most one day
/// may send (`smart.jev.dailyRequests`) — `None` when the person set no limit.
/// The dashboard draws it over the table (t-6243 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayBudget {
    pub sent: u64,
    pub most: Option<u64>,
}

/// The day's budget, read where the door reads it (`systemone::Wire::pass`):
/// the count file beside the settings file for the day the window counts in,
/// and the limit by the door's own reader of the settings. A settings file
/// that does not read limits nothing, as it limits nothing at the door.
///
/// Its own read rather than a field of [`TypeSafeSettings`]: the dashboard
/// asks it on every refresh, and the settings answer reads the keychain,
/// which is a `security` process each time.
#[must_use]
pub fn read_day(path: &Path) -> DayBudget {
    let root = crate::api_routers::read_zo_settings_root(path).map_or(Value::Null, Value::Object);
    let sent = path.parent().map_or(0, |home| {
        count::sent(&count::requests_path(home, &crate::systemone::today()))
    });
    DayBudget {
        sent,
        most: JevSettings::from_root(&root).daily_requests,
    }
}

/// A person's decision as the dashboard's editor sends it and is sent it
/// back (t-62170): a definition's fields ([`custom::Definition`]), with no id
/// until the store keeps one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionDraft {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub question: String,
    pub answer: AnswerDraft,
    pub at: custom::Point,
    pub reads: BTreeSet<custom::Input>,
    /// The folders it asks in; none for every folder the person consented to.
    #[serde(default)]
    pub folders: Option<BTreeSet<String>>,
    pub act_from: f64,
    pub max_per_hour: u64,
}

/// What a decision's answer is, as the editor writes it: a choice's options
/// in the order the person wrote them, so a name written twice is seen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerDraft {
    YesNo { yes: String, no: String },
    Choice { options: Vec<ChoiceOption> },
    Score { levels: Vec<String> },
}

/// One option of a choice: its name, and what it means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub name: String,
    pub means: String,
}

impl AnswerDraft {
    /// One empty answer of every kind, in the order the editor offers them.
    const EVERY_KIND: [Self; 3] = [
        Self::YesNo {
            yes: String::new(),
            no: String::new(),
        },
        Self::Choice {
            options: Vec::new(),
        },
        Self::Score { levels: Vec::new() },
    ];
}

impl DecisionDraft {
    /// A kept definition as the editor writes it: a choice's options in
    /// core's order, and no folders for every folder the person consented to.
    fn of(definition: custom::Definition) -> Self {
        Self {
            id: Some(definition.id),
            name: definition.name,
            question: definition.question,
            answer: match definition.answer {
                custom::Answer::YesNo { yes, no } => AnswerDraft::YesNo { yes, no },
                custom::Answer::Choice(options) => AnswerDraft::Choice {
                    options: options
                        .into_iter()
                        .map(|(name, means)| ChoiceOption { name, means })
                        .collect(),
                },
                custom::Answer::Score(levels) => AnswerDraft::Score { levels },
            },
            at: definition.at,
            reads: definition.reads,
            folders: match definition.folders {
                custom::Folders::Consented => None,
                custom::Folders::Chosen(folders) => Some(folders),
            },
            act_from: definition.act_from,
            max_per_hour: definition.max_per_hour,
        }
    }

    /// The definition the draft writes under `id`, and the options a choice
    /// names a second time: core's map holds a name once, so a second is
    /// refused at its name as one already offered
    /// ([`custom::Refusal::OptionName`]).
    fn definition(self, id: String) -> (custom::Definition, Vec<custom::Refusal>) {
        let mut twice = Vec::new();
        let answer = match self.answer {
            AnswerDraft::YesNo { yes, no } => custom::Answer::YesNo { yes, no },
            AnswerDraft::Choice { options } => {
                let mut named = BTreeMap::new();
                for option in options {
                    match named.entry(option.name) {
                        btree_map::Entry::Vacant(slot) => {
                            slot.insert(option.means);
                        }
                        btree_map::Entry::Occupied(taken) => {
                            twice.push(custom::Refusal::OptionName(taken.key().clone()));
                        }
                    }
                }
                custom::Answer::Choice(named)
            }
            AnswerDraft::Score { levels } => custom::Answer::Score(levels),
        };
        let definition = custom::Definition {
            id,
            name: self.name,
            question: self.question,
            answer,
            at: self.at,
            reads: self.reads,
            folders: self
                .folders
                .map_or(custom::Folders::Consented, custom::Folders::Chosen),
            act_from: self.act_from,
            max_per_hour: self.max_per_hour,
        };
        (definition, twice)
    }
}

/// Keep `draft` in the person's store under `home`, zo's config home, at
/// `now_ms`: a new decision when it has no id, recording before anything
/// acts on it, else the newest version of the one with its id. Its version.
///
/// # Errors
/// Whatever keeps it from the store.
pub fn save_custom_decision_at(
    home: &Path,
    draft: DecisionDraft,
    now_ms: i64,
) -> Result<String, custom::store::Error> {
    let editing = draft.id.is_some();
    let id = draft.id.clone().unwrap_or_else(custom::new_id);
    let (definition, twice) = draft.definition(id);
    if !twice.is_empty() {
        return Err(custom::store::Error::Refused(
            twice
                .into_iter()
                .chain(custom::check(&definition))
                .collect(),
        ));
    }
    if editing {
        custom::store::edit(home, &definition, now_ms)
    } else {
        custom::store::create(home, &definition, JevMode::Shadow, now_ms)
    }
}

/// What the dashboard's editor of a person's decisions reads before any is
/// listed (t-62170): the feature they stand under, every bound a definition
/// keeps, the kinds of answer, where one may ask and what it may read there,
/// and the modes one offers, each core's, so the page writes none.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomDecisionRules {
    /// The use the person's decisions stand under in the use table.
    pub feature: &'static str,
    pub decisions_max: usize,
    pub name_max_chars: usize,
    pub question_max_chars: usize,
    pub meaning_max_chars: usize,
    pub options: Within<usize>,
    pub hourly: Within<u64>,
    pub hourly_default: u64,
    /// The most requests every decision sends together in an hour, as the
    /// person's settings hold it.
    pub hourly_all: u64,
    pub act_from: Within<f64>,
    pub act_from_default: f64,
    /// How many days a deleted decision waits to be restored.
    pub trash_kept_days: i64,
    /// The word each kind of answer travels under, in the editor's order.
    pub kinds: Vec<String>,
    /// Where a decision may ask, and what it may read there.
    pub points: Vec<DecisionPoint>,
    /// The modes a decision offers, each by what it does.
    pub modes: Vec<ModeChoice>,
}

/// The least and the most a bound allows, both allowed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Within<T> {
    pub least: T,
    pub most: T,
}

/// A point a decision may ask at, and the inputs it may read there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DecisionPoint {
    pub at: custom::Point,
    pub inputs: &'static [custom::Input],
}

impl CustomDecisionRules {
    fn of(document: &Value) -> Self {
        Self {
            feature: zerocode_core::jev::CUSTOM_DECISIONS.id,
            decisions_max: custom::DECISIONS_MAX,
            name_max_chars: custom::NAME_MAX_CHARS,
            question_max_chars: custom::QUESTION_MAX_CHARS,
            meaning_max_chars: custom::MEANING_MAX_CHARS,
            options: Within::of(&custom::OPTIONS),
            hourly: Within::of(&custom::HOURLY_REQUESTS),
            hourly_default: custom::HOURLY_DEFAULT,
            hourly_all: door::custom_hourly_requests(document),
            act_from: Within::of(&custom::ACT_FROM),
            act_from_default: custom::ACT_FROM_DEFAULT,
            trash_kept_days: custom::store::TRASH_KEPT_DAYS,
            kinds: AnswerDraft::EVERY_KIND
                .iter()
                .filter_map(|answer| {
                    serde_json::to_value(answer)
                        .ok()?
                        .get("kind")?
                        .as_str()
                        .map(str::to_string)
                })
                .collect(),
            points: custom::Point::ALL
                .map(|at| DecisionPoint {
                    at,
                    inputs: at.inputs(),
                })
                .to_vec(),
            modes: custom::MODES.map(ModeChoice::of).to_vec(),
        }
    }
}

impl<T: Copy> Within<T> {
    fn of(bound: &std::ops::RangeInclusive<T>) -> Self {
        Self {
            least: *bound.start(),
            most: *bound.end(),
        }
    }
}

/// A decision the store keeps, as the dashboard lists it: its mode, its
/// version, when it was made and changed, and its newest version as the
/// editor writes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptDecision {
    pub mode: &'static str,
    pub version: String,
    pub created_at: String,
    pub updated_at: String,
    pub draft: DecisionDraft,
}

/// A deleted decision the store can still bring back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashedDecision {
    pub id: String,
    pub name: String,
    pub deleted_at: String,
}

/// Every decision the person keeps, the deleted ones still to be restored,
/// and the folders a decision may be narrowed to.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomDecisions {
    pub decisions: Vec<KeptDecision>,
    pub trash: Vec<TrashedDecision>,
    /// The folders the person consented to by name.
    pub folders: Vec<String>,
    /// Whether the person consented to every folder.
    pub everywhere: bool,
}

/// What core says of a draft before anything keeps it: the rules it breaks
/// — by their words on the wire ([`RefusalWords`]) — the version it would
/// be, the question it would ask, and the least sure line an agent would be
/// told of it: its first option at its act-from line.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftCheck {
    #[serde(serialize_with = "refusal_words")]
    pub refusals: Vec<custom::Refusal>,
    pub version: String,
    pub questions: Value,
    pub line: String,
}

/// A rule a draft breaks, as the drawer reads it: core's word for the rule,
/// core's word for the field it was found in, the line of that field, and
/// the option it names or the level, counted from zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusalWords {
    pub word: &'static str,
    pub field: Option<&'static str>,
    pub line: Option<usize>,
    pub option: Option<String>,
    pub level: Option<usize>,
}

impl RefusalWords {
    #[must_use]
    pub fn of(refusal: &custom::Refusal) -> Self {
        let (field, line) = match refusal {
            custom::Refusal::SecretLine { field, line } => (Some(field), Some(*line)),
            custom::Refusal::MeaningLength(field) => (Some(field), None),
            _ => (None, None),
        };
        let option = match (refusal, field) {
            (custom::Refusal::OptionName(name), _)
            | (_, Some(custom::Field::OptionName(name) | custom::Field::Meaning(name))) => {
                Some(name.clone())
            }
            _ => None,
        };
        let level = match field {
            Some(custom::Field::Level(index)) => Some(*index),
            _ => None,
        };
        Self {
            word: refusal.word(),
            field: field.map(custom::Field::word),
            line,
            option,
            level,
        }
    }
}

fn refusal_words<S: serde::Serializer>(
    refusals: &[custom::Refusal],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(refusals.iter().map(RefusalWords::of))
}

/// Why a change to a person's decisions was not made, as the drawer reads
/// it: core's refusals by their words, a decision the store no longer keeps,
/// or what kept the store from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionRefusal {
    pub refusals: Vec<RefusalWords>,
    pub missing: bool,
    pub message: String,
}

impl From<custom::store::Error> for DecisionRefusal {
    fn from(error: custom::store::Error) -> Self {
        match error {
            custom::store::Error::Refused(refusals) => Self {
                refusals: refusals.iter().map(RefusalWords::of).collect(),
                missing: false,
                message: String::new(),
            },
            custom::store::Error::Missing => Self {
                refusals: Vec::new(),
                missing: true,
                message: String::new(),
            },
            custom::store::Error::Io(error) => error.into(),
        }
    }
}

impl From<std::io::Error> for DecisionRefusal {
    fn from(error: std::io::Error) -> Self {
        Self {
            refusals: Vec::new(),
            missing: false,
            message: error.to_string(),
        }
    }
}

impl From<RouterRefusal> for DecisionRefusal {
    fn from(refusal: RouterRefusal) -> Self {
        Self {
            refusals: Vec::new(),
            missing: false,
            message: refusal.message,
        }
    }
}

/// The person's decisions under `home`, zo's config home, and the folders
/// `settings` consents to.
///
/// # Errors
/// A store that cannot be read.
pub fn custom_decisions_at(home: &Path, settings: &Value) -> std::io::Result<CustomDecisions> {
    let decisions = custom::store::list(home)?
        .into_iter()
        .map(|entry| {
            Ok(KeptDecision {
                draft: DecisionDraft::of(custom::store::definition(home, &entry.definition)?),
                mode: entry.mode.key(),
                version: entry.definition,
                created_at: entry.created_at,
                updated_at: entry.updated_at,
            })
        })
        .collect::<std::io::Result<_>>()?;
    let trash = custom::store::trash(home)?
        .into_iter()
        .map(|trashed| TrashedDecision {
            id: trashed.decision.id,
            name: trashed.decision.name,
            deleted_at: trashed.deleted_at,
        })
        .collect();
    let door = JevSettings::from_root(settings);
    Ok(CustomDecisions {
        decisions,
        trash,
        folders: door.folders().map(str::to_string).collect(),
        everywhere: door.everywhere(),
    })
}

/// Core's check of `draft`, with nothing kept.
#[must_use]
pub fn check_custom_decision(draft: DecisionDraft) -> DraftCheck {
    let id = draft.id.clone().unwrap_or_else(custom::new_id);
    let first = match &draft.answer {
        AnswerDraft::YesNo { .. } => AGENT_TOOL_ASK_OPTIONS[0].to_string(),
        AnswerDraft::Choice { options } => options
            .first()
            .map(|option| option.name.clone())
            .unwrap_or_default(),
        AnswerDraft::Score { levels } => custom::shown_level(0, levels.len()),
    };
    let (definition, twice) = draft.definition(id);
    DraftCheck {
        refusals: twice
            .into_iter()
            .chain(custom::check(&definition))
            .collect(),
        version: definition.version().unwrap_or_default(),
        questions: custom::questions(&definition),
        line: custom::line(&definition.name, &first, definition.act_from),
    }
}

/// Delete the decision `id` under `home` at `now_ms`, and empty the trash of
/// what waited in it past its days.
///
/// # Errors
/// Whatever keeps the store from it.
pub fn delete_custom_decision_at(
    home: &Path,
    id: &str,
    now_ms: i64,
) -> Result<(), custom::store::Error> {
    custom::store::delete(home, id, now_ms)?;
    custom::store::empty_trash(home, now_ms)?;
    Ok(())
}

/// Set the decision `id` under `home` to the mode named `mode` at `now_ms`.
///
/// # Errors
/// A mode no decision offers, and whatever keeps the store from it.
pub fn set_custom_decision_mode_at(
    home: &Path,
    id: &str,
    mode: &str,
    now_ms: i64,
) -> Result<(), custom::store::Error> {
    let Some(mode) = JevMode::ALL
        .into_iter()
        .find(|offered| offered.key() == mode)
    else {
        return Err(custom::store::Error::Refused(vec![custom::Refusal::Mode]));
    };
    custom::store::set_mode(home, id, mode, now_ms)
}

#[cfg(test)]
mod provider_tests;

#[cfg(test)]
mod builtin_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_provider_apply_compares_the_identity_inside_the_settings_transaction() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        let original = serde_json::json!({"theme":"dark","smart":{"jev":{"enabled":false}}});
        std::fs::write(&path, original.to_string()).unwrap();
        let expected = selected_provider(&path).unwrap().identity();
        let provider = judgment_provider::Provider {
            kind: judgment_provider::ProviderKind::Local,
            protocol: judgment_provider::Protocol::SystemOne,
            base_url: "http://127.0.0.1:43210".to_string(),
            model: "fixture-native".to_string(),
            requires_auth: false,
            connection: None,
        };
        assert_eq!(
            set_provider_if_identity(&path, provider.clone(), "old-provider", None),
            Err("settings_changed".to_string())
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original.to_string()
        );
        set_provider_if_identity(&path, provider.clone(), &expected, None).unwrap();
        let applied = std::fs::read_to_string(&path).unwrap();
        assert_eq!(selected_provider(&path).unwrap(), provider);
        assert_eq!(
            set_provider_if_identity(&path, provider, &expected, None),
            Err("settings_changed".to_string())
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), applied);
        let root: Value = serde_json::from_str(&applied).unwrap();
        assert_eq!(root["theme"], "dark");
        assert_eq!(root["smart"]["jev"]["enabled"], false);
    }

    #[test]
    fn managed_selection_is_saved_with_the_provider_and_cleared_by_manual_apply() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","smart":{"jev":{"enabled":false}}}"#,
        )
        .unwrap();
        let expected = selected_provider(&path).unwrap().identity();
        let provider = judgment_provider::Provider {
            kind: judgment_provider::ProviderKind::Local,
            protocol: judgment_provider::Protocol::SystemOne,
            base_url: "http://127.0.0.1:43210".to_string(),
            model: "fixture-native".to_string(),
            requires_auth: false,
            connection: None,
        };
        let binding = local_judgment::Binding {
            model_id: provider.model.clone(),
            revision: "1".repeat(40),
            provider_identity: provider.identity(),
        };
        set_provider_if_identity(&path, provider.clone(), &expected, Some(&binding)).unwrap();
        let applied = std::fs::read_to_string(&path).unwrap();
        let root: Value = serde_json::from_str(&applied).unwrap();
        assert_eq!(
            root["smart"][local_judgment::MANAGED_MODEL_KEY],
            serde_json::to_value(&binding).unwrap()
        );
        assert_eq!(root["theme"], "dark");
        assert_eq!(root["smart"]["jev"]["enabled"], false);
        assert_eq!(
            set_provider_if_identity(&path, provider.clone(), &expected, Some(&binding)),
            Err("settings_changed".to_string())
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), applied);
        set_provider(&path, provider.clone()).unwrap();
        let manual: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            manual["smart"]
                .get(local_judgment::MANAGED_MODEL_KEY)
                .is_none()
        );
        assert_eq!(selected_provider(&path).unwrap(), provider);
    }
    use crate::api_routers::{HeldKeys, Keychain, RouterRefusalKind};

    #[test]
    fn review_retention_changes_neither_consent_nor_explicit_feature_choices() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        let before = serde_json::json!({"theme":"dark","smart":{
            "jev":{"enabled":false,"workspaces":["/work/acme"],"dailyRequests":12},
            "jevCompaction":"off","skillSearch":"shadow","jevModel":"jev-held"
        }});
        std::fs::write(&path, before.to_string()).unwrap();
        for on in [true, false] {
            set_label_drafts(&path, on).unwrap();
            let mut after: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(after["smart"]["jev"]["labelDrafts"], on);
            after["smart"]["jev"]
                .as_object_mut()
                .unwrap()
                .remove("labelDrafts");
            assert_eq!(after, before);
        }
        let invalid = serde_json::json!({"smart":{"jev":false}}).to_string();
        std::fs::write(&path, &invalid).unwrap();
        assert!(set_label_drafts(&path, true).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
    }

    #[test]
    fn a_summon_row_is_written_one_at_a_time_and_cleared_back_to_the_lineup() {
        use zerocode_core::summon_difficulty::{LADDER, PROFILES_SETTING, Written};
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        let before = serde_json::json!({"smart":{"jevModel":"held"}, "theme":"dark"});
        std::fs::write(&path, before.to_string()).unwrap();
        let read =
            || -> Value { serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap() };
        let high = LADDER[2].0;
        let effort_only = Written {
            model: None,
            effort: Some("high".into()),
        };
        set_summon_profile(&path, "codex", high, &effort_only, None).unwrap();
        let after = read();
        assert_eq!(after["theme"], before["theme"]);
        assert_eq!(after["smart"]["jevModel"], before["smart"]["jevModel"]);
        assert_eq!(
            after["smart"][PROFILES_SETTING],
            serde_json::json!({"codex": {high: {"effort": "high"}}}),
            "only the field written, the model left to the lineup"
        );
        set_summon_profile(&path, "codex", high, &Written::default(), None).unwrap();
        assert!(
            read()["smart"].get(PROFILES_SETTING).is_none(),
            "the last row cleared leaves no table"
        );
        assert_eq!(read()["smart"]["jevModel"], before["smart"]["jevModel"]);
        assert!(set_summon_profile(&path, "codex", "extreme", &effort_only, None).is_err());
        assert!(set_summon_profile(&path, "unknown", high, &effort_only, None).is_err());
    }

    #[test]
    fn a_summon_row_the_lineup_does_not_offer_is_refused_by_name_and_nothing_is_written() {
        use zerocode_core::summon_difficulty::lineup::{Lineup, Refusal};
        use zerocode_core::summon_difficulty::{LADDER, Written};
        let lineup = Lineup::from_catalog(
            &serde_json::json!({"models": [{"provider": "openai", "id": "model-a",
                "band": "second", "rungs": ["hard"], "efforts": ["low", "high"]}]}),
            "openai",
        )
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        let before = serde_json::json!({"theme":"dark"});
        std::fs::write(&path, before.to_string()).unwrap();
        let read =
            || -> Value { serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap() };
        let high = LADDER[2].0;
        let cell = |model: &str, effort: Option<&str>| Written {
            model: Some(model.into()),
            effort: effort.map(str::to_string),
        };
        let refused =
            set_summon_profile(&path, "codex", high, &cell("model-z", None), Some(&lineup))
                .unwrap_err();
        assert_eq!(refused.kind, "summon-row");
        assert_eq!(
            refused.refusal,
            Some(Refusal::NotListed {
                model: "model-z".into()
            })
        );
        let refused = set_summon_profile(
            &path,
            "codex",
            high,
            &cell("model-a", Some("max")),
            Some(&lineup),
        )
        .unwrap_err();
        assert!(matches!(
            refused.refusal,
            Some(Refusal::EffortOutside { .. })
        ));
        assert_eq!(read(), before, "nothing written");
        set_summon_profile(
            &path,
            "codex",
            high,
            &cell("model-a", Some("high")),
            Some(&lineup),
        )
        .unwrap();
        assert_eq!(
            read()["smart"]["summonProfiles"]["codex"][high],
            serde_json::json!({"model": "model-a", "effort": "high"})
        );
    }

    #[test]
    fn the_key_lives_in_the_item_zo_reads() {
        assert_eq!(
            typesafe_keychain_service(),
            "dev.zerocode.key.TYPESAFE_API_KEY"
        );
    }

    /// A saved key is kept trimmed under the item zo reads, reported as saved
    /// and never echoed; an empty one is refused, and a removal forgets it.
    #[test]
    fn a_key_is_saved_trimmed_reported_without_itself_and_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let keys = HeldKeys::default();

        assert!(
            !read_settings(&path, &keys, true)
                .expect("settings")
                .key_saved
        );
        assert!(save_key("   ", &keys).is_err(), "an empty key is refused");
        assert_eq!(keys.held(&typesafe_keychain_service()), None);

        save_key("  apikey_test  ", &keys).expect("save");
        assert_eq!(
            keys.held(&typesafe_keychain_service()).as_deref(),
            Some("apikey_test")
        );
        let settings = read_settings(&path, &keys, true).expect("settings");
        assert!(settings.key_saved);
        let wire = serde_json::to_string(&settings).expect("serializes");
        assert!(
            !wire.contains("apikey_test"),
            "the pane is never sent the key: {wire}"
        );

        remove_key(&keys).expect("remove");
        assert_eq!(keys.held(&typesafe_keychain_service()), None);
        assert!(
            !read_settings(&path, &keys, true)
                .expect("settings")
                .key_saved
        );
    }

    /// Off macOS there is no keychain: the save is refused in the word the
    /// pane translates, rather than "kept" nowhere.
    #[test]
    fn a_machine_with_no_keychain_refuses_the_key_by_kind() {
        let refusal = save_key("apikey_test", &Keychain::without_a_store()).expect_err("refused");
        assert_eq!(refusal.kind, RouterRefusalKind::KeychainUnavailable);
    }

    /// The one switch (§6.1), pressed from the file a person who set every
    /// feature by hand keeps — twenty words, four folders, and the door's own
    /// switch never written, as this machine's file holds it on 2026-09-23.
    /// Before the press the card says Jev is in use in some folders; one press
    /// on and every feature stands at its recommendation in every folder; one
    /// press off and nothing is in use. Nothing the press does not own moves:
    /// the router rows, the model pin, other `smart` knobs, the door's own
    /// neighbours and unknown keys stay as they were.
    #[test]
    fn the_one_switch_is_written_where_both_programs_read_it_and_nothing_else_moves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let mut smart = serde_json::Map::new();
        for row in &JEV_USES {
            let word = if row.modes.contains(&JevMode::Auto) {
                JevMode::Auto
            } else {
                JevMode::On
            };
            smart.insert(row.setting.to_string(), Value::from(word.key()));
        }
        smart.insert(
            door::JEV_SETTINGS_KEY.to_string(),
            serde_json::json!({
                door::WORKSPACES_SETTING: ["/work/a", "/work/b", "/work/c", "/work/d"],
                "labelDrafts": true,
            }),
        );
        smart.insert(MODEL_SETTING.to_string(), Value::from("jev-1.13.0"));
        smart.insert("plan".to_string(), serde_json::json!({"minEdge": 3}));
        let before = serde_json::json!({
            "providers": [{"name": "OpenRouter", "base_url": "https://openrouter.ai/api/v1",
                           "models": [], "requires_auth": true, "auth_env": "ZEROCODE_ROUTER_OPENROUTER_KEY"}],
            "model": "fable",
            SMART_SETTINGS_KEY: smart,
        });
        std::fs::write(&path, before.to_string()).expect("write");
        let keys = HeldKeys::default();
        let read = || read_settings(&path, &keys, true).expect("settings");
        let file = || -> Value {
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json")
        };
        let untouched = |after: &Value| {
            for key in ["providers", "model"] {
                assert_eq!(after[key], before[key], "{key} moved");
            }
            for key in [MODEL_SETTING, "plan"] {
                assert_eq!(
                    after[SMART_SETTINGS_KEY][key], before[SMART_SETTINGS_KEY][key],
                    "smart.{key} moved"
                );
            }
            assert_eq!(
                after[SMART_SETTINGS_KEY][door::JEV_SETTINGS_KEY]["labelDrafts"],
                true,
                "the door's neighbour moved"
            );
        };

        let held = read();
        assert_eq!(
            held.jev,
            JevSwitch {
                on: true,
                everywhere: false,
                folders: 4,
            },
            "a file set by hand sends from its four folders"
        );
        assert!(held.switches.iter().all(|row| row.written));

        set_enabled(&path, true).expect("on");
        let after = file();
        untouched(&after);
        let held = read();
        assert_eq!(
            held.jev,
            JevSwitch {
                on: true,
                everywhere: true,
                folders: 0,
            }
        );
        for (row, painted) in JEV_USES.iter().zip(&held.switches) {
            assert_eq!(painted.mode, row.recommended.key(), "{}", row.id);
            assert!(!painted.written, "{} kept its own word", row.id);
            // The wall the feature's paragraph names comes from the table,
            // never from its words (t-45961).
            assert_eq!(
                painted.wall_ms,
                zerocode_core::jev::worded_wall_ms(row),
                "{}",
                row.id
            );
        }

        set_enabled(&path, false).expect("off");
        let mut expected = after.clone();
        expected[SMART_SETTINGS_KEY][door::JEV_SETTINGS_KEY][door::ENABLED_SETTING] =
            Value::Bool(false);
        assert_eq!(file(), expected, "off moves the switch and nothing else");
        let held = read();
        assert!(!held.jev.on);
        for painted in &held.switches {
            assert_eq!(painted.mode, JevMode::Off.key(), "{}", painted.id);
        }

        // A machine that has never met Jev: nothing in use, and one press on
        // makes the object it needs.
        let fresh = dir.path().join("fresh.json");
        assert!(!read_settings(&fresh, &keys, true).expect("settings").jev.on);
        set_enabled(&fresh, true).expect("on");
        let fresh = read_settings(&fresh, &keys, true).expect("settings");
        assert!(fresh.jev.on && fresh.jev.everywhere);
    }

    /// The model pin (`smart.jevModel`, t-6187) is written where both
    /// programs' door reads it and nothing else of the file moves; the card
    /// reads it back as the door does. An empty pin unpins — the key leaves
    /// the file and the alias is asked again — and a pin that is not one word
    /// is refused rather than written, because the door would read it as no
    /// pin at all.
    #[test]
    fn the_model_pin_is_written_where_the_door_reads_it_and_an_empty_one_unpins() {
        use zerocode_core::jev::door::JevSettings;
        use zerocode_core::jev::{DEFAULT_MODEL, MODEL_SETTING};
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let before = serde_json::json!({
            "smart": {"plan": {"minEdge": 3}, "decisionShadow": "shadow"},
            "model": "fable",
        });
        std::fs::write(&path, before.to_string()).expect("write");
        let keys = HeldKeys::default();
        let door = || {
            let root: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            JevSettings::from_root(&root).model
        };
        let card = || read_settings(&path, &keys, true).expect("settings").model;

        assert_eq!(card().model, DEFAULT_MODEL);
        assert!(!card().pinned);
        assert_eq!(card().alias, DEFAULT_MODEL);

        let pinned = set_model(&path, " jev-1.13.0 ").expect("a version is a pin");
        assert_eq!(pinned.model, "jev-1.13.0");
        assert!(pinned.pinned);
        assert_eq!(
            door(),
            "jev-1.13.0",
            "the door reads the pin the card wrote"
        );
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(after["smart"][MODEL_SETTING], "jev-1.13.0");
        assert_eq!(after["smart"]["plan"], before["smart"]["plan"]);
        assert_eq!(after["smart"]["decisionShadow"], "shadow");
        assert_eq!(
            after["model"], "fable",
            "zo's own model setting is not the pin"
        );

        for slip in ["jev 1.13", "jev-1.13.0\nx", "jev\t1"] {
            assert!(set_model(&path, slip).is_err(), "{slip:?} was written");
            assert_eq!(door(), "jev-1.13.0", "a refused pin left the file alone");
        }

        let unpinned = set_model(&path, "  ").expect("an empty pin unpins");
        assert!(!unpinned.pinned);
        assert_eq!(unpinned.model, DEFAULT_MODEL);
        assert_eq!(door(), DEFAULT_MODEL);
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert!(
            after["smart"].get(MODEL_SETTING).is_none(),
            "unpinned is no key, not the alias written down: {after}"
        );
    }

    /// Each switch reads as its reader reads it: a mode its row offers, in any
    /// case; a typo or a boolean is off; a `smart` that is not an object is
    /// refused rather than overwritten.
    #[test]
    fn the_switch_reads_as_zo_reads_it_and_refuses_to_overwrite_a_stranger() {
        for row in &JEV_USES {
            let read = |value: Value| {
                row.mode_in(&serde_json::json!({ SMART_SETTINGS_KEY: { row.setting: value } }))
            };
            for mode in row.modes {
                let shouted = format!(" {} ", mode.key().to_ascii_uppercase());
                assert_eq!(read(serde_json::json!(shouted)), *mode, "{}", row.id);
            }
            assert_eq!(read(serde_json::json!("shadwo")), JevMode::Off);
            assert_eq!(read(serde_json::json!(true)), JevMode::Off);
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        for stranger in [r#"{"smart": "fast"}"#, r#"{"smart": {"jev": ["/work"]}}"#] {
            std::fs::write(&path, stranger).expect("write");
            for on in [true, false] {
                assert!(set_enabled(&path, on).is_err(), "{stranger} {on}");
            }
            assert_eq!(
                std::fs::read_to_string(&path).expect("read"),
                stranger,
                "a refused press writes nothing"
            );
        }
    }

    /// A file written before the skill suggestion was a seat of its own
    /// reads as it did before the update (t-6877 round 3, the coordinator's
    /// migration contract m-8181): its person turned every feature off by
    /// hand, the skill search among them, and the suggestion — which read the
    /// search's word until the split — stands off too, governed by that word
    /// and not by the switch, so the card says nothing is sent; a word
    /// written for the suggestion itself is its own, and the search's `off`
    /// stands beside it.
    #[test]
    fn a_file_from_before_the_split_keeps_the_suggestion_at_the_searchs_word() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let suggestion = &zerocode_core::jev::SKILL_SUGGESTION;
        let search = &zerocode_core::jev::SKILLS;
        let mut smart = serde_json::Map::new();
        for row in JEV_USES.iter().filter(|row| row.id != suggestion.id) {
            smart.insert(row.setting.to_string(), Value::from(JevMode::Off.key()));
        }
        smart.insert(
            door::JEV_SETTINGS_KEY.to_string(),
            serde_json::json!({
                door::ENABLED_SETTING: true,
                door::WORKSPACES_SETTING: [door::EVERY_WORKSPACE],
            }),
        );
        let keys = HeldKeys::default();
        let read = |smart: &serde_json::Map<String, Value>| {
            std::fs::write(
                &path,
                serde_json::json!({ SMART_SETTINGS_KEY: smart }).to_string(),
            )
            .expect("write");
            read_settings(&path, &keys, true).expect("settings")
        };
        let row = |state: &TypeSafeSettings, id: &str| {
            state
                .switches
                .iter()
                .find(|row| row.id == id)
                .map(|row| (row.mode, row.written))
                .expect("a row")
        };

        let before = read(&smart);
        assert_eq!(
            row(&before, suggestion.id),
            (JevMode::Off.key(), true),
            "the suggestion stands at the search's word, which the switch does not govern"
        );
        assert!(
            !before.jev.on,
            "nothing asks, so the switch says nothing is sent"
        );

        smart.insert(
            suggestion.setting.to_string(),
            Value::from(JevMode::Auto.key()),
        );
        let split = read(&smart);
        assert_eq!(row(&split, suggestion.id), (JevMode::Auto.key(), true));
        assert_eq!(row(&split, search.id), (JevMode::Off.key(), true));
        assert!(split.jev.on, "the suggestion asks on its own word");
    }

    /// zo prints one JSON object whether or not anything answered; anything
    /// else on stdout is no answer at all.
    #[test]
    fn the_check_is_read_from_zos_one_json_line() {
        assert_eq!(
            read_check(
                b"{\"answered\":true,\"elapsedMs\":664,\"model\":\"jev-1.13.0\",\"retries\":0}\n"
            ),
            Some(TypeSafeCheck {
                answered: true,
                model: Some("jev-1.13.0".to_string()),
                failure: None,
                elapsed_ms: 664,
            })
        );
        assert_eq!(
            read_check(b"{\"answered\":false,\"elapsedMs\":515,\"failure\":\"unauthorized\",\"retries\":0}"),
            Some(TypeSafeCheck {
                answered: false,
                model: None,
                failure: Some("unauthorized".to_string()),
                elapsed_ms: 515,
            })
        );
        assert_eq!(read_check(b"unknown argument 'decision-shadow'"), None);
    }

    /// The pane lists each row's modes, in the row's order, from the state it
    /// paints — the page holds no option of its own — asks every command it
    /// paints from, and every command it asks is registered.
    #[test]
    fn the_pane_offers_the_rows_modes_and_asks_registered_commands() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = read_settings(
            &dir.path().join("settings.json"),
            &HeldKeys::default(),
            true,
        )
        .expect("settings");
        assert_eq!(
            state.switches.len(),
            JEV_USES.len(),
            "the card carries every seat the table names"
        );
        for (row, painted) in JEV_USES.iter().zip(&state.switches) {
            assert_eq!(painted.id, row.id, "the card keeps the table's order");
            assert_eq!(painted.setting, row.setting, "{}", row.id);
            // The comparison sample the judge wants before accuracy may speak,
            // which the dashboard fills its bar toward (t-6243 D2).
            assert_eq!(
                painted.agreement_rows_wanted, row.agreement_rows_wanted,
                "{}",
                row.id
            );
            let offered = &painted.modes;
            let words: Vec<&str> = offered.iter().map(|choice| choice.mode).collect();
            let expected: Vec<&str> = row.modes.iter().map(|mode| mode.key()).collect();
            assert_eq!(
                words, expected,
                "{} offers its row's modes, in order",
                row.id
            );
            for (choice, mode) in offered.iter().zip(row.modes) {
                assert_eq!(
                    (choice.asks, choice.applies, choice.automatic),
                    (mode.asks(), mode.applies(), mode.automatic()),
                    "{} {}",
                    row.id,
                    choice.mode
                );
            }
        }

        let page = include_str!("../../../ui/index.html");
        let card = &page[page.find("id=\"typesafe-card\"").expect("the key card")..];
        let card = &card[..card
            .find("<template id=\"jev-features\">")
            .expect("the features follow the card")];
        assert_eq!(card.matches("data-jev-switch ").count(), 1, "one switch");
        let select_ids: Vec<&str> = card
            .split("<select")
            .skip(1)
            .map(|select| {
                let attributes = select.split_once('>').expect("select opening tag").0;
                attributes
                    .split_once("id=\"")
                    .and_then(|(_, value)| value.split_once('"'))
                    .expect("every model setting select has an id")
                    .0
            })
            .collect();
        assert_eq!(
            select_ids,
            [
                "managed-judgment-filter",
                "judgment-provider-kind",
                "judgment-protocol",
                "judgment-listed-model",
            ],
            "model selection and library filters do not add per-feature mode controls"
        );
        assert!(card.contains("id=\"judgment-provider-kind\""));
        // The worker rows and the classifier stand on the orchestration pane
        // (t-75762 v2, decision 4), each on a card of its own since the
        // person passed the before/after draft (2026-10-27): nothing folds
        // them away, and the classifier is still the only choice that names
        // a mode there.
        let orchestration = &page[page
            .find("<section class=\"settings-pane\" data-pane=\"orchestration\"")
            .expect("the orchestration pane")..];
        let orchestration = &orchestration["<section".len()..];
        let orchestration = &orchestration[..orchestration
            .find("<section class=\"settings-pane\"")
            .expect("the next pane")];
        for card in ["id=\"worker-models-card\"", "id=\"route-classifier-card\""] {
            let at = orchestration
                .find(card)
                .unwrap_or_else(|| panic!("the orchestration pane has no {card}"));
            assert!(
                orchestration[..at].matches("<details").count()
                    == orchestration[..at].matches("</details>").count(),
                "{card} stands under a fold"
            );
        }
        assert!(
            orchestration.contains("id=\"route-classifier-choices\""),
            "the classifier's choices are on its card"
        );
        assert!(
            page.contains("id=\"jev-builtin-form\""),
            "built-in decisions have their own editor"
        );

        let main = include_str!("main.rs");
        let handlers = main
            .split_once("tauri::generate_handler![")
            .map(|(_, list)| list)
            .expect("the handler list");
        // The card's script and the dashboard's: one Jev, two surfaces, and
        // the switch's door (`set_jev_enabled`) and the ledgers' numbers
        // (`jev_summary`) are asked from the file both surfaces share.
        let pane = concat!(
            include_str!("../../../ui/shell-settings.js"),
            include_str!("../../../ui/shell-judgment.js"),
            include_str!("../../../ui/shell-jev.js")
        );
        assert!(
            !handlers.contains("set_jev_mode,"),
            "a feature's mode is written by the file's own hand, not by a door"
        );
        for command in [
            "typesafe_settings",
            "save_typesafe_key",
            "remove_typesafe_key",
            "set_jev_enabled",
            "builtin_decision_definition",
            "update_builtin_decision",
            "set_route_classifier",
            "set_judgment_provider",
            "judgment_models",
            "preflight_judgment_provider",
            "check_typesafe_key",
            "jev_summary",
            "jev_day",
        ] {
            assert!(
                handlers.contains(&format!("{command},")),
                "{command} is not registered"
            );
            assert!(
                pane.contains(&format!("invoke(\"{command}\"")),
                "the pane never asks {command}"
            );
        }
    }

    /// The reading of the current model's real use is a door of its own
    /// (t-53792): registered with the webview, beside the settings it does
    /// not change, and a read — it asks no model and writes nothing.
    #[test]
    fn the_latest_request_reading_is_a_registered_door_that_only_reads() {
        let main = include_str!("main.rs");
        let handlers = main
            .split_once("tauri::generate_handler![")
            .map(|(_, list)| list)
            .expect("the handler list");
        assert!(
            handlers.contains("judgment_latest_request,"),
            "judgment_latest_request is not registered"
        );
        let doors = include_str!("cmd/typesafe.rs");
        let door = &doors[doors
            .find("fn judgment_latest_request(")
            .expect("the reading's door")..];
        let door = &door[..door.find("\n}\n").expect("the door closes")];
        assert!(door.contains("systemone::latest::read("), "{door}");
        for acting in ["ask(", "ask_candidate(", "set_", "write", "remove"] {
            assert!(
                !door.contains(acting),
                "the reading does `{acting}`: {door}"
            );
        }
    }

    /// Every feature names itself in the markup's own language and in each of
    /// the four catalogs — its name, the one-line summary and the paragraph of
    /// what it sends — so no language falls back to another feature's words or
    /// to a bare key. The markup keeps them once, in the use table's order
    /// (`#jev-features`), and a feature added to the table turns this red
    /// until its words exist.
    #[test]
    fn every_feature_speaks_in_every_catalog() {
        let page = include_str!("../../../ui/index.html");
        let i18n = include_str!("../../../ui/shell-i18n.js");
        let template = &page[page
            .find("<template id=\"jev-features\">")
            .expect("the features' words")..];
        let template = &template[..template.find("</template>").expect("the template closes")];
        let named: Vec<&str> = template
            .split("data-jev-feature=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect();
        let table: Vec<&str> = JEV_USES.iter().map(|row| row.id).collect();
        assert_eq!(named, table, "the features stand in the table's order");
        for (row, words) in JEV_USES
            .iter()
            .zip(template.split("data-jev-feature=\"").skip(1))
        {
            let key = |part: &str| {
                let marker = format!("data-jev-{part} data-i18n=\"");
                let at = words
                    .find(&marker)
                    .unwrap_or_else(|| panic!("{} has no {part}", row.id));
                words[at + marker.len()..]
                    .split('"')
                    .next()
                    .expect("a key")
                    .to_string()
            };
            let name = key("name");
            assert!(
                name.starts_with("settings.typesafe."),
                "{}: {name} is not a feature key",
                row.id
            );
            for (part, suffix) in [("name", ""), ("summary", "Summary"), ("hint", "Hint")] {
                let said = key(part);
                assert_eq!(said, format!("{name}{suffix}"), "{} {part}", row.id);
                assert_eq!(
                    i18n.matches(&format!("\"{said}\"")).count(),
                    4,
                    "{said} must be in en/ja/zh/es"
                );
            }
        }
    }

    /// What a person reads about Jev — the key card, the card of features and
    /// the dashboard — is said in the product's plain words and not in the
    /// codebase's metaphors (t-6243 D0,
    /// docs/design/jev-dashboard-improvement-plan-20260923.md): a person asks
    /// how often a feature asked, how right it was and what is left before it
    /// applies by itself, and a screen that answers in seats, rows, windows,
    /// thresholds, ledgers, probes, walks and rising is answering a question
    /// only the code asks.
    ///
    /// Read from where the words are written: the markup's Korean under every
    /// `settings.typesafe.*` and `jev.*` key, every Korean literal in the
    /// dashboard's script and in the card's part of the settings script, and
    /// the English catalog under the same keys.
    #[test]
    fn the_jev_surfaces_speak_plain_words_not_the_codebases() {
        /// The metaphors, each as its word and whether a use of it is
        /// recognised anywhere inside a word (`false`) or only as a word of
        /// its own — a noun with its particle, or a counter after a number —
        /// because `행` is also the second syllable of 진행 and `창` the last
        /// of 입력창.
        const KOREAN: [(&str, bool); 11] = [
            ("자리", false),
            ("행", true),
            ("창", true),
            ("문턱", false),
            ("원장", false),
            ("프로브", false),
            ("걷기", false),
            ("오르", false),
            ("오른", true),
            ("물은", true),
            ("답한", true),
        ];
        /// The particles a noun standing alone may carry.
        const PARTICLES: [&str; 22] = [
            "", "은", "는", "이", "가", "을", "를", "의", "만", "도", "에", "과", "와", "으로",
            "로", "마다", "씩", "에서", "까지", "부터", "뿐", "안",
        ];
        /// The uses of a metaphor's word that mean the plain thing, each with
        /// the reason it is plain here.
        const PLAIN_USES: [(&str, &str, &str); 6] = [
            (
                "창",
                "워커 창 배치",
                "the worker's own pane in this app, which is a window",
            ),
            (
                "창",
                "창 제목",
                "a desktop app's window title, sent as it is",
            ),
            (
                "창",
                "대상 창",
                "the desktop app's window a press lands in (t-12979)",
            ),
            (
                "창",
                "다른 창",
                "another app's window standing over the place to press",
            ),
            (
                "창",
                "덮은 창",
                "the window standing over the place to press",
            ),
            (
                "창",
                "떠 있는 창",
                "another app's window that floats above ordinary ones",
            ),
        ];
        /// The English catalog's metaphors, matched as whole words.
        const ENGLISH: [&str; 14] = [
            "seat", "seats", "ledger", "ledgers", "probe", "probes", "rise", "rises", "rising",
            "risen", "rose", "walk", "walks", "walking",
        ];

        fn hangul(text: &str) -> bool {
            text.chars().any(|c| ('가'..='힣').contains(&c))
        }
        /// The double-quoted and template literals of a script, outside its
        /// comments.
        fn literals(source: &str) -> Vec<String> {
            let mut found = Vec::new();
            let mut chars = source.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '/' if chars.peek() == Some(&'/') => {
                        for next in chars.by_ref() {
                            if next == '\n' {
                                break;
                            }
                        }
                    }
                    '/' if chars.peek() == Some(&'*') => {
                        chars.next();
                        let mut last = ' ';
                        for next in chars.by_ref() {
                            if last == '*' && next == '/' {
                                break;
                            }
                            last = next;
                        }
                    }
                    '"' | '`' | '\'' => {
                        let mut literal = String::new();
                        while let Some(next) = chars.next() {
                            match next {
                                '\\' => {
                                    chars.next();
                                }
                                _ if next == c => break,
                                _ => literal.push(next),
                            }
                        }
                        found.push(literal);
                    }
                    _ => {}
                }
            }
            found
        }
        fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
            let start = source.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &source[start..];
            let end = rest[from.len()..]
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}` at `{to}`"));
            &rest[..from.len() + end]
        }
        fn metaphors_in(text: &str) -> Vec<&'static str> {
            let mut plain = text.to_string();
            for (_, phrase, _) in PLAIN_USES {
                plain = plain.replace(phrase, " ");
            }
            let words: Vec<&str> = plain
                .split(|c: char| !(c.is_alphanumeric() || c == '{' || c == '}'))
                .filter(|word| !word.is_empty())
                .collect();
            KOREAN
                .iter()
                .filter(|(metaphor, alone)| {
                    words.iter().any(|word| {
                        word.match_indices(metaphor).any(|(at, _)| {
                            if !alone {
                                return true;
                            }
                            let before = &word[..at];
                            let after = &word[at + metaphor.len()..];
                            let counted = before
                                .chars()
                                .next_back()
                                .is_some_and(|c| c.is_ascii_digit() || c == '}');
                            (before.is_empty() || counted) && PARTICLES.contains(&after)
                        })
                    })
                })
                .map(|(metaphor, _)| *metaphor)
                .collect()
        }

        let page = include_str!("../../../ui/index.html");
        let mut said: Vec<(String, String)> = Vec::new();
        for prefix in ["data-i18n=\"settings.typesafe.", "data-i18n=\"jev."] {
            let mut rest = page;
            while let Some(at) = rest.find(prefix) {
                rest = &rest[at + "data-i18n=\"".len()..];
                let key = rest.split('"').next().unwrap_or_default().to_string();
                let text = rest
                    .split_once('>')
                    .map(|(_, body)| body.split('<').next().unwrap_or_default())
                    .unwrap_or_default();
                said.push((format!("index.html {key}"), text.trim().to_string()));
            }
        }
        let scripts = [
            ("shell-jev.js", include_str!("../../../ui/shell-jev.js")),
            (
                "shell-settings.js (TypeSafe)",
                between(
                    include_str!("../../../ui/shell-settings.js"),
                    "/* ---- TypeSafe (Jev) ----",
                    "\n/* ---- ",
                ),
            ),
        ];
        for (file, source) in scripts {
            for literal in literals(source) {
                if hangul(&literal) {
                    said.push((file.to_string(), literal));
                }
            }
        }
        let mut offenders: Vec<String> = said
            .iter()
            .flat_map(|(from, text)| {
                metaphors_in(text)
                    .into_iter()
                    .map(move |word| format!("{from}: 「{word}」 in {text}"))
            })
            .collect();

        let i18n = include_str!("../../../ui/shell-i18n.js");
        let english = between(i18n, "\n  en: {\n", "\n  ja: {\n");
        for line in english.lines() {
            let line = line.trim();
            let Some((key, value)) = line
                .strip_prefix('"')
                .and_then(|rest| rest.split_once("\": \""))
            else {
                continue;
            };
            if !(key.starts_with("settings.typesafe.") || key.starts_with("jev.")) {
                continue;
            }
            for word in value
                .split(|c: char| !c.is_ascii_alphabetic())
                .map(str::to_ascii_lowercase)
            {
                if ENGLISH.contains(&word.as_str()) {
                    offenders.push(format!("en {key}: “{word}” in {value}"));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "{} Jev string(s) speak the codebase's metaphors — say what a person \
             means (기능·판단·요청·응답·정확도·표본·기준·자동 적용), or add the \
             plain use to PLAIN_USES with its reason:\n{}",
            offenders.len(),
            offenders.join("\n")
        );

        // The tables that word a token zo answered — the line a judgment
        // turned on (`JEV_LINES`), why a feature's rows compared nothing or
        // its record draws no confidence bar (`JEV_REASONS`, t-9935), and why
        // an agent's work is not asked or what its hooks miss (`JEV_UNSEEN`,
        // t-10916) — speak every catalog under every key they name, so a
        // token is never read in Korean in another language. A token no table
        // words stands on the screen as itself; that is the tables' reach, not
        // a word missing from a catalog.
        let jev = include_str!("../../../ui/shell-jev.js");
        let catalogs = [
            ("en", english),
            ("ja", between(i18n, "\n  ja: {\n", "\n  zh: {\n")),
            ("zh", between(i18n, "\n  zh: {\n", "\n  es: {\n")),
            ("es", between(i18n, "\n  es: {\n", "\n};")),
        ];
        let mut unspoken: Vec<String> = Vec::new();
        for table in [
            "const JEV_LINES = Object.freeze({",
            "const JEV_REASONS = Object.freeze({",
            "const JEV_UNSEEN = Object.freeze({",
        ] {
            let keys: Vec<&str> = between(jev, table, "\n});")
                .split("key: \"")
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
                .collect();
            assert!(!keys.is_empty(), "`{table}` words no token");
            for key in keys {
                for (language, catalog) in catalogs {
                    if !catalog.contains(&format!("\"{key}\": \"")) {
                        unspoken.push(format!("{language} {key}"));
                    }
                }
            }
        }
        assert!(
            unspoken.is_empty(),
            "a token's words are missing from a catalog:\n{}",
            unspoken.join("\n")
        );
    }

    /// The Jev surfaces' styles speak in tokens (t-6277 D10): every size, ink,
    /// weight and tint on the dashboard and on the settings card's Jev part is
    /// a `var(--…)` from tokens.css, so the two treatments and the contrast the
    /// window suite measures (`contrastTable`) are decided in one place. A
    /// share of a box (0%, 50%, 100%) is geometry, not a token.
    #[test]
    fn the_jev_styles_read_every_size_ink_and_weight_from_tokens() {
        fn block<'a>(css: &'a str, from: &str, to: &str) -> &'a str {
            let start = css.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &css[start..];
            &rest[..rest
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}`"))]
        }
        let css = include_str!("../../../ui/shell.css");
        let blocks = [
            (
                "dashboard",
                block(css, "/* ---- Jev dashboard (t-5807)", ".skills-view {"),
            ),
            (
                "settings card",
                block(
                    css,
                    "/* ---- Jev on the settings card",
                    "/* A row whose mode nothing can reach",
                ),
            ),
        ];
        let mut literals = Vec::new();
        for (surface, text) in blocks {
            let bytes = text.as_bytes();
            let mut at = 0;
            while at < bytes.len() {
                let byte = bytes[at];
                let starts_number = byte.is_ascii_digit()
                    && (at == 0
                        || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'-'));
                if starts_number {
                    let end = text[at..]
                        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                        .map_or(text.len(), |len| at + len);
                    let number = &text[at..end];
                    let unit: String = text[end..]
                        .chars()
                        .take_while(char::is_ascii_alphabetic)
                        .collect();
                    let percent = text[end..].starts_with('%');
                    let geometry = percent && ["0", "50", "100"].contains(&number);
                    if (unit == "px" || percent) && !geometry {
                        literals.push(format!(
                            "{surface}: {number}{}",
                            if percent { "%" } else { "px" }
                        ));
                    }
                    at = end.max(at + 1);
                    continue;
                }
                at += 1;
            }
            for (at, _) in text.match_indices("font-weight:") {
                let value = text[at + "font-weight:".len()..].trim_start();
                if value.starts_with(|c: char| c.is_ascii_digit()) {
                    literals.push(format!(
                        "{surface}: font-weight {}",
                        &value[..3.min(value.len())]
                    ));
                }
            }
            for marker in ["#", "rgb(", "rgba(", "hsl("] {
                for (at, _) in text.match_indices(marker) {
                    let after = &text[at + marker.len()..];
                    let colour =
                        marker != "#" || after.starts_with(|c: char| c.is_ascii_hexdigit());
                    if colour {
                        literals.push(format!(
                            "{surface}: {marker}{}",
                            &after[..6.min(after.len())]
                        ));
                    }
                }
            }
        }
        assert!(
            literals.is_empty(),
            "the Jev styles spell a size, ink or weight of their own — name it in \
             tokens.css's Jev block:\n  {}",
            literals.join("\n  ")
        );
    }

    /// The settings harness's fake backend answers every seat the table names,
    /// with the settings key it writes and the modes it offers, so the pane is
    /// driven by the words and meanings it will read. A seat added to the table
    /// turns this red until the fixture carries it too.
    #[test]
    fn the_settings_harness_mirrors_the_rows() {
        let harness = include_str!("../../../ui/tests/settings.mjs");
        let list = &harness[harness.find("const JEV_SEATS").expect("the fixture")..];
        let list = &list[..list.find("]);").expect("the fixture closes")];
        let mirrored: Vec<&str> = list
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("id:"))
            .collect();
        let expected: Vec<String> = JEV_USES
            .iter()
            .map(|row| {
                let modes: Vec<&str> = row.modes.iter().map(|mode| mode.key()).collect();
                // The setting a feature split off another reads while it has
                // no word of its own (t-6877), named only where there is one.
                let follows = row
                    .follows
                    .map(|setting| format!(", follows: \"{setting}\""))
                    .unwrap_or_default();
                format!(
                    "Object.freeze({{ id: \"{}\", setting: \"{}\", modes: \"{}\", recommended: \"{}\"{follows} }}),",
                    row.id,
                    row.setting,
                    modes.join(" "),
                    row.recommended.key()
                )
            })
            .collect();
        assert_eq!(mirrored, expected);

        // And the meanings behind those words, which the fixture keeps once.
        let meanings = &harness[harness
            .find("const TYPESAFE_DECISION_MODES")
            .expect("the mode list")..];
        let meanings = &meanings[..meanings.find("]);").expect("the mode list closes")];
        let said: Vec<&str> = meanings
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("mode:"))
            .collect();
        let every: Vec<String> = JevMode::ALL
            .iter()
            .map(|mode| {
                format!(
                    "Object.freeze({{ mode: \"{}\", asks: {}, applies: {}, automatic: {} }}),",
                    mode.key(),
                    mode.asks(),
                    mode.applies(),
                    mode.automatic()
                )
            })
            .collect();
        assert_eq!(said, every);
    }

    /// A mode word is spelled in the Jev use table and nowhere its readers
    /// live: zo's executors, parser, doctor row and key check, the window's
    /// settings, its commands and its browser recovery, and the pane. A file is
    /// read up to where its tests begin; a file Jev shares with others, only
    /// where Jev is.
    #[test]
    fn the_mode_words_are_spelled_only_in_the_use_table() {
        // Up to the file's test module — not its first test attribute, which
        // can sit on one test-only item above the code that matters.
        fn product(source: &str) -> &str {
            source
                .split("#[cfg(test)]\nmod tests")
                .next()
                .unwrap_or(source)
        }
        fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
            let start = source.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &source[start..];
            let end = rest[from.len()..]
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}` at `{to}`"));
            &rest[..from.len() + end]
        }
        let page = include_str!("../../../ui/index.html");
        let readers = [
            (
                "zo routing judgment",
                product(include_str!(
                    "../../../zo-ide/crates/tools/src/misc_tools/smart_router/decision_shadow.rs"
                )),
            ),
            (
                "zo recall judgment",
                product(include_str!(
                    "../../../zo-ide/crates/tools/src/misc_tools/smart_router/rerank_shadow.rs"
                )),
            ),
            (
                "zo step effort seat",
                product(include_str!(
                    "../../../zo-ide/crates/tools/src/misc_tools/smart_router/step_effort.rs"
                )),
            ),
            (
                "zo settings",
                between(
                    include_str!(
                        "../../../zo-ide/crates/tools/src/misc_tools/smart_router/settings.rs"
                    ),
                    "/// How a Jev use is set",
                    "/// The plan scorer's knobs",
                ),
            ),
            (
                "zo doctor",
                between(
                    include_str!("../../../zo-ide/crates/zo-ide/src/doctor.rs"),
                    "fn inspect_decision_shadow(",
                    "fn percent(",
                ),
            ),
            (
                "zo key check",
                product(include_str!(
                    "../../../zo-ide/crates/zo-ide/src/decision_shadow_cli.rs"
                )),
            ),
            (
                "window settings",
                product(include_str!("typesafe_settings.rs")),
            ),
            ("window commands", product(include_str!("cmd/typesafe.rs"))),
            (
                "window errand",
                product(include_str!("computer_use/errand.rs")),
            ),
            (
                "window screen judge",
                product(include_str!("computer_use/errand/live.rs")),
            ),
            ("window wire", product(include_str!("systemone.rs"))),
            (
                "window worker room",
                product(include_str!("cmd/worker_room.rs")),
            ),
            (
                "window stall cause",
                product(include_str!("orchestration/stall_cause.rs")),
            ),
            (
                "window mail triage",
                product(include_str!("orchestration/mail_triage.rs")),
            ),
            (
                "window browser read",
                product(include_str!("browser_read.rs")),
            ),
            (
                "window walk",
                product(include_str!("computer_use/errand/walk.rs")),
            ),
            (
                "window goal walk",
                product(include_str!("computer_use/errand/desk.rs")),
            ),
            (
                "pane script",
                between(
                    include_str!("../../../ui/shell-settings.js"),
                    "/* ---- TypeSafe (Jev) ----",
                    "\n/* ---- ",
                ),
            ),
            ("dashboard script", include_str!("../../../ui/shell-jev.js")),
            // The card's switch and the features' words — where a mode word
            // could stand now that no select offers one (§6.1).
            (
                "pane switch",
                between(page, "data-jev-switch-row", "</label>"),
            ),
            (
                "pane features",
                between(page, "<template id=\"jev-features\">", "</template>"),
            ),
        ];
        for (reader, source) in readers {
            for mode in JevMode::ALL {
                assert!(
                    !source.contains(&format!("\"{}\"", mode.key())),
                    "{reader} spells the mode word `{}` — read it from zerocode_core::jev",
                    mode.key()
                );
            }
        }
    }

    /// The gate in front of the routing seat is on the card, offers the four
    /// words the core table has, and spells none of them.
    ///
    /// Everything this pane says about the classifier is a claim about zo's
    /// routing, so the claim is held to zo's own source in
    /// `zerocode_core::jev` (`the_routing_seat_is_only_asked_under_the_probing_word`);
    /// what is held HERE is that the card reads that table rather than
    /// carrying a second copy of it.
    #[test]
    fn the_classifier_stands_on_the_card_and_spells_no_word_of_its_own() {
        let page = include_str!("../../../ui/index.html");
        let choices = &page[page
            .find("id=\"route-classifier-choices\"")
            .expect("the card has no classifier choices")..];
        let choices = &choices[..choices.find("</div>").expect("the choices close")];
        assert!(
            !choices.contains("<input") && !choices.contains("<option"),
            "the classifier lists a word of its own: {choices}"
        );
        // The row the gate stands in front of is named by the backend, so the
        // page carries a warning with no seat written into it.
        assert!(
            page.contains("data-jev-unreachable"),
            "no row can say its mode cannot be reached"
        );
        assert!(
            !page.contains("data-jev-unreachable-routing"),
            "the warning names its seat instead of being told which one"
        );

        // Each reader is cut to where the classifier is, because two of these
        // four words are ordinary English elsewhere in the same file and one
        // of them (`off`) is also a Jev mode word, which a different contract
        // above already holds.
        fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
            let start = source.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &source[start..];
            let end = rest[from.len()..]
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}` at `{to}`"));
            &rest[..from.len() + end]
        }
        let script = include_str!("../../../ui/shell-settings.js");
        let readers = [
            (
                "window settings",
                include_str!("typesafe_settings.rs")
                    .split("#[cfg(test)]")
                    .next()
                    .unwrap_or_default(),
            ),
            ("window commands", include_str!("cmd/typesafe.rs")),
            (
                "pane script",
                between(
                    script,
                    "function paintClassifierChoices(",
                    "\nasync function",
                ),
            ),
            (
                "pane script gate",
                between(
                    script,
                    "el(\"route-classifier-choices\")?.addEventListener",
                    "\n  });",
                ),
            ),
            // The classifier's card, up to its status line.
            (
                "pane card",
                between(
                    page,
                    "id=\"route-classifier-card\"",
                    "id=\"route-classifier-status\"",
                ),
            ),
        ];
        for (reader, source) in readers {
            for mode in ClassifierMode::ALL {
                assert!(
                    !source.contains(&format!("\"{}\"", mode.key())),
                    "{reader} spells the classifier word `{}` — read it from \
                     zerocode_core::jev::ClassifierMode",
                    mode.key()
                );
            }
        }
        assert_eq!(CLASSIFIER_SETTING, "autoClassifier");
    }

    /// The harness's fake backend presses the switch the way the core does
    /// (§6.1): the door's own object and the every-folder word are the
    /// core's, and the card asks the switch's own door.
    #[test]
    fn the_settings_harness_mirrors_the_switch() {
        let harness = include_str!("../../../ui/tests/settings.mjs");
        assert!(
            harness.contains(&format!(
                "const JEV_SETTINGS_KEY = \"{}\"",
                door::JEV_SETTINGS_KEY
            )),
            "the harness writes the switch under another object than the door reads"
        );
        assert!(
            harness.contains(&format!(
                "const JEV_EVERY_WORKSPACE = \"{}\"",
                door::EVERY_WORKSPACE
            )),
            "the harness's every-folder word is not the core's"
        );
        assert!(harness.contains("case \"set_jev_enabled\""));
    }

    /// The harness's fake backend answers the model pin the way the window
    /// does (t-6187): the core's key and the core's alias, and the card
    /// asks the pin's own door.
    #[test]
    fn the_settings_harness_mirrors_the_model_pin() {
        let harness = include_str!("../../../ui/tests/settings.mjs");
        assert!(
            harness.contains(&format!("const JEV_MODEL_SETTING = \"{MODEL_SETTING}\"")),
            "the harness pins another key than the door reads"
        );
        assert!(
            harness.contains(&format!("const JEV_MODEL_ALIAS = \"{DEFAULT_MODEL}\"")),
            "the harness's alias is not the core's"
        );
        assert!(harness.contains("case \"set_jev_model\""));
    }

    /// The harness's fake backend answers the classifier the way the window
    /// does: the four words, what each one does, and which seat it gates.
    #[test]
    fn the_settings_harness_mirrors_the_classifier() {
        let harness = include_str!("../../../ui/tests/settings.mjs");
        let list = &harness[harness.find("const CLASSIFIER_MODES").expect("the fixture")..];
        let list = &list[..list.find("]);").expect("the fixture closes")];
        let said: Vec<&str> = list
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("mode:"))
            .collect();
        let every: Vec<String> = ClassifierMode::ALL
            .iter()
            .map(|mode| {
                format!(
                    "Object.freeze({{ mode: \"{}\", runs: {}, markers: {}, probes: {}, reaches: {} }}),",
                    mode.key(),
                    mode.runs(),
                    mode.markers(),
                    mode.probes(),
                    mode.reaches()
                )
            })
            .collect();
        assert_eq!(said, every);
        assert!(
            harness.contains(&format!(
                "const CLASSIFIER_SETTING = \"{CLASSIFIER_SETTING}\""
            )),
            "the fixture writes a different settings key"
        );
    }

    /// The card's gate answers the file it reads, the seat it stands in front
    /// of, and refuses a word the setting does not offer.
    #[test]
    fn the_gate_reads_its_file_and_refuses_a_word_zo_would_not_honour() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let keys = HeldKeys::default();
        let read = || {
            read_settings(&path, &keys, true)
                .expect("settings")
                .classifier
        };

        // Nothing written: zo reads that as the probing word, so the card says
        // the seat below is reachable.
        let untouched = read();
        assert_eq!(untouched.setting, CLASSIFIER_SETTING);
        assert_eq!(untouched.mode, ClassifierMode::Probed.key());
        assert!(untouched.probes);
        assert!(untouched.reaches);
        assert_eq!(
            untouched.gates, ROUTING.id,
            "the gate names the seat it gates"
        );
        assert_eq!(untouched.modes.len(), ClassifierMode::ALL.len());

        // A word this setting does not offer is refused rather than written:
        // zo would read it as the provider-free verdict and the seat would go
        // quiet without anybody being told.
        assert!(set_classifier(&path, "surprise").is_err());
        assert!(set_classifier(&path, "PROBED").is_err());
        assert_eq!(read().mode, ClassifierMode::Probed.key());

        // A written word stands, and nothing else of the file moves.
        std::fs::write(
            &path,
            br#"{"providers":[{"name":"keep"}],"smart":{"decisionShadow":"on"}}"#,
        )
        .expect("seed");
        set_classifier(&path, ClassifierMode::Deterministic.key()).expect("set");
        let quiet = read();
        assert_eq!(quiet.mode, ClassifierMode::Deterministic.key());
        assert!(!quiet.probes, "the provider-free word reaches no probe");
        assert!(
            quiet.reaches,
            "the provider-free word still reaches the routing seat (t-6346)"
        );
        set_classifier(&path, ClassifierMode::Off.key()).expect("set");
        assert!(
            !read().reaches,
            "with automatic routing off nothing reaches the seat"
        );
        set_classifier(&path, ClassifierMode::Deterministic.key()).expect("set");
        let root = crate::api_routers::read_zo_settings_root(&path).expect("root");
        assert_eq!(
            root.get("providers")
                .and_then(|rows| rows.as_array())
                .map(Vec::len),
            Some(1),
            "moving the gate moved the router rows"
        );
        assert_eq!(
            read_settings(&path, &keys, true)
                .expect("settings")
                .switches
                .iter()
                .find(|row| row.id == ROUTING.id)
                .map(|row| row.mode),
            Some(JevMode::On.key()),
            "moving the gate moved the seat it gates"
        );
    }

    /// Every outcome token a feature's week can carry has its words in the
    /// one table the dashboard's chips and the card's key check both read
    /// (`JEV_TOKENS`, t-6243 D5): each of zo's wire failures, each of the
    /// door's refusals but `off`, which is a mode word the dashboard never
    /// spells, and each refusal judgment-provider names, which a local
    /// server's `502` carries to the row with a word in each of the four
    /// catalogs. A token zo, the door or judgment-provider gains turns this
    /// red until it is worded.
    #[test]
    fn the_failures_the_pane_words_are_zos_tokens() {
        fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
            let start = source.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &source[start..];
            let end = rest[from.len()..]
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}` at `{to}`"));
            &rest[..from.len() + end]
        }
        let zo = include_str!("../../../zo-ide/crates/api/src/systemone.rs");
        let wire: Vec<&str> = between(zo, "pub const fn token(self)", "\n    }")
            .lines()
            .filter_map(|line| line.split_once("=> \""))
            .filter_map(|(_, rest)| rest.split('"').next())
            .collect();
        assert!(
            wire.contains(&"unauthorized") && wire.contains(&"no_key"),
            "zo's failure tokens were not read: {wire:?}"
        );
        let door = zerocode_core::jev::door::Refused::ALL
            .into_iter()
            .filter(|refused| *refused != zerocode_core::jev::door::Refused::Off)
            .map(zerocode_core::jev::door::Refused::token);
        let refusals: Vec<&str> = between(
            include_str!("../../judgment-provider/src/lib.rs"),
            "pub const fn token(self)",
            "\n    }",
        )
        .lines()
        .filter_map(|line| line.split_once("=> \""))
        .filter_map(|(_, rest)| rest.split('"').next())
        .collect();
        assert!(
            refusals.contains(&judgment_provider::Refusal::DeadlineBudget.token()),
            "judgment-provider's refusal words were not read: {refusals:?}"
        );
        let table = between(
            include_str!("../../../ui/shell-jev.js"),
            "const JEV_TOKENS = Object.freeze({",
            "\n});",
        );
        for token in wire
            .iter()
            .copied()
            .chain(door)
            .chain(refusals.iter().copied())
        {
            assert!(
                table.contains(&format!("\n  {token}: {{")),
                "the dashboard's token table does not word `{token}`"
            );
        }
        let catalogs = include_str!("../../../ui/shell-i18n.js");
        for refusal in &refusals {
            let key = table
                .split_once(&format!("\n  {refusal}: {{"))
                .and_then(|(_, row)| row.split_once("key: \""))
                .and_then(|(_, rest)| rest.split('"').next())
                .unwrap_or_default();
            assert_eq!(
                catalogs.matches(&format!("\"{key}\":")).count(),
                4,
                "`{refusal}`'s words `{key}` are not in each of the four catalogs"
            );
        }
        // The card's key check says its failures from the same rows.
        let check = between(
            include_str!("../../../ui/shell-settings.js"),
            "function typesafeCheckFailure(",
            "\n}",
        );
        assert!(
            check.contains("jevTokenRow("),
            "the key check words its failures somewhere else"
        );
    }

    /// Every cause zo's door rests a seat for has the panel's words
    /// (t-58193): a row in the panel's cause table, whose Korean is the row's
    /// own, and a word in each of the four catalogs — never zo's word itself.
    /// The sentence that counts a rest's unanswered requests fills the count
    /// from the field zo sends, so no catalog spells the number. A cause the
    /// door gains turns this red until it is worded.
    #[test]
    fn every_cause_a_seat_rests_for_has_the_panels_words() {
        let gate =
            include_str!("../../../zo-ide/crates/tools/src/misc_tools/smart_router/jev_gate.rs");
        let causes: Vec<&str> = gate
            .split_once("impl RestCause {")
            .map_or("", |(_, rest)| {
                rest.split_once("\n}").map_or(rest, |(body, _)| body)
            })
            .lines()
            .filter_map(|line| line.split_once("=> \""))
            .filter_map(|(_, rest)| rest.split('"').next())
            .collect();
        assert!(
            causes.contains(&"waiting") && causes.contains(&"timeout"),
            "zo's rest causes were not read: {causes:?}"
        );
        let table = include_str!("../../../ui/shell-jev.js")
            .split_once("const JEV_REST_CAUSES = Object.freeze({")
            .map_or("", |(_, rest)| {
                rest.split_once("\n});").map_or(rest, |(table, _)| table)
            });
        let catalogs = include_str!("../../../ui/shell-i18n.js");
        for cause in causes {
            let row = table
                .lines()
                .find(|line| line.trim_start().starts_with(&format!("{cause}: {{")))
                .unwrap_or_else(|| panic!("the panel's cause table does not word `{cause}`"));
            let key = row
                .split_once("key: \"")
                .and_then(|(_, rest)| rest.split('"').next())
                .unwrap_or_default();
            assert_eq!(
                catalogs.matches(&format!("\"{key}\":")).count(),
                4,
                "`{cause}`'s words `{key}` are not in each of the four catalogs"
            );
        }
        let sentences: Vec<&str> = catalogs
            .lines()
            .filter(|line| line.trim_start().starts_with("\"jev.rest.unanswered\":"))
            .collect();
        assert!(
            sentences.len() == 4 && sentences.iter().all(|line| line.contains("{{count}}")),
            "every catalog's rest sentence counts with the number zo sends: {sentences:?}"
        );
    }

    /// The day's count the dashboard draws is the door's own: the file the
    /// door counts one byte per request into, for the day the window counts
    /// in, against the limit the door reads — and a day nothing was sent is
    /// zero, a person who set no limit has none (t-6243 D1).
    #[test]
    fn the_day_is_counted_where_the_door_counts_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        assert_eq!(
            read_day(&path),
            DayBudget {
                sent: 0,
                most: None
            },
            "no file and no setting is nothing sent and no limit"
        );
        let today = count::requests_path(dir.path(), &crate::systemone::today());
        for _ in 0..3 {
            count::count_one(&today).expect("count one");
        }
        std::fs::write(
            &path,
            r#"{"smart":{"jev":{"dailyRequests":500,"workspaces":["/x"]}}}"#,
        )
        .expect("write settings");
        assert_eq!(
            read_day(&path),
            DayBudget {
                sent: 3,
                most: Some(500)
            }
        );
        let drawn = serde_json::to_value(read_day(&path)).expect("serializes");
        assert_eq!(drawn, serde_json::json!({ "sent": 3, "most": 500 }));
    }

    /// The summary flag the window passes is one zo's CLI documents.
    #[test]
    fn the_summary_sessions_flag_is_the_one_zo_documents() {
        let zo = include_str!("../../../zo-ide/crates/zo-ide/src/jev_cli.rs");
        let documented = format!("zo {}", ZO_JEV_SUMMARY_ARGS[..2].join(" "));
        assert!(
            zo.contains(&documented),
            "zo no longer documents `{documented}`"
        );
        assert!(
            zo.contains(&format!("[{ZO_JEV_SUMMARY_SESSIONS_FLAG} <sessions-dir>]")),
            "zo no longer documents `{ZO_JEV_SUMMARY_SESSIONS_FLAG}`"
        );
        assert!(
            zo.contains(&format!("[{ZO_JEV_SUMMARY_CWD_FLAG} <dir>]")),
            "zo no longer documents `{ZO_JEV_SUMMARY_CWD_FLAG}`"
        );
        assert!(
            zo.contains(&format!("[{ZO_JEV_SUMMARY_RECENT_FLAG} <n>]")),
            "zo no longer documents `{ZO_JEV_SUMMARY_RECENT_FLAG}`"
        );
    }

    /// The dashboard's fields — the days, the recent list, the applied count
    /// and the door's refusals — are read when zo sends them and stay empty
    /// from a zo that does not, so the card keeps drawing beside an older zo.
    #[test]
    fn a_summary_carries_the_days_and_the_recent_list_when_zo_sends_them() {
        let stdout = br#"{"windowDays":7,"judgedEveryRows":20,"seats":[
          {"id":"summon","setting":"summonChoice","mode":"on","ledger":"summon-choice.jsonl",
           "found":"/x/summon-choice.jsonl",
           "today":{"rows":2,"answered":2,"refused":0,"applied":0,"refusals":[],"answeredShare":1.0,
                    "answeredLowerBound":0.34,"called":2,"requests":2,"redactedLines":4,"inputTokens":0,
                    "p50Ms":227,"p95Ms":236,"failures":[]},
           "week":{"rows":50,"answered":48,"refused":2,"applied":0,
                   "refusals":[{"token":"not_consented","rows":2}],
                   "answeredShare":0.96,"answeredLowerBound":0.86,"called":48,"requests":48,
                   "redactedLines":100,"inputTokens":0,"p50Ms":230,"p95Ms":410,
                   "failures":[{"token":"not_consented","rows":2}]},
           "costUsd":0.01,"askedModel":"jev-latest","model":"jev-1.13.0",
           "riseFloorPermille":900,"clearsRiseFloor":false,"rowsToNextJudgment":10,
           "judged":{"window":{"rows":34,"answered":34,"answeredShare":1.0,"answeredLowerBound":0.89,
                     "called":34,"requests":34,"redactedLines":0,"inputTokens":0,"p50Ms":230,"p95Ms":410,
                     "failures":[]},"windowWanted":34,
                     "agreement":{"compared":44,"agreed":16,"lowerBound":0.24,"controlRows":0,
                                  "baselineCompared":44,"baselineAgreed":40,"baselineShare":0.91,"notCompared":3,
                                  "notComparedBy":{"not_offered":2,"pinned":1}}},
           "agreementWeek":{"compared":50,"agreed":20,"notCompared":5,
                            "notComparedBy":{"not_offered":3,"pinned":2}},
           "baseline":"todays_rule","negativesWanted":3,
           "stand":"recording","applies":true,
           "verdict":{"verdict":"hold","line":"answered","cutModel":"jev-1.12.0"},
           "days":[{"startMs":1789900000000,"tally":{"rows":3,"answered":3,"answeredShare":1.0,
                    "answeredLowerBound":0.43,"p50Ms":220},
                    "agreement":{"compared":3,"agreed":1,"notCompared":1,"notComparedBy":{"pinned":1}}}],
           "recent":[{"at":1790001955550,"outcome":"answered","elapsedMs":227,"cached":false,
                      "asked":{"task":"t-5807","worker":"w-5814","options":5},"answered":"claude",
                      "confidence":0.19,"applied":null,"agreed":true,"followed":null}]}
        ]}"#;
        let seats = read_summary(stdout).expect("a summary");
        let summon = &seats[0];
        // The id asked, the version that answered and the version cut away
        // ride through to the card (t-6187).
        assert_eq!(summon.asked_model.as_deref(), Some("jev-latest"));
        assert_eq!(summon.model.as_deref(), Some("jev-1.13.0"));
        assert_eq!(
            summon
                .verdict
                .as_ref()
                .and_then(|verdict| verdict.cut_model.as_deref()),
            Some("jev-1.12.0")
        );
        let drawn = serde_json::to_value(summon).expect("serializes");
        assert_eq!(drawn["askedModel"], "jev-latest");
        assert_eq!(drawn["verdict"]["cutModel"], "jev-1.12.0");
        assert_eq!(summon.clears_rise_floor, Some(false));
        assert_eq!(summon.week.p50_ms, Some(230));
        assert_eq!(summon.week.called, 48);
        assert_eq!(
            summon.week.refusals,
            vec![SeatFailure {
                token: "not_consented".to_string(),
                rows: 2
            }]
        );
        let judged = summon.judged.as_ref().expect("judged");
        assert_eq!(
            (judged.agreement.compared, judged.agreement.agreed),
            (44, 16)
        );
        // The cheapest reader beside it, and what compared nothing, ride
        // through to the drawer (t-6342).
        assert_eq!(
            (
                judged.agreement.baseline_compared,
                judged.agreement.baseline_agreed,
                judged.agreement.not_compared
            ),
            (44, 40, 3)
        );
        assert_eq!(summon.baseline.as_deref(), Some("todays_rule"));
        assert_eq!(summon.negatives_wanted, Some(3));
        assert_eq!(drawn["judged"]["agreement"]["baselineShare"], 0.91);
        // Why the rows that compared nothing say so, word by word (t-9556):
        // the dashboard names them in a row and lists them in its drawer
        // (t-9935) — the judged window's, the week's and each day's.
        let words = |pairs: &[(&str, usize)]| -> std::collections::BTreeMap<String, usize> {
            pairs
                .iter()
                .map(|(word, count)| ((*word).to_string(), *count))
                .collect()
        };
        assert_eq!(
            judged.agreement.not_compared_by,
            words(&[("not_offered", 2), ("pinned", 1)])
        );
        assert_eq!(
            summon
                .agreement_week
                .as_ref()
                .map(|week| week.not_compared_by.clone()),
            Some(words(&[("not_offered", 3), ("pinned", 2)]))
        );
        assert_eq!(
            summon.days[0].agreement.not_compared_by,
            words(&[("pinned", 1)])
        );
        assert_eq!(
            drawn["agreementWeek"]["notComparedBy"],
            serde_json::json!({ "not_offered": 3, "pinned": 2 })
        );
        assert_eq!(
            drawn["days"][0]["agreement"]["notComparedBy"],
            serde_json::json!({ "pinned": 1 })
        );
        assert_eq!(summon.days.len(), 1);
        assert_eq!(summon.days[0].start_ms, 1_789_900_000_000);
        assert_eq!(summon.days[0].tally.rows, 3);
        assert_eq!(summon.days[0].agreement.agreed, 1);
        assert_eq!(
            summon.days[0].tally.p95_ms, None,
            "a day zo left short is read short"
        );
        assert_eq!(summon.recent.len(), 1);
        let one = &summon.recent[0];
        assert_eq!(one.asked.get("task"), Some(&Value::from("t-5807")));
        assert_eq!(one.answered, Value::from("claude"));
        assert_eq!((one.applied, one.agreed), (None, Some(true)));

        // An older zo sends none of it, and the seat reads with the empties.
        let older = br#"{"seats":[{"id":"summon","stand":"recording","applies":false,
          "today":{"rows":0,"answered":0},"week":{"rows":0,"answered":0}}]}"#;
        let seat = &read_summary(older).expect("a summary")[0];
        assert!(seat.days.is_empty() && seat.recent.is_empty());
        assert_eq!(seat.clears_rise_floor, None);
        assert!(seat.week.refusals.is_empty() && seat.week.failures.is_empty());
        assert_eq!((seat.calibration.as_ref(), seat.apply_share), (None, None));
    }

    /// A seat's act line reaches the dashboard as the screen reads it
    /// (t-9468): the line drawn or why none, the table's line, the answers
    /// at the fixed and drawn lines, and at the table's line the three
    /// numbers — while zo's grid and the row the replay keeps stay in zo's
    /// answer.
    #[test]
    fn a_summary_carries_the_act_line_the_screen_draws_and_not_the_grid() {
        let stdout = br#"{"seats":[{"id":"notify","stand":"recording","applies":false,
          "today":{"rows":1,"answered":1},"week":{"rows":1,"answered":1},
          "calibration":{"readsActLine":true,"marksWanted":40,"actFromPermille":300,"reason":null,
            "grid":[{"fromPermille":0,"marks":100}],"row":{"seat":"notify","grid":[]},
            "fixed":{"fromPermille":850,"marks":50,"applyShare":0.5,"errorPermille":60,
                     "baselineErrorPermille":1000,"underErrorPermille":500},
            "drawn":{"fromPermille":300,"marks":50,"applyShare":0.5,"errorPermille":60,
                     "baselineErrorPermille":1000,"underErrorPermille":500},
            "tableLine":300,"atTableLine":{"fromPermille":300}},
          "applyShare":0.5,"appliedErrorPermille":60,"baselineErrorPermille":1000}]}"#;
        let notify = &read_summary(stdout).expect("a summary")[0];
        let calibration = notify.calibration.as_ref().expect("the act line");
        assert_eq!(
            (
                calibration.act_from_permille,
                calibration.table_line,
                calibration.reason.as_deref()
            ),
            (Some(300), Some(300), None)
        );
        assert_eq!(calibration.fixed.map(|at| at.from_permille), Some(850));
        assert_eq!(
            (
                notify.apply_share,
                notify.applied_error_permille,
                notify.baseline_error_permille
            ),
            (Some(0.5), Some(60), Some(1_000))
        );
        let drawn = serde_json::to_value(notify).expect("serializes");
        assert_eq!(drawn["calibration"]["drawn"]["underErrorPermille"], 500);
        assert!(
            drawn["calibration"].get("grid").is_none() && drawn["calibration"].get("row").is_none(),
            "the grid and the row stay in zo's answer: {}",
            drawn["calibration"]
        );
    }

    /// The command line the window execs is the one zo's CLI documents.
    #[test]
    fn the_check_command_line_is_the_one_zo_documents() {
        let zo = include_str!("../../../zo-ide/crates/zo-ide/src/decision_shadow_cli.rs");
        let documented = format!("zo {}", ZO_KEY_CHECK_ARGS[..2].join(" "));
        assert!(
            zo.contains(&format!("{documented} [{}]", ZO_KEY_CHECK_ARGS[2])),
            "zo no longer documents `{documented}`"
        );
    }

    #[test]
    fn a_summary_is_read_seat_by_seat_and_a_seat_nobody_asked_keeps_its_empties() {
        let stdout = br#"{"windowDays":7,"judgedEveryRows":20,"seats":[
          {"id":"routing","setting":"decisionShadow","mode":"on","ledger":"decision-shadow.jsonl",
           "found":"/x/decision-shadow.jsonl",
           "today":{"rows":1,"answered":1,"answeredShare":1.0,"answeredLowerBound":0.2,"called":1,
                    "requests":1,"redactedLines":2,"inputTokens":9,"p50Ms":616,"p95Ms":616,"failures":[]},
           "week":{"rows":26,"answered":23,"answeredShare":0.8846,"answeredLowerBound":0.7102,"called":26,
                   "requests":26,"redactedLines":30,"inputTokens":90,"p50Ms":616,"p95Ms":4847,
                   "failures":[{"token":"no_key","rows":3}],
                   "guards":{"instructed":2,"walled":1},"controls":{"named":7,"destructiveHeld":2}},
           "costUsd":null,"riseFloorPermille":950,"clearsRiseFloor":false,"rowsToNextJudgment":14,
           "stand":"recording","applies":true,"verdict":{"verdict":"hold","line":"answered"}},
          {"id":"summon","setting":"summonChoice","mode":"auto","ledger":"summon-choice.jsonl",
           "found":null,
           "today":{"rows":0,"answered":0,"answeredShare":null,"answeredLowerBound":null,"called":0,
                    "requests":0,"redactedLines":0,"inputTokens":0,"p50Ms":null,"p95Ms":null,"failures":[]},
           "week":{"rows":0,"answered":0,"answeredShare":null,"answeredLowerBound":null,"called":0,
                   "requests":0,"redactedLines":0,"inputTokens":0,"p50Ms":null,"p95Ms":null,"failures":[]},
           "costUsd":0.0,"riseFloorPermille":null,"clearsRiseFloor":null,"rowsToNextJudgment":null,
           "stand":"recording","applies":false,"verdict":null}
        ]}"#;
        let seats = read_summary(stdout).expect("a summary");
        assert_eq!(seats.len(), 2);

        let routing = &seats[0];
        assert_eq!(routing.id, "routing");
        assert_eq!(routing.week.rows, 26);
        assert_eq!(routing.week.p95_ms, Some(4_847));
        assert_eq!(routing.week.redacted_lines, 30);
        // What a screen seat's guards stopped and the controls it handed over
        // (t-6277 D6), as the core counted them.
        assert_eq!(
            routing.week.guards,
            SeatGuards {
                instructed: 2,
                walled: 1
            }
        );
        assert_eq!(
            routing.week.controls,
            SeatControls {
                named: 7,
                destructive_held: 2
            }
        );
        assert_eq!(routing.rise_floor_permille, Some(950));
        assert_eq!(routing.rows_to_next_judgment, Some(14));
        assert!(
            routing.applies,
            "a person's `on` acts whatever the judge says"
        );
        let verdict = routing.verdict.as_ref().expect("a judged seat");
        assert_eq!(
            (verdict.verdict.as_str(), verdict.line.as_deref()),
            ("hold", Some("answered"))
        );

        let summon = &seats[1];
        assert_eq!(
            summon.week.answered_share, None,
            "never asked is not zero percent"
        );
        assert_eq!(summon.week.p95_ms, None);
        assert_eq!(
            (summon.week.guards, summon.week.controls),
            (SeatGuards::default(), SeatControls::default()),
            "a zo older than the count reads as nothing stopped"
        );
        assert_eq!(
            summon.verdict, None,
            "a seat that never rises is never judged"
        );
        assert!(!summon.applies);
    }

    #[test]
    fn an_answer_this_reader_does_not_understand_is_no_answer() {
        assert_eq!(read_summary(b""), None);
        assert_eq!(read_summary(b"zo: unknown argument 'jev'"), None);
        assert_eq!(read_summary(br#"{"seats":"soon"}"#), None);
        assert_eq!(
            read_summary(br#"{"windowDays":7}"#),
            None,
            "no seats is not an empty card"
        );
        assert_eq!(read_summary(br#"{"seats":[]}"#), Some(Vec::new()));
    }

    /// zo names the mode it asks each seat in for the folder it counted —
    /// for the person's own decisions the lower of their switch and their
    /// own modes, which the settings file alone cannot say — and the card
    /// reads that word, so the reader keeps it and hands it on.
    #[test]
    fn a_summary_keeps_the_mode_zo_reads_each_feature_at() {
        let stdout = br#"{"seats":[
          {"id":"custom","setting":"customDecisions","mode":"off","stand":"recording","applies":false,
           "today":{"rows":0,"answered":0,"answeredShare":null,"answeredLowerBound":null,"called":0,
                    "requests":0,"redactedLines":0,"inputTokens":0,"p50Ms":null,"p95Ms":null,"failures":[]},
           "week":{"rows":0,"answered":0,"answeredShare":null,"answeredLowerBound":null,"called":0,
                   "requests":0,"redactedLines":0,"inputTokens":0,"p50Ms":null,"p95Ms":null,"failures":[]}}
        ]}"#;
        let seats = read_summary(stdout).expect("a summary");
        let sent = serde_json::to_value(&seats).expect("the seats, sent");
        assert_eq!(
            sent[0]["mode"],
            serde_json::json!("off"),
            "the mode zo read never reaches the card: {sent}"
        );
    }

    /// The store refuses a secret line whoever asks it: a draft that never
    /// passed the editor's check is refused at the line core's detector
    /// reads, and nothing is kept (t-62170 stage 3, test 9).
    #[test]
    fn saving_refuses_a_secret_line_without_the_screen() {
        let home = tempfile::tempdir().expect("tempdir");
        let draft = DecisionDraft {
            id: None,
            name: "Release notes".to_string(),
            question: "Did this turn change what a person sees?\nRead the diff first.\n\
                       Authorization: Bearer abc\nThen answer."
                .to_string(),
            answer: AnswerDraft::YesNo {
                yes: "It changed what a person sees.".to_string(),
                no: "Nothing a person sees changed.".to_string(),
            },
            at: custom::Point::TurnEnd,
            reads: BTreeSet::from([custom::Input::Task]),
            folders: None,
            act_from: custom::ACT_FROM_DEFAULT,
            max_per_hour: custom::HOURLY_DEFAULT,
        };
        match save_custom_decision_at(home.path(), draft, 1_792_300_000_000) {
            Err(custom::store::Error::Refused(refusals)) => assert_eq!(
                refusals,
                vec![custom::Refusal::SecretLine {
                    field: custom::Field::Question,
                    line: 3,
                }],
                "refused, but not at the secret line"
            ),
            kept => panic!("the save kept a question with a secret line: {kept:?}"),
        }
        assert!(
            custom::store::list(home.path())
                .expect("the store reads")
                .is_empty(),
            "a refused decision stands in the store"
        );
    }

    /// A refusal reaches the drawer in core's words (t-62170 stage 3): the
    /// rule's word, the word of the field it was found in, the line, and the
    /// option or level it names — so the drawer says it and marks the control
    /// it points at with no word of its own for either. A decision the store
    /// no longer keeps is said apart from a refusal.
    #[test]
    fn a_refusal_reaches_the_drawer_in_cores_words_and_names_where() {
        let secret = custom::Refusal::SecretLine {
            field: custom::Field::Question,
            line: 3,
        };
        let level = custom::Refusal::MeaningLength(custom::Field::Level(2));
        let meaning = custom::Refusal::MeaningLength(custom::Field::Meaning("ship".to_string()));
        let twice = custom::Refusal::OptionName("ship".to_string());
        let refused = DecisionRefusal::from(custom::store::Error::Refused(vec![
            secret.clone(),
            level.clone(),
            meaning.clone(),
            twice.clone(),
            custom::Refusal::NameTaken,
        ]));
        assert_eq!(
            serde_json::to_value(refused).expect("serializes"),
            serde_json::json!({
                "refusals": [
                    { "word": secret.word(), "field": custom::Field::Question.word(),
                      "line": 3, "option": null, "level": null },
                    { "word": level.word(), "field": custom::Field::Level(2).word(),
                      "line": null, "option": null, "level": 2 },
                    { "word": meaning.word(), "field": custom::Field::Meaning(String::new()).word(),
                      "line": null, "option": "ship", "level": null },
                    { "word": twice.word(), "field": null, "line": null, "option": "ship",
                      "level": null },
                    { "word": custom::Refusal::NameTaken.word(), "field": null, "line": null,
                      "option": null, "level": null },
                ],
                "missing": false,
                "message": "",
            }),
            "a refusal crosses as core's words, with the field, line, option and level it names"
        );
        assert_eq!(
            serde_json::to_value(DecisionRefusal::from(custom::store::Error::Missing))
                .expect("serializes"),
            serde_json::json!({ "refusals": [], "missing": true, "message": "" }),
            "a decision the store no longer keeps is said apart from a refusal"
        );
    }

    /// The drawer's check names the least sure line an agent would be told of
    /// a draft — core's line for its first option at its act-from line: a yes,
    /// a choice's first option as written, a score's lowest level as an agent
    /// is shown it — and crosses with its refusals in core's words.
    #[test]
    fn the_drawers_check_names_the_line_an_agent_would_be_told() {
        let draft = DecisionDraft {
            id: None,
            name: "Release notes".to_string(),
            question: "Did this turn change what a person sees?\nAuthorization: Bearer abc"
                .to_string(),
            answer: AnswerDraft::YesNo {
                yes: "It changed what a person sees.".to_string(),
                no: "Nothing a person sees changed.".to_string(),
            },
            at: custom::Point::TurnEnd,
            reads: BTreeSet::from([custom::Input::Task]),
            folders: None,
            act_from: custom::ACT_FROM_DEFAULT,
            max_per_hour: custom::HOURLY_DEFAULT,
        };
        let checked = check_custom_decision(draft.clone());
        assert_eq!(
            checked.line,
            custom::line(&draft.name, AGENT_TOOL_ASK_OPTIONS[0], draft.act_from),
            "a yes or no is told as a yes at its act-from line"
        );
        let wire = serde_json::to_value(&checked).expect("serializes");
        let secret = custom::Refusal::SecretLine {
            field: custom::Field::Question,
            line: 2,
        };
        assert_eq!(
            (&wire["refusals"], &wire["line"]),
            (
                &serde_json::json!([
                    { "word": secret.word(), "field": custom::Field::Question.word(),
                      "line": 2, "option": null, "level": null },
                ]),
                &serde_json::json!(checked.line),
            ),
            "the check crosses with its refusals in core's words and its line"
        );
        let options = vec![
            ChoiceOption {
                name: "ship".to_string(),
                means: "Ship it this week.".to_string(),
            },
            ChoiceOption {
                name: "hold".to_string(),
                means: "Hold it back.".to_string(),
            },
        ];
        let choice = DecisionDraft {
            answer: AnswerDraft::Choice { options },
            ..draft.clone()
        };
        assert_eq!(
            check_custom_decision(choice).line,
            custom::line(&draft.name, "ship", draft.act_from),
            "a choice is told as its first option as written"
        );
        let levels = vec!["Low".to_string(), "Some".to_string(), "High".to_string()];
        let count = levels.len();
        let score = DecisionDraft {
            answer: AnswerDraft::Score { levels },
            ..draft.clone()
        };
        assert_eq!(
            check_custom_decision(score).line,
            custom::line(&draft.name, &custom::shown_level(0, count), draft.act_from),
            "a score is told as its lowest level, as an agent is shown it"
        );
    }

    /// The editor of a person's decisions offers and bounds only what core
    /// holds (t-62170 stage 3): the feature, every limit, the shared hourly
    /// bound as the person's settings hold it, the kinds of answer under the
    /// words core's own files keep, the points with their inputs, and the
    /// modes, each by what it does. The page reads them; it writes none.
    #[test]
    fn the_editor_reads_every_bound_and_word_of_a_persons_decisions_from_core() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let hourly_all = custom::ALL_HOURLY_DEFAULT + 1;
        let document = serde_json::json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: {
            door::CUSTOM_HOURLY_REQUESTS_SETTING: hourly_all,
        } } });
        std::fs::write(&path, document.to_string()).expect("write");
        let settings = read_settings(&path, &HeldKeys::default(), true).expect("settings");
        let sent = serde_json::to_value(&settings).expect("the settings, sent");
        let kinds: Vec<Value> = [
            custom::Answer::YesNo {
                yes: String::new(),
                no: String::new(),
            },
            custom::Answer::Choice(BTreeMap::new()),
            custom::Answer::Score(Vec::new()),
        ]
        .into_iter()
        .map(|answer| {
            let written = custom::Definition::new(
                custom::new_id(),
                String::new(),
                String::new(),
                answer,
                custom::Point::TurnStart,
            )
            .to_bytes()
            .expect("a definition writes");
            serde_json::from_slice::<Value>(&written).expect("its bytes read")["kind"].clone()
        })
        .collect();
        assert_eq!(
            sent["customDecisions"],
            serde_json::json!({
                "feature": zerocode_core::jev::CUSTOM_DECISIONS.id,
                "decisionsMax": custom::DECISIONS_MAX,
                "nameMaxChars": custom::NAME_MAX_CHARS,
                "questionMaxChars": custom::QUESTION_MAX_CHARS,
                "meaningMaxChars": custom::MEANING_MAX_CHARS,
                "options": { "least": custom::OPTIONS.start(), "most": custom::OPTIONS.end() },
                "hourly": {
                    "least": custom::HOURLY_REQUESTS.start(),
                    "most": custom::HOURLY_REQUESTS.end(),
                },
                "hourlyDefault": custom::HOURLY_DEFAULT,
                "hourlyAll": hourly_all,
                "actFrom": { "least": custom::ACT_FROM.start(), "most": custom::ACT_FROM.end() },
                "actFromDefault": custom::ACT_FROM_DEFAULT,
                "trashKeptDays": custom::store::TRASH_KEPT_DAYS,
                "kinds": kinds,
                "points": custom::Point::ALL.map(|at| serde_json::json!({
                    "at": at.key(),
                    "inputs": at.inputs().iter().map(|input| input.key()).collect::<Vec<_>>(),
                })),
                "modes": custom::MODES.map(ModeChoice::of),
            }),
            "the editor offers or bounds something core does not hold"
        );
    }

    /// The editor's six doors (t-62170 stage 3) are registered with the
    /// webview, answered by the settings harness's fake backend, and asked
    /// by the dashboard: a door the page asks that nobody registered is a
    /// button that does nothing, and one the harness never answers is one no
    /// page test presses.
    #[test]
    fn the_decision_editor_asks_six_registered_doors_the_harness_answers() {
        let main = include_str!("main.rs");
        let handlers = main
            .split_once("tauri::generate_handler![")
            .map(|(_, list)| list)
            .expect("the handler list");
        let harness = include_str!("../../../ui/tests/settings.mjs");
        let dashboard = include_str!("../../../ui/shell-jev.js");
        for command in [
            "list_custom_decisions",
            "check_custom_decision",
            "save_custom_decision",
            "delete_custom_decision",
            "restore_custom_decision",
            "set_custom_decision_mode",
        ] {
            assert!(
                handlers.contains(&format!("{command},")),
                "{command} is not registered"
            );
            assert!(
                harness.contains(&format!("case \"{command}\"")),
                "the settings harness never answers {command}"
            );
            assert!(
                dashboard.contains(&format!("invoke(\"{command}\"")),
                "the dashboard never asks {command}"
            );
        }
    }

    /// The editor says every word core and the settings answer hand it in a
    /// person's language: each point, when it asks and what it does when on,
    /// each input, each kind of answer, each field a refusal names and each
    /// refusal has its words in the markup under core's own word
    /// (`#jev-custom-words`), with a key all four catalogs say. A refusal that
    /// names no field points at a part the drawer has. A word core adds turns
    /// this red until a person can read it.
    #[test]
    fn the_decision_editor_words_every_name_core_hands_it() {
        let page = include_str!("../../../ui/index.html");
        let i18n = include_str!("../../../ui/shell-i18n.js");
        let dashboard = include_str!("../../../ui/shell-jev.js");
        let template = &page[page
            .find("<template id=\"jev-custom-words\">")
            .expect("the editor's words")..];
        let template = &template[..template.find("</template>").expect("the template closes")];
        let worded = |kind: &str, word: &str| -> String {
            let marker = format!("data-jev-{kind}=\"{word}\"");
            let node = &template[template
                .find(&marker)
                .unwrap_or_else(|| panic!("the markup has no words for the {kind} {word}"))..];
            let node = &node[..node.find('>').expect("the node opens")];
            let key = node
                .split("data-i18n=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_else(|| panic!("the {kind} {word} has no key"));
            assert!(key.starts_with("jev.custom."), "{kind} {word}: {key}");
            assert_eq!(
                i18n.matches(&format!("\"{key}\"")).count(),
                4,
                "{key} must be in en/ja/zh/es"
            );
            node.to_string()
        };
        let rules = CustomDecisionRules::of(&serde_json::json!({}));
        for kind in &rules.kinds {
            worded("kind", kind);
        }
        for point in &rules.points {
            for kind in ["point", "point-about", "point-act"] {
                worded(kind, point.at.key());
            }
            for input in point.inputs {
                worded("input", input.key());
            }
        }
        for field in [
            custom::Field::Name,
            custom::Field::Question,
            custom::Field::Yes,
            custom::Field::No,
            custom::Field::OptionName(String::new()),
            custom::Field::Meaning(String::new()),
            custom::Field::Level(0),
        ] {
            match field {
                custom::Field::Name
                | custom::Field::Question
                | custom::Field::Yes
                | custom::Field::No
                | custom::Field::OptionName(_)
                | custom::Field::Meaning(_)
                | custom::Field::Level(_) => worded("field", field.word()),
            };
        }
        for refusal in [
            custom::Refusal::Id,
            custom::Refusal::IdTaken,
            custom::Refusal::NameLength,
            custom::Refusal::NameTaken,
            custom::Refusal::QuestionLength,
            custom::Refusal::OptionCount,
            custom::Refusal::OptionName(String::new()),
            custom::Refusal::MeaningLength(custom::Field::Yes),
            custom::Refusal::SecretLine {
                field: custom::Field::Question,
                line: 1,
            },
            custom::Refusal::Reads,
            custom::Refusal::Folders,
            custom::Refusal::ActFrom,
            custom::Refusal::MaxPerHour,
            custom::Refusal::Mode,
            custom::Refusal::TooMany,
            custom::Refusal::TooLarge,
        ] {
            let node = match refusal {
                custom::Refusal::Id
                | custom::Refusal::IdTaken
                | custom::Refusal::NameLength
                | custom::Refusal::NameTaken
                | custom::Refusal::QuestionLength
                | custom::Refusal::OptionCount
                | custom::Refusal::OptionName(_)
                | custom::Refusal::MeaningLength(_)
                | custom::Refusal::SecretLine { .. }
                | custom::Refusal::Reads
                | custom::Refusal::Folders
                | custom::Refusal::ActFrom
                | custom::Refusal::MaxPerHour
                | custom::Refusal::Mode
                | custom::Refusal::TooMany
                | custom::Refusal::TooLarge => worded("refusal", refusal.word()),
            };
            if let Some(at) = node
                .split("data-jev-refusal-at=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
            {
                assert!(
                    dashboard.contains(&format!("jevCustomGroup(\"{at}\"")),
                    "{} points at {at}, a part the drawer does not have",
                    refusal.word()
                );
            }
        }
    }

    /// A decision made in the window comes back from the store as the editor
    /// wrote it, recording first; core's check finds nothing in it and names
    /// the version the store keeps; switched on and then edited into a new
    /// version, it keeps its mode; a mode no decision offers is refused;
    /// deleted, it waits in the trash and comes back as it was; and a choice
    /// that names an option twice is refused at that name (t-62170 stage 3).
    #[test]
    fn a_decision_goes_through_the_editors_doors_and_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        // Core keeps a decision's folders only as absolute paths, and an
        // absolute path on Windows starts at a drive.
        let acme = if cfg!(windows) {
            "C:\\work\\acme"
        } else {
            "/work/acme"
        };
        let settings = serde_json::json!({ SMART_SETTINGS_KEY: { door::JEV_SETTINGS_KEY: {
            door::WORKSPACES_SETTING: [acme],
        } } });
        let option = |name: &str, means: &str| ChoiceOption {
            name: name.to_string(),
            means: means.to_string(),
        };
        let draft = DecisionDraft {
            id: None,
            name: "Release risk".to_string(),
            question: "How risky is this change to ship today?".to_string(),
            answer: AnswerDraft::Choice {
                options: vec![
                    option("hold", "Wait for a person."),
                    option("ship", "Ship it now."),
                ],
            },
            at: custom::Point::TurnEnd,
            reads: BTreeSet::from([custom::Input::Task, custom::Input::EditedPaths]),
            folders: Some(BTreeSet::from([acme.to_string()])),
            act_from: custom::ACT_FROM_DEFAULT,
            max_per_hour: custom::HOURLY_DEFAULT,
        };
        let first = save_custom_decision_at(home, draft.clone(), 1_000).expect("kept");
        let listed = custom_decisions_at(home, &settings).expect("listed");
        assert_eq!(listed.decisions.len(), 1, "one decision is kept");
        let kept = listed.decisions[0].clone();
        let id = kept.draft.id.clone().expect("the store keeps it by an id");
        assert_eq!(
            kept.draft,
            DecisionDraft {
                id: Some(id.clone()),
                ..draft.clone()
            },
            "the editor gets back what it wrote"
        );
        assert_eq!(
            (kept.mode, kept.version.as_str()),
            (JevMode::Shadow.key(), first.as_str()),
            "a new decision records first"
        );
        assert_eq!(
            (listed.folders, listed.everywhere),
            (vec![acme.to_string()], false)
        );
        let checked = check_custom_decision(kept.draft.clone());
        assert_eq!(
            (checked.refusals, checked.version),
            (Vec::new(), first.clone()),
            "core finds nothing in a kept decision, and names its version"
        );

        set_custom_decision_mode_at(home, &id, JevMode::On.key(), 2_000).expect("on");
        let edited = DecisionDraft {
            question: "How risky is this change to ship this week?".to_string(),
            ..kept.draft.clone()
        };
        let second = save_custom_decision_at(home, edited, 3_000).expect("edited");
        assert_ne!(second, first, "an edited question is a new version");
        let listed = custom_decisions_at(home, &settings).expect("listed");
        assert_eq!(
            (
                listed.decisions[0].mode,
                listed.decisions[0].version.as_str()
            ),
            (JevMode::On.key(), second.as_str()),
            "an edit keeps the mode"
        );
        match set_custom_decision_mode_at(home, &id, JevMode::Auto.key(), 4_000) {
            Err(custom::store::Error::Refused(refusals)) => assert_eq!(
                refusals,
                vec![custom::Refusal::Mode],
                "refused, but not as a mode no decision offers"
            ),
            kept => panic!("a decision took the automatic mode: {kept:?}"),
        }

        delete_custom_decision_at(home, &id, 5_000).expect("deleted");
        let listed = custom_decisions_at(home, &settings).expect("listed");
        assert!(
            listed.decisions.is_empty(),
            "a deleted decision is not listed"
        );
        assert_eq!(
            listed
                .trash
                .iter()
                .map(|trashed| trashed.id.as_str())
                .collect::<Vec<_>>(),
            vec![id.as_str()],
            "it waits in the trash"
        );
        custom::store::restore(home, &id).expect("restored");
        let listed = custom_decisions_at(home, &settings).expect("listed");
        assert_eq!(
            (
                listed.decisions[0].mode,
                listed.decisions[0].version.as_str()
            ),
            (JevMode::On.key(), second.as_str()),
            "it comes back as it was"
        );

        let twice = DecisionDraft {
            answer: AnswerDraft::Choice {
                options: vec![option("ship", "Ship it now."), option("ship", "Ship it.")],
            },
            ..draft
        };
        match save_custom_decision_at(home, twice, 6_000) {
            Err(custom::store::Error::Refused(refusals)) => assert!(
                refusals.contains(&custom::Refusal::OptionName("ship".to_string())),
                "refused, but not at the name given twice: {refusals:?}"
            ),
            kept => panic!("a choice that names an option twice was kept: {kept:?}"),
        }
    }
}

/// One seat's numbers, as `zo jev summary --json` reports them and the card
/// draws them beside that seat's switch.
///
/// Every field is optional in the answer and stays optional here: a seat
/// nothing has asked yet has no share and no p95, and a card that drew those
/// as zero would be telling a person their seat is failing.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatNumbers {
    pub id: String,
    /// Where the seat stands, and whether it acts right now.
    pub stand: String,
    pub applies: bool,
    /// The mode zo asks the seat in for the folder it counted: for the
    /// person's own decisions, the lower of their switch and their own modes.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub verdict: Option<SeatVerdict>,
    #[serde(default)]
    pub rows_to_next_judgment: Option<usize>,
    #[serde(default)]
    pub rise_floor_permille: Option<u16>,
    /// Whether the judged window's lower bound clears the rise line; absent
    /// for a seat that never rises or a window nothing has filled.
    #[serde(default)]
    pub clears_rise_floor: Option<bool>,
    pub today: SeatWindow,
    pub week: SeatWindow,
    /// The window the judge read and the agreement over it, for a seat that
    /// rises — absent from a zo that judged on the week.
    #[serde(default)]
    pub judged: Option<SeatJudged>,
    /// Every `agreed` mark of the week, whether or not the seat rises —
    /// absent from a zo older than the label rows (t-5806).
    #[serde(default)]
    pub agreement_week: Option<SeatAgreement>,
    /// The cheapest reader the seat is held against, by the table's word
    /// (`always_same`, `todays_rule`, `none`), and how many times its label
    /// must have said no (t-6342) — absent from a zo older than the baseline.
    #[serde(default)]
    pub baseline: Option<String>,
    #[serde(default)]
    pub negatives_wanted: Option<usize>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// The id the seat asks with — the person's pin or the alias — and the
    /// version the newest answer named (t-6187): the two halves of the line
    /// the card and the dashboard draw under every seat. Absent from a zo
    /// older than the pin, and `model` from a seat nothing answered.
    #[serde(default)]
    pub asked_model: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Today and the days before it, oldest first — the trend the dashboard
    /// draws; empty from a zo that counts only the week.
    #[serde(default)]
    pub days: Vec<SeatDay>,
    /// The seat's last requests, newest first, when the command asked for
    /// them ([`ZO_JEV_SUMMARY_RECENT_FLAG`]); empty otherwise.
    #[serde(default)]
    pub recent: Vec<SeatDecision>,
    /// What the seat's graded answers say of its act line, beside the line
    /// the product reads for it now (t-9468) — absent from a zo older than
    /// the act line, and for a seat that never rises.
    #[serde(default)]
    pub calibration: Option<SeatCalibration>,
    /// At the line the product reads for the seat now: the share of its
    /// answered requests it acts on, how often the marks of what it acts on
    /// say it was wrong, and how often its baseline was on the same marks,
    /// per thousand (t-9468) — absent while no line is read for it.
    #[serde(default)]
    pub apply_share: Option<f64>,
    #[serde(default)]
    pub applied_error_permille: Option<u16>,
    #[serde(default)]
    pub baseline_error_permille: Option<u16>,
    /// The week's requests by who asked them — zo, or the agent of a window
    /// pane (t-10916) — for a seat several programs ask into one ledger;
    /// empty from a zo older than the column. Sent even when empty: a sum
    /// reads every reading's map, and one a reading left out is no map.
    #[serde(default)]
    pub askers_week: std::collections::BTreeMap<String, usize>,
    /// For a seat a pane's moments ask, every agent's line: its requests of
    /// the week, and what its hooks show the seat
    /// ([`crate::jev_scope::with_askers`]); empty for every other seat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub askers: Vec<SeatAsker>,
    /// The file zo read the seat's rows from — kept to say where they are
    /// kept ([`crate::jev_scope::with_reach`]), never sent on: a path of the
    /// person's disk is not the dashboard's to draw.
    #[serde(default, skip_serializing)]
    pub found: Option<String>,
    /// Where the seat's rows are kept — its project's own, or this machine's
    /// and so the same in every scope (t-9091); absent for a seat with no
    /// rows on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<crate::jev_scope::Reach>,
    /// Across every project summed: how many were counted, in how many the
    /// seat acts, and whether its numbers are a sum (t-9091); absent in a
    /// reading of one checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub across: Option<crate::jev_scope::SeatAcross>,
    /// Why the seat rests now and until when, as zo's door reads it
    /// (t-58193): the one field the panel's chip and sentence both read;
    /// absent while it may ask, and from a zo older than the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resting: Option<SeatResting>,
}

/// Why a seat rests and until when (t-58193). `cause` is the word zo's door
/// rests it for (`RestCause::word`), which the panel words in every language
/// and never shows; `unanswered` is how many of its requests in a row went
/// unanswered, for a rest of its own — the door's own number, which the
/// panel's sentence counts with.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatResting {
    /// When the seat may ask again, in Unix milliseconds.
    pub until_ms: i64,
    pub cause: String,
    #[serde(default)]
    pub unanswered: Option<usize>,
}

/// One agent's line under a seat a pane's moments ask (t-10916): its
/// requests of the week and, by the core's words
/// ([`zerocode_core::hook_guard::Unseen::word`]), why the window cannot ask
/// the seat of its work (`unseen`) — or, when it can, what is said beside
/// the count (`notes`): what of the seat's reading its hooks miss, or that
/// its own runtime asks. A count of zero with no `unseen` is an agent nobody
/// ran; with one, an agent whose work the window cannot see.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatAsker {
    /// The agent's catalog slug, and the name a person calls it.
    pub agent: String,
    pub label: String,
    pub requests: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unseen: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// What a seat's graded answers say of its act line (t-9468), as the
/// dashboard reads it: the line they draw or why none, the line the product
/// reads now, and the answers at the bands' fixed line and at the drawn one.
/// zo's grid and the row the replay keeps stay in zo's answer — the screen
/// draws none of them.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatCalibration {
    #[serde(default)]
    pub reads_act_line: bool,
    /// The line the graded answers draw, per thousand — or why none
    /// (`reason`: `whole`, `one_colour`, `non_monotone`, `no_lift`,
    /// `no_confidence`, or the judge's own line word).
    #[serde(default)]
    pub act_from_permille: Option<u16>,
    #[serde(default)]
    pub reason: Option<String>,
    /// The line the product reads for the seat now — the table's row.
    #[serde(default)]
    pub table_line: Option<u16>,
    /// The answers at the line the seat's bands fix, and at the drawn one.
    #[serde(default)]
    pub fixed: Option<SeatLine>,
    #[serde(default)]
    pub drawn: Option<SeatLine>,
}

/// A seat's graded answers at one line: the line, how many marks it holds,
/// the share of answered requests it acts on, and how often the marks of
/// what it acts on, the baseline on those marks and the marks of what it
/// leaves alone say wrong, per thousand.
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatLine {
    pub from_permille: u16,
    #[serde(default)]
    pub marks: usize,
    #[serde(default)]
    pub apply_share: Option<f64>,
    #[serde(default)]
    pub error_permille: Option<u16>,
    #[serde(default)]
    pub baseline_error_permille: Option<u16>,
    #[serde(default)]
    pub under_error_permille: Option<u16>,
}

/// One local day of a seat's ledger, counted the way the week is.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatDay {
    pub start_ms: i64,
    pub tally: SeatWindow,
    #[serde(default)]
    pub agreement: SeatAgreement,
}

/// One request of a seat, as zo digested it (`zerocode_core::jev::recent`):
/// the row's own facts, never its body. Passed through as zo shaped it — the
/// dashboard draws whatever facts a seat's rows carry, by their names, and
/// this reader names none of them.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatDecision {
    pub at: i64,
    pub outcome: String,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    #[serde(default)]
    pub cached: bool,
    #[serde(default)]
    pub asked: Map<String, Value>,
    #[serde(default)]
    pub answered: Value,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub applied: Option<bool>,
    #[serde(default)]
    pub agreed: Option<bool>,
    #[serde(default)]
    pub followed: Option<String>,
}

/// One outcome token and how many rows carried it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatFailure {
    pub token: String,
    pub rows: usize,
}

/// The numbers a seat is promoted on: the last requests its floor can be
/// cleared on, and how often its judgment named what the probe named there.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatJudged {
    pub window: SeatWindow,
    #[serde(default)]
    pub window_wanted: usize,
    #[serde(default)]
    pub agreement: SeatAgreement,
}

/// Comparisons with the probe over the judged window.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatAgreement {
    pub compared: usize,
    pub agreed: usize,
    #[serde(default)]
    pub lower_bound: Option<f64>,
    /// Control rows the comparison borrowed — the routing seat's probe run
    /// once more beside a judgment already acted on; zero elsewhere.
    #[serde(default)]
    pub control_rows: usize,
    /// The seat's cheapest baseline over the same marks, and its share
    /// (t-6342); zeros and `None` from a zo older than the baseline.
    #[serde(default)]
    pub baseline_compared: usize,
    #[serde(default)]
    pub baseline_agreed: usize,
    #[serde(default)]
    pub baseline_share: Option<f64>,
    /// Label rows that compared nothing and said why.
    #[serde(default)]
    pub not_compared: usize,
    /// Why, word by word — the writers' own words with their counts
    /// (t-9556); empty from a zo older than the words.
    #[serde(default)]
    pub not_compared_by: std::collections::BTreeMap<String, usize>,
}

/// What the judge said of a seat's recent window.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatVerdict {
    pub verdict: String,
    #[serde(default)]
    pub line: Option<String>,
    /// The version the window's rows were cut away from at the last change
    /// of version, beside the line it holds on (t-6187).
    #[serde(default)]
    pub cut_model: Option<String>,
    /// The verdict before the evidence rules of t-26584, when it differs.
    #[serde(default)]
    pub before: Option<String>,
    /// The gate that holds the seat on this verdict (t-26584), when one does.
    #[serde(default)]
    pub basis: Option<String>,
}

/// One window of a seat's ledger, counted.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatWindow {
    pub rows: usize,
    pub answered: usize,
    /// Rows the door refused before anything was sent; absent from a zo that
    /// counted them as the seat's own failures.
    #[serde(default)]
    pub refused: usize,
    #[serde(default)]
    pub answered_share: Option<f64>,
    #[serde(default)]
    pub answered_lower_bound: Option<f64>,
    #[serde(default)]
    pub p50_ms: Option<u64>,
    #[serde(default)]
    pub p95_ms: Option<u64>,
    #[serde(default)]
    pub redacted_lines: u64,
    /// Rows whose judgment went over the wire.
    #[serde(default)]
    pub called: usize,
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub unmetered_requests: u64,
    /// Rows whose answer is what the product did.
    #[serde(default)]
    pub applied: usize,
    /// Every outcome that was not an answer, by token, most frequent first.
    #[serde(default)]
    pub failures: Vec<SeatFailure>,
    /// The door's refusals among them — no key, not consented, budget — the
    /// rows `refused` counts, one token at a time.
    #[serde(default)]
    pub refusals: Vec<SeatFailure>,
    /// What a screen seat's guards stopped (t-6187), counted by the core
    /// (`summary::Guards`); zeros from a zo older than the count.
    #[serde(default)]
    pub guards: SeatGuards,
    /// The controls a screen seat's rows named, and the ones it handed to the
    /// person (`summary::Controls`).
    #[serde(default)]
    pub controls: SeatControls,
}

/// Presses a screen seat's two guards stopped: the screen's own text told an
/// assistant what to do, or the screen was a wall in front of the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatGuards {
    #[serde(default)]
    pub instructed: usize,
    #[serde(default)]
    pub walled: usize,
}

/// The controls a screen seat's rows named, and the ones a press cannot take
/// back that it handed to the person instead of pressing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatControls {
    #[serde(default)]
    pub named: usize,
    #[serde(default)]
    pub destructive_held: usize,
}

/// What `zo jev summary --json` said, or `None` when it printed nothing this
/// reader understands — an older zo, or one that failed before it answered.
#[must_use]
pub fn read_summary(stdout: &[u8]) -> Option<Vec<SeatNumbers>> {
    let value: Value = serde_json::from_slice(stdout).ok()?;
    serde_json::from_value(value.get("seats")?.clone()).ok()
}
