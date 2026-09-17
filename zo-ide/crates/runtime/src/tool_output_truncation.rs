//! Unified middleware for truncating tool output to a safe character limit.
//!
//! Uses [`std::fmt::Write`] internally; the trait is not re-exported.
//!
//! This module provides a single truncation pass applied to all tool results
//! before they are returned to the conversation loop.  Individual tools may
//! impose tighter limits on their own output (e.g. `bash.rs` caps at 16 KiB),
//! but this layer enforces the global ceiling of ~30 000 chars that matches the
//! Python reference implementation.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::bash::BashCommandOutput;
use crate::context_compression::compact_bash_stream;
use crate::file_ops::ReadFileOutput;

/// Configuration for the output-truncation middleware.
#[derive(Debug, Clone)]
pub struct TruncationConfig {
    /// Global character limit applied to all tools that have no override.
    pub default_max_chars: usize,
    /// Per-tool overrides keyed by tool name (e.g. `"bash"`, `"read_file"`).
    pub tool_overrides: HashMap<String, usize>,
}

impl TruncationConfig {
    /// The per-tool override, matched exactly and then case-insensitively:
    /// the live dispatch path canonicalises names before asking, so a raw
    /// model-provided `"Bash"` still finds the `bash` cap.
    #[must_use]
    pub fn explicit_override(&self, tool_name: &str) -> Option<usize> {
        self.tool_overrides
            .get(tool_name)
            .or_else(|| self.tool_overrides.get(&tool_name.to_lowercase()))
            .copied()
    }

    /// The character ceiling this tool's output is held to.
    #[must_use]
    pub fn limit_for(&self, tool_name: &str) -> usize {
        self.explicit_override(tool_name)
            .unwrap_or(self.default_max_chars)
    }
}

impl Default for TruncationConfig {
    fn default() -> Self {
        let mut tool_overrides = HashMap::new();
        tool_overrides.insert("bash".to_string(), 16_384);
        Self {
            default_max_chars: 30_000,
            tool_overrides,
        }
    }
}

/// The result of a truncation pass on a single piece of tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncatedOutput {
    /// The (possibly truncated) content ready to be returned to the model.
    pub content: String,
    /// Whether the output was shortened.
    pub was_truncated: bool,
    /// Unicode character count of the original string before any truncation.
    pub original_len: usize,
}

/// Tools whose result is a JSON envelope that downstream consumers re-parse
/// (the TUI renderer reads `filePath`/`content`/`structuredPatch`, and the wire
/// seam re-parses it to build the outline view). A blind `chars().take()` on
/// such an envelope cuts it mid-JSON and yields invalid JSON, which both breaks
/// the renderer and forces the wire compressor to fail-open (no outline).
///
/// These tools are exempt from RAW-envelope char truncation — but no longer
/// from the cap itself: [`digest_read_file_envelope`] shortens the `content`
/// FIELD and re-serializes, so the envelope stays valid JSON and the ceiling
/// still applies. r27 measured what the exemption cost: 164 of this corpus's
/// `read_file` results carried more than the cap inside a perfectly legal
/// 2,000-line window (`docs/analysis/tool-caps-r27.md` §1), the largest
/// 422,179 chars.
#[must_use]
fn is_json_envelope_tool(tool_name: &str) -> bool {
    matches!(tool_name.to_ascii_lowercase().as_str(), "read_file" | "read")
}

/// Truncate `output` according to the limit for `tool_name` in `config`.
///
/// Truncation is performed on Unicode code-point boundaries so that multibyte
/// characters (Korean, emoji, etc.) are never split.  When the content is
/// shortened a human-readable notice is appended.
///
/// JSON-envelope tools (see `is_json_envelope_tool`) are returned intact even
/// when oversized: cutting their envelope mid-string would store invalid JSON.
/// Their model-facing size is bounded by the structural/outline compression
/// pass, and their full bytes stay recoverable via the artifact store.
#[must_use]
pub fn truncate_tool_output(
    output: &str,
    tool_name: &str,
    config: &TruncationConfig,
) -> TruncatedOutput {
    let original_len = output.chars().count();
    let explicit_override = config.explicit_override(tool_name);
    // JSON-envelope tools are returned intact when oversized: cutting their
    // envelope mid-string would store invalid JSON. An EXPLICIT per-tool override
    // is an opt-in to raw truncation and still wins, so the documented override
    // capability is preserved; absent one, the envelope is left whole and its
    // model-facing size is bounded by the structural/outline compression pass.
    if explicit_override.is_none() && is_json_envelope_tool(tool_name) {
        // Cap the envelope's BODY rather than the envelope: `content` becomes a
        // head+tail digest and the payload gains `contentTruncated` /
        // `originalContentChars`, so the result is still valid JSON, the model
        // can tell it was cut, and the cap comes from the same `TruncationConfig`
        // every other tool reads. `None` means the envelope did not parse or its
        // body already fits — both keep today's pass-through, byte for byte.
        if let Some(content) = digest_read_file_envelope(output, config.default_max_chars) {
            return TruncatedOutput {
                content,
                was_truncated: true,
                original_len,
            };
        }
        return TruncatedOutput {
            content: output.to_string(),
            was_truncated: false,
            original_len,
        };
    }
    let limit = config.limit_for(tool_name);

    if original_len <= limit {
        return TruncatedOutput {
            content: output.to_string(),
            was_truncated: false,
            original_len,
        };
    }

    if tool_name.eq_ignore_ascii_case("bash") {
        if let Some(mut content) = digest_bash_over_cap(output, limit) {
            if content.chars().count() > limit {
                content = content.chars().take(limit).collect();
            }
            return TruncatedOutput {
                content,
                was_truncated: true,
                original_len,
            };
        }
    }

    // WebFetch returns a fetched page's readable body (metadata header + text).
    // Head-only truncation would silently drop the tail — a long article's
    // conclusion, a doc page's reference/API list at the bottom. A head+tail
    // digest keeps both ends and marks the elided middle; the full body stays
    // recoverable via the artifact store (the dispatch seam preserves the whole
    // pre-truncation output and appends the sha-bearing recovery notice). This
    // mirrors the bash digest, but the body is plain text so no envelope parsing
    // is needed. The noun customises the marker ("page body"/"page").
    if is_web_fetch_tool(tool_name) {
        return TruncatedOutput {
            content: digest_text_head_tail(output, limit, "page body", "page"),
            was_truncated: true,
            original_len,
        };
    }

    // Every remaining tool — grep/glob and, dominantly, MCP/plugin tools — used
    // to get a head-only cut (`chars().take`), silently discarding the tail. For
    // most tool output the tail is the highest-signal region (a conclusion, an
    // error summary, a final status line), so head-only loss is exactly wrong.
    // Reuse the same head+tail digest as WebFetch so both ends survive with an
    // elision marker for the middle; the full pre-truncation bytes stay
    // recoverable via the artifact store, and the dispatch seam appends the
    // sha-bearing recovery notice only when the store actually persisted (so the
    // marker never over-promises retrieval beyond today's invariant).
    TruncatedOutput {
        content: digest_text_head_tail(output, limit, "output", "output"),
        was_truncated: true,
        original_len,
    }
}


