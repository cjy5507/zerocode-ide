//! Ownership of the `ZO_MODEL_CONTEXT_WINDOWS` process bridge for
//! settings-declared and discovered model rows.
//!
//! The `api` crate holds the model catalog — which id a selection is actually
//! served under — but cannot depend on runtime config, so a model declared in
//! `settings.json` or learned from a provider reaches it through this one
//! environment variable. It is the same bridge shape as
//! [`crate::custom_provider_env`], and it exists for the same reason: a model
//! the provider ships after this binary was built must be usable without a
//! rebuild.
//!
//! An operator may export the variable themselves. That export is a decision,
//! so it is captured once and always kept — and kept FIRST, because the catalog
//! resolves to the first entry that names a model, which makes leading position
//! precedence. The overlay (`model-catalog.json`, or the legacy settings key)
//! is layered after it, and discovered entries after that, each contributing
//! only what the layers ahead did not declare. `zo models --refresh` leaves the
//! whole picture — every layer in that order, the shipped seed last — as
//! `cache/model-catalog/merged.json` (`audit_document`).
//!
//! A zo started by another zo — a helper, or a `zo` run from a session's shell
//! tool — inherits the variable its parent published, and that is not an
//! operator's decision: it is what the parent knew when it last published. A
//! child that took it for one kept the parent's start-of-session answers
//! ahead of its own discovery, blind to every model newer than the parent
//! (t-17403: `sol` stayed `gpt-6-sol` after `gpt-6.1-sol` was discovered). So
//! every publish marks what it made — the value's fingerprint, and the
//! operator's own export, if there was one, in a companion variable each
//! (`Bridge`) — and a process that finds a value its mark fits takes it as the
//! LAST layer above the shipped seed, a place to start from, and the
//! operator's export from its companion variable as the first. A value with no
//! mark, or one whose mark no longer fits (an export made afresh in a
//! session's shell), is an operator's export, as ever.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

const OPERATOR_EXPORT_LAYER: &str = "operator export";
const OVERLAY_LAYER: &str = "model catalog overlay";
const DISCOVERED_LAYER: &str = "discovered model catalog";
const PARENT_LAYER: &str = "parent zo's published catalog";
const SHIPPED_LAYER: &str = "shipped";

/// One process bridge: the variable `api` reads, and the two that travel with
/// it so a child can tell a parent's snapshot from an operator's decision.
struct Bridge {
    /// The variable `api` reads.
    variable: &'static str,
    /// The fingerprint of what zo last published into `variable`. A value it
    /// fits is zo's; anything else there is somebody's export.
    published: &'static str,
    /// The operator's own export of `variable`, verbatim, as the publishing
    /// process first found it — present only when there was one.
    operator: &'static str,
}

const CONTEXT_WINDOWS: Bridge = Bridge {
    variable: api::MODEL_CONTEXT_WINDOWS_ENV,
    published: "ZO_MODEL_CONTEXT_WINDOWS_PUBLISHED",
    operator: "ZO_MODEL_CONTEXT_WINDOWS_OPERATOR",
};
const EFFORT_CEILINGS: Bridge = Bridge {
    variable: api::MODEL_EFFORT_CEILINGS_ENV,
    published: "ZO_MODEL_EFFORT_CEILINGS_PUBLISHED",
    operator: "ZO_MODEL_EFFORT_CEILINGS_OPERATOR",
};
/// Beside the discovery cache: the merged copy a refresh leaves for audit.
const AUDIT_FILE: &str = "merged.json";

/// What one publish made live: each layer in precedence order, and the merged
/// document the variable now carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub layers: Vec<(&'static str, String)>,
    pub json: String,
}

/// What the environment held for one bridge before zo first wrote it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Inherited {
    /// An operator's own export: a decision, kept first.
    operator: Option<String>,
    /// What a parent zo published: a place to start from, kept last.
    snapshot: Option<String>,
}

/// [`Inherited`], or "not looked yet".
///
/// "Not looked yet" has to stay distinguishable from "looked, and there was
/// nothing" (`Inherited::default()`), because the answer is captured ONCE and
/// reused. `/model` rebuilds the runtime and republishes, and re-reading the
/// variable each time would fold zo's own previous output back into the
/// operator half and grow it without bound.
enum Captured {
    Unread,
    Read(Inherited),
}

static PUBLISHED: Mutex<Option<Published>> = Mutex::new(None);
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

fn record_published(published: Option<Published>) -> Option<Published> {
    let mut held = PUBLISHED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if *held != published {
        held.clone_from(&published);
        GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    published
}

/// Aggregate evidence for the selected row only. Free-form source notes are
/// never copied into diagnostics; a layered/family match is explicitly mixed.
pub(crate) fn selection_provenance(model: &str) -> (&'static str, &'static str) {
    let held = PUBLISHED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let layers = held.as_ref().map_or(&[][..], |p| p.layers.as_slice());
    for (source, raw) in layers.iter().map(|(s, r)| (*s, r.as_str()))
        .chain(std::iter::once((SHIPPED_LAYER, api::builtin_model_catalog_json()))) {
        let Ok((rows, _)) = rows_of(source, raw) else { continue; };
        if let Some(row) = rows.iter().find(|row| row.get("ids").and_then(Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str().is_some_and(|id| id.eq_ignore_ascii_case(model))))) {
            let source = match source {
                OPERATOR_EXPORT_LAYER => "export", OVERLAY_LAYER => "overlay",
                DISCOVERED_LAYER => "discovered", PARENT_LAYER => "inherited", _ => "shipped",
            };
            let provenance = if matches!(source, "discovered" | "inherited") || row.get("effort_levels").is_none() {
                "mixed"
            } else { "declared" };
            return (source, provenance);
        }
    }
    ("unknown", "unknown")
}

static OPERATOR_BASE: Mutex<Captured> = Mutex::new(Captured::Unread);
static EFFORT_BASE: Mutex<Captured> = Mutex::new(Captured::Unread);

/// What was in the environment for `bridge` the first time anyone asked; the
/// same answer every time after.
fn captured(slot: &Mutex<Captured>, bridge: &Bridge) -> Inherited {
    let mut slot = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if matches!(*slot, Captured::Unread) {
        *slot = Captured::Read(read_inherited(bridge));
    }
    match &*slot {
        Captured::Read(inherited) => inherited.clone(),
        Captured::Unread => Inherited::default(),
    }
}

