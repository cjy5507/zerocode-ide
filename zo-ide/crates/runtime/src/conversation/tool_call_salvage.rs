//! Salvage of tool-call markup that a model leaked as plain assistant text.
//!
//! Some Claude responses emit the internal tool-call markup — an `invoke`
//! tag wrapping `parameter` tags, usually with the `antml:` namespace prefix
//! stripped — as literal text instead of a native `tool_use` block. The turn
//! then ends as text, the intended tool never runs, and the user sees a wall
//! of raw JSON. A stray residue line (for example `count`) sometimes precedes
//! the tags, a fragment of the mangled tag stream.
//!
//! [`salvage_leaked_tool_calls`] detects that shape once the stream has been
//! reduced to content blocks and re-materializes the calls as synthetic
//! `ToolUse` blocks so the normal dispatch path executes them (an unknown
//! tool name simply produces a normal `tool_result` error the model can react
//! to). Guards keep documentation or quoted examples from being mistaken for
//! real calls: the markup must be complete (balanced open/close tags), must
//! sit outside any fenced code block, and must be the last non-whitespace
//! content of the text block — a leak is an attempted call, so it always ends
//! the text, while a mid-text appearance is prose about a call.
//!
//! All tag strings are assembled at runtime (never written as complete
//! literals) so this source file can never itself be mistaken for leaked
//! markup by tooling that scans for it.

use serde_json::Value;

use crate::session::ContentBlock;

/// Namespace prefix the model-side markup carries. Leaks usually arrive with
/// it stripped, but both spellings are accepted.
const NS: &str = "antml:";

/// One tool call recovered from leaked markup.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct SalvagedToolCall {
    pub(super) name: String,
    /// JSON-encoded input object (`{"param": value, …}`), the same string
    /// shape a native `ToolUse` block carries.
    pub(super) input: String,
}

/// Result of successfully salvaging trailing tool-call markup from a text
/// block.
#[derive(Debug)]
pub(super) struct ToolCallSalvage {
    /// The text with the markup (and any stray residue line directly above
    /// it) removed. Empty when the whole block was markup.
    pub(super) cleaned_text: String,
    /// Recovered calls, in the order they appeared. Never empty.
    pub(super) calls: Vec<SalvagedToolCall>,
}

/// Scan the last `Text` block of a finished assistant message for leaked
/// trailing tool-call markup; on a hit, rewrite the text (or drop the block
/// when it was all markup) and insert synthetic `ToolUse` blocks right after
/// it so the normal dispatch path executes the calls. A message that already
/// carries native `tool_use` blocks is still salvaged — trailing complete
/// markup is an attempted call even in a mixed message.
pub(super) fn salvage_leaked_tool_calls(blocks: &mut Vec<ContentBlock>) {
    let Some(text_index) = blocks
        .iter()
        .rposition(|block| matches!(block, ContentBlock::Text { .. }))
    else {
        return;
    };
    let ContentBlock::Text { text } = &blocks[text_index] else {
        return;
    };
    let Some(salvage) = salvage_trailing_tool_call_markup(text) else {
        return;
    };

    // Replace the text block with the cleaned prose (dropped when the whole
    // block was markup) followed by the synthetic tool_use blocks.
    let mut replacement = Vec::with_capacity(salvage.calls.len() + 1);
    if !salvage.cleaned_text.is_empty() {
        replacement.push(ContentBlock::Text {
            text: salvage.cleaned_text,
        });
    }
    replacement.extend(salvage.calls.into_iter().enumerate().map(|(index, call)| {
        // Fold the call content into the id so two salvaged calls for
        // different tools never share an id across the session. A conversation
        // -global consumer (Gemini's tool_use id->name map, which recovers a
        // functionResponse's name from the matching call) would otherwise
        // last-wins-collide on a fixed `salvaged-toolcall-1`, mispairing the
        // result after a provider swap. Identical calls sharing an id is
        // harmless — they resolve to the same name.
        let id = format!(
            "salvaged-toolcall-{}-{:08x}",
            index + 1,
            content_fingerprint(&call.name, &call.input)
        );
        eprintln!(
            "[zo] warning: model leaked a tool call as text; salvaged `{}` as synthetic tool_use {id}",
            call.name,
        );
        ContentBlock::ToolUse {
            id,
            name: call.name,
            input: call.input,
        }
    }));
    blocks.splice(text_index..=text_index, replacement);
}