/// Shorten an oversized `read_file` envelope's `content` field, keeping the
/// envelope valid JSON.
///
/// Returns `None` — leaving the caller's pass-through untouched — when the
/// output is not a `read_file` envelope (fail-open: an unparseable envelope
/// must not be mangled) or when its body already fits. That second case is what
/// keeps every read under the cap byte-for-byte identical: the envelope is not
/// even re-serialized.
///
/// The body gets the same head+tail digest as any other over-cap text, so the
/// tail of the window survives; `notice` carries the recovery instruction in the
/// wording the default-line-cap note already uses (`compress_read_file` prints
/// it as `[note] …`, so the model sees it on the wire and not only in the raw
/// envelope). The full bytes stay recoverable via the artifact store, which the
/// dispatch seam advertises in a `recoveryNotice` field.
#[must_use]
fn digest_read_file_envelope(output: &str, limit: usize) -> Option<String> {
    let mut parsed: ReadFileOutput = serde_json::from_str(output).ok()?;
    let original_chars = parsed.file.content.chars().count();
    if original_chars <= limit {
        return None;
    }
    parsed.file.content = digest_text_head_tail(&parsed.file.content, limit, "file content", "file");
    parsed.file.content_truncated = true;
    parsed.file.original_content_chars = Some(original_chars);
    let note = format!(
        "content shortened to {limit} chars of {original_chars} — re-read a narrower window with offset/limit (0-based lines)"
    );
    parsed.file.notice = Some(match parsed.file.notice.take() {
        Some(existing) => format!("{existing} · {note}"),
        None => note,
    });
    serde_json::to_string_pretty(&parsed).ok()
}

/// Build a bounded plain-text view for an oversized bash JSON envelope.
///
/// This protects downstream consumers from invalid mid-JSON cuts while keeping
/// the command outcome and the high-signal tail of each stream visible. The
/// omitted-middle marker deliberately points only at full artifact retrieval:
/// bash artifacts are structured JSON, so line-window retrieval does not apply
/// to individual stdout/stderr fields.
#[must_use]
fn digest_bash_over_cap(output: &str, limit: usize) -> Option<String> {
    let parsed: BashCommandOutput = serde_json::from_str(output).ok()?;
    if parsed.structured_content.is_some() || parsed.is_image == Some(true) {
        return None;
    }

    let header = bounded_bash_digest_header(&parsed, limit);
    if header.chars().count() >= limit {
        return Some(header);
    }

    let stdout = compact_bash_stream(&parsed.stdout);
    let stderr = compact_bash_stream(&parsed.stderr);
    if stdout.is_empty() && stderr.is_empty() {
        return Some(append_within_limit(header, "(no output)", limit));
    }

    let remaining_without_separator = limit.saturating_sub(header.chars().count());
    if remaining_without_separator < short_elision_marker().chars().count() {
        return Some(append_within_limit(
            header,
            &fit_elision_marker("output", 0, 0, remaining_without_separator),
            limit,
        ));
    }

    let stderr_separator = if stderr.is_empty() { "" } else { "── stderr ──\n" };
    let stream_count = usize::from(!stdout.is_empty()) + usize::from(!stderr.is_empty());
    let structural_cost = header.chars().count()
        + stderr_separator.chars().count()
        + stream_count * short_elision_marker().chars().count();
    if structural_cost > limit {
        let remaining = limit.saturating_sub(header.chars().count());
        let marker = fit_elision_marker("output", 0, 0, remaining);
        return Some(append_within_limit(header, &marker, limit));
    }

    let stream_budget_total = limit
        .saturating_sub(header.chars().count())
        .saturating_sub(stderr_separator.chars().count());
    let (stdout_budget, stderr_budget) = split_stream_budget(stream_budget_total, &stdout, &stderr);

    let mut view = String::with_capacity(limit.min(output.len()));
    view.push_str(&header);
    if !stdout.is_empty() {
        view.push_str(&render_bash_stream_window("stdout", &stdout, stdout_budget));
    }
    if !stderr.is_empty() {
        if !view.ends_with('\n') {
            view.push('\n');
        }
        view.push_str(stderr_separator);
        view.push_str(&render_bash_stream_window("stderr", &stderr, stderr_budget));
    }

    debug_assert!(view.chars().count() <= limit);
    Some(view)
}

/// Case- and separator-insensitive match for the `WebFetch` tool (accepts
/// `WebFetch`, `web_fetch`, `web-fetch`). Runtime cannot depend on `tools`, so
/// this mirrors the dispatch-side canonical name locally.
fn is_web_fetch_tool(tool_name: &str) -> bool {
    tool_name.trim().replace(['-', '_'], "").eq_ignore_ascii_case("webfetch")
}

