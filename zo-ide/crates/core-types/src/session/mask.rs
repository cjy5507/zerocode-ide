//! Taking the reasoning out of a session record's line before a tool hands it on.
//!
//! A session transcript, its vault and the copies of them are JSON Lines: one
//! record per line, and an assistant message's `blocks` hold the model's own
//! reasoning as `thinking` blocks (and, encrypted, `redacted_thinking`). A tool
//! that returns such a line to a model returns the reasoning with it — a
//! search for a phrase, a read of the file — and a provider's classifier reads
//! that reasoning as part of the next request (t-17474: the request that
//! carried a search of the person's own history was declined on Opus 5.5).
//!
//! `session_recall` already renders a message with [`THINKING_MARKER`] where it
//! reasoned; this is the same rule for a raw line, so the two doors cannot
//! drift apart.

use std::borrow::Cow;

use serde_json::Value;

/// What stands where a `thinking` block's text was.
pub const THINKING_MARKER: &str = "[thinking]";
/// What stands where a `redacted_thinking` block's data was.
pub const REDACTED_THINKING_MARKER: &str = "[redacted thinking]";
/// What stands for a line that carries reasoning and cannot be parsed to take it
/// out — a write cut short. Whole, because a torn line cannot be masked in
/// part.
pub const WITHHELD_LINE_MARKER: &str =
    "[record line withheld: it carries reasoning and cannot be masked]";

/// The shapes a reasoning block takes in JSON once its escapes are gone: a
/// `thinking` key with a string, or a block typed as one.
const REASONING_SHAPES: [&str; 6] = [
    r#""thinking":""#,
    r#""thinking": ""#,
    r#""type":"thinking""#,
    r#""type": "thinking""#,
    r#""type":"redacted_thinking""#,
    r#""type": "redacted_thinking""#,
];

/// `line` with the text of every `thinking` block and the data of every
/// `redacted_thinking` block replaced by its marker — nested as they are in a
/// record, and inside the string of a tool result that was shown a record —
/// and everything else exactly as it was.
///
/// A line that carries no reasoning comes back borrowed and byte for byte, and
/// so does one that only says the word. A line that carries a reasoning block
/// and does not parse is [`WITHHELD_LINE_MARKER`]: it cannot be masked in
/// part, and it must not be handed on.
#[must_use]
pub fn mask_thinking_in_record_line(line: &str) -> Cow<'_, str> {
    masked_record_line(line).map_or(Cow::Borrowed(line), Cow::Owned)
}

/// [`mask_thinking_in_record_line`] as a change: `Some` with the line as it is
/// handed on when reasoning was taken out of it, `None` when the line stands.
#[must_use]
pub fn masked_record_line(line: &str) -> Option<String> {
    // Nearly every line of a record has none of it in it; the word is the cheap
    // test, and the parse below is the exact one.
    if !line.contains("thinking") {
        return None;
    }
    if let Ok(mut record) = serde_json::from_str::<Value>(line) {
        return mask_value(&mut record).then(|| record.to_string());
    }
    // A search row is `path:12:{record}`: the record starts at a brace, and what
    // stands before it is the row's own.
    match masked_after_prefix(line) {
        Prefixed::Masked(masked) => return Some(masked),
        Prefixed::Unchanged => return None,
        Prefixed::NoRecord => {}
    }
    has_reasoning_shape(line).then(|| WITHHELD_LINE_MARKER.to_owned())
}

/// What became of a line that is a record behind a prefix.
enum Prefixed {
    /// A record followed the prefix, and reasoning was taken out of it.
    Masked(String),
    /// A record followed the prefix and carried none.
    Unchanged,
    /// No record follows any prefix of it.
    NoRecord,
}

/// How many braces of a line are tried as the start of its record: a search
/// row's record is behind the first, and a long line must not be parsed once
/// per brace it holds.
const RECORD_START_CANDIDATES: usize = 4;

