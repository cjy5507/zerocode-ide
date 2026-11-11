//! A tool call's JSON arguments, as a model sent them.
//!
//! Strict JSON first. When that fails, mend only the slips that leave the
//! call's meaning as the model wrote it — a code fence around the object, the
//! object sent as a JSON string, a raw newline or tab inside a string, a
//! trailing comma — and parse once more. A cut-off call (an unclosed string,
//! object or array) is never completed: guessing the rest of a command is
//! worse than asking for it again.

use std::fmt::Write as _;

use serde_json::Value;

/// Parse a tool call's arguments, mending what the module docs list. The
/// error is the strict parser's, so a refusal still names what was sent.
///
/// # Errors
/// The strict parse error when the input is not JSON and mending would have
/// to invent something to make it so.
pub fn parse_tool_arguments(input: &str) -> Result<Value, serde_json::Error> {
    let strict = match serde_json::from_str::<Value>(input) {
        Ok(value) => return Ok(object_inside_string(value)),
        Err(error) => error,
    };
    mended(input)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .map(object_inside_string)
        .ok_or(strict)
}

/// An object a model sent as a JSON string (`"{\"path\":\"a\"}"`) is that
/// object; every other value is itself.
fn object_inside_string(value: Value) -> Value {
    if let Value::String(text) = &value {
        if let Ok(inner @ Value::Object(_)) = serde_json::from_str::<Value>(text) {
            return inner;
        }
    }
    value
}

/// The input with its fence dropped, raw control characters inside strings
/// escaped and trailing commas removed — or `None` when nothing changed or
/// the input is cut off.
fn mended(input: &str) -> Option<String> {
    let text = without_code_fence(input.trim());
    let mut out = String::with_capacity(text.len() + 8);
    let mut in_string = false;
    let mut escaped = false;
    let mut depth: i64 = 0;
    for (at, ch) in text.char_indices() {
        if in_string {
            match ch {
                _ if escaped => {
                    escaped = false;
                    out.push(ch);
                }
                '\\' => {
                    escaped = true;
                    out.push(ch);
                }
                '"' => {
                    in_string = false;
                    out.push(ch);
                }
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                control if u32::from(control) < 0x20 => {
                    // Writing to a String cannot fail.
                    let _ = write!(out, "\\u{:04x}", u32::from(control));
                }
                other => out.push(other),
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            ',' if text[at + 1..].trim_start().starts_with(['}', ']']) => continue,
            _ => {}
        }
        out.push(ch);
    }
    let complete = !in_string && depth == 0;
    (complete && out != input).then_some(out)
}

fn without_code_fence(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let body = rest.split_once('\n').map_or(rest, |(_, body)| body);
    body.trim_end().strip_suffix("```").unwrap_or(body).trim()
}

/// How many lines a read skips for the `offset` a model gave it.
///
/// A model's `offset` is the line it starts at, counted from 1 as the lines
/// it is shown are, and `0` is the start too. It was read as a count of lines
/// to skip, so a model that asked for a file "from line 1" — `offset: 1` —
/// never saw the file's first line, in any file it read that way (zo 1.1.143,
/// 2026-11-12: a read asked with `offset: 1, limit: 100` showed `:2-101`).
/// Read as a line number, neither way of counting loses a line: a model that
/// pages by lines skipped reads one line twice.
///
/// Every reader of a read call's `offset` goes through here — the tool, the
/// row that names the lines read, the guard that knows a window was read
/// before — so the three agree on which lines a call means.
#[must_use]
pub fn read_start_index<N>(offset: N) -> N
where
    N: Copy + PartialOrd + From<u8> + std::ops::Sub<Output = N>,
{
    if offset > N::from(0) {
        offset - N::from(1)
    } else {
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::parse_tool_arguments;
    use serde_json::json;

    #[test]
    fn passes_valid_json_through_unchanged() {
        let parsed = parse_tool_arguments(r#"{"path":"a.rs","limit":3}"#).expect("valid");

        assert_eq!(parsed, json!({ "path": "a.rs", "limit": 3 }));
    }

    #[test]
    fn opens_an_object_sent_as_a_json_string() {
        let parsed = parse_tool_arguments(r#""{\"path\":\"a.rs\"}""#).expect("string-wrapped object");

        assert_eq!(parsed, json!({ "path": "a.rs" }));
    }

    #[test]
    fn keeps_a_plain_string_a_string() {
        let parsed = parse_tool_arguments(r#""just words""#).expect("a string");

        assert_eq!(parsed, json!("just words"));
    }

    #[test]
    fn drops_a_code_fence() {
        let parsed = parse_tool_arguments("```json\n{\"command\":\"ls\"}\n```").expect("fenced");

        assert_eq!(parsed, json!({ "command": "ls" }));
    }

    #[test]
    fn escapes_a_raw_newline_inside_a_string() {
        let parsed = parse_tool_arguments("{\"content\":\"line one\nline two\"}").expect("raw newline");

        assert_eq!(parsed, json!({ "content": "line one\nline two" }));
    }

    #[test]
    fn removes_trailing_commas() {
        let parsed = parse_tool_arguments(r#"{"items":[1,2,],"path":"a",}"#).expect("trailing commas");

        assert_eq!(parsed, json!({ "items": [1, 2], "path": "a" }));
    }

    #[test]
    fn keeps_a_comma_inside_a_string() {
        let parsed = parse_tool_arguments("{\"text\":\"a, }\nb\"}").expect("comma in string");

        assert_eq!(parsed, json!({ "text": "a, }\nb" }));
    }

    #[test]
    fn never_completes_a_cut_off_call() {
        let refused = parse_tool_arguments(r#"{"command":"rm -rf build"#);

        assert!(refused.is_err(), "{refused:?}");
    }

    /// A read's `offset` is a line number counted from 1, and 0 is the start
    /// too: the first line is skipped by neither.
    #[test]
    fn a_reads_offset_is_a_line_number_and_zero_is_the_start_too() {
        use super::read_start_index;
        assert_eq!(read_start_index(0usize), 0);
        assert_eq!(read_start_index(1usize), 0);
        assert_eq!(read_start_index(2usize), 1);
        assert_eq!(read_start_index(101u64), 100);
        assert_eq!(read_start_index(u64::MAX), u64::MAX - 1);
    }
}
