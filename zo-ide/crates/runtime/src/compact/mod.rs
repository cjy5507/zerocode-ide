use crate::session::{AnchorSummary, ContentBlock, ConversationMessage, MessageRole, Session};

const COMPACT_CONTINUATION_PREAMBLE: &str = "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.\n\n";
const COMPACT_RECENT_MESSAGES_NOTE: &str = "Recent messages are preserved verbatim.";
const COMPACT_DIRECT_RESUME_INSTRUCTION: &str = "Continue the conversation from where it left off without asking the user any further questions. Resume directly — do not acknowledge the summary, do not recap what was happening, and do not preface with continuation text.";

/// System prompt sent to the API when requesting an LLM-generated compaction summary.
///
/// The model receives the conversation messages to be compacted along with this prompt,
/// and should respond with `<analysis>…</analysis>` and `<summary>…</summary>` blocks.
pub const COMPACTION_SYSTEM_PROMPT: &str = "\
You are summarizing a coding conversation that is about to run out of context window. Produce a detailed but information-dense summary that lets the assistant resume the work seamlessly, losing no critical technical detail.

Rules:
1. Read the entire conversation and capture every fact needed to continue.
2. Preserve exact identifiers verbatim: file paths, function/type names, error messages, command lines, and key tool outputs.
3. Be specific, not vague — prefer concrete names and values over descriptions.
4. Do NOT include pleasantries, meta-commentary, or filler.

Respond in this exact format:

<analysis>
Briefly walk through the conversation chronologically and note what matters for each of the summary sections below. This is your scratchpad.
</analysis>

<summary>
1. Primary Request and Intent: [the user's explicit asks and overall goal, in detail]
2. Key Technical Concepts: [technologies, libraries, patterns, and conventions in play]
3. Files and Code Sections: [each file viewed or changed, with why it matters and the specific functions/edits — include important code snippets where they aid resumption]
4. Errors and Fixes: [errors encountered, how they were resolved, and any user feedback]
5. Problem Solving: [problems solved and ongoing troubleshooting, with reasoning]
6. All User Messages: [list the user's non-tool messages so intent and corrections are not lost]
7. Pending Tasks: [explicitly requested work that is not yet done]
8. Current Work: [precisely what was being done right before this summary, with file names and code so it can be picked up immediately] and Next Step: [the single most logical next action, only if it directly continues the current work]
</summary>";

/// Build the compaction-summary system prompt, optionally augmented with the
/// user's `/compact <focus>` directive (Claude Code "Compact Instructions"
/// parity).
///
/// Bare compaction (`focus == None`, or an all-whitespace focus) returns
/// [`COMPACTION_SYSTEM_PROMPT`] byte-for-byte, so the no-focus path — and the
/// prompt-cache blocks that ride the system prompt — are unchanged. When a
/// focus is present it is appended as an explicit, highest-priority
/// preservation instruction so the model steers the *retained* detail toward
/// what the user asked to keep. This is what makes `/compact <focus>` raise
/// summary quality on the focus area, instead of the old behavior where routing
/// to the deterministic extractor made the summary *worse* the more specific
/// the request got.
#[must_use]
pub fn compaction_system_prompt(focus: Option<&str>) -> String {
    match focus.map(str::trim).filter(|focus| !focus.is_empty()) {
        Some(focus) => format!(
            "{COMPACTION_SYSTEM_PROMPT}\n\nCompact Instructions: the user asked to focus this summary on \"{focus}\". Preserve every detail relevant to {focus} above all else — when you must condense, condense unrelated material first and keep information about {focus} verbatim. Still produce all eight numbered sections in the exact format above."
        ),
        None => COMPACTION_SYSTEM_PROMPT.to_string(),
    }
}

/// Thresholds controlling when and how a session is compacted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionConfig {
    pub preserve_recent_messages: usize,
    pub max_estimated_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            preserve_recent_messages: 4,
            max_estimated_tokens: 10_000,
        }
    }
}

/// Result of compacting a session into a summary plus preserved tail messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionResult {
    pub summary: String,
    pub formatted_summary: String,
    pub compacted_session: Session,
    pub removed_message_count: usize,
}

/// Prepared compaction boundaries — the messages to remove, the tail to preserve,
/// and any existing summary from a prior compaction round.
#[derive(Debug, Clone)]
pub struct CompactionPlan {
    /// The live messages that sit BEFORE `messages_to_compact` and are not
    /// being summarized — in practice the prior round's continuation message,
    /// or empty on the first round.
    ///
    /// It exists for the cached-prefix summary request, whose entire value is
    /// being byte-identical to the live conversation's opening so the provider
    /// serves it from cache. `messages_to_compact` starts one message late
    /// from round two onward, and a request that skips the live index 0 is not
    /// a prefix of anything — it diverges at the very first message and the
    /// whole history bills as fresh input.
    pub cache_prefix: Vec<ConversationMessage>,
    /// Messages that will be summarized and removed.
    pub messages_to_compact: Vec<ConversationMessage>,
    /// Messages preserved verbatim at the end of the session.
    pub preserved_tail: Vec<ConversationMessage>,
    /// Typed anchor from a prior compaction round, if one exists — recovered
    /// from `session.compaction.anchor` (or parsed from a legacy continuation
    /// message on the first post-upgrade round). The next summary folds into it.
    pub existing_anchor: Option<AnchorSummary>,
    /// Lightweight clone of the original session with messages cleared,
    /// carrying only the metadata (id, timestamps, compaction state, fork).
    /// Avoids cloning the entire message history (P0 L2).
    session_shell: Session,
}

/// Trait for generating a summary of messages being compacted.
///
/// Implementations can produce summaries locally (deterministic extraction)
/// or by calling an LLM API (higher quality, matches upstream TS behavior).
pub trait CompactionSummarizer {
    /// Generate a summary of the given messages.
    ///
    /// The returned string should ideally contain `<summary>…</summary>` tags.
    /// An `<analysis>…</analysis>` block is optional and will be stripped during formatting.
    fn summarize(&self, messages: &[ConversationMessage]) -> String;
}

/// Local deterministic summarizer — extracts structure from messages without API calls.
///
/// This is the default summarizer and matches the behavior prior to the trait introduction.
#[derive(Debug, Default)]
pub struct LocalSummarizer;

impl CompactionSummarizer for LocalSummarizer {
    fn summarize(&self, messages: &[ConversationMessage]) -> String {
        summarize_messages(messages)
    }
}

/// Summarizer that injects a user focus directive (from `/compact <focus>`) as
/// the first line inside the `<summary>` block of the deterministic summary, so
/// the resumed assistant prioritizes it. Extraction stays deterministic; the
/// focus is carried as an explicit header rather than steering the extraction.
#[derive(Debug)]
pub struct FocusSummarizer {
    /// The user's focus directive (already trimmed, non-empty).
    pub focus: String,
}

impl CompactionSummarizer for FocusSummarizer {
    fn summarize(&self, messages: &[ConversationMessage]) -> String {
        let base = LocalSummarizer.summarize(messages);
        let focus_line = format!("Focus (user /compact directive): {}", self.focus);
        match base.find("<summary>") {
            Some(idx) => {
                let cut = idx + "<summary>".len();
                format!("{}\n{focus_line}{}", &base[..cut], &base[cut..])
            }
            None => format!("{focus_line}\n{base}"),
        }
    }
}

/// Roughly estimates the token footprint of the current session transcript.
#[must_use]
pub fn estimate_session_tokens(session: &Session) -> usize {
    session.messages.iter().map(estimate_message_tokens).sum()
}

/// At least this many messages must remain summarizable after the preserved
/// tail is carved off, or a token-budgeted tail could swallow the whole
/// transcript and turn the round into a no-op that immediately re-triggers.
const MIN_COMPACTABLE_MESSAGES: usize = 8;

/// Number of trailing messages whose estimated tokens fit `budget_tokens` —
/// the CC-parity token-budgeted preserved tail. Capped so at least
/// [`MIN_COMPACTABLE_MESSAGES`] remain to summarize, then floored at the
/// legacy 4-message default ([`CompactionConfig::default`]): the floor wins
/// on a session too small for both bounds, which keeps small-session
/// behavior byte-identical to the fixed-tail era (the budget only ever
/// GROWS the tail on sessions big enough to afford it).
#[must_use]
pub fn preserved_tail_len_for_budget(
    messages: &[ConversationMessage],
    budget_tokens: u64,
) -> usize {
    let mut total: u64 = 0;
    let mut count = 0usize;
    for message in messages.iter().rev() {
        total = total.saturating_add(estimate_message_tokens(message) as u64);
        if total > budget_tokens {
            break;
        }
        count += 1;
    }
    let cap = messages.len().saturating_sub(MIN_COMPACTABLE_MESSAGES);
    count
        .min(cap)
        .max(CompactionConfig::default().preserve_recent_messages)
}

/// Per-tool-result cap (chars) on the copy of the transcript sent to the
/// summarizer. The 8-section summary needs the structural facts a result
/// carries, not a 40k-char build log verbatim; oversized bodies keep head and
/// tail (`elide_middle`) so identifiers at either end survive.
const SUMMARY_INPUT_TOOL_RESULT_MAX_CHARS: usize = 6_000;

/// P3: pre-trim the SUMMARY REQUEST copy of the messages (never the session):
/// oversized tool-result bodies are middle-elided and image blocks are
/// replaced with a placeholder — both pure input-cost reduction for the
/// summary round-trip. Everything else passes through byte-identical.
#[must_use]
pub fn pretrim_messages_for_summary(messages: &[ConversationMessage]) -> Vec<ConversationMessage> {
    messages
        .iter()
        .map(|message| {
            let mut message = message.clone();
            let needs_trim = message.blocks.iter().any(|block| match block {
                ContentBlock::ToolResult { output, .. } => {
                    output.chars().count() > SUMMARY_INPUT_TOOL_RESULT_MAX_CHARS
                }
                ContentBlock::Image { .. } => true,
                _ => false,
            });
            if !needs_trim {
                return message;
            }
            message.blocks = message
                .blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::ToolResult { output, .. }
                        if output.chars().count() > SUMMARY_INPUT_TOOL_RESULT_MAX_CHARS =>
                    {
                        let mut trimmed = block.clone();
                        if let ContentBlock::ToolResult { output, .. } = &mut trimmed {
                            *output = core_types::text::elide_middle(
                                output,
                                SUMMARY_INPUT_TOOL_RESULT_MAX_CHARS,
                            );
                        }
                        trimmed
                    }
                    ContentBlock::Image { .. } => ContentBlock::Text {
                        text: "[image omitted from summary input]".to_string(),
                    },
                    other => other.clone(),
                })
                .collect();
            message
        })
        .collect()
}

/// Returns `true` when the session exceeds the configured compaction budget.
#[must_use]
pub fn should_compact(session: &Session, config: CompactionConfig) -> bool {
    let start = compacted_summary_prefix_len(session);
    let compactable = &session.messages[start..];

    compactable.len() > config.preserve_recent_messages
        && compactable
            .iter()
            .map(estimate_message_tokens)
            .sum::<usize>()
            >= config.max_estimated_tokens
}

/// Normalizes a compaction summary into user-facing continuation text.
#[must_use]
pub fn format_compact_summary(summary: &str) -> String {
    let without_analysis = strip_tag_block(summary, "analysis");
    let formatted = if let Some(content) = extract_tag_block(&without_analysis, "summary") {
        without_analysis.replace(
            &format!("<summary>{content}</summary>"),
            &format!("Summary:\n{}", content.trim()),
        )
    } else {
        without_analysis
    };

    collapse_blank_lines(&formatted).trim().to_string()
}

