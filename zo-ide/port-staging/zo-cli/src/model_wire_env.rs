//! Ownership of the `ZO_MODEL_CONTEXT_WINDOWS` process bridge for
//! settings-declared model wire ids.
//!
//! The `api` crate holds the model catalog — which id a selection is actually
//! served under — but cannot depend on runtime config, so a model declared in
//! `settings.json` reaches it through this one environment variable. It is the
//! same bridge shape as [`crate::custom_provider_env`], and it exists for the
//! same reason: a model the provider ships after this binary was built must be
//! usable without a rebuild.
//!
//! An operator may export the variable themselves. That export is a decision,
//! so it is captured once and always kept — and kept FIRST, because the catalog
//! resolves to the first entry that names a model, which makes leading position
//! precedence. Settings entries are layered after it, contributing the models
//! the export does not declare.

use std::sync::Mutex;

use serde_json::{Map, Value};

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

static OPERATOR_BASE: Mutex<OperatorBase> = Mutex::new(OperatorBase::Unread);

fn operator_base() -> Option<String> {
    let mut slot = OPERATOR_BASE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if matches!(*slot, OperatorBase::Unread) {
        *slot = std::env::var(api::MODEL_CONTEXT_WINDOWS_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map_or(OperatorBase::Absent, OperatorBase::Export);
    }
    match &*slot {
        OperatorBase::Export(value) => Some(value.clone()),
        OperatorBase::Unread | OperatorBase::Absent => None,
    }
}

/// Forget the captured export so a test can exercise both ownership modes.
#[cfg(test)]
pub(crate) fn reset_ownership_for_tests() {
    if let Ok(mut slot) = OPERATOR_BASE.lock() {
        *slot = OperatorBase::Unread;
    }
}

/// Make the settings-declared wire catalog live.
///
/// `None` means settings declare nothing, which is NOT the same as an empty
/// catalog: with no operator export there is simply nothing to publish, and the
/// variable is left untouched rather than being set to an empty catalog that
/// would read as a deliberate "no models are declared".
pub(crate) fn publish(settings_json: Option<&str>) -> Result<(), String> {
    let base = operator_base();
    let published = match (base.as_deref(), settings_json) {
        (None, None) => None,
        // Nothing of zo's to add, but a previous publish may have appended
        // entries that settings no longer declare — restore the export.
        (Some(base), None) => Some(base.to_string()),
        (None, Some(settings)) => {
            validate(settings)?;
            Some(settings.to_string())
        }
        (Some(base), Some(settings)) => Some(merge(base, settings)?),
    };
    let Some(published) = published else {
        return Ok(());
    };
    std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, &published);
    // The wire lookup re-reads the variable per call, but the alias registry is
    // built once and cached, so it has to be told. Idempotent on identical
    // bytes, which is what every rebuild-triggered republish sends.
    api::refresh_model_registry_from_json(&published);
    Ok(())
}

/// Layer settings models after the operator's, which keeps the export's entries
/// ahead of them and therefore authoritative for every id it names.
fn merge(base: &str, settings: &str) -> Result<String, String> {
    let mut models = models_of(api::MODEL_CONTEXT_WINDOWS_ENV, base)?;
    models.extend(models_of("settings.modelCatalog", settings)?);
    let mut root = Map::new();
    root.insert("models".to_string(), Value::Array(models));
    serde_json::to_string(&Value::Object(root)).map_err(|error| error.to_string())
}

fn validate(raw: &str) -> Result<(), String> {
    models_of("settings.modelCatalog", raw).map(|_| ())
}

fn models_of(label: &str, raw: &str) -> Result<Vec<Value>, String> {
    let parsed: Value = serde_json::from_str(raw).map_err(|error| format!("{label}: {error}"))?;
    match parsed.get("models") {
        Some(Value::Array(models)) => Ok(models.clone()),
        // An export with no `models` key is not an error the user can act on
        // here — the catalog reader ignores it too — so carry it as empty
        // rather than refusing to publish anything at all.
        Some(_) => Err(format!("{label}: \"models\" must be a JSON array")),
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::{merge, publish, reset_ownership_for_tests};

    struct EnvVarGuard {
        key: &'static str,
        old: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let old = std::env::var(key).ok();
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            Self { key, old }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.old.as_deref() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
            reset_ownership_for_tests();
        }
    }

    const SETTINGS: &str = r#"{"models":[{"provider":"google","ids":["gemini-3.7-flash"],"wire":{"low":"gemini-3.7-flash-low","high":"gemini-3.7-flash-high"}}]}"#;

    /// The whole point of the bridge: a model declared only in settings is
    /// served under the id those settings name, with no rebuild.
    #[test]
    fn a_settings_declared_model_reaches_the_catalog() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        reset_ownership_for_tests();

        publish(Some(SETTINGS)).expect("publish");
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
        let _env = EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        reset_ownership_for_tests();

        publish(Some(
            r#"{"models":[],"aliases":[{"alias":"google-latest","canonical":"gemini-3.7-flash","provider":"google"}]}"#,
        ))
        .expect("publish");
        assert_eq!(api::resolve_catalog_alias("google-latest"), "gemini-3.7-flash");

        // Leave the process-global registry as we found it for other tests.
        publish(Some(r#"{"models":[],"aliases":[]}"#)).expect("restore");
        assert_eq!(api::resolve_catalog_alias("google-latest"), "gemini-3.6-flash");
    }

    /// Republishing must track settings: a `/model` edit that drops a model
    /// cannot leave the boot-time snapshot serving it.
    #[test]
    fn republishing_tracks_settings() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        reset_ownership_for_tests();

        publish(Some(SETTINGS)).expect("publish");
        publish(Some(r#"{"models":[]}"#)).expect("republish");
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
        let _env = EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, Some(export));
        reset_ownership_for_tests();

        publish(Some(
            r#"{"models":[{"provider":"google","ids":["gemini-3.7-flash"],"wire":"from-settings"},{"provider":"google","ids":["gemini-4-flash"],"wire":"gemini-4-flash-low"}]}"#,
        ))
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
        let _env = EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, Some(export));
        reset_ownership_for_tests();

        publish(Some(SETTINGS)).expect("first");
        let after_first = std::env::var(api::MODEL_CONTEXT_WINDOWS_ENV).expect("set");
        publish(Some(SETTINGS)).expect("second");
        assert_eq!(
            std::env::var(api::MODEL_CONTEXT_WINDOWS_ENV).as_deref(),
            Ok(after_first.as_str())
        );
    }

    #[test]
    fn a_malformed_models_field_is_reported_rather_than_silently_dropped() {
        let error = merge(r#"{"models":{}}"#, SETTINGS).expect_err("rejected");
        assert!(error.contains("must be a JSON array"), "{error}");
    }
}
