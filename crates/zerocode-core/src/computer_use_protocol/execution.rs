//! Background input is app-scoped semantic input, never a foreground fallback.

use serde_json::{Map, Value};

use super::{ProviderError, error_code};

pub const BACKGROUND: &str = "background";
pub const REQUIRES_FOREGROUND: &str = "requires_foreground";
pub const METHODS: &[&str] = &[
    "getAppState",
    "readText",
    "findElements",
    "waitFor",
    "click",
    "setValue",
    "scroll",
];

#[must_use]
pub fn requested(params: &Map<String, Value>) -> bool {
    params.get(BACKGROUND) == Some(&Value::Bool(true))
}

#[must_use]
pub fn unavailable() -> ProviderError {
    ProviderError::new(
        REQUIRES_FOREGROUND,
        "this operation has no supported background path; no foreground or synthetic fallback was attempted",
    )
}

pub fn validate(method: &str, params: &Map<String, Value>) -> Result<(), ProviderError> {
    if params
        .get(BACKGROUND)
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(ProviderError::invalid_argument(
            "background must be a boolean",
        ));
    }
    if !requested(params) {
        return Ok(());
    }
    if params.get("restoreWindow") == Some(&Value::Bool(true)) {
        return Err(ProviderError::invalid_argument(
            "background cannot restore or activate a window",
        ));
    }
    if !METHODS.contains(&method) {
        return Err(unavailable());
    }
    if params
        .get("app")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(ProviderError::invalid_argument(
            "background requires an explicit target app",
        ));
    }
    Ok(())
}

pub fn require_support(supported: bool, params: &Map<String, Value>) -> Result<(), ProviderError> {
    if requested(params) && !supported {
        return Err(ProviderError::new(
            error_code::UNSUPPORTED_CAPABILITY,
            "the provider has not advertised background input; update it or choose an explicit foreground workflow",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_providers_share_the_background_admission_table() {
        for line in include_str!("cases/background.tsv").lines() {
            let row = line.split('\t').collect::<Vec<_>>();
            let mut params = json!({"background": true, "restoreWindow": row[1] == "true"});
            if row[2] == "true" {
                params["app"] = "Fixture".into();
            }
            let result = validate(row[0], params.as_object().unwrap());
            assert_eq!(
                result.err().map_or("ok".to_owned(), |error| error.code),
                row[3],
                "{line}"
            );
        }
    }

    #[test]
    fn background_cannot_escape_to_global_input_or_activation() {
        let params = json!({"background": true, "app": "Fixture"});
        for method in [
            "pressKey",
            "hotkey",
            "pasteText",
            "typeText",
            "drag",
            "activateApp",
            "windowAction",
            "mouseClick",
            "mouseMove",
            "clipboardWrite",
            "reflexStart",
            "unknown",
        ] {
            assert_eq!(
                validate(method, params.as_object().unwrap())
                    .unwrap_err()
                    .code,
                REQUIRES_FOREGROUND
            );
        }
        for method in METHODS {
            assert!(validate(method, params.as_object().unwrap()).is_ok());
        }
    }

    #[test]
    fn background_is_explicit_scoped_and_fail_closed_on_old_providers() {
        for params in [
            json!({"background": "true"}),
            json!({"background": true}),
            json!({"background": true, "app": "Fixture", "restoreWindow": true}),
        ] {
            assert!(validate("getAppState", params.as_object().unwrap()).is_err());
        }
        let params = json!({"background": true, "app": "Fixture"});
        assert!(require_support(false, params.as_object().unwrap()).is_err());
        assert!(require_support(true, params.as_object().unwrap()).is_ok());
        assert!(validate("mouseClick", &Map::new()).is_ok());
        assert!(require_support(false, &Map::new()).is_ok());
    }
}
