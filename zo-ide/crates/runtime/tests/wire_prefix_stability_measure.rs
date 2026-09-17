//! Measurement harness: how stable is the request prefix across the
//! consecutive requests of one real session?
//!
//! This exists because the Lossless→Full question cannot be answered by
//! reading the code. Both halves of the policy are individually defensible —
//! the newest tool result must not be handed back elided (the model asked for
//! it *this* turn), and history must be elidable (the context budget matters
//! more than re-reading it verbatim) — so what matters is the SIZE of what the
//! transition costs on real transcripts, which only measurement can say.
//!
//! Ignored by default: it reads sessions from disk and reports numbers rather
//! than asserting a contract. Run it against real data with
//!
//! ```text
//! ZO_MEASURE_SESSIONS=/path/a.jsonl:/path/b.jsonl \
//!   cargo test -p runtime --test wire_prefix_stability_measure -- --ignored --nocapture
//! ```
//!
//! or point `ZO_MEASURE_SESSION_DIR` at a directory to sweep every
//! `session-*.jsonl` under it.
//!
//! What it replays is the production path itself — `convert_messages` followed
//! by `mark_conversation_cache_breakpoints` — so the numbers describe the
//! shipped LOWERING behavior and not a model of it.
//!
//! KNOW WHAT THIS CANNOT SEE. It loads ONE snapshot, freezes it into a single
//! immutable `history`, and builds every simulated request as a prefix
//! `history[..boundary]`. So `history[..b1]` is literally a prefix of
//! `history[..b2]`, and the only thing that can make the measured divergence
//! index fall inside the shared span is `convert_messages` itself being
//! non-monotonic. What this harness measures is therefore LOWERING-PATH
//! MONOTONICITY OVER A FIXED SNAPSHOT — a real and useful property, but it is
//! structurally blind to in-place mutation of a message that has already been
//! sent.
//!
//! That blindness matters because such mutation is real: `microcompact_session`
//! rewrites old tool-result bodies to a placeholder in place, and it IS
//! persisted (the pass ends with `mark_transcript_dirty`, forcing the next
//! persist onto a full snapshot). A persisted transcript is consequently NOT a
//! faithful record of what each live request carried — measured on real data,
//! 54.7% of tool results in live session files are already the placeholder. A
//! "100% pure append" result here is therefore not evidence that the live wire
//! prefix was append-only. Detecting mid-prefix rewrites needs a CROSS-snapshot
//! differ (consecutive `session-*.jsonl` / `*.rot-*.jsonl` siblings), not this.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use core_types::session::{ContentBlock, ConversationMessage, MessageRole, Session};
use runtime::{convert_messages, mark_conversation_cache_breakpoints};

/// The repo's own rough token estimator, used everywhere a byte count has to
/// be reported as tokens. Kept here so this harness quotes the same unit the
/// compaction thresholds are expressed in.
fn tokens(bytes: usize) -> usize {
    bytes / 4
}

/// `part / whole` as a float, via `u32` so the cast is exact. These are
/// reporting counts, not the token totals, so the narrowing is always safe and
/// a saturating conversion keeps it that way.
fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    f64::from(u32::try_from(part).unwrap_or(u32::MAX)) / f64::from(u32::try_from(whole).unwrap_or(u32::MAX))
}

/// Serialized size of one lowered message — what the provider actually bills
/// and hashes for a cache prefix match.
fn message_bytes(message: &api::InputMessage) -> usize {
    serde_json::to_string(message).map_or(0, |text| text.len())
}

/// One simulated request: the lowered messages, with breakpoints applied
/// exactly as the wire path applies them.
struct Request {
    messages: Vec<api::InputMessage>,
    /// Message indices carrying a `cache_control` breakpoint.
    breakpoints: Vec<usize>,
}

impl Request {
    fn build(history: &[ConversationMessage]) -> Self {
        let mut messages = convert_messages(history);
        mark_conversation_cache_breakpoints(&mut messages);
        let breakpoints = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| has_breakpoint(message))
            .map(|(index, _)| index)
            .collect();
        Self {
            messages,
            breakpoints,
        }
    }

    fn bytes_through(&self, end: usize) -> usize {
        self.messages[..end.min(self.messages.len())]
            .iter()
            .map(message_bytes)
            .sum()
    }

    fn total_bytes(&self) -> usize {
        self.bytes_through(self.messages.len())
    }
}

