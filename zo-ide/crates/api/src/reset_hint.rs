//! The reset a provider names inside a refusal body.
//!
//! ChatGPT writes it into the `error` object of a usage-limit refusal, as
//! `resets_in_seconds` (relative) or `resets_at` (unix seconds); the HTTP
//! `Retry-After` header is read elsewhere. Both spellings are searched
//! anywhere in the body so a frame that nests them (`rate_limits.primary`)
//! still answers, relative winning over absolute inside one object.

use std::time::Duration;

use serde_json::Value;

const RELATIVE_SECONDS_KEY: &str = "resets_in_seconds";
const ABSOLUTE_UNIX_KEY: &str = "resets_at";

/// The reset `value` names, as a wait from `now_unix`; `None` when it names
/// none. An absolute time already in the past is a zero wait, which the
/// readers treat as "no hint".
#[must_use]
pub(crate) fn reset_hint_in_json(value: &Value, now_unix: u64) -> Option<Duration> {
    match value {
        Value::Object(map) => {
            if let Some(secs) = map.get(RELATIVE_SECONDS_KEY).and_then(as_seconds) {
                return Some(Duration::from_secs(secs));
            }
            if let Some(at) = map.get(ABSOLUTE_UNIX_KEY).and_then(as_seconds) {
                return Some(Duration::from_secs(at.saturating_sub(now_unix)));
            }
            map.values()
                .find_map(|child| reset_hint_in_json(child, now_unix))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| reset_hint_in_json(child, now_unix)),
        _ => None,
    }
}

/// [`reset_hint_in_json`] over a raw body; a body that is not JSON names no
/// reset.
#[must_use]
pub(crate) fn reset_hint_in_body(body: &str, now_unix: u64) -> Option<Duration> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| reset_hint_in_json(&value, now_unix))
}

/// A JSON number (integer or float) or a numeric string, as whole seconds.
fn as_seconds(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().and_then(whole_seconds))
        .or_else(|| value.as_str().and_then(|text| text.trim().parse::<u64>().ok()))
}

/// The whole seconds of a non-negative finite float; the fraction is dropped
/// on purpose (a reset is never owed a sub-second), and anything else names no
/// seconds at all.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn whole_seconds(secs: f64) -> Option<u64> {
    (secs.is_finite() && secs >= 0.0).then(|| secs.trunc() as u64)
}

#[cfg(test)]
mod tests {
    use super::{reset_hint_in_body, reset_hint_in_json};
    use serde_json::json;
    use std::time::Duration;

    /// The `error` object's own reset is read in either spelling; the relative
    /// one wins inside one object, and a nested window still answers.
    #[test]
    fn a_refusal_body_names_its_reset_in_either_spelling() {
        let now = 1_000;
        assert_eq!(
            reset_hint_in_json(
                &json!({"error":{"code":"usage_limit_reached","resets_in_seconds":8220}}),
                now
            ),
            Some(Duration::from_secs(8220))
        );
        assert_eq!(
            reset_hint_in_json(&json!({"error":{"resets_at":9_220}}), now),
            Some(Duration::from_secs(8220))
        );
        assert_eq!(
            reset_hint_in_json(
                &json!({"error":{"resets_in_seconds":30,"resets_at":9_220}}),
                now
            ),
            Some(Duration::from_secs(30)),
            "relative wins inside one object"
        );
        assert_eq!(
            reset_hint_in_json(
                &json!({"rate_limits":{"primary":{"resets_at":"1300"}}}),
                now
            ),
            Some(Duration::from_secs(300)),
            "a nested numeric string still answers"
        );
        assert_eq!(
            reset_hint_in_json(&json!({"error":{"resets_at":5}}), now),
            Some(Duration::ZERO),
            "a reset already past is a zero wait"
        );
        assert_eq!(reset_hint_in_json(&json!({"error":{"message":"nope"}}), now), None);
        assert_eq!(
            reset_hint_in_body(r#"{"error":{"resets_in_seconds":7}}"#, now),
            Some(Duration::from_secs(7))
        );
        assert_eq!(reset_hint_in_body("not json", now), None);
    }
}