fn masked_after_prefix(line: &str) -> Prefixed {
    for (start, _) in line.match_indices('{').take(RECORD_START_CANDIDATES) {
        let (prefix, tail) = line.split_at(start);
        // Reasoning ahead of the record would ride out with the prefix.
        if has_reasoning_shape(prefix) {
            return Prefixed::NoRecord;
        }
        if let Ok(mut record) = serde_json::from_str::<Value>(tail) {
            return if mask_value(&mut record) {
                Prefixed::Masked(format!("{prefix}{record}"))
            } else {
                Prefixed::Unchanged
            };
        }
    }
    Prefixed::NoRecord
}

/// Whether `text` spells a reasoning block, however deeply its quotes were
/// escaped on the way into the string that holds it.
fn has_reasoning_shape(text: &str) -> bool {
    let flat: Cow<'_, str> = if text.contains('\\') {
        Cow::Owned(text.replace('\\', ""))
    } else {
        Cow::Borrowed(text)
    };
    REASONING_SHAPES.iter().any(|shape| flat.contains(shape))
}

/// Mask `value` in place; whether anything was masked.
fn mask_value(value: &mut Value) -> bool {
    match value {
        Value::Object(object) => {
            let mut masked = false;
            let kind = object.get("type").and_then(Value::as_str).map(str::to_owned);
            match kind.as_deref() {
                Some("thinking") => {
                    masked |= replace_string(object.get_mut("thinking"), THINKING_MARKER);
                    // The signature is bound to the text it signs and means
                    // nothing without it.
                    if masked {
                        replace_string(object.get_mut("signature"), "");
                    }
                }
                Some("redacted_thinking") => {
                    masked |= replace_string(object.get_mut("data"), REDACTED_THINKING_MARKER);
                }
                _ => {}
            }
            for child in object.values_mut() {
                masked |= mask_value(child);
            }
            masked
        }
        Value::Array(items) => {
            let mut masked = false;
            for item in items {
                masked |= mask_value(item);
            }
            masked
        }
        Value::String(text) => match mask_embedded_lines(text) {
            Some(rewritten) => {
                *text = rewritten;
                true
            }
            None => false,
        },
        _ => false,
    }
}

/// Overwrite a string field with `replacement`; whether there was one.
fn replace_string(field: Option<&mut Value>, replacement: &str) -> bool {
    match field {
        Some(slot) if slot.is_string() => {
            *slot = Value::String(replacement.to_owned());
            true
        }
        _ => false,
    }
}