fn has_breakpoint(message: &api::InputMessage) -> bool {
    message.content.iter().any(|block| {
        matches!(
            block,
            api::InputContentBlock::Text {
                cache_control: Some(_),
                ..
            } | api::InputContentBlock::ToolUse {
                cache_control: Some(_),
                ..
            } | api::InputContentBlock::ToolResult {
                cache_control: Some(_),
                ..
            } | api::InputContentBlock::Image {
                cache_control: Some(_),
                ..
            }
        )
    })
}

/// First index at which two consecutive requests differ. Equal to
/// `previous.messages.len()` when the newer request is a pure APPEND — the
/// case the whole prefix cache depends on.
fn divergence_index(previous: &Request, next: &Request) -> usize {
    let shared = previous.messages.len().min(next.messages.len());
    for index in 0..shared {
        let (left, right) = (&previous.messages[index], &next.messages[index]);
        // Compare the BILLED form. `cache_control` is excluded deliberately:
        // a breakpoint moving is not a content change, and the provider
        // matches on content.
        if strip_cache_control(left) != strip_cache_control(right) {
            return index;
        }
    }
    shared
}

fn strip_cache_control(message: &api::InputMessage) -> serde_json::Value {
    let mut value = serde_json::to_value(message).unwrap_or(serde_json::Value::Null);
    if let Some(content) = value.get_mut("content").and_then(|c| c.as_array_mut()) {
        for block in content {
            if let Some(object) = block.as_object_mut() {
                object.remove("cache_control");
            }
        }
    }
    value
}

/// Why a message's lowered form changed between two requests, when it did.
/// Attribution matters: a prefix rewrite caused by the Lossless→Full flip is a
/// different finding from one caused by, say, a reminder being rewritten.
fn attribute(previous: &ConversationMessage, index: usize, history: &[ConversationMessage]) -> &'static str {
    let _ = index;
    let _ = history;
    let has_tool_result = previous
        .blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult { .. }));
    if has_tool_result {
        "tool_result_rewrite"
    } else if previous.role == MessageRole::System {
        "system_reminder_change"
    } else {
        "other"
    }
}

#[derive(Default)]
struct Totals {
    requests: usize,
    pure_appends: usize,
    prefix_rewrites: usize,
    /// Bytes of the previous request that sat AT or AFTER the divergence —
    /// content that was billed (and possibly cache-written) and can never be
    /// read back.
    stranded_bytes: usize,
    /// Bytes still shared as a true prefix at each step.
    reusable_bytes: usize,
    /// Breakpoints whose covered prefix survived intact into the next request.
    breakpoints_readable: usize,
    /// Breakpoints whose covered prefix was invalidated before it could ever
    /// be read.
    breakpoints_stranded: usize,
    causes: BTreeMap<&'static str, usize>,
}

fn measure_session(path: &Path) -> Option<Totals> {
    let session = Session::load_from_path(path).ok()?;
    let history: Vec<ConversationMessage> = session.messages.as_ref().clone();

    // A request is built immediately before each assistant reply, so every
    // assistant message marks one real request boundary.
    let boundaries: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == MessageRole::Assistant)
        .map(|(index, _)| index)
        .collect();
    if boundaries.len() < 2 {
        return None;
    }

    let mut totals = Totals::default();
    let mut previous: Option<Request> = None;
    for &boundary in &boundaries {
        let request = Request::build(&history[..boundary]);
        if let Some(previous) = previous.as_ref() {
            totals.requests += 1;
            let divergence = divergence_index(previous, &request);
            totals.reusable_bytes += previous.bytes_through(divergence);

            if divergence >= previous.messages.len() {
                totals.pure_appends += 1;
                totals.breakpoints_readable += previous.breakpoints.len();
            } else {
                totals.prefix_rewrites += 1;
                totals.stranded_bytes += previous.total_bytes() - previous.bytes_through(divergence);
                for &breakpoint in &previous.breakpoints {
                    // A breakpoint caches the prefix ENDING at it, so it stays
                    // readable only while every message up to and including it
                    // is byte-identical.
                    if breakpoint < divergence {
                        totals.breakpoints_readable += 1;
                    } else {
                        totals.breakpoints_stranded += 1;
                    }
                }
                // Attribute against the SESSION message the diverging lowered
                // message came from. Lowering can drop messages, so walk to the
                // divergence-th surviving message rather than indexing directly.
                if let Some(source) = nth_lowered_source(&history[..boundary], divergence) {
                    *totals
                        .causes
                        .entry(attribute(source, divergence, &history))
                        .or_default() += 1;
                }
            }
        }
        previous = Some(request);
    }
    Some(totals)
}

