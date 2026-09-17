//! Ownership of the `ZO_CUSTOM_PROVIDERS` process bridge.
//!
//! The `api` crate cannot depend on runtime config, so the merged settings
//! `providers` array reaches it through one environment variable. That makes
//! the variable two different things depending on who wrote it:
//!
//! * **Operator-exported** — the user set it in their shell before launching
//!   zo. It is an explicit override and zo must never rewrite it; settings-side
//!   entries are only *added* for names the override does not mention.
//! * **Zo-seeded** — zo copied `settings.json` into it at boot. It is a cache
//!   of the settings file, so after any `/connect` or `/providers` mutation it
//!   must be re-published or the live catalog keeps serving the boot-time
//!   snapshot: newly merged models would stay invisible and deletes would not
//!   take effect until the next restart.
//!
//! Which of the two applies is decided **once**, from the value present before
//! zo writes anything, and latched for the process.

use std::sync::Mutex;

use serde_json::Value;

/// The exact string zo last wrote to `ZO_CUSTOM_PROVIDERS`, or `None` before it
/// has written any. Ownership is decided by comparing against this rather than
/// by a one-way "who got there first" flag: the comparison stays correct no
/// matter how many times the variable is set, cleared, or re-exported.
static ZO_PUBLISHED: Mutex<Option<String>> = Mutex::new(None);