/// Head+tail digest for an oversized plain-text tool output (a fetched web page,
/// an MCP/plugin result, a search dump). Keeps the first ~2/3 and last ~1/3 of
/// the visible budget with an elision marker in between; the omitted counts in
/// the marker are advisory estimates (a second pass re-fits the marker after the
/// split is known, as in [`render_bash_stream_window`]). `elided_noun`/`full_noun`
/// customise the marker wording (`"page body"`/`"page"` for `WebFetch`, `"output"`
/// for everything else). The result is bounded by `limit` on Unicode char
/// boundaries; the elided middle stays recoverable via the artifact store.
fn digest_text_head_tail(output: &str, limit: usize, elided_noun: &str, full_noun: &str) -> String {
    let total_chars = output.chars().count();
    if total_chars <= limit {
        return output.to_string();
    }

    // Reserve budget for the marker using a worst-case width first, then split
    // the remaining budget head-biased.
    let provisional = text_elision_marker(elided_noun, full_noun, total_chars, total_chars);
    if provisional.chars().count() >= limit {
        return take_chars(&provisional, limit);
    }
    let visible = limit - provisional.chars().count();
    let (head, tail) = head_tail_split(visible);
    let omitted_chars = total_chars.saturating_sub(head + tail);
    let omitted_lines = omitted_line_count(output, head, tail);

    let marker = text_elision_marker(elided_noun, full_noun, omitted_chars, omitted_lines);
    let marker_chars = marker.chars().count();
    if marker_chars >= limit {
        return take_chars(&marker, limit);
    }
    let visible = limit - marker_chars;
    let (head, tail) = head_tail_split(visible);

    let mut out = String::with_capacity(limit.min(output.len()));
    out.push_str(&take_chars(output, head));
    out.push_str(&marker);
    out.push_str(&take_last_chars(output, tail));
    debug_assert!(out.chars().count() <= limit);
    out
}

/// Split a visible-char budget into a head-biased (2:1) head/tail pair. A page's
/// lead usually carries more signal than its footer, but the tail still surfaces
/// the conclusion.
fn head_tail_split(visible: usize) -> (usize, usize) {
    let head = (visible * 2) / 3;
    (head, visible - head)
}

/// Elision marker for the plain-text head+tail digest. `elided_noun` names what
/// was cut (`"page body"`, `"output"`); `full_noun` names the whole thing the
/// retrieval pointer restores (`"page"`, `"output"`). The retrieval phrasing is
/// a sha-less signpost — the actual sha-bearing pointer is appended by the
/// dispatch seam only when the artifact store persisted, so this marker never
/// over-promises retrieval on its own.
fn text_elision_marker(
    elided_noun: &str,
    full_noun: &str,
    omitted_chars: usize,
    omitted_lines: usize,
) -> String {
    format!(
        "\n\n… [{elided_noun} middle elided: {omitted_chars} chars / {omitted_lines} lines — full {full_noun} available via retrieve_tool_output full artifact]\n\n"
    )
}

fn bounded_bash_digest_header(parsed: &BashCommandOutput, limit: usize) -> String {
    let full = bash_digest_header(parsed);
    let recovery_marker = short_elision_marker();
    let full_cost_with_recovery = full.chars().count() + recovery_marker.chars().count();
    if full_cost_with_recovery <= limit {
        return full;
    }

    let minimal = "[bash]\n";
    let minimal_cost_with_recovery = minimal.chars().count() + recovery_marker.chars().count();
    if minimal_cost_with_recovery <= limit {
        return minimal.to_string();
    }

    // Supported useful bash digest caps can carry at least the minimal header
    // plus a retrieval marker. Smaller caps (including 0/1) are still bounded
    // and panic-free by emitting the marker prefix on a UTF-8 char boundary;
    // this avoids exposing a blind partial header or cut JSON/content.
    take_chars(recovery_marker, limit)
}

fn bash_digest_header(parsed: &BashCommandOutput) -> String {
    let mut header_notes = Vec::new();
    if parsed.interrupted {
        header_notes.push("interrupted".to_string());
    }
    if let Some(id) = &parsed.background_task_id {
        header_notes.push(format!("background_task_id={id}"));
    }
    if parsed.backgrounded_by_user == Some(true) {
        header_notes.push("backgrounded_by_user".to_string());
    }
    if parsed.assistant_auto_backgrounded == Some(true) {
        header_notes.push("auto_backgrounded".to_string());
    }
    if parsed.dangerously_disable_sandbox == Some(true) {
        header_notes.push("sandbox_disabled".to_string());
    }
    if let Some(interp) = &parsed.return_code_interpretation {
        header_notes.push(format!("exit: {interp}"));
    }
    if parsed.no_output_expected == Some(true) {
        header_notes.push("no_output_expected".to_string());
    }
    if let Some(path) = &parsed.persisted_output_path {
        let size = parsed
            .persisted_output_size
            .map(|s| format!(" ({s} bytes)"))
            .unwrap_or_default();
        header_notes.push(format!("full output: {path}{size}"));
    }
    if let Some(path) = &parsed.raw_output_path {
        header_notes.push(format!("raw output: {path}"));
    }
    if let Some(warning) = &parsed.safety_warning {
        header_notes.push(format!("safety: {warning}"));
    }
    if let Some(status) = &parsed.sandbox_status {
        if let Ok(rendered) = serde_json::to_string(status) {
            header_notes.push(format!("sandbox: {rendered}"));
        }
    }

    let mut header = String::new();
    if header_notes.is_empty() {
        header.push_str("[bash]");
    } else {
        let _ = write!(header, "[bash] {}", header_notes.join(" · "));
    }
    header.push('\n');
    header
}

fn append_within_limit(mut base: String, suffix: &str, limit: usize) -> String {
    let remaining = limit.saturating_sub(base.chars().count());
    base.push_str(&take_chars(suffix, remaining));
    base
}

fn split_stream_budget(total: usize, stdout: &str, stderr: &str) -> (usize, usize) {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => (0, 0),
        (false, true) => (total, 0),
        (true, false) => (0, total),
        (false, false) => {
            let first = total / 2;
            (first, total - first)
        }
    }
}

fn render_bash_stream_window(stream_name: &str, text: &str, budget: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= budget {
        return text.to_string();
    }

    let omitted_lines_for_marker = text.lines().count();
    let marker = fit_elision_marker(stream_name, total_chars, omitted_lines_for_marker, budget);
    let marker_chars = marker.chars().count();
    if marker_chars >= budget {
        return marker;
    }

    let visible_budget = budget - marker_chars;
    let head_chars = visible_budget / 3;
    let tail_chars = visible_budget - head_chars;
    let omitted_chars = total_chars.saturating_sub(head_chars + tail_chars);
    let omitted_lines = omitted_line_count(text, head_chars, tail_chars);
    let marker = fit_elision_marker(stream_name, omitted_chars, omitted_lines, budget);
    let marker_chars = marker.chars().count();
    let visible_budget = budget.saturating_sub(marker_chars);
    let head_chars = visible_budget / 3;
    let tail_chars = visible_budget - head_chars;

    let mut out = String::with_capacity(budget.min(text.len()));
    out.push_str(&take_chars(text, head_chars));
    out.push_str(&marker);
    out.push_str(&take_last_chars(text, tail_chars));
    debug_assert!(out.chars().count() <= budget);
    out
}