/// The session message that produced the `target`-th lowered message.
/// `convert_messages` skips empty lowerings and coalesces adjacent tool runs,
/// so the two index spaces are not the same.
fn nth_lowered_source(history: &[ConversationMessage], target: usize) -> Option<&ConversationMessage> {
    let mut lowered = 0usize;
    let mut previous_was_tool = false;
    for message in history {
        let converted = convert_messages(std::slice::from_ref(message));
        if converted.is_empty() {
            continue;
        }
        let is_tool = message.role == MessageRole::Tool;
        let coalesced = is_tool && previous_was_tool;
        if !coalesced {
            if lowered == target {
                return Some(message);
            }
            lowered += 1;
        }
        previous_was_tool = is_tool;
    }
    None
}

fn session_paths() -> Vec<PathBuf> {
    if let Ok(list) = std::env::var("ZO_MEASURE_SESSIONS") {
        return list
            .split(':')
            .filter(|entry| !entry.is_empty())
            .map(PathBuf::from)
            .collect();
    }
    let Ok(dir) = std::env::var("ZO_MEASURE_SESSION_DIR") else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "jsonl")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("session-"))
        })
        .collect();
    paths.sort();
    paths
}

#[test]
#[ignore = "measurement harness: needs real sessions via ZO_MEASURE_SESSIONS / ZO_MEASURE_SESSION_DIR"]
fn measure_wire_prefix_stability() {
    let paths = session_paths();
    assert!(
        !paths.is_empty(),
        "set ZO_MEASURE_SESSIONS (colon-separated) or ZO_MEASURE_SESSION_DIR"
    );

    let mut grand = Totals::default();
    println!(
        "{:<44} {:>7} {:>8} {:>9} {:>12} {:>12}",
        "session", "reqs", "append", "rewrite", "stranded_tok", "reusable_tok"
    );
    for path in &paths {
        let Some(totals) = measure_session(path) else {
            continue;
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .chars()
            .take(44)
            .collect::<String>();
        println!(
            "{:<44} {:>7} {:>8} {:>9} {:>12} {:>12}",
            name,
            totals.requests,
            totals.pure_appends,
            totals.prefix_rewrites,
            tokens(totals.stranded_bytes),
            tokens(totals.reusable_bytes),
        );
        grand.requests += totals.requests;
        grand.pure_appends += totals.pure_appends;
        grand.prefix_rewrites += totals.prefix_rewrites;
        grand.stranded_bytes += totals.stranded_bytes;
        grand.reusable_bytes += totals.reusable_bytes;
        grand.breakpoints_readable += totals.breakpoints_readable;
        grand.breakpoints_stranded += totals.breakpoints_stranded;
        for (cause, count) in totals.causes {
            *grand.causes.entry(cause).or_default() += count;
        }
    }

    println!("\n=== TOTAL over {} session(s) ===", paths.len());
    println!("requests compared      : {}", grand.requests);
    let pct = |part: usize| ratio(part, grand.requests) * 100.0;
    println!(
        "pure appends           : {} ({:.1}%)",
        grand.pure_appends,
        pct(grand.pure_appends)
    );
    println!(
        "prefix rewrites        : {} ({:.1}%)",
        grand.prefix_rewrites,
        pct(grand.prefix_rewrites)
    );
    println!(
        "stranded tokens        : {} (billed, never re-readable)",
        tokens(grand.stranded_bytes)
    );
    println!("reusable prefix tokens : {}", tokens(grand.reusable_bytes));
    println!(
        "breakpoints readable   : {}  stranded: {}",
        grand.breakpoints_readable, grand.breakpoints_stranded
    );
    println!("rewrite causes         : {:?}", grand.causes);
}

/// Independent upper bound on the same question, so the replay above can be
/// trusted or caught: count the tool results whose Lossless and Full wire views
/// actually DIFFER.
///
/// Every such result flips exactly once as history moves past it, so this count
/// is the number of prefix rewrites the flip can possibly cause. If the replay
/// reports far fewer, the replay is missing events; if it reports more,
/// something other than the flip is rewriting prefixes. The two must agree.
#[test]
#[ignore = "measurement harness: needs real sessions via ZO_MEASURE_SESSIONS / ZO_MEASURE_SESSION_DIR"]
fn count_tool_results_whose_wire_view_flips() {
    use runtime::context_compression::{wire_tool_output, WireRewrite};

    let paths = session_paths();
    assert!(!paths.is_empty(), "set ZO_MEASURE_SESSIONS / ZO_MEASURE_SESSION_DIR");

    let (mut results, mut flipping, mut lossless_bytes, mut full_bytes) = (0usize, 0usize, 0usize, 0usize);
    let mut by_tool: BTreeMap<String, usize> = BTreeMap::new();
    for path in &paths {
        let Ok(session) = Session::load_from_path(path) else {
            continue;
        };
        for message in session.messages.iter() {
            for block in &message.blocks {
                let ContentBlock::ToolResult {
                    tool_name,
                    output,
                    is_error,
                    ..
                } = block
                else {
                    continue;
                };
                results += 1;
                let lossless = wire_tool_output(output, tool_name, *is_error, WireRewrite::Lossless);
                let full = wire_tool_output(output, tool_name, *is_error, WireRewrite::Full);
                if lossless != full {
                    flipping += 1;
                    lossless_bytes += lossless.len();
                    full_bytes += full.len();
                    *by_tool.entry(tool_name.clone()).or_default() += 1;
                }
            }
        }
    }

    println!("tool results scanned : {results}");
    println!(
        "views that flip      : {flipping} ({:.3}% of results)",
        ratio(flipping, results) * 100.0
    );
    println!("  lossless tokens    : {}", tokens(lossless_bytes));
    println!("  full tokens        : {}", tokens(full_bytes));
    println!(
        "  delta (stranded)   : {}",
        tokens(lossless_bytes.saturating_sub(full_bytes))
    );
    println!("  by tool            : {by_tool:?}");
}

// ---------------------------------------------------------------------------
// Standing guarantees (run in CI; no real sessions needed)
// ---------------------------------------------------------------------------
//
// The measurement above found ordinary agentic traffic to be 99.8% pure
// appends across 1,148 real sessions and 13,388 consecutive request pairs.
// That number is the health of the whole prefix cache, and nothing pinned it:
// the 2026-08-01 wire-reminder defect — a per-request reminder attached to the
// newest user message and MOVED on the next request — would have driven it to
// roughly zero, and it went unnoticed for weeks. These two tests turn that
// measured property into a contract.

/// Build a realistic agentic transcript: a user ask, then `iterations` rounds
/// of `[reminder, assistant+tool_use, tool results]`.
fn synthetic_session(iterations: usize, read_output: impl Fn(usize) -> String) -> Vec<ConversationMessage> {
    let mut history = vec![ConversationMessage::user_text("refactor the parser")];
    for round in 0..iterations {
        history.push(reminder_message(&format!(
            "<system-reminder>\ntodo: step {round} in progress\n</system-reminder>"
        )));
        history.push(ConversationMessage::assistant(vec![
            ContentBlock::Text {
                text: format!("reading file {round}"),
            },
            ContentBlock::ToolUse {
                id: format!("call_{round}"),
                name: "read_file".to_string(),
                input: format!(r#"{{"path":"src/mod_{round}.rs"}}"#),
            },
        ]));
        history.push(ConversationMessage::tool_result(
            format!("call_{round}"),
            "read_file",
            read_output(round),
            false,
        ));
    }
    history
}

fn reminder_message(text: &str) -> ConversationMessage {
    ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }
}

/// A `read_file` envelope holding `functions` rustfmt-shaped functions.
///
/// The bodies are indented past `OUTLINE_KEEP_INDENT` and long enough to clear
/// `OUTLINE_MIN_ELIDE_RUN`, because that is what the outline view elides — a
/// wall of unindented lines has no structure to keep and outlines to nothing,
/// so it would not exercise the flip at all.
fn read_file_output(path: &str, functions: usize) -> String {
    let mut rendered = Vec::new();
    for index in 0..functions {
        rendered.push(format!("pub fn compute_{index}(input: usize) -> usize {{"));
        for step in 0..8 {
            rendered.push(format!(
                "        let step_{step} = input.wrapping_mul({step}).wrapping_add({index});"
            ));
        }
        rendered.push("        step_0".to_string());
        rendered.push("}".to_string());
        rendered.push(String::new());
    }
    let lines = rendered.len();
    let content = rendered.join("\n");
    serde_json::to_string(&serde_json::json!({
        "type": "text",
        "file": {
            "filePath": path,
            "content": content,
            "numLines": lines,
            "startLine": 1,
            "totalLines": lines,
        }
    }))
    .expect("envelope")
}

/// Every message index at which consecutive requests over `history` diverge
/// before the end of the earlier request — i.e. every prefix REWRITE.
fn rewrite_points(history: &[ConversationMessage]) -> Vec<usize> {
    let boundaries: Vec<usize> = history
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == MessageRole::Assistant)
        .map(|(index, _)| index)
        .collect();
    let mut rewrites = Vec::new();
    let mut previous: Option<Request> = None;
    for &boundary in &boundaries {
        let request = Request::build(&history[..boundary]);
        if let Some(previous) = previous.as_ref() {
            let divergence = divergence_index(previous, &request);
            if divergence < previous.messages.len() {
                rewrites.push(divergence);
            }
        }
        previous = Some(request);
    }
    rewrites
}

