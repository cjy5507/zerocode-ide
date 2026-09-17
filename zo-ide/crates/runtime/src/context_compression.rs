//! Context compression: structural, model-facing compression of tool outputs.
//!
//! The dispatch pipeline serializes every tool result as *pretty JSON* before
//! it becomes a `tool_result` block — so the model pays for JSON escaping
//! (`\n`, `\"` are two chars each), indentation, and absolute paths repeated
//! on every grep match line. This module rewrites those envelopes into a
//! compact plain-text view *without dropping information*:
//!
//! - `read_file` / `bash`: unwrap the JSON envelope; file/stdout content is
//!   reproduced byte-identically (lossless — only the wrapper changes).
//! - `grep_search` (content mode): group match lines per file so the absolute
//!   path appears once per file instead of once per line.
//! - `grep_search` / `glob_search` file lists: render paths relative to the
//!   workspace root, stated once in the header (reconstructible — lossless).
//! - `bash` stdout/stderr additionally strip ANSI escapes and collapse runs of
//!   identical lines into `… ⟨repeated ×N⟩` markers (information-preserving).
//!
//! Guarantees:
//! - **Fail-open**: anything that does not parse as the expected envelope is
//!   returned untouched.
//! - **Never worse**: if the compressed view is not actually smaller, the
//!   original is returned untouched.
//! - **No model calls, no I/O**: pure string transformation, hot-path safe.
//!
//! This layer runs *before* `truncate_tool_output`, so when a large output is
//! compressed under the truncation limit, content that would have been cut
//! off survives instead — compression both saves tokens on small outputs and
//! recovers information on large ones. The session still keeps the original
//! output; only the provider-facing view is compressed.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use crate::bash::BashCommandOutput;
use crate::file_ops::{GlobSearchOutput, GrepSearchOutput, ReadFileOutput};

/// Minimum chars a rewrite must save before we prefer it over the original.
/// Below this the churn (different shape for the model, artifact bookkeeping)
/// is not worth single-digit savings.
const MIN_SAVINGS_CHARS: usize = 24;

/// When the lossless unwrap of a `read_file` is still longer than this, the
/// head-only truncation cut in `truncate_tool_output` would discard the whole
/// tail of the file. At that point an *outline* view (structure kept, deep
/// bodies elided with explicit re-read instructions) shows the model the
/// entire file instead of its first ~27%. Mirrors
/// `TruncationConfig::default().default_max_chars`.
const OUTLINE_THRESHOLD_CHARS: usize = 30_000;

/// Indentation (spaces) at and below which a line counts as *structure* for
/// the outline view. rustfmt puts top-level items at 0 and impl/trait members
/// at 4; deeper lines are bodies.
const OUTLINE_KEEP_INDENT: usize = 4;

/// Elided runs shorter than this are kept verbatim — a 3-line body is cheaper
/// than its elision marker.
const OUTLINE_MIN_ELIDE_RUN: usize = 4;

/// Runs of identical lines longer than this are collapsed (bash output only).
const REPEAT_COLLAPSE_THRESHOLD: usize = 3;

/// The outcome of a compression pass over a single tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressionOutcome {
    /// Model-facing content: compressed view, or the original when no
    /// rewrite applied (`was_compressed == false`).
    pub content: String,
    /// Whether `content` differs from the input.
    pub was_compressed: bool,
    /// Unicode char count of the input.
    pub original_chars: usize,
    /// Unicode char count of `content`.
    pub compressed_chars: usize,
}

impl CompressionOutcome {
    fn unchanged(raw: &str) -> Self {
        let chars = raw.chars().count();
        Self {
            content: raw.to_string(),
            was_compressed: false,
            original_chars: chars,
            compressed_chars: chars,
        }
    }

    fn pick_smaller(raw: &str, candidate: String) -> Self {
        let original_chars = raw.chars().count();
        let compressed_chars = candidate.chars().count();
        if compressed_chars + MIN_SAVINGS_CHARS <= original_chars {
            Self {
                content: candidate,
                was_compressed: true,
                original_chars,
                compressed_chars,
            }
        } else {
            Self::unchanged(raw)
        }
    }
}

/// Kill switch for the wire-facing compression pass:
/// `ZO_DISABLE_CONTEXT_COMPRESSION=1` (or `true`) sends every tool result
/// to the model verbatim, exactly as before. Read once per process.
#[must_use]
pub fn compression_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("ZO_DISABLE_CONTEXT_COMPRESSION").as_deref(),
            Ok("1" | "true")
        )
    })
}

/// How much of a tool result may be rewritten on the way to the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireRewrite {
    /// Structure-preserving rewrites only: envelope removal, grouping,
    /// deduplication. Nothing the caller asked for is elided.
    ///
    /// This is what the newest tool result in a request gets. The model called
    /// that tool *this turn*; handing back an outline of the file, or its first
    /// 120 lines and last 40, answers a question it did not ask — and it cannot
    /// tell that the rest existed without reading again.
    Lossless,
    /// Also permit the views that elide content for oversized results. Correct
    /// for history, where the full text has already served its purpose and the
    /// context budget matters more than re-reading it verbatim.
    Full,
}

/// Model-facing view of one tool result, for the provider wire conversion
/// (`convert_messages`). The session keeps the original output (so history,
/// TUI rendering, and reversibility are untouched); only what the model sees
/// is compressed. Error outputs and disabled compression pass through
/// verbatim. Deterministic, so the prompt-cache prefix stays stable across
/// turns.
#[must_use]
pub fn wire_tool_output(
    output: &str,
    tool_name: &str,
    is_error: bool,
    rewrite: WireRewrite,
) -> String {
    if is_error || !compression_enabled() {
        return output.to_string();
    }
    // `convert_messages` re-derives the wire view of EVERY historical tool result
    // on every turn — and again for each fan-out sub-agent's request — so without
    // a memo this is O(history) pure CPU on the request-build seam, worst right
    // after a fan-out injects several large agent outputs. The transform is
    // deterministic in `(output, tool_name)` whenever compression is enabled, so
    // a content-keyed memo returns byte-identical results: prompt-cache-safe and
    // token-neutral, only the wall-clock build cost drops to O(new-results).
    // `rewrite` is part of the key: the same result yields a different view as
    // the newest entry than it does once history has moved past it.
    let key = wire_memo_key(tool_name, output, rewrite);
    if let Some(hit) = wire_memo_get(key) {
        return hit;
    }
    let computed = compress_wire_view(output, tool_name, rewrite);
    wire_memo_put(key, &computed);
    computed
}

/// The uncached wire transform: strip any recovery notice, compress the body,
/// then re-append the notice. Deterministic in `(output, tool_name)`.
fn compress_wire_view(output: &str, tool_name: &str, rewrite: WireRewrite) -> String {
    let (body, recovery_notice) = split_recovery_notice(output);
    let mut content =
        compress_tool_output_with(body.as_ref(), tool_name, None, rewrite).content;
    if let Some(notice) = recovery_notice {
        content.push('\n');
        content.push_str(&notice);
    }
    content
}

/// Max distinct tool-result views the wire-compression memo retains (bounded
/// FIFO). Each value is a compressed view (≤ tens of KB), so this bounds memory.
const WIRE_MEMO_CAPACITY: usize = 256;

struct WireMemo {
    map: HashMap<(u64, u64), String>,
    order: VecDeque<(u64, u64)>,
}