/// Builds the synthetic system message used after session compaction.
///
/// `vault_ranges` are the Raw Vault seq spans this session has sealed (LAVA P1).
/// When non-empty, a single recall-affordance line is appended AFTER the summary
/// body and the resume/notes so the model knows the exact pre-compaction
/// originals are recoverable and how to address them — see
/// `format_vault_recall_affordance`. Pass `&[]` for an unsealed session (or a
/// caller that does not surface recovery).
#[must_use]
pub fn get_compact_continuation_message(
    summary: &str,
    suppress_follow_up_questions: bool,
    recent_messages_preserved: bool,
    vault_ranges: &[(u32, u32)],
) -> String {
    let mut base = format!(
        "{COMPACT_CONTINUATION_PREAMBLE}{}",
        format_compact_summary(summary)
    );

    if recent_messages_preserved {
        base.push_str("\n\n");
        base.push_str(COMPACT_RECENT_MESSAGES_NOTE);
    }

    if suppress_follow_up_questions {
        base.push('\n');
        base.push_str(COMPACT_DIRECT_RESUME_INSTRUCTION);
    }

    // The recall affordance rides OUTSIDE the summary body (after the
    // resume/notes) so the legacy prose reparse in `parse_anchor_from_summary`,
    // which only inspects the extracted `<summary>`/continuation body, never
    // folds it into the anchor.
    if let Some(affordance) = format_vault_recall_affordance(vault_ranges) {
        base.push_str("\n\n");
        base.push_str(&affordance);
    }

    base
}

/// Render the model-facing Raw Vault recall affordance appended to the
/// continuation message: one line naming the exact `session_recall` seq spans
/// the raw pre-compaction originals live at, so the model can pull back an exact
/// detail instead of trusting the lossy summary. Returns `None` when nothing was
/// sealed (no ranges), so the line appears only when there is a vault to point
/// at. The parameter names match the `session_recall` tool spec (`seq_from` /
/// `seq_to` / `include_tool_results`) so the model reads a directly-usable call.
fn format_vault_recall_affordance(vault_ranges: &[(u32, u32)]) -> Option<String> {
    if vault_ranges.is_empty() {
        return None;
    }
    let spans = vault_ranges
        .iter()
        .map(|(lo, hi)| format!("{lo}-{hi}"))
        .collect::<Vec<_>>()
        .join(", ");
    let seq_from = vault_ranges.iter().map(|(lo, _)| *lo).min().unwrap_or(0);
    let seq_to = vault_ranges.iter().map(|(_, hi)| *hi).max().unwrap_or(0);
    Some(format!(
        "Raw originals of every compacted message are preserved in this session's vault (seq {spans}). \
         Retrieve exact pre-compaction originals with the session_recall tool: \
         {{\"session_ref\": \"current\", \"seq_from\": {seq_from}, \"seq_to\": {seq_to}}} \
         (optional: query, role, include_tool_results)."
    ))
}

/// Prepares a compaction plan without generating the summary yet.
///
/// Returns `None` if the session does not need compaction.
#[must_use]
pub fn prepare_compaction(session: &Session, config: CompactionConfig) -> Option<CompactionPlan> {
    if !should_compact(session, config) {
        return None;
    }

    // Detect (structurally) whether index 0 is a prior continuation message,
    // and recover the prior round's state as a TYPED anchor — from the stored
    // `compaction.anchor` (P1 path), or, for a session compacted before P1,
    // by parsing the legacy continuation prose (upgrades it to typed on this
    // round). The structural detection (not the anchor) drives the cut point,
    // so the boundary logic is unchanged from before P1.
    let existing_continuation = session
        .messages
        .first()
        .and_then(extract_existing_compacted_summary);
    let compacted_prefix_len = usize::from(existing_continuation.is_some());
    let existing_anchor = session
        .compaction
        .as_ref()
        .and_then(|compaction| compaction.anchor.clone())
        // A `Some` but entirely-empty anchor (e.g. a `"anchor": null` from
        // corruption/migration) is indistinguishable from real state here, so
        // treat it as absent and fall back to recovering from the rendered
        // continuation rather than silently dropping prior context.
        .filter(|anchor| !anchor.is_empty())
        .or_else(|| {
            existing_continuation
                .as_deref()
                .map(parse_anchor_from_summary)
        });
    let mut keep_from = session
        .messages
        .len()
        .saturating_sub(config.preserve_recent_messages);

    // Never let the preserved tail begin with an orphan `tool_result`: the Anthropic
    // API rejects such payloads with `unexpected tool_use_id in tool_result blocks`
    // because the matching assistant `tool_use` would be lost in the summary. Walk
    // the cut point backward past any user message carrying a tool_result so the
    // preceding assistant `tool_use` travels with it into the tail.
    while keep_from > compacted_prefix_len
        && session
            .messages
            .get(keep_from)
            .is_some_and(message_has_tool_result)
    {
        keep_from -= 1;
    }

    let mut session_shell = session.clone();
    session_shell.messages = std::sync::Arc::new(Vec::new());

    Some(CompactionPlan {
        cache_prefix: session.messages[..compacted_prefix_len].to_vec(),
        messages_to_compact: session.messages[compacted_prefix_len..keep_from].to_vec(),
        preserved_tail: session.messages[keep_from..].to_vec(),
        existing_anchor,
        session_shell,
    })
}

/// Returns `true` if `message` contains any `ToolResult` content block.
///
/// Used by [`prepare_compaction`] to keep `tool_use`/`tool_result` pairs together
/// across the compaction cut point — otherwise the preserved tail can begin
/// with an orphaned `tool_result` that the Anthropic API rejects.
fn message_has_tool_result(message: &ConversationMessage) -> bool {
    message
        .blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
}

/// Applies a compaction plan using the given raw summary text.
///
/// The `raw_summary` is the output from a [`CompactionSummarizer`] — either a local
/// extraction or an LLM-generated summary with `<analysis>` and `<summary>` tags.
/// Applies a compaction plan by value, consuming the plan to avoid
/// redundant clones of `preserved_tail` and `original_session`.
#[must_use]
pub fn apply_compaction(plan: CompactionPlan, raw_summary: &str) -> CompactionResult {
    let CompactionPlan {
        // Only the summary REQUEST needs the prefix CONTENT; the live message it
        // names is already in the session and is replaced by this round's
        // continuation, so it is dropped below rather than held. Its LENGTH is
        // load-bearing here and cannot be: it is the offset between an index into
        // `messages_to_compact` and the live index microcompact stamped its
        // pre-clear seal from.
        cache_prefix,
        mut messages_to_compact,
        preserved_tail,
        existing_anchor,
        session_shell,
    } = plan;
    let compacted_prefix_len = cache_prefix.len();
    drop(cache_prefix);
    let removed_message_count = messages_to_compact.len();
    // Fold this round's delta into the prior typed anchor (verbatim carry, no
    // re-truncation), then RENDER the model-facing summary from the folded
    // anchor — the single source of truth. This replaces the prose
    // merge-of-summaries that eroded identifiers over many rounds.
    let delta_anchor = parse_anchor_from_summary(raw_summary);
    let mut folded_anchor = fold_anchor(existing_anchor.as_ref(), delta_anchor);

    // The session shell carries the persistence path and the pre-compaction
    // `first_message_index`, so it is both the seal target and the source the
    // microcompact-restore reads its raw originals from.
    let mut compacted_session = session_shell;

    // LAVA lossless: before sealing, swap any microcompact-cleared tool-result
    // body — and any microcompact-cleared standalone image — back to its raw
    // original, from the vault the clear itself sealed it into (falling back, for
    // tool results, to the on-disk transcript), so the vault seals the pre-clear
    // original rather than the placeholder. This also protects the seal below
    // from shadowing a good record: `read_vault` dedups last-wins, so re-sealing
    // this same seq with a placeholder buries the original the clear had stored.
    // Best-effort: a missing/corrupt/unmatched source degrades to sealing the
    // in-memory (placeholder) state, never failing the compaction.
    //
    // The image half is addressed POSITIONALLY (it has no id), so the seqs are
    // computed here, on the same axis microcompact stamped: a message at index
    // `i` of `messages_to_compact` sits at live index `compacted_prefix_len + i`,
    // and `first_message_index` has not been rebased yet (`record_compaction`
    // runs below, inside `apply_compaction_atomic`).
    let restore_base_seq = compacted_session
        .first_message_index()
        .saturating_add(u32::try_from(compacted_prefix_len).unwrap_or(u32::MAX));
    let restore_seqs: Vec<u32> = (0..messages_to_compact.len())
        .map(|offset| restore_base_seq.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX)))
        .collect();
    restore_microcompacted_bodies(&mut messages_to_compact, &compacted_session, &restore_seqs);

    // Seal the raw evicted messages to the append-only vault BEFORE
    // `record_compaction` overwrites the transcript with the lossy summary, so
    // the originals survive this and every later compaction round losslessly.
    // The returned span records which vault seqs this round owns; append it to
    // the anchor (the fold carries prior rounds' spans forward verbatim, so the
    // new span is added exactly once, here) so the continuation can advertise
    // the exact recall range.
    if let Some(span) = compacted_session.seal_evicted_to_vault(&messages_to_compact) {
        folded_anchor.vault_ranges.push(span);
    }

    // Keep the running summary from growing to dominate the window: the fold's
    // unbounded union is trimmed to its most recent content per section now that
    // the raw originals are sealed to the vault (so any trimmed detail remains
    // recoverable via `session_recall`).
    bound_anchor(&mut folded_anchor);

    let summary = render_anchor_to_summary_text(&folded_anchor);
    let formatted_summary = format_compact_summary(&summary);
    let continuation = get_compact_continuation_message(
        &summary,
        true,
        !preserved_tail.is_empty(),
        &folded_anchor.vault_ranges,
    );

    let mut compacted_messages = vec![ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text { text: continuation }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
            model: None,
    }];
    compacted_messages.extend(preserved_tail);

    // Atomic seam: hand the compacted transcript to the session so it captures
    // the pre-compaction snapshot BEFORE replacing `messages`. On a persistence
    // Conflict this rolls the whole mutation (messages + metadata) back to the
    // original state instead of leaving memory holding a compacted view that
    // diverges from a peer's newer file.
    compacted_session.apply_compaction_atomic(
        std::sync::Arc::new(compacted_messages),
        summary.clone(),
        removed_message_count,
        Some(folded_anchor),
    );

    CompactionResult {
        summary,
        formatted_summary,
        compacted_session,
        removed_message_count,
    }
}

/// A tool result's raw `(output, images)`, BORROWED from the source that holds
/// it. Owning these instead would duplicate the recovery source in RSS — the
/// vault of a long session is tens of MB and every non-placeholder body in it
/// would be cloned while the materialized vault is still alive — so the two
/// sources are indexed in sequence, each inside its own scope, rather than
/// merged into one owned map.
type RawToolResultBody<'a> = (&'a String, &'a Vec<(String, String)>);

/// Whether any block in `messages` is a microcompact placeholder — a cleared
/// tool-result body or a cleared standalone image — i.e. what
/// [`seal_microcompact_originals`] checks before paying for the heal.
/// [`restore_microcompacted_bodies`] needs the positions/ids themselves, so it
/// re-derives them; this only answers "is there anything to heal at all".
fn needs_microcompact_heal(messages: &[ConversationMessage]) -> bool {
    messages.iter().any(|message| {
        message.blocks.iter().any(|block| match block {
            ContentBlock::ToolResult { output, .. } => output == MICROCOMPACT_PLACEHOLDER,
            ContentBlock::Text { text } => text == MICROCOMPACT_IMAGE_PLACEHOLDER,
            _ => false,
        })
    })
}

/// Whether `sealed` is the pre-clear ORIGINAL of `live` — the same message, with
/// the blocks microcompact blanks still intact.
///
/// This is the verification a POSITIONAL heal needs and a content-addressed one
/// does not. A standalone pasted image carries no `tool_use_id`, so the only
/// address a sealed image has is its `vault_seq`, and that axis is off by one for
/// any record written before the session's first full compaction (see the AXIS
/// note on [`Session::seal_originals_to_vault`]). Grafting the neighbouring
/// turn's image into this turn is strictly worse than leaving the placeholder, so
/// the address is treated as a CANDIDATE and this predicate decides.
///
/// Structural, not fuzzy: microcompact rewrites blocks IN PLACE, so a blanked
/// message differs from its original only where a clear ran — same role, same
/// block count, same order. The two blanking rules are the only tolerated
/// differences:
///
/// * an `Image` became the [`MICROCOMPACT_IMAGE_PLACEHOLDER`] text block, and
/// * a `ToolResult` body became [`MICROCOMPACT_PLACEHOLDER`] (its
///   `tool_use_id` must still match, and its out-of-band images are ignored
///   because the clear drops them).
///
/// A superseded REMINDER clear is deliberately NOT tolerated: a message that lost
/// both a reminder and an image would fail here and keep its placeholder (the
/// accepted loss), which needs a `System` message carrying an `Image` — no
/// producer in this codebase makes one, and admitting the case would let any text
/// block match any other, which is exactly the discrimination this predicate
/// exists to provide.
#[must_use]
pub fn is_pre_clear_original_of(sealed: &ConversationMessage, live: &ConversationMessage) -> bool {
    sealed.role == live.role
        && sealed.blocks.len() == live.blocks.len()
        && sealed
            .blocks
            .iter()
            .zip(live.blocks.iter())
            .all(|(sealed_block, live_block)| is_pre_clear_block(sealed_block, live_block))
}