fn env_value() -> Option<String> {
    std::env::var(api::CUSTOM_PROVIDERS_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn remember_published(value: &str) {
    if let Ok(mut slot) = ZO_PUBLISHED.lock() {
        *slot = Some(value.to_string());
    }
}

/// Whether the value currently in `ZO_CUSTOM_PROVIDERS` came from the operator
/// rather than from zo's own settings mirror.
///
/// A non-empty value zo did not write is an override; anything else (absent, or
/// byte-identical to zo's last publish) means zo owns the variable.
pub(crate) fn operator_override_active() -> bool {
    let Some(current) = env_value() else {
        return false;
    };
    ZO_PUBLISHED
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .is_none_or(|published| published != current)
}

/// Forget what zo published, so a test can exercise both ownership modes.
#[cfg(test)]
pub(crate) fn reset_ownership_for_tests() {
    if let Ok(mut slot) = ZO_PUBLISHED.lock() {
        *slot = None;
    }
}

/// Make `settings_json` (the merged global `providers` array) the live catalog.
///
/// Under an operator override the export stays byte-identical and settings
/// entries are merged in only for names it does not already declare — an
/// override is a decision, not a suggestion. Otherwise the variable is
/// re-seeded from settings, which is what makes a `/connect` merge or a
/// `/providers` delete visible in the same session.
pub(crate) fn publish(settings_json: &str) -> Result<(), String> {
    if let Some(override_json) = env_value().filter(|_| operator_override_active()) {
        let merged = merge_operator_override(&override_json, settings_json)?;
        return api::refresh_custom_providers_from_json(&merged).map_err(|error| error.to_string());
    }
    std::env::set_var(api::CUSTOM_PROVIDERS_ENV, settings_json);
    remember_published(settings_json);
    api::refresh_custom_providers_from_json(settings_json).map_err(|error| error.to_string())
}

/// Whether a mutation just written to settings can take effect in this process.
///
/// `false` only under an operator override, where the export pins the catalog;
/// callers turn this into an explicit "restart or unset" note rather than
/// reporting a change that silently did nothing.
pub(crate) fn mutations_apply_live() -> bool {
    !operator_override_active()
}

/// Layer settings entries under an operator override: the override wins every
/// name it declares, settings contribute the rest.
fn merge_operator_override(env_json: &str, settings_json: &str) -> Result<String, String> {
    let mut entries = parse_array(api::CUSTOM_PROVIDERS_ENV, env_json)?;
    for entry in parse_array("settings.providers", settings_json)? {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            entries.push(entry);
            continue;
        };
        if !entries
            .iter()
            .any(|existing| existing.get("name").and_then(Value::as_str) == Some(name))
        {
            entries.push(entry);
        }
    }
    serde_json::to_string(&Value::Array(entries)).map_err(|error| error.to_string())
}

fn parse_array(label: &str, raw: &str) -> Result<Vec<Value>, String> {
    match serde_json::from_str::<Value>(raw).map_err(|error| error.to_string())? {
        Value::Array(entries) => Ok(entries),
        _ => Err(format!("{label} must be a JSON array")),
    }
}

#[cfg(test)]
mod tests {
    use super::{merge_operator_override, mutations_apply_live, publish, reset_ownership_for_tests};

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

    const SETTINGS: &str = r#"[{"name":"deepseek","base_url":"https://api.deepseek.com","models":["deepseek-chat"],"requires_auth":false}]"#;

    /// Whether the live catalog serves `model`. Model ids — not provider names —
    /// are the reliable probe: `api` appends built-in seed providers that reuse
    /// well-known names whenever a matching credential happens to be present.
    fn registers_model(model: &str) -> bool {
        api::custom_provider_catalog()
            .iter()
            .any(|(_, models)| models.iter().any(|candidate| candidate == model))
    }

    /// Without an operator export, a later publish must actually reach the live
    /// catalog — this is what makes a merge or a delete visible without a restart.
    #[test]
    fn zo_seeded_bridge_republishes_settings() {
        let _lock = crate::test_env_lock();
        let _env = EnvVarGuard::set(api::CUSTOM_PROVIDERS_ENV, None);
        reset_ownership_for_tests();

        publish(SETTINGS).expect("seed");
        assert!(mutations_apply_live());
        assert!(
            registers_model("deepseek-chat"),
            "the settings entry reaches the live catalog"
        );

        // The delete case: settings now say "nothing registered". Assert on the
        // registered model id, not on an empty catalog — `api` always appends
        // its own built-in seed providers when a matching credential exists.
        publish("[]").expect("republish");
        assert!(
            !registers_model("deepseek-chat"),
            "a delete must leave the live catalog, not wait for a restart"
        );
        assert_eq!(
            std::env::var(api::CUSTOM_PROVIDERS_ENV).as_deref(),
            Ok("[]"),
            "the zo-seeded bridge tracks settings"
        );
    }

    /// An operator export is a decision: it stays byte-identical and still wins
    /// by name, while settings-only entries are additive.
    #[test]
    fn an_operator_override_is_never_rewritten() {
        let _lock = crate::test_env_lock();
        let override_json = r#"[{"name":"env-only","base_url":"http://env.example/v1","models":["env-model"],"requires_auth":false}]"#;
        let _env = EnvVarGuard::set(api::CUSTOM_PROVIDERS_ENV, Some(override_json));
        reset_ownership_for_tests();

        publish(SETTINGS).expect("publish");
        assert!(!mutations_apply_live());
        assert_eq!(
            std::env::var(api::CUSTOM_PROVIDERS_ENV).as_deref(),
            Ok(override_json)
        );
        assert!(registers_model("env-model"), "the override is honored");
        assert!(
            registers_model("deepseek-chat"),
            "settings still contribute names the override does not declare"
        );

        api::refresh_custom_providers_from_json("[]").expect("restore");
    }

    #[test]
    fn the_override_wins_a_name_collision() {
        let env_json = r#"[{"name":"deepseek","base_url":"http://pinned/v1","models":["pinned"]}]"#;
        let merged = merge_operator_override(env_json, SETTINGS).expect("merge");
        let entries: Vec<serde_json::Value> = serde_json::from_str(&merged).expect("array");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["base_url"], "http://pinned/v1");
    }

    #[test]
    fn a_non_array_override_is_reported_rather_than_silently_dropped() {
        let error = merge_operator_override("{}", SETTINGS).expect_err("rejected");
        assert!(error.contains("must be a JSON array"), "{error}");
    }
}
