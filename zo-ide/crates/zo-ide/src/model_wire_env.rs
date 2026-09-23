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

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

const OPERATOR_EXPORT_LAYER: &str = "operator export";
const OVERLAY_LAYER: &str = "model catalog overlay";
const DISCOVERED_LAYER: &str = "discovered model catalog";
const SHIPPED_LAYER: &str = "shipped";
/// Beside the discovery cache: the merged copy a refresh leaves for audit.
const AUDIT_FILE: &str = "merged.json";

/// What one publish made live: each layer in precedence order, and the merged
/// document the variable now carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub layers: Vec<(&'static str, String)>,
    pub json: String,
}

/// What the operator had exported before zo first wrote the variable.
///
/// Three states, not two: "not looked yet" has to stay distinguishable from
/// "looked, and there was no export", because the answer is captured ONCE and
/// reused. `/model` rebuilds the runtime and republishes, and re-reading the
/// variable each time would fold zo's own previous output back into the
/// operator half and grow it without bound.
enum OperatorBase {
    Unread,
    Absent,
    Export(String),
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
                DISCOVERED_LAYER => "discovered", _ => "shipped",
            };
            let provenance = if source == "discovered" || row.get("effort_levels").is_none() {
                "mixed"
            } else { "declared" };
            return (source, provenance);
        }
    }
    ("unknown", "unknown")
}

static OPERATOR_BASE: Mutex<OperatorBase> = Mutex::new(OperatorBase::Unread);
static EFFORT_BASE: Mutex<OperatorBase> = Mutex::new(OperatorBase::Unread);

fn captured(slot: &Mutex<OperatorBase>, key: &str) -> Option<String> {
    let mut slot = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if matches!(*slot, OperatorBase::Unread) {
        *slot = std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map_or(OperatorBase::Absent, OperatorBase::Export);
    }
    match &*slot {
        OperatorBase::Export(value) => Some(value.clone()),
        OperatorBase::Unread | OperatorBase::Absent => None,
    }
}

fn operator_base() -> Option<String> {
    captured(&OPERATOR_BASE, api::MODEL_CONTEXT_WINDOWS_ENV)
}

/// Forget the captured exports so a test can exercise both ownership modes.
#[cfg(test)]
pub(crate) fn reset_ownership_for_tests() {
    if let Ok(mut slot) = OPERATOR_BASE.lock() {
        *slot = OperatorBase::Unread;
    }
    if let Ok(mut slot) = EFFORT_BASE.lock() {
        *slot = OperatorBase::Unread;
    }
}

/// Make the overlay-declared and discovered catalogs live.
///
/// `None` for both means nothing to add, which is NOT the same as an empty
/// catalog: with no operator export there is simply nothing to publish, and
/// the variable is left untouched (`Ok(None)`) rather than being set to an
/// empty catalog that would read as a deliberate "no models are declared".
pub(crate) fn publish(
    overlay_json: Option<&str>,
    discovered_json: Option<&str>,
) -> Result<Option<Published>, String> {
    let base = operator_base();
    let own: Vec<(&'static str, &str)> = [
        (OVERLAY_LAYER, overlay_json),
        (DISCOVERED_LAYER, discovered_json),
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
    std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, &json);
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
/// rest. `None` with no export leaves the variable untouched.
pub(crate) fn publish_effort_ceilings(discovered_json: Option<&str>) -> Result<(), String> {
    let base = captured(&EFFORT_BASE, api::MODEL_EFFORT_CEILINGS_ENV);
    let published = match (base.as_deref(), discovered_json) {
        (None, None) => None,
        (Some(base), None) => Some(base.to_string()),
        (None, Some(discovered)) => {
            object_of("discovered effort ceilings", discovered)?;
            Some(discovered.to_string())
        }
        (Some(base), Some(discovered)) => {
            let mut merged = object_of("discovered effort ceilings", discovered)?;
            for (key, value) in object_of(api::MODEL_EFFORT_CEILINGS_ENV, base)? {
                merged.insert(key, value);
            }
            Some(serde_json::to_string(&Value::Object(merged)).map_err(|error| error.to_string())?)
        }
    };
    if let Some(published) = published {
        std::env::set_var(api::MODEL_EFFORT_CEILINGS_ENV, published);
    }
    Ok(())
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
        write_audit, OVERLAY_LAYER,
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
}