/// One block pair under the [`is_pre_clear_original_of`] rules.
fn is_pre_clear_block(sealed: &ContentBlock, live: &ContentBlock) -> bool {
    match (sealed, live) {
        (ContentBlock::Image { .. }, ContentBlock::Text { text }) => {
            text == MICROCOMPACT_IMAGE_PLACEHOLDER
        }
        (
            ContentBlock::ToolResult {
                tool_use_id: sealed_id,
                ..
            },
            ContentBlock::ToolResult {
                tool_use_id: live_id,
                output,
                ..
            },
        ) if output == MICROCOMPACT_PLACEHOLDER => sealed_id == live_id,
        _ => sealed == live,
    }
}

/// Indices of the messages in `messages` that carry a cleared standalone image.
fn cleared_image_indices(messages: &[ConversationMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.blocks.iter().any(|block| {
                matches!(block, ContentBlock::Text { text } if text == MICROCOMPACT_IMAGE_PLACEHOLDER)
            })
        })
        .map(|(index, _)| index)
        .collect()
}

/// Swap every microcompact-cleared standalone image in `messages` back to the
/// `Image` block the pre-clear seal stored, addressing the seal by `seqs[i]` (the
/// absolute vault seq of `messages[i]`) and verifying the hit before using it.
///
/// The seq is the only address a pasted image has, and it is not reliable on its
/// own: a record sealed before the session's FIRST full compaction is one behind
/// the live axis forever after (the AXIS note on
/// [`Session::seal_originals_to_vault`]), so a bare `seqs[i]` lookup can land on
/// the NEXT turn's record. Two gates make a wrong graft unreachable and turn the
/// failure mode into "still blanked", which is the pre-existing behavior:
///
/// 1. [`is_pre_clear_original_of`] must accept the record for THIS message, and
/// 2. the record must not equally accept the message one seq later — the only
///    other turn the +1 drift could hand it to. Two adjacent turns that are
///    byte-identical once blanked (e.g. two bare pasted images in a row) are
///    therefore both left blanked rather than guessed at.
///
/// Gate 2 is fully armed only where `seqs` is contiguous, which is the call site
/// that matters: `apply_compaction`, whose seal is the permanent one. From
/// [`seal_microcompact_originals`] the batch is a scattered selection, so when the
/// `seq + 1` neighbour is not in the batch the check is vacuous there.
fn heal_cleared_images(
    messages: &mut [ConversationMessage],
    seqs: &[u32],
    blanked: &[usize],
    vault: &[crate::session::VaultRecord],
) {
    // Index only the seqs actually wanted, and only records that hold an image
    // at all — a whole-vault index would be proportional to session history to
    // serve the handful of turns being healed.
    let wanted: std::collections::HashSet<u32> =
        blanked.iter().filter_map(|&index| seqs.get(index).copied()).collect();
    let sealed: std::collections::HashMap<u32, &ConversationMessage> = vault
        .iter()
        .filter(|record| {
            wanted.contains(&record.vault_seq)
                && record
                    .message
                    .blocks
                    .iter()
                    .any(|block| matches!(block, ContentBlock::Image { .. }))
        })
        .map(|record| (record.vault_seq, &record.message))
        .collect();
    if sealed.is_empty() {
        return;
    }
    for &index in blanked {
        let (Some(&seq), Some(live)) = (seqs.get(index), messages.get(index)) else {
            continue;
        };
        let Some(&original) = sealed.get(&seq) else {
            continue;
        };
        // Gate 1: the record must be the pre-clear shape of THIS message —
        // same role, same block count, every surviving block byte-identical,
        // every blanked block accounted for. A neighbour's record differs in
        // its own surviving text and is refused here.
        if !is_pre_clear_original_of(original, live) {
            continue;
        }
        // Gate 2: nor may the record equally claim the message one seq later,
        // the only other owner the +1 drift could hand it to.
        let next_owner = seqs
            .iter()
            .position(|&other| other == seq.saturating_add(1))
            .and_then(|position| messages.get(position));
        if next_owner.is_some_and(|next| is_pre_clear_original_of(original, next)) {
            continue;
        }
        // Restore only the blocks that were actually blanked, so anything the
        // tool-result pass already healed into this message survives.
        let restored: Vec<(usize, ContentBlock)> = live
            .blocks
            .iter()
            .enumerate()
            .filter(|(block_index, block)| {
                matches!(block, ContentBlock::Text { text } if text == MICROCOMPACT_IMAGE_PLACEHOLDER)
                    && matches!(original.blocks.get(*block_index), Some(ContentBlock::Image { .. }))
            })
            .map(|(block_index, _)| (block_index, original.blocks[block_index].clone()))
            .collect();
        for (block_index, block) in restored {
            messages[index].blocks[block_index] = block;
        }
    }
}

/// Index the non-placeholder tool-result bodies in `messages` that some caller
/// actually WANTS, keyed by `tool_use_id`.
///
/// `wanted` is the set of ids currently holding the placeholder. Filtering on it
/// keeps this map proportional to what is actually being restored: a recovery
/// source is the WHOLE vault (or the whole transcript), so indexing unfiltered
/// builds an entry per tool result in the session's history to serve the handful
/// about to be swapped.
///
/// Within one source, first writer wins: a duplicated id (should not happen —
/// ids are session-unique) keeps the earliest. Priority BETWEEN sources is not
/// expressed here but by call order — a body already swapped in is no longer a
/// placeholder, so a later source cannot overwrite it.
fn index_raw_tool_bodies<'a>(
    messages: impl IntoIterator<Item = &'a ConversationMessage>,
    wanted: &std::collections::HashSet<String>,
    originals: &mut std::collections::HashMap<&'a str, RawToolResultBody<'a>>,
) {
    for message in messages {
        for block in &message.blocks {
            if let ContentBlock::ToolResult {
                tool_use_id,
                output,
                images,
                ..
            } = block
            {
                if output != MICROCOMPACT_PLACEHOLDER && wanted.contains(tool_use_id) {
                    originals
                        .entry(tool_use_id.as_str())
                        .or_insert((output, images));
                }
            }
        }
    }
}

/// The `tool_use_id`s in `messages` whose tool-result body is the microcompact
/// placeholder — exactly the ids a recovery source is worth reading for, and
/// therefore also the guard for whether to read one at all (an empty set means
/// nothing was cleared).
///
/// Owned rather than borrowed because the caller needs it alive across the
/// mutable borrow that performs the swap.
fn cleared_tool_use_ids(messages: &[ConversationMessage]) -> std::collections::HashSet<String> {
    let mut wanted = std::collections::HashSet::new();
    for message in messages {
        for block in &message.blocks {
            if let ContentBlock::ToolResult {
                tool_use_id,
                output,
                ..
            } = block
            {
                if output == MICROCOMPACT_PLACEHOLDER {
                    wanted.insert(tool_use_id.clone());
                }
            }
        }
    }
    wanted
}

/// Replace every microcompact placeholder body in `messages` whose `tool_use_id`
/// is present in `originals`. Returns `true` when no placeholder was left
/// unresolved, which is what lets the caller skip a second recovery source.
fn swap_placeholders_from(
    messages: &mut [ConversationMessage],
    originals: &std::collections::HashMap<&str, RawToolResultBody<'_>>,
) -> bool {
    let mut resolved_all = true;
    for message in messages.iter_mut() {
        for block in &mut message.blocks {
            let ContentBlock::ToolResult {
                tool_use_id,
                output,
                images,
                ..
            } = block
            else {
                continue;
            };
            if output != MICROCOMPACT_PLACEHOLDER {
                continue;
            }
            if let Some((original_output, original_images)) = originals.get(tool_use_id.as_str()) {
                output.clone_from(original_output);
                images.clone_from(original_images);
            } else {
                resolved_all = false;
            }
        }
    }
    resolved_all
}

/// Swap microcompact-cleared tool-result bodies in `messages` back to their raw
/// originals — the step that makes the vault seal truly lossless even for a body
/// a cheaper tier already trimmed out of the live context.
///
/// PRIMARY SOURCE: the Raw Vault. `microcompact_session` seals each pre-clear
/// original before it overwrites the body, so the original is in the vault by
/// construction, for the life of the session, immune to every later rewrite.
///
/// FALLBACK SOURCE: the persisted transcript. It is a fallback and not the
/// primary because microcompact's clear IS persisted — the pass ends with
/// `mark_transcript_dirty`, which forces the next persist onto the full-snapshot
/// path, so the disk copy holds the placeholder from roughly one turn after the
/// clear onward. Measured across real sessions, only 3.7% of vault-placeholder
/// `tool_use_id`s still had an intact original anywhere on disk, which is why
/// this path alone could never make the seal lossless. It is retained rather
/// than deleted because it is the only source that covers two real cases the
/// vault cannot: a clear that happened before the pre-clear seal shipped (an
/// older session resumed on a newer binary), and a run with
/// `ZO_DISABLE_RAW_VAULT` set. It is also skipped entirely whenever the vault
/// already resolved every placeholder, so in the healthy case it costs nothing.
///
/// TOOL RESULTS are matched by `tool_use_id` (a required, session-unique field on
/// every tool result, preserved verbatim through microcompact), so a cleared
/// result is paired with its exact original regardless of position; no positional
/// fallback is needed because both sources are indexed whole.
///
/// STANDALONE IMAGES have no such id, so they are matched positionally, by
/// `seqs[i]` — the absolute vault seq of `messages[i]`, which the caller must
/// stamp on the same axis the pre-clear seal used. Because that axis carries a
/// documented +1 drift, every positional hit is verified with
/// [`is_pre_clear_original_of`] before it is used; see [`heal_cleared_images`]
/// for the two gates and for what they refuse. Images are recovered from the
/// VAULT ONLY: the disk fallback below is keyed by id and has nothing to offer a
/// block that has none.
///
/// Strictly best-effort: no persistence path, an unreadable/corrupt transcript,
/// an empty vault, an id whose original survives in neither source, or a
/// positional hit that fails verification all degrade to leaving the placeholder
/// in place — the pre-vault lossy behavior, never worse, and never a failed
/// compaction.
fn restore_microcompacted_bodies(
    messages: &mut [ConversationMessage],
    session: &Session,
    seqs: &[u32],
) {
    // Only pay for a recovery source when something was actually cleared — the
    // common case (no microcompact this session) reads nothing.
    let wanted = cleared_tool_use_ids(messages);
    let blanked_images = cleared_image_indices(messages);
    if wanted.is_empty() && blanked_images.is_empty() {
        return;
    }
    // Each source lives in its own scope, and its index borrows from it rather
    // than cloning. Both matter because a recovery source is a whole-history
    // read: the largest real vault measured is 52MB, an owned index of its
    // bodies would roughly double that, and merging both sources into one map
    // would hold the disk snapshot alongside the vault. Scoped + borrowed keeps
    // the peak at one source at a time.
    let resolved_all = {
        let vault = session.read_vault();
        let mut originals: std::collections::HashMap<&str, RawToolResultBody<'_>> =
            std::collections::HashMap::new();
        index_raw_tool_bodies(vault.iter().map(|record| &record.message), &wanted, &mut originals);
        let resolved_all = swap_placeholders_from(messages, &originals);
        // Images run AFTER the tool-result swap so a message that lost both is
        // already healed on the id-addressed side when its shape is verified.
        heal_cleared_images(messages, seqs, &blanked_images, &vault);
        resolved_all
    };
    if resolved_all {
        return;
    }
    // The vault left at least one placeholder unresolved. Pay for the disk
    // snapshot only now, and re-run the swap for what is still missing — the
    // bodies the vault already restored are no longer placeholders, so this pass
    // cannot overwrite them with an older copy.
    let Some(persisted) = session.load_persisted_messages() else {
        return;
    };
    let mut originals: std::collections::HashMap<&str, RawToolResultBody<'_>> =
        std::collections::HashMap::new();
    index_raw_tool_bodies(&persisted, &wanted, &mut originals);
    let _ = swap_placeholders_from(messages, &originals);
}