fn fit_elision_marker(stream_name: &str, omitted_chars: usize, omitted_lines: usize, budget: usize) -> String {
    if budget == 0 {
        return String::new();
    }

    let full = format!(
        "… [{stream_name} middle elided: {omitted_chars} chars / {omitted_lines} lines — full stream available via retrieve_tool_output full artifact]\n"
    );
    if full.chars().count() <= budget {
        return full;
    }

    let short = short_elision_marker();
    if short.chars().count() <= budget {
        return short.to_string();
    }

    let shorter = "… retrieve\n";
    if shorter.chars().count() <= budget {
        return shorter.to_string();
    }

    take_chars("…", budget)
}

fn short_elision_marker() -> &'static str {
    "… retrieve_tool_output\n"
}

fn take_chars(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

fn take_last_chars(text: &str, count: usize) -> String {
    let mut chars: Vec<char> = text.chars().rev().take(count).collect();
    chars.reverse();
    chars.into_iter().collect()
}

fn omitted_line_count(text: &str, head_chars: usize, tail_chars: usize) -> usize {
    let total_chars = text.chars().count();
    let start = head_chars.min(total_chars);
    let end = total_chars.saturating_sub(tail_chars);
    if start >= end {
        return 0;
    }
    text.chars()
        .skip(start)
        .take(end - start)
        .collect::<String>()
        .lines()
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> TruncationConfig {
        TruncationConfig::default()
    }

    // basic behaviour

    // --- r27: the read_file envelope's body is capped, the envelope is not ---

    fn read_file_envelope(path: &str, content: &str, notice: Option<&str>) -> String {
        let total = content.lines().count();
        serde_json::to_string_pretty(&ReadFileOutput {
            kind: "text".to_string(),
            file: crate::file_ops::TextFilePayload {
                file_path: path.to_string(),
                content: content.to_string(),
                num_lines: total,
                start_line: 1,
                total_lines: total,
                notice: notice.map(ToString::to_string),
                content_truncated: false,
                original_content_chars: None,
                neighbours: Vec::new(),
            },
        })
        .expect("serialize envelope")
    }

    fn wide_source(lines: usize) -> String {
        let mut out = String::new();
        for n in 0..lines {
            let _ = writeln!(out, "fn item_{n}() {{ let _ = deep_body_line_that_is_fairly_long(); }}");
        }
        out
    }

    /// The r27 pin. An over-cap read (the corpus's dominant shape: an ordinary
    /// 2,000-line window that simply holds more than 30,000 chars) must come
    /// back as VALID JSON with a head+tail body, both ends intact, and a flag
    /// the model can see.
    #[test]
    fn oversized_read_file_body_is_head_tail_and_the_envelope_stays_json() {
        let cfg = default_config();
        let content = wide_source(2000);
        assert!(content.chars().count() > cfg.default_max_chars);
        let envelope = read_file_envelope("/ws/big.rs", &content, None);

        let result = truncate_tool_output(&envelope, "read_file", &cfg);
        assert!(result.was_truncated);
        assert_eq!(result.original_len, envelope.chars().count());

        let parsed: ReadFileOutput =
            serde_json::from_str(&result.content).expect("the envelope stays valid JSON");
        let body = &parsed.file.content;
        assert!(body.chars().count() <= cfg.default_max_chars);
        assert!(parsed.file.content_truncated);
        assert_eq!(
            parsed.file.original_content_chars,
            Some(content.chars().count())
        );
        assert!(body.starts_with("fn item_0()"), "the head survives");
        assert!(body.trim_end().ends_with("fn item_1999() { let _ = deep_body_line_that_is_fairly_long(); }"),
            "the tail survives — head-only truncation is what this replaces");
        assert!(body.contains("file content middle elided"));
        // The window fields still describe the WINDOW that was read, not the
        // digest: a consumer computing "lines X-Y of Z" must not be told the
        // file shrank.
        assert_eq!(parsed.file.num_lines, 2000);
        assert_eq!(parsed.file.total_lines, 2000);
        let notice = parsed.file.notice.expect("a truncated body carries a notice");
        assert!(
            notice.contains("offset/limit"),
            "the model must be told how to get the rest back, got: {notice}"
        );
    }

    /// The other half of the pin: a read under the cap is not merely
    /// "unchanged in meaning" — it is the same bytes. The digest must not
    /// re-serialize an envelope it had no reason to touch.
    #[test]
    fn read_file_within_the_cap_is_byte_identical() {
        let cfg = default_config();
        let envelope = read_file_envelope("/ws/small.rs", &wide_source(50), Some("showing lines 1-50 of 900 — pass offset/limit to read more"));
        assert!(envelope.chars().count() < cfg.default_max_chars);

        let result = truncate_tool_output(&envelope, "read_file", &cfg);
        assert!(!result.was_truncated);
        assert_eq!(result.content, envelope);
    }

    /// An envelope large enough to be over the cap only because of JSON
    /// escaping and indentation (18 of the corpus's 182 over-cap results) has a
    /// body that fits, and a body that fits is not touched — the overhead is not
    /// content and cutting content would not remove it.
    #[test]
    fn read_file_over_cap_only_by_envelope_overhead_is_byte_identical() {
        let cfg = default_config();
        // Every line ends in `\n` and carries quotes: 2 escaped chars each.
        let mut content = String::new();
        while content.chars().count() < cfg.default_max_chars - 2_000 {
            content.push_str("let s = \"a quoted fragment with escapes\";\n");
        }
        let envelope = read_file_envelope("/ws/quotes.rs", &content, None);
        assert!(content.chars().count() < cfg.default_max_chars);
        assert!(envelope.chars().count() > cfg.default_max_chars);

        let result = truncate_tool_output(&envelope, "read_file", &cfg);
        assert!(!result.was_truncated);
        assert_eq!(result.content, envelope);
    }

    /// Fail-open: anything under a `read_file` name that is not a parseable
    /// envelope passes through whole rather than being cut into invalid JSON.
    /// (`read_file` errors and microcompaction placeholders both take this
    /// path.)
    #[test]
    fn unparseable_read_file_output_still_passes_through_whole() {
        let cfg = default_config();
        let raw = "io error: No such file or directory (os error 2)".repeat(2_000);
        let result = truncate_tool_output(&raw, "read_file", &cfg);
        assert!(!result.was_truncated);
        assert_eq!(result.content, raw);
    }

    /// The cap value must come from the config, not from a constant sprinkled
    /// into the envelope path — goal 2.3 of the r27 brief. Two configs, two
    /// body sizes.
    #[test]
    fn read_file_body_cap_is_read_from_the_config() {
        let content = wide_source(2000);
        let envelope = read_file_envelope("/ws/big.rs", &content, None);
        let tight = TruncationConfig {
            default_max_chars: 5_000,
            ..Default::default()
        };

        let wide = truncate_tool_output(&envelope, "read_file", &TruncationConfig::default());
        let narrow = truncate_tool_output(&envelope, "read_file", &tight);
        let body = |raw: &str| {
            serde_json::from_str::<ReadFileOutput>(raw)
                .expect("valid envelope")
                .file
                .content
                .chars()
                .count()
        };
        assert!(body(&wide.content) <= 30_000);
        assert!(body(&narrow.content) <= 5_000);
        assert!(body(&narrow.content) < body(&wide.content));
    }

    /// An explicit per-tool override is still an opt-in to RAW truncation (the
    /// documented escape hatch), so it keeps winning over the envelope digest.
    #[test]
    fn an_explicit_read_file_override_still_takes_the_raw_path() {
        let mut cfg = TruncationConfig::default();
        cfg.tool_overrides.insert("read_file".to_string(), 1_000);
        let envelope = read_file_envelope("/ws/big.rs", &wide_source(2000), None);

        let result = truncate_tool_output(&envelope, "read_file", &cfg);
        assert!(result.was_truncated);
        assert!(result.content.chars().count() <= 1_000);
        assert!(serde_json::from_str::<ReadFileOutput>(&result.content).is_err());
    }

    /// An MCP result reaches the middleware under its qualified name and must
    /// land on the generic head+tail digest — the same shape bash gets, which is
    /// what the r27 decision asked for.
    #[test]
    fn an_mcp_result_gets_the_head_tail_digest() {
        let cfg = default_config();
        let raw = format!("HEAD-ROW{}TAIL-ROW", "payload ".repeat(40_000));
        let result = truncate_tool_output(&raw, "mcp__atlassian__getJiraIssue", &cfg);
        assert!(result.was_truncated);
        assert!(result.content.chars().count() <= cfg.default_max_chars);
        assert!(result.content.starts_with("HEAD-ROW"));
        assert!(result.content.ends_with("TAIL-ROW"));
        assert!(result.content.contains("output middle elided"));
    }

    #[test]
    fn short_output_is_returned_unchanged() {
        let cfg = default_config();
        let result = truncate_tool_output("hello world", "read_file", &cfg);
        assert_eq!(result.content, "hello world");
        assert!(!result.was_truncated);
        assert_eq!(result.original_len, "hello world".chars().count());
    }

    #[test]
    fn empty_string_is_returned_unchanged() {
        let cfg = default_config();
        let result = truncate_tool_output("", "read_file", &cfg);
        assert_eq!(result.content, "");
        assert!(!result.was_truncated);
        assert_eq!(result.original_len, 0);
    }

    #[test]
    fn output_exactly_at_limit_is_unchanged() {
        let cfg = default_config();
        let s = "x".repeat(cfg.default_max_chars);
        let result = truncate_tool_output(&s, "grep", &cfg);
        assert!(!result.was_truncated);
        assert_eq!(result.content, s);
    }

    #[test]
    fn output_one_over_limit_is_truncated() {
        let cfg = default_config();
        let s = "a".repeat(cfg.default_max_chars + 1);
        let result = truncate_tool_output(&s, "grep", &cfg);
        assert!(result.was_truncated);
        assert_eq!(result.original_len, cfg.default_max_chars + 1);
        // Generic over-cap text now gets a head+tail digest, not a head-only cut.
        assert!(result.content.chars().count() <= cfg.default_max_chars);
        assert!(result.content.contains("output middle elided"));
        assert!(result.content.contains("retrieve_tool_output"));
    }

    #[test]
    fn generic_digest_marker_reports_omitted_counts() {
        let cfg = default_config();
        let original_len = cfg.default_max_chars + 500;
        let s = "b".repeat(original_len);
        let result = truncate_tool_output(&s, "grep", &cfg);
        assert!(result.was_truncated);
        // The elision marker carries the omitted char/line counts (the head-only
        // "[output truncated — N chars exceeded limit]" notice is retired).
        assert!(result.content.contains("output middle elided:"));
        assert!(result.content.contains("chars /"));
        assert!(result.content.contains("full output available via retrieve_tool_output"));
    }

    // per-tool overrides

    #[test]
    fn tool_override_is_respected() {
        let mut cfg = default_config();
        cfg.tool_overrides.insert("read_file".to_string(), 100);

        // 150 chars -- under global limit but over the per-tool limit. An
        // explicit per-tool override opts read_file back into truncation; the
        // digest respects the 100-char cap.
        let s = "z".repeat(150);
        let result = truncate_tool_output(&s, "read_file", &cfg);
        assert!(result.was_truncated);
        assert!(result.content.chars().count() <= 100);
    }

    #[test]
    fn tool_override_does_not_affect_other_tools() {
        let mut cfg = default_config();
        cfg.tool_overrides.insert("read_file".to_string(), 100);

        // 150 chars -- under global limit; "grep" has no override.
        let s = "z".repeat(150);
        let result = truncate_tool_output(&s, "grep", &cfg);
        assert!(!result.was_truncated);
    }

    // default bash override (hole-006)

    #[test]
    fn default_config_has_bash_override_at_16384() {
        let cfg = default_config();
        assert_eq!(cfg.tool_overrides.get("bash").copied(), Some(16_384));
    }

    #[test]
    fn override_lookup_is_case_insensitive_for_raw_tool_names() {
        // Defence-in-depth: even if a caller passes the raw PascalCase name
        // ("Bash") instead of the canonical lowercase key, the per-tool cap
        // must still apply rather than silently using the global default.
        let cfg = default_config();
        let s = "a".repeat(16_385); // one over the bash override (16_384)
        let result = truncate_tool_output(&s, "Bash", &cfg);
        // `was_truncated` is the discriminating signal: 16_385 < the 30_000
        // global default, so it truncates only if the 16_384 bash override
        // actually applied (case-insensitively) for the raw "Bash" name.
        assert!(result.was_truncated);
        assert!(result.content.chars().count() <= 16_384);
    }

    #[test]
    fn bash_uses_16384_char_limit_by_default() {
        let cfg = default_config();

        // 16_384 chars -- exactly at the bash limit, should not truncate.
        let s = "a".repeat(16_384);
        let result = truncate_tool_output(&s, "bash", &cfg);
        assert!(!result.was_truncated);

        // 16_385 chars -- one over the bash limit, should truncate. This blob is
        // not a valid bash JSON envelope, so the envelope digest declines and it
        // falls through to the generic head+tail digest, still capped at 16_384.
        let s_over = "a".repeat(16_385);
        let result_over = truncate_tool_output(&s_over, "bash", &cfg);
        assert!(result_over.was_truncated);
        assert!(result_over.content.chars().count() <= 16_384);
    }


    fn unique_lines(prefix: &str, count: usize) -> String {
        let mut out = String::new();
        for n in 0..count {
            let _ = writeln!(out, "{prefix} {n}");
        }
        out
    }

    fn bash_envelope(stdout: &str, stderr: &str, exit: Option<&str>) -> String {
        serde_json::to_string_pretty(&serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "rawOutputPath": null,
            "interrupted": false,
            "isImage": null,
            "backgroundTaskId": null,
            "backgroundedByUser": null,
            "assistantAutoBackgrounded": null,
            "dangerouslyDisableSandbox": null,
            "returnCodeInterpretation": exit,
            "noOutputExpected": null,
            "structuredContent": null,
            "persistedOutputPath": null,
            "persistedOutputSize": null,
            "sandboxStatus": null,
        }))
        .unwrap()
    }

    #[test]
    fn over_cap_bash_envelope_gets_plain_text_head_tail_digest() {
        let cfg = default_config();
        let mut stdout = String::new();
        for n in 0..1200 {
            let _ = writeln!(stdout, "stdout-line-{n:04} detail detail detail");
        }
        let mut stderr = String::new();
        for n in 0..500 {
            let _ = writeln!(stderr, "stderr-line-{n:04} failure detail");
        }
        let envelope = bash_envelope(&stdout, &stderr, Some("exit code 1 (failure)"));
        assert!(envelope.chars().count() > 16_384);

        let result = truncate_tool_output(&envelope, "bash", &cfg);

        assert!(result.was_truncated);
        assert!(result.content.starts_with("[bash] exit: exit code 1 (failure)\n"));
        assert!(result.content.chars().count() <= 16_384);
        assert!(result.content.contains("stdout-line-0000"), "stdout head kept");
        assert!(result.content.contains("stdout-line-1199"), "stdout tail kept");
        assert!(result.content.contains("stderr-line-0000"), "stderr head kept");
        assert!(result.content.contains("stderr-line-0499"), "stderr tail kept");
        assert!(result.content.contains("middle elided:"));
        assert!(result.content.contains("chars /"));
        assert!(result.content.contains("lines — full stream available via retrieve_tool_output full artifact"));
        assert!(serde_json::from_str::<serde_json::Value>(&result.content).is_err());
    }

    #[test]
    fn under_cap_bash_envelope_is_byte_identical() {
        let cfg = default_config();
        let envelope = bash_envelope("short stdout\n", "", Some("exit code 0 (success)"));

        let result = truncate_tool_output(&envelope, "bash", &cfg);

        assert!(!result.was_truncated);
        assert_eq!(result.content, envelope);
    }

    #[test]
    fn over_cap_generic_text_gets_head_and_tail_digest() {
        // The old behavior head-only-cut this and dropped the tail. The digest
        // must keep BOTH ends and mark the elided middle, staying within the cap.
        let cfg = default_config();
        let text = format!("HEAD_MARKER{}TAIL_MARKER", "a".repeat(cfg.default_max_chars + 100));

        let result = truncate_tool_output(&text, "grep", &cfg);

        assert!(result.was_truncated);
        assert!(result.content.chars().count() <= cfg.default_max_chars);
        assert!(result.content.starts_with("HEAD_MARKER"), "head kept");
        assert!(result.content.contains("TAIL_MARKER"), "tail kept");
        assert!(result.content.contains("output middle elided"));
        assert!(result.content.contains("retrieve_tool_output"));
    }

    #[test]
    fn over_cap_mcp_tool_output_gets_head_and_tail_digest() {
        // MCP/plugin tools are the dominant users of the generic branch. Their
        // over-cap output must keep the high-signal tail (conclusion / error
        // summary) via the head+tail digest rather than the old head-only cut.
        let cfg = default_config();
        let mut body = String::from("HEAD_SUMMARY_UNIQUE\n");
        for n in 0..6_000 {
            let _ = writeln!(body, "mcp result row {n} lorem ipsum dolor sit amet consectetur");
        }
        body.push_str("TAIL_CONCLUSION_UNIQUE\n");
        assert!(body.chars().count() > cfg.default_max_chars);

        let result = truncate_tool_output(&body, "mcp__weather__forecast", &cfg);

        assert!(result.was_truncated);
        assert_eq!(result.original_len, body.chars().count());
        assert!(result.content.chars().count() <= cfg.default_max_chars);
        assert!(result.content.contains("HEAD_SUMMARY_UNIQUE"), "head kept");
        assert!(result.content.contains("TAIL_CONCLUSION_UNIQUE"), "tail kept");
        assert!(result.content.contains("output middle elided"));
        assert!(result.content.contains("full output available via retrieve_tool_output"));
        // The retired head-only cut would NOT have kept the tail conclusion.
    }

    #[test]
    fn generic_digest_is_char_bounded_with_multibyte() {
        // 한글/emoji generic output over the cap must never split a codepoint.
        let mut cfg = default_config();
        cfg.default_max_chars = 400;
        let body: String = "가나다라 🚀 ".repeat(400); // well over 400 chars
        assert!(body.chars().count() > 400);

        let result = truncate_tool_output(&body, "mcp__x__y", &cfg);

        assert!(result.was_truncated);
        assert!(result.content.chars().count() <= 400);
        assert!(result.content.is_char_boundary(result.content.len()));
        assert!(!result.content.contains('\u{FFFD}'));
        assert!(result.content.contains("retrieve_tool_output"));
    }

    #[test]
    fn bash_over_cap_digest_is_deterministic() {
        let cfg = default_config();
        let envelope = bash_envelope(&"line\n".repeat(20_000), "", Some("exit code 0 (success)"));

        let first = truncate_tool_output(&envelope, "bash", &cfg);
        let second = truncate_tool_output(&envelope, "bash", &cfg);

        assert_eq!(first.content, second.content);
    }

    #[test]
    fn tiny_bash_caps_keep_recovery_marker_and_bound() {
        for limit in [200usize, 50] {
            let mut cfg = default_config();
            cfg.tool_overrides.insert("bash".to_string(), limit);
            let envelope = bash_envelope(
                &unique_lines("stdout noise line", 500),
                &unique_lines("stderr failure line", 500),
                Some("exit code 1 (failure)"),
            );

            let result = truncate_tool_output(&envelope, "bash", &cfg);

            assert!(result.was_truncated);
            assert!(
                result.content.chars().count() <= limit,
                "digest exceeded limit {limit}: {} chars",
                result.content.chars().count()
            );
            assert!(result.content.contains("retrieve_tool_output"));
        }
    }

    #[test]
    fn extremely_tiny_bash_caps_prefer_recovery_marker_prefix_over_header_cut() {
        let envelope = bash_envelope(
            &unique_lines("stdout noise line", 500),
            &unique_lines("stderr failure line", 500),
            Some("exit code 1 (failure)"),
        );
        let marker = short_elision_marker();
        let marker_len = marker.chars().count();

        for limit in [0usize, 1, 6, 7, 8] {
            let mut cfg = default_config();
            cfg.tool_overrides.insert("bash".to_string(), limit);

            let result = truncate_tool_output(&envelope, "bash", &cfg);

            assert!(result.was_truncated);
            assert!(
                result.content.chars().count() <= limit,
                "digest exceeded limit {limit}: {} chars",
                result.content.chars().count()
            );
            assert_eq!(result.content, take_chars(marker, limit));
            assert!(result.content.is_char_boundary(result.content.len()));
            assert!(
                !result.content.starts_with("[bash"),
                "blind partial bash header leaked at limit {limit}: {:?}",
                result.content
            );
        }

        for limit in [marker_len, marker_len + "[bash]\n".chars().count()] {
            let mut cfg = default_config();
            cfg.tool_overrides.insert("bash".to_string(), limit);

            let result = truncate_tool_output(&envelope, "bash", &cfg);

            assert!(result.was_truncated);
            assert!(result.content.chars().count() <= limit);
            assert!(result.content.contains("retrieve_tool_output"));
            assert!(result.content.is_char_boundary(result.content.len()));
        }
    }

    #[test]
    fn tiny_bash_cap_with_long_header_does_not_blind_cut_marker() {
        let mut cfg = default_config();
        cfg.tool_overrides.insert("bash".to_string(), 50);
        let envelope = bash_envelope(
            &unique_lines("line", 500),
            "",
            Some("exit code 101 (very long failure interpretation with extra context)"),
        );

        let result = truncate_tool_output(&envelope, "bash", &cfg);

        assert!(result.was_truncated);
        assert!(result.content.chars().count() <= 50);
        assert!(result.content.starts_with("[bash]\n"));
        assert!(result.content.contains("retrieve_tool_output"));
    }

    #[test]
    fn bash_digest_handles_multibyte_and_stream_shapes() {
        let mut cfg = default_config();
        cfg.tool_overrides.insert("bash".to_string(), 200);

        let stdout_only = truncate_tool_output(
            &bash_envelope(&unique_lines("가나다 stdout", 120), "", None),
            "bash",
            &cfg,
        );
        assert!(stdout_only.content.chars().count() <= 200);
        assert!(stdout_only.content.contains("retrieve_tool_output"));
        assert!(stdout_only.content.is_char_boundary(stdout_only.content.len()));

        let stderr_only = truncate_tool_output(
            &bash_envelope("", &unique_lines("🚨 stderr", 120), None),
            "bash",
            &cfg,
        );
        assert!(stderr_only.content.chars().count() <= 200);
        assert!(stderr_only.content.contains("── stderr ──") || stderr_only.content.contains("retrieve_tool_output"));
        assert!(stderr_only.content.is_char_boundary(stderr_only.content.len()));

        let empty = truncate_tool_output(&bash_envelope("", "", None), "bash", &cfg);
        assert!(empty.content.chars().count() <= 200);
        assert!(empty.content.contains("(no output)"));
    }

    #[test]
    fn over_cap_cargo_test_digest_is_failure_first_before_windowing() {
        let mut cfg = default_config();
        cfg.tool_overrides.insert("bash".to_string(), 800);
        let mut stdout = String::from("running 901 tests\n");
        for n in 0..900 {
            let _ = writeln!(stdout, "test pass_{n:04} ... ok");
        }
        stdout.push_str("test fail_case ... FAILED\n\n");
        stdout.push_str("failures:\n\n---- fail_case stdout ----\nexpected true\n\n");
        stdout.push_str("test result: FAILED. 900 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n");

        let result = truncate_tool_output(&bash_envelope(&stdout, "", None), "bash", &cfg);

        let summary = result.content.find("test result: FAILED").expect("summary kept");
        let failure = result.content.find("---- fail_case stdout ----").expect("failure kept");
        assert!(summary < failure);
        assert!(summary < result.content.chars().count() / 2, "summary should be in the front half");
        if let Some(pass_noise) = result.content.find("test pass_0000 ... ok") {
            assert!(summary < pass_noise, "summary must precede passing noise");
        }
    }

    // WebFetch head+tail digest

    #[test]
    fn web_fetch_over_cap_gets_head_and_tail_digest() {
        // A long fetched page: the head-only default would drop HEAD_ONLY... no,
        // it would drop the TAIL. The digest must keep BOTH ends and mark the
        // elided middle, staying within the cap on char boundaries.
        let cfg = default_config();
        let mut body = String::from("HEAD_MARKER_UNIQUE\n");
        for n in 0..5_000 {
            let _ = writeln!(body, "middle filler line {n} lorem ipsum dolor sit amet");
        }
        body.push_str("TAIL_MARKER_UNIQUE\n");
        assert!(body.chars().count() > cfg.default_max_chars);

        let result = truncate_tool_output(&body, "WebFetch", &cfg);

        assert!(result.was_truncated);
        assert_eq!(result.original_len, body.chars().count());
        assert!(
            result.content.chars().count() <= cfg.default_max_chars,
            "digest must stay within the cap"
        );
        assert!(result.content.contains("HEAD_MARKER_UNIQUE"), "head kept");
        assert!(result.content.contains("TAIL_MARKER_UNIQUE"), "tail kept");
        assert!(result.content.contains("page body middle elided"));
        assert!(
            result.content.contains("retrieve_tool_output"),
            "the marker points at full-artifact recovery"
        );
        // WebFetch keeps its own marker wording ("page body"); a generic tool
        // now also gets a head+tail digest, but with the generic "output" noun.
        let generic = truncate_tool_output(&body, "grep", &cfg);
        assert!(generic.content.contains("TAIL_MARKER_UNIQUE"), "generic tail kept");
        assert!(generic.content.contains("output middle elided"));
        assert!(!generic.content.contains("page body middle elided"));
    }

    #[test]
    fn web_fetch_under_cap_is_returned_unchanged() {
        let cfg = default_config();
        let body = "Title: Docs\nURL: https://example.com\n\n# Heading\n\nBody text.";
        let result = truncate_tool_output(body, "WebFetch", &cfg);
        assert!(!result.was_truncated);
        assert_eq!(result.content, body);
    }

    #[test]
    fn web_fetch_digest_is_char_bounded_with_multibyte() {
        // 한글/emoji body over the cap must never split a codepoint and must
        // stay within the limit.
        let mut cfg = default_config();
        cfg.default_max_chars = 500;
        let body: String = "가나다라 🚀 ".repeat(400); // well over 500 chars
        assert!(body.chars().count() > 500);

        let result = truncate_tool_output(&body, "web_fetch", &cfg);

        assert!(result.was_truncated);
        assert!(result.content.chars().count() <= 500);
        assert!(result.content.is_char_boundary(result.content.len()));
        assert!(result.content.contains("retrieve_tool_output"));
    }

    // JSON-envelope handling (read_file)

    #[test]
    fn read_file_envelope_is_never_raw_truncated_when_oversized() {
        // A >30k read_file result is a JSON envelope the renderer and the wire
        // seam re-parse. A blind `chars().take()` would cut it mid-string and
        // store invalid JSON. Since r27 the size IS bounded here — the `content`
        // FIELD is shortened and the envelope re-serialized — but the invariant
        // this test was written for is the same one: whatever comes back parses.
        //
        // The fixture is a COMPLETE envelope on purpose. An incomplete one
        // (`{"file":{"filePath":…,"content":…}}` with no `type`/`numLines`) does
        // not deserialize, so it takes the fail-open path and would keep this
        // test green no matter what the cap did — which is how it passed
        // unchanged across the r27 policy flip.
        let cfg = default_config();
        let body = "x".repeat(cfg.default_max_chars + 5_000);
        let envelope = read_file_envelope("/tmp/big.rs", &body, None);
        assert!(envelope.chars().count() > cfg.default_max_chars);

        let result = truncate_tool_output(&envelope, "read_file", &cfg);

        assert!(result.was_truncated, "an over-cap body is shortened");
        assert_eq!(result.original_len, envelope.chars().count());
        let parsed: ReadFileOutput = serde_json::from_str(&result.content)
            .expect("oversized read_file output must remain valid JSON");
        assert!(parsed.file.content.chars().count() <= cfg.default_max_chars);
        assert!(parsed.file.content_truncated);
    }

    // Unicode / multibyte correctness

    #[test]
    fn korean_chars_counted_as_single_chars() {
        // Each Korean char is 3 bytes in UTF-8 but must count as 1 char.
        let s: String = "\u{AC00}".repeat(10); // 10 chars, 30 bytes
        let mut cfg = default_config();
        cfg.default_max_chars = 10;

        // Exactly 10 chars -- should not truncate.
        let result = truncate_tool_output(&s, "read_file", &cfg);
        assert!(!result.was_truncated);
        assert_eq!(result.original_len, 10);
    }

    #[test]
    fn korean_chars_truncated_on_char_boundary() {
        // 400 Korean chars; limit at 300 so the digest keeps a real head+tail of
        // Korean text (a tiny limit would degenerate to a marker prefix).
        let s: String = "\u{AC00}".repeat(400);
        let mut cfg = default_config();
        cfg.default_max_chars = 300;

        let result = truncate_tool_output(&s, "grep", &cfg);
        assert!(result.was_truncated);
        assert_eq!(result.original_len, 400);

        // The digest must stay within the cap and never split a codepoint.
        assert!(result.content.chars().count() <= 300);
        assert!(result.content.is_char_boundary(result.content.len()));
        // Every non-marker char is a whole Korean syllable (no split bytes).
        assert!(result.content.contains('\u{AC00}'));
        assert!(!result.content.contains('\u{FFFD}'));
    }

    #[test]
    fn emoji_chars_counted_as_single_chars() {
        // U+1F600 GRINNING FACE is 4 bytes in UTF-8 but 1 char.
        let s: String = "\u{1F600}".repeat(5); // 5 chars, 20 bytes
        let mut cfg = default_config();
        cfg.default_max_chars = 5;

        let result = truncate_tool_output(&s, "read_file", &cfg);
        assert!(!result.was_truncated);
        assert_eq!(result.original_len, 5);
    }

    #[test]
    fn emoji_truncation_does_not_split_codepoint() {
        // 400 emoji chars; limit at 300 so the digest keeps a real head+tail.
        let s: String = "\u{1F600}".repeat(400);
        let mut cfg = default_config();
        cfg.default_max_chars = 300;

        let result = truncate_tool_output(&s, "grep", &cfg);
        assert!(result.was_truncated);

        // 4-byte codepoints must never be split by the head/tail windows.
        assert!(result.content.chars().count() <= 300);
        assert!(result.content.is_char_boundary(result.content.len()));
        assert!(result.content.contains('\u{1F600}'));
        assert!(!result.content.contains('\u{FFFD}'));
    }

    #[test]
    fn original_len_is_char_count_not_byte_count() {
        // 5 Korean chars = 15 bytes; original_len must report 5 (chars).
        let s: String = "\u{AC00}".repeat(5);
        let mut cfg = default_config();
        cfg.default_max_chars = 100;

        let result = truncate_tool_output(&s, "read_file", &cfg);
        assert_eq!(result.original_len, 5);
        assert_ne!(result.original_len, s.len()); // must NOT be byte count
    }
}