/// Ordinary agentic traffic must be APPEND-ONLY on the wire: each request is
/// the previous one plus new messages, never a rewrite of an earlier one.
///
/// This is the single property the prompt cache rests on. A rewrite at message
/// `i` re-bills every token from `i` onward at full price on every subsequent
/// request, so one moved byte early in a long transcript costs more than every
/// compression win downstream of it combined.
#[test]
fn consecutive_requests_are_append_only_for_ordinary_traffic() {
    let history = synthetic_session(12, |round| {
        read_file_output(&format!("src/mod_{round}.rs"), 6)
    });
    assert!(
        history.len() > 30,
        "premise: a transcript long enough for a prefix to exist"
    );

    assert_eq!(
        rewrite_points(&history),
        Vec::<usize>::new(),
        "a request must be its predecessor plus new messages — nothing earlier may change"
    );
}

/// The ONE documented exception, pinned with the trade it represents.
///
/// A `read_file` whose lossless view exceeds the outline threshold is sent in
/// full as the newest result — the model asked for that file this turn, so
/// handing back an outline would answer a question it did not ask — and is
/// elided once history moves past it. That flip rewrites the prefix exactly
/// once per oversized read.
///
/// Measured on 1,148 real sessions: 26 occurrences in 13,388 requests (0.19%),
/// 0.124% of all tool results, stranding 0.029% of input-side tokens. The
/// alternative — never sending the newest result losslessly — trades a
/// capability the model depends on for three hundredths of a percent, so the
/// flip is kept deliberately. This test exists so that a change to the
/// threshold, the outline policy, or the freshness window surfaces here as a
/// changed count rather than as a silent shift in that trade.
#[test]
fn an_oversized_read_rewrites_the_prefix_exactly_once_and_nothing_else_does() {
    // Round 3's read is far past OUTLINE_THRESHOLD_CHARS (30k); the rest are small.
    let history = synthetic_session(8, |round| {
        let functions = if round == 3 { 400 } else { 3 };
        read_file_output(&format!("src/mod_{round}.rs"), functions)
    });

    let rewrites = rewrite_points(&history);
    assert_eq!(
        rewrites.len(),
        1,
        "exactly one flip, from the one oversized read: {rewrites:?}"
    );

    // And it is that read's own message that changed, not something downstream.
    let flip = rewrites[0];
    let request = Request::build(&history);
    let flipped = serde_json::to_string(&request.messages[flip]).expect("json");
    assert!(
        flipped.contains("src/mod_3.rs"),
        "the rewrite must be the oversized read itself, got: {}",
        &flipped[..200.min(flipped.len())]
    );

    // Same transcript with that read small: no rewrite at all. This is what
    // proves the flip is the cause rather than a coincidence of length.
    let small = synthetic_session(8, |round| {
        read_file_output(&format!("src/mod_{round}.rs"), 3)
    });
    assert!(rewrite_points(&small).is_empty());
}
