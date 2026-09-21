//! Model discovery — what the connected providers say they serve today.
//!
//! The shipped catalog (`api`'s `model_context_windows.json`) is a photograph
//! of the lineup on the day this binary was built. Providers ship models
//! between builds, and every OAuth login can say what it may use today: the
//! ChatGPT backend lists the models a Codex login serves (the request Codex
//! makes to fill `models_cache.json`), Anthropic and Google answer a
//! `models` request, the Antigravity registry lists its tiers. This module
//! asks them, keeps one cache under the zo home, and turns the difference
//! into an override in the shipped catalog's own shape — new ids as identity
//! rows, and each family alias (`fable`, `opus`, `sol`, `gemini-flash`, …)
//! re-pointed at the newest release of its family — so nothing downstream
//! learns a second vocabulary.
//!
//! Three rules from t-3054 ("astra 모델이 갑자기 안 보임"), whose numbers sit
//! in the connection table below:
//!
//! - **Refresh on connect.** A session start and an opened `/model` picker
//!   ask again every source whose last answer is older than
//!   [`LIVE_TTL_SECS`], failed, or never came ([`due_sources`]). The OpenAI
//!   column is fetched live for the selected account and written back into
//!   that account's `models_cache.json` in Codex's own format — one file that
//!   Codex, the window's account sync and zo all read; when the live call
//!   fails, the freshest cache this machine has answers instead
//!   ([`freshest_codex_cache`]), and the report line says which and how old.
//! - **A failed source keeps its rows** ([`carry_forward`]) — never an empty
//!   family for one transport error.
//! - **A selected model never vanishes** ([`keep_selected`]): a row the
//!   source stops listing while a session has it selected stays, stamped
//!   `unlisted_since`, for the picker to show dimmed with the reason, and is
//!   dropped only after [`UNLISTED_GRACE_SECS`] of never coming back.
//!
//! Precedence stays what the catalog bridge already defines: an operator
//! export first, then settings, then what was discovered, then the shipped
//! rows. Discovery never edits a row the shipped catalog carries; it only adds
//! ids the binary predates and moves aliases forward. `modelUpdatePolicy` in
//! settings decides how far: `auto` (default) publishes both, `notify` adds the
//! ids but leaves aliases where they are and reports what it would have moved,
//! `pinned` publishes nothing at all.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// The connection table — every number of the "refresh on connect" rule.
// ---------------------------------------------------------------------------

/// How long one source's answer is trusted before a session start or an
/// opened `/model` picker asks it again (t-3054). Every live source shares
/// it: the ChatGPT backend, the Anthropic model list, the Antigravity
/// registry and the Google key list.
pub const LIVE_TTL_SECS: u64 = 60 * 60;
/// How long a selected row its source stopped listing stays — dimmed, with
/// the time it went unlisted — before it is dropped for never coming back.
pub const UNLISTED_GRACE_SECS: u64 = 7 * 24 * 60 * 60;
/// The Codex client version the live ChatGPT request names when no cache
/// says one. The backend gates the list on it, and Codex refetches a cache
/// whose `client_version` is not its own — so the cache's word wins.
pub const CODEX_CLIENT_VERSION_DEFAULT: &str = "0.153.4";
/// Where the ChatGPT backend lists the models a Codex login may use — the
/// very request Codex makes to fill `models_cache.json`. It is Codex's
/// `/backend-api/codex/models`, not the plain `/backend-api/models`: that one
/// is the ChatGPT web picker (twenty rows such as `gpt-5-6-instant` and the
/// `-wm` variants, no `visibility`), and written back as Codex's cache it
/// blanked every OpenAI row the window lists (measured 2026-09-08).
pub const CHATGPT_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";
/// Codex's cache file inside a Codex home.
pub const CODEX_MODELS_CACHE_FILE: &str = "models_cache.json";
/// How many selected models the process remembers for the keep-selected rule.
const SELECTED_MEMORY: usize = 8;

/// Settings key holding the update policy (`auto` | `notify` | `pinned`).
pub const SETTINGS_POLICY_KEY: &str = "modelUpdatePolicy";
/// Settings key holding the refresh interval in hours (default one — the
/// connection table's [`LIVE_TTL_SECS`]).
pub const SETTINGS_TTL_KEY: &str = "modelDiscoveryTtlHours";
/// How long a source's answer is trusted when settings say nothing.
pub const DEFAULT_TTL_SECS: u64 = LIVE_TTL_SECS;
/// Settings key holding the session's model — the persisted selection the
/// keep-selected rule protects beside what the running session chose.
const SETTINGS_MODEL_KEY: &str = "model";
const CACHE_DIR: &str = "model-catalog";
const CACHE_FILE: &str = "discovered.json";
const HTTP_TIMEOUT: Duration = Duration::from_secs(6);
const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";
const GOOGLE_MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models?pageSize=200";

/// The source keys, one per column of the report.
pub const OPENAI_SOURCE: &str = "chatgpt-backend";
/// What the OpenAI rows were called before the live source existed — a
/// previous cache still spells its rows this way and must carry forward.
const LEGACY_OPENAI_SOURCE: &str = "codex-cache";
pub const ANTHROPIC_SOURCE: &str = "anthropic-api";
pub const GOOGLE_API_SOURCE: &str = "google-api";
pub const ANTIGRAVITY_SOURCE: &str = "antigravity-registry";
/// Every source, in report order: `(provider, source)`.
pub const SOURCES: [(&str, &str); 4] = [
    ("openai", OPENAI_SOURCE),
    ("anthropic", ANTHROPIC_SOURCE),
    ("google", GOOGLE_API_SOURCE),
    ("google", ANTIGRAVITY_SOURCE),
];
/// The report's word for rows that came from the provider just now.
pub const ORIGIN_LIVE: &str = "live";
/// The report's word for rows a failed refresh kept from the previous one.
pub const ORIGIN_PREVIOUS: &str = "previous";

/// How far discovery may move the live catalog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpdatePolicy {
    /// New ids become selectable AND family aliases follow the newest release.
    #[default]
    Auto,
    /// New ids become selectable; aliases stay, and the moves are reported.
    Notify,
    /// Nothing discovered reaches the live catalog.
    Pinned,
}

impl UpdatePolicy {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "follow" => Some(Self::Auto),
            "notify" => Some(Self::Notify),
            "pinned" | "pin" | "off" => Some(Self::Pinned),
            _ => None,
        }
    }

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Notify => "notify",
            Self::Pinned => "pinned",
        }
    }

    /// The policy the user's settings declare, `Auto` when they say nothing.
    #[must_use]
    pub fn load() -> Self {
        Self::load_from(&crate::default_config_home().join("settings.json"))
    }

    #[must_use]
    pub fn load_from(path: &Path) -> Self {
        settings_object(path)
            .get(SETTINGS_POLICY_KEY)
            .and_then(Value::as_str)
            .and_then(Self::parse)
            .unwrap_or_default()
    }
}

/// The refresh interval the settings declare, in seconds.
#[must_use]
pub fn ttl_secs() -> u64 {
    ttl_secs_from(&crate::default_config_home().join("settings.json"))
}

#[must_use]
pub fn ttl_secs_from(path: &Path) -> u64 {
    settings_object(path)
        .get(SETTINGS_TTL_KEY)
        .and_then(Value::as_f64)
        .filter(|hours| hours.is_finite() && *hours >= 0.0)
        .map_or(DEFAULT_TTL_SECS, |hours| {
            // Non-negative and finite by the filter, and capped at a year, so
            // the cast can neither truncate nor lose a sign.
            let secs = (hours * 3600.0).min(365.0 * 86_400.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let secs = secs as u64;
            secs
        })
}

fn settings_object(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// One model a provider says it serves.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredModel {
    /// Catalog provider key: `anthropic` | `openai` | `google`.
    pub provider: String,
    pub id: String,
    pub display_name: String,
    /// `frontier` | `balanced` | `fast` when the source states a lineup position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// Every reasoning-effort level the source declares (`low`…`ultra`), in the
    /// catalog's spelling. Empty when the source says nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effort_levels: Vec<String>,
    /// Priority-serving tiers the source declares (Codex `additional_speed_tiers`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speed_tiers: Vec<String>,
    /// Release date as a `YYYYMMDD` ordinal, `0` when the source has none.
    #[serde(default)]
    pub released: u64,
    /// The provider's own ordering (lower first) — Codex `priority`, list position.
    #[serde(default)]
    pub prominence: u32,
    /// Where the row came from (`codex-cache`, `anthropic-api`, `google-api`).
    #[serde(default)]
    pub source: String,
    /// The row is reachable only over an API key (a discovered Gemini id is
    /// not in the Antigravity OAuth registry, which serves tiered ids).
    #[serde(default)]
    pub api_key_only: bool,
    /// The wire id(s) the provider serves for this selection id, in the
    /// catalog's `wire` shape (`"id"` or `{"low","medium","high"}`), when the
    /// selection id is not itself on the wire — Antigravity's tiered Gemini
    /// ids fold back into one selection id per release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire: Option<Value>,
    /// Unix seconds since the source stopped listing this row while a session
    /// had it selected. A field, not a removal: the picker shows the row
    /// dimmed with this time, and the wire still decides whether it answers.
    /// Cleared the moment the source lists it again; the row is dropped after
    /// [`UNLISTED_GRACE_SECS`] of never coming back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlisted_since: Option<u64>,
}

impl DiscoveredModel {
    /// The top of the declared effort scale, when the source declared one.
    #[must_use]
    pub fn effort_ceiling(&self) -> Option<String> {
        self.effort_levels
            .iter()
            .filter_map(|level| api::parse_effort_level(level))
            .max_by_key(|level| api::effort_rank(*level))
            .map(|level| level.key().to_string())
    }
}

/// What one source answered on the last refresh.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReport {
    pub provider: String,
    pub source: String,
    pub ok: bool,
    pub detail: String,
    pub count: usize,
    /// Unix seconds of the answer these rows came from — the source's own
    /// clock for the connection rule, not the catalog's. `0` when unknown.
    #[serde(default)]
    pub fetched_at: u64,
    /// Where the rows came from: [`ORIGIN_LIVE`], `cache:<codex home>`, or
    /// [`ORIGIN_PREVIOUS`] when a failed refresh kept last time's rows.
    #[serde(default)]
    pub origin: String,
}

impl SourceReport {
    /// Whether this source has to be asked again at a connection — never
    /// answered, failed last time, or answered longer than `ttl_secs` ago.
    ///
    /// A SKIP is an answer, not a failure: "no Google API key" is a fact
    /// about this machine, dated like any other answer, and asking it again
    /// on every connection re-stamps the cache for nothing. Left as "due",
    /// a keyless source made every catalog publish spawn a refresh that
    /// rewrote `discovered.json` (33 a second under a status line that
    /// resolves an alias per paint, 2026-09-08). A failure is asked again at
    /// the next connection, as before.
    #[must_use]
    pub fn due(&self, now_secs: u64, ttl_secs: u64) -> bool {
        if self.fetched_at == 0 || now_secs.saturating_sub(self.fetched_at) >= ttl_secs {
            return true;
        }
        !self.ok && !self.skipped()
    }

    /// A source that was deliberately not asked — no key, no login.
    #[must_use]
    pub fn skipped(&self) -> bool {
        self.detail.starts_with("skipped:")
    }
}

/// Everything the last refresh learned, as cached on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredCatalog {
    /// Unix seconds of the refresh that produced this.
    pub fetched_at: u64,
    #[serde(default)]
    pub reports: Vec<SourceReport>,
    #[serde(default)]
    pub models: Vec<DiscoveredModel>,
}

impl DiscoveredCatalog {
    /// Older than `ttl_secs`, or never fetched.
    #[must_use]
    pub fn stale(&self, now_secs: u64, ttl_secs: u64) -> bool {
        self.fetched_at == 0 || now_secs.saturating_sub(self.fetched_at) >= ttl_secs
    }
}

/// A family alias moved (or, under `notify`, a move that was withheld).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AliasUpdate {
    pub provider: String,
    pub alias: String,
    pub from: String,
    pub to: String,
}

/// The discovered difference, in the shipped catalog's shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overlay {
    /// `{"models": […], "aliases": […]}` for the catalog bridge, or `None`
    /// when there is nothing to layer.
    pub json: Option<String>,
    /// Ids the shipped catalog does not carry, newest first per family.
    pub new_models: Vec<DiscoveredModel>,
    /// Aliases the overlay re-points (policy `auto`).
    pub alias_updates: Vec<AliasUpdate>,
    /// Moves withheld by policy `notify`, for the person to act on.
    pub alias_candidates: Vec<AliasUpdate>,
}

#[must_use]
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// `<zo home>/cache/model-catalog/` — the discovery cache and, beside it,
/// the merged copy `zo models --refresh` leaves for audit.
#[must_use]
pub fn cache_dir() -> PathBuf {
    crate::default_config_home().join("cache").join(CACHE_DIR)
}

/// `<zo home>/cache/model-catalog/discovered.json`.
#[must_use]
pub fn cache_path() -> PathBuf {
    cache_dir().join(CACHE_FILE)
}

#[must_use]
pub fn load_cached() -> Option<DiscoveredCatalog> {
    load_cached_from(&cache_path())
}

#[must_use]
pub fn load_cached_from(path: &Path) -> Option<DiscoveredCatalog> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save(catalog: &DiscoveredCatalog) -> io::Result<()> {
    save_to(&cache_path(), catalog)
}

pub fn save_to(path: &Path, catalog: &DiscoveredCatalog) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_vec_pretty(catalog).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, payload)?;
    std::fs::rename(&tmp, path)
}

static CURRENT: RwLock<Option<Arc<DiscoveredCatalog>>> = RwLock::new(None);
static LOADED: Once = Once::new();

/// The process's snapshot of the last refresh, read from disk once.
#[must_use]
pub fn current() -> Option<Arc<DiscoveredCatalog>> {
    LOADED.call_once(|| {
        if let Some(catalog) = load_cached() {
            *CURRENT.write().unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(Arc::new(catalog));
        }
    });
    CURRENT
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Make a fresh refresh the process snapshot and write it to the cache.
pub fn install(catalog: DiscoveredCatalog) -> io::Result<()> {
    save(&catalog)?;
    LOADED.call_once(|| {});
    *CURRENT.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(catalog));
    Ok(())
}