/// Sort what a bridge variable holds into a decision or a snapshot: a value
/// its fingerprint fits is what a zo published — its operator half is in the
/// companion variable — and any other value is an operator's export.
fn read_inherited(bridge: &Bridge) -> Inherited {
    let text = |key: &str| std::env::var(key).ok().filter(|value| !value.trim().is_empty());
    let Some(value) = text(bridge.variable) else {
        return Inherited::default();
    };
    if text(bridge.published).is_some_and(|mark| mark == fingerprint(&value)) {
        Inherited { operator: text(bridge.operator), snapshot: Some(value) }
    } else {
        Inherited { operator: Some(value), snapshot: None }
    }
}

/// A fingerprint that means the same in every zo that ever ran: FNV-1a over
/// the bytes, and their number. It is not a secret and guards nothing but a
/// mix-up — a value someone exported afresh under a mark a session left.
fn fingerprint(value: &str) -> String {
    let hash = value
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3));
    format!("{hash:016x}-{}", value.len())
}

/// Put a bridge's value in the environment. A value zo made (`authored`) goes
/// with its fingerprint and the operator's export, so a process that inherits
/// it can tell it from a decision; the operator's own export, restored as it
/// was, is exactly what it was and carries neither.
fn set_bridge(bridge: &Bridge, value: &str, operator: Option<&str>, authored: bool) {
    std::env::set_var(bridge.variable, value);
    if authored {
        std::env::set_var(bridge.published, fingerprint(value));
        match operator {
            Some(export) => std::env::set_var(bridge.operator, export),
            None => std::env::remove_var(bridge.operator),
        }
    } else {
        std::env::remove_var(bridge.published);
        std::env::remove_var(bridge.operator);
    }
}

/// Forget the captured environment so a test can play a fresh process.
#[cfg(test)]
pub(crate) fn reset_ownership_for_tests() {
    if let Ok(mut slot) = OPERATOR_BASE.lock() {
        *slot = Captured::Unread;
    }
    if let Ok(mut slot) = EFFORT_BASE.lock() {
        *slot = Captured::Unread;
    }
    if let Ok(mut reported) = REPORTED_DIFFERENCES.lock() {
        reported.clear();
    }
}

/// Make the overlay-declared and discovered catalogs live.
///
/// `None` for both means nothing to add, which is NOT the same as an empty
/// catalog: with no operator export and nothing inherited from a parent zo
/// there is simply nothing to publish, and the variable is left untouched
/// (`Ok(None)`) rather than being set to an empty catalog that would read as a
/// deliberate "no models are declared".
pub(crate) fn publish(
    overlay_json: Option<&str>,
    discovered_json: Option<&str>,
) -> Result<Option<Published>, String> {
    let Inherited { operator: base, snapshot } = captured(&OPERATOR_BASE, &CONTEXT_WINDOWS);
    // A parent's snapshot goes last: under everything this process declares or
    // discovers, over only the shipped seed.
    let own: Vec<(&'static str, &str)> = [
        (OVERLAY_LAYER, overlay_json),
        (DISCOVERED_LAYER, discovered_json),
        (PARENT_LAYER, snapshot.as_deref()),
    ]
    .into_iter()
    .filter_map(|(label, json)| json.map(|json| (label, json)))
    .collect();
    let json = match (base.as_deref(), own.is_empty()) {
        (None, true) => return Ok(record_published(None)),
        // Nothing of zo's to add, but a previous publish may have appended
        // entries the overlay no longer declares — restore the export.
        (Some(base), true) => base.to_string(),
        (base, false) => merge(base, &own)?,
    };
    set_bridge(&CONTEXT_WINDOWS, &json, base.as_deref(), !own.is_empty());
    if let Some(snapshot) = snapshot.as_deref() {
        // Everything ranked ahead of the parent's snapshot, in order.
        let ahead: Vec<(&str, &str)> = base
            .as_deref()
            .map(|export| (OPERATOR_EXPORT_LAYER, export))
            .into_iter()
            .chain(own.iter().copied().filter(|(label, _)| *label != PARENT_LAYER))
            .collect();
        announce_differences(&ahead, snapshot);
    }
    // The wire lookup re-reads the variable per call, but the alias registry is
    // built once and cached, so it has to be told. Idempotent on identical
    // bytes, which is what every rebuild-triggered republish sends.
    api::refresh_model_registry_from_json(&json);
    let layers = base
        .map(|base| (OPERATOR_EXPORT_LAYER, base))
        .into_iter()
        .chain(own.into_iter().map(|(label, json)| (label, json.to_string())))
        .collect();
    Ok(record_published(Some(Published { layers, json })))
}

/// The aliases this process resolves differently from the parent zo it
/// inherited a snapshot from — `(alias, the parent's answer, this process's)`.
/// `ahead` is every layer ranked above the snapshot; the first row of an alias
/// in them answers, as it does for the catalog.
fn differing_answers(ahead: &[(&str, &str)], snapshot: &str) -> Vec<(String, String, String)> {
    let answers = |label: &str, raw: &str| -> Vec<(String, String)> {
        let Ok((_, aliases)) = rows_of(label, raw) else {
            return Vec::new();
        };
        aliases
            .iter()
            .filter_map(|row| {
                let name = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_string);
                Some((name("alias")?, name("canonical")?))
            })
            .collect()
    };
    let mine: Vec<(String, String)> = ahead.iter().flat_map(|(label, raw)| answers(label, raw)).collect();
    let mut seen = std::collections::HashSet::new();
    answers(PARENT_LAYER, snapshot)
        .into_iter()
        .filter(|(alias, _)| seen.insert(alias.to_ascii_lowercase()))
        .filter_map(|(alias, parents)| {
            let (_, answer) = mine.iter().find(|(name, _)| name.eq_ignore_ascii_case(&alias))?;
            (!answer.eq_ignore_ascii_case(&parents)).then(|| (alias, parents, answer.clone()))
        })
        .collect()
}

