//! Lenient parsing for model-emitted JSON string arguments.
//!
//! Models occasionally corrupt string-encoded JSON arguments in ways strict
//! `serde_json` rejects wholesale. Three corruptions are seen in the wild:
//!
//! * **Stray backslashes** — a hand-stringified payload containing `\d`,
//!   `C:\path`, or a literal `\t` that is not a legal JSON escape
//!   (`invalid escape at …`).
//! * **Trailing tool-call markup** — Claude models sometimes append their
//!   text-format tool-call closing tags (`</parameter>\n</invoke>`) after the
//!   JSON value inside a *native* `tool_use` string argument, so strict
//!   parsing fails with `trailing characters` even though the value itself is
//!   intact.
//! * **Markdown fences** — the value wrapped in ```` ```json … ``` ````.
//!
//! [`parse_model_json`] repairs all three. The strict parse runs first and
//! each repair only runs after the previous stage fails, so well-formed input
//! never pays for the leniency. On failure the *strict* parse error is
//! reported — it names the first real corruption instead of a repair
//! artifact.

use serde_json::Value;

/// Parse a model-emitted JSON string, tolerating the corruptions described in
/// the module docs. Returns the first complete JSON value when trailing
/// garbage follows it; the discarded tail is markup noise, never a second
/// payload a well-behaved model would send.
pub(crate) fn parse_model_json(raw: &str) -> Result<Value, String> {
    let trimmed = strip_markdown_fence(raw.trim());
    let strict_err = match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => return Ok(value),
        Err(err) => err,
    };
    if let Some(value) = first_json_value(trimmed) {
        return Ok(value);
    }
    let repaired = escape_stray_backslashes(trimmed);
    if let Ok(value) = serde_json::from_str::<Value>(&repaired) {
        return Ok(value);
    }
    if let Some(value) = first_json_value(&repaired) {
        return Ok(value);
    }
    Err(strict_err.to_string())
}

/// Extract the first complete JSON value from `raw`, ignoring anything after
/// it (e.g. leaked `</parameter></invoke>` tool-call tags). `None` when the
/// text does not even start with a complete value.
fn first_json_value(raw: &str) -> Option<Value> {
    serde_json::Deserializer::from_str(raw)
        .into_iter::<Value>()
        .next()?
        .ok()
}

/// Strip one surrounding markdown code fence (```` ```json … ``` ````),
/// leaving the input untouched when no complete fence wraps it.
fn strip_markdown_fence(trimmed: &str) -> &str {
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(body_start) = rest.find('\n') else {
        return trimmed;
    };
    let body = &rest[body_start + 1..];
    let Some(end) = body.rfind("```") else {
        return trimmed;
    };
    body[..end].trim()
}

/// Escape every backslash that does not begin a valid JSON escape sequence,
/// turning `\d` into `\\d` while leaving `\n`, `\"`, and `\uXXXX` intact.
pub(crate) fn escape_stray_backslashes(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    for (idx, ch) in input.char_indices() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let is_valid = match bytes.get(idx + 1).copied() {
            Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => true,
            Some(b'u') => bytes
                .get(idx + 2..idx + 6)
                .is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)),
            _ => false,
        };
        if is_valid {
            out.push('\\');
        } else {
            // Stray backslash: emit a literal `\\` so it survives JSON parsing.
            out.push_str("\\\\");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_json_passes_through() {
        assert_eq!(
            parse_model_json(r#"[{"a": 1}]"#).unwrap(),
            json!([{"a": 1}])
        );
    }

    #[test]
    fn trailing_invoke_markup_is_ignored() {
        // Exact wild shape: Claude appended its text-format tool-call closing
        // tags after the array inside a native tool_use string argument.
        let raw = "[{\"content\": \"1단계: DB 확장\", \"status\": \"pending\"}]</parameter>\n</invoke>\n";
        let parsed = parse_model_json(raw).unwrap();
        assert_eq!(
            parsed,
            json!([{"content": "1단계: DB 확장", "status": "pending"}])
        );
    }

    #[test]
    fn stray_backslash_with_trailing_markup_is_repaired() {
        let raw = "[{\"content\": \"regex \\d+\"}]</invoke>";
        let parsed = parse_model_json(raw).unwrap();
        assert_eq!(parsed, json!([{"content": "regex \\d+"}]));
    }

    #[test]
    fn markdown_fence_is_stripped() {
        let raw = "```json\n{\"a\": 1}\n```";
        assert_eq!(parse_model_json(raw).unwrap(), json!({"a": 1}));
    }

    #[test]
    fn first_of_multiple_values_wins() {
        assert_eq!(
            parse_model_json(r#"{"a": 1}{"b": 2}"#).unwrap(),
            json!({"a": 1})
        );
    }

    #[test]
    fn escape_stray_backslashes_preserves_valid_escapes() {
        // Valid JSON escapes (\n, \", \uXXXX) must survive byte-for-byte; only
        // illegal ones (\d) get doubled.
        let input = "line\\nbreak \\\"quote\\\" \\uc218\\uc815 regex \\d";
        let repaired = escape_stray_backslashes(input);
        assert_eq!(repaired, "line\\nbreak \\\"quote\\\" \\uc218\\uc815 regex \\\\d");
    }

    #[test]
    fn garbage_reports_the_strict_error() {
        let err = parse_model_json("not json at all").unwrap_err();
        assert!(err.contains("expected"), "unexpected error text: {err}");
    }

    #[test]
    fn truncated_value_still_fails() {
        assert!(parse_model_json(r#"[{"a": 1}"#).is_err());
    }
}