/// Parse trailing leaked tool-call markup out of `text`.
///
/// Returns `None` unless every guard holds: at least one complete `invoke`
/// block exists outside any code fence, and the trailing run of such blocks
/// (optionally wrapped in `function_calls` tags) is the final non-whitespace
/// content of the text. Parameter values that parse as a complete JSON
/// document are kept structurally; anything else becomes a trimmed string.
#[must_use]
pub(super) fn salvage_trailing_tool_call_markup(text: &str) -> Option<ToolCallSalvage> {
    let tags = Tags::new();
    let content = text.trim_end();
    if content.is_empty() {
        return None;
    }

    // Cheap early-out: a leaked call always ends the text with a closing tag.
    if !tags
        .invoke_close
        .iter()
        .chain(tags.wrapper_close.iter())
        .any(|tag| content.ends_with(tag.as_str()))
    {
        return None;
    }

    // Peel an optional trailing wrapper closer so the last invoke block is
    // expected to end at `effective_end`.
    let mut effective_end = content.len();
    for closer in &tags.wrapper_close {
        if let Some(without) = content.strip_suffix(closer.as_str()) {
            effective_end = without.trim_end().len();
            break;
        }
    }

    let fences = code_fence_ranges(text);
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while let Some((start, name_start)) = find_next_open(text, cursor, &tags.invoke_open) {
        if start >= effective_end {
            break;
        }
        // A real leak is emitted flush against the left margin. Indentation
        // (a 4-space markdown code block) or a `>` blockquote marker before
        // the tag means a quoted example, not a call — never execute those.
        if in_fence(&fences, start) || !at_line_start(text, start) {
            cursor = start + 1;
            continue;
        }
        if let Some(block) = parse_invoke_block(text, &tags, start, name_start) {
            cursor = block.end;
            blocks.push(block);
        } else {
            cursor = start + 1;
        }
    }

    // Guard: the last complete block must be the final non-whitespace content.
    if blocks.last()?.end != effective_end {
        return None;
    }

    // Extend the salvaged run backwards over whitespace-separated blocks;
    // earlier blocks with prose after them stay untouched (quoted examples).
    let mut run_start = blocks.len() - 1;
    while run_start > 0
        && text[blocks[run_start - 1].end..blocks[run_start].start]
            .trim()
            .is_empty()
    {
        run_start -= 1;
    }

    // Include an optional wrapper opener directly above the run.
    let mut removal_start = blocks[run_start].start;
    let prefix = text[..removal_start].trim_end();
    for opener in &tags.wrapper_open {
        if prefix.ends_with(opener.as_str()) {
            removal_start = prefix.len() - opener.len();
            break;
        }
    }

    // Drop a stray residue line (for example `count`) directly above the
    // markup — a fragment of the mangled tag stream, not prose.
    let mut cleaned = text[..removal_start].trim_end();
    let line_start = cleaned.rfind('\n').map_or(0, |pos| pos + 1);
    if is_stray_residue_line(&cleaned[line_start..]) {
        cleaned = cleaned[..line_start].trim_end();
    }

    let calls: Vec<SalvagedToolCall> = blocks
        .drain(run_start..)
        .map(|block| SalvagedToolCall {
            name: block.name,
            input: Value::Object(block.params.into_iter().collect()).to_string(),
        })
        .collect();

    Some(ToolCallSalvage {
        cleaned_text: cleaned.to_string(),
        calls,
    })
}

/// Both accepted spellings (bare and namespace-prefixed) of every tag the
/// parser needs, assembled once per salvage attempt.
struct Tags {
    invoke_open: [String; 2],
    invoke_close: [String; 2],
    param_open: [String; 2],
    param_close: [String; 2],
    wrapper_open: [String; 2],
    wrapper_close: [String; 2],
}