/// What one source answered: its rows, where they came from and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answered {
    pub source: String,
    pub models: Vec<DiscoveredModel>,
    /// [`ORIGIN_LIVE`] or `cache:<codex home>`.
    pub origin: String,
    /// Unix seconds the rows date from: now for a live answer, the cache's
    /// own `fetched_at` for a cached one.
    pub fetched_at: u64,
    /// A caveat worth a report line beside a success — the live call that
    /// failed before the cache answered, a cache that could not be written.
    pub note: Option<String>,
}

/// What one source answers: its rows, or its name and why not.
pub type SourceAnswer = Result<Answered, (String, String)>;

/// Ask every source, blocking. Meant for a background thread or `zo models
/// --refresh`; boot reads the cache instead.
#[must_use]
pub fn discover() -> DiscoveredCatalog {
    let previous = load_cached();
    let every: Vec<&'static str> = SOURCES.iter().map(|(_, source)| *source).collect();
    refresh_with(previous.as_ref(), &every, now_secs(), &selected_models(), ask)
}

/// Ask only the sources whose last answer is older than `ttl_secs` (or
/// failed), and carry the others as they are — the connection rule, at a
/// session start and when the `/model` picker opens.
#[must_use]
pub fn discover_due(now_secs: u64, ttl_secs: u64) -> DiscoveredCatalog {
    let previous = load_cached();
    let due = due_sources(previous.as_ref(), now_secs, ttl_secs);
    refresh_with(previous.as_ref(), &due, now_secs, &selected_models(), ask)
}

/// The sources a connection at `now_secs` has to ask again: every source the
/// previous catalog has no successful answer for that is younger than
/// `ttl_secs`. Empty when nothing is due — the cheap case, the common one.
#[must_use]
pub fn due_sources(previous: Option<&DiscoveredCatalog>, now_secs: u64, ttl_secs: u64) -> Vec<&'static str> {
    SOURCES
        .iter()
        .map(|(_, source)| *source)
        .filter(|source| {
            previous
                .and_then(|catalog| catalog.reports.iter().find(|report| report.source == *source))
                .is_none_or(|report| report.due(now_secs, ttl_secs))
        })
        .collect()
}

/// One refresh: the sources in `due` are asked through `fetch`, every other
/// source keeps the rows and report the previous catalog carried, a failed
/// source keeps last time's rows ([`carry_forward`]), and a selected model
/// the sources stopped listing stays as an unlisted row ([`keep_selected`]).
pub fn refresh_with(
    previous: Option<&DiscoveredCatalog>,
    due: &[&str],
    now_secs: u64,
    selected: &[String],
    fetch: impl Fn(&'static str) -> SourceAnswer,
) -> DiscoveredCatalog {
    let mut catalog = DiscoveredCatalog {
        fetched_at: now_secs,
        ..Default::default()
    };
    for (provider, source) in SOURCES {
        if !due.contains(&source) {
            let carried = previous.and_then(|catalog| {
                catalog
                    .reports
                    .iter()
                    .find(|report| report.source == source)
                    .cloned()
            });
            if let Some(report) = carried {
                catalog.reports.push(report);
                if let Some(previous) = previous {
                    catalog.models.extend(
                        previous
                            .models
                            .iter()
                            .filter(|model| model.source == source)
                            .cloned(),
                    );
                }
                continue;
            }
        }
        match fetch(source) {
            Ok(answered) => {
                let count = answered.models.len();
                let detail = match answered.note {
                    Some(note) => format!("{count} model(s) · {note}"),
                    None => format!("{count} model(s)"),
                };
                catalog.reports.push(SourceReport {
                    provider: provider.to_string(),
                    source: answered.source,
                    ok: true,
                    detail,
                    count,
                    fetched_at: answered.fetched_at,
                    origin: answered.origin,
                });
                catalog.models.extend(answered.models);
            }
            Err((source, detail)) => catalog.reports.push(SourceReport {
                provider: provider.to_string(),
                source,
                ok: false,
                detail,
                count: 0,
                fetched_at: now_secs,
                origin: String::new(),
            }),
        }
    }
    let catalog = carry_forward(catalog, previous);
    keep_selected(catalog, previous, selected, now_secs)
}

/// The real fetchers, by source key.
fn ask(source: &'static str) -> SourceAnswer {
    match source {
        OPENAI_SOURCE => openai_models(),
        ANTHROPIC_SOURCE => anthropic_models(),
        GOOGLE_API_SOURCE => google_models(),
        ANTIGRAVITY_SOURCE => antigravity_models(),
        other => Err((other.to_string(), "skipped: unknown source".to_string())),
    }
}

/// A source that FAILED this refresh keeps the rows it answered last time.
///
/// One transport error must not empty a provider's column: on 2026-09-04 a
/// single failed registry request made `gemini-3.8-flash` and its alias
/// vanish for the whole TTL, and the person read that as "the Gemini models
/// disappeared". A source that is deliberately `skipped:` (no key, no login)
/// keeps nothing — the rows it once served are no longer reachable — and a
/// source that answered replaces its rows as before. The report stays
/// `ok: false`, says how many rows it carried and dates them from the answer
/// they came from, so `zo models` still shows the failure instead of a quiet
/// success.
#[must_use]
pub fn carry_forward(
    mut fresh: DiscoveredCatalog,
    previous: Option<&DiscoveredCatalog>,
) -> DiscoveredCatalog {
    if let Some(previous) = previous {
        for report in &mut fresh.reports {
            if report.ok || report.detail.starts_with("skipped:") {
                continue;
            }
            let kept: Vec<DiscoveredModel> = previous
                .models
                .iter()
                .filter(|model| same_source(&model.source, &report.source))
                .cloned()
                .collect();
            if kept.is_empty() {
                continue;
            }
            report.count = kept.len();
            report.detail = format!(
                "{} · kept {} model(s) from the previous refresh",
                report.detail,
                kept.len()
            );
            if let Some(last) = previous.reports.iter().find(|last| last.source == report.source) {
                report.fetched_at = last.fetched_at;
            }
            report.origin = ORIGIN_PREVIOUS.to_string();
            fresh.models.extend(kept);
        }
    }
    drop_api_key_duplicates(&mut fresh.models);
    fresh
}

/// A row's source key names the report it belongs to — under the name it
/// was written with, or the OpenAI rows' name from before the live source.
fn same_source(row_source: &str, report_source: &str) -> bool {
    row_source == report_source || (report_source == OPENAI_SOURCE && row_source == LEGACY_OPENAI_SOURCE)
}

/// A selected model never vanishes because a list omitted it.
///
/// Every row the previous catalog carried for a `selected` id (the session's
/// model, the persisted one — matched on the id alone, case-insensitively)
/// that `fresh` no longer lists stays, stamped `unlisted_since` with the
/// first refresh that missed it; a row already stamped keeps its stamp and
/// stays whether or not it is still selected. A stamped row the source lists
/// again arrives unstamped, as any fresh row does. A stamped row nobody has
/// listed for [`UNLISTED_GRACE_SECS`] is dropped. An id that was never
/// selected is dropped the moment its source stops listing it, as before.
#[must_use]
pub fn keep_selected(
    mut fresh: DiscoveredCatalog,
    previous: Option<&DiscoveredCatalog>,
    selected: &[String],
    now_secs: u64,
) -> DiscoveredCatalog {
    if let Some(previous) = previous {
        let listed: HashSet<(String, String)> = fresh
            .models
            .iter()
            .map(|model| (model.provider.clone(), model.id.to_ascii_lowercase()))
            .collect();
        for model in &previous.models {
            let key = (model.provider.clone(), model.id.to_ascii_lowercase());
            if listed.contains(&key) {
                continue;
            }
            let wanted = model.unlisted_since.is_some()
                || selected.iter().any(|id| id.eq_ignore_ascii_case(&model.id));
            if !wanted {
                continue;
            }
            let mut kept = model.clone();
            kept.unlisted_since = Some(model.unlisted_since.unwrap_or(now_secs));
            fresh.models.push(kept);
        }
    }
    fresh.models.retain(|model| {
        model
            .unlisted_since
            .is_none_or(|since| now_secs.saturating_sub(since) < UNLISTED_GRACE_SECS)
    });
    fresh
}

static SELECTED: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Remember a model a session selected (its launch model, a `/model` choice),
/// so the next refresh keeps its row even when the source omits it.
pub fn note_selected(model: &str) {
    let model = model.trim();
    if model.is_empty() {
        return;
    }
    let mut selected = SELECTED.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    selected.retain(|known| !known.eq_ignore_ascii_case(model));
    selected.insert(0, model.to_string());
    selected.truncate(SELECTED_MEMORY);
}

/// The ids the keep-selected rule protects: what this process's sessions
/// selected, and the model settings persist — each as spelled and as its
/// alias resolves today.
#[must_use]
pub fn selected_models() -> Vec<String> {
    let mut ids: Vec<String> = SELECTED
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(persisted) = settings_object(&crate::default_config_home().join("settings.json"))
        .get(SETTINGS_MODEL_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        ids.push(persisted.to_string());
    }
    let resolved: Vec<String> = ids.iter().map(|id| api::resolve_catalog_alias(id)).collect();
    ids.extend(resolved);
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(id.to_ascii_lowercase()));
    ids
}

/// When the source stopped listing `id`, if the current catalog carries it as
/// an unlisted row — what the picker dims.
#[must_use]
pub fn unlisted_since(id: &str) -> Option<u64> {
    current()?
        .models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(id))
        .and_then(|model| model.unlisted_since)
}

/// One row per id: when the OAuth registry and the API-key list both name a
/// release, the registry row stays — it is the one the login can reach, and
/// it carries the wire map the API-key list has no notion of.
fn drop_api_key_duplicates(models: &mut Vec<DiscoveredModel>) {
    let reachable: HashSet<(String, String)> = models
        .iter()
        .filter(|model| !model.api_key_only)
        .map(|model| (model.provider.clone(), model.id.to_ascii_lowercase()))
        .collect();
    models.retain(|model| {
        !model.api_key_only
            || !reachable.contains(&(model.provider.clone(), model.id.to_ascii_lowercase()))
    });
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The Codex home the live answer is written back to and the token is read
/// from: whatever `api::managed_account::resolve_codex_home` names — the
/// account the channel switched to, the launch `CODEX_HOME`, or the window's
/// own managed home — then `$ZO_CODEX_HOME`, then `~/.codex`.
///
/// `$CODEX_HOME` is not read again here; the resolution table above already
/// holds that row, and asking twice is how two answers start to differ.
#[must_use]
pub fn codex_home() -> Option<PathBuf> {
    api::managed_account::codex_home()
        .map(PathBuf::from)
        .or_else(|| non_empty_env("ZO_CODEX_HOME").map(PathBuf::from))
        .or_else(|| non_empty_env("HOME").map(|home| PathBuf::from(home).join(".codex")))
}

/// Every Codex home this machine may hold a `models_cache.json` under, the
/// selected account's first: [`codex_home`], `~/.codex`, and the window's
/// shared runtime home (`api::managed_account::ide_codex_home`, which the
/// window syncs the selected account's cache into). De-duplicated, in that
/// order.
#[must_use]
pub fn codex_cache_paths() -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = Vec::new();
    if let Some(home) = codex_home() {
        homes.push(home);
    }
    if let Some(home) = non_empty_env("HOME").map(PathBuf::from) {
        homes.push(home.join(".codex"));
    }
    if let Some(home) = api::managed_account::ide_codex_home() {
        homes.push(home);
    }
    let mut seen = HashSet::new();
    homes
        .into_iter()
        .filter(|home| seen.insert(home.clone()))
        .map(|home| home.join(CODEX_MODELS_CACHE_FILE))
        .collect()
}

/// One Codex `models_cache.json`, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexCache {
    pub path: PathBuf,
    /// Unix seconds parsed from the file's RFC 3339 `fetched_at`; `0` when
    /// the stamp is missing or unreadable, which sorts it last.
    pub fetched_at: u64,
    pub etag: Option<String>,
    pub client_version: Option<String>,
    /// The `{"models": […]}` document [`codex_cache_rows`] reads.
    pub document: Value,
}

/// Read one Codex cache file. `Err` says why not — a missing file, bad JSON,
/// no `models` array.
pub fn read_codex_cache(path: &Path) -> Result<CodexCache, String> {
    let raw = std::fs::read_to_string(path).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => "not found".to_string(),
        _ => error.to_string(),
    })?;
    let document: Value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    if !document.get("models").is_some_and(Value::is_array) {
        return Err("no models array".to_string());
    }
    let text = |key: &str| {
        document
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let fetched_at = text("fetched_at")
        .and_then(|stamp| core_types::date::unix_secs_from_rfc3339(&stamp))
        .and_then(|secs| u64::try_from(secs).ok())
        .unwrap_or(0);
    Ok(CodexCache {
        path: path.to_path_buf(),
        fetched_at,
        etag: text("etag"),
        client_version: text("client_version"),
        document,
    })
}

/// The newest readable cache among `paths` by its own `fetched_at` — the
/// fallback when the live call fails. Ties keep the earlier path (the
/// selected account's home comes first). `None` when none can be read.
#[must_use]
pub fn freshest_codex_cache(paths: &[PathBuf]) -> Option<CodexCache> {
    paths
        .iter()
        .filter_map(|path| read_codex_cache(path).ok())
        .fold(None, |best: Option<CodexCache>, cache| match best {
            Some(best) if best.fetched_at >= cache.fetched_at => Some(best),
            _ => Some(cache),
        })
}

/// The Codex home a cache path sits in, for the report's `cache:<home>`.
fn cache_origin(path: &Path) -> String {
    format!("cache:{}", path.parent().unwrap_or(path).display())
}

/// The OpenAI column: the ChatGPT backend live, else the freshest Codex
/// cache this machine has — with the live failure on the report line.
fn openai_models() -> SourceAnswer {
    match chatgpt_backend_models() {
        Ok(answer) => Ok(answer),
        Err((source, live)) => match codex_cache_models() {
            Ok(mut cached) => {
                cached.note = Some(format!("live: {live}"));
                Ok(cached)
            }
            Err((_, cache)) => {
                let detail = format!("live: {live} · cache: {cache}");
                let detail = if live.starts_with("skipped:") && cache.starts_with("skipped:") {
                    format!("skipped: {detail}")
                } else {
                    detail
                };
                Err((source, detail))
            }
        },
    }
}

/// The freshest Codex cache on this machine, as rows.
fn codex_cache_models() -> SourceAnswer {
    codex_cache_models_from(&codex_cache_paths())
}