const STATE_DISTILL_MAX_CHARS: usize = 1_600;

/// Build a deterministic, bounded working-state snapshot without removing any
/// transcript messages.
///
/// This is the cheap "state distill" tier used before full compaction: it gives
/// the model a compact anchor for the current goal/files/next work while the
/// original transcript remains intact. It deliberately reuses the same local
/// extraction helpers as deterministic compaction, so this tier cannot
/// fabricate unseen identifiers and has no provider round-trip.
#[must_use]
pub fn distill_session_state(session: &Session) -> Option<String> {
    let messages = session.messages.as_ref();
    if messages.is_empty() {
        return None;
    }

    let mut lines = vec!["# Distilled working state".to_string()];
    if let Some(current_work) = infer_current_work(messages) {
        lines.push(format!("- Current work: {current_work}"));
    }

    let recent_requests = collect_recent_role_summaries(messages, MessageRole::User, 2);
    if !recent_requests.is_empty() {
        lines.push("- Recent user requests:".to_string());
        lines.extend(recent_requests.into_iter().map(|request| format!("  - {request}")));
    }

    let pending_work = infer_pending_work(messages);
    if !pending_work.is_empty() {
        lines.push("- Pending/next signals:".to_string());
        lines.extend(pending_work.into_iter().map(|item| format!("  - {item}")));
    }

    let key_files = collect_key_files(messages);
    if !key_files.is_empty() {
        lines.push(format!("- Key files: {}", key_files.join(", ")));
    }

    if lines.len() == 1 {
        return None;
    }
    Some(truncate_summary(&lines.join("\n"), STATE_DISTILL_MAX_CHARS))
}

/// This is the primary entry point for compaction with a custom summarizer.
#[must_use]
pub fn compact_session_with<S: CompactionSummarizer>(
    session: &Session,
    config: CompactionConfig,
    summarizer: &S,
) -> CompactionResult {
    let Some(plan) = prepare_compaction(session, config) else {
        return CompactionResult {
            summary: String::new(),
            formatted_summary: String::new(),
            compacted_session: session.clone(),
            removed_message_count: 0,
        };
    };
    let raw_summary = summarizer.summarize(&plan.messages_to_compact);
    apply_compaction(plan, &raw_summary)
}

/// Compacts a session by summarizing older messages and preserving the recent tail.
///
/// Uses the [`LocalSummarizer`] for deterministic local summary generation.
#[must_use]
pub fn compact_session(session: &Session, config: CompactionConfig) -> CompactionResult {
    compact_session_with(session, config, &LocalSummarizer)
}

fn compacted_summary_prefix_len(session: &Session) -> usize {
    usize::from(
        session
            .messages
            .first()
            .and_then(extract_existing_compacted_summary)
            .is_some(),
    )
}

fn summarize_messages(messages: &[ConversationMessage]) -> String {
    let user_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .count();
    let assistant_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .count();
    let tool_messages = messages
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .count();

    let mut tool_names = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            ContentBlock::ToolResult { tool_name, .. } => Some(tool_name.as_str()),
            ContentBlock::Text { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::Thinking { .. }
            | ContentBlock::RedactedThinking { .. } => None,
        })
        .collect::<Vec<_>>();
    tool_names.sort_unstable();
    tool_names.dedup();

    let mut lines = vec![
        "<summary>".to_string(),
        "Conversation summary:".to_string(),
        format!(
            "- Scope: {} earlier messages compacted (user={}, assistant={}, tool={}).",
            messages.len(),
            user_messages,
            assistant_messages,
            tool_messages
        ),
    ];

    if !tool_names.is_empty() {
        lines.push(format!("- Tools mentioned: {}.", tool_names.join(", ")));
    }

    let recent_user_requests = collect_recent_role_summaries(messages, MessageRole::User, 3);
    if !recent_user_requests.is_empty() {
        lines.push("- Recent user requests:".to_string());
        lines.extend(
            recent_user_requests
                .into_iter()
                .map(|request| format!("  - {request}")),
        );
    }

    let pending_work = infer_pending_work(messages);
    if !pending_work.is_empty() {
        lines.push("- Pending work:".to_string());
        lines.extend(pending_work.into_iter().map(|item| format!("  - {item}")));
    }

    let key_files = collect_key_files(messages);
    if !key_files.is_empty() {
        lines.push(format!("- Key files referenced: {}.", key_files.join(", ")));
    }

    if let Some(current_work) = infer_current_work(messages) {
        lines.push(format!("- Current work: {current_work}"));
    }

    lines.push("- Key timeline:".to_string());
    for message in messages {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let content = message
            .blocks
            .iter()
            .map(summarize_block)
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!("  - {role}: {content}"));
    }
    lines.push("</summary>".to_string());
    lines.join("\n")
}

// ── LAVA P1: typed anchor (single source of truth) ─────────────────────────
//
// The model-facing continuation message is RENDERED from the typed
// `AnchorSummary`, which folds in only each round's delta and carries prior
// entries forward verbatim. This replaces the old prose `merge_compact_summaries`
// (which re-parsed + re-truncated the prior summary every round, eroding
// identifiers over a long session — the "summary of a summary" drift).

/// Parse an LLM (or local) compaction summary into the typed [`AnchorSummary`].
///
/// The eight numbered sections of [`COMPACTION_SYSTEM_PROMPT`] are recognized by
/// a leading `N. ` marker; list sections become one item per non-empty line
/// (bullet markers stripped). A summary without the numbered structure (the
/// [`LocalSummarizer`] output, or a legacy continuation message being upgraded
/// on resume) is carried whole into `problem_solving`, so nothing is lost.
fn parse_anchor_from_summary(raw_summary: &str) -> AnchorSummary {
    let body = extract_tag_block(raw_summary, "summary")
        .unwrap_or_else(|| format_compact_summary(raw_summary));
    let sections = split_numbered_sections(&body);
    if sections.is_empty() {
        // No recognizable section structure (LocalSummarizer output, or a legacy
        // continuation being upgraded). Carry every line as its own item so the
        // content survives and a later render→parse round-trip is identity (one
        // multiline blob would otherwise re-split into many items).
        return AnchorSummary {
            problem_solving: section_items(&body),
            ..AnchorSummary::default()
        };
    }
    let section = |n: usize| {
        sections
            .iter()
            .find(|(idx, _)| *idx == n)
            .map(|(_, text)| text.as_str())
            .unwrap_or_default()
    };
    AnchorSummary {
        intent: section(1).trim().to_string(),
        concepts: section_items(section(2)),
        files: section_items(section(3)),
        errors_and_fixes: section_items(section(4)),
        problem_solving: section_items(section(5)),
        user_messages: section_items(section(6)),
        pending_tasks: section_items(section(7)),
        current_work: section(8).trim().to_string(),
        // A summary carries no vault spans; `apply_compaction` appends the round's
        // span to the folded anchor after the seal.
        vault_ranges: Vec::new(),
    }
}

/// Split a summary body into `(section_number, content)` pairs, stripping the
/// `N. Label:` prefix from each section's first line. Content after a marker
/// accumulates until the next `N. ` marker.
fn split_numbered_sections(body: &str) -> Vec<(usize, String)> {
    let mut sections: Vec<(usize, String)> = Vec::new();
    let mut max_section = 0;
    for line in body.lines() {
        let trimmed_start = line.trim_start();
        // Accept a marker only if its number advances (sections run 1..8 in
        // order) AND the remainder carries the `Label:` colon. This rejects
        // numbered prose substeps like "3. run cargo test after patch" that
        // would otherwise be misread as a section header and silently dropped.
        let after_marker = section_marker(trimmed_start)
            .filter(|&number| number > max_section)
            .and_then(|number| {
                trimmed_start
                    .split_once(". ")
                    .map(|(_, rest)| rest)
                    .filter(|rest| rest.contains(':'))
                    .map(|rest| (number, rest))
            });
        if let Some((number, rest)) = after_marker {
            let content = rest
                .split_once(':')
                .map_or("", |(_, value)| value)
                .trim_start()
                .to_string();
            max_section = number;
            sections.push((number, content));
        } else if let Some(last) = sections.last_mut() {
            if !line.trim().is_empty() {
                if !last.1.is_empty() {
                    last.1.push('\n');
                }
                last.1.push_str(line.trim_end());
            }
        }
    }
    sections
}

/// Recognize a `1. ` .. `8. ` section marker at the start of a line.
fn section_marker(line: &str) -> Option<usize> {
    let mut chars = line.chars();
    let digit = chars.next()?;
    if chars.next()? != '.' || chars.next()? != ' ' {
        return None;
    }
    let number = digit.to_digit(10)? as usize;
    (1..=8).contains(&number).then_some(number)
}

/// One item per non-empty line, bullet markers and `None`/`N/A` placeholders stripped.
fn section_items(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['-', '*', ' '])
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty() && line != "None" && line != "N/A")
        .collect()
}

/// Fold this round's `delta` into the `prior` anchor.
///
/// Every list section accumulates **losslessly** — prior entries carry forward
/// verbatim and the delta is appended with exact-duplicate dedup, so a fact
/// captured in an early round is byte-identical many rounds later. (An earlier
/// design superseded `files` by path, but that silently dropped distinct facts
/// about the *same* file — two different functions in `lib.rs` — so it was
/// removed in favor of strict no-loss accumulation. Semantic supersede of a
/// corrected fact needs an explicit correction marker from the summarizer and
/// is deferred.) `intent` is set once and kept; `current_work` is the only
/// latest-wins field (the prior "current work" is, by definition, now history
/// preserved in `problem_solving` and the raw vault).
///
/// `vault_ranges` are carried forward verbatim from `prior` (the delta parsed
/// from a summary never sets them). The NEW round's span is appended once by
/// [`apply_compaction`] after the seal returns it — never here — so folding can
/// never double-count a span.
fn fold_anchor(prior: Option<&AnchorSummary>, delta: AnchorSummary) -> AnchorSummary {
    let Some(prior) = prior else {
        return delta;
    };
    let mut folded = prior.clone();
    if folded.intent.is_empty() {
        folded.intent = delta.intent;
    }
    folded.concepts = union_dedup(folded.concepts, delta.concepts);
    folded.files = union_dedup(folded.files, delta.files);
    folded.errors_and_fixes = union_dedup(folded.errors_and_fixes, delta.errors_and_fixes);
    folded.problem_solving = union_dedup(folded.problem_solving, delta.problem_solving);
    folded.user_messages = union_dedup(folded.user_messages, delta.user_messages);
    // Pending tasks accumulate rather than replace: a per-round delta only
    // restates the tasks it saw, so replacing would silently drop a task still
    // pending from an earlier round. Lingering-completed is tolerable (the model
    // reconciles against the verbatim recent tail); losing a real pending task
    // is not.
    //
    // Restatement REFRESHES position, unlike the other sections. A round that
    // still sees a task is live evidence the task is still open, so the list
    // orders by "most recently observed pending" — which is what lets
    // `bound_anchor` cap this section while still honoring the invariant
    // above. Keeping the first-seen position instead would have inverted it:
    // a task restated every single round would sit at the front and be evicted
    // FIRST, while a task nothing has mentioned in twenty rounds outlived it.
    folded.pending_tasks = union_refreshing(folded.pending_tasks, delta.pending_tasks);
    if !delta.current_work.is_empty() {
        folded.current_work = delta.current_work;
    }
    folded
}

fn union_dedup(mut base: Vec<String>, extra: Vec<String>) -> Vec<String> {
    for item in extra {
        if !base.contains(&item) {
            base.push(item);
        }
    }
    base
}

/// [`union_dedup`], except a repeat moves to the BACK instead of keeping its
/// first-seen position — so position means "most recently restated".
///
/// Only `pending_tasks` wants this. For a knowledge section (concepts, files)
/// first-seen order is a stable reading order and a repeat carries no news.
/// For a task list a repeat is the whole signal: the round looked and the task
/// was still open. That ordering is what makes eviction from the front drop
/// the items most likely already done.
fn union_refreshing(base: Vec<String>, extra: Vec<String>) -> Vec<String> {
    let mut merged: Vec<String> = base
        .into_iter()
        .filter(|item| !extra.contains(item))
        .collect();
    for item in extra {
        if !merged.contains(&item) {
            merged.push(item);
        }
    }
    merged
}