fn wire_memo() -> &'static Mutex<WireMemo> {
    static MEMO: OnceLock<Mutex<WireMemo>> = OnceLock::new();
    MEMO.get_or_init(|| {
        Mutex::new(WireMemo {
            map: HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

/// 128-bit content key: two FNV-1a passes over `tool_name` + NUL + `output` with
/// distinct offset bases, so a hash collision returning a *wrong* cached view is
/// statistically impossible (the failure mode that would matter for correctness).
fn wire_memo_key(tool_name: &str, output: &str, rewrite: WireRewrite) -> (u64, u64) {
    // The permitted rewrite depth changes the view, so it seeds the hash rather
    // than being ignored — otherwise the lossless view of the newest result and
    // the elided view of the same bytes in history would share one cache slot.
    let (a, b) = match rewrite {
        WireRewrite::Lossless => (0xcbf2_9ce4_8422_2325, 0x0102_0304_0506_0708),
        WireRewrite::Full => (0x9e37_79b9_7f4a_7c15, 0x8ebc_6af0_9c88_c6e3),
    };
    (fnv1a_wire(a, tool_name, output), fnv1a_wire(b, tool_name, output))
}

fn fnv1a_wire(seed: u64, tool_name: &str, output: &str) -> u64 {
    let mut hash = seed;
    for byte in tool_name
        .as_bytes()
        .iter()
        .chain(b"\0".iter())
        .chain(output.as_bytes().iter())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn wire_memo_get(key: (u64, u64)) -> Option<String> {
    wire_memo().lock().ok()?.map.get(&key).cloned()
}

fn wire_memo_put(key: (u64, u64), value: &str) {
    let Ok(mut memo) = wire_memo().lock() else {
        return;
    };
    if memo.map.contains_key(&key) {
        return;
    }
    if memo.order.len() >= WIRE_MEMO_CAPACITY {
        if let Some(evicted) = memo.order.pop_front() {
            memo.map.remove(&evicted);
        }
    }
    memo.map.insert(key, value.to_string());
    memo.order.push_back(key);
}

fn split_recovery_notice(output: &str) -> (Cow<'_, str>, Option<String>) {
    const MARKER: &str = "\n[full output preserved — call retrieve_tool_output";
    if let Some(index) = output.rfind(MARKER) {
        let (body, notice) = output.split_at(index);
        return (Cow::Borrowed(body), Some(notice.trim_start().to_string()));
    }

    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(output) else {
        return (Cow::Borrowed(output), None);
    };
    let Some(object) = value.as_object_mut() else {
        return (Cow::Borrowed(output), None);
    };
    let Some(notice) = object
        .remove("recoveryNotice")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
    else {
        return (Cow::Borrowed(output), None);
    };
    let body = serde_json::to_string_pretty(&value).unwrap_or_else(|_| output.to_string());
    (Cow::Owned(body), Some(notice))
}

/// Compress one tool output for the model. `workspace_root` (when known)
/// shortens absolute paths to workspace-relative ones; the root is stated in
/// the header so nothing is lost.
#[must_use]
pub fn compress_tool_output(
    raw: &str,
    tool_name: &str,
    workspace_root: Option<&Path>,
) -> CompressionOutcome {
    compress_tool_output_with(raw, tool_name, workspace_root, WireRewrite::Full)
}

/// [`compress_tool_output`], bounded by how much rewriting is permitted.
#[must_use]
pub fn compress_tool_output_with(
    raw: &str,
    tool_name: &str,
    workspace_root: Option<&Path>,
    rewrite: WireRewrite,
) -> CompressionOutcome {
    // Normalize the wire name first. Claude presents Claude-Code PascalCase tool
    // names (`Read`, `Edit`, `Bash`, `Grep`, `Glob`, `Write`); without this every
    // Claude tool result fell through to `unchanged`, forfeiting wire compression
    // in the dominant case. Mirrors the dispatch-side SSOT
    // `tools::aliases::canonical_tool_name` for the six compressible tools
    // (runtime cannot depend on `tools`). Case- and `-`/`_`-insensitive.
    let canonical = tool_name.trim().replace('-', "_").to_ascii_lowercase();
    match canonical.as_str() {
        "read" | "read_file" => compress_read_file(raw, rewrite, workspace_root),
        "bash" => compress_bash(raw),
        "grep" | "grep_search" => compress_grep(raw, workspace_root),
        "glob" | "glob_search" => compress_glob(raw, workspace_root),
        // `MultiEdit` returns the same envelope as `edit_file` (`filePath` +
        // `structuredPatch`, plus `editsApplied`), but its canonical name is
        // `multiedit`, which fell through to `unchanged` — the one file-editing
        // result that still shipped its argument echo (r31 §4 proposal 1).
        "edit" | "edit_file" | "multiedit" | "multi_edit" => compress_edit_file(raw),
        "write" | "write_file" => compress_write_file(raw),
        _ => CompressionOutcome::unchanged(raw),
    }
}

// ---------------------------------------------------------------------------
// edit_file / write_file
// ---------------------------------------------------------------------------

/// `edit_file` returns the model's own arguments back (`oldString`,
/// `newString`) plus the applied diff (`structuredPatch`). The echo is pure
/// duplication — the model already holds its `tool_use` input in context —
/// and the patch carries the before/after lines anyway. The compact view is
/// the path header plus the patch as unified-diff text.
fn compress_edit_file(raw: &str) -> CompressionOutcome {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return CompressionOutcome::unchanged(raw);
    };
    let Some(obj) = value.as_object() else {
        return CompressionOutcome::unchanged(raw);
    };
    let Some(path) = obj.get("filePath").and_then(|v| v.as_str()) else {
        return CompressionOutcome::unchanged(raw);
    };
    let Some(hunks) = obj.get("structuredPatch").and_then(|v| v.as_array()) else {
        return CompressionOutcome::unchanged(raw);
    };

    let mut view = String::with_capacity(raw.len() / 3);
    let _ = write!(view, "[edit] {path} · applied");
    // `MultiEdit` reports how many edits landed in the one atomic write; the
    // count is the only fact its envelope adds over `edit_file`'s.
    if let Some(edits) = obj.get("editsApplied").and_then(serde_json::Value::as_u64) {
        let _ = write!(view, " · {edits} edits");
    }
    if obj.get("replaceAll").and_then(serde_json::Value::as_bool) == Some(true) {
        view.push_str(" · replace_all");
    }
    if obj.get("userModified").and_then(serde_json::Value::as_bool) == Some(true) {
        view.push_str(" · user_modified");
    }
    view.push('\n');
    render_patch_hunks(&mut view, hunks);
    append_tool_feedback(&mut view, obj);
    CompressionOutcome::pick_smaller(raw, view)
}

/// `write_file` echoes the full written `content` back at the model — the
/// exact bytes it just sent as the tool input. Keep the header and the diff,
/// drop the echo.
fn compress_write_file(raw: &str) -> CompressionOutcome {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return CompressionOutcome::unchanged(raw);
    };
    let Some(obj) = value.as_object() else {
        return CompressionOutcome::unchanged(raw);
    };
    let Some(path) = obj.get("filePath").and_then(|v| v.as_str()) else {
        return CompressionOutcome::unchanged(raw);
    };
    let lines_written = obj
        .get("content")
        .and_then(|v| v.as_str())
        .map(|content| content.lines().count());

    let mut view = String::with_capacity(512);
    let _ = write!(view, "[write] {path} · written");
    if let Some(lines) = lines_written {
        let _ = write!(view, " · {lines} lines");
    }
    view.push('\n');
    if let Some(patch) = obj.get("structuredPatch").and_then(|v| v.as_array()) {
        render_patch_hunks(&mut view, patch);
    }
    append_tool_feedback(&mut view, obj);
    CompressionOutcome::pick_smaller(raw, view)
}

/// Render `structuredPatch` hunks (whose `lines` already carry the
/// ` `/`-`/`+` prefixes) as unified-diff text.
fn render_patch_hunks(out: &mut String, hunks: &[serde_json::Value]) {
    for hunk in hunks {
        let Some(obj) = hunk.as_object() else {
            continue;
        };
        let old_start = obj
            .get("oldStart")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let old_lines = obj
            .get("oldLines")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let new_start = obj
            .get("newStart")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let new_lines = obj
            .get("newLines")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let _ = writeln!(
            out,
            "@@ -{old_start},{old_lines} +{new_start},{new_lines} @@"
        );
        if let Some(lines) = obj.get("lines").and_then(|v| v.as_array()) {
            for line in lines {
                if let Some(text) = line.as_str() {
                    out.push_str(text);
                    out.push('\n');
                }
            }
        }
    }
}

/// Preserve the enrichment feedback (`toolFeedback`: auto-format + LSP
/// diagnostics) that `fold_enrichment_into_output` folds into the envelope.
fn append_tool_feedback(out: &mut String, obj: &serde_json::Map<String, serde_json::Value>) {
    if let Some(feedback) = obj.get("toolFeedback").and_then(|v| v.as_str()) {
        if !feedback.trim().is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(feedback.trim_end());
            out.push('\n');
        }
    }
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

fn compress_read_file(
    raw: &str,
    rewrite: WireRewrite,
    workspace_root: Option<&Path>,
) -> CompressionOutcome {
    let Ok(parsed) = serde_json::from_str::<ReadFileOutput>(raw) else {
        return CompressionOutcome::unchanged(raw);
    };
    let file = &parsed.file;
    let end_line = file
        .start_line
        .saturating_add(file.num_lines.saturating_sub(1));
    let mut view = String::with_capacity(file.content.len() + 128);
    let _ = writeln!(
        view,
        "[file] {} · lines {}-{} of {}",
        file.file_path, file.start_line, end_line, file.total_lines
    );
    // Surface the default-cap note (set only when an unbounded read hit the line
    // cap) so the model sees the actionable "read more" hint on the wire, not
    // just in the raw envelope the wire pass discards.
    if let Some(notice) = &file.notice {
        let _ = writeln!(view, "[note] {notice}");
    }
    // What to read next, on the result that raises the question (r42): the
    // files this one imports, its module, its tests — so the batch that
    // follows can be wide instead of one file at a time.
    if !file.neighbours.is_empty() {
        let root = root_for_paths(workspace_root);
        let _ = writeln!(
            view,
            "[neighbours] {}",
            render_neighbours(&file.neighbours, root.as_deref())
        );
    }
    // The file content itself is reproduced byte-identically: this rewrite is
    // lossless, it only removes the JSON envelope (escaping + indentation).
    view.push_str(&file.content);

    // When even the lossless view exceeds the truncation ceiling, head-only
    // truncation would silently discard the file's tail. For code files an
    // outline (all structure, bodies elided with exact re-read instructions)
    // covers 100% of the file instead of its head — reversible by
    // construction, since every elision marker names the offset/limit that
    // restores the original lines.
    //
    // Both elided views still run when the truncation middleware has already
    // capped `content` — and they have to. The outline is a much tighter bound
    // than the cap (median 15,456 chars against 30,000 on this corpus), and the
    // history view is re-derived into the prefix of every later request, so
    // dropping it to "the body is already ≤ 30,000" would cost ~49,500 chars per
    // rolling request against the ~127 the cap saves at append time
    // (`docs/analysis/tool-caps-r27.md` §3). What changes on a capped body is
    // only what the markers may CLAIM: see [`elision_pointer`].
    if rewrite == WireRewrite::Full && view.chars().count() > OUTLINE_THRESHOLD_CHARS {
        if is_code_path(&file.file_path) {
            if let Some(outline) = outline_view(file) {
                return CompressionOutcome::pick_smaller(raw, outline);
            }
        } else if let Some(bounded) = bounded_text_view(file) {
            // Non-code files have no reliable outline structure, but the lossless
            // unwrap is unbounded — a large .log/.json would flood the wire. Keep
            // a bounded head+tail and name the exact re-read window so nothing is
            // lost (full bytes also stay recoverable via the artifact store).
            return CompressionOutcome::pick_smaller(raw, bounded);
        }
    }
    CompressionOutcome::pick_smaller(raw, view)
}

/// Bounded head+tail view for an oversized NON-code file. The outline view
/// relies on indentation structure that only holds for code; for prose/data we
/// keep the first [`BOUNDED_HEAD_LINES`] and last [`BOUNDED_TAIL_LINES`] lines
/// and elide the middle with a marker carrying the exact `read_file` window that
/// restores it. Returns `None` when the file is too short to bound usefully (it
/// then falls through to the lossless unwrap).
fn bounded_text_view(file: &crate::file_ops::TextFilePayload) -> Option<String> {
    const BOUNDED_HEAD_LINES: usize = 120;
    const BOUNDED_TAIL_LINES: usize = 40;
    let lines: Vec<&str> = file.content.lines().collect();
    if lines.len() <= BOUNDED_HEAD_LINES + BOUNDED_TAIL_LINES + 1 {
        return None;
    }
    let elided = lines.len() - BOUNDED_HEAD_LINES - BOUNDED_TAIL_LINES;
    let first_elided = file.start_line.saturating_add(BOUNDED_HEAD_LINES);
    let end_line = file
        .start_line
        .saturating_add(file.num_lines.saturating_sub(1));
    let mut view = String::with_capacity(OUTLINE_THRESHOLD_CHARS);
    let _ = writeln!(
        view,
        "[file] {} · lines {}-{} of {} · middle elided",
        file.file_path, file.start_line, end_line, file.total_lines
    );
    for line in &lines[..BOUNDED_HEAD_LINES] {
        view.push_str(line);
        view.push('\n');
    }
    let _ = writeln!(
        view,
        "… [{elided} lines elided — {}]",
        elision_pointer(file, first_elided, elided)
    );
    for line in &lines[lines.len() - BOUNDED_TAIL_LINES..] {
        view.push_str(line);
        view.push('\n');
    }
    Some(view)
}

/// What an elision marker may point the model at.
///
/// Both elided views name the exact `read_file offset=…/limit=…` that restores
/// what they cut, deriving the line numbers as `start_line + index-within-
/// content`. That mapping holds only while `content` is the verbatim window.
/// Once the truncation middleware has replaced the body's middle with a digest
/// marker (r27), every line after it sits at a different offset — and a marker
/// naming the WRONG window is worse than one naming none, because the model
/// re-reads and silently gets other lines. So a capped body gets the pointer
/// that is still true: the artifact holds the whole thing.
fn elision_pointer(
    file: &crate::file_ops::TextFilePayload,
    first_elided: usize,
    elided: usize,
) -> String {
    if file.content_truncated {
        return "body already shortened; call retrieve_tool_output for the full file".to_string();
    }
    format!(
        "re-read: read_file {} offset={first_elided} limit={elided}, or call retrieve_tool_output",
        file.file_path
    )
}

/// File types where indentation reliably maps to structure (formatter-enforced
/// or syntactic), making the outline view meaningful.
use crate::file_neighbours::is_code_path;

/// Width of the leading whitespace in display columns (tab = 4).
fn indent_width(line: &str) -> usize {
    let mut width = 0;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += 4,
            _ => break,
        }
    }
    width
}

/// Build the outline view: keep lines indented ≤ [`OUTLINE_KEEP_INDENT`]
/// (top-level items + impl/trait member signatures), elide deeper runs with a
/// marker carrying the exact `read_file` window that restores them. Returns
/// `None` when nothing would be elided.
fn outline_view(file: &crate::file_ops::TextFilePayload) -> Option<String> {
    let lines: Vec<&str> = file.content.lines().collect();
    if lines.is_empty() {
        return None;
    }

    // Classify: structure lines are kept; blank lines are neutral and join
    // whichever side absorbs them (a blank inside a body stays elided, a blank
    // between two kept items stays visible via the short-run promotion below).
    let mut keep: Vec<bool> = lines
        .iter()
        .map(|line| !line.trim().is_empty() && indent_width(line) <= OUTLINE_KEEP_INDENT)
        .collect();

    // Promote short elide runs: a marker line costs more than it saves.
    let mut index = 0;
    while index < lines.len() {
        if keep[index] {
            index += 1;
            continue;
        }
        let run_start = index;
        while index < lines.len() && !keep[index] {
            index += 1;
        }
        if index - run_start < OUTLINE_MIN_ELIDE_RUN {
            keep[run_start..index].fill(true);
        }
    }
    if keep.iter().all(|kept| *kept) {
        return None;
    }

    // `start_line` is 1-based; `read_file`'s `offset` is a 0-based line index.
    let first_line = file.start_line;
    let mut out = String::with_capacity(file.content.len() / 3);
    if file.content_truncated {
        let _ = writeln!(
            out,
            "[file:outline] {} · {} lines · body already shortened, then deep bodies elided — \
             call retrieve_tool_output for the full file, or re-read a narrower window with \
             read_file offset/limit",
            file.file_path, file.total_lines
        );
    } else {
        let _ = writeln!(
            out,
            "[file:outline] {} · {} lines · deep bodies elided — each ⟨…⟩ marker names the \
             read_file offset/limit that returns the elided lines verbatim",
            file.file_path, file.total_lines
        );
    }
    let mut idx = 0;
    while idx < lines.len() {
        if keep[idx] {
            out.push_str(lines[idx]);
            out.push('\n');
            idx += 1;
            continue;
        }
        let run_start = idx;
        while idx < lines.len() && !keep[idx] {
            idx += 1;
        }
        let from = first_line + run_start; // 1-based first elided line
        let to = first_line + idx - 1; // 1-based last elided line
        let count = idx - run_start;
        if file.content_truncated {
            // Line arithmetic is off once the body itself was cut, so the
            // marker states the size and points at the artifact instead of
            // naming a window that does not exist (see `elision_pointer`).
            let _ = writeln!(out, "    ⟨{count} lines elided · retrieve_tool_output⟩");
        } else {
            let _ = writeln!(
                out,
                "    ⟨lines {from}-{to} elided · read_file offset={} limit={count}⟩",
                from - 1
            );
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// bash
// ---------------------------------------------------------------------------

fn compress_bash(raw: &str) -> CompressionOutcome {
    let Ok(parsed) = serde_json::from_str::<BashCommandOutput>(raw) else {
        return CompressionOutcome::unchanged(raw);
    };
    // Structured/multimodal payloads carry data the plain-text view cannot
    // represent — leave those envelopes intact.
    if parsed.structured_content.is_some() || parsed.is_image == Some(true) {
        return CompressionOutcome::unchanged(raw);
    }

    let mut header_notes: Vec<String> = Vec::new();
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
        // An inactive, unrequested, undegraded status is the session-invariant
        // common case (macOS: sandbox unsupported) — ~430 chars of identical
        // JSON on every bash result. Render the note only when it carries
        // signal: sandboxing is on, was asked for, or degraded with a reason.
        let carries_signal = status.enabled
            || status.active
            || status.requested.enabled
            || status.fallback_reason.is_some();
        if carries_signal {
            if let Ok(rendered) = serde_json::to_string(status) {
                header_notes.push(format!("sandbox: {rendered}"));
            }
        }
    }

    let stdout = compact_log_text(&parsed.stdout);
    let stderr = compact_log_text(&parsed.stderr);

    let mut view = String::with_capacity(stdout.len() + stderr.len() + 96);
    if header_notes.is_empty() {
        view.push_str("[bash]");
    } else {
        let _ = write!(view, "[bash] {}", header_notes.join(" · "));
    }
    view.push('\n');
    if !stdout.is_empty() {
        view.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !view.ends_with('\n') {
            view.push('\n');
        }
        view.push_str("── stderr ──\n");
        view.push_str(&stderr);
    }
    if stdout.is_empty() && stderr.is_empty() {
        view.push_str("(no output)");
    }
    CompressionOutcome::pick_smaller(raw, view)
}

pub(crate) fn compact_bash_stream(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let stripped = strip_ansi(text);
    compact_cargo_test_log(&stripped).unwrap_or_else(|| collapse_repeats(&stripped))
}

/// Strip ANSI escapes and collapse long runs of identical lines. The collapse
/// leaves an explicit `⟨repeated ×N⟩` marker, so the information (how many
/// times the line occurred) is preserved.
fn compact_log_text(text: &str) -> String {
    compact_bash_stream(text)
}

fn compact_cargo_test_log(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() || !looks_like_cargo_test_log(&lines) {
        return None;
    }

    let mut summaries = Vec::new();
    let mut failed_status_lines = Vec::new();
    let mut passed = 0usize;
    for line in &lines {
        if is_cargo_test_summary(line) {
            summaries.push(line.trim().to_string());
        }
        if let Some(status) = cargo_test_status(line) {
            match status {
                "ok" => passed += 1,
                "FAILED" => failed_status_lines.push(line.trim().to_string()),
                _ => {}
            }
        }
    }

    let mut view = String::with_capacity(text.len() / 3);
    view.push_str("[cargo test]\n");
    for summary in &summaries {
        view.push_str(summary);
        view.push('\n');
    }

    for failed in &failed_status_lines {
        view.push_str(failed);
        view.push('\n');
    }

    if let Some(failures) = cargo_failures_section(&lines) {
        if !view.ends_with('\n') {
            view.push('\n');
        }
        view.push_str(&failures);
        if !view.ends_with('\n') {
            view.push('\n');
        }
    }

    if passed > 0 {
        let _ = writeln!(view, "{passed} passed tests collapsed");
    }

    if view.trim() == "[cargo test]" {
        return None;
    }
    Some(view)
}

fn looks_like_cargo_test_log(lines: &[&str]) -> bool {
    let has_summary = lines.iter().any(|line| is_cargo_test_summary(line));
    let has_running = lines.iter().any(|line| is_cargo_running_line(line));
    let has_status = lines.iter().any(|line| cargo_test_status(line).is_some());
    has_summary && (has_running || has_status)
}

fn is_cargo_running_line(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix("running ") else {
        return false;
    };
    let Some((digits, suffix)) = rest.split_once(' ') else {
        return false;
    };
    !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && matches!(suffix, "test" | "tests")
}

fn is_cargo_test_summary(line: &str) -> bool {
    line.trim_start().starts_with("test result:")
}

fn cargo_test_status(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("test ") {
        return None;
    }
    let (_, status) = trimmed.rsplit_once(" ... ")?;
    match status.trim() {
        "ok" => Some("ok"),
        "FAILED" => Some("FAILED"),
        "ignored" => Some("ignored"),
        _ => None,
    }
}

fn cargo_failures_section(lines: &[&str]) -> Option<String> {
    let start = lines.iter().position(|line| line.trim() == "failures:")?;
    let mut out = String::new();
    for line in &lines[start..] {
        if is_cargo_test_summary(line) {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    (!out.trim().is_empty()).then_some(out)
}

/// Remove ANSI/VT escape sequences (CSI, OSC, and two-byte ESC sequences).
/// These are styling noise by the time output reaches the model.
fn strip_ansi(text: &str) -> String {
    if !text.contains('\u{1b}') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC '[' …params… final byte in @-~
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ']' … terminated by BEL or ESC '\'
            Some(']') => {
                chars.next();
                let mut prev_esc = false;
                for c in chars.by_ref() {
                    if c == '\u{7}' || (prev_esc && c == '\\') {
                        break;
                    }
                    prev_esc = c == '\u{1b}';
                }
            }
            // Charset designation: ESC '(' / ')' plus one designator char.
            Some('(' | ')') => {
                chars.next();
                chars.next();
            }
            // Two-byte sequences (ESC + single char), e.g. ESC= ESC>
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Collapse runs of > [`REPEAT_COLLAPSE_THRESHOLD`] identical lines into the
/// line itself plus an explicit repetition marker.
fn collapse_repeats(text: &str) -> String {
    let ends_with_newline = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let mut run = 1;
        while index + run < lines.len() && lines[index + run] == line {
            run += 1;
        }
        if run > REPEAT_COLLAPSE_THRESHOLD && !line.trim().is_empty() {
            out.push_str(line);
            out.push('\n');
            let _ = writeln!(out, "⟨repeated ×{run}⟩");
        } else {
            for _ in 0..run {
                out.push_str(line);
                out.push('\n');
            }
        }
        index += run;
    }
    if !ends_with_newline && out.ends_with('\n') {
        out.pop();
    }
    out
}

// ---------------------------------------------------------------------------
// grep_search / glob_search
// ---------------------------------------------------------------------------

fn compress_grep(raw: &str, workspace_root: Option<&Path>) -> CompressionOutcome {
    let Ok(parsed) = serde_json::from_str::<GrepSearchOutput>(raw) else {
        return CompressionOutcome::unchanged(raw);
    };
    let root = root_for_paths(workspace_root);
    let mode = parsed.mode.as_deref().unwrap_or("files_with_matches");

    let mut view = String::with_capacity(raw.len() / 2);
    let mut header = format!("[grep:{mode}] {} files", parsed.num_files);
    if let Some(matches) = parsed.num_matches {
        let _ = write!(header, " · {matches} matches");
    }
    if let Some(lines) = parsed.num_lines {
        let _ = write!(header, " · {lines} lines");
    }
    if let Some(limit) = parsed.applied_limit {
        let _ = write!(header, " · limit {limit}");
    }
    if let Some(offset) = parsed.applied_offset {
        let _ = write!(header, " · offset {offset}");
    }
    if let Some(root) = &root {
        let _ = write!(header, " · paths relative to {}", root.display());
    }
    view.push_str(&header);
    view.push('\n');

    if let ("content", Some(content)) = (mode, &parsed.content) {
        // Group `path:line:text` rows under one path header per file. The
        // filenames list gives us the exact path prefixes, so splitting is
        // exact (no colon heuristics), and every row is preserved.
        let grouped = group_grep_content(content, &parsed.filenames, root.as_deref());
        view.push_str(&grouped);
    } else {
        for path in &parsed.filenames {
            view.push_str(&relativize(path, root.as_deref()));
            view.push('\n');
        }
        if let Some(count) = parsed.num_matches {
            let _ = writeln!(view, "total matches: {count}");
        }
    }
    CompressionOutcome::pick_smaller(raw, view)
}

/// Re-group grep content-mode rows (`<abs path>:<line>:<text>` or
/// `<abs path>:<text>`) per file. Rows that do not match any known filename
/// prefix are kept verbatim, so unknown shapes degrade gracefully.
fn group_grep_content(content: &str, filenames: &[String], root: Option<&Path>) -> String {
    // Longest-first so nested paths (a.rs vs a.rs.bak) match correctly.
    let mut by_len: Vec<&String> = filenames.iter().collect();
    by_len.sort_by_key(|path| std::cmp::Reverse(path.len()));

    let mut out = String::with_capacity(content.len());
    let mut current_file: Option<&str> = None;
    for row in content.lines() {
        let mut matched: Option<(&str, &str)> = None;
        for path in &by_len {
            if let Some(rest) = row.strip_prefix(path.as_str()) {
                if let Some(rest) = rest.strip_prefix(':') {
                    matched = Some((path.as_str(), rest));
                    break;
                }
            }
        }
        if let Some((path, rest)) = matched {
            if current_file != Some(path) {
                let _ = writeln!(out, "▸ {}", relativize(path, root));
                current_file = Some(path);
            }
            out.push_str(rest);
            out.push('\n');
        } else {
            out.push_str(row);
            out.push('\n');
            current_file = None;
        }
    }
    out
}

fn compress_glob(raw: &str, workspace_root: Option<&Path>) -> CompressionOutcome {
    let Ok(parsed) = serde_json::from_str::<GlobSearchOutput>(raw) else {
        return CompressionOutcome::unchanged(raw);
    };
    let root = root_for_paths(workspace_root);
    let mut view = String::with_capacity(raw.len() / 2);
    let mut header = format!("[glob] {} files", parsed.num_files);
    if parsed.truncated {
        header.push_str(" (truncated at 100)");
    }
    if let Some(root) = &root {
        let _ = write!(header, " · paths relative to {}", root.display());
    }
    view.push_str(&header);
    view.push('\n');
    for path in &parsed.filenames {
        view.push_str(&relativize(path, root.as_deref()));
        view.push('\n');
    }
    CompressionOutcome::pick_smaller(raw, view)
}

/// `imports: a, b · module: c · tests: d`, each path relative to `root` the
/// way grep and glob paths are ([`relativize`]), grouped in that order.
fn render_neighbours(
    neighbours: &[crate::file_neighbours::Neighbour],
    root: Option<&Path>,
) -> String {
    use crate::file_neighbours::Relation;
    let mut parts = Vec::new();
    for relation in [Relation::Imports, Relation::Module, Relation::Tests] {
        let paths: Vec<String> = neighbours
            .iter()
            .filter(|neighbour| neighbour.relation == relation)
            .map(|neighbour| relativize(&neighbour.path, root))
            .collect();
        if !paths.is_empty() {
            parts.push(format!("{}: {}", relation.label(), paths.join(", ")));
        }
    }
    parts.join(" · ")
}

fn root_for_paths(workspace_root: Option<&Path>) -> Option<std::path::PathBuf> {
    workspace_root
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
}

fn relativize(path: &str, root: Option<&Path>) -> String {
    if let Some(root) = root {
        if let Ok(stripped) = Path::new(path).strip_prefix(root) {
            let rendered = stripped.to_string_lossy();
            if !rendered.is_empty() {
                return rendered.into_owned();
            }
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_ops::TextFilePayload;

    /// `Lossless` forbids the views that elide content; `Full` permits them.
    ///
    /// This is what protects the result the model just asked for. An oversized
    /// read is summarized for history — a code file to an outline, anything else
    /// to a head and a tail — and answering "show me this file" with a
    /// description of it leaves the model unable to see what it requested.
    #[test]
    fn lossless_rewrite_keeps_an_oversized_read_whole() {
        let body = (0..4_000)
            .map(|line| format!("<p>line {line} of the page</p>"))
            .collect::<Vec<_>>()
            .join("\n");
        let envelope = read_file_envelope("/ws/page.html", &body);

        let fresh = wire_tool_output(&envelope, "read_file", false, WireRewrite::Lossless);
        assert!(fresh.contains("line 2000 of the page"), "middle survives");
        assert!(fresh.contains("line 3999 of the page"), "tail survives");
        assert!(!fresh.contains("lines elided"), "no elision marker");

        let historical = wire_tool_output(&envelope, "read_file", false, WireRewrite::Full);
        assert!(historical.contains("lines elided"), "history may elide");
        assert!(!historical.contains("line 2000 of the page"));
        // Both still drop the JSON envelope: that rewrite is lossless, so it
        // applies either way.
        assert!(!fresh.contains("\"filePath\""));
        assert!(!historical.contains("\"filePath\""));
    }

    /// A whole-file code read carries its neighbours on the wire view, one
    /// `[neighbours]` line under the header with paths relative to the
    /// workspace; an envelope without any is rendered exactly as before.
    #[test]
    fn a_read_with_neighbours_lists_them_under_the_header() {
        let content = "use crate::x::X;\nfn main() {}\n";
        let mut parsed: ReadFileOutput =
            serde_json::from_str(&read_file_envelope("/ws/src/main.rs", content)).unwrap();
        parsed.file.neighbours = vec![
            crate::file_neighbours::Neighbour {
                relation: crate::file_neighbours::Relation::Imports,
                path: "/ws/src/x.rs".to_string(),
            },
            crate::file_neighbours::Neighbour {
                relation: crate::file_neighbours::Relation::Tests,
                path: "/ws/src/main/tests.rs".to_string(),
            },
        ];
        let raw = serde_json::to_string_pretty(&parsed).unwrap();
        let view = compress_tool_output(&raw, "read_file", Some(std::path::Path::new("/ws"))).content;
        let mut lines = view.lines();
        assert_eq!(lines.next().unwrap(), "[file] /ws/src/main.rs · lines 1-2 of 2");
        assert_eq!(
            lines.next().unwrap(),
            "[neighbours] imports: src/x.rs · tests: src/main/tests.rs"
        );
        assert_eq!(lines.next().unwrap(), "use crate::x::X;");
        let plain = compress_tool_output(&read_file_envelope("/ws/src/main.rs", content), "read_file", None).content;
        assert!(!plain.contains("[neighbours]"));
    }

    fn read_file_envelope(path: &str, content: &str) -> String {
        let total = content.lines().count();
        serde_json::to_string_pretty(&ReadFileOutput {
            kind: "text".to_string(),
            file: TextFilePayload {
                file_path: path.to_string(),
                content: content.to_string(),
                num_lines: total,
                start_line: 1,
                total_lines: total,
                notice: None,
                content_truncated: false,
                original_content_chars: None,
                neighbours: Vec::new(),
            },
        })
        .expect("serialize test envelope")
    }

    // --- wire memo ---

    #[test]
    fn wire_memo_hit_matches_uncached_view() {
        // A repeated call (the cache-hit path) must return exactly what the
        // uncached transform produces — determinism is the load-bearing
        // invariant that keeps the memo prompt-cache-safe and token-neutral.
        let content = "fn main() {\n    let x = 1;\n}\n".repeat(30);
        let raw = read_file_envelope("/ws/src/lib.rs", &content);
        let first = wire_tool_output(&raw, "read_file", false, WireRewrite::Full); // miss → computes + caches
        let second = wire_tool_output(&raw, "read_file", false, WireRewrite::Full); // hit
        assert_eq!(first, second, "cache hit must equal the first computation");
        assert_eq!(
            first,
            compress_wire_view(&raw, "read_file", WireRewrite::Full),
            "cached view must equal the uncached transform"
        );
    }

    #[test]
    fn wire_memo_distinct_inputs_do_not_collide() {
        let a = wire_tool_output(&read_file_envelope("/ws/a.rs", "fn a() {}\n"), "read_file", false, WireRewrite::Full);
        let b = wire_tool_output(&read_file_envelope("/ws/b.rs", "fn b() {}\n"), "read_file", false, WireRewrite::Full);
        assert_ne!(a, b, "different file contents must not share a cached view");
    }

    // --- read_file ---

    #[test]
    fn read_file_unwrap_is_lossless_and_smaller() {
        let content = "fn main() {\n    println!(\"hi \\\"there\\\"\");\n}\n".repeat(40);
        let raw = read_file_envelope("/ws/src/main.rs", &content);
        let out = compress_tool_output(&raw, "read_file", None);
        assert!(out.was_compressed, "expected a rewrite");
        assert!(out.compressed_chars < out.original_chars);
        // Lossless: the exact file bytes appear after the single header line.
        let (header, body) = out.content.split_once('\n').expect("header line");
        assert!(header.starts_with("[file] /ws/src/main.rs"));
        assert_eq!(body, content, "file content must be byte-identical");
    }

    #[test]
    fn read_file_header_reports_line_window() {
        let raw = serde_json::to_string_pretty(&ReadFileOutput {
            kind: "text".to_string(),
            file: TextFilePayload {
                file_path: "/ws/a.rs".to_string(),
                content: "line\n".repeat(300),
                num_lines: 300,
                start_line: 101,
                total_lines: 900,
                notice: None,
                content_truncated: false,
                original_content_chars: None,
                neighbours: Vec::new(),
            },
        })
        .unwrap();
        let out = compress_tool_output(&raw, "read_file", None);
        assert!(out.was_compressed);
        assert!(out
            .content
            .starts_with("[file] /ws/a.rs · lines 101-400 of 900"));
    }

    #[test]
    fn read_file_garbage_fails_open() {
        let out = compress_tool_output("not json at all", "read_file", None);
        assert!(!out.was_compressed);
        assert_eq!(out.content, "not json at all");
    }

    #[test]
    fn oversized_non_code_read_is_bounded_with_reread_window() {
        // A large .log has no code outline, but the lossless unwrap would flood
        // the wire — it must be bounded head+tail with an exact re-read window.
        let raw = serde_json::to_string_pretty(&ReadFileOutput {
            kind: "text".to_string(),
            file: TextFilePayload {
                file_path: "/ws/server.log".to_string(),
                content: {
                    use std::fmt::Write as _;
                    let mut s = String::new();
                    for n in 1..=2000 {
                        let _ = writeln!(s, "entry-{n} event occurred with some padding detail");
                    }
                    s
                },
                num_lines: 2000,
                start_line: 1,
                total_lines: 2000,
                notice: None,
                content_truncated: false,
                original_content_chars: None,
                neighbours: Vec::new(),
            },
        })
        .unwrap();
        let out = compress_tool_output(&raw, "read_file", None);
        assert!(out.was_compressed, "oversized non-code read must be bounded");
        assert!(out.compressed_chars < out.original_chars);
        assert!(out.content.contains("lines elided — re-read: read_file"));
        assert!(out.content.contains("entry-1 "), "head retained");
        assert!(out.content.contains("entry-2000 "), "tail retained");
        assert!(
            !out.content.contains("entry-1000 "),
            "middle must be elided, not sent whole"
        );
    }

    #[test]
    fn pascalcase_tool_names_still_compress() {
        // Claude sends Claude-Code PascalCase names on the wire. `Read` must reach
        // the same compressor as `read_file` — otherwise wire compression
        // silently no-ops for the entire common Claude case (the headline
        // compression advantage was being forfeited). Case-insensitive too.
        let content = "fn main() {}\n".repeat(60);
        let raw = read_file_envelope("/ws/src/main.rs", &content);
        let snake = compress_tool_output(&raw, "read_file", None);
        for alias in ["Read", "READ", "read"] {
            let out = compress_tool_output(&raw, alias, None);
            assert!(out.was_compressed, "{alias} must compress like read_file");
            assert_eq!(
                out.content, snake.content,
                "{alias} must produce identical compression to read_file"
            );
        }
    }

    #[test]
    fn enriched_output_with_trailing_notes_fails_open() {
        // write/edit enrichment appends notes after the JSON envelope; that no
        // longer parses as the envelope alone, so it must pass through.
        let mut raw = read_file_envelope("/ws/a.rs", "x\n");
        raw.push_str("\n[auto-format] reformatted");
        let out = compress_tool_output(&raw, "read_file", None);
        assert!(!out.was_compressed);
    }

    #[test]
    fn wire_tool_output_compresses_body_with_recovery_notice() {
        let content = "fn main() { println!(\"hi\"); }\n".repeat(80);
        let mut raw = read_file_envelope("/ws/a.rs", &content);
        raw.push_str(
            "\n[full output preserved — call retrieve_tool_output {\"sha256\": \"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\"}; window large outputs with offset/limit (0-based lines)]",
        );

        let wired = wire_tool_output(&raw, "read_file", false, WireRewrite::Full);

        assert!(
            wired.starts_with("[file] /ws/a.rs"),
            "known recovery notices must not prevent model-facing compression: {wired}"
        );
        assert!(wired.contains("retrieve_tool_output"));
    }

    #[test]
    fn wire_tool_output_extracts_structured_recovery_notice_before_compressing() {
        let content = "fn main() { println!(\"hi\"); }\n".repeat(80);
        let mut value: serde_json::Value =
            serde_json::from_str(&read_file_envelope("/ws/a.rs", &content)).expect("json");
        value["recoveryNotice"] = serde_json::Value::String(
            "[full output preserved — call retrieve_tool_output {\"sha256\": \"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\"}; window large outputs with offset/limit (0-based lines)]".to_string(),
        );
        let raw = serde_json::to_string_pretty(&value).expect("json");

        let wired = wire_tool_output(&raw, "read_file", false, WireRewrite::Full);

        assert!(
            wired.starts_with("[file] /ws/a.rs"),
            "structured notice must not block read_file compression: {wired}"
        );
        assert!(wired.contains("retrieve_tool_output"));
        assert!(!wired.contains("recoveryNotice"));
    }

    // --- bash ---

    fn bash_envelope(stdout: &str, stderr: &str) -> String {
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
            "returnCodeInterpretation": null,
            "noOutputExpected": null,
            "structuredContent": null,
            "persistedOutputPath": null,
            "persistedOutputSize": null,
            "sandboxStatus": null,
        }))
        .unwrap()
    }

    #[test]
    fn bash_unwrap_preserves_stdout_and_stderr() {
        let stdout = "Compiling zo v0.1.0\nFinished dev in 2.31s\n".repeat(30);
        let stderr = "warning: unused variable `x`\n";
        let raw = bash_envelope(&stdout, stderr);
        let out = compress_tool_output(&raw, "bash", None);
        assert!(out.was_compressed);
        assert!(out.content.starts_with("[bash]\n"));
        assert!(out.content.contains("Compiling zo v0.1.0"));
        assert!(out
            .content
            .contains("── stderr ──\nwarning: unused variable `x`"));
        assert!(out.compressed_chars < out.original_chars);
    }

    #[test]
    fn bash_strips_ansi_escapes() {
        let stdout = "\u{1b}[32mok\u{1b}[0m test result\n".repeat(20);
        let raw = bash_envelope(&stdout, "");
        let out = compress_tool_output(&raw, "bash", None);
        assert!(out.was_compressed);
        assert!(!out.content.contains('\u{1b}'));
        assert!(out.content.contains("ok test result"));
    }

    #[test]
    fn bash_collapses_repeated_lines_with_marker() {
        let stdout = format!("{}{}", "spinner tick\n".repeat(50), "done\n");
        let raw = bash_envelope(&stdout, "");
        let out = compress_tool_output(&raw, "bash", None);
        assert!(out.was_compressed);
        assert!(out.content.contains("spinner tick\n⟨repeated ×50⟩"));
        assert!(out.content.contains("done"));
        // Collapsed exactly once — the line itself plus the marker.
        assert_eq!(out.content.matches("spinner tick").count(), 1);
    }

    #[test]
    fn cargo_test_log_is_failure_first_and_collapses_passing_noise() {
        let mut stdout = String::from("running 4 tests\n");
        stdout.push_str("test alpha::passes ... ok\n");
        stdout.push_str("test beta::passes ... ok\n");
        stdout.push_str("test gamma::fails ... FAILED\n");
        stdout.push_str("test delta::ignored ... ignored\n\n");
        stdout.push_str("failures:\n\n");
        stdout.push_str("---- gamma::fails stdout ----\n");
        stdout.push_str("thread 'gamma::fails' panicked at crates/demo.rs:42:9:\nexpected true\n\n");
        stdout.push_str("failures:\n    gamma::fails\n\n");
        stdout.push_str("test result: FAILED. 2 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s\n");
        let raw = bash_envelope(&stdout, "");

        let out = compress_tool_output(&raw, "bash", None);

        assert!(out.was_compressed);
        let summary = out.content.find("test result: FAILED").expect("summary kept");
        let failure_detail = out
            .content
            .find("---- gamma::fails stdout ----")
            .expect("failure detail kept");
        let collapsed = out
            .content
            .find("2 passed tests collapsed")
            .expect("passing noise collapsed");
        assert!(summary < failure_detail, "summary leads failure details");
        assert!(failure_detail < collapsed, "collapse marker follows failures");
        assert!(!out.content.contains("test alpha::passes ... ok"));
        assert!(out.compressed_chars < out.original_chars);
    }

    #[test]
    fn non_cargo_log_uses_existing_repeat_compaction() {
        let text = format!(
            "{}{}{}",
            "\u{1b}[32mspin\u{1b}[0m\n".repeat(10),
            "unique\n",
            "done"
        );
        let expected = collapse_repeats(&strip_ansi(&text));

        assert_eq!(compact_log_text(&text), expected);
    }

    #[test]
    fn bash_structured_content_fails_open() {
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "stdout": "x".repeat(500),
            "stderr": "",
            "rawOutputPath": null,
            "interrupted": false,
            "isImage": null,
            "backgroundTaskId": null,
            "backgroundedByUser": null,
            "assistantAutoBackgrounded": null,
            "dangerouslyDisableSandbox": null,
            "returnCodeInterpretation": null,
            "noOutputExpected": null,
            "structuredContent": [{"type": "x"}],
            "persistedOutputPath": null,
            "persistedOutputSize": null,
            "sandboxStatus": null,
        }))
        .unwrap();
        let out = compress_tool_output(&raw, "bash", None);
        assert!(!out.was_compressed, "structured content must pass through");
    }

    /// Envelope carrying a real serialized [`crate::sandbox::SandboxStatus`],
    /// so the fixture can never drift from the struct's serde shape (a
    /// hand-written status with wrong field names fails envelope parsing and
    /// silently tests nothing).
    fn bash_envelope_with_sandbox(status: &crate::sandbox::SandboxStatus) -> String {
        serde_json::to_string_pretty(&serde_json::json!({
            "stdout": "ok\n".repeat(40),
            "stderr": "",
            "rawOutputPath": null,
            "interrupted": false,
            "isImage": null,
            "backgroundTaskId": null,
            "backgroundedByUser": null,
            "assistantAutoBackgrounded": null,
            "dangerouslyDisableSandbox": null,
            "returnCodeInterpretation": null,
            "noOutputExpected": null,
            "structuredContent": null,
            "persistedOutputPath": null,
            "persistedOutputSize": null,
            "sandboxStatus": serde_json::to_value(status).expect("status serializes"),
        }))
        .unwrap()
    }

    #[test]
    fn bash_omits_inactive_sandbox_status_but_keeps_signal() {
        // The default status is the session-invariant common case: nothing
        // requested, nothing active, no degradation — pure noise per call.
        let inactive = bash_envelope_with_sandbox(&crate::sandbox::SandboxStatus::default());
        let inactive_out = compress_tool_output(&inactive, "bash", None);
        assert!(inactive_out.was_compressed);
        assert!(!inactive_out.content.contains("sandbox:"));

        // Any of active / enabled / requested / degraded is signal and must
        // still render.
        for signal in [
            crate::sandbox::SandboxStatus {
                active: true,
                ..Default::default()
            },
            crate::sandbox::SandboxStatus {
                enabled: true,
                ..Default::default()
            },
            crate::sandbox::SandboxStatus {
                requested: crate::sandbox::SandboxRequest {
                    enabled: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            crate::sandbox::SandboxStatus {
                fallback_reason: Some("namespace isolation unavailable".to_string()),
                ..Default::default()
            },
        ] {
            let out = compress_tool_output(&bash_envelope_with_sandbox(&signal), "bash", None);
            assert!(out.was_compressed);
            assert!(
                out.content.contains("sandbox:"),
                "signal-bearing status must render: {signal:?}"
            );
        }
    }

    #[test]
    fn bash_meta_fields_survive_in_header() {
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "stdout": "partial output\n".repeat(20),
            "stderr": "",
            "rawOutputPath": null,
            "interrupted": true,
            "isImage": null,
            "backgroundTaskId": "task-7",
            "backgroundedByUser": null,
            "assistantAutoBackgrounded": true,
            "dangerouslyDisableSandbox": null,
            "returnCodeInterpretation": "exit code 1 (failure)",
            "noOutputExpected": null,
            "structuredContent": null,
            "persistedOutputPath": "/tmp/full.log",
            "persistedOutputSize": 12345,
            "sandboxStatus": null,
        }))
        .unwrap();
        let out = compress_tool_output(&raw, "bash", None);
        assert!(out.was_compressed);
        let header = out.content.lines().next().unwrap();
        assert!(header.contains("interrupted"));
        assert!(header.contains("background_task_id=task-7"));
        assert!(header.contains("auto_backgrounded"));
        assert!(header.contains("exit: exit code 1 (failure)"));
        assert!(header.contains("full output: /tmp/full.log (12345 bytes)"));
    }

    // --- grep ---

    #[test]
    fn grep_content_mode_groups_rows_per_file() {
        let root = Path::new("/ws");
        let file_a = "/ws/crates/runtime/src/bash.rs";
        let file_b = "/ws/crates/tools/src/dispatch.rs";
        let content = format!(
            "{file_a}:10:fn execute() {{\n{file_a}:42:    execute_inner()\n{file_b}:7:use runtime::execute;\n"
        );
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "mode": "content",
            "numFiles": 2,
            "filenames": [file_a, file_b],
            "content": content.trim_end(),
            "numLines": 3,
            "numMatches": null,
            "appliedLimit": null,
            "appliedOffset": null,
        }))
        .unwrap();
        let out = compress_tool_output(&raw, "grep_search", Some(root));
        assert!(out.was_compressed);
        // Paths appear once per file, relativized; every row survives.
        assert_eq!(out.content.matches("crates/runtime/src/bash.rs").count(), 1);
        assert!(out.content.contains("▸ crates/runtime/src/bash.rs"));
        assert!(out.content.contains("10:fn execute() {"));
        assert!(out.content.contains("42:    execute_inner()"));
        assert!(out.content.contains("▸ crates/tools/src/dispatch.rs"));
        assert!(out.content.contains("7:use runtime::execute;"));
        assert!(out.content.contains("paths relative to /ws"));
    }

    /// End-to-end contract with `file_ops::grep_search`: an offset-paged
    /// content window still groups, because the tool derives `filenames` from
    /// the rows that survive the window (they stay valid path prefixes).
    #[test]
    fn grep_offset_paged_output_still_groups_rows_per_file() {
        let dir = std::env::temp_dir().join(format!(
            "zo-grep-group-offset-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should move forward")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        // grep_search normalizes paths (e.g. /var → /private/var on macOS);
        // hand the compressor the same canonical root so relativize applies.
        let dir = dir.canonicalize().expect("canonicalize");
        std::fs::write(dir.join("a.txt"), "needle 1\nneedle 2\nneedle 3\n").expect("write a");
        std::fs::write(dir.join("b.txt"), "needle 4\nneedle 5\nneedle 6\n").expect("write b");

        let output = crate::file_ops::grep_search(&crate::file_ops::GrepSearchInput {
            pattern: String::from("needle"),
            path: Some(dir.to_string_lossy().into_owned()),
            glob: None,
            output_mode: Some(String::from("content")),
            before: None,
            after: None,
            context_short: None,
            context: None,
            line_numbers: Some(true),
            case_insensitive: None,
            file_type: None,
            head_limit: Some(3),
            offset: Some(3),
            multiline: None,
        })
        .expect("grep should succeed");
        let raw = serde_json::to_string_pretty(&output).expect("serialize");

        let out = compress_tool_output(&raw, "grep_search", Some(&dir));
        std::fs::remove_dir_all(&dir).ok();

        assert!(out.was_compressed);
        // Grouped: one `▸ <file>` header, and no absolute-path row leaks
        // (a stray `<tmp>/<file>.txt:` prefix would mean grouping fell apart).
        assert_eq!(out.content.matches("▸ ").count(), 1);
        let dir_prefix = dir.to_string_lossy().into_owned();
        assert!(
            !out.content.contains(&format!("{dir_prefix}/")),
            "rows must be grouped under a relativized header, got:\n{}",
            out.content
        );
    }

    #[test]
    fn grep_files_mode_relativizes_paths() {
        let root = Path::new("/ws");
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "mode": "files_with_matches",
            "numFiles": 2,
            "filenames": ["/ws/a/long/path/one.rs", "/ws/a/long/path/two.rs"],
            "content": null,
            "numLines": null,
            "numMatches": null,
            "appliedLimit": null,
            "appliedOffset": null,
        }))
        .unwrap();
        let out = compress_tool_output(&raw, "grep_search", Some(root));
        assert!(out.was_compressed);
        assert!(out.content.contains("a/long/path/one.rs\n"));
        assert!(!out.content.contains("\"/ws/a/long/path/one.rs\""));
    }

    #[test]
    fn grep_unknown_rows_kept_verbatim() {
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "mode": "content",
            "numFiles": 1,
            "filenames": ["/ws/a.rs"],
            "content": format!("{}\nsome stray row without a path prefix", "/ws/a.rs:1:hit"),
            "numLines": 2,
            "numMatches": null,
            "appliedLimit": null,
            "appliedOffset": null,
        }))
        .unwrap();
        let out = compress_tool_output(&raw, "grep_search", Some(Path::new("/ws")));
        if out.was_compressed {
            assert!(out.content.contains("some stray row without a path prefix"));
        }
    }

    // --- glob ---

    #[test]
    fn glob_list_relativizes_and_keeps_count() {
        let paths: Vec<String> = (0..40)
            .map(|i| format!("/ws/crates/runtime/src/module_{i}.rs"))
            .collect();
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "durationMs": 12,
            "numFiles": paths.len(),
            "filenames": paths,
            "truncated": true,
        }))
        .unwrap();
        let out = compress_tool_output(&raw, "glob_search", Some(Path::new("/ws")));
        assert!(out.was_compressed);
        assert!(out
            .content
            .starts_with("[glob] 40 files (truncated at 100)"));
        assert!(out.content.contains("crates/runtime/src/module_0.rs\n"));
        assert!(out.compressed_chars < out.original_chars);
    }

    // --- edit_file / write_file ---

    #[test]
    fn edit_file_drops_echo_keeps_patch_and_feedback() {
        let old_body = "    let total = items.iter().sum::<usize>();\n".repeat(40);
        let new_body = "    let total: usize = items.iter().copied().sum();\n".repeat(40);
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "filePath": "/ws/src/lib.rs",
            "oldString": old_body,
            "newString": new_body,
            "structuredPatch": [{
                "oldStart": 10, "oldLines": 3, "newStart": 10, "newLines": 3,
                "lines": [" fn sum() {", "-    let total = 0;", "+    let total: usize = 0;", " }"],
            }],
            "userModified": false,
            "replaceAll": false,
            "gitDiff": null,
            "toolFeedback": "[auto-format] rustfmt: reformatted",
        }))
        .unwrap();
        let out = compress_tool_output(&raw, "edit_file", None);
        assert!(out.was_compressed);
        assert!(out.content.starts_with("[edit] /ws/src/lib.rs · applied"));
        assert!(out.content.contains("@@ -10,3 +10,3 @@"));
        assert!(out.content.contains("-    let total = 0;"));
        assert!(out.content.contains("+    let total: usize = 0;"));
        assert!(out.content.contains("[auto-format] rustfmt: reformatted"));
        // The echoed arguments are gone.
        assert!(!out.content.contains("copied().sum()"));
        assert!(
            out.compressed_chars < out.original_chars / 4,
            "echo dominates; expect >75% cut"
        );
    }

    /// `MultiEdit` is `edit_file`'s envelope under another name, and r31 found
    /// it was the one file-editing result still carrying its argument echo on
    /// the wire: its canonical name is `multiedit`, and the dispatch above only
    /// knew `edit`/`edit_file`. Same view, plus the edit count.
    #[test]
    fn multiedit_result_gets_the_edit_wire_view() {
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "filePath": "/ws/src/lib.rs",
            "editsApplied": 2,
            "structuredPatch": [{
                "oldStart": 4, "oldLines": 2, "newStart": 4, "newLines": 2,
                "lines": [" fn a() {", "-    let x = 1;", "+    let x: u8 = 1;", " }"],
            }],
            "userModified": false,
            "gitDiff": null,
            "edits": [
                {"old_string": "x".repeat(400), "new_string": "y".repeat(400)},
                {"old_string": "p".repeat(400), "new_string": "q".repeat(400)},
            ],
        }))
        .unwrap();
        for name in ["MultiEdit", "multiedit", "multi_edit"] {
            let out = compress_tool_output(&raw, name, None);
            assert!(out.was_compressed, "{name} must take the edit view");
            assert!(
                out.content.starts_with("[edit] /ws/src/lib.rs · applied · 2 edits"),
                "{name}: {}",
                out.content
            );
            assert!(out.content.contains("+    let x: u8 = 1;"));
            assert!(!out.content.contains("yyyy"), "the argument echo must be gone");
        }
    }

    #[test]
    fn edit_file_flags_surface_in_header() {
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "filePath": "/ws/a.rs",
            "oldString": "x".repeat(300),
            "newString": "y".repeat(300),
            "structuredPatch": [],
            "userModified": true,
            "replaceAll": true,
            "gitDiff": null,
        }))
        .unwrap();
        let out = compress_tool_output(&raw, "edit_file", None);
        assert!(out.was_compressed);
        let header = out.content.lines().next().unwrap();
        assert!(header.contains("replace_all"));
        assert!(header.contains("user_modified"));
    }

    #[test]
    fn edit_file_without_patch_field_fails_open() {
        let raw = r#"{"filePath": "/ws/a.rs", "somethingElse": true}"#;
        let out = compress_tool_output(raw, "edit_file", None);
        assert!(!out.was_compressed);
    }

    #[test]
    fn write_file_drops_content_echo() {
        let content = "fn generated() -> usize {\n    42\n}\n".repeat(60);
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "type": "create",
            "filePath": "/ws/gen.rs",
            "content": content,
            "structuredPatch": [{
                "oldStart": 0, "oldLines": 0, "newStart": 1, "newLines": 2,
                "lines": ["+fn generated() -> usize {", "+    42"],
            }],
            "gitDiff": null,
        }))
        .unwrap();
        let out = compress_tool_output(&raw, "write_file", None);
        assert!(out.was_compressed);
        assert!(out
            .content
            .starts_with("[write] /ws/gen.rs · written · 180 lines"));
        assert!(out.content.contains("+fn generated() -> usize {"));
        assert!(out.compressed_chars < out.original_chars / 5);
    }

    // --- read_file outline (large code files) ---

    /// A synthetic rustfmt-shaped source big enough to clear
    /// [`OUTLINE_THRESHOLD_CHARS`] after the lossless unwrap.
    fn large_rust_source() -> String {
        let mut src = String::new();
        src.push_str("//! Synthetic module for outline tests.\n\nuse std::fmt::Write as _;\n\n");
        for item in 0..120 {
            let _ = writeln!(src, "/// Doc for item {item}.");
            let _ = writeln!(src, "pub fn function_{item}(input: usize) -> usize {{");
            for line in 0..8 {
                let _ = writeln!(
                    src,
                    "        let value_{line} = input.wrapping_mul({line}) + {item}; // body detail"
                );
            }
            src.push_str("        value_7\n}\n\n");
        }
        src
    }

    #[test]
    fn oversized_code_file_gets_outline_with_reversible_markers() {
        let content = large_rust_source();
        assert!(
            content.chars().count() > OUTLINE_THRESHOLD_CHARS,
            "fixture must be oversized"
        );
        let raw = read_file_envelope("/ws/big.rs", &content);
        let out = compress_tool_output(&raw, "read_file", None);
        assert!(out.was_compressed);
        assert!(out.content.starts_with("[file:outline] /ws/big.rs"));
        // Structure survives: every signature and doc line is visible.
        assert!(out
            .content
            .contains("pub fn function_0(input: usize) -> usize {"));
        assert!(out
            .content
            .contains("pub fn function_119(input: usize) -> usize {"));
        assert!(out.content.contains("/// Doc for item 60."));
        // Bodies are elided.
        assert!(!out.content.contains("value_3 = input.wrapping_mul(3)"));
        assert!(out.content.contains("elided · read_file offset="));
        // And the outline is dramatically smaller than the head-only cut.
        assert!(out.compressed_chars < OUTLINE_THRESHOLD_CHARS);
    }

    /// The r27 ordering pin, and the most valuable assertion in this file.
    ///
    /// A `read_file` body that the truncation middleware already capped must
    /// STILL get the outline. Measured on the corpus: the outline is a tighter
    /// bound than the cap (median 15,456 chars against 30,000), it is re-derived
    /// into the prefix of every later request, and turning it off for a capped
    /// body costs ~49,539 chars per rolling request against the ~131 the cap
    /// saves at append time — a 388x net loss from one ordering decision
    /// (`docs/analysis/tool-caps-r27.md` §4).
    ///
    /// The markers change, though: on a capped body they may no longer name a
    /// `read_file offset/limit` window, because the line numbers behind it are
    /// computed from positions inside a body whose middle is gone.
    #[test]
    fn a_capped_body_still_outlines_but_stops_naming_reread_windows() {
        let content = large_rust_source();
        let capped = crate::truncate_tool_output(
            &read_file_envelope("/ws/big.rs", &content),
            "read_file",
            &crate::TruncationConfig::default(),
        );
        assert!(capped.was_truncated, "fixture must exercise the cap");

        let out = compress_tool_output(&capped.content, "read_file", None);

        assert!(
            out.content.starts_with("[file:outline] /ws/big.rs"),
            "a capped body must still be outlined, got: {}",
            out.content.chars().take(120).collect::<String>()
        );
        assert!(
            out.compressed_chars < OUTLINE_THRESHOLD_CHARS,
            "the outline must stay tighter than the cap itself, got {} chars",
            out.compressed_chars
        );
        assert!(
            !out.content.contains("read_file offset="),
            "a capped body's line numbers are no longer the file's — a marker \
             naming a window here would send the model to other lines"
        );
        assert!(
            out.content.contains("retrieve_tool_output"),
            "so the marker must point at what is still true"
        );
    }

    #[test]
    fn outline_markers_reconstruct_the_original_exactly() {
        let content = large_rust_source();
        let lines: Vec<&str> = content.lines().collect();
        let raw = read_file_envelope("/ws/big.rs", &content);
        let out = compress_tool_output(&raw, "read_file", None);
        assert!(out.content.starts_with("[file:outline]"));

        // Replay the outline: kept lines verbatim, each marker expanded via
        // the read_file window it names. The result must equal the original.
        let mut rebuilt: Vec<String> = Vec::new();
        for line in out.content.lines().skip(1) {
            if let Some(marker) = line.trim_start().strip_prefix('⟨') {
                let offset: usize = marker
                    .split("offset=")
                    .nth(1)
                    .and_then(|rest| rest.split(' ').next())
                    .and_then(|n| n.parse().ok())
                    .expect("marker carries offset");
                let limit: usize = marker
                    .split("limit=")
                    .nth(1)
                    .and_then(|rest| rest.split('⟩').next())
                    .and_then(|n| n.parse().ok())
                    .expect("marker carries limit");
                for original in &lines[offset..offset + limit] {
                    rebuilt.push((*original).to_string());
                }
            } else {
                rebuilt.push(line.to_string());
            }
        }
        assert_eq!(
            rebuilt.len(),
            lines.len(),
            "outline + markers cover every line"
        );
        assert_eq!(
            rebuilt, lines,
            "expansion restores the original byte-for-byte"
        );
    }

    #[test]
    fn oversized_non_code_file_keeps_lossless_unwrap() {
        let content = "prose paragraph without indentation\n".repeat(1_200);
        assert!(content.chars().count() > OUTLINE_THRESHOLD_CHARS);
        let raw = read_file_envelope("/ws/notes.md", &content);
        let out = compress_tool_output(&raw, "read_file", None);
        assert!(out.was_compressed);
        assert!(
            out.content.starts_with("[file] /ws/notes.md"),
            "no outline for prose"
        );
    }

    #[test]
    fn small_code_file_is_not_outlined() {
        let content = "pub fn tiny() {\n        let deep_body_line = 1;\n        let another = 2;\n        let third = 3;\n        let fourth = 4;\n}\n"
            .repeat(20);
        assert!(content.chars().count() < OUTLINE_THRESHOLD_CHARS);
        let raw = read_file_envelope("/ws/small.rs", &content);
        let out = compress_tool_output(&raw, "read_file", None);
        assert!(out.was_compressed);
        assert!(
            out.content.starts_with("[file] /ws/small.rs"),
            "lossless unwrap only"
        );
        assert!(out.content.contains("deep_body_line"));
    }

    #[test]
    fn outline_respects_windowed_reads() {
        // A read with offset: marker line numbers must be absolute, not
        // window-relative. Window starts at line 101 (offset=100).
        let content = large_rust_source();
        let window: Vec<&str> = content.lines().skip(100).collect();
        let windowed = window.join("\n");
        let raw = serde_json::to_string_pretty(&ReadFileOutput {
            kind: "text".to_string(),
            file: TextFilePayload {
                file_path: "/ws/big.rs".to_string(),
                content: windowed,
                num_lines: window.len(),
                start_line: 101,
                total_lines: content.lines().count(),
                notice: None,
                content_truncated: false,
                original_content_chars: None,
                neighbours: Vec::new(),
            },
        })
        .unwrap();
        let out = compress_tool_output(&raw, "read_file", None);
        assert!(out.content.starts_with("[file:outline]"));
        let first_marker_offset: usize = out
            .content
            .split("offset=")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .and_then(|n| n.parse().ok())
            .expect("has marker");
        assert!(
            first_marker_offset >= 100,
            "offsets are absolute file positions, got {first_marker_offset}"
        );
    }

    // --- guarantees ---

    #[test]
    fn other_tools_pass_through_untouched() {
        let raw = r#"{"anything": "goes"}"#;
        let out = compress_tool_output(raw, "edit_file", None);
        assert!(!out.was_compressed);
        assert_eq!(out.content, raw);
    }

    #[test]
    fn rewrite_below_min_savings_keeps_original() {
        // A candidate that saves less than MIN_SAVINGS_CHARS is rejected.
        let raw = "a".repeat(100);
        let near_same = "b".repeat(100 - MIN_SAVINGS_CHARS + 1);
        let out = CompressionOutcome::pick_smaller(&raw, near_same);
        assert!(!out.was_compressed);
        assert_eq!(out.content, raw);

        let clearly_smaller = "b".repeat(100 - MIN_SAVINGS_CHARS);
        let out = CompressionOutcome::pick_smaller(&raw, clearly_smaller);
        assert!(out.was_compressed);
    }

    #[test]
    fn strip_ansi_handles_csi_osc_and_two_byte() {
        let input = "a\u{1b}[1;32mb\u{1b}[0mc\u{1b}]0;title\u{7}d\u{1b}(Be";
        assert_eq!(strip_ansi(input), "abcde");
    }

    #[test]
    fn collapse_keeps_short_runs_verbatim() {
        let input = "x\nx\nx\ny\n";
        assert_eq!(collapse_repeats(input), input);
    }

    #[test]
    fn collapse_preserves_missing_trailing_newline() {
        let input = "a\nb";
        assert_eq!(collapse_repeats(input), "a\nb");
    }
}