/// Names already reported in this process, lower-cased.
static REPORTED_DIFFERENCES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Say, once per alias and on the log (stderr), what this process resolves
/// differently from the parent zo that started it — the alias, the parent's
/// answer, this process's. A child that knows a newer model than its parent's
/// snapshot answers with the newer one; that is the point, and it is written
/// down so a session and its helper naming one model differently is a line to
/// read, not a mystery. Returns the lines it wrote.
fn announce_differences(ahead: &[(&str, &str)], snapshot: &str) -> Vec<String> {
    let mut reported = REPORTED_DIFFERENCES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let lines: Vec<String> = differing_answers(ahead, snapshot)
        .into_iter()
        .filter(|(alias, _, _)| {
            let key = alias.to_ascii_lowercase();
            let new = !reported.contains(&key);
            if new {
                reported.push(key);
            }
            new
        })
        .map(|(alias, parents, mine)| {
            format!("model catalog: {alias} resolves to {mine} here; the zo that started this one had it as {parents}")
        })
        .collect();
    for line in &lines {
        eprintln!("[zo] {line}");
    }
    lines
}

/// `<config home>/cache/model-catalog/merged.json`.
pub(crate) fn audit_path() -> PathBuf {
    runtime::model_discovery::cache_dir().join(AUDIT_FILE)
}

/// The effective catalog as one document: every published layer in the order
/// the registry consults them, the shipped seed last, and the merged bytes the
/// variable carries. The first layer that names a model or alias is the one
/// that answers — so "why does `sol` resolve there" is a read of this file.
pub(crate) fn audit_document(published: Option<&Published>, now: u64) -> Result<Value, String> {
    let mut layers = published
        .map_or(&[][..], |published| published.layers.as_slice())
        .iter()
        .map(|(source, raw)| layer_document(source, raw))
        .collect::<Result<Vec<_>, _>>()?;
    layers.push(layer_document(SHIPPED_LAYER, api::builtin_model_catalog_json())?);
    let merged = published
        .map(|published| serde_json::from_str::<Value>(&published.json))
        .transpose()
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "writtenAt": now,
        "variable": api::MODEL_CONTEXT_WINDOWS_ENV,
        "layers": layers,
        "published": merged,
    }))
}

fn layer_document(source: &str, raw: &str) -> Result<Value, String> {
    let (models, aliases) = rows_of(source, raw)?;
    let mut document = json!({ "source": source, "models": models, "aliases": aliases });
    if let Some(priors) = priors_of(source, raw)? {
        document["priors"] = serde_json::to_value(priors).map_err(|error| error.to_string())?;
    }
    Ok(document)
}

pub(crate) fn write_audit(path: &Path, document: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let rendered = serde_json::to_string_pretty(document).map_err(io::Error::other)?;
    runtime::file_ops::replace_file_atomic(path, format!("{rendered}\n").as_bytes())
}

/// Make discovered effort ceilings live through `api::MODEL_EFFORT_CEILINGS_ENV`.
///
/// An operator export keeps every id it names; discovered ceilings fill in the
/// rest, over what a parent zo published (which fills in only what nothing
/// else names). `None` with nothing inherited leaves the variable untouched.
pub(crate) fn publish_effort_ceilings(discovered_json: Option<&str>) -> Result<(), String> {
    let Inherited { operator: base, snapshot } = captured(&EFFORT_BASE, &EFFORT_CEILINGS);
    match (base.as_deref(), discovered_json, snapshot.as_deref()) {
        // Nothing of ours, and a parent's snapshot is already what stands.
        (None, None, _) => Ok(()),
        (Some(export), None, None) => {
            set_bridge(&EFFORT_CEILINGS, export, None, false);
            Ok(())
        }
        (None, Some(discovered), None) => {
            object_of("discovered effort ceilings", discovered)?;
            set_bridge(&EFFORT_CEILINGS, discovered, None, true);
            Ok(())
        }
        (base, discovered, snapshot) => {
            // Lowest first, so a later layer's entry replaces an earlier one's.
            let mut merged = Map::new();
            for (label, raw) in [
                ("parent zo's effort ceilings", snapshot),
                ("discovered effort ceilings", discovered),
                (api::MODEL_EFFORT_CEILINGS_ENV, base),
            ] {
                if let Some(raw) = raw {
                    merged.extend(object_of(label, raw)?);
                }
            }
            let merged = serde_json::to_string(&Value::Object(merged)).map_err(|error| error.to_string())?;
            set_bridge(&EFFORT_CEILINGS, &merged, base, true);
            Ok(())
        }
    }
}

fn object_of(label: &str, raw: &str) -> Result<Map<String, Value>, String> {
    serde_json::from_str::<Value>(raw)
        .map_err(|error| format!("{label}: {error}"))?
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{label}: must be a JSON object"))
}

/// Layer the given catalogs after the operator's, which keeps the export's
/// entries ahead of them and therefore authoritative for every id it names.
/// Both the `models` and the `aliases` arrays are carried: an alias row is how
/// a new release becomes a family's default, so dropping it would leave the
/// model reachable only by its full id.
fn merge(base: Option<&str>, layers: &[(&str, &str)]) -> Result<String, String> {
    let mut models = Vec::new();
    let mut aliases = Vec::new();
    let mut priors = api::RouterPriors::default();
    let base = base.map(|base| (api::MODEL_CONTEXT_WINDOWS_ENV, base));
    for (label, raw) in base.iter().chain(layers) {
        let (layer_models, layer_aliases) = rows_of(label, raw)?;
        models.extend(layer_models);
        aliases.extend(layer_aliases);
        // Priors layer per field, first filled wins — the same precedence
        // the rows get from their order.
        if let Some(layer_priors) = priors_of(label, raw)? {
            priors.fill_from(&layer_priors);
        }
    }
    let mut root = Map::new();
    root.insert("models".to_string(), Value::Array(models));
    root.insert("aliases".to_string(), Value::Array(aliases));
    if priors.declares_anything() {
        root.insert(
            "priors".to_string(),
            serde_json::to_value(&priors).map_err(|error| error.to_string())?,
        );
    }
    serde_json::to_string(&Value::Object(root)).map_err(|error| error.to_string())
}