impl Tags {
    fn new() -> Self {
        Self {
            invoke_open: open_prefixes("invoke"),
            invoke_close: close_tags("invoke"),
            param_open: open_prefixes("parameter"),
            param_close: close_tags("parameter"),
            wrapper_open: bare_tags("function_calls"),
            wrapper_close: close_tags("function_calls"),
        }
    }
}

/// The attribute-carrying opening prefix of `tag`, up to and including the
/// opening quote of its `name` attribute.
fn open_prefixes(tag: &str) -> [String; 2] {
    [format!("<{tag} name=\""), format!("<{NS}{tag} name=\"")]
}

fn close_tags(tag: &str) -> [String; 2] {
    [format!("</{tag}>"), format!("</{NS}{tag}>")]
}

fn bare_tags(tag: &str) -> [String; 2] {
    [format!("<{tag}>"), format!("<{NS}{tag}>")]
}

/// One complete `invoke` block found in the text.
struct InvokeBlock {
    /// Byte offset of the opening tag.
    start: usize,
    /// Byte offset just past the closing tag.
    end: usize,
    name: String,
    params: Vec<(String, Value)>,
}

/// Parse one complete `invoke` block starting at `start` (with the tool name
/// beginning at `name_start`). Returns `None` on any malformation — a
/// truncated block, a parameter missing its closing tag, or unexpected
/// content between parameters.
fn parse_invoke_block(text: &str, tags: &Tags, start: usize, name_start: usize) -> Option<InvokeBlock> {
    let (name, mut cursor) = parse_quoted_name(text, name_start)?;
    let mut params = Vec::new();
    loop {
        cursor = skip_whitespace(text, cursor);
        let rest = &text[cursor..];
        if let Some(close_len) = starts_with_any(rest, &tags.invoke_close) {
            return Some(InvokeBlock {
                start,
                end: cursor + close_len,
                name,
                params,
            });
        }
        let open_len = starts_with_any(rest, &tags.param_open)?;
        let (param_name, value_start) = parse_quoted_name(text, cursor + open_len)?;
        let (close_rel, close_len) = find_first_any(&text[value_start..], &tags.param_close)?;
        let raw_value = &text[value_start..value_start + close_rel];
        params.push((param_name, parse_parameter_value(raw_value)));
        cursor = value_start + close_rel + close_len;
    }
}

/// Read a quoted attribute value starting at `name_start` (just past the
/// opening quote) and the `>` that closes the opening tag. Returns the name
/// and the offset just past that `>`.
fn parse_quoted_name(text: &str, name_start: usize) -> Option<(String, usize)> {
    let quote_rel = text[name_start..].find('"')?;
    let name = &text[name_start..name_start + quote_rel];
    if name.is_empty() || name.contains(['<', '>', '\n']) {
        return None;
    }
    let cursor = skip_whitespace(text, name_start + quote_rel + 1);
    if !text[cursor..].starts_with('>') {
        return None;
    }
    Some((name.to_string(), cursor + 1))
}

/// A parameter value that parses as one complete JSON document is stored
/// structurally (objects, arrays, numbers, booleans survive verbatim);
/// anything else is kept as a trimmed string.
fn parse_parameter_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    serde_json::from_str::<Value>(trimmed).unwrap_or_else(|_| Value::String(trimmed.to_string()))
}

/// Known residue tokens that the mangled tag stream leaves on their own line
/// directly above the markup (`count` seen in the wild; `call` is a
/// `function_calls` fragment). Deliberately a fixed allowlist rather than a
/// shape heuristic: eating any lone ASCII word would silently drop a
/// legitimate one-word final line of prose. Add tokens here as new residue
/// shapes are observed.
const RESIDUE_TOKENS: &[&str] = &["count", "call"];

/// Whether `line` is exactly one of the known stray-residue tokens.
fn is_stray_residue_line(line: &str) -> bool {
    RESIDUE_TOKENS.contains(&line.trim())
}