/// `codex_cache_models` over explicit cache paths.
pub fn codex_cache_models_from(paths: &[PathBuf]) -> SourceAnswer {
    let source = OPENAI_SOURCE.to_string();
    if paths.is_empty() {
        return Err((source, "skipped: no Codex home".to_string()));
    }
    let Some(cache) = freshest_codex_cache(paths) else {
        let homes: Vec<String> = paths
            .iter()
            .map(|path| path.parent().unwrap_or(path).display().to_string())
            .collect();
        return Err((source, format!("skipped: no {CODEX_MODELS_CACHE_FILE} under {}", homes.join(", "))));
    };
    Ok(Answered {
        models: codex_cache_rows(&cache.document, &source),
        source,
        origin: cache_origin(&cache.path),
        fetched_at: cache.fetched_at,
        note: None,
    })
}

/// What the live ChatGPT request answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveModels {
    /// The `{"models": […]}` document — the cache's own shape.
    pub document: Value,
    pub etag: Option<String>,
    pub client_version: String,
    /// The backend said `304 Not Modified` to the cache's etag: `document`
    /// is the cache's, re-dated.
    pub not_modified: bool,
}

/// The live OpenAI source: the ChatGPT backend's model list for the selected
/// account, exactly as Codex asks for it, written back into that account's
/// `models_cache.json` so Codex, the window's sync and zo read one file.
fn chatgpt_backend_models() -> SourceAnswer {
    let source = OPENAI_SOURCE.to_string();
    let Some(tokens) = api::resolve_openai_oauth_fresh() else {
        return Err((source, "skipped: no ChatGPT login".to_string()));
    };
    let Some(home) = codex_home() else {
        return Err((source, "skipped: no Codex home".to_string()));
    };
    let path = home.join(CODEX_MODELS_CACHE_FILE);
    let cache = read_codex_cache(&path).ok();
    chatgpt_backend_models_at(CHATGPT_MODELS_URL, &tokens, &path, cache.as_ref(), now_secs())
}

/// `chatgpt_backend_models` against an explicit endpoint and cache file.
/// The answer is written back only when `cache_path`'s home is this login's
/// (or nobody's): a bare-terminal zo with its own ChatGPT login must not
/// overwrite the person's `~/.codex` cache with another account's list.
pub fn chatgpt_backend_models_at(
    url: &str,
    tokens: &core_types::OpenAiOAuthTokens,
    cache_path: &Path,
    cache: Option<&CodexCache>,
    now_secs: u64,
) -> SourceAnswer {
    let source = OPENAI_SOURCE.to_string();
    let answer = fetch_chatgpt_models(url, tokens, cache).map_err(|detail| (source.clone(), detail))?;
    let mut note = answer.not_modified.then(|| "304, the cache is current".to_string());
    let written = cache_write_allowed(cache_path, tokens)
        .and_then(|()| write_codex_cache(cache_path, &answer, SystemTime::now()).map_err(|error| error.to_string()));
    if let Err(error) = written {
        let refused = format!("{} not written: {error}", cache_path.display());
        note = Some(note.map_or(refused.clone(), |note| format!("{note} · {refused}")));
    }
    Ok(Answered {
        models: codex_cache_rows(&answer.document, &source),
        source,
        origin: ORIGIN_LIVE.to_string(),
        fetched_at: now_secs,
        note,
    })
}

/// `GET <url>?client_version=<v>` with the login's bearer, its
/// `chatgpt-account-id`, Codex's client fingerprint and — when `cache` has
/// one — `If-None-Match: <etag>`. `304` keeps the cache's document; any
/// other non-success status, a transport failure or a body without a
/// `models` array is the `Err` the caller falls back on. Never logs the
/// token.
pub fn fetch_chatgpt_models(
    url: &str,
    tokens: &core_types::OpenAiOAuthTokens,
    cache: Option<&CodexCache>,
) -> Result<LiveModels, String> {
    let client_version = cache
        .and_then(|cache| cache.client_version.clone())
        .unwrap_or_else(|| CODEX_CLIENT_VERSION_DEFAULT.to_string());
    let etag = cache.and_then(|cache| cache.etag.clone());
    let request_url = format!("{url}?client_version={client_version}");
    let access_token = tokens.access_token.clone();
    let account_id = tokens.account_id.clone();
    let etag_sent = etag.clone();
    let (status, body, answered_etag) = api::sync_bridge::run_blocking(async move {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client
            .get(&request_url)
            .header("accept", "application/json")
            .header("originator", api::CHATGPT_ORIGINATOR)
            .header("user-agent", api::CHATGPT_USER_AGENT)
            .bearer_auth(access_token);
        if let Some(account_id) = account_id {
            request = request.header("chatgpt-account-id", account_id);
        }
        if let Some(etag) = etag_sent {
            request = request.header("if-none-match", etag);
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let status = response.status();
        let answered_etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response.text().await.map_err(|error| error.to_string())?;
        Ok::<_, String>((status, body, answered_etag))
    })?;
    if status == reqwest::StatusCode::NOT_MODIFIED {
        let cache = cache.ok_or_else(|| "HTTP 304 without a cache to keep".to_string())?;
        return Ok(LiveModels {
            document: json!({ "models": cache.document.get("models").cloned().unwrap_or_else(|| json!([])) }),
            etag: cache.etag.clone().or(answered_etag),
            client_version,
            not_modified: true,
        });
    }
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", body.chars().take(160).collect::<String>()));
    }
    let parsed: Value = serde_json::from_str(&body).map_err(|error| error.to_string())?;
    let Some(models) = parsed.get("models").filter(|models| models.is_array()).cloned() else {
        return Err("no models array in the answer".to_string());
    };
    if !is_codex_model_list(&models) {
        return Err(
            "the answer is not Codex's model list (rows without slug and visibility — \
             the ChatGPT web picker answers like that); nothing written"
                .to_string(),
        );
    }
    Ok(LiveModels {
        document: json!({ "models": models }),
        etag: answered_etag,
        client_version,
        not_modified: false,
    })
}

/// Whether the login may write its model list into the Codex home holding
/// `cache_path`: yes when that home's `auth.json` is absent or names no
/// account, or names this login's account; no when it is someone else's —
/// the same refusal `codex_auth::save_at` makes for the tokens themselves.
/// Codex's list is rows that each name a `slug` and a `visibility`; anything
/// else (the web picker's rows, an empty array) is not a cache to write.
fn is_codex_model_list(models: &Value) -> bool {
    models.as_array().is_some_and(|rows| {
        !rows.is_empty()
            && rows.iter().all(|row| {
                row.get("slug").and_then(Value::as_str).is_some()
                    && row.get("visibility").and_then(Value::as_str).is_some()
            })
    })
}

fn cache_write_allowed(cache_path: &Path, tokens: &core_types::OpenAiOAuthTokens) -> Result<(), String> {
    let Some(home) = cache_path.parent() else {
        return Ok(());
    };
    let Ok(raw) = std::fs::read(home.join("auth.json")) else {
        return Ok(());
    };
    let held = serde_json::from_slice::<Value>(&raw)
        .ok()
        .and_then(|root| root.get("tokens")?.get("account_id")?.as_str().map(str::to_string));
    match (held, tokens.account_id.as_deref()) {
        (Some(held), Some(mine)) if held != mine => {
            Err("another account's Codex home".to_string())
        }
        _ => Ok(()),
    }
}

/// Codex's own file shape, field for field and in its order.
#[derive(Serialize)]
struct CodexCacheFile<'a> {
    fetched_at: String,
    etag: Option<&'a str>,
    client_version: &'a str,
    models: &'a Value,
}

/// Write a live answer into `path` in Codex's format — `fetched_at` as RFC
/// 3339 with microseconds, `etag`, `client_version`, `models` — through a
/// temp file and a rename, so a reader never sees half a cache.
pub fn write_codex_cache(path: &Path, answer: &LiveModels, now: SystemTime) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = CodexCacheFile {
        fetched_at: codex_stamp(now),
        etag: answer.etag.as_deref(),
        client_version: &answer.client_version,
        models: answer.document.get("models").unwrap_or(&Value::Null),
    };
    let payload = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, payload)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `2026-09-07T08:25:08.756120Z` — the stamp Codex writes.
#[must_use]
pub fn codex_stamp(now: SystemTime) -> String {
    let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let base = api::oauth_store::codex_auth::rfc3339_utc(since.as_secs());
    format!(
        "{}.{:06}Z",
        base.strip_suffix('Z').unwrap_or(&base),
        since.subsec_micros()
    )
}