/// A layer's `priors` section, when it declares one.
fn priors_of(label: &str, raw: &str) -> Result<Option<api::RouterPriors>, String> {
    let parsed: Value = serde_json::from_str(raw).map_err(|error| format!("{label}: {error}"))?;
    match parsed.get("priors") {
        None | Some(Value::Null) => Ok(None),
        Some(section) => serde_json::from_value::<api::RouterPriors>(section.clone())
            .map(|priors| priors.declares_anything().then_some(priors))
            .map_err(|error| format!("{label}: \"priors\": {error}")),
    }
}

fn rows_of(label: &str, raw: &str) -> Result<(Vec<Value>, Vec<Value>), String> {
    let parsed: Value = serde_json::from_str(raw).map_err(|error| format!("{label}: {error}"))?;
    let array = |key: &str| -> Result<Vec<Value>, String> {
        match parsed.get(key) {
            Some(Value::Array(rows)) => Ok(rows.clone()),
            // A document with no such key is not an error the user can act on
            // here — the catalog reader ignores it too — so carry it as empty
            // rather than refusing to publish anything at all.
            Some(_) => Err(format!("{label}: \"{key}\" must be a JSON array")),
            None => Ok(Vec::new()),
        }
    };
    Ok((array("models")?, array("aliases")?))
}

#[cfg(test)]
mod tests {
    use super::{
        audit_document, merge, publish, publish_effort_ceilings, reset_ownership_for_tests,
        write_audit, DISCOVERED_LAYER, OVERLAY_LAYER,
    };

    const OVERLAY: &str = r#"{"models":[{"provider":"google","ids":["gemini-3.7-flash"],"wire":{"low":"gemini-3.7-flash-low","high":"gemini-3.7-flash-high"}}]}"#;

    /// The whole point of the bridge: a model declared only in settings is
    /// served under the id those settings name, with no rebuild.
    #[test]
    fn a_settings_declared_model_reaches_the_catalog() {
        let _lock = crate::test_env_lock();
        let _env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        reset_ownership_for_tests();

        publish(Some(OVERLAY), None).expect("publish");
        assert_eq!(
            api::wire_model_for_effort("gemini-3.7-flash", api::EffortLevel::Low).as_deref(),
            Some("gemini-3.7-flash-low")
        );
        // The absent middle rung resolves up rather than silently serving less
        // reasoning than the caller asked for.
        assert_eq!(
            api::wire_model_for_effort("gemini-3.7-flash", api::EffortLevel::Medium).as_deref(),
            Some("gemini-3.7-flash-high")
        );
    }