/// A string value that holds record lines — a tool result shown a session file
/// carries them — masked line by line; `None` when nothing in it changed.
fn mask_embedded_lines(text: &str) -> Option<String> {
    if !text.contains("thinking") {
        return None;
    }
    let mut changed = false;
    let lines: Vec<Cow<'_, str>> = text
        .split('\n')
        .map(|line| match masked_record_line(line) {
            Some(masked) => {
                changed = true;
                Cow::Owned(masked)
            }
            None => Cow::Borrowed(line),
        })
        .collect();
    changed.then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::{
        mask_thinking_in_record_line, REDACTED_THINKING_MARKER, THINKING_MARKER,
        WITHHELD_LINE_MARKER,
    };
    use crate::session::{ContentBlock, ConversationMessage};
    use std::borrow::Cow;

    const SENTINEL: &str = "SENTINEL-REASONING-1207";

    /// A transcript line written by the session's own writer.
    fn record_line(blocks: Vec<ContentBlock>) -> String {
        format!(
            r#"{{"message":{},"turn_index":2,"type":"message"}}"#,
            ConversationMessage::assistant(blocks).to_json().render()
        )
    }

    fn reasoning(text: &str) -> ContentBlock {
        ContentBlock::Thinking {
            thinking: text.to_string(),
            signature: "sig-abc".to_string(),
        }
    }

    fn said(text: &str) -> ContentBlock {
        ContentBlock::Text {
            text: text.to_string(),
        }
    }

    #[test]
    fn a_thinking_block_written_by_the_session_loses_its_text_and_keeps_the_words() {
        let line = record_line(vec![reasoning(SENTINEL), said("Hello")]);
        assert!(line.contains(SENTINEL), "premise: the writer stores the reasoning: {line}");
        let masked = mask_thinking_in_record_line(&line);
        assert!(!masked.contains(SENTINEL), "{masked}");
        assert!(masked.contains(THINKING_MARKER) && masked.contains("Hello"), "{masked}");
        assert!(!masked.contains("sig-abc"), "the signature goes with the text: {masked}");
    }

    #[test]
    fn a_redacted_thinking_block_loses_its_data() {
        let line = record_line(vec![
            ContentBlock::RedactedThinking {
                data: format!("{SENTINEL}-opaque"),
            },
            said("Hello"),
        ]);
        let masked = mask_thinking_in_record_line(&line);
        assert!(!masked.contains(SENTINEL), "{masked}");
        assert!(masked.contains(REDACTED_THINKING_MARKER), "{masked}");
    }

    /// A tool result that was shown a record line holds it in a string, quotes
    /// escaped; a second search finds the first search's result. The reasoning
    /// inside is masked at any depth.
    #[test]
    fn reasoning_inside_a_tool_result_that_was_shown_a_record_is_masked() {
        let inner = record_line(vec![reasoning(SENTINEL), said("inner")]);
        let outer_result = ContentBlock::ToolResult {
            tool_use_id: "t1".to_string(),
            tool_name: "grep_search".to_string(),
            output: format!("/x/session.jsonl:3:{inner}\n/x/other.jsonl:9:no match here"),
            is_error: false,
            images: Vec::new(),
        };
        let outer = record_line(vec![outer_result]);
        assert!(outer.contains(SENTINEL), "premise: the reasoning sits in the string: {outer}");
        let masked = mask_thinking_in_record_line(&outer);
        assert!(!masked.contains(SENTINEL), "{masked}");
        assert!(masked.contains("no match here") && masked.contains("inner"), "{masked}");
        let twice = record_line(vec![ContentBlock::ToolResult {
            tool_use_id: "t2".to_string(),
            tool_name: "grep_search".to_string(),
            output: format!("/x/session.jsonl:4:{outer}"),
            is_error: false,
            images: Vec::new(),
        }]);
        let twice = mask_thinking_in_record_line(&twice);
        assert!(!twice.contains(SENTINEL), "{twice}");
        assert!(twice.contains("inner"), "the words said survive every depth: {twice}");
    }

    /// A search row is `path:12:{record}`: the record behind the prefix is
    /// masked and the prefix is the row's own.
    #[test]
    fn a_search_row_keeps_its_prefix_and_loses_the_reasoning_behind_it() {
        let row = format!(
            "/x/session.jsonl:3:{}",
            record_line(vec![reasoning(SENTINEL), said("Hello")])
        );
        let masked = mask_thinking_in_record_line(&row);
        assert!(masked.starts_with("/x/session.jsonl:3:{"), "{masked}");
        assert!(!masked.contains(SENTINEL) && masked.contains("Hello"), "{masked}");

        // Reasoning ahead of the record would ride out with the prefix, so a
        // row that has some there is withheld whole.
        let ahead = format!(
            r#"{{"thinking":"{SENTINEL}"}} {}"#,
            record_line(vec![said("Hello")])
        );
        assert_eq!(mask_thinking_in_record_line(&ahead), WITHHELD_LINE_MARKER);
    }

    /// Only a line that loses reasoning is re-written: the rest come back as
    /// they were, keys in the order they were written.
    #[test]
    fn a_line_without_reasoning_comes_back_borrowed_and_byte_for_byte() {
        for line in [
            r#"{"z":1,"a":"thinking about lunch","type":"note"}"#,
            r#"{"message":{"blocks":[{"text":"no reasoning here","type":"text"}],"role":"user"},"type":"message"}"#,
            "plain words about thinking",
            "",
        ] {
            assert!(
                matches!(mask_thinking_in_record_line(line), Cow::Borrowed(same) if same == line),
                "{line}"
            );
        }
        // Code that names the block type is not a record.
        let code = r#"            "thinking" => Ok(Self::Thinking {"#;
        assert!(matches!(mask_thinking_in_record_line(code), Cow::Borrowed(same) if same == code));
    }

    #[test]
    fn a_torn_line_that_carries_a_reasoning_block_is_withheld() {
        let line = record_line(vec![reasoning(SENTINEL), said("Hello")]);
        let torn = &line[..line.len() - 12];
        assert_eq!(mask_thinking_in_record_line(torn), WITHHELD_LINE_MARKER);
    }
}