/// Rows from a Codex `models_cache.json` document.
#[must_use]
pub fn codex_cache_rows(document: &Value, source: &str) -> Vec<DiscoveredModel> {
    let Some(models) = document.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for (index, model) in models.iter().enumerate() {
        let Some(slug) = model.get("slug").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if slug.is_empty() {
            continue;
        }
        if model
            .get("visibility")
            .and_then(Value::as_str)
            .is_some_and(|visibility| !visibility.eq_ignore_ascii_case("list"))
        {
            continue;
        }
        let description = model
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let class = if description.contains("frontier") {
            Some("frontier")
        } else if description.contains("balanced") {
            Some("balanced")
        } else if description.contains("fast") || description.contains("affordable") {
            Some("fast")
        } else {
            None
        };
        let percent = model
            .get("effective_context_window_percent")
            .and_then(Value::as_u64)
            .filter(|percent| (1..=100).contains(percent))
            .unwrap_or(100);
        let context_window = model
            .get("context_window")
            .and_then(Value::as_u64)
            .map(|window| window.saturating_mul(percent) / 100)
            .filter(|window| *window > 0);
        let effort_levels = model
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .map(|levels| {
                levels
                    .iter()
                    .filter_map(|level| level.get("effort").and_then(Value::as_str))
                    .filter_map(api::parse_effort_level)
                    .map(|level| level.key().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let speed_tiers = model
            .get("additional_speed_tiers")
            .and_then(Value::as_array)
            .map(|tiers| {
                tiers
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|tier| tier.trim().to_ascii_lowercase())
                    .filter(|tier| !tier.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let prominence = model
            .get("priority")
            .and_then(Value::as_u64)
            .and_then(|priority| u32::try_from(priority).ok())
            .unwrap_or_else(|| u32::try_from(index).unwrap_or(u32::MAX));
        rows.push(DiscoveredModel {
            provider: "openai".to_string(),
            id: slug.to_string(),
            display_name: model
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(slug)
                .to_string(),
            class: class.map(str::to_string),
            context_window,
            max_output_tokens: model.get("max_output_tokens").and_then(Value::as_u64),
            effort_levels,
            speed_tiers,
            released: 0,
            prominence,
            source: source.to_string(),
            api_key_only: false,
            wire: None,
            unlisted_since: None,
        });
    }
    rows
}

/// The maker word the Anthropic API puts in front of every display name
/// (`Claude Opus 5`). The catalog shows the lineup name alone and the prompt
/// label adds the maker once, so it comes off here.
const ANTHROPIC_DISPLAY_PREFIX: &str = "Claude ";

fn anthropic_models() -> SourceAnswer {
    let source = ANTHROPIC_SOURCE.to_string();
    let Some(auth) = api::resolve_claude_auth_fresh() else {
        return Err((source, "skipped: no Anthropic credential".to_string()));
    };
    let base = non_empty_env("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|| ANTHROPIC_DEFAULT_BASE_URL.to_string());
    let url = format!("{}/v1/models?limit=100", base.trim_end_matches('/'));
    let bearer = auth.bearer_token().is_some();
    let body = api::sync_bridge::run_blocking(async move {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client
            .get(&url)
            .header("anthropic-version", api::DEFAULT_ANTHROPIC_VERSION)
            .header("accept", "application/json");
        if bearer {
            request = request.header("anthropic-beta", ANTHROPIC_OAUTH_BETA);
        }
        let response = auth
            .apply(request)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let text = response.text().await.map_err(|error| error.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {status}: {}", text.chars().take(160).collect::<String>()));
        }
        Ok(text)
    })
    .map_err(|detail| (source.clone(), detail))?;
    let parsed: Value = serde_json::from_str(&body).map_err(|error| (source.clone(), error.to_string()))?;
    Ok(live_answer(source, |source| anthropic_rows(&parsed, source)))
}

/// A source that answered just now.
fn live_answer(source: String, rows: impl FnOnce(&str) -> Vec<DiscoveredModel>) -> Answered {
    Answered {
        models: rows(&source),
        source,
        origin: ORIGIN_LIVE.to_string(),
        fetched_at: now_secs(),
        note: None,
    }
}

/// Rows from an Anthropic `GET /v1/models` document.
#[must_use]
pub fn anthropic_rows(document: &Value, source: &str) -> Vec<DiscoveredModel> {
    let Some(data) = document.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for (index, model) in data.iter().enumerate() {
        let Some(id) = model.get("id").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        rows.push(DiscoveredModel {
            provider: "anthropic".to_string(),
            id: id.to_string(),
            display_name: model
                .get("display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map_or_else(
                    || api::display_name_from_id(api::ProviderKind::Anthropic, id),
                    |name| name.strip_prefix(ANTHROPIC_DISPLAY_PREFIX).unwrap_or(name).to_string(),
                ),
            class: None,
            context_window: None,
            max_output_tokens: None,
            effort_levels: Vec::new(),
            speed_tiers: Vec::new(),
            released: model
                .get("created_at")
                .and_then(Value::as_str)
                .map_or(0, date_ordinal),
            prominence: u32::try_from(index).unwrap_or(u32::MAX),
            source: source.to_string(),
            api_key_only: false,
            wire: None,
            unlisted_since: None,
        });
    }
    rows
}

/// `2026-08-31T00:00:00Z` → `20260831`; anything else → `0`.
fn date_ordinal(stamp: &str) -> u64 {
    let digits: String = stamp
        .chars()
        .take(10)
        .filter(char::is_ascii_digit)
        .collect();
    if digits.len() == 8 {
        digits.parse().unwrap_or(0)
    } else {
        0
    }
}

fn google_api_key() -> Option<String> {
    non_empty_env("GOOGLE_API_KEY")
        .or_else(|| non_empty_env("GEMINI_API_KEY"))
        .or_else(|| {
            api::oauth_store::load_openai_compat_api_key("GOOGLE_API_KEY")
                .ok()
                .flatten()
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
        })
}

fn google_models() -> SourceAnswer {
    let source = GOOGLE_API_SOURCE.to_string();
    let Some(key) = google_api_key() else {
        return Err((source, "skipped: no Google API key (OAuth registries serve tiered ids)".to_string()));
    };
    let body = api::sync_bridge::run_blocking(async move {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        let response = client
            .get(GOOGLE_MODELS_URL)
            .header("x-goog-api-key", key)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let text = response.text().await.map_err(|error| error.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {status}: {}", text.chars().take(160).collect::<String>()));
        }
        Ok(text)
    })
    .map_err(|detail| (source.clone(), detail))?;
    let parsed: Value = serde_json::from_str(&body).map_err(|error| (source.clone(), error.to_string()))?;
    Ok(live_answer(source, |source| google_rows(&parsed, source)))
}

const GOOGLE_EXCLUDED_TOKENS: &[&str] = &[
    "embedding",
    "aqa",
    "imagen",
    "veo",
    "tts",
    "audio",
    "image",
    "live",
    "robotics",
    "computer-use",
    "exp",
];

/// Rows from a Google `GET /v1beta/models` document: text generation Gemini
/// models only.
#[must_use]
pub fn google_rows(document: &Value, source: &str) -> Vec<DiscoveredModel> {
    let Some(models) = document.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for (index, model) in models.iter().enumerate() {
        let Some(name) = model.get("name").and_then(Value::as_str) else {
            continue;
        };
        let id = name.trim().trim_start_matches("models/").to_string();
        let lower = id.to_ascii_lowercase();
        if !lower.starts_with("gemini-") {
            continue;
        }
        if GOOGLE_EXCLUDED_TOKENS.iter().any(|token| lower.contains(token)) {
            continue;
        }
        let generates = model
            .get("supportedGenerationMethods")
            .and_then(Value::as_array)
            .is_some_and(|methods| {
                methods
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|method| method == "generateContent")
            });
        if !generates {
            continue;
        }
        rows.push(DiscoveredModel {
            provider: "google".to_string(),
            id: id.clone(),
            display_name: model
                .get("displayName")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(&id)
                .to_string(),
            class: None,
            context_window: model.get("inputTokenLimit").and_then(Value::as_u64),
            max_output_tokens: model.get("outputTokenLimit").and_then(Value::as_u64),
            effort_levels: Vec::new(),
            speed_tiers: Vec::new(),
            released: 0,
            prominence: u32::try_from(index).unwrap_or(u32::MAX),
            source: source.to_string(),
            api_key_only: true,
            wire: None,
            unlisted_since: None,
        });
    }
    rows
}

fn antigravity_models() -> SourceAnswer {
    let source = ANTIGRAVITY_SOURCE.to_string();
    if !api::google_code_assist_oauth_present() {
        return Err((source, "skipped: no Google (Antigravity) login".to_string()));
    }
    let Some(tokens) = api::google_code_assist_fresh_oauth() else {
        return Err((source, "skipped: the Google login could not be read".to_string()));
    };
    let document = api::sync_bridge::run_blocking(async move {
        api::GeminiCodeAssistClient::new(tokens.access_token)
            .fetch_available_models()
            .await
            .map_err(|error| error.to_string())
    })
    .map_err(|detail| (source.clone(), detail))?;
    Ok(live_answer(source, |source| antigravity_rows(&document, source)))
}

/// The reasoning-tier suffixes the Antigravity registry folds into a model id,
/// longest first so `-extra-low` is not read as `-low`.
const ANTIGRAVITY_TIERS: [(&str, &str); 5] = [
    ("-extra-low", "extra-low"),
    ("-tiered", "tiered"),
    ("-medium", "medium"),
    ("-high", "high"),
    ("-low", "low"),
];

/// `gemini-3.7-flash-tiered` → `("tiered", "gemini-3.7-flash")`. Only a
/// versioned Gemini id qualifies: the registry's unversioned agent aliases
/// (`gemini-pro-agent`, `gemini-3-flash-agent`) and its non-Google rows are
/// not releases a selection id can be folded from.
fn split_antigravity_tier(lower: &str) -> Option<(&'static str, String)> {
    let rest = lower.strip_prefix("gemini-")?;
    if !rest.chars().next().is_some_and(|first| first.is_ascii_digit()) {
        return None;
    }
    ANTIGRAVITY_TIERS.iter().find_map(|(suffix, tier)| {
        lower
            .strip_suffix(suffix)
            .filter(|base| base.len() > "gemini-".len())
            .map(|base| (*tier, base.to_string()))
    })
}

/// `Gemini 3.6 Flash (High)` → `Gemini 3.6 Flash`.
fn strip_tier_label(name: &str) -> String {
    let trimmed = name.trim();
    let base = ["(Low)", "(Medium)", "(High)", "(Extra Low)"]
        .iter()
        .find_map(|label| trimmed.strip_suffix(label))
        .unwrap_or(trimmed);
    base.trim().to_string()
}

/// One release's tier rungs: `tier` → (wire id, its registry entry).
type TierIds<'a> = HashMap<&'static str, (String, &'a Value)>;

/// Rows from an Antigravity `fetchAvailableModels` document: the tiered Gemini
/// ids folded back into one selection id per release, each carrying the wire
/// map the request path resolves an effort through. Explicit tier ids win
/// over a `-tiered` id when a release publishes both (they are the proven
/// path); a release that publishes only `-tiered` rides it for every effort.
#[must_use]
pub fn antigravity_rows(document: &Value, source: &str) -> Vec<DiscoveredModel> {
    let Some(models) = document.get("models").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut families: Vec<(String, TierIds<'_>)> = Vec::new();
    for (wire_id, entry) in models {
        let lower = wire_id.to_ascii_lowercase();
        let Some((tier, base)) = split_antigravity_tier(&lower) else {
            continue;
        };
        let foreign = entry
            .get("modelProvider")
            .and_then(Value::as_str)
            .is_some_and(|provider| provider != "MODEL_PROVIDER_GOOGLE");
        if foreign {
            continue;
        }
        let at = families
            .iter()
            .position(|(id, _)| *id == base)
            .unwrap_or_else(|| {
                families.push((base, HashMap::new()));
                families.len() - 1
            });
        families[at].1.entry(tier).or_insert((wire_id.clone(), entry));
    }
    families
        .into_iter()
        .enumerate()
        .filter_map(|(index, (id, tiers))| {
            let pick = |tier: &str| tiers.get(tier).map(|(wire, _)| wire.clone());
            let explicit_low = pick("extra-low").or_else(|| pick("low"));
            let explicit_medium = pick("medium").or_else(|| pick("extra-low").and_then(|_| pick("low")));
            let explicit_high = pick("high");
            let wire = if explicit_low.is_some() || explicit_medium.is_some() || explicit_high.is_some() {
                let mut by_effort = Map::new();
                for (rung, wire_id) in [("low", explicit_low), ("medium", explicit_medium), ("high", explicit_high)] {
                    if let Some(wire_id) = wire_id {
                        by_effort.insert(rung.to_string(), json!(wire_id));
                    }
                }
                Value::Object(by_effort)
            } else {
                json!(pick("tiered")?)
            };
            let entry = ["high", "tiered", "medium", "low", "extra-low"]
                .iter()
                .find_map(|tier| tiers.get(tier).map(|(_, entry)| *entry))?;
            let display_name = ["high", "medium", "low", "extra-low", "tiered"]
                .iter()
                .filter_map(|tier| tiers.get(tier))
                .filter_map(|(_, entry)| entry.get("displayName").and_then(Value::as_str))
                .map(strip_tier_label)
                .find(|name| !name.is_empty())
                .unwrap_or_else(|| api::display_name_from_id(api::ProviderKind::Google, &id));
            Some(DiscoveredModel {
                provider: "google".to_string(),
                id,
                display_name,
                class: None,
                context_window: entry.get("maxTokens").and_then(Value::as_u64),
                max_output_tokens: entry.get("maxOutputTokens").and_then(Value::as_u64),
                effort_levels: Vec::new(),
                speed_tiers: Vec::new(),
                released: 0,
                prominence: u32::try_from(index).unwrap_or(u32::MAX),
                source: source.to_string(),
                api_key_only: false,
                wire: Some(wire),
                unlisted_since: None,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Families, ranks, and the overlay
// ---------------------------------------------------------------------------

/// The lineup family an id belongs to within its provider — the token a
/// family alias follows (`fable`, `opus`, `sol`, `astra`, `flash-lite`, …):
/// what its catalog row declares, else what the provider's id grammar spells
/// ([`api::family_from_id`]). `None` for a provider whose ids carry no lineup.
#[must_use]
pub fn family_key(provider: &str, id: &str) -> Option<String> {
    let kind = crate::model_catalog::CatalogProvider::from_key(provider)?.kind();
    api::declared_model_family(id).or_else(|| api::family_from_id(kind, id))
}

fn rank(id: &str) -> u32 {
    crate::model_inventory::release_rank_for_model(id)
}

fn provider_key(kind: api::ProviderKind) -> Option<&'static str> {
    match kind {
        api::ProviderKind::Anthropic => Some("anthropic"),
        api::ProviderKind::OpenAi => Some("openai"),
        api::ProviderKind::Google => Some("google"),
        api::ProviderKind::Xai | api::ProviderKind::Ollama => None,
    }
}

/// A discovered row's provider key as the shipped tables spell it.
fn shipped_provider(provider: &str) -> Option<&'static str> {
    provider_key(crate::model_catalog::CatalogProvider::from_key(provider)?.kind())
}

fn class_key(class: api::ModelClass) -> &'static str {
    match class {
        api::ModelClass::Frontier => "frontier",
        api::ModelClass::Balanced => "balanced",
        api::ModelClass::Fast => "fast",
    }
}

/// The shipped catalog's view of a provider: every id it names (aliases and
/// canonicals, lowercase), the head release of each family, and the newest
/// release the provider ships at all.
struct Shipped {
    ids: HashSet<String>,
    /// `(provider, family)` → the highest-ranked shipped canonical of that family.
    heads: HashMap<(&'static str, String), String>,
    /// provider → the highest release rank among its shipped canonicals. A
    /// discovered id from a family the catalog has no head for still has to
    /// clear this: a lineup the catalog retired (`gpt-5.5`, superseded by the
    /// 5.6 line) is not new because a cache still lists it.
    newest: HashMap<&'static str, u32>,
    /// provider → the canonical its shipped `<provider>-latest` pointer names:
    /// the prior for a family the catalog never saw. Read from the shipped
    /// catalog, never the live registry — the overlay repoints that pointer
    /// once published, and a prior that followed it would move under the
    /// next republish.
    pointers: HashMap<&'static str, String>,
}

fn shipped() -> Shipped {
    let mut ids = HashSet::new();
    let mut heads: HashMap<(&'static str, String), String> = HashMap::new();
    let mut newest: HashMap<&'static str, u32> = HashMap::new();
    let mut pointers: HashMap<&'static str, String> = HashMap::new();
    for entry in api::builtin_provider_catalog() {
        ids.insert(entry.alias.to_ascii_lowercase());
        ids.insert(entry.canonical_model_id.to_ascii_lowercase());
        let Some(provider) = provider_key(entry.provider) else {
            continue;
        };
        if entry.alias.to_ascii_lowercase().ends_with(PROVIDER_POINTER_SUFFIX) {
            pointers.insert(provider, entry.canonical_model_id.to_string());
        }
        let canonical_rank = rank(entry.canonical_model_id);
        newest
            .entry(provider)
            .and_modify(|best| *best = (*best).max(canonical_rank))
            .or_insert(canonical_rank);
        let Some(family) = family_key(provider, entry.canonical_model_id) else {
            continue;
        };
        let canonical = entry.canonical_model_id.to_string();
        heads
            .entry((provider, family))
            .and_modify(|head| {
                if rank(&canonical) > rank(head) {
                    head.clone_from(&canonical);
                }
            })
            .or_insert(canonical);
    }
    Shipped { ids, heads, newest, pointers }
}

/// Newer-first ordering key: version rank, then release date, then the
/// provider's own prominence (lower is more prominent).
fn recency(model: &DiscoveredModel) -> (u32, u64, std::cmp::Reverse<u32>) {
    (rank(&model.id), model.released, std::cmp::Reverse(model.prominence))
}

/// Ids the shipped catalog does not carry and that are at least as recent as
/// their family's shipped head — the rows worth showing and routing to.
#[must_use]
pub fn new_models(catalog: &DiscoveredCatalog) -> Vec<DiscoveredModel> {
    let shipped = shipped();
    let mut seen = HashSet::new();
    let mut rows: Vec<DiscoveredModel> = catalog
        .models
        .iter()
        .filter(|model| {
            let lower = model.id.to_ascii_lowercase();
            if shipped.ids.contains(&lower) || !seen.insert((model.provider.clone(), lower)) {
                return false;
            }
            let Some(family) = family_key(&model.provider, &model.id) else {
                return false;
            };
            let model_rank = rank(&model.id);
            if model_rank == 0 {
                return false;
            }
            let Some(provider) = shipped_provider(&model.provider) else {
                return false;
            };
            // Same family, same release number as the shipped head: the
            // same release under another name (`gemini-3.1-pro` beside the
            // shipped `gemini-3.1-pro-preview`, whose wire already reaches
            // it), not a new one. Only a higher number is news.
            match shipped.heads.get(&(provider, family)) {
                Some(head) => model_rank > rank(head),
                None => shipped
                    .newest
                    .get(provider)
                    .is_none_or(|best| model_rank >= *best),
            }
        })
        .cloned()
        .collect();
    rows.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| recency(right).cmp(&recency(left)))
    });
    rows
}

/// The discovered difference under `policy`.
#[must_use]
pub fn overlay(catalog: &DiscoveredCatalog, policy: UpdatePolicy) -> Overlay {
    if policy == UpdatePolicy::Pinned {
        return Overlay::default();
    }
    let fresh = new_models(catalog);
    let shipped = shipped();
    let mut models = Vec::new();
    let mut aliases = Vec::new();
    for model in &fresh {
        let family = family_key(&model.provider, &model.id);
        let provider = shipped_provider(&model.provider);
        let head = provider.zip(family.as_ref()).and_then(|(provider, family)| {
            shipped.heads.get(&(provider, family.clone())).cloned()
        });
        let prior = head
            .clone()
            .or_else(|| provider.and_then(|provider| shipped.pointers.get(provider).cloned()));
        models.push(discovered_row(
            model,
            family.as_deref(),
            head.as_deref(),
            prior.as_deref(),
            catalog.fetched_at,
        ));
        aliases.push(json!({
            "alias": model.id,
            "canonical": model.id,
            "provider": model.provider,
        }));
    }

    let moves = family_alias_moves(&fresh, policy);
    aliases.extend(moves.rows);
    let alias_updates = moves.updates;
    let alias_candidates = moves.candidates;

    let json = (!models.is_empty() || !aliases.is_empty())
        .then(|| serde_json::to_string(&json!({ "models": models, "aliases": aliases })).ok())
        .flatten();
    Overlay {
        json,
        new_models: fresh,
        alias_updates,
        alias_candidates,
    }
}

/// One discovered release as a catalog row: what the source said, and — for
/// what it did not say — what its family head declares, or the provider's
/// shipped pointer for a family the catalog has never seen (`prior`). The
/// facts the rules read (`family`, `effort_levels`, `speed_tiers`,
/// `capabilities`) travel on the row, so a release the binary predates is as
/// capable as the source says without a rebuild.
fn discovered_row(
    model: &DiscoveredModel,
    family: Option<&str>,
    head: Option<&str>,
    prior: Option<&str>,
    fetched_at: u64,
) -> Value {
    // A family's shipped window and class are the best statement about a
    // release the provider did not describe: the GPT cap (user-directed
    // 258k) and Anthropic's 1M/258k split both live in the shipped rows.
    let head_window = head.map(api::context_window_for_model);
    let context_window = match (model.context_window, head_window) {
        (Some(discovered), Some(head)) if model.provider == "openai" => Some(discovered.min(head)),
        (Some(discovered), _) => Some(discovered),
        (None, head) => head,
    };
    let class = model
        .class
        .clone()
        .or_else(|| head.and_then(api::declared_model_class).map(|class| class_key(class).to_string()));
    let effort_levels = if model.effort_levels.is_empty() {
        prior
            .and_then(api::declared_effort_levels)
            .map(|levels| levels.iter().map(|level| level.key().to_string()).collect::<Vec<_>>())
            .unwrap_or_default()
    } else {
        model.effort_levels.clone()
    };
    let capabilities = prior.map(api::declared_capabilities).unwrap_or_default();

    let mut row = Map::new();
    row.insert("provider".to_string(), json!(model.provider));
    row.insert("ids".to_string(), json!([model.id]));
    if let Some(window) = context_window {
        row.insert("context_window".to_string(), json!(window));
    }
    if let Some(max_output) = model.max_output_tokens {
        row.insert("max_output_tokens".to_string(), json!(max_output));
    }
    if let Some(class) = class {
        row.insert("class".to_string(), json!(class));
    }
    if let Some(wire) = &model.wire {
        row.insert("wire".to_string(), wire.clone());
    }
    if let Some(family) = family {
        row.insert("family".to_string(), json!(family));
    }
    row.insert("display_name".to_string(), json!(model.display_name));
    if !effort_levels.is_empty() {
        row.insert("effort_levels".to_string(), json!(effort_levels));
    }
    if !model.speed_tiers.is_empty() {
        row.insert("speed_tiers".to_string(), json!(model.speed_tiers));
    }
    if !capabilities.is_empty() {
        row.insert("capabilities".to_string(), json!(capabilities));
    }
    row.insert(
        "source".to_string(),
        json!(format!(
            "discovered via {} at {}{}",
            model.source,
            fetched_at,
            prior
                .map(|prior| format!("; undeclared facts inherited from {prior}"))
                .unwrap_or_default()
        )),
    );
    Value::Object(row)
}

/// Where the family aliases go once the discovered rows are known.
struct AliasMoves {
    /// Alias rows for the overlay (policy `auto` only).
    rows: Vec<Value>,
    updates: Vec<AliasUpdate>,
    candidates: Vec<AliasUpdate>,
}

/// A provider pointer (`openai-latest`, `claude-latest`): the alias that names
/// the provider's newest release, whatever family it comes from.
const PROVIDER_POINTER_SUFFIX: &str = "-latest";

/// The token a provider's family aliases may be spelled behind
/// (`claude-opus`, `gemini-flash`); OpenAI aliases are bare (`sol`).
fn alias_prefix(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("claude"),
        "google" => Some("gemini"),
        _ => None,
    }
}

/// Where the shipped aliases go once the discovered rows are known, and the
/// aliases a family the catalog has never seen is given — carried as override
/// rows under `auto`, reported under `notify`, ignored under `pinned`.
///
/// - A family alias (`fable`, `sol`, `gemini-flash`) follows the newest
///   discovered release of its family. It keeps its shipped duties
///   (`orchestration_rank`, `demotes_to`): a new release changes what the name
///   resolves to, not what the name is for.
/// - A provider pointer (`openai-latest`) follows the provider's newest
///   release across families, so a new generation heads the provider the day
///   it is discovered (GPT-6 astra after the 5.6 line).
/// - A new family gets the aliases its provider's grammar gives every family:
///   the codename (`astra`), the prefixed spelling where the provider uses one
///   (`claude-nova`), and for OpenAI the versioned lineup name (`gpt-6`) that
///   pins the generation the way `gpt-5.6` does.
impl AliasMoves {
    /// One alias landing on `to`: an override row (with the duties it keeps)
    /// under `auto`, a candidate under `notify`, nothing under `pinned`.
    fn record(
        &mut self,
        policy: UpdatePolicy,
        alias: &str,
        provider: &str,
        from: &str,
        to: &DiscoveredModel,
        duties: Option<&api::ProviderCatalogEntry>,
    ) {
        let update = AliasUpdate {
            provider: provider.to_string(),
            alias: alias.to_string(),
            from: from.to_string(),
            to: to.id.clone(),
        };
        match policy {
            UpdatePolicy::Auto => {
                let mut row = Map::new();
                row.insert("alias".to_string(), json!(alias));
                row.insert("canonical".to_string(), json!(to.id));
                row.insert("provider".to_string(), json!(provider));
                if let Some(orchestration_rank) = duties.and_then(|entry| entry.orchestration_rank) {
                    row.insert("orchestration_rank".to_string(), json!(orchestration_rank));
                }
                if let Some(demotes_to) = duties.and_then(|entry| entry.demotes_to) {
                    row.insert("demotes_to".to_string(), json!(demotes_to));
                }
                self.rows.push(Value::Object(row));
                self.updates.push(update);
            }
            UpdatePolicy::Notify => self.candidates.push(update),
            UpdatePolicy::Pinned => {}
        }
    }
}

fn family_alias_moves(fresh: &[DiscoveredModel], policy: UpdatePolicy) -> AliasMoves {
    let mut moves = AliasMoves {
        rows: Vec::new(),
        updates: Vec::new(),
        candidates: Vec::new(),
    };
    let mut shipped_families: Vec<(&'static str, String)> = Vec::new();
    for entry in api::builtin_provider_catalog() {
        let Some(provider) = provider_key(entry.provider) else {
            continue;
        };
        let Some(family) = family_key(provider, entry.canonical_model_id) else {
            continue;
        };
        // A versioned name is a pin, not a family pointer: `gemini-3.6-flash`
        // must keep meaning 3.6 the day 3.7 appears, and `claude-opus-4-8`
        // stays 4.8. Only names with no release in them (`gemini-flash`,
        // `google-latest`, `fable`, `opus[1m]`) follow a newer release.
        if rank(entry.alias) > 0 {
            continue;
        }
        let bare = entry.alias.split('[').next().unwrap_or(entry.alias);
        if bare.eq_ignore_ascii_case(&family)
            || alias_prefix(provider).is_some_and(|prefix| bare.eq_ignore_ascii_case(&format!("{prefix}-{family}")))
        {
            shipped_families.push((provider, family.clone()));
        }
        let is_pointer = entry.alias.to_ascii_lowercase().ends_with(PROVIDER_POINTER_SUFFIX);
        let current_rank = rank(entry.canonical_model_id);
        let Some(newest) = fresh
            .iter()
            .filter(|model| model.provider == provider)
            .filter(|model| is_pointer || family_key(provider, &model.id).as_deref() == Some(family.as_str()))
            .max_by_key(|model| recency(model))
        else {
            continue;
        };
        if rank(&newest.id) <= current_rank || newest.id.eq_ignore_ascii_case(entry.canonical_model_id) {
            continue;
        }
        moves.record(policy, entry.alias, provider, entry.canonical_model_id, newest, Some(entry));
    }
    mint_new_family_aliases(fresh, &shipped_families, policy, &mut moves);
    moves
}

/// Families the shipped catalog has never named get one set of aliases each,
/// on the family's newest release: the codename, the provider's prefixed
/// spelling where it uses one, and for OpenAI the versioned lineup name.
fn mint_new_family_aliases(
    fresh: &[DiscoveredModel],
    shipped_families: &[(&'static str, String)],
    policy: UpdatePolicy,
    moves: &mut AliasMoves,
) {
    let mut minted: Vec<(String, String)> = Vec::new();
    for model in fresh {
        let Some(family) = family_key(&model.provider, &model.id) else {
            continue;
        };
        let provider = model.provider.as_str();
        // A codename-less id (`gpt-6`) keys on its lineup word, and that word
        // is the lineup's own alias (`gpt`), never a family to mint. Decided
        // by the id grammar, not the registry: once a minted alias is
        // published it is a registered OpenAI name, and a registry test would
        // stop minting it on the next publish — erasing it.
        let lineup_word = api::is_openai_lineup_word(&family);
        if lineup_word
            || shipped_families.iter().any(|(p, f)| *p == provider && f.eq_ignore_ascii_case(&family))
            || minted.iter().any(|(p, f)| p == provider && f.eq_ignore_ascii_case(&family))
        {
            continue;
        }
        let Some(newest) = fresh
            .iter()
            .filter(|candidate| candidate.provider == provider)
            .filter(|candidate| family_key(provider, &candidate.id).as_deref() == Some(family.as_str()))
            .max_by_key(|candidate| recency(candidate))
        else {
            continue;
        };
        minted.push((provider.to_string(), family.clone()));
        moves.record(policy, &family, provider, "", newest, None);
        if let Some(prefix) = alias_prefix(provider) {
            moves.record(policy, &format!("{prefix}-{family}"), provider, "", newest, None);
        }
        if provider == "openai" {
            // `gpt-6-astra` minus its codename is `gpt-6`, the versioned lineup
            // name; a codename-less id has nothing left to mint.
            let lower = newest.id.to_ascii_lowercase();
            let versioned = lower
                .split('-')
                .filter(|token| !token.eq_ignore_ascii_case(&family))
                .collect::<Vec<_>>()
                .join("-");
            if versioned != lower && rank(&versioned) > 0 {
                moves.record(policy, &versioned, provider, "", newest, None);
            }
        }
    }
}

/// `{"<id>": "<ceiling>"}` for the discovered ids that declare one, in the
/// shape `api::MODEL_EFFORT_CEILINGS_ENV` reads. Shipped ids are left to the
/// shipped rules.
#[must_use]
pub fn effort_ceilings_json(catalog: &DiscoveredCatalog, policy: UpdatePolicy) -> Option<String> {
    if policy == UpdatePolicy::Pinned {
        return None;
    }
    let mut ceilings = Map::new();
    for model in new_models(catalog) {
        if let Some(ceiling) = model.effort_ceiling() {
            ceilings.insert(model.id.clone(), json!(ceiling));
        }
    }
    (!ceilings.is_empty())
        .then(|| serde_json::to_string(&Value::Object(ceilings)).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::{
        anthropic_rows, antigravity_rows, codex_cache_rows, drop_api_key_duplicates,
        effort_ceilings_json, family_key, google_rows, new_models, overlay, DiscoveredCatalog,
        DiscoveredModel, UpdatePolicy,
    };
    use serde_json::{json, Value};
    use std::collections::HashMap;

    fn model(provider: &str, id: &str, released: u64) -> DiscoveredModel {
        DiscoveredModel {
            provider: provider.to_string(),
            id: id.to_string(),
            display_name: id.to_string(),
            released,
            source: "test".to_string(),
            ..Default::default()
        }
    }

    fn catalog(models: Vec<DiscoveredModel>) -> DiscoveredCatalog {
        DiscoveredCatalog {
            fetched_at: 1_700_000_000,
            reports: Vec::new(),
            models,
        }
    }

    fn overlay_value(catalog: &DiscoveredCatalog, policy: UpdatePolicy) -> Value {
        serde_json::from_str(&overlay(catalog, policy).json.expect("an overlay")).unwrap()
    }

    #[test]
    fn a_new_family_head_repoints_every_alias_of_that_family_under_auto() {
        let catalog = catalog(vec![
            model("anthropic", "claude-fable-5-2", 20_261_201),
            model("anthropic", "claude-fable-5-1", 20_260_901),
            model("anthropic", "claude-fable-5", 20_260_601),
            model("anthropic", "claude-3-5-sonnet-20241022", 20_241_022),
        ]);
        let result = overlay(&catalog, UpdatePolicy::Auto);
        let ids: Vec<&str> = result.new_models.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["claude-fable-5-2"], "shipped and stale ids stay out");
        let aliases: Vec<(String, String)> = result
            .alias_updates
            .iter()
            .map(|update| (update.alias.clone(), update.to.clone()))
            .collect();
        assert!(aliases.contains(&("fable".to_string(), "claude-fable-5-2".to_string())));
        assert!(aliases.contains(&("claude-fable".to_string(), "claude-fable-5-2".to_string())));
        assert!(
            !aliases.iter().any(|(alias, _)| alias == "opus"),
            "no newer opus was discovered, so opus stays"
        );
        let value = overlay_value(&catalog, UpdatePolicy::Auto);
        let fable = value["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["alias"] == "fable")
            .expect("fable row");
        assert_eq!(fable["canonical"], "claude-fable-5-2");
        assert_eq!(fable["orchestration_rank"], 0, "the reserved rank travels with the alias");
        let row = value["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["ids"][0] == "claude-fable-5-2")
            .expect("model row");
        assert_eq!(row["context_window"], 1_000_000, "the family's shipped window is inherited");
        assert_eq!(row["class"], "frontier", "and so is its declared class");
    }

    #[test]
    fn notify_keeps_aliases_but_still_adds_the_ids() {
        let catalog = catalog(vec![model("anthropic", "claude-opus-5-1", 20_260_901)]);
        let result = overlay(&catalog, UpdatePolicy::Notify);
        assert_eq!(result.new_models.len(), 1);
        assert!(result.alias_updates.is_empty());
        assert!(result.alias_candidates.iter().any(|update| update.alias == "opus" && update.to == "claude-opus-5-1"));
        let value = overlay_value(&catalog, UpdatePolicy::Notify);
        assert!(value["aliases"].as_array().unwrap().iter().all(|row| row["alias"] == "claude-opus-5-1"));
    }

    #[test]
    fn pinned_publishes_nothing() {
        let catalog = catalog(vec![model("anthropic", "claude-opus-5-1", 20_260_901)]);
        let result = overlay(&catalog, UpdatePolicy::Pinned);
        assert!(result.json.is_none());
        assert!(result.new_models.is_empty());
        assert!(effort_ceilings_json(&catalog, UpdatePolicy::Pinned).is_none());
    }

    #[test]
    fn an_older_or_versionless_id_never_becomes_a_row() {
        let catalog = catalog(vec![
            model("openai", "gpt-reserve", 0),
            model("openai", "codex-auto-review", 0),
            // Retired lineups a cache still lists: no family head of their
            // own, and older than the newest shipped release.
            model("openai", "gpt-5.5", 0),
            model("openai", "gpt-5.4", 0),
            model("openai", "gpt-5.4-mini", 0),
            model("openai", "gpt-5.6-sol", 0),
            model("anthropic", "claude-opus-4-7", 20_260_301),
        ]);
        assert!(new_models(&catalog).is_empty());
        // A genuinely newer generic release still clears the bar.
        let newer = DiscoveredCatalog {
            fetched_at: 1_700_000_000,
            reports: Vec::new(),
            models: vec![model("openai", "gpt-6", 0)],
        };
        assert_eq!(new_models(&newer).len(), 1);
    }

    #[test]
    fn a_newer_gpt_inherits_the_family_cap_and_its_declared_ceiling() {
        let document = json!({
            "models": [
                {"slug": "gpt-5.7-sol", "display_name": "GPT-5.7-Sol", "description": "Latest frontier agentic coding model.", "priority": 1, "visibility": "list", "context_window": 400_000, "effective_context_window_percent": 95, "supported_reasoning_levels": [{"effort": "low"}, {"effort": "ultra"}], "additional_speed_tiers": ["fast"]},
                {"slug": "gpt-5.7-hidden", "visibility": "hide", "priority": 9}
            ]
        });
        let rows = codex_cache_rows(&document, "codex-cache");
        assert_eq!(rows.len(), 1, "hidden rows stay hidden");
        assert_eq!(rows[0].class.as_deref(), Some("frontier"));
        assert_eq!(rows[0].effort_ceiling().as_deref(), Some("ultra"));
        assert_eq!(rows[0].speed_tiers, vec!["fast".to_string()]);
        assert_eq!(rows[0].context_window, Some(380_000));
        let catalog = catalog(rows);
        let value = overlay_value(&catalog, UpdatePolicy::Auto);
        let row = &value["models"].as_array().unwrap()[0];
        assert_eq!(row["context_window"], 258_000, "the GPT family cap holds");
        assert!(value["aliases"].as_array().unwrap().iter().any(|row| row["alias"] == "sol" && row["canonical"] == "gpt-5.7-sol"));
        assert!(value["aliases"].as_array().unwrap().iter().any(|row| row["alias"] == "openai-latest" && row["canonical"] == "gpt-5.7-sol"));
        let ceilings: Value = serde_json::from_str(&effort_ceilings_json(&catalog, UpdatePolicy::Auto).unwrap()).unwrap();
        assert_eq!(ceilings["gpt-5.7-sol"], "ultra");
    }

    #[test]
    fn anthropic_and_google_documents_parse_to_rows() {
        let anthropic = json!({"data": [
            {"id": "claude-fable-5-1", "display_name": "Claude Fable 5.1", "created_at": "2026-09-01T00:00:00Z", "type": "model"},
            {"id": "", "display_name": "blank"}
        ]});
        let rows = anthropic_rows(&anthropic, "anthropic-api");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].released, 20_260_901);
        // The API's maker word comes off: the catalog shows the lineup name
        // and the prompt label adds "Claude" once.
        assert_eq!(rows[0].display_name, "Fable 5.1");

        let google = json!({"models": [
            {"name": "models/gemini-3.7-flash", "displayName": "Gemini 3.7 Flash", "inputTokenLimit": 1_048_576, "outputTokenLimit": 65536, "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/gemini-embedding-2", "supportedGenerationMethods": ["embedContent"]},
            {"name": "models/gemini-3.7-flash-image", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/gemini-3.7-pro-exp-0901", "supportedGenerationMethods": ["generateContent"]}
        ]});
        let rows = google_rows(&google, "google-api");
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["gemini-3.7-flash"]);
        assert!(rows[0].api_key_only);
        let catalog = catalog(rows);
        let value = overlay_value(&catalog, UpdatePolicy::Auto);
        assert!(value["aliases"].as_array().unwrap().iter().any(|row| row["alias"] == "gemini-flash" && row["canonical"] == "gemini-3.7-flash"));
        assert!(value["aliases"].as_array().unwrap().iter().any(|row| row["alias"] == "google-latest" && row["canonical"] == "gemini-3.7-flash"));
        assert!(!value["aliases"].as_array().unwrap().iter().any(|row| row["alias"] == "gemini-flash-lite"), "a flash release is not a flash-lite release");
        for pinned in ["gemini-3.6-flash", "gemini-3.5-flash", "gemini-3-flash", "gemini-3-flash-preview"] {
            assert!(
                !value["aliases"].as_array().unwrap().iter().any(|row| row["alias"] == pinned),
                "{pinned} names a release; it must not follow the family head"
            );
        }
    }

    #[test]
    fn the_same_release_number_under_another_name_is_not_a_new_model() {
        // The shipped pro head is a 3.1 (`gemini-3.1-pro-preview`); the
        // registry's folded `gemini-3.1-pro` is the same release, not news.
        let same = catalog(vec![model("google", "gemini-3.1-pro", 0)]);
        assert!(new_models(&same).is_empty());
        let newer = catalog(vec![model("google", "gemini-3.7-flash", 0)]);
        assert_eq!(new_models(&newer).len(), 1);
    }

    #[test]
    fn family_keys_follow_the_id_grammar() {
        assert_eq!(family_key("anthropic", "claude-opus-5-1").as_deref(), Some("opus"));
        assert_eq!(family_key("anthropic", "claude-fable-5[1m]").as_deref(), Some("fable"));
        assert_eq!(family_key("openai", "gpt-5.7-terra").as_deref(), Some("terra"));
        assert_eq!(family_key("openai", "gpt-6").as_deref(), Some("gpt"));
        assert_eq!(family_key("openai", "gpt-6-astra").as_deref(), Some("astra"));
        assert_eq!(family_key("openai", "gpt-5.3-codex-spark").as_deref(), Some("spark"));
        assert_eq!(family_key("google", "gemini-3.1-flash-lite").as_deref(), Some("flash-lite"));
        assert_eq!(family_key("google", "gemini-4-pro").as_deref(), Some("pro"));
        assert_eq!(family_key("xai", "grok-5"), None);
    }

    /// GPT-6 astra, the case that found the gap (t-2506): a release from a
    /// family the catalog has never seen must head the provider, get its
    /// own aliases, and carry the facts the rules read — without a rebuild.
    #[test]
    fn a_new_generation_heads_its_provider_and_mints_its_aliases() {
        let mut astra = model("openai", "gpt-6-astra", 0);
        astra.display_name = "GPT-6-Astra".to_string();
        astra.effort_levels = ["low", "medium", "high", "xhigh", "max", "ultra"]
            .map(str::to_string)
            .to_vec();
        astra.speed_tiers = vec!["fast".to_string()];
        astra.prominence = 1;
        let catalog = catalog(vec![astra, model("openai", "gpt-5.6-sol", 0)]);

        let result = overlay(&catalog, UpdatePolicy::Auto);
        let moved: Vec<(String, String)> = result
            .alias_updates
            .iter()
            .map(|update| (update.alias.clone(), update.to.clone()))
            .collect();
        for alias in ["openai-latest", "astra", "gpt-6"] {
            assert!(
                moved.contains(&(alias.to_string(), "gpt-6-astra".to_string())),
                "{alias} should point at astra: {moved:?}"
            );
        }
        assert!(!moved.iter().any(|(alias, _)| alias == "sol"), "sol is not astra's family");

        let value = overlay_value(&catalog, UpdatePolicy::Auto);
        let row = value["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["ids"][0] == "gpt-6-astra")
            .expect("astra row");
        assert_eq!(row["family"], "astra");
        assert_eq!(row["display_name"], "GPT-6-Astra");
        assert_eq!(row["effort_levels"].as_array().unwrap().len(), 6);
        assert_eq!(row["speed_tiers"], json!(["fast"]));
        assert_eq!(
            row["capabilities"],
            json!(["imagegen"]),
            "a family the catalog never saw inherits the provider default's capabilities"
        );
        let pointer = value["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["alias"] == "openai-latest")
            .expect("pointer row");
        assert_eq!(pointer["orchestration_rank"], 1, "the pointer keeps its duty");
        assert_eq!(
            effort_ceilings_json(&catalog, UpdatePolicy::Auto).as_deref(),
            Some(r#"{"gpt-6-astra":"ultra"}"#)
        );

        // Under `notify` the same moves are offered, not applied.
        let withheld = overlay(&catalog, UpdatePolicy::Notify);
        assert!(withheld.alias_updates.is_empty());
        assert!(withheld.alias_candidates.iter().any(|update| update.alias == "astra"));
    }

    #[test]
    fn the_policy_and_ttl_read_from_settings() {
        let dir = std::env::temp_dir().join(format!("zo-discovery-policy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"modelUpdatePolicy": "notify", "modelDiscoveryTtlHours": 0.5}"#).unwrap();
        assert_eq!(UpdatePolicy::load_from(&path), UpdatePolicy::Notify);
        assert_eq!(super::ttl_secs_from(&path), 1800);
        std::fs::write(&path, r#"{"modelUpdatePolicy": "bogus"}"#).unwrap();
        assert_eq!(UpdatePolicy::load_from(&path), UpdatePolicy::Auto);
        assert_eq!(super::ttl_secs_from(&path), super::DEFAULT_TTL_SECS);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("zo-discovery-cache-{}", std::process::id()));
        let path = dir.join("nested").join("discovered.json");
        let catalog = catalog(vec![model("anthropic", "claude-fable-5-1", 20_260_901)]);
        super::save_to(&path, &catalog).unwrap();
        assert_eq!(super::load_cached_from(&path), Some(catalog.clone()));
        assert!(catalog.stale(catalog.fetched_at + 2 * super::DEFAULT_TTL_SECS, super::DEFAULT_TTL_SECS));
        assert!(!catalog.stale(catalog.fetched_at + super::DEFAULT_TTL_SECS / 2, super::DEFAULT_TTL_SECS));
        assert_eq!(super::DEFAULT_TTL_SECS, super::LIVE_TTL_SECS, "one table: the default TTL is the connection TTL");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn codex_cache_document(stamp: &str, etag: Option<&str>, version: &str, slugs: &[&str]) -> String {
        let models: Vec<Value> = slugs
            .iter()
            .enumerate()
            .map(|(index, slug)| {
                json!({
                    "slug": slug,
                    "display_name": slug.to_ascii_uppercase(),
                    "description": "Our most capable model for complex, demanding work.",
                    "supported_reasoning_levels": [{"effort": "low"}, {"effort": "ultra"}],
                    "visibility": "list",
                    "priority": index + 1,
                    "context_window": 400_000,
                })
            })
            .collect();
        serde_json::to_string_pretty(&json!({
            "fetched_at": stamp,
            "etag": etag,
            "client_version": version,
            "models": models,
        }))
        .unwrap()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zo-discovery-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// (a) The fallback picks the newest cache by its OWN `fetched_at`, not
    /// by list order or mtime, and names the file it chose; a file that is
    /// not a cache is skipped, not fatal.
    #[test]
    fn the_freshest_codex_cache_is_picked_by_its_own_stamp_and_named() {
        let dir = scratch("freshest");
        let account = dir.join("account").join(super::CODEX_MODELS_CACHE_FILE);
        let personal = dir.join("personal").join(super::CODEX_MODELS_CACHE_FILE);
        let runtime = dir.join("runtime").join(super::CODEX_MODELS_CACHE_FILE);
        let broken = dir.join("broken").join(super::CODEX_MODELS_CACHE_FILE);
        for path in [&account, &personal, &runtime, &broken] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        // The account cache is the oldest (the 09-04 file that lost astra),
        // the personal one is the newest, the runtime one sits between.
        std::fs::write(&account, codex_cache_document("2026-09-04T09:00:00.000000Z", None, "0.153.1", &["gpt-5.6-sol"])).unwrap();
        std::fs::write(&personal, codex_cache_document("2026-09-07T08:25:08.756120Z", Some("W/\"a\""), "0.153.4", &["gpt-6-astra", "gpt-5.6-sol"])).unwrap();
        std::fs::write(&runtime, codex_cache_document("2026-09-05T00:00:00Z", None, "0.153.1", &["gpt-5.6-sol"])).unwrap();
        std::fs::write(&broken, "{not json").unwrap();

        let paths = vec![account.clone(), broken.clone(), runtime.clone(), personal.clone()];
        let freshest = super::freshest_codex_cache(&paths).expect("a readable cache");
        assert_eq!(freshest.path, personal, "the newest stamp wins wherever it sits in the list");
        assert_eq!(freshest.fetched_at, 1_788_769_508, "the stamp is read as unix seconds");
        assert_eq!(freshest.etag.as_deref(), Some("W/\"a\""));
        assert_eq!(freshest.client_version.as_deref(), Some("0.153.4"));

        let answer = super::codex_cache_models_from(&paths).expect("rows from the freshest cache");
        let ids: Vec<&str> = answer.models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["gpt-6-astra", "gpt-5.6-sol"]);
        assert_eq!(answer.origin, format!("cache:{}", personal.parent().unwrap().display()), "the report names the home");
        assert_eq!(answer.fetched_at, freshest.fetched_at, "and dates the rows from that file");
        assert!(answer.models.iter().all(|model| model.source == super::OPENAI_SOURCE));

        let (_, detail) = super::codex_cache_models_from(&[dir.join("absent").join("models_cache.json")])
            .expect_err("no cache anywhere is a skip, not rows");
        assert!(detail.starts_with("skipped: no models_cache.json under "), "{detail}");
        assert!(super::freshest_codex_cache(&[]).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// One scripted answer: `(status line, extra headers, body)`.
    type ScriptedAnswer = (&'static str, Vec<(&'static str, &'static str)>, String);

    /// A one-connection-at-a-time HTTP server on a thread: it answers the
    /// script in order, and every request head it saw is handed back for
    /// the assertions.
    fn fake_backend(script: Vec<ScriptedAnswer>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::fmt::Write as _;
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/backend-api/codex/models", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut heads = Vec::new();
            for (status, headers, body) in script {
                let (mut socket, _) = listener.accept().unwrap();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && socket.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                heads.push(String::from_utf8_lossy(&head).into_owned());
                let mut response = format!("HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: {}\r\n", body.len());
                for (name, value) in &headers {
                    let _ = write!(response, "{name}: {value}\r\n");
                }
                response.push_str("\r\n");
                response.push_str(&body);
                socket.write_all(response.as_bytes()).unwrap();
                socket.flush().unwrap();
            }
            heads
        });
        (url, handle)
    }

    /// The live request asks Codex's own list, never the ChatGPT web picker's:
    /// the plain `/backend-api/models` answers 200 with twenty web-app rows
    /// (`gpt-5-6-instant`, `-wm` variants, no `visibility`), and written back
    /// as Codex's cache they blanked every OpenAI row the window lists
    /// (2026-09-08). An answer whose rows do not carry `slug` and
    /// `visibility` is not Codex's list — it is refused, nothing is written,
    /// and the source falls back like any other failure.
    #[test]
    fn the_web_pickers_catalog_is_refused_and_nothing_is_written() {
        assert!(
            super::CHATGPT_MODELS_URL.ends_with("/backend-api/codex/models"),
            "the live source must ask Codex's list: {}",
            super::CHATGPT_MODELS_URL
        );
        let dir = scratch("web-picker");
        let cache_path = dir.join("account").join(super::CODEX_MODELS_CACHE_FILE);
        let tokens = core_types::OpenAiOAuthTokens {
            access_token: "secret-bearer".to_string(),
            refresh_token: None,
            expires_at: None,
            account_id: Some("acct_42".to_string()),
            scopes: Vec::new(),
        };
        let web_body = json!({
            "categories": [], "default_model_slug": "gpt-5-6", "model_picker_version": 3,
            "models": [
                {"slug": "gpt-5-6", "title": "GPT-5.6", "tags": ["default"]},
                {"slug": "gpt-5-6-instant", "title": "GPT-5.6 Instant", "tags": []},
                {"slug": "gpt-6-astra-wm", "title": "GPT-6 Astra", "tags": []}
            ]
        });
        let (url, backend) = fake_backend(vec![("200 OK", vec![], web_body.to_string())]);
        let (source, detail) = super::chatgpt_backend_models_at(&url, &tokens, &cache_path, None, 1_788_800_000)
            .expect_err("the web picker's catalog is not Codex's list");
        assert_eq!(source, super::OPENAI_SOURCE);
        assert!(detail.contains("not Codex's model list"), "{detail}");
        assert!(!cache_path.exists(), "nothing is written back from a refused answer");
        assert_eq!(backend.join().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// (b) The live source: parses the cache-shaped answer, sends the cache's
    /// etag and keeps the cache on 304, writes Codex's own format back, and
    /// falls back to the freshest cache when the backend refuses or is gone.
    #[test]
    fn the_live_chatgpt_source_parses_honours_the_etag_writes_codex_format_and_falls_back() {
        let dir = scratch("live");
        let account_home = dir.join("account");
        let cache_path = account_home.join(super::CODEX_MODELS_CACHE_FILE);
        let tokens = core_types::OpenAiOAuthTokens {
            access_token: "secret-bearer".to_string(),
            refresh_token: None,
            expires_at: None,
            account_id: Some("acct_42".to_string()),
            scopes: Vec::new(),
        };
        let live_body = json!({"models": [
            {"slug": "gpt-6-astra", "display_name": "GPT-6-Astra", "description": "Our most capable model.", "visibility": "list", "priority": 1, "context_window": 400_000, "supported_reasoning_levels": [{"effort": "low"}, {"effort": "ultra"}]},
            {"slug": "gpt-5.6-sol", "display_name": "GPT-5.6-Sol", "visibility": "list", "priority": 2},
            {"slug": "gpt-5.6-hidden", "visibility": "hide", "priority": 3}
        ]})
        .to_string();
        let (url, server) = fake_backend(vec![
            ("200 OK", vec![("etag", "W/\"v1\""), ("content-type", "application/json")], live_body),
            ("304 Not Modified", vec![("etag", "W/\"v1\"")], String::new()),
            ("401 Unauthorized", vec![], "{\"detail\":\"expired\"}".to_string()),
        ]);

        // 1. No cache yet: a plain fetch, parsed with the cache parser, written back.
        let first = super::chatgpt_backend_models_at(&url, &tokens, &cache_path, None, 1_788_500_000)
            .expect("the live answer");
        let ids: Vec<&str> = first.models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["gpt-6-astra", "gpt-5.6-sol"], "hidden rows stay hidden, as in the cache parser");
        assert_eq!(first.origin, super::ORIGIN_LIVE);
        assert_eq!(first.fetched_at, 1_788_500_000);
        assert_eq!(first.note, None);
        let raw = std::fs::read_to_string(&cache_path).unwrap();
        let written: Value = serde_json::from_str(&raw).unwrap();
        let at = |key: &str| raw.find(&format!("\"{key}\":")).unwrap_or_else(|| panic!("{key} in {raw}"));
        assert!(
            at("fetched_at") < at("etag") && at("etag") < at("client_version") && at("client_version") < at("models"),
            "Codex's own field order, top to bottom: {}",
            raw.lines().take(4).collect::<Vec<_>>().join(" | ")
        );
        assert!(raw.starts_with("{\n  \"fetched_at\": \""), "pretty-printed with two spaces, as Codex writes it: {raw:.60}");
        assert_eq!(written["etag"], "W/\"v1\"");
        assert_eq!(written["client_version"], super::CODEX_CLIENT_VERSION_DEFAULT, "no cache named a version: the table's default");
        assert_eq!(written["models"].as_array().unwrap().len(), 3, "the answer is stored whole, hidden rows included");
        let stamp = written["fetched_at"].as_str().unwrap();
        assert!(stamp.len() == 27 && stamp.ends_with('Z') && stamp.as_bytes()[19] == b'.', "an RFC 3339 stamp with microseconds, as Codex writes it: {stamp}");
        assert!(core_types::date::unix_secs_from_rfc3339(stamp).is_some());
        assert!(!std::fs::read_dir(&account_home).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().contains(".tmp")), "the temp file is gone");
        let cache = super::read_codex_cache(&cache_path).unwrap();
        assert!(cache.fetched_at > 0);

        // 2. The cache carries the etag: the request says If-None-Match, 304 keeps it.
        let second = super::chatgpt_backend_models_at(&url, &tokens, &cache_path, Some(&cache), 1_788_503_600)
            .expect("304 is an answer");
        assert_eq!(second.models.len(), 2, "the cache's rows, re-dated");
        assert_eq!(second.fetched_at, 1_788_503_600);
        assert_eq!(second.note.as_deref(), Some("304, the cache is current"));
        let rewritten: Value = serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        assert_eq!(rewritten["models"], written["models"], "a 304 keeps the models");
        assert_eq!(rewritten["etag"], "W/\"v1\"");
        assert_ne!(rewritten["fetched_at"], written["fetched_at"], "and re-dates the file so Codex trusts it too");

        // 3. 401: the live call fails and the freshest cache answers instead.
        let refreshed = super::read_codex_cache(&cache_path).unwrap();
        let (source, detail) = super::chatgpt_backend_models_at(&url, &tokens, &cache_path, Some(&refreshed), 1_788_507_200)
            .expect_err("401 is a failure of the live source");
        assert_eq!(source, super::OPENAI_SOURCE);
        assert!(detail.starts_with("HTTP 401"), "{detail}");
        assert!(!detail.contains("secret-bearer"), "the token never reaches a report line");
        let fallback = super::codex_cache_models_from(std::slice::from_ref(&cache_path)).expect("the cache we wrote");
        assert_eq!(fallback.origin, format!("cache:{}", account_home.display()));
        assert_eq!(fallback.models.len(), 2);

        let heads = server.join().unwrap();
        assert_eq!(heads.len(), 3);
        for head in &heads {
            let request_line = head.lines().next().unwrap();
            assert!(request_line.starts_with(&format!("GET /backend-api/codex/models?client_version={} HTTP/1.1", super::CODEX_CLIENT_VERSION_DEFAULT)), "{request_line}");
            let lower = head.to_ascii_lowercase();
            assert!(lower.contains("authorization: bearer secret-bearer"), "{head}");
            assert!(lower.contains("chatgpt-account-id: acct_42"), "{head}");
            assert!(lower.contains("originator: codex_cli_rs"), "{head}");
            assert!(lower.contains("user-agent: codex_cli_rs/"), "{head}");
        }
        assert!(!heads[0].to_ascii_lowercase().contains("if-none-match"), "no cache, no etag: {}", heads[0]);
        assert!(heads[1].to_ascii_lowercase().contains("if-none-match: w/\"v1\""), "{}", heads[1]);

        // 4. Offline: nothing listens on the port any more.
        let (offline_url, gone) = fake_backend(Vec::new());
        gone.join().unwrap();
        let (_, detail) = super::chatgpt_backend_models_at(&offline_url, &tokens, &cache_path, None, 1_788_510_800)
            .expect_err("a refused connection is a failure of the live source");
        assert!(!detail.starts_with("skipped:"), "a transport failure keeps last time's rows: {detail}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The write-back never lands in another account's Codex home: a home
    /// whose `auth.json` names a different account keeps its cache, one that
    /// names the same account (or none) takes it.
    #[test]
    fn the_cache_is_written_only_into_the_logins_own_codex_home() {
        let tokens = |account: Option<&str>| core_types::OpenAiOAuthTokens {
            access_token: "bearer".to_string(),
            refresh_token: None,
            expires_at: None,
            account_id: account.map(str::to_string),
            scopes: Vec::new(),
        };
        let dir = scratch("write-guard");
        let mine = dir.join("mine");
        let theirs = dir.join("theirs");
        let nobody = dir.join("nobody");
        let empty = dir.join("empty");
        for home in [&mine, &theirs, &nobody, &empty] {
            std::fs::create_dir_all(home).unwrap();
        }
        std::fs::write(mine.join("auth.json"), r#"{"tokens":{"account_id":"acct_1","access_token":"x"}}"#).unwrap();
        std::fs::write(theirs.join("auth.json"), r#"{"tokens":{"account_id":"acct_2","access_token":"x"}}"#).unwrap();
        std::fs::write(nobody.join("auth.json"), r#"{"OPENAI_API_KEY":"sk"}"#).unwrap();
        let cache = |home: &std::path::Path| home.join(super::CODEX_MODELS_CACHE_FILE);
        assert_eq!(super::cache_write_allowed(&cache(&mine), &tokens(Some("acct_1"))), Ok(()));
        assert_eq!(
            super::cache_write_allowed(&cache(&theirs), &tokens(Some("acct_1"))),
            Err("another account's Codex home".to_string())
        );
        assert_eq!(super::cache_write_allowed(&cache(&nobody), &tokens(Some("acct_1"))), Ok(()));
        assert_eq!(super::cache_write_allowed(&cache(&empty), &tokens(Some("acct_1"))), Ok(()));
        assert_eq!(super::cache_write_allowed(&cache(&theirs), &tokens(None)), Ok(()), "an unknown account is not someone else");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// (c) The connection rule, on an injected clock: a source answered
    /// within the TTL is not asked again; one older, failed or never asked
    /// is — and the ones not asked keep their rows and their report.
    #[test]
    fn a_connection_asks_only_the_sources_older_than_the_ttl() {
        use std::cell::RefCell;
        let ttl = super::LIVE_TTL_SECS;
        let now = 1_788_500_000;
        let report = |source: &str, ok: bool, fetched_at: u64| super::SourceReport {
            provider: "x".to_string(),
            source: source.to_string(),
            ok,
            detail: "1 model(s)".to_string(),
            count: 1,
            fetched_at,
            origin: super::ORIGIN_LIVE.to_string(),
        };
        let row = |source: &str, id: &str| DiscoveredModel {
            provider: "openai".to_string(),
            id: id.to_string(),
            display_name: id.to_string(),
            source: source.to_string(),
            ..Default::default()
        };
        let previous = super::DiscoveredCatalog {
            fetched_at: now - 30 * 60,
            reports: vec![
                report(super::OPENAI_SOURCE, true, now - 30 * 60),
                report(super::ANTHROPIC_SOURCE, true, now - 2 * ttl),
                report(super::GOOGLE_API_SOURCE, false, now - 60),
            ],
            models: vec![row(super::OPENAI_SOURCE, "gpt-6-astra"), row(super::ANTHROPIC_SOURCE, "claude-fable-5-1")],
        };
        assert_eq!(
            super::due_sources(Some(&previous), now, ttl),
            vec![super::ANTHROPIC_SOURCE, super::GOOGLE_API_SOURCE, super::ANTIGRAVITY_SOURCE],
            "older than the TTL, failed, and never asked are due; the young one is not"
        );
        assert_eq!(super::due_sources(None, now, ttl).len(), 4, "no catalog: everything is due");
        assert!(super::due_sources(Some(&previous), now, 0).contains(&super::OPENAI_SOURCE), "a zero TTL asks every time");
        assert!(super::due_sources(Some(&previous), previous.fetched_at + ttl - 1, ttl).iter().all(|source| *source != super::OPENAI_SOURCE));

        let asked = RefCell::new(Vec::new());
        let due = super::due_sources(Some(&previous), now, ttl);
        let fetched = super::refresh_with(Some(&previous), &due, now, &[], |source| {
            asked.borrow_mut().push(source);
            match source {
                super::ANTHROPIC_SOURCE => Ok(super::Answered {
                    source: source.to_string(),
                    models: vec![row(source, "claude-fable-5-2")],
                    origin: super::ORIGIN_LIVE.to_string(),
                    fetched_at: now,
                    note: None,
                }),
                other => Err((other.to_string(), "http error".to_string())),
            }
        });
        assert_eq!(asked.into_inner(), due, "exactly the due sources were asked, in report order");
        let ids: Vec<&str> = fetched.models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["gpt-6-astra", "claude-fable-5-2"], "the young source kept its rows, the old one was replaced");
        let openai = fetched.reports.iter().find(|report| report.source == super::OPENAI_SOURCE).unwrap();
        assert_eq!(openai.fetched_at, now - 30 * 60, "a carried report keeps its own clock");
        assert!(openai.ok);
        let anthropic = fetched.reports.iter().find(|report| report.source == super::ANTHROPIC_SOURCE).unwrap();
        assert_eq!(anthropic.fetched_at, now);
        assert_eq!(fetched.reports.len(), 4);
        assert_eq!(fetched.fetched_at, now);

        // A catalog younger than the TTL on every source asks nothing.
        let young = super::DiscoveredCatalog {
            fetched_at: now - 60,
            reports: super::SOURCES.iter().map(|(_, source)| report(source, true, now - 60)).collect(),
            models: Vec::new(),
        };
        assert!(super::due_sources(Some(&young), now, ttl).is_empty());
    }

    /// (d) A selected model the source omits stays as an unlisted row with
    /// the time it went missing, comes back clean when listed again, and is
    /// dropped only after the grace period of never coming back.
    #[test]
    fn a_selected_model_the_source_omits_stays_unlisted_until_the_grace_ends() {
        let now = 1_788_500_000;
        let mut astra = model("openai", "gpt-6-astra", 0);
        astra.source = super::OPENAI_SOURCE.to_string();
        let mut sol = model("openai", "gpt-5.6-sol", 0);
        sol.source = super::OPENAI_SOURCE.to_string();
        let mut luna = model("openai", "gpt-5.6-luna", 0);
        luna.source = super::OPENAI_SOURCE.to_string();
        let previous = catalog(vec![astra.clone(), sol.clone(), luna.clone()]);
        let selected = vec!["GPT-6-Astra".to_string()];

        // The live list dropped astra and luna; only astra is selected.
        let fresh = catalog(vec![sol.clone()]);
        let kept = super::keep_selected(fresh, Some(&previous), &selected, now);
        let ids: Vec<(&str, Option<u64>)> = kept.models.iter().map(|model| (model.id.as_str(), model.unlisted_since)).collect();
        assert_eq!(ids, vec![("gpt-5.6-sol", None), ("gpt-6-astra", Some(now))], "the selected row stays, stamped; the unselected one is gone");

        // A day later, still missing: the stamp is the first miss, not the latest.
        let later = super::keep_selected(catalog(vec![sol.clone()]), Some(&kept), &[], now + 86_400);
        let astra_row = later.models.iter().find(|model| model.id == "gpt-6-astra").expect("still there, even unselected now");
        assert_eq!(astra_row.unlisted_since, Some(now));

        // Listed again: the fresh row is clean.
        let back = super::keep_selected(catalog(vec![sol.clone(), astra.clone()]), Some(&later), &selected, now + 2 * 86_400);
        let astra_row = back.models.iter().find(|model| model.id == "gpt-6-astra").unwrap();
        assert_eq!(astra_row.unlisted_since, None);
        assert_eq!(back.models.len(), 2);

        // Never back for the whole grace: dropped.
        let expiring = super::keep_selected(catalog(vec![sol.clone()]), Some(&kept), &selected, now + super::UNLISTED_GRACE_SECS - 1);
        assert!(expiring.models.iter().any(|model| model.id == "gpt-6-astra"), "one second inside the grace it stays");
        let expired = super::keep_selected(catalog(vec![sol.clone()]), Some(&kept), &selected, now + super::UNLISTED_GRACE_SECS);
        assert!(!expired.models.iter().any(|model| model.id == "gpt-6-astra"), "at the grace it goes");

        // The unlisted row still reaches the overlay — the picker shows it, the wire decides.
        let value = overlay_value(&kept, UpdatePolicy::Auto);
        assert!(value["models"].as_array().unwrap().iter().any(|row| row["ids"][0] == "gpt-6-astra"));
        assert_eq!(kept.models[1].source, super::OPENAI_SOURCE);
        // The stamp survives the cache file.
        let dir = scratch("unlisted");
        let path = dir.join("discovered.json");
        super::save_to(&path, &kept).unwrap();
        assert_eq!(super::load_cached_from(&path), Some(kept));
        let _ = std::fs::remove_dir_all(dir);

        // The selected registry: latest first, de-duplicated, capped.
        super::note_selected(" gpt-6-astra ");
        super::note_selected("GPT-6-ASTRA");
        let selected = super::selected_models();
        assert_eq!(selected.iter().filter(|id| id.eq_ignore_ascii_case("gpt-6-astra")).count(), 1);
        assert!(selected.iter().any(|id| id == "GPT-6-ASTRA"));
    }

    /// A keyless source answers "skipped" and that answer keeps for the TTL
    /// like any other; a source that FAILED is asked again at the very next
    /// connection; one never asked, or asked longer than the TTL ago, is due
    /// whatever it said. (2026-09-08: `google-api` skipped for want of a key
    /// was due on every publish, and every publish spawned a refresh.)
    #[test]
    fn a_skipped_source_is_due_at_the_ttl_and_a_failed_one_at_every_connection() {
        let report = |ok: bool, detail: &str, fetched_at: u64| super::SourceReport {
            provider: "google".to_string(),
            source: "google-api".to_string(),
            ok,
            detail: detail.to_string(),
            count: 0,
            fetched_at,
            origin: String::new(),
        };
        let ttl = 3_600;
        let skipped = report(false, "skipped: no Google API key", 10_000);
        assert!(skipped.skipped());
        assert!(!skipped.due(10_000 + ttl - 1, ttl), "a fresh skip was asked again");
        assert!(skipped.due(10_000 + ttl, ttl), "a skip older than the TTL stands");
        let failed = report(false, "http error: error sending request", 10_000);
        assert!(!failed.skipped());
        assert!(failed.due(10_001, ttl), "a failure waits for the TTL");
        let answered = report(true, "5 model(s)", 10_000);
        assert!(!answered.due(10_000 + ttl - 1, ttl));
        assert!(answered.due(10_000 + ttl, ttl));
        assert!(report(true, "5 model(s)", 0).due(1, ttl), "never answered is due");
    }

    /// One failed registry request must not empty the Gemini column for a
    /// whole TTL (2026-09-04, "제미니 모델이 사라짐"). A source that failed
    /// keeps last time's rows and says so; a source that was skipped on
    /// purpose keeps nothing; a source that answered replaces its rows.
    #[test]
    fn a_failed_source_keeps_last_refreshs_rows_and_a_skipped_one_keeps_none() {
        let mut registry = model("google", "gemini-3.8-flash", 20_260_901);
        registry.source = "antigravity-registry".to_string();
        let mut keyed = model("google", "gemini-3.1-pro-preview", 20_260_701);
        keyed.source = "google-api".to_string();
        keyed.api_key_only = true;
        let mut old_openai = model("openai", "gpt-5.6-sol", 20_260_801);
        old_openai.source = "codex-cache".to_string();
        let previous = catalog(vec![registry.clone(), keyed.clone(), old_openai]);

        let mut new_openai = model("openai", "gpt-5.7-sol", 20_260_902);
        new_openai.source = "codex-cache".to_string();
        let report = |source: &str, ok: bool, detail: &str, count: usize| super::SourceReport {
            provider: if source == "codex-cache" { "openai" } else { "google" }.to_string(),
            source: source.to_string(),
            ok,
            detail: detail.to_string(),
            count,
            fetched_at: 0,
            origin: String::new(),
        };
        let fresh = super::DiscoveredCatalog {
            fetched_at: previous.fetched_at + 60,
            reports: vec![
                report("codex-cache", true, "1 model(s)", 1),
                report("google-api", false, "skipped: no Google API key", 0),
                report("antigravity-registry", false, "http error: error sending request", 0),
            ],
            models: vec![new_openai.clone()],
        };

        let merged = super::carry_forward(fresh, Some(&previous));
        let ids: Vec<&str> = merged.models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["gpt-5.7-sol", "gemini-3.8-flash"],
            "the failed registry keeps its row, the skipped key list keeps none, \
             and the answered source replaced its own"
        );
        let registry_report = merged
            .reports
            .iter()
            .find(|report| report.source == "antigravity-registry")
            .expect("the registry report");
        assert!(!registry_report.ok, "a carried row is not a success");
        assert_eq!(registry_report.count, 1);
        assert!(
            registry_report.detail.ends_with("kept 1 model(s) from the previous refresh"),
            "{}",
            registry_report.detail
        );
        let keyed_report = merged
            .reports
            .iter()
            .find(|report| report.source == "google-api")
            .expect("the key-list report");
        assert_eq!(keyed_report.count, 0);
        assert_eq!(keyed_report.detail, "skipped: no Google API key");

        // Without a previous catalog nothing changes.
        let alone = super::carry_forward(
            super::DiscoveredCatalog {
                fetched_at: 1,
                reports: vec![report("antigravity-registry", false, "http error", 0)],
                models: Vec::new(),
            },
            None,
        );
        assert!(alone.models.is_empty());
    }

    #[test]
    fn antigravity_rows_fold_tiered_ids_into_one_selection_id_with_its_wire_map() {
        let document = json!({"models": {
            "gemini-3.7-flash-tiered": {"maxTokens": 1_048_576, "maxOutputTokens": 65535, "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.6-flash-high": {"displayName": "Gemini 3.6 Flash (High)", "maxTokens": 1_048_576, "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.6-flash-low": {"displayName": "Gemini 3.6 Flash (Low)", "maxTokens": 1_048_576, "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.6-flash-medium": {"displayName": "Gemini 3.6 Flash (Medium)", "maxTokens": 1_048_576, "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.6-flash-tiered": {"maxTokens": 1_048_576, "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.5-flash-extra-low": {"displayName": "Gemini 3.5 Flash (Low)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3.5-flash-low": {"displayName": "Gemini 3.5 Flash (Medium)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-3-flash-agent": {"displayName": "Gemini 3.5 Flash (High)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-pro-agent": {"displayName": "Gemini 3.1 Pro (High)", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "gemini-2.5-flash": {"displayName": "Gemini 3.1 Flash Lite", "modelProvider": "MODEL_PROVIDER_GOOGLE"},
            "claude-sonnet-4-6": {"displayName": "Claude Sonnet 4.6 (Thinking)", "modelProvider": "MODEL_PROVIDER_ANTHROPIC"},
            "chat_23310": {"modelProvider": "MODEL_PROVIDER_INTERNAL"}
        }});
        let rows = antigravity_rows(&document, "antigravity-registry");
        let by_id: HashMap<&str, &DiscoveredModel> = rows.iter().map(|row| (row.id.as_str(), row)).collect();
        let mut ids: Vec<&str> = by_id.keys().copied().collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["gemini-3.5-flash", "gemini-3.6-flash", "gemini-3.7-flash"]);

        let flash37 = by_id["gemini-3.7-flash"];
        assert_eq!(flash37.wire, Some(json!("gemini-3.7-flash-tiered")), "only a -tiered id: fixed for every effort");
        assert_eq!(flash37.display_name, "Gemini 3.7 Flash", "no display name in the registry: spelled from the id");
        assert_eq!(flash37.context_window, Some(1_048_576));
        assert_eq!(flash37.max_output_tokens, Some(65_535));
        assert!(!flash37.api_key_only, "the login reaches it");

        let flash36 = by_id["gemini-3.6-flash"];
        assert_eq!(
            flash36.wire,
            Some(json!({"low": "gemini-3.6-flash-low", "medium": "gemini-3.6-flash-medium", "high": "gemini-3.6-flash-high"})),
            "explicit tiers win over the -tiered id"
        );
        assert_eq!(flash36.display_name, "Gemini 3.6 Flash");

        let flash35 = by_id["gemini-3.5-flash"];
        assert_eq!(
            flash35.wire,
            Some(json!({"low": "gemini-3.5-flash-extra-low", "medium": "gemini-3.5-flash-low"})),
            "extra-low is the low rung and low the medium; no high is invented"
        );
    }

    #[test]
    fn a_registry_release_reaches_the_overlay_with_its_wire_map_and_moves_the_flash_alias() {
        let catalog = DiscoveredCatalog {
            fetched_at: 1,
            models: vec![DiscoveredModel {
                provider: "google".to_string(),
                id: "gemini-3.7-flash".to_string(),
                display_name: "Gemini 3.7 Flash".to_string(),
                context_window: Some(1_048_576),
                source: "antigravity-registry".to_string(),
                wire: Some(json!("gemini-3.7-flash-tiered")),
                ..Default::default()
            }],
            ..Default::default()
        };
        let overlay = overlay(&catalog, UpdatePolicy::Auto);
        let value: Value = serde_json::from_str(overlay.json.as_deref().expect("an overlay")).unwrap();
        let row = &value["models"][0];
        assert_eq!(row["ids"], json!(["gemini-3.7-flash"]));
        assert_eq!(row["wire"], json!("gemini-3.7-flash-tiered"), "the wire map rides into the catalog row");
        assert_eq!(row["context_window"], 1_048_576);
        let aliases = value["aliases"].as_array().unwrap();
        assert!(aliases.iter().any(|alias| alias["alias"] == "gemini-flash" && alias["canonical"] == "gemini-3.7-flash"));
    }

    #[test]
    fn the_registry_row_outlives_the_api_key_row_for_the_same_release() {
        let mut models = vec![
            DiscoveredModel {
                provider: "google".to_string(),
                id: "gemini-3.7-flash".to_string(),
                source: "google-api".to_string(),
                api_key_only: true,
                ..Default::default()
            },
            DiscoveredModel {
                provider: "google".to_string(),
                id: "Gemini-3.7-Flash".to_string(),
                source: "antigravity-registry".to_string(),
                ..Default::default()
            },
            DiscoveredModel {
                provider: "google".to_string(),
                id: "gemini-3.7-pro".to_string(),
                source: "google-api".to_string(),
                api_key_only: true,
                ..Default::default()
            },
        ];
        drop_api_key_duplicates(&mut models);
        let sources: Vec<&str> = models.iter().map(|model| model.source.as_str()).collect();
        assert_eq!(sources, vec!["antigravity-registry", "google-api"]);
    }
}