/// Per-section char budget for the accumulating anchor sections. The `fold_anchor`
/// union grows these sections without bound, so a long-lived session's running
/// summary can swell until it dominates the context window — a live 18-hour
/// session reached a ~150k-token anchor that alone filled 60% of a 258k GPT
/// window, leaving compaction only the small compactable tail to reclaim before
/// it re-fired within a few turns. Bounding each accumulating section keeps the
/// summary small; evicted detail stays recoverable via `session_recall` (every
/// evicted original is sealed to the vault). ~3k tokens per section leaves the
/// most recent tens of entries — ample for continuity — while capping the total.
const ANCHOR_SECTION_MAX_CHARS: usize = 12_000;

/// Per-round char budget for `pending_tasks`, kept separate from
/// [`ANCHOR_SECTION_MAX_CHARS`] and deliberately larger.
///
/// A task list is the section where an eviction has a consequence beyond lost
/// reading detail, so it gets the most room the anchor can afford — but it
/// cannot be the one section with NO bound. Left unbounded it was the only
/// part of the anchor that grew forever, sitting at index 0 of every request,
/// and it is self-accelerating: a bigger anchor raises the post-compaction
/// floor, so the next threshold crossing arrives sooner, so more rounds fire,
/// so it grows faster. At ~24k chars a session would need to hold hundreds of
/// simultaneously-open tasks to reach it, and the eviction order
/// ([`union_refreshing`]) drops the ones no recent round restated first.
const PENDING_TASKS_MAX_CHARS: usize = 24_000;

/// Bound the accumulating anchor sections so the running summary cannot grow to
/// dominate the context window over a long session. Only the union-accumulating
/// sections are trimmed; `intent`/`current_work` are single latest-state strings
/// and `vault_ranges` is location metadata, not prose — both left intact.
/// Applied once per round in [`apply_compaction`], AFTER the raw originals are
/// sealed to the vault, so nothing trimmed here is unrecoverable (it stays
/// reachable through `session_recall`).
///
/// `pending_tasks` is bounded on its own, larger budget rather than exempted.
/// The exemption was there to honor "never silently drop a still-open item",
/// but it bought that at the price of the only unbounded section in the anchor
/// — and it was not even buying it reliably, since under the old first-seen
/// ordering the tasks restated every round were the ones sitting closest to
/// the front. With restatement refreshing position, the front of this list is
/// precisely the set nothing has mentioned in a long time, which is the right
/// thing to evict first and the closest a bounded list can come to that
/// invariant.
fn bound_anchor(anchor: &mut AnchorSummary) {
    for section in [
        &mut anchor.concepts,
        &mut anchor.files,
        &mut anchor.errors_and_fixes,
        &mut anchor.problem_solving,
        &mut anchor.user_messages,
    ] {
        retain_recent_within_budget(section, ANCHOR_SECTION_MAX_CHARS);
    }
    retain_recent_within_budget(&mut anchor.pending_tasks, PENDING_TASKS_MAX_CHARS);
}

/// Drop oldest (front) items until the remaining items' cumulative rendered
/// length fits `budget`, keeping the most recent. Each item's rendered cost
/// includes the `"- "` bullet prefix and trailing newline that
/// [`render_list_section`] adds. The most recent item is always kept even if it
/// alone exceeds `budget` — dropping the newest entry loses more than a slightly
/// over-budget section, and truncating mid-item would corrupt an identifier.
fn retain_recent_within_budget(items: &mut Vec<String>, budget: usize) {
    const PER_ITEM_OVERHEAD: usize = "- \n".len();
    let mut total = 0usize;
    let mut kept = 0usize;
    for item in items.iter().rev() {
        let next = total.saturating_add(item.len() + PER_ITEM_OVERHEAD);
        if next > budget && kept > 0 {
            break;
        }
        total = next;
        kept += 1;
    }
    items.drain(0..items.len() - kept);
}

/// Render the typed anchor back into a `<summary>` block — the single,
/// drift-free source the model-facing continuation message is built from.
fn render_anchor_to_summary_text(anchor: &AnchorSummary) -> String {
    let mut lines = vec!["<summary>".to_string()];
    if !anchor.intent.is_empty() {
        lines.push(format!("1. Primary Request and Intent: {}", anchor.intent));
    }
    render_list_section(&mut lines, "2. Key Technical Concepts", &anchor.concepts);
    render_list_section(&mut lines, "3. Files and Code Sections", &anchor.files);
    render_list_section(&mut lines, "4. Errors and Fixes", &anchor.errors_and_fixes);
    render_list_section(&mut lines, "5. Problem Solving", &anchor.problem_solving);
    render_list_section(&mut lines, "6. All User Messages", &anchor.user_messages);
    render_list_section(&mut lines, "7. Pending Tasks", &anchor.pending_tasks);
    if !anchor.current_work.is_empty() {
        lines.push(format!("8. Current Work: {}", anchor.current_work));
    }
    lines.push("</summary>".to_string());
    lines.join("\n")
}

fn render_list_section(lines: &mut Vec<String>, header: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(format!("{header}:"));
    lines.extend(items.iter().map(|item| format!("- {item}")));
}

/// Conservative deterministic faithfulness check (LAVA P1 verifier): does the
/// API summary cite path/code identifiers that appear NOWHERE in the evicted
/// source (nor the prior anchor)? Returns `true` only on egregious fabrication —
/// several identifiers, none grounded — so a faithful-but-paraphrased summary is
/// never rejected. The caller falls back to the non-fabricating
/// [`LocalSummarizer`] when this trips.
#[must_use]
pub fn summary_fabricates_identifiers(
    raw_summary: &str,
    evicted: &[ConversationMessage],
    prior: Option<&AnchorSummary>,
) -> bool {
    let identifiers = extract_verifiable_identifiers(raw_summary);
    if identifiers.len() < 3 {
        return false;
    }
    let haystack = grounding_haystack(evicted, prior);
    let grounded = identifiers
        .iter()
        .filter(|identifier| haystack.contains(identifier.as_str()))
        .count();
    // Reject when the MAJORITY of cited identifiers are ungrounded — so recycling
    // one real path can't immunize many fabricated ones, while a faithful summary
    // that paraphrases a single identifier still passes.
    let ungrounded = identifiers.len() - grounded;
    ungrounded * 2 > identifiers.len()
}

/// Extract high-confidence verifiable identifiers from a summary: single-token
/// backtick spans and path-like bare tokens (`contains('/')` and an extension).
fn extract_verifiable_identifiers(summary: &str) -> Vec<String> {
    let body = format_compact_summary(summary);
    let mut identifiers = Vec::new();
    let mut rest = body.as_str();
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else { break };
        let span = rest[..close].trim();
        if span.len() >= 3 && !span.contains(char::is_whitespace) {
            identifiers.push(span.to_string());
        }
        rest = &rest[close + 1..];
    }
    for token in body.split(|c: char| c.is_whitespace() || matches!(c, '`' | ',' | '(' | ')' | '"')) {
        let trimmed = token.trim_matches(|c: char| matches!(c, '.' | ',' | ':' | ';'));
        if trimmed.contains('/') && trimmed.contains('.') && trimmed.len() > 4 && !trimmed.contains("://")
        {
            identifiers.push(trimmed.to_string());
        }
    }
    identifiers.sort_unstable();
    identifiers.dedup();
    identifiers
}

/// Concatenated text of the evicted messages plus the prior anchor — the ground
/// truth a summary's identifiers must be verbatim-present in.
fn grounding_haystack(evicted: &[ConversationMessage], prior: Option<&AnchorSummary>) -> String {
    let mut haystack = String::new();
    for message in evicted {
        for block in &message.blocks {
            match block {
                ContentBlock::Text { text } => haystack.push_str(text),
                ContentBlock::ToolUse { name, input, .. } => {
                    haystack.push_str(name);
                    haystack.push(' ');
                    haystack.push_str(input);
                }
                ContentBlock::ToolResult {
                    tool_name, output, ..
                } => {
                    haystack.push_str(tool_name);
                    haystack.push(' ');
                    haystack.push_str(output);
                }
                // Reasoning blocks are internal, not a source of ground-truth
                // identifiers a summary must cite — excluded like images.
                ContentBlock::Image { .. }
                | ContentBlock::Thinking { .. }
                | ContentBlock::RedactedThinking { .. } => {}
            }
            haystack.push('\n');
        }
    }
    if let Some(prior) = prior {
        haystack.push_str(&render_anchor_to_summary_text(prior));
    }
    haystack
}

fn summarize_block(block: &ContentBlock) -> String {
    let raw = match block {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::ToolUse { name, input, .. } => format!("tool_use {name}({input})"),
        ContentBlock::ToolResult {
            tool_name,
            output,
            is_error,
            ..
        } => format!(
            "tool_result {tool_name}: {}{output}",
            if *is_error { "error " } else { "" }
        ),
        ContentBlock::Image { media_type, .. } => format!("[image: {media_type}]"),
        ContentBlock::Thinking { .. } => "[thinking]".to_string(),
        ContentBlock::RedactedThinking { .. } => "[redacted thinking]".to_string(),
    };
    truncate_summary(&raw, 160)
}

fn collect_recent_role_summaries(
    messages: &[ConversationMessage],
    role: MessageRole,
    limit: usize,
) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message.role == role)
        .rev()
        .filter_map(|message| first_text_block(message))
        .take(limit)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn infer_pending_work(messages: &[ConversationMessage]) -> Vec<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .filter(|text| {
            let lowered = text.to_ascii_lowercase();
            lowered.contains("todo")
                || lowered.contains("next")
                || lowered.contains("pending")
                || lowered.contains("follow up")
                || lowered.contains("remaining")
        })
        .take(3)
        .map(|text| truncate_summary(text, 160))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn collect_key_files(messages: &[ConversationMessage]) -> Vec<String> {
    let mut files = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .map(|block| match block {
            ContentBlock::Text { text } => text.as_str(),
            ContentBlock::ToolUse { input, .. } => input.as_str(),
            ContentBlock::ToolResult { output, .. } => output.as_str(),
            ContentBlock::Image { .. }
            | ContentBlock::Thinking { .. }
            | ContentBlock::RedactedThinking { .. } => "",
        })
        .flat_map(extract_file_candidates)
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files.into_iter().take(8).collect()
}

fn infer_current_work(messages: &[ConversationMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter_map(first_text_block)
        .find(|text| !text.trim().is_empty())
        .map(|text| truncate_summary(text, 200))
}

fn first_text_block(message: &ConversationMessage) -> Option<&str> {
    message.blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
        ContentBlock::ToolUse { .. }
        | ContentBlock::ToolResult { .. }
        | ContentBlock::Text { .. }
        | ContentBlock::Image { .. }
        | ContentBlock::Thinking { .. }
        | ContentBlock::RedactedThinking { .. } => None,
    })
}