    /// The other half of the bridge: a settings-declared alias repoints a short
    /// name at a model the binary predates, which is how a new release becomes
    /// the default without a rebuild.
    #[test]
    fn a_settings_declared_alias_reaches_the_registry() {
        let _lock = crate::test_env_lock();
        let _env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        reset_ownership_for_tests();

        publish(
            Some(r#"{"models":[],"aliases":[{"alias":"google-latest","canonical":"gemini-3.7-flash","provider":"google"}]}"#),
            None,
        )
        .expect("publish");
        assert_eq!(api::resolve_catalog_alias("google-latest"), "gemini-3.7-flash");

        // Leave the process-global registry as we found it for other tests.
        publish(Some(r#"{"models":[],"aliases":[]}"#), None).expect("restore");
        assert_eq!(api::resolve_catalog_alias("google-latest"), "gemini-3.6-flash");
    }

    /// Discovery is the third layer: its alias rows re-point a family the
    /// settings say nothing about, and a settings row for the same alias wins.
    #[test]
    fn a_discovered_alias_follows_settings_and_precedes_the_shipped_row() {
        let _lock = crate::test_env_lock();
        let _env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        reset_ownership_for_tests();

        let discovered = r#"{"models":[{"provider":"anthropic","ids":["claude-fable-5-2"],"context_window":1000000,"class":"frontier"}],"aliases":[{"alias":"claude-fable-5-2","canonical":"claude-fable-5-2","provider":"anthropic"},{"alias":"fable","canonical":"claude-fable-5-2","provider":"anthropic","orchestration_rank":0},{"alias":"google-latest","canonical":"gemini-3.7-flash","provider":"google"}]}"#;
        publish(None, Some(discovered)).expect("publish discovered");
        assert_eq!(api::resolve_catalog_alias("fable"), "claude-fable-5-2");
        assert_eq!(api::context_window_for_model("claude-fable-5-2"), 1_000_000);
        assert_eq!(api::declared_model_class("claude-fable-5-2"), Some(api::ModelClass::Frontier));

        publish(
            Some(r#"{"models":[],"aliases":[{"alias":"google-latest","canonical":"gemini-3.8-flash","provider":"google"}]}"#),
            Some(discovered),
        )
        .expect("publish both");
        assert_eq!(api::resolve_catalog_alias("google-latest"), "gemini-3.8-flash", "settings outrank discovery");
        assert_eq!(api::resolve_catalog_alias("fable"), "claude-fable-5-2", "discovery still fills the rest");

        publish(Some(r#"{"models":[],"aliases":[]}"#), None).expect("restore");
        assert_eq!(api::resolve_catalog_alias("fable"), "claude-fable-5-1");
    }

    /// A family the shipped catalog never named gets its aliases minted from
    /// the id grammar (t-2506). The catalog is republished on every runtime
    /// rebuild and after every background refresh, and by then the minted
    /// alias is a registered OpenAI name — the second overlay must still mint
    /// it, byte for byte, or the republish erases what the first one made.
    #[test]
    fn a_minted_alias_survives_its_own_publish() {
        use runtime::model_discovery::{overlay, DiscoveredCatalog, DiscoveredModel, UpdatePolicy};

        let _lock = crate::test_env_lock();
        let _env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        reset_ownership_for_tests();

        let catalog = DiscoveredCatalog {
            fetched_at: 1_700_000_000,
            reports: Vec::new(),
            models: vec![DiscoveredModel {
                provider: "openai".to_string(),
                id: "gpt-6-astra".to_string(),
                display_name: "GPT-6-Astra".to_string(),
                source: "test".to_string(),
                ..Default::default()
            }],
            withdrawn: Vec::new(),
        };
        let minted = |overlay: &runtime::model_discovery::Overlay| -> Vec<String> {
            overlay
                .alias_updates
                .iter()
                .filter(|update| update.from.is_empty())
                .map(|update| update.alias.clone())
                .collect()
        };

        let first = overlay(&catalog, UpdatePolicy::Auto);
        assert_eq!(minted(&first), ["astra", "gpt-6"]);
        publish(None, first.json.as_deref()).expect("publish discovered");
        assert_eq!(api::resolve_catalog_alias("astra"), "gpt-6-astra");
        assert!(api::is_openai_model("astra"), "the minted alias is a registered OpenAI name now");

        let second = overlay(&catalog, UpdatePolicy::Auto);
        assert_eq!(minted(&second), minted(&first));
        assert_eq!(second.json, first.json, "a republish sends the same bytes");

        publish(Some(r#"{"models":[],"aliases":[]}"#), None).expect("restore");
        assert_eq!(api::resolve_catalog_alias("astra"), "astra");
    }

    /// Republishing must track settings: a `/model` edit that drops a model
    /// cannot leave the boot-time snapshot serving it.
    #[test]
    fn republishing_tracks_settings() {
        let _lock = crate::test_env_lock();
        let _env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        reset_ownership_for_tests();

        publish(Some(OVERLAY), None).expect("publish");
        publish(Some(r#"{"models":[]}"#), None).expect("republish");
        assert!(
            api::wire_model_for_effort("gemini-3.7-flash", api::EffortLevel::Low).is_none(),
            "a removed declaration must not wait for a restart"
        );
    }

    /// An operator export stays authoritative for every id it names, and
    /// settings contribute the rest.
    #[test]
    fn an_operator_export_wins_and_settings_fill_in() {
        let _lock = crate::test_env_lock();
        let export = r#"{"models":[{"provider":"google","ids":["gemini-3.7-flash"],"wire":"pinned-by-operator"}]}"#;
        let _env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, Some(export));
        reset_ownership_for_tests();

        publish(
            Some(r#"{"models":[{"provider":"google","ids":["gemini-3.7-flash"],"wire":"from-settings"},{"provider":"google","ids":["gemini-4-flash"],"wire":"gemini-4-flash-low"}]}"#),
            None,
        )
        .expect("publish");

        assert_eq!(
            api::wire_model_for_effort("gemini-3.7-flash", api::EffortLevel::Low).as_deref(),
            Some("pinned-by-operator"),
            "the export decides the ids it names"
        );
        assert_eq!(
            api::wire_model_for_effort("gemini-4-flash", api::EffortLevel::Low).as_deref(),
            Some("gemini-4-flash-low"),
            "settings still contribute ids the export does not name"
        );
    }

    /// Repeated publishes must not fold zo's own output back into the operator
    /// half — the runtime is rebuilt many times per session.
    #[test]
    fn repeated_publishes_do_not_grow_the_catalog() {
        let _lock = crate::test_env_lock();
        let export = r#"{"models":[{"provider":"google","ids":["a"],"wire":"a-wire"}]}"#;
        let _env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, Some(export));
        reset_ownership_for_tests();

        publish(Some(OVERLAY), None).expect("first");
        let after_first = std::env::var(api::MODEL_CONTEXT_WINDOWS_ENV).expect("set");
        publish(Some(OVERLAY), None).expect("second");
        assert_eq!(
            std::env::var(api::MODEL_CONTEXT_WINDOWS_ENV).as_deref(),
            Ok(after_first.as_str())
        );
    }

    /// The audit copy is the effective catalog end to end: the layers in the
    /// order the registry consults them, the shipped seed last, and the
    /// merged bytes the variable carries.
    #[test]
    fn the_audit_copy_lists_every_layer_in_precedence_order() {
        let _lock = crate::test_env_lock();
        let export = r#"{"models":[{"provider":"google","ids":["gemini-3.7-flash"],"wire":"pinned-by-operator"}]}"#;
        let _env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, Some(export));
        reset_ownership_for_tests();

        let discovered = r#"{"models":[],"aliases":[{"alias":"google-latest","canonical":"gemini-3.8-flash","provider":"google"}]}"#;
        let published = publish(Some(OVERLAY), Some(discovered))
            .expect("publish")
            .expect("something to publish");
        let document = audit_document(Some(&published), 1_700_000_000).expect("document");
        let sources: Vec<&str> = document["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|layer| layer["source"].as_str().unwrap())
            .collect();
        assert_eq!(
            sources,
            ["operator export", "model catalog overlay", "discovered model catalog", "shipped"]
        );
        assert_eq!(document["layers"][0]["models"][0]["wire"], "pinned-by-operator");
        assert_eq!(document["layers"][1]["models"][0]["ids"][0], "gemini-3.7-flash");
        assert_eq!(document["layers"][2]["aliases"][0]["alias"], "google-latest");
        assert!(
            document["layers"][3]["models"].as_array().unwrap().len() > 1,
            "the shipped seed is part of the picture"
        );
        assert_eq!(
            document["published"],
            serde_json::from_str::<serde_json::Value>(&published.json).unwrap()
        );
        assert_eq!(document["variable"], api::MODEL_CONTEXT_WINDOWS_ENV);

        let dir = crate::support::temp_dir("catalog-audit");
        let path = dir.join("cache").join("merged.json");
        write_audit(&path, &document).expect("write");
        let read: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read, document);
        let _ = std::fs::remove_dir_all(&dir);

        // Nothing published: the seed alone is the effective catalog.
        let bare = audit_document(None, 1).expect("document");
        assert_eq!(bare["layers"].as_array().unwrap().len(), 1);
        assert!(bare["published"].is_null());

        publish(Some(r#"{"models":[],"aliases":[]}"#), None).expect("restore");
    }

    /// The router's priors ride the same bridge: an operator export that
    /// names one field wins that field, an overlay fills the next, and the
    /// merged document carries only what some layer declared — the api crate
    /// fills the rest from the shipped catalog.
    #[test]
    fn priors_layer_per_field_across_the_bridge() {
        let _lock = crate::test_env_lock();
        let export = r#"{"models":[],"priors":{"small_tokens":["tiny"]}}"#;
        let env = crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, Some(export));
        reset_ownership_for_tests();

        let published = publish(
            Some(r#"{"models":[],"priors":{"small_tokens":["ignored"],"deep_flagship_tokens":["titan"]}}"#),
            None,
        )
        .expect("publish")
        .expect("published");
        let merged: serde_json::Value = serde_json::from_str(&published.json).unwrap();
        assert_eq!(merged["priors"]["small_tokens"], serde_json::json!(["tiny"]));
        assert_eq!(merged["priors"]["deep_flagship_tokens"], serde_json::json!(["titan"]));
        assert!(
            merged["priors"]["frontier_family_tokens"].as_array().unwrap().is_empty(),
            "a field no layer fills is left for the shipped catalog"
        );
        let live = api::router_priors();
        assert_eq!(live.small_tokens, ["tiny"]);
        assert_eq!(live.deep_flagship_tokens, ["titan"]);
        assert!(live.frontier_family_tokens.iter().any(|token| token == "claude"));

        publish(Some(r#"{"models":[],"aliases":[]}"#), None).expect("republish");
        assert_eq!(api::router_priors().small_tokens, ["tiny"], "the export still names it");

        // Leave the process-global priors as we found them: without the export
        // a publish that declares none puts the shipped words back.
        drop(env);
        reset_ownership_for_tests();
        publish(Some(r#"{"models":[],"aliases":[]}"#), None).expect("restore");
        assert!(api::router_priors().small_tokens.iter().any(|token| token == "haiku"));
    }

    #[test]
    fn a_malformed_models_field_is_reported_rather_than_silently_dropped() {
        let error = merge(Some(r#"{"models":{}}"#), &[(OVERLAY_LAYER, OVERLAY)]).expect_err("rejected");
        assert!(error.contains("must be a JSON array"), "{error}");
    }

    /// Discovered ceilings reach the api crate's override, and an operator's
    /// export keeps the ids it names.
    #[test]
    fn discovered_effort_ceilings_fill_in_under_an_operator_export() {
        let _lock = crate::test_env_lock();
        let _env = crate::support::EnvVarGuard::set(
            api::MODEL_EFFORT_CEILINGS_ENV,
            Some(r#"{"gpt-5.7-sol":"max"}"#),
        );
        reset_ownership_for_tests();

        publish_effort_ceilings(Some(r#"{"gpt-5.7-sol":"ultra","gpt-5.7-luna":"max"}"#)).expect("publish");
        assert_eq!(api::max_supported_effort("gpt-5.7-sol"), api::EffortLevel::Max, "the export holds");
        assert_eq!(api::max_supported_effort("gpt-5.7-luna"), api::EffortLevel::Max, "discovery fills in");
        publish_effort_ceilings(None).expect("restore");
        assert_eq!(std::env::var(api::MODEL_EFFORT_CEILINGS_ENV).as_deref(), Ok(r#"{"gpt-5.7-sol":"max"}"#));
    }

    // ---- t-17403: a zo started by a zo ------------------------------------------

    /// What a parent zo had discovered when it published: `google-latest` was
    /// 3.7, and there was a model only it knew.
    const PARENT_DISCOVERY: &str = r#"{"models":[{"provider":"google","ids":["gemini-parent-only"],"wire":"gemini-parent-only-low"}],"aliases":[{"alias":"google-latest","canonical":"gemini-3.7-flash","provider":"google"}]}"#;
    /// What a child zo discovered since: `google-latest` is 3.8.
    const CHILD_DISCOVERY: &str = r#"{"models":[],"aliases":[{"alias":"google-latest","canonical":"gemini-3.8-flash","provider":"google"}]}"#;
    /// An operator's decision about the same name.
    const OPERATOR_PIN: &str = r#"{"models":[],"aliases":[{"alias":"google-latest","canonical":"gemini-3.5-flash","provider":"google"}]}"#;

    /// Both bridges: the variable `api` reads, the mark of what zo published
    /// into it, and the operator's export carried beside it.
    const BRIDGE_VARIABLES: [&str; 6] = [
        api::MODEL_CONTEXT_WINDOWS_ENV,
        "ZO_MODEL_CONTEXT_WINDOWS_PUBLISHED",
        "ZO_MODEL_CONTEXT_WINDOWS_OPERATOR",
        api::MODEL_EFFORT_CEILINGS_ENV,
        "ZO_MODEL_EFFORT_CEILINGS_PUBLISHED",
        "ZO_MODEL_EFFORT_CEILINGS_OPERATOR",
    ];

    /// Every bridge variable cleared for the length of a test, and put back after.
    fn bare_bridges() -> Vec<crate::support::EnvVarGuard> {
        BRIDGE_VARIABLES.into_iter().map(|key| crate::support::EnvVarGuard::set(key, None)).collect()
    }

    /// The next process starts in the environment the last one left: nothing
    /// is captured yet.
    fn next_process() {
        reset_ownership_for_tests();
    }

    /// Leaves the process-global registry as it was found — nothing in the
    /// environment, nothing captured, an empty overlay published — when the
    /// test ends, whether it passed or not.
    struct RegistryBack;

    impl Drop for RegistryBack {
        fn drop(&mut self) {
            for key in BRIDGE_VARIABLES {
                std::env::remove_var(key);
            }
            next_process();
            let _ = publish(Some(r#"{"models":[],"aliases":[]}"#), None);
        }
    }

    fn google_latest() -> String {
        api::resolve_catalog_alias("google-latest")
    }

    /// (b) A zo started by a zo inherits the variable its parent published.
    /// That is what the parent knew, not an operator's decision, and the marks
    /// say whose it is: the child ranks it under its own discovery — a
    /// snapshot to start from — and not ahead of everything. (2026-09-30: `sol`
    /// stayed `gpt-6-sol` in every helper after `gpt-6.1-sol` was discovered.)
    #[test]
    fn a_child_zo_ranks_its_parents_published_catalog_under_its_own_discovery() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        next_process();

        publish(None, Some(PARENT_DISCOVERY)).expect("the parent publishes");
        assert_eq!(google_latest(), "gemini-3.7-flash");

        next_process();
        publish(None, Some(CHILD_DISCOVERY)).expect("the child publishes");
        assert_eq!(google_latest(), "gemini-3.8-flash", "the child's own discovery outranks the parent's snapshot");
        assert_eq!(
            api::wire_model_for_effort("gemini-parent-only", api::EffortLevel::Low).as_deref(),
            Some("gemini-parent-only-low"),
            "what only the parent knew still stands"
        );

    }

    /// (e) The same discovery in both, the same answer: the child and its
    /// parent name one model alike unless the child has learned something.
    #[test]
    fn a_child_zo_with_the_parents_discovery_resolves_as_the_parent_does() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        next_process();

        publish(None, Some(PARENT_DISCOVERY)).expect("the parent publishes");
        next_process();
        publish(None, Some(PARENT_DISCOVERY)).expect("the child publishes");
        assert_eq!(google_latest(), "gemini-3.7-flash");
        assert_eq!(
            api::wire_model_for_effort("gemini-parent-only", api::EffortLevel::Low).as_deref(),
            Some("gemini-parent-only-low")
        );

    }

    /// (a) A value the person exports by hand in a zo's shell is theirs: the
    /// marks their zo left in the environment do not fit it, so it stays ahead
    /// of everything in the child, and the parent's value is gone with the
    /// one it replaced.
    #[test]
    fn a_value_exported_by_hand_in_a_zo_shell_wins_in_the_child() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        next_process();

        publish(None, Some(PARENT_DISCOVERY)).expect("the parent publishes");
        std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, OPERATOR_PIN);

        next_process();
        publish(None, Some(CHILD_DISCOVERY)).expect("the child publishes");
        assert_eq!(google_latest(), "gemini-3.5-flash", "the export outranks the child's discovery");
        assert!(
            api::wire_model_for_effort("gemini-parent-only", api::EffortLevel::Low).is_none(),
            "the export replaced the parent's value"
        );

    }

    /// (c) A published value someone edited is no longer what zo published:
    /// the marks do not fit it, and it is read as the export it has become.
    #[test]
    fn a_published_value_that_was_edited_is_read_as_an_operators_export() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        next_process();

        publish(None, Some(PARENT_DISCOVERY)).expect("the parent publishes");
        let published = std::env::var(api::MODEL_CONTEXT_WINDOWS_ENV).expect("the parent published");
        std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, published.replace("gemini-3.7-flash", "gemini-3.6-pinned"));

        next_process();
        let child = publish(None, Some(CHILD_DISCOVERY)).expect("the child publishes").expect("published");
        assert_eq!(google_latest(), "gemini-3.6-pinned", "the edited value is a decision");
        assert_eq!(child.layers[0].0, "operator export");

    }

    /// (d) A value with no marks — an older zo wrote it, or a person did — is
    /// an operator's export, as every value was before the marks.
    #[test]
    fn a_value_with_no_marks_is_an_operators_export_as_it_always_was() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, PARENT_DISCOVERY);
        next_process();

        let child = publish(None, Some(CHILD_DISCOVERY)).expect("the child publishes").expect("published");
        assert_eq!(google_latest(), "gemini-3.7-flash", "a decision stands ahead of the child's discovery");
        let sources: Vec<&str> = child.layers.iter().map(|(source, _)| *source).collect();
        assert_eq!(sources, ["operator export", "discovered model catalog"]);

    }

    /// A real export stays first through every generation: the parent merges
    /// it in, the child and the grandchild inherit the merge, and none of them
    /// mistakes it for a snapshot.
    #[test]
    fn a_real_export_outranks_the_parent_the_child_and_the_grandchild() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, OPERATOR_PIN);
        next_process();

        publish(None, Some(PARENT_DISCOVERY)).expect("the parent publishes");
        assert_eq!(google_latest(), "gemini-3.5-flash");
        for generation in ["child", "grandchild"] {
            next_process();
            publish(None, Some(CHILD_DISCOVERY)).expect("published");
            assert_eq!(google_latest(), "gemini-3.5-flash", "the {generation} keeps the export first");
        }

    }

    /// What zo publishes is marked, and the operator's export rides beside it
    /// verbatim; an export restored as it was carries neither. The names are a
    /// contract between zo versions: a child of another version reads them.
    #[test]
    fn what_zo_publishes_is_marked_and_an_operators_export_is_carried_beside_it() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        let var = |key: &str| std::env::var(key).ok();
        next_process();

        publish(None, Some(PARENT_DISCOVERY)).expect("published");
        assert!(
            var("ZO_MODEL_CONTEXT_WINDOWS_PUBLISHED").is_some_and(|mark| !mark.is_empty()),
            "a published value carries a mark"
        );
        assert_eq!(var("ZO_MODEL_CONTEXT_WINDOWS_OPERATOR"), None, "no export, no operator half");

        std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, OPERATOR_PIN);
        std::env::remove_var("ZO_MODEL_CONTEXT_WINDOWS_PUBLISHED");
        next_process();
        publish(None, Some(PARENT_DISCOVERY)).expect("published");
        assert_eq!(var("ZO_MODEL_CONTEXT_WINDOWS_OPERATOR").as_deref(), Some(OPERATOR_PIN));
        assert_ne!(var(api::MODEL_CONTEXT_WINDOWS_ENV).as_deref(), Some(OPERATOR_PIN), "the merge is not the export");

        // Nothing of zo's to add: the export comes back as it was, unmarked.
        publish(None, None).expect("restored");
        assert_eq!(var(api::MODEL_CONTEXT_WINDOWS_ENV).as_deref(), Some(OPERATOR_PIN));
        assert_eq!(var("ZO_MODEL_CONTEXT_WINDOWS_PUBLISHED"), None);
        assert_eq!(var("ZO_MODEL_CONTEXT_WINDOWS_OPERATOR"), None);

    }

    /// The audit lists the parent's snapshot as a layer of its own, under the
    /// child's, and a model only the snapshot names is read as inherited.
    #[test]
    fn the_audit_lists_the_parents_snapshot_under_the_childs_own_layers() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        next_process();

        publish(None, Some(PARENT_DISCOVERY)).expect("the parent publishes");
        next_process();
        let child = publish(None, Some(CHILD_DISCOVERY)).expect("the child publishes").expect("published");
        let document = audit_document(Some(&child), 1_700_000_000).expect("document");
        let sources: Vec<&str> =
            document["layers"].as_array().unwrap().iter().map(|layer| layer["source"].as_str().unwrap()).collect();
        assert_eq!(sources, ["discovered model catalog", "parent zo's published catalog", "shipped"]);
        assert_eq!(super::selection_provenance("gemini-parent-only"), ("inherited", "mixed"));

    }

    /// The effort ceilings ride the same bridge shape and follow the same
    /// rule: a parent's are a snapshot under the child's own discovery, and an
    /// operator's export stays first through every generation. (A model no
    /// family knows gets `High` when nothing declares its ceiling, so every
    /// value below is one the bridge, and only the bridge, can have set.)
    #[test]
    fn effort_ceilings_follow_the_same_rule() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        let ceiling = || api::max_supported_effort("acme-model-1");
        next_process();

        publish_effort_ceilings(Some(r#"{"acme-model-1":"low","acme-model-2":"xhigh"}"#)).expect("the parent publishes");
        assert_eq!(ceiling(), api::EffortLevel::Low);

        next_process();
        publish_effort_ceilings(Some(r#"{"acme-model-1":"max"}"#)).expect("the child publishes");
        assert_eq!(ceiling(), api::EffortLevel::Max, "the child's discovery outranks the parent's snapshot");
        assert_eq!(api::max_supported_effort("acme-model-2"), api::EffortLevel::Xhigh, "what only the parent knew stands");

        // An export the person made: first for the parent, and for every process after it.
        for key in BRIDGE_VARIABLES {
            std::env::remove_var(key);
        }
        std::env::set_var(api::MODEL_EFFORT_CEILINGS_ENV, r#"{"acme-model-1":"medium"}"#);
        next_process();
        publish_effort_ceilings(Some(r#"{"acme-model-1":"low"}"#)).expect("the parent publishes");
        assert_eq!(ceiling(), api::EffortLevel::Medium);
        next_process();
        publish_effort_ceilings(Some(r#"{"acme-model-1":"max"}"#)).expect("the child publishes");
        assert_eq!(ceiling(), api::EffortLevel::Medium, "the export outranks the child's discovery");

        // A value exported afresh under a parent's marks is an export too.
        std::env::set_var(api::MODEL_EFFORT_CEILINGS_ENV, r#"{"acme-model-1":"xhigh"}"#);
        next_process();
        publish_effort_ceilings(Some(r#"{"acme-model-1":"max"}"#)).expect("published");
        assert_eq!(ceiling(), api::EffortLevel::Xhigh);

    }

    /// The same child publishing twice sends the same bytes: the snapshot is
    /// carried once, not folded in again each time.
    #[test]
    fn a_child_publishing_twice_sends_the_same_bytes() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        next_process();

        publish(None, Some(PARENT_DISCOVERY)).expect("the parent publishes");
        next_process();
        publish(None, Some(CHILD_DISCOVERY)).expect("first");
        let first = std::env::var(api::MODEL_CONTEXT_WINDOWS_ENV).expect("set");
        publish(None, Some(CHILD_DISCOVERY)).expect("second");
        assert_eq!(std::env::var(api::MODEL_CONTEXT_WINDOWS_ENV).as_deref(), Ok(first.as_str()));

    }

    /// The aliases a child resolves differently from the snapshot it inherited,
    /// read layer by layer: the first row of a name in the layers ahead of the
    /// snapshot answers, as it does for the catalog. A name only the child
    /// declares is not a difference (the parent had no answer), and neither is
    /// a model row.
    #[test]
    fn the_aliases_a_child_resolves_differently_are_read_layer_by_layer() {
        let ahead = [
            ("operator export", r#"{"aliases":[{"alias":"Pinned","canonical":"model-b"}]}"#),
            ("discovered model catalog", r#"{"aliases":[{"alias":"moved","canonical":"model-2"},{"alias":"only-mine","canonical":"model-x"},{"alias":"same","canonical":"model-1"}]}"#),
        ];
        let snapshot = r#"{"models":[{"ids":["model-9"]}],"aliases":[{"alias":"pinned","canonical":"model-a"},{"alias":"moved","canonical":"model-1"},{"alias":"moved","canonical":"model-0"},{"alias":"same","canonical":"MODEL-1"},{"alias":"only-parent","canonical":"model-p"}]}"#;
        assert_eq!(
            super::differing_answers(&ahead, snapshot),
            [
                ("pinned".to_string(), "model-a".to_string(), "model-b".to_string()),
                ("moved".to_string(), "model-1".to_string(), "model-2".to_string()),
            ],
            "the parent's first row per name is its answer; unchanged and one-sided names are silent"
        );
        assert!(super::differing_answers(&[], snapshot).is_empty(), "no layer ahead, nothing differs");
    }

    /// (e) The difference is written down — the alias, the parent's answer,
    /// the child's — once per alias, so a session and its helper naming one
    /// model differently is a line to read and not a mystery; the same
    /// discovery says nothing.
    #[test]
    fn a_child_says_once_per_alias_what_it_resolves_differently_from_its_parent() {
        let _lock = crate::test_env_lock();
        let _bridges = bare_bridges();
        let _registry = RegistryBack;
        next_process();

        publish(None, Some(PARENT_DISCOVERY)).expect("the parent publishes");
        let snapshot = std::env::var(api::MODEL_CONTEXT_WINDOWS_ENV).expect("the parent published");

        // The same discovery in both: nothing to say.
        next_process();
        publish(None, Some(PARENT_DISCOVERY)).expect("the child publishes");
        assert!(super::announce_differences(&[(DISCOVERED_LAYER, PARENT_DISCOVERY)], &snapshot).is_empty());

        // A newer discovery: publishing says it, and says it only once.
        next_process();
        publish(None, Some(CHILD_DISCOVERY)).expect("the child publishes");
        assert_eq!(google_latest(), "gemini-3.8-flash");
        assert!(
            super::announce_differences(&[(DISCOVERED_LAYER, CHILD_DISCOVERY)], &snapshot).is_empty(),
            "publish had already written the line for this alias"
        );

        // A fresh process says it again, with the alias and both answers.
        next_process();
        let lines = super::announce_differences(&[(DISCOVERED_LAYER, CHILD_DISCOVERY)], &snapshot);
        assert_eq!(lines.len(), 1, "{lines:?}");
        for word in ["google-latest", "gemini-3.7-flash", "gemini-3.8-flash"] {
            assert!(lines[0].contains(word), "{word} is missing from {:?}", lines[0]);
        }
        assert!(super::announce_differences(&[(DISCOVERED_LAYER, CHILD_DISCOVERY)], &snapshot).is_empty());
    }
}