/// Order-independent fingerprint of a salvaged call's identity (FNV-1a over
/// name and input), used only to keep synthetic `tool_use` ids unique.
fn content_fingerprint(name: &str, input: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.bytes().chain(b"\0".iter().copied()).chain(input.bytes()) {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Byte ranges of fenced code blocks (` ``` ` line toggles). An unclosed
/// fence swallows the rest of the text.
fn code_fence_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut fence_start: Option<usize> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        if line.trim_start().starts_with("```") {
            match fence_start.take() {
                Some(start) => ranges.push((start, offset)),
                None => fence_start = Some(line_start),
            }
        }
    }
    if let Some(start) = fence_start {
        ranges.push((start, text.len()));
    }
    ranges
}

fn in_fence(fences: &[(usize, usize)], at: usize) -> bool {
    fences.iter().any(|&(start, end)| at >= start && at < end)
}

/// Whether the tag at `start` begins its line. Leaked markup is flush against
/// the left margin; any prefix (a 4-space code-block indent, a `>` blockquote
/// marker) means a quoted example that must never be executed.
fn at_line_start(text: &str, start: usize) -> bool {
    let line_start = text[..start].rfind('\n').map_or(0, |pos| pos + 1);
    text[line_start..start].is_empty()
}

/// Earliest match of either opening-prefix spelling at or after `from`.
/// Returns `(tag_start, name_start)`.
fn find_next_open(text: &str, from: usize, opens: &[String; 2]) -> Option<(usize, usize)> {
    opens
        .iter()
        .filter_map(|open| {
            text[from..]
                .find(open.as_str())
                .map(|pos| (from + pos, from + pos + open.len()))
        })
        .min_by_key(|&(start, _)| start)
}

/// Earliest match of either spelling anywhere in `text`: `(position, length)`.
fn find_first_any(text: &str, tags: &[String; 2]) -> Option<(usize, usize)> {
    tags.iter()
        .filter_map(|tag| text.find(tag.as_str()).map(|pos| (pos, tag.len())))
        .min_by_key(|&(pos, _)| pos)
}

/// Length of the matching spelling when `text` starts with one of `tags`.
fn starts_with_any(text: &str, tags: &[String; 2]) -> Option<usize> {
    tags.iter()
        .find(|tag| text.starts_with(tag.as_str()))
        .map(String::len)
}