fn has_interesting_extension(candidate: &str) -> bool {
    std::path::Path::new(candidate)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["rs", "ts", "tsx", "js", "json", "md"]
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

fn extract_file_candidates(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter_map(|token| {
            let candidate = token.trim_matches(|char: char| {
                matches!(char, ',' | '.' | ':' | ';' | ')' | '(' | '"' | '\'' | '`')
            });
            if candidate.contains('/') && has_interesting_extension(candidate) {
                Some(candidate.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn truncate_summary(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut truncated = content.chars().take(max_chars).collect::<String>();
    truncated.push('…');
    truncated
}

/// Per-message overhead added for role markers, message boundaries, and
/// structural framing that the tokenizer produces but raw text does not.
const MESSAGE_OVERHEAD_TOKENS: usize = 4;

fn estimate_message_tokens(message: &ConversationMessage) -> usize {
    let content_tokens: usize = message
        .blocks
        .iter()
        .map(|block| match block {
            // Use byte length / 4 as a fast O(1) token estimate.
            // For ASCII-dominant content (code, English) this matches
            // chars().count()/4 exactly. For CJK/emoji it slightly
            // overestimates (UTF-8 multibyte → higher byte count) which
            // is acceptable for compaction threshold decisions.
            ContentBlock::Text { text } => text.len() / 4 + 1,
            ContentBlock::ToolUse { name, input, .. } => (name.len() + input.len()) / 4 + 8,
            ContentBlock::ToolResult {
                tool_name,
                output,
                images,
                ..
            } => (tool_name.len() + output.len()) / 4 + 8 + images.len() * 1_600,
            // Reasoning replayed to the wire is billed like ordinary tokens;
            // estimate from its byte length the same way as text.
            ContentBlock::Thinking { thinking, signature } => {
                (thinking.len() + signature.len()) / 4 + 1
            }
            ContentBlock::RedactedThinking { data } => data.len() / 4 + 1,
            ContentBlock::Image { .. } => {
                // Anthropic charges image tokens based on pixel dimensions,
                // not base64 data size. A typical image costs 1000–1600
                // tokens. Use a conservative fixed estimate since we don't
                // have pixel dimensions here.
                1_600
            }
        })
        .sum();
    content_tokens + MESSAGE_OVERHEAD_TOKENS
}

/// Placeholder left in place of a cleared tool-result body. Wording matches
/// Claude Code so models recognize the marker as "this result existed but was
/// trimmed", not as live tool output.
pub const MICROCOMPACT_PLACEHOLDER: &str = "[Old tool result content cleared]";

/// Replacement text for an old user-pasted image cleared by microcompact.
pub const MICROCOMPACT_IMAGE_PLACEHOLDER: &str =
    "[Old pasted image cleared to save context — ask the user to re-attach it if needed]";

/// Replacement text for a persisted reminder block that a byte-identical later
/// copy has superseded.
///
/// Deliberately terse and NOT itself a `<system-reminder>`: it must not read as
/// a live instruction, and it must not match
/// [`crate::is_persisted_reminder_text`], or a cleared block would be a
/// clearing candidate again on the next pass and the operation would stop being
/// idempotent.
///
/// That non-matching is why the value is defined in `core-types` beside
/// [`crate::session::is_reminder_lineage_text`] rather than here: everything
/// that asks "is this block reminder machinery?" — the TUI's resume seeder,
/// [`crate::session::ConversationMessage::is_reminder_annotation`] — must
/// recognize a cleared block, and only the CLEARING question may refuse it.
pub const MICROCOMPACT_REMINDER_PLACEHOLDER: &str =
    crate::session::CLEARED_REMINDER_PLACEHOLDER;

/// Tool names whose result body is a *mutation record* (the applied diff /
/// written content), not a re-readable observation. Their bodies are exempt
/// from the microcompact trim: unlike a `read_file` or `grep_search` body —
/// which the model can always reproduce by calling the tool again — a cleared
/// `edit_file`/`write_file` diff is unrecoverable from context and erases the
/// evidence that the change was *already applied*. Losing it is the root cause
/// of the "model re-edits or reverts its own prior change after compaction"
/// failure on long sessions. Kept in lockstep with the conversation layer's
/// `is_edit_or_write_tool` (the two must agree on what counts as a mutation).
const EDIT_RESULT_TOOL_NAMES: &[&str] = &[
    "Edit",
    "MultiEdit",
    "Write",
    "NotebookEdit",
    "edit_file",
    "write_file",
];

/// File-edit verbs recognized on a *namespaced* tool's leaf — the segment after
/// the last `__` (e.g. an MCP file server's `mcp__fs__write_file`, or a plugin's
/// `myeditor__edit_file`). Deliberately limited to verbs that denote an APPLIED
/// FILE EDIT whose diff body must survive, so a non-file MCP tool
/// (`mcp__memory__search`, `mcp__db__create_entities`, `mcp__fs__move_file`) is
/// never misclassified as a mutation.
const EDIT_RESULT_TOOL_LEAF_VERBS: &[&str] = &["write_file", "edit_file"];

/// Whether a tool result records a file mutation (an applied edit/write) whose
/// body must survive the microcompact trim. Matches the built-in
/// `EDIT_RESULT_TOOL_NAMES` exactly, plus any MCP/plugin-namespaced tool whose
/// leaf verb is a recognized file edit (`mcp__<server>__write_file` etc.) — so an
/// edit applied through an MCP file server or editor plugin is preserved the same
/// way a built-in `edit_file` is, instead of being cleared and then reverted.
#[must_use]
pub fn is_edit_result_tool(tool_name: &str) -> bool {
    if EDIT_RESULT_TOOL_NAMES.contains(&tool_name) {
        return true;
    }
    // Leaf matching applies only to *namespaced* tools (containing `__`); a bare
    // name is authoritative via the exact list above, never broadened here.
    let leaf = tool_name.rsplit("__").next().unwrap_or(tool_name);
    leaf != tool_name && EDIT_RESULT_TOOL_LEAF_VERBS.contains(&leaf)
}

/// Extract the distinct file paths mutated by edit/write tool results in
/// `messages`, in first-seen order. Pure and tolerant: each
/// [`is_edit_result_tool`] result's JSON envelope is parsed for its `filePath`
/// (the canonical key the `edit_file`/`write_file` envelopes serialize; `path`
/// / `file_path` are accepted as fallbacks), and anything that does not parse
/// is skipped, never fatal. A microcompact-cleared placeholder body yields no
/// path, so a trimmed result simply contributes nothing.
///
/// This is the bridge that lets both the durable turn trace
/// (`turn_trace::TurnRecord::files_edited`) and the post-compaction reminder
/// name *which files this session already changed*, so the model does not
/// re-apply or revert its own prior edit once the original diff scrolls out of
/// the context window.
#[must_use]
pub fn edited_file_paths(messages: &[ConversationMessage]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for message in messages {
        for block in &message.blocks {
            let ContentBlock::ToolResult {
                tool_name, output, ..
            } = block
            else {
                continue;
            };
            if !is_edit_result_tool(tool_name) {
                continue;
            }
            if let Some(path) = edited_path_from_output(output) {
                if !paths.iter().any(|existing| existing == &path) {
                    paths.push(path);
                }
            }
        }
    }
    paths
}

/// Pull the mutated file path out of one edit/write result envelope. Tolerant:
/// returns `None` for a cleared placeholder, non-JSON, or an envelope without a
/// recognizable path key.
fn edited_path_from_output(output: &str) -> Option<String> {
    if output == MICROCOMPACT_PLACEHOLDER {
        return None;
    }
    let value = serde_json::from_str::<serde_json::Value>(output).ok()?;
    let object = value.as_object()?;
    ["filePath", "path", "file_path"]
        .iter()
        .find_map(|key| object.get(*key).and_then(|v| v.as_str()))
        .map(str::to_string)
}

/// Outcome of one [`microcompact_session`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MicrocompactEvent {
    /// Tool-result bodies replaced with [`MICROCOMPACT_PLACEHOLDER`].
    pub cleared_results: usize,
    /// Standalone user-pasted images replaced with
    /// [`MICROCOMPACT_IMAGE_PLACEHOLDER`]. Tool-result images are cleared with
    /// their result and counted there, not here.
    pub cleared_images: usize,
    /// Persisted reminder blocks a byte-identical later copy superseded (or, for
    /// the recalled-memory lineage, any later section), replaced with
    /// `MICROCOMPACT_REMINDER_PLACEHOLDER`.
    ///
    /// When `mutation_frontier` is `Some`, this counts only the copies at or
    /// after it — the pass never reaches further back into the prefix than the
    /// tool-result/image passes already do. When it is `None` (this pass clears
    /// no tool result and no image, so there is no re-billed region to ride on),
    /// the filter is `is_none_or` and this counts EVERY superseded copy in the
    /// whole transcript: the reminders are then the entire pass and set the
    /// divergence index themselves. See `plan_superseded_reminders` for why
    /// that case stays enabled and what prices it.
    pub cleared_superseded_reminders: usize,
    /// Fast estimate of the context tokens the clearing freed.
    pub estimated_tokens_saved: u64,
}

/// One [`microcompact_session`] pass's candidate selection, computed
/// read-only so it can be shared between the estimate helper (which never
/// mutates) and the actual clear (which consumes it once). Keeping this
/// selection in exactly one place means the two can never drift apart: the
/// batch an estimate reports is, by construction, the same batch a
/// subsequent clear would act on.
struct MicrocompactPlan {
    /// `(message_index, block_index)` of each tool result to blank.
    tool_results: Vec<(usize, usize)>,
    /// `(message_index, block_index)` of each standalone image to placeholder.
    images: Vec<(usize, usize)>,
    /// `(message_index, block_index)` of each persisted reminder block a later
    /// copy has superseded, restricted to [`mutation_frontier`] so this pass
    /// never reaches further back than the tool-result/image passes already do —
    /// except when that frontier is `None`, where the whole transcript is in
    /// scope because the reminders ARE the pass (see
    /// [`plan_superseded_reminders`]).
    superseded_reminders: Vec<(usize, usize)>,
}

/// Locate every tool result and standalone image [`microcompact_session`]
/// would clear on this pass, keeping the most recent `keep_recent` of each
/// intact. Pure and read-only; see [`microcompact_session`]'s doc for the
/// exact eligibility rules (placeholder/exempt/size checks).
fn plan_microcompact_clears(
    session: &Session,
    keep_recent: usize,
    min_output_bytes: usize,
) -> MicrocompactPlan {
    // Pass 1 (read-only): locate every clearable tool result so "keep the
    // most recent N results" counts across the whole transcript.
    let clearable: Vec<(usize, usize)> =
        session
            .messages
            .iter()
            .enumerate()
            .flat_map(|(message_index, message)| {
                message.blocks.iter().enumerate().filter_map(
                    move |(block_index, block)| match block {
                        ContentBlock::ToolResult {
                            tool_name,
                            output,
                            images,
                            ..
                        } if output != MICROCOMPACT_PLACEHOLDER
                            && !is_edit_result_tool(tool_name)
                            && (output.len() >= min_output_bytes || !images.is_empty()) =>
                        {
                            Some((message_index, block_index))
                        }
                        _ => None,
                    },
                )
            })
            .collect();
    let clear_count = clearable.len().saturating_sub(keep_recent);
    // Standalone (user-pasted) images are their own pass: they are not tool
    // results, so before this they were re-sent as base64 (~1,600 tokens each)
    // every request until FULL compaction evicted the whole message. Keep the
    // most recent `keep_recent` images — those may still be under discussion.
    let clearable_images: Vec<(usize, usize)> = session
        .messages
        .iter()
        .enumerate()
        .flat_map(|(message_index, message)| {
            message
                .blocks
                .iter()
                .enumerate()
                .filter_map(move |(block_index, block)| {
                    matches!(block, ContentBlock::Image { .. })
                        .then_some((message_index, block_index))
                })
        })
        .collect();
    let image_clear_count = clearable_images.len().saturating_sub(keep_recent);
    let tool_results: Vec<(usize, usize)> = clearable.into_iter().take(clear_count).collect();
    let images: Vec<(usize, usize)> =
        clearable_images.into_iter().take(image_clear_count).collect();
    // The reminder pass runs LAST and only inside the region the two passes
    // above already price — see `mutation_frontier`.
    let superseded_reminders =
        plan_superseded_reminders(session, mutation_frontier(&tool_results, &images));
    MicrocompactPlan {
        tool_results,
        images,
        superseded_reminders,
    }
}

/// The earliest message index this pass is already going to rewrite in place,
/// or `None` when it would rewrite nothing.
///
/// This index IS the price of a microcompact firing. Both rolling message cache
/// breakpoints sit on the tail ([`crate::convert_messages`] marks the last two
/// cacheable messages and no more), so an in-place edit anywhere in the prefix
/// drops `cache_read` back to the system blocks and re-bills essentially the
/// whole context once. Where the edit lands therefore decides nothing about the
/// bill *except* through this minimum — and everything the pass additionally
/// clears at or after it rides along for free.
fn mutation_frontier(tool_results: &[(usize, usize)], images: &[(usize, usize)]) -> Option<usize> {
    tool_results
        .iter()
        .chain(images.iter())
        .map(|&(message_index, _)| message_index)
        .min()
}

const RECALL_REMINDER_LINEAGE_ANCHOR: &str = "# Recalled memory";

// Keep the sole lineage allowlist entry tied to the renderer's canonical
// header; a renamed header must fail compilation rather than silently disable
// reclamation or broaden it to unrelated reminder families.
const _: () = {
    let header = crate::memory::recall::RECALL_SECTION_HEADER.as_bytes();
    let anchor = RECALL_REMINDER_LINEAGE_ANCHOR.as_bytes();
    assert!(header.len() > anchor.len());
    let mut index = 0;
    while index < anchor.len() {
        assert!(header[index] == anchor[index]);
        index += 1;
    }
    assert!(header[anchor.len()] == b'\n');
};

/// Whether this persisted reminder block IS a recalled-memory section, as
/// opposed to merely mentioning one.
///
/// Structural, never `contains`. `plan_superseded_reminders` walks every
/// `System` message, and a compaction continuation is a `System` message whose
/// summary prose can quote both the reminder tag and this header — this very
/// audit produced such summaries. Under a substring test that summary would be
/// classified into the recall lineage and, being older than the next real
/// recall section, reclaimed: the compressed history of the conversation,
/// replaced by a placeholder. The anchor therefore has to sit where the
/// renderer actually puts it, immediately inside the reminder wrapper.
fn is_recalled_memory_reminder(text: &str) -> bool {
    text.split_once(crate::session::REMINDER_TAG_OPEN)
        .is_some_and(|(_, body)| body.trim_start().starts_with(RECALL_REMINDER_LINEAGE_ANCHOR))
}

/// Every persisted reminder block a LATER copy supersedes — byte-identically for
/// every family but the recalled-memory lineage, where any newer section counts
/// (the paragraph on the allowlist below owns that distinction).
///
/// The reminder set is re-appended to the transcript whenever the live set
/// falls out of the dedupe window, deliberately, so the nag stays near the
/// tail. What is deliberate is keeping the newest copy near the tail — nothing
/// wanted the older ones, and until now nothing could reclaim them: microcompact
/// clears only tool results and images, so this mass was structurally invisible
/// to it and only a full compaction round could evict it. Measured on real
/// sessions that is tens of thousands of tokens (one repeated a single
/// 2,170-byte reminder seventeen times), and because it is context nothing can
/// reclaim, it pulls each full compaction round earlier than it needed to be.
///
/// Only an EXACT byte match counts as superseded except for recalled-memory
/// sections, whose latest copy can reseed any full body reclaimed from an older
/// copy. No other reminder family has that contract, so the lineage allowlist
/// deliberately contains only [`RECALL_REMINDER_LINEAGE_ANCHOR`]. The LAST copy
/// is always kept, with no `keep_recent` slack: an older superseded copy carries
/// no information the newest valid copy does not.
///
/// FRONTIER DISCIPLINE. `frontier` is [`mutation_frontier`] — the earliest
/// message index the tool-result and image passes of this same pass are already
/// rewriting. Only reminders at or after it are reclaimed, because those sit
/// inside the prefix that is being re-billed anyway, so taking them costs
/// nothing extra. Without that restriction this scan had no lower bound at all
/// (it still has no `keep_recent`), so a single 100-byte duplicate reminder near
/// message 1 could drag the divergence index to the head of a 900-message
/// conversation and re-bill ~95% of the context to reclaim ~25 tokens. Measured
/// across three days of real `breaks.jsonl`: rows diverging in the first dozen
/// messages carry ~9.0M of 35.5M pure-divergence `cache_creation`.
///
/// The trade-off is deliberate and one-directional: the tool frontier only ever
/// moves FORWARD (a cleared body is never a candidate again), so a superseded
/// copy left behind it is not picked up by a later pass either — it keeps
/// costing its own bytes until full compaction evicts it. Those bytes are a few
/// hundred per copy per request; pulling the divergence index to the head costs
/// a whole-context re-bill, once, at cache-write price. The first firing on a
/// fresh prefix clears from the OLDEST results, so in practice the stranded band
/// is only what precedes the very first clearable tool result.
///
/// `frontier == None` means this pass clears no tool result and no image, so
/// the reminders ARE the pass and have no free region to ride on — they set the
/// divergence index themselves. They are still planned rather than dropped,
/// because the caller's break-even gate
/// (`ConversationRuntime::maybe_microcompact_for_tokens`) is calibrated for
/// exactly that worst case: it requires the whole batch to be at least a fifth
/// of the live context before anything fires. That makes "a lone small reminder
/// buys a whole-prefix rewrite" unreachable, while the case this pass was
/// written for — a long, low-tool-traffic session whose only reclaimable mass is
/// tens of thousands of tokens of repeated nag — still reaches it. Dropping the
/// pass whenever the tool set is empty would have switched it off precisely
/// there, since an empty tool set is also what "fewer than `keep_recent`
/// clearable results" looks like.
fn plan_superseded_reminders(session: &Session, frontier: Option<usize>) -> Vec<(usize, usize)> {
    let mut newest_exact_at: std::collections::HashMap<&str, (usize, usize)> =
        std::collections::HashMap::new();
    let mut newest_recall_at: Option<(usize, usize)> = None;
    let mut candidates: Vec<((usize, usize), &str, bool)> = Vec::new();
    // The "which copy is newest" scan deliberately covers the WHOLE transcript
    // even when a frontier will discard most of the result: a copy at or after
    // the frontier is only superseded if a later copy exists, and answering that
    // from a truncated view could clear the last surviving copy of a reminder
    // whose earlier duplicates were the ones cut off.
    for (message_index, message) in session.messages.iter().enumerate() {
        if message.role != MessageRole::System {
            continue;
        }
        for (block_index, block) in message.blocks.iter().enumerate() {
            let ContentBlock::Text { text } = block else {
                continue;
            };
            if !crate::convert_messages::is_persisted_reminder_text(text) {
                continue;
            }
            let position = (message_index, block_index);
            let is_recall = is_recalled_memory_reminder(text);
            if is_recall {
                newest_recall_at = Some(position);
            } else {
                newest_exact_at.insert(text.as_str(), position);
            }
            candidates.push((position, text.as_str(), is_recall));
        }
    }
    candidates
        .into_iter()
        .filter(|((message_index, _), _, _)| {
            frontier.is_none_or(|earliest| *message_index >= earliest)
        })
        .filter(|(position, text, is_recall)| {
            if *is_recall {
                newest_recall_at != Some(*position)
            } else {
                newest_exact_at.get(text) != Some(position)
            }
        })
        .map(|(position, _, _)| position)
        .collect()
}

/// Fast token estimate for a [`MicrocompactPlan`] against the session it was
/// planned from: cleared bytes / 4 for each tool result plus 1,600 per image
/// (tool-result-embedded or standalone) — the same arithmetic
/// [`microcompact_session`] used to report `estimated_tokens_saved` before
/// this was factored out. Must be called before any mutation of `session`
/// consumes the plan's indices.
fn plan_estimated_tokens(session: &Session, plan: &MicrocompactPlan) -> u64 {
    let mut estimated = 0u64;
    for &(message_index, block_index) in &plan.tool_results {
        if let ContentBlock::ToolResult { output, images, .. } =
            &session.messages[message_index].blocks[block_index]
        {
            estimated += output.len().saturating_sub(MICROCOMPACT_PLACEHOLDER.len()) as u64 / 4
                + images.len() as u64 * 1_600;
        }
    }
    estimated += plan.images.len() as u64 * 1_600;
    for &(message_index, block_index) in &plan.superseded_reminders {
        if let ContentBlock::Text { text } = &session.messages[message_index].blocks[block_index] {
            estimated += text
                .len()
                .saturating_sub(MICROCOMPACT_REMINDER_PLACEHOLDER.len()) as u64
                / 4;
        }
    }
    estimated
}

/// Seal the pre-clear ORIGINAL of every message this plan is about to mutate
/// into the session's Raw Vault, so the bodies survive the trim.
///
/// This is the fix for a defect that made the vault's "lossless" guarantee false
/// in production: microcompact's clear IS persisted (the pass ends with
/// `mark_transcript_dirty`, which forces the next persist onto a full snapshot),
/// so by the time full compaction ran and tried to recover the originals from
/// disk, the disk already held the placeholder and the seal quietly stored the
/// placeholder instead — measured at 35-70% of tool results across real
/// `.vault.jsonl` files. Sealing here removes the race entirely: the original is
/// captured while it is still in hand, one statement before it is destroyed.
///
/// SCOPE. Messages carrying a cleared tool result or a cleared standalone image
/// are sealed; messages carrying only a superseded reminder are NOT. The two
/// reminder families this leaves behind are not equivalent, and only one of them
/// is genuinely free:
///
/// * EXACT-DUPLICATE family — the cleared copy is byte-identical to a later one
///   that survives, so the bytes are still in the transcript and will themselves
///   be sealed intact when full compaction evicts that later copy. Sealing here
///   would write a second copy of bytes the vault is already guaranteed to
///   receive. Nothing is lost but *positional* recovery: "what stood at seq N"
///   answers with the placeholder.
/// * RECALL LINEAGE — [`plan_superseded_reminders`] takes `newest_recall_at`
///   from ANY recalled-memory section and clears every earlier one, byte-identical
///   or not. What survives is therefore the reseed CONTRACT (the newest section
///   plus a `recalled_memory_slugs` recount can re-render an EQUIVALENT body, and
///   every cleared entry line named the memory file it was rendered from), NOT
///   the original bytes. The historical rendering — which entries were relevant
///   at that turn, and how each snippet read then — is given up.
///
/// The recall-lineage bytes are deliberately NOT sealed, against the general
/// preference in this pass for sealing anything it destroys, because a seal here
/// would be write-only: a reminder message carries no `tool_use_id` and no
/// `Image`, which are the only two things either read path keys a heal off
/// (`session_recall`'s `healed_live_messages`), and while the message is still
/// live its record sits at or above `first_message_index`, where both readers
/// exclude it from the evicted listing. Once full compaction does evict the
/// message, its own seal writes the PLACEHOLDER — `restore_microcompacted_bodies`
/// heals tool-result bodies and standalone images, never reminders — into the
/// contiguous run of seqs that swallows the microcompact record's seq, and
/// because that run is appended later, last-record-wins dedup drops the original.
/// So the cost would be one vault record per cleared lineage reminder for bytes
/// no reader can surface. Making it worth paying needs two changes this pass is
/// not making: a reminder-placeholder heal in `restore_microcompacted_bodies` (so
/// the compaction seal carries the original), and a reminder arm in
/// `healed_live_messages` (so recall can show it). Both are positional, on the
/// axis that is misaligned by one (see [`Session::seal_originals_to_vault`]), and
/// unlike an image a reminder has no verification to lean on: the clear replaces
/// its bytes with a fixed string, so nothing about the placeholder identifies
/// which sealed record was its original (`is_pre_clear_original_of` deliberately
/// refuses that pairing for the same reason).
///
/// The whole message is sealed, not just the blocks being cleared, because the
/// vault's unit is a message. That is also what gives a standalone `Image`, which
/// has no stable id, an original to be recovered FROM — but the seal alone does
/// not make it recoverable: full compaction re-seals that same message, now
/// carrying the placeholder, and last-record-wins dedup buries the good record
/// unless the eviction path heals the image first. It does, positionally and
/// verified (see [`heal_cleared_images`]); before that heal existed, every sealed
/// pasted image was destroyed by the next full compaction. It does NOT put this
/// record on the same `vault_seq` as the later full-compaction seal of the same
/// message except on a session's first compaction round — see the AXIS note on
/// [`Session::seal_originals_to_vault`] for why a duplicate line is expected
/// from the second round on.
///
/// A message already holding an EARLIER round's placeholder — of either kind — is
/// healed from the vault first, so round two cannot seal a partially-cleared
/// snapshot over round one's pristine record (dedup is last-wins). The guard used
/// to ask about tool-result placeholders alone, which left the image instance of
/// exactly the same burial live: a message holding two pasted images has its
/// second image cleared a round after its first, and re-sealing it with image one
/// already blanked would have buried the record that still had it. That heal
/// reads the vault, so it is guarded on the batch actually containing such a
/// message — rare in practice: across 400 real session transcripts, all 35,163
/// tool-result-bearing messages carried EXACTLY one result, so a message can
/// almost never be selected twice. Steady-state cost of this function is
/// therefore one append, no read.
///
/// Returns whether the caller may now destroy those bodies — `false` only when
/// the vault append FAILED, so the sole surviving copy is still the one in
/// memory. An empty selection returns `true`: this function wrote nothing, so
/// there is no append failure to report. A reminder-only plan is the live
/// instance of that, and SCOPE above states exactly what it costs. The
/// `true`-with-nothing-written modes are enumerated on
/// [`Session::seal_originals_to_vault`], which owns them.
#[must_use]
fn seal_microcompact_originals(session: &Session, plan: &MicrocompactPlan) -> bool {
    let mut indices: Vec<usize> = plan
        .tool_results
        .iter()
        .chain(plan.images.iter())
        .map(|&(message_index, _)| message_index)
        .collect();
    indices.sort_unstable();
    indices.dedup();
    if indices.is_empty() {
        return true;
    }
    let base_seq = session.first_message_index();
    let seqs: Vec<u32> = indices
        .iter()
        .map(|&message_index| {
            base_seq.saturating_add(u32::try_from(message_index).unwrap_or(u32::MAX))
        })
        .collect();
    let mut batch: Vec<ConversationMessage> = indices
        .iter()
        .map(|&message_index| session.messages[message_index].clone())
        .collect();
    if needs_microcompact_heal(&batch) {
        restore_microcompacted_bodies(&mut batch, session, &seqs);
    }
    let originals: Vec<(u32, ConversationMessage)> = seqs.into_iter().zip(batch).collect();
    session.seal_originals_to_vault(&originals)
}

/// Read-only token estimate of what [`microcompact_session`] would free on
/// `session` right now, without mutating anything. Exists so a caller can
/// decide whether firing is even worth it *before* paying for it: a
/// microcompact pass invalidates the prompt cache from its earliest cleared
/// block onward, so every firing re-bills the entire prefix up to that point
/// on the next request — a break-even check needs this number before the
/// clear happens, not after.
#[must_use]
pub fn microcompact_clearable_estimate(
    session: &Session,
    keep_recent: usize,
    min_output_bytes: usize,
) -> u64 {
    microcompact_quote(session, keep_recent, min_output_bytes).clearable_tokens
}

/// Both sides of a microcompact firing, priced before it happens.
///
/// The gain has always been available ([`microcompact_clearable_estimate`]);
/// the COST was being guessed. A firing invalidates the prompt cache from its
/// earliest rewritten message onward, so what it re-bills is the transcript
/// from that message to the end — and where that message sits matters far more
/// than how many blocks the pass touches. Live ledger: 65 rewrites landed a
/// placeholder at message index 2 of a long history and re-billed 17.1M tokens
/// between them, while a rewrite of the same size near the tail costs a
/// rounding error.
///
/// A gate that compares the gain against the whole context (as this one did)
/// cannot tell those apart, so it prices every firing as if it were the
/// expensive one — and declines cheap tail trims that would have relieved the
/// pressure for almost nothing, leaving the session to reach the far more
/// expensive full compaction instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MicrocompactQuote {
    /// Tokens this firing would remove from every subsequent request.
    pub clearable_tokens: u64,
    /// Tokens the next request would have to re-read at full price: the
    /// transcript from the earliest rewritten message to the end. Zero when
    /// there is nothing to rewrite.
    pub rebilled_tokens: u64,
    /// Index of the earliest message the firing would rewrite.
    pub frontier: Option<usize>,
}

impl MicrocompactQuote {
    /// Whether the firing pays for itself within `payback_requests` more
    /// requests. A trim that rewrites nothing (`frontier` is `None`) costs no
    /// invalidation, so it is always worth running.
    #[must_use]
    pub fn pays_back_within(self, payback_requests: u64) -> bool {
        self.clearable_tokens
            .saturating_mul(payback_requests)
            .max(u64::from(self.frontier.is_none()) * u64::MAX)
            >= self.rebilled_tokens
    }
}

/// Price a microcompact firing on `session` without mutating anything.
#[must_use]
pub fn microcompact_quote(
    session: &Session,
    keep_recent: usize,
    min_output_bytes: usize,
) -> MicrocompactQuote {
    let plan = plan_microcompact_clears(session, keep_recent, min_output_bytes);
    let frontier = mutation_frontier(&plan.tool_results, &plan.images).or_else(|| {
        plan.superseded_reminders
            .iter()
            .map(|&(message_index, _)| message_index)
            .min()
    });
    let rebilled = frontier.map_or(0, |frontier| {
        session.messages[frontier..]
            .iter()
            .map(estimated_message_tokens)
            .sum()
    });
    MicrocompactQuote {
        clearable_tokens: plan_estimated_tokens(session, &plan),
        rebilled_tokens: rebilled,
        frontier,
    }
}

/// Rough token size of one stored message, in the repo's `chars / 4` unit.
fn estimated_message_tokens(message: &ConversationMessage) -> u64 {
    message
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.len() as u64 / 4,
            ContentBlock::Thinking { thinking, .. } => thinking.len() as u64 / 4,
            ContentBlock::RedactedThinking { data } => data.len() as u64 / 4,
            ContentBlock::ToolUse { input, .. } => input.len() as u64 / 4,
            ContentBlock::ToolResult { output, images, .. } => {
                output.len() as u64 / 4 + images.len() as u64 * 1_600
            }
            // Same 1,600-token figure `plan_estimated_tokens` prices an image
            // at, so the gain and the cost are quoted in one unit.
            ContentBlock::Image { .. } => 1_600,
        })
        .sum()
}

/// Tier-1 context trim (Claude Code "microcompact" parity): replace the
/// bodies of OLD tool results with a short placeholder, keeping the most
/// recent `keep_recent` results and every `tool_use` block intact — the model
/// still sees that it made each call (and with what input), only the bulky
/// response body is gone. No LLM round-trip, no message removal; strictly
/// cheaper than full compaction, and idempotent (a cleared result is never
/// counted as clearable again).
///
/// Results whose body is already small (`< min_output_bytes`) are left alone
/// unless they carry images — clearing a tiny body saves nothing and costs
/// information, while each image is ~1,600 estimated tokens. Results from
/// file-mutation tools (`edit_file`/`write_file` and friends, per
/// [`is_edit_result_tool`]) are *always* exempt: their body is the applied
/// diff, which — unlike a re-readable `read_file` — cannot be reconstructed and
/// is the only in-context evidence that the change was already made.
///
/// Candidate selection is shared with [`microcompact_clearable_estimate`] via
/// `plan_microcompact_clears` (never duplicated), so callers that gate on
/// the estimate before calling this are guaranteed to get exactly the batch
/// they were quoted.
///
/// The superseded-reminder pass rides along rather than leading: it only
/// reclaims copies at or after `mutation_frontier`, the earliest message the
/// tool-result/image passes are already rewriting, because that index — not the
/// number of blocks touched — is what a firing costs. The exception is a pass
/// that clears no tool result and no image at all: with no frontier there is no
/// free region to ride on, so the reminders lead and the whole transcript is in
/// scope. See `plan_superseded_reminders` for the trade-off that leaves behind
/// and for why that case is still worth running.
///
/// Not purely in-memory: every message whose tool-result body or standalone image
/// this pass is about to destroy is first sealed to the session's Raw Vault by
/// `seal_microcompact_originals`. Sealing is necessary but NOT sufficient for
/// that part of the trim to be reversible: the seal only survives if the eviction
/// path heals the message before re-sealing the same seq, which
/// `restore_microcompacted_bodies` does for both kinds — by `tool_use_id` for a
/// body, positionally-and-verified for an image. A message the pass touches only
/// to clear a superseded reminder is deliberately NOT sealed, and that clear is
/// therefore NOT reversible — see that function's SCOPE note, which states what
/// the recall lineage gives up for it. That
/// costs one append per firing (no-op for an unpersisted session or with
/// `ZO_DISABLE_RAW_VAULT`), and it is what makes it safe for this pass to end by
/// marking the transcript dirty — which publishes the cleared bodies to disk.
/// A seal that FAILS to write returns `None` without touching the transcript:
/// this pass never destroys a body whose original is unrecoverable, and the
/// declined round is indistinguishable to callers from "nothing to clear".
pub fn microcompact_session(
    session: &mut Session,
    keep_recent: usize,
    min_output_bytes: usize,
) -> Option<MicrocompactEvent> {
    let plan = plan_microcompact_clears(session, keep_recent, min_output_bytes);
    if plan.tool_results.is_empty() && plan.images.is_empty() && plan.superseded_reminders.is_empty()
    {
        return None;
    }
    let estimated_tokens_saved = plan_estimated_tokens(session, &plan);

    // Seal the pre-clear originals BEFORE the mutation below destroys them. This
    // is the only moment the raw bodies are guaranteed to exist: the clear is
    // persisted one turn later (`mark_transcript_dirty` at the end of this
    // function), which is what used to leave full compaction with nothing but
    // placeholders to seal. Deliberately placed after the empty-plan early
    // return, so a no-op pass appends nothing and re-running the clear cannot
    // double-seal.
    //
    // A FAILED append aborts the trim: the bodies below are irrecoverable once
    // overwritten (the disk fallback reads a snapshot this pass is about to
    // poison), so the only safe response to "the copy did not land" is to keep
    // the original. This cannot wedge a session that needs to shrink — the trim
    // is not the tier that has to succeed. `maybe_microcompact*` treats `None`
    // as "did not trim" and the caller falls straight through to
    // `maybe_auto_compact*` on the same iteration
    // (`auto_compaction_config_if_preflight_ready` on iteration 1,
    // `auto_compaction_config_if_ready` from iteration 2 and at the post-turn
    // seam — neither predicate reads the trim's outcome, only token
    // thresholds); full compaction's own seal is deliberately allowed to
    // proceed through a write failure because it is the only tier that can
    // shrink a transcript. And a transient failure costs one round, not the
    // session: the plan is recomputed from scratch next request.
    if !seal_microcompact_originals(session, &plan) {
        return None;
    }

    let messages = std::sync::Arc::make_mut(&mut session.messages);
    for &(message_index, block_index) in &plan.tool_results {
        if let ContentBlock::ToolResult { output, images, .. } =
            &mut messages[message_index].blocks[block_index]
        {
            *output = MICROCOMPACT_PLACEHOLDER.to_string();
            images.clear();
        }
    }
    for &(message_index, block_index) in &plan.images {
        let block = &mut messages[message_index].blocks[block_index];
        if matches!(block, ContentBlock::Image { .. }) {
            *block = ContentBlock::Text {
                text: MICROCOMPACT_IMAGE_PLACEHOLDER.to_string(),
            };
        }
    }
    for &(message_index, block_index) in &plan.superseded_reminders {
        if let ContentBlock::Text { text } = &mut messages[message_index].blocks[block_index] {
            *text = MICROCOMPACT_REMINDER_PLACEHOLDER.to_string();
        }
    }
    session.mark_transcript_dirty();
    Some(MicrocompactEvent {
        cleared_results: plan.tool_results.len(),
        cleared_images: plan.images.len(),
        cleared_superseded_reminders: plan.superseded_reminders.len(),
        estimated_tokens_saved,
    })
}

fn extract_tag_block(content: &str, tag: &str) -> Option<String> {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    let start_index = content.find(&start)? + start.len();
    let end_index = content[start_index..].find(&end)? + start_index;
    Some(content[start_index..end_index].to_string())
}

fn strip_tag_block(content: &str, tag: &str) -> String {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    if let (Some(start_index), Some(end_index_rel)) = (content.find(&start), content.find(&end)) {
        let end_index = end_index_rel + end.len();
        let mut stripped = String::new();
        stripped.push_str(&content[..start_index]);
        stripped.push_str(&content[end_index..]);
        stripped
    } else {
        content.to_string()
    }
}

fn collapse_blank_lines(content: &str) -> String {
    let mut result = String::new();
    let mut last_blank = false;
    for line in content.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && last_blank {
            continue;
        }
        result.push_str(line);
        result.push('\n');
        last_blank = is_blank;
    }
    result
}

fn extract_existing_compacted_summary(message: &ConversationMessage) -> Option<String> {
    if message.role != MessageRole::System {
        return None;
    }

    let text = first_text_block(message)?;
    let summary = text.strip_prefix(COMPACT_CONTINUATION_PREAMBLE)?;
    let summary = summary
        .split_once(&format!("\n\n{COMPACT_RECENT_MESSAGES_NOTE}"))
        .map_or(summary, |(value, _)| value);
    let summary = summary
        .split_once(&format!("\n{COMPACT_DIRECT_RESUME_INSTRUCTION}"))
        .map_or(summary, |(value, _)| value);
    Some(summary.trim().to_string())
}

#[cfg(test)]
mod tests;