fn skip_whitespace(text: &str, at: usize) -> usize {
    text.len() - text[at..].trim_start().len()
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{salvage_trailing_tool_call_markup, NS};
    use crate::conversation::{AssistantEvent, AssistantTurn};
    use crate::session::ContentBlock;

    /// The spec payload from the live artifact this module was written for
    /// (an assistant turn that ended in leaked `Workflow` markup).
    const SPEC_JSON: &str =
        r#"{"budget": {"max_agents": 8}, "phases": [{"id": "p1", "goal": "P1 DB pool 운영 튜닝"}]}"#;

    const ARTIFACT_PROSE: &str = "분석 완료. 운영 튜닝 필요(P1 DB pool과 연결)";

    // Fixture builders assemble the markup at runtime so this file never
    // contains a complete literal tag.
    fn open_tag(tag: &str, name: &str) -> String {
        format!("<{tag} name=\"{name}\">")
    }

    fn close_tag(tag: &str) -> String {
        format!("</{tag}>")
    }

    fn invoke_block(name: &str, params: &[(&str, &str)]) -> String {
        let mut out = open_tag("invoke", name);
        for (param_name, value) in params {
            out.push('\n');
            out.push_str(&open_tag("parameter", param_name));
            out.push_str(value);
            out.push_str(&close_tag("parameter"));
        }
        out.push('\n');
        out.push_str(&close_tag("invoke"));
        out
    }

    /// The real leak shape: prose, a blank line, a stray `count` residue
    /// line, then the complete markup as the final content.
    fn artifact_text() -> String {
        format!(
            "{ARTIFACT_PROSE}\n\ncount\n{}",
            invoke_block("Workflow", &[("spec", SPEC_JSON)])
        )
    }

    #[test]
    fn salvages_real_workflow_artifact_with_residue_line() {
        let salvage = salvage_trailing_tool_call_markup(&artifact_text()).expect("must salvage");
        assert_eq!(salvage.cleaned_text, ARTIFACT_PROSE, "prose preserved, residue line gone");
        assert_eq!(salvage.calls.len(), 1);
        assert_eq!(salvage.calls[0].name, "Workflow");
        let input: Value = serde_json::from_str(&salvage.calls[0].input).expect("input is JSON");
        let expected_spec: Value = serde_json::from_str(SPEC_JSON).expect("fixture JSON");
        assert_eq!(input, json!({ "spec": expected_spec }), "spec JSON restored verbatim");
    }

    #[test]
    fn multiple_parameters_build_one_input_object() {
        let text = format!(
            "파일을 읽습니다.\n{}",
            invoke_block("read_file", &[("path", "/tmp/a.rs"), ("limit", "120")])
        );
        let salvage = salvage_trailing_tool_call_markup(&text).expect("must salvage");
        let input: Value = serde_json::from_str(&salvage.calls[0].input).expect("input is JSON");
        // Non-JSON value stays a string; a bare number is kept structurally.
        assert_eq!(input, json!({ "path": "/tmp/a.rs", "limit": 120 }));
    }

    #[test]
    fn back_to_back_invoke_blocks_are_all_salvaged_in_order() {
        let text = format!(
            "두 파일을 읽습니다.\n{}\n\n{}",
            invoke_block("read_file", &[("path", "/tmp/a.rs")]),
            invoke_block("read_file", &[("path", "/tmp/b.rs")])
        );
        let salvage = salvage_trailing_tool_call_markup(&text).expect("must salvage");
        assert_eq!(salvage.cleaned_text, "두 파일을 읽습니다.");
        assert_eq!(salvage.calls.len(), 2);
        let first: Value = serde_json::from_str(&salvage.calls[0].input).expect("json");
        let second: Value = serde_json::from_str(&salvage.calls[1].input).expect("json");
        assert_eq!(first, json!({ "path": "/tmp/a.rs" }));
        assert_eq!(second, json!({ "path": "/tmp/b.rs" }));
    }

    #[test]
    fn namespaced_markup_with_wrapper_tags_is_salvaged() {
        let ns = NS;
        let text = format!(
            "진행합니다.\n<{ns}function_calls>\n<{ns}invoke name=\"read_file\">\n<{ns}parameter name=\"path\">/tmp/a.rs</{ns}parameter>\n</{ns}invoke>\n</{ns}function_calls>"
        );
        let salvage = salvage_trailing_tool_call_markup(&text).expect("must salvage");
        assert_eq!(salvage.cleaned_text, "진행합니다.");
        assert_eq!(salvage.calls[0].name, "read_file");
        let input: Value = serde_json::from_str(&salvage.calls[0].input).expect("json");
        assert_eq!(input, json!({ "path": "/tmp/a.rs" }));
    }

    #[test]
    fn bare_wrapper_tags_are_salvaged() {
        let text = format!(
            "진행합니다.\n{}\n{}\n{}",
            format_args!("<{}>", "function_calls"),
            invoke_block("read_file", &[("path", "/tmp/a.rs")]),
            close_tag("function_calls")
        );
        let salvage = salvage_trailing_tool_call_markup(&text).expect("must salvage");
        assert_eq!(salvage.cleaned_text, "진행합니다.");
        assert_eq!(salvage.calls.len(), 1);
    }

    #[test]
    fn markup_inside_closed_code_fence_is_not_salvaged() {
        let text = format!(
            "마크업 예시입니다:\n```\n{}\n```",
            invoke_block("Workflow", &[("spec", "{}")])
        );
        assert!(salvage_trailing_tool_call_markup(&text).is_none());
    }

    #[test]
    fn markup_inside_unclosed_code_fence_is_not_salvaged() {
        // Ends with the closing tag, so only the fence guard rejects it.
        let text = format!(
            "마크업 예시입니다:\n```\n{}",
            invoke_block("Workflow", &[("spec", "{}")])
        );
        assert!(salvage_trailing_tool_call_markup(&text).is_none());
    }

    #[test]
    fn indented_markup_example_is_not_salvaged() {
        // A 4-space markdown code block quoting the markup, as the final
        // content: must not be promoted to a real call.
        let block = invoke_block("bash", &[("command", "rm -rf build")]);
        let mut indented = String::new();
        for line in block.lines() {
            indented.push_str("    ");
            indented.push_str(line);
            indented.push('\n');
        }
        let text = format!("레거시 마크업 형식 예시:\n\n{}", indented.trim_end());
        assert!(salvage_trailing_tool_call_markup(&text).is_none());
    }

    #[test]
    fn blockquoted_markup_example_is_not_salvaged() {
        let text = format!(
            "예시:\n> {}",
            invoke_block("bash", &[("command", "rm -rf build")]).replace('\n', "\n> ")
        );
        assert!(salvage_trailing_tool_call_markup(&text).is_none());
    }

    #[test]
    fn different_tools_get_distinct_synthetic_ids() {
        // Two salvaged calls with different content must not collide on a
        // fixed id (Gemini's global id->name map would mispair after a swap).
        let a = build(vec![
            AssistantEvent::TextDelta(format!(
                "본문.\n{}",
                invoke_block("Workflow", &[("spec", SPEC_JSON)])
            )),
            AssistantEvent::MessageStop,
        ]);
        let b = build(vec![
            AssistantEvent::TextDelta(format!(
                "본문.\n{}",
                invoke_block("read_file", &[("path", "/tmp/a.rs")])
            )),
            AssistantEvent::MessageStop,
        ]);
        let id_of = |m: &crate::session::ConversationMessage| match m.blocks.last().unwrap() {
            ContentBlock::ToolUse { id, .. } => id.clone(),
            other => panic!("expected tool_use, got {other:?}"),
        };
        assert_ne!(id_of(&a), id_of(&b), "distinct content must yield distinct ids");
    }

    #[test]
    fn markup_followed_by_prose_is_not_salvaged() {
        let text = format!(
            "{}\n\n위와 같은 형식으로 호출됩니다.",
            invoke_block("Workflow", &[("spec", "{}")])
        );
        assert!(salvage_trailing_tool_call_markup(&text).is_none());
    }

    #[test]
    fn quoted_example_earlier_in_text_does_not_join_the_trailing_run() {
        let text = format!(
            "{}\n\n예시는 위와 같고, 실제 호출은 아래입니다.\n{}",
            invoke_block("read_file", &[("path", "/tmp/example.rs")]),
            invoke_block("read_file", &[("path", "/tmp/real.rs")])
        );
        let salvage = salvage_trailing_tool_call_markup(&text).expect("must salvage");
        // Only the trailing block is a call; the earlier one stays as text.
        assert_eq!(salvage.calls.len(), 1);
        let input: Value = serde_json::from_str(&salvage.calls[0].input).expect("json");
        assert_eq!(input, json!({ "path": "/tmp/real.rs" }));
        assert!(salvage.cleaned_text.contains("/tmp/example.rs"));
        assert!(salvage.cleaned_text.ends_with("실제 호출은 아래입니다."));
    }

    #[test]
    fn unbalanced_markup_is_not_salvaged() {
        // Parameter never closed, invoke closed: parse fails on the block.
        let unclosed_param = format!(
            "호출:\n{}\n{}/tmp/x\n{}",
            open_tag("invoke", "read_file"),
            open_tag("parameter", "path"),
            close_tag("invoke")
        );
        assert!(salvage_trailing_tool_call_markup(&unclosed_param).is_none());

        // Invoke never closed: text does not even end with a closing tag.
        let unclosed_invoke = format!(
            "호출:\n{}\n{}/tmp/x{}",
            open_tag("invoke", "read_file"),
            open_tag("parameter", "path"),
            close_tag("parameter")
        );
        assert!(salvage_trailing_tool_call_markup(&unclosed_invoke).is_none());
    }

    #[test]
    fn plain_prose_line_above_markup_is_preserved() {
        let text = format!(
            "마지막 줄은 실제 문장입니다.\n{}",
            invoke_block("read_file", &[("path", "/tmp/a.rs")])
        );
        let salvage = salvage_trailing_tool_call_markup(&text).expect("must salvage");
        assert_eq!(salvage.cleaned_text, "마지막 줄은 실제 문장입니다.");
    }

    #[test]
    fn all_markup_text_yields_empty_cleaned_text() {
        let text = invoke_block("Workflow", &[("spec", SPEC_JSON)]);
        let salvage = salvage_trailing_tool_call_markup(&text).expect("must salvage");
        assert!(salvage.cleaned_text.is_empty());
        assert_eq!(salvage.calls.len(), 1);
    }

    #[test]
    fn zero_parameter_invoke_salvages_with_empty_input_object() {
        let text = format!(
            "확인합니다.\n{}{}",
            open_tag("invoke", "list_files"),
            close_tag("invoke")
        );
        let salvage = salvage_trailing_tool_call_markup(&text).expect("must salvage");
        assert_eq!(salvage.calls[0].input, "{}");
    }

    // --- build_assistant_message integration -----------------------------

    fn build(events: Vec<AssistantEvent>) -> crate::session::ConversationMessage {
        match super::super::helpers::build_assistant_message(events) {
            AssistantTurn::Content { message, .. } => message,
            AssistantTurn::Empty { .. } => panic!("expected assistant content"),
        }
    }

    #[test]
    fn build_assistant_message_salvages_leaked_workflow_call() {
        let message = build(vec![
            AssistantEvent::TextDelta(artifact_text()),
            AssistantEvent::MessageStop,
        ]);
        assert_eq!(message.blocks.len(), 2, "{:?}", message.blocks);
        assert!(matches!(
            &message.blocks[0],
            ContentBlock::Text { text } if text == ARTIFACT_PROSE
        ));
        match &message.blocks[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert!(id.starts_with("salvaged-toolcall-1-"), "unexpected id {id}");
                assert_eq!(name, "Workflow");
                let parsed: Value = serde_json::from_str(input).expect("input is JSON");
                assert_eq!(parsed["spec"]["budget"]["max_agents"], json!(8));
            }
            other => panic!("expected salvaged tool_use, got {other:?}"),
        }
    }

    #[test]
    fn build_assistant_message_replaces_all_markup_text_block() {
        let message = build(vec![
            AssistantEvent::TextDelta(invoke_block("Workflow", &[("spec", SPEC_JSON)])),
            AssistantEvent::MessageStop,
        ]);
        assert_eq!(message.blocks.len(), 1, "{:?}", message.blocks);
        assert!(matches!(
            &message.blocks[0],
            ContentBlock::ToolUse { name, .. } if name == "Workflow"
        ));
    }

    #[test]
    fn build_assistant_message_salvages_alongside_native_tool_use() {
        let message = build(vec![
            AssistantEvent::TextDelta(format!(
                "본문.\n{}",
                invoke_block("read_file", &[("path", "/tmp/a.rs")])
            )),
            AssistantEvent::ToolUse {
                id: "toolu_native".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            },
            AssistantEvent::MessageStop,
        ]);
        assert_eq!(message.blocks.len(), 3, "{:?}", message.blocks);
        assert!(matches!(
            &message.blocks[0],
            ContentBlock::Text { text } if text == "본문."
        ));
        assert!(matches!(
            &message.blocks[1],
            ContentBlock::ToolUse { id, name, .. }
                if id.starts_with("salvaged-toolcall-1-") && name == "read_file"
        ));
        assert!(matches!(
            &message.blocks[2],
            ContentBlock::ToolUse { id, .. } if id == "toolu_native"
        ));
    }

    #[test]
    fn build_assistant_message_leaves_plain_text_untouched() {
        let message = build(vec![
            AssistantEvent::TextDelta("그냥 텍스트 답변입니다.".to_string()),
            AssistantEvent::MessageStop,
        ]);
        assert_eq!(message.blocks.len(), 1);
        assert!(matches!(
            &message.blocks[0],
            ContentBlock::Text { text } if text == "그냥 텍스트 답변입니다."
        ));
    }
}
