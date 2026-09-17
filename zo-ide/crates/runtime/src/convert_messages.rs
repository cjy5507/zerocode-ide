//! Single source of truth for lowering stored [`ConversationMessage`]s into the
//! provider-facing [`InputMessage`] wire form.
//!
//! Both the foreground conversation and sub-agent provider clients resend their
//! full history every iteration, so this conversion is on the hottest seam and
//! had drifted into two near-identical copies. Keeping it here — beside the
//! [`crate::context_compression::wire_tool_output`] it depends on — collapses
//! that drift surface to one place.

use api::{ImageSource, InputContentBlock, InputMessage, ToolResultContentBlock};

use crate::image_guard::{guard_wire_image_base64, oversized_placeholder, WireImageOutcome};
use crate::{ContentBlock, ConversationMessage, MessageRole};

/// Lower stored conversation messages into provider wire messages.
///
/// Maps every block variant, applies the shared model-facing structural
/// compression to tool output (the stored session block keeps the original, so
/// the TUI, persistence, and reversibility are unaffected), carries each turn's
/// Gemini thought signature through, and drops messages that lower to no content.
///
/// Consecutive [`MessageRole::Tool`] messages coalesce into ONE wire `user`
/// message. A parallel tool batch is stored as one result message per tool,
/// but the Anthropic API requires every `tool_use` id of an assistant turn to
/// be answered in *the single next* message — as separate messages the request
/// only survived because the API happens to merge same-role neighbours, and
/// anything ever slotted between two results 400s the whole turn
/// (`tool_use ids were found without tool_result blocks immediately after`).
/// Enforcing the invariant here, at the lowering SSOT, protects every
/// provider and every injection feature above it. Per-block encoders
/// (`openai_compat` emits one `role:"tool"` message per `ToolResult` block)
/// are unaffected by the grouping.
/// Index of the first message in the newest unbroken run of tool results.
///
/// Parallel tool calls come back as several adjacent `Tool` messages, and all of
/// them belong to the turn that just ran — so freshness is a property of the
/// trailing run, not of the single last message. Returns `messages.len()` when
/// the conversation does not end in tool results, which leaves every historical
/// result eligible for summarization.
///
/// `System` messages are transparent to the scan: the runtime persists the
/// per-request reminder set as a trailing `System` message (see
/// [`crate::ConversationRuntime::absorb_wire_reminders_into_session`]), so the
/// stored tail of an in-flight turn is `[assistant, tool.., reminder]`. Without
/// the skip, that reminder would end the trailing run at length zero and the
/// tool results the model asked for *this turn* would be handed back elided.
fn newest_tool_run_start(messages: &[ConversationMessage]) -> usize {
    let mut start = messages.len();
    for (index, message) in messages.iter().enumerate().rev() {
        if message.role == MessageRole::System {
            continue;
        }
        if message.role != MessageRole::Tool {
            break;
        }
        start = index;
    }
    start
}

/// Whether the target provider can replay a stored reasoning block natively.
///
/// Only Anthropic verifies and accepts a signed thinking block; every other
/// encoder matches the variant and drops it. That drop used to be the end of
/// the story, which meant switching providers mid-session silently deleted
/// every earlier turn's reasoning from what the next model was shown — the
/// text and the tool calls survived, the *why* did not. See
/// `reasoning_passport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningReplay {
    /// Signed blocks ride the wire verbatim (Anthropic).
    Native,
    /// The target cannot verify a signature, so reasoning is carried as text.
    AsText,
}

impl ReasoningReplay {
    /// What the model this request is going to can do with stored reasoning.
    #[must_use]
    pub fn for_model(model: &str) -> Self {
        if api::detect_provider_kind(model) == api::ProviderKind::Anthropic {
            Self::Native
        } else {
            Self::AsText
        }
    }
}

/// Per-block budget for carried reasoning, in characters (~250 tokens).
///
/// The cap is per block and EVERY unreplayable block is carried, rather than
/// a sliding window of the last few. A window would be cheaper, but a block
/// leaving it as the conversation grows would rewrite history the cache had
/// already stored — the exact defect class this module spent a round fixing.
/// A deterministic per-block digest lowers to the same bytes on every request
/// of the session.
const REASONING_PASSPORT_CHARS: usize = 1_000;

/// Label that opens a carried reasoning block on the wire.
pub const REASONING_PASSPORT_LABEL: &str = "[earlier reasoning]";

/// Whether assistant text is a wire-only carried-reasoning passport.
///
/// Replay surfaces use this at their display boundary so provider-switch
/// context remains on the wire without becoming a visible assistant answer.
#[must_use]
pub fn is_reasoning_passport_text(text: &str) -> bool {
    text.starts_with(REASONING_PASSPORT_LABEL)
}

/// Assistant text with a leading passport label taken off — the display
/// boundary's answer to a model that IMITATES the label. A passport never
/// enters the stored transcript (it is minted for the wire), so text that
/// starts with the label there is the model's own reply, written in the
/// shape of the carried thought it just read ("[earlier reasoning]\n…", seen
/// live from gpt-5.6 after a handoff from claude-opus-5, 2026-09-02). The
/// reply is kept; only the label line goes, so nothing the model said is
/// lost on screen or in `--last-message`.
#[must_use]
pub fn strip_reasoning_passport_label(text: &str) -> &str {
    text.strip_prefix(REASONING_PASSPORT_LABEL)
        .map_or(text, |rest| rest.trim_start_matches(['\n', '\r']))
}

/// Carry a reasoning block the target provider cannot replay, as text.
///
/// Deliberately NOT a thinking block: it is attributed, plain assistant text,
/// so no provider can reject it as an unverified signature. The full original
/// stays in the transcript vault, reachable with `session_recall`, so the cap
/// costs recall effort rather than the record itself.
///
/// `None` for reasoning that carries nothing (empty, or whitespace only).
fn reasoning_passport(thinking: &str) -> Option<String> {
    let trimmed = thinking.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= REASONING_PASSPORT_CHARS {
        return Some(format!("{REASONING_PASSPORT_LABEL}\n{trimmed}"));
    }
    // Cut on a line boundary when one is near the cap, so the carried text
    // ends on a thought rather than mid-word.
    let head: String = trimmed.chars().take(REASONING_PASSPORT_CHARS).collect();
    let cut = head
        .rfind('\n')
        .filter(|boundary| *boundary * 4 > REASONING_PASSPORT_CHARS * 3)
        .unwrap_or(head.len());
    Some(format!(
        "{REASONING_PASSPORT_LABEL}\n{}\n[… truncated; the full reasoning is in this session's \
         transcript, reachable with session_recall]",
        &head[..cut]
    ))
}

/// The note left in the transcript at a model switch.
///
/// A switch is invisible from inside the conversation: the next model reads a
/// history in its own voice and, before the passport above, one with the
/// reasoning silently removed. Two failure modes follow from that — it reads
/// the earlier turns as its own work, and it re-litigates decisions that were
/// already made and paid for. Both get worse the longer the session runs,
/// which is exactly where a model switch is most likely.
///
/// This is a message, not a reminder: it is pushed into the transcript once,
/// at the position where the switch happened, so it is append-only (the cache
/// behind it is untouched), it survives a resume, and it never repeats.
///
/// `None` when nothing changed hands.
#[must_use]
pub fn model_handoff_notice(previous: &str, next: &str) -> Option<String> {
    if previous == next {
        return None;
    }
    let mut note = format!(
        "Model handoff: everything above this line was produced by `{previous}`. \
         You are `{next}`. Treat that work as a colleague's — the decisions in it \
         stand unless you find a concrete reason to revisit one, and re-deriving \
         them costs the session twice."
    );
    // Only say the reasoning changed shape when it actually did. Claiming it on
    // a same-provider swap would send the model looking for markers that are
    // not there.
    if ReasoningReplay::for_model(previous) != ReasoningReplay::for_model(next) {
        use std::fmt::Write as _;
        let _ = write!(
            note,
            " The earlier model's reasoning could not be replayed to you natively, so it \
             appears as `{REASONING_PASSPORT_LABEL}` text and may be truncated; \
             `session_recall` has the full record. Never write that label yourself: it \
             marks carried thought, not a reply, and your own messages are plain prose."
        );
    }
    Some(note)
}

/// Lower stored messages for a target that replays reasoning natively.
///
/// Kept for callers with no model in hand (tests, measurement harnesses).
/// Production paths use [`convert_messages_for`] so a provider switch does not
/// silently drop the reasoning behind the conversation.
#[must_use]
pub fn convert_messages(messages: &[ConversationMessage]) -> Vec<InputMessage> {
    convert_messages_for(messages, ReasoningReplay::Native)
}

#[must_use]
pub fn convert_messages_for(
    messages: &[ConversationMessage],
    reasoning: ReasoningReplay,
) -> Vec<InputMessage> {
    let mut out: Vec<InputMessage> = Vec::with_capacity(messages.len());
    // Whether the newest message in `out` lowered from `MessageRole::Tool` —
    // messages that lower to no content are invisible on the wire and keep
    // the adjacency alive.
    let mut tail_is_tool_run = false;
    // The newest tool results are the ones the model just asked for, so they
    // reach the wire without any elision. Anything older may be summarized to
    // protect the context budget — by then the text has already done its job.
    let newest_tool_run_start = newest_tool_run_start(messages);
    for (index, message) in messages.iter().enumerate() {
        let role = match message.role {
            MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
            MessageRole::Assistant => "assistant",
        };
        let rewrite = if index >= newest_tool_run_start {
            crate::context_compression::WireRewrite::Lossless
        } else {
            crate::context_compression::WireRewrite::Full
        };
        let content = convert_blocks(message, rewrite, reasoning);
        if content.is_empty() {
            continue;
        }
        let is_tool = message.role == MessageRole::Tool;
        if is_tool && tail_is_tool_run {
            if let Some(previous) = out.last_mut() {
                previous.content.extend(content);
                continue;
            }
        }
        out.push(InputMessage {
            role: role.to_string(),
            content,
            // Carry the turn's Gemini thought signature through. Harmless for
            // other providers: their encoders ignore `thought_signature`, and
            // it is `serde(skip)` so it never reaches any wire but Gemini's.
            thought_signature: message.thought_signature.clone(),
            // Carry the turn's ChatGPT reasoning-replay payload through, same
            // isolation as `thought_signature` above: only the ChatGPT
            // encoder reads it.
            reasoning_replay: message.reasoning_replay.clone(),
        });
        tail_is_tool_run = is_tool;
    }
    for message in &mut out {
        enforce_tool_results_lead(message);
    }
    out
}

/// Re-order a lowered `user` message so its `tool_result` blocks lead.
///
/// The Anthropic validator only credits results at the head of the message:
/// any other block ahead of a `tool_result` hides every result behind it and
/// the turn 400s (`tool_use ids were found without tool_result blocks
/// immediately after`, naming exactly the hidden ids). Text can legitimately
/// land amid a coalesced result run — `reconcile_tool_history` rewrites a
/// no-longer-advertised tool's result (e.g. a deferred builtin on a fresh
/// resume) into a text block in place, mid-batch. The partition is stable, so
/// both the result order and the narrative order are preserved.
fn enforce_tool_results_lead(message: &mut InputMessage) {
    if message.role != "user" {
        return;
    }
    let mut seen_other = false;
    let misplaced = message.content.iter().any(|block| {
        let is_result = matches!(block, InputContentBlock::ToolResult { .. });
        let out_of_place = is_result && seen_other;
        seen_other |= !is_result;
        out_of_place
    });
    if !misplaced {
        return;
    }
    let (mut results, rest): (Vec<_>, Vec<_>) = message
        .content
        .drain(..)
        .partition(|block| matches!(block, InputContentBlock::ToolResult { .. }));
    results.extend(rest);
    message.content = results;
}

/// Lower one stored message's blocks into wire content blocks.
///
/// Returns `Option` per block so a block that lowers to nothing (an empty
/// reasoning block, an opaque redacted one the target cannot verify) is
/// omitted rather than sent malformed. Block order is otherwise preserved, so
/// replayed thinking still leads the assistant turn.
fn convert_blocks(
    message: &ConversationMessage,
    rewrite: crate::context_compression::WireRewrite,
    reasoning: ReasoningReplay,
) -> Vec<InputContentBlock> {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(InputContentBlock::Text {
                        text: text.clone(),
                        cache_control: None,
                    }),
                    // Reasoning has two ways onto the wire, and exactly one of
                    // them is available per target.
                    //
                    // Signed + Anthropic: re-send VERBATIM so interleaved
                    // thinking keeps its continuity across tool calls. The
                    // signature is the whole point — Anthropic 400s on a
                    // modified or unsigned thinking block.
                    //
                    // Everything else — a different provider, or a block this
                    // session's earlier (non-Anthropic) model produced with no
                    // signature — is carried as attributed TEXT. It used to be
                    // dropped, which is why switching models mid-session
                    // handed the next model a conversation with every "why"
                    // removed while the "what" stayed. Text cannot be rejected
                    // as an unverified signature, and it is what the receiving
                    // model can actually read.
                    ContentBlock::Thinking { thinking, signature } => {
                        if reasoning == ReasoningReplay::Native && !signature.is_empty() {
                            Some(InputContentBlock::Thinking {
                                thinking: thinking.clone(),
                                signature: signature.clone(),
                            })
                        } else {
                            reasoning_passport(thinking).map(|text| InputContentBlock::Text {
                                text,
                                cache_control: None,
                            })
                        }
                    }
                    // Redacted reasoning is an opaque blob: only the provider
                    // that sealed it can read it, so there is nothing to carry
                    // anywhere else. Replayed natively, omitted otherwise.
                    ContentBlock::RedactedThinking { data } => {
                        (reasoning == ReasoningReplay::Native)
                            .then(|| InputContentBlock::RedactedThinking { data: data.clone() })
                    }
                    ContentBlock::ToolUse { id, name, input } => Some(InputContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: serde_json::from_str(input)
                            .unwrap_or_else(|_| serde_json::json!({ "raw": input })),
                        cache_control: None,
                    }),
                    ContentBlock::ToolResult {
                        tool_use_id,
                        tool_name,
                        output,
                        is_error,
                        images,
                    } => {
                        // Model-facing structural compression (lossless unwrap
                        // / grouping; outline only for oversized code files).
                        // The session block keeps the original output, so the
                        // TUI, persistence, and reversibility are unaffected.
                        let mut content = vec![ToolResultContentBlock::Text {
                            text: crate::context_compression::wire_tool_output(
                                output, tool_name, *is_error, rewrite,
                            ),
                        }];
                        // Dimension-guard every stored image on the way to the
                        // wire (see `image_guard`): an oversized screenshot
                        // baked into history otherwise 400s every turn. A drop
                        // degrades to a text placeholder so the model still
                        // learns an image was present.
                        content.extend(images.iter().map(|(media_type, data)| {
                            match guard_image_source(media_type, data) {
                                Ok(source) => ToolResultContentBlock::Image { source },
                                Err(placeholder) => {
                                    ToolResultContentBlock::Text { text: placeholder }
                                }
                            }
                        }));
                        Some(InputContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content,
                            is_error: *is_error,
                            cache_control: None,
                        })
                    }
                    ContentBlock::Image { media_type, data } => {
                        Some(match guard_image_source(media_type, data) {
                            Ok(source) => InputContentBlock::Image {
                                source,
                                cache_control: None,
                            },
                            Err(placeholder) => InputContentBlock::Text {
                                text: placeholder,
                                cache_control: None,
                            },
                        })
                    }
                })
        .collect::<Vec<_>>()
}

/// Dimension-guard one stored `(media_type, base64)` image on the way to the
/// provider wire. Returns an `ImageSource` for a kept or downscaled image, or a
/// text placeholder (`Err`) when a confirmed-oversized image could not be
/// downscaled — in which case the caller substitutes a text block so the model
/// still sees that an image was present. Shared by the `ToolResult` image list
/// and standalone `Image` arms, which lower into different wire enums.
fn guard_image_source(media_type: &str, data: &str) -> Result<ImageSource, String> {
    match guard_wire_image_base64(data) {
        WireImageOutcome::Keep => Ok(ImageSource {
            kind: "base64".to_string(),
            media_type: media_type.to_string(),
            data: data.to_string(),
        }),
        WireImageOutcome::Rescaled {
            media_type,
            data_b64,
        } => Ok(ImageSource {
            kind: "base64".to_string(),
            media_type,
            data: data_b64,
        }),
        WireImageOutcome::Drop { width, height } => Err(oversized_placeholder(width, height)),
    }
}

/// Append harness reminders (recalled memory, todo progress, `TeamInbox` digest,
/// …) to the newest `user`-role wire message as trailing text blocks.
///
/// Message-level injection — rather than a trailing system-prompt section — is
/// what keeps the Anthropic prefix cache alive: the cache hierarchy is
/// tools → system → messages, so a system block that changes (`system_changed`)
/// invalidates every message breakpoint behind it, re-billing the entire
/// history at cache-write price each time recall or a reminder refreshes.
/// Riding the newest user message instead re-bills only that message.
///
/// Each reminder is wrapped in `<system-reminder>` tags (the static prompt
/// tells the model these carry system information) unless the producer already
/// embedded its own tags, e.g. the `UserPromptSubmit` hook reminder — wrapping
/// those again would nest tags.
///
/// Trailing position also satisfies the API rule that `tool_result` blocks
/// lead their user message.
///
/// **Legacy wire-only seam.** The conversation runtime no longer routes its
/// per-turn reminders through here: mutating the newest user message meant the
/// *previous* request's newest message re-rendered without its reminder tail on
/// the next request, so every request re-billed the prior tail as uncached
/// input (91% of measured Anthropic cache writes were such re-writes). The
/// runtime now persists the reminder set into the transcript instead
/// (`crate::ConversationRuntime::absorb_wire_reminders_into_session`), which
/// keeps every historical render byte-identical. This function remains for
/// callers that assemble an [`crate::conversation::ApiRequest`] by hand with
/// explicit `wire_reminders`; on the runtime path the slice is empty and this
/// is a no-op.
pub fn append_wire_reminders(messages: &mut Vec<InputMessage>, reminders: &[String]) {
    let mut blocks: Vec<InputContentBlock> = reminders
        .iter()
        .filter(|reminder| !reminder.trim().is_empty())
        .map(|reminder| InputContentBlock::Text {
            text: wrap_reminder(reminder),
            cache_control: None,
        })
        .collect();
    if blocks.is_empty() {
        return;
    }
    if let Some(last_user) = messages
        .iter_mut()
        .rev()
        .find(|message| message.role == "user")
    {
        last_user.content.append(&mut blocks);
    } else {
        // First request of a session always carries a user message; this arm
        // only guards a pathological empty/assistant-tail history.
        messages.push(InputMessage {
            role: "user".to_string(),
            content: blocks,
            thought_signature: None,
            reasoning_replay: None,
        });
    }
}

/// Wrap one reminder in `<system-reminder>` tags unless the producer already
/// embedded its own tags (e.g. the `UserPromptSubmit` hook reminder — wrapping
/// those again would nest tags). Shared by the legacy wire-only injection
/// ([`append_wire_reminders`]) and the persisted-reminder path
/// (`crate::ConversationRuntime::absorb_wire_reminders_into_session`) so both
/// produce byte-identical text for the same reminder.
#[must_use]
pub fn wrap_reminder(reminder: &str) -> String {
    if reminder.contains(crate::session::REMINDER_TAG_OPEN) {
        reminder.to_string()
    } else {
        format!("<system-reminder>\n{reminder}\n</system-reminder>")
    }
}

/// Whether a persisted `System`-role text block carries a LIVE harness reminder
/// (as opposed to e.g. a compaction summary, which also rides `System`).
/// Reminder blocks always carry a [`crate::session::REMINDER_TAG_OPEN`] tag —
/// most as a leading wrapper from [`wrap_reminder`], but some producers embed
/// the tag after their own prefix line (e.g. the `UserPromptSubmit` hook
/// context), so this checks for containment rather than a prefix.
///
/// Deliberately narrower than [`crate::session::is_reminder_lineage_text`]:
/// this is the CLEARING question, so microcompact's placeholder must not match
/// it or a cleared block would be a candidate again on the next pass. Consumers
/// asking whether a block is reminder machinery — the TUI's resume seeder,
/// history surgery — want the lineage predicate instead.
#[must_use]
pub fn is_persisted_reminder_text(text: &str) -> bool {
    text.contains(crate::session::REMINDER_TAG_OPEN)
}

/// Place Anthropic prompt-cache breakpoints on the latest conversation prefix.
///
/// System blocks already consume up to two of Anthropic's four cache-control
/// slots. Marking the last two cacheable message blocks gives every multi-turn
/// caller — the foreground turn and sub-agent provider clients alike — a
/// rolling conversation prefix cache while staying within the provider limit.
/// The sub-agent path previously skipped this step entirely, so only its
/// system blocks ever cached and each iteration re-billed the full transcript
/// as uncached input.
///
/// Run this after [`append_wire_reminders`]. Persisted reminder-only messages
/// stay behind the newest breakpoint: their per-request text remains visible,
/// while marker placement is owned by durable conversation messages. Harmless
/// on non-Anthropic wires: the OpenAI and Gemini encoders build their own
/// payloads from these blocks and never serialize `cache_control`.
///
/// # Why the 5-minute TTL, on every caller
///
/// Both markers ride the conversation tail, which the *next* request overwrites
/// — so what they need to survive is one inter-request gap, not a working
/// session. The 1h write premium (2.0x vs 1.25x) is therefore paid on 100% of
/// writes to insure a gap distribution that almost never reaches it:
///
/// * 98.8% of consecutive requests land within 5 minutes (128,169 measured
///   gaps); only 1.2% fall in the 5m–1h window where 1h beats 5m at all.
/// * Priced against this session's own ledger (write 8,334/req, conversation
///   read ~35,577/req), 5m nets **+5,760 token-equivalents per request**.
///   It would take a 15.3% expiry rate to break even — 13x the measured one.
/// * The 1h TTL was not buying the coverage it promised regardless: of the
///   evictions this ledger recorded *while running at 1h*, 71.5% happened
///   within 5 minutes (provider-side eviction, which no TTL prevents) and
///   15.8% after 1h (which 1h does not cover either).
///
/// The sub-agent path reached this conclusion first, for the narrower reason
/// that its iterations land seconds apart; the gap measurement showed the same
/// answer holds for the foreground turn, so the two paths share one function.
///
/// The system prefix keeps its 1h breakpoints (`push_cache_block`,
/// `agent_system_blocks`) — it is read across *sessions*, where hours-long
/// gaps are the norm — and sitting ahead of these markers it also satisfies
/// Anthropic's longer-TTL-before-shorter ordering rule.
///
/// `api::PromptCacheConfig::prompt_ttl` mirrors this choice for break
/// classification; the two must move together or legitimate expiries get
/// logged as unexpected breaks. `breakpoint_ttl_matches_break_classifier`
/// pins that pairing so it cannot drift silently.
///
/// # The anchor may outlive the tail (2026-09-16)
///
/// The r21 arithmetic priced ONE marker's lifetime against every write. The
/// two markers do different jobs, though: the rolling marker insures the
/// tail the next request overwrites anyway, while the ANCHOR — the previous
/// request's tail, i.e. the whole conversation up to this turn — is what a
/// person's coffee break must not evict. This machine's request ledger
/// (2,789 requests, 114 breaks) put the cost of the 5-minute anchor in
/// numbers: TTL expiries were 29 breaks but 6.2M of the 13.1M tokens every
/// break together re-billed (47%), 27 of 34 all-time expiries fell inside an
/// hour, and the anchor moved on only 2.2% of requests (median 41k tokens
/// of context when it did). So a 1h anchor pays its premium on 2.2% of
/// requests and saves a whole-prefix rewrite on the 1.7% that follow a
/// 5-to-60-minute gap — roughly five to one in its favour, against r21's
/// blanket-1h case that lost three to one. Provider-side, 1h-before-5m is
/// the ordering Anthropic requires, and a hit at the tail refreshes both.
///
/// It is an opt-in ([`declare_conversation_anchor_ttl`], from the host's
/// `cache.anchorTtl` setting) until the ledger has judged it: the `ttl`
/// column of every request row records which policy it ran under, so the
/// soak reads straight off the ledger. The rolling marker keeps the 5-minute
/// TTL under every policy, which is why the break classifier's window does
/// not move with the anchor: a 5-to-60-minute gap under a 1h anchor now
/// costs the tail alone, and a longer gap is a TTL expiry under both.
///
/// Sub-agents never take the long anchor
/// ([`mark_conversation_cache_breakpoints_short_lived`]): a spawn's tail dies
/// within minutes, so the premium would buy nothing.
pub fn mark_conversation_cache_breakpoints(messages: &mut [InputMessage]) {
    mark_breakpoints_with_ttl(
        messages,
        &conversation_anchor_ttl().cache_control(),
        &api::CacheControl::ephemeral(),
    );
}

/// The sub-agent variant: both markers at the 5-minute TTL regardless of the
/// declared anchor policy — a spawn's requests land seconds apart and its
/// tail dies within minutes, so a longer anchor would pay the write premium
/// and never be read back across a gap.
pub fn mark_conversation_cache_breakpoints_short_lived(messages: &mut [InputMessage]) {
    let short = api::CacheControl::ephemeral();
    mark_breakpoints_with_ttl(messages, &short, &short);
}

/// How long the ANCHOR conversation marker asks the provider to keep the
/// prefix it stands on. The rolling marker always asks for five minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConversationAnchorTtl {
    /// r21's choice: one gap's worth, the cheapest write.
    #[default]
    FiveMinutes,
    /// The opt-in under soak: the prefix survives a person's break.
    OneHour,
}

impl ConversationAnchorTtl {
    /// Both policies, the one place the set is enumerated.
    pub const ALL: [ConversationAnchorTtl; 2] = [Self::FiveMinutes, Self::OneHour];

    /// The label a setting spells (`5m` / `1h`) — the same words the
    /// provider's `ttl` field and the ledger's `ttl` column use.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneHour => "1h",
        }
    }

    /// The policy a label names, `None` for anything else.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|ttl| ttl.label().eq_ignore_ascii_case(label.trim()))
    }

    /// The marker this policy puts on the anchor.
    #[must_use]
    pub fn cache_control(self) -> api::CacheControl {
        match self {
            Self::FiveMinutes => api::CacheControl::ephemeral(),
            Self::OneHour => api::CacheControl::ephemeral_1h(),
        }
    }
}

/// The declared anchor policy for this process; zero is the default.
static CONVERSATION_ANCHOR_TTL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Declare the anchor marker's TTL for this process. The plain host calls
/// this on the way into every turn from its settings, like
/// [`crate::declare_attendance`]; sub-agent clients ignore it by
/// construction (they call the short-lived marking).
pub fn declare_conversation_anchor_ttl(ttl: ConversationAnchorTtl) {
    CONVERSATION_ANCHOR_TTL.store(
        match ttl {
            ConversationAnchorTtl::FiveMinutes => 0,
            ConversationAnchorTtl::OneHour => 1,
        },
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The anchor policy the host declared, or five minutes when none did.
#[must_use]
pub fn conversation_anchor_ttl() -> ConversationAnchorTtl {
    match CONVERSATION_ANCHOR_TTL.load(std::sync::atomic::Ordering::Relaxed) {
        1 => ConversationAnchorTtl::OneHour,
        _ => ConversationAnchorTtl::FiveMinutes,
    }
}

fn mark_breakpoints_with_ttl(
    messages: &mut [InputMessage],
    anchor_ttl: &api::CacheControl,
    rolling_ttl: &api::CacheControl,
) {
    const MAX_MESSAGE_CACHE_BREAKPOINTS: usize = 2;

    let mut marked = 0;
    // VERIFY runs against an isolated packet, then its raw transcript is
    // spliced back for persistence, resume, and transcript inspection. Pin one
    // of the existing message slots immediately before the newest VERIFY leg;
    // otherwise both rolling slots can land deep inside a long appended leg.
    // This makes the unchanged prefix eligible for direct reuse, but actual
    // cache hits still depend on provider behavior.
    let verify_boundary = messages
        .iter()
        .rposition(is_deep_verify_prompt)
        .and_then(|verify_start| {
            (0..verify_start)
                .rev()
                .find(|&index| is_cache_marker_eligible(&messages[index]))
        });
    // The tail marker rolls; slot one is an ANCHOR, not a second rolling
    // marker (see [`previous_request_tail`]). Without it both markers sit on
    // content this request invented, so neither can ever be an exact hit.
    let rolling = messages
        .iter()
        .rposition(is_cache_marker_eligible);
    let anchor = verify_boundary.or_else(|| previous_request_tail(messages, rolling?));
    if let Some(index) = anchor {
        if mark_last_cacheable_block(&mut messages[index].content, anchor_ttl) {
            marked += 1;
        }
    }

    for (index, message) in messages.iter_mut().enumerate().rev() {
        if marked == MAX_MESSAGE_CACHE_BREAKPOINTS {
            break;
        }
        if Some(index) == anchor {
            continue;
        }
        if !is_cache_marker_eligible(message) {
            continue;
        }
        if mark_last_cacheable_block(&mut message.content, rolling_ttl) {
            marked += 1;
        }
    }
}

/// The message the PREVIOUS request ended on — the newest position whose prefix
/// the provider has already written a cache entry for.
///
/// A cache entry exists only where a `cache_control` marker stood, and a read
/// happens at a marker (Anthropic scans a bounded window of earlier blocks, but
/// only a bounded one). Marking the last two messages therefore puts both
/// markers on content this request invented: neither prefix was ever cached, so
/// every hit had to come out of that lookback window, and a turn that appended
/// more blocks than the window holds lost the whole conversation prefix. That
/// is the single shape behind all 610 "provider-side miss" rows in the local
/// prompt-cache ledger — 64.8M cache-read tokens, uncorrelated with elapsed
/// time (77 of them inside ten seconds), so neither TTL nor eviction.
///
/// A request always ends on a user-role message: the user's prompt, or the tool
/// results answering the last assistant turn. So the predecessor's tail is the
/// second-to-last user-role message here, and marking it asks for a prefix the
/// provider wrote itself one turn ago. The tail marker still rolls, writing the
/// entry the NEXT request will anchor on.
///
/// Found as the newest durable user-role message strictly behind `rolling`, the
/// slot the tail marker takes. Persisted `System` reminders also lower to wire
/// `user`, but are transparent here: otherwise a changed reminder becomes the
/// request tail and displaces the cache entry it was meant to follow. In the
/// agentic shape `rolling` IS the newest user message and this is the one before
/// it; behind an assistant tail it is the user message that assistant turn is
/// answering — which is the predecessor's tail in both cases, where
/// "second-to-last user message" would overshoot.
///
/// `None` on the first request of a session (one user message, no
/// predecessor), which then keeps the plain two-from-the-tail placement.
fn previous_request_tail(messages: &[InputMessage], rolling: usize) -> Option<usize> {
    messages[..rolling]
        .iter()
        .rposition(|message| message.role == "user" && is_cache_marker_eligible(message))
}

fn is_deep_verify_prompt(message: &InputMessage) -> bool {
    message.role == "user"
        && message.content.iter().any(|block| {
            matches!(
                block,
                InputContentBlock::Text { text, .. }
                    if text.trim_start().starts_with("[deep:VERIFY]")
                        || text.trim_start().starts_with("[[ZO-DEEP:VERIFY]]")
            )
        })
}

fn has_cacheable_block(blocks: &[InputContentBlock]) -> bool {
    blocks.iter().any(|block| match block {
        InputContentBlock::Text { text, .. } => !text.trim().is_empty(),
        InputContentBlock::Image { .. }
        | InputContentBlock::Document { .. }
        | InputContentBlock::ToolUse { .. }
        | InputContentBlock::ToolResult { .. } => true,
        InputContentBlock::Thinking { .. } | InputContentBlock::RedactedThinking { .. } => false,
    })
}

/// Whether a wire message may own an anchor or rolling cache breakpoint.
///
/// Stored `System` reminders lower to wire role `user`, so role alone cannot
/// distinguish them from durable conversation. Their text shape remains
/// explicit, however, and an absorber-created reminder message contains only
/// such blocks. A legacy mixed user message (ordinary content plus appended
/// reminder blocks) remains eligible under that legacy path's existing marker
/// semantics.
fn is_cache_marker_eligible(message: &InputMessage) -> bool {
    has_cacheable_block(&message.content)
        && !message.content.iter().all(|block| {
            matches!(
                block,
                InputContentBlock::Text { text, .. } if is_persisted_reminder_text(text)
            )
        })
}

/// Mark one message's last markable block, returning whether a marker landed.
///
/// Tool blocks are markable: an agentic loop's newest messages are almost
/// always a pure `tool_use`/`tool_result` exchange, and skipping them (the
/// pre-fix shape, when the wire enum had no `cache_control` field on tool
/// variants) pinned the breakpoint at the last *text-bearing* message — every
/// iteration then re-billed the entire tool tail behind it as uncached input.
///
/// Two variants still fall through to the previous block: thinking blocks
/// take no `cache_control` at all, and Anthropic 400s on `cache_control` over
/// an *empty* text block (`cache_control cannot be set for empty text
/// blocks`) — a long session can end a message with a blank text block, e.g.
/// a trailing empty delta.
fn mark_last_cacheable_block(blocks: &mut [InputContentBlock], ttl: &api::CacheControl) -> bool {
    for block in blocks.iter_mut().rev() {
        match block {
            InputContentBlock::Text {
                text,
                cache_control,
            } => {
                if text.trim().is_empty() {
                    continue;
                }
                if cache_control.is_none() {
                    *cache_control = Some(ttl.clone());
                }
                return true;
            }
            InputContentBlock::Image { cache_control, .. }
            | InputContentBlock::Document { cache_control, .. }
            | InputContentBlock::ToolUse { cache_control, .. }
            | InputContentBlock::ToolResult { cache_control, .. } => {
                if cache_control.is_none() {
                    *cache_control = Some(ttl.clone());
                }
                return true;
            }
            InputContentBlock::Thinking { .. } | InputContentBlock::RedactedThinking { .. } => {}
        }
    }
    false
}

#[cfg(test)]
mod cache_marking_tests {
    use super::{convert_messages, mark_conversation_cache_breakpoints, mark_last_cacheable_block};
    use crate::{ContentBlock, ConversationMessage};
    use api::InputContentBlock;

    fn block_cache(block: &InputContentBlock) -> Option<&api::CacheControl> {
        match block {
            InputContentBlock::Text { cache_control, .. }
            | InputContentBlock::Image { cache_control, .. }
            | InputContentBlock::Document { cache_control, .. }
            | InputContentBlock::ToolUse { cache_control, .. }
            | InputContentBlock::ToolResult { cache_control, .. } => cache_control.as_ref(),
            InputContentBlock::Thinking { .. } | InputContentBlock::RedactedThinking { .. } => {
                None
            }
        }
    }

    /// One marker anchors on the previous request's tail, one rolls onto this
    /// one. The anchor is the whole point: a cache entry exists only where a
    /// marker stood, so a marker on the assistant turn in between — content
    /// invented this request — can never be an exact hit.
    #[test]
    fn one_marker_anchors_on_the_previous_request_tail_and_one_rolls() {
        let messages = vec![
            ConversationMessage::user_text("first prompt"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "second response".to_string(),
            }]),
            ConversationMessage::user_text("third prompt"),
        ];

        let mut converted = convert_messages(&messages);
        mark_conversation_cache_breakpoints(&mut converted);

        assert_eq!(
            block_cache(&converted[0].content[0]),
            Some(&api::CacheControl::ephemeral()),
            "the prompt the previous request ended on is the anchor"
        );
        assert!(
            block_cache(&converted[1].content[0]).is_none(),
            "the assistant turn in between never was a request tail"
        );
        assert_eq!(
            block_cache(&converted[2].content[0]),
            Some(&api::CacheControl::ephemeral()),
            "the rolling marker writes the entry the next request anchors on"
        );
    }

    /// The invariant the anchor exists for, stated across two turns: whatever
    /// the rolling marker wrote last request is exactly what this request asks
    /// the provider for.
    #[test]
    fn this_requests_anchor_is_the_entry_the_last_request_wrote() {
        let marked_indices = |messages: &[ConversationMessage]| {
            let mut converted = convert_messages(messages);
            mark_conversation_cache_breakpoints(&mut converted);
            converted
                .iter()
                .enumerate()
                .filter(|(_, message)| {
                    message.content.iter().any(|block| block_cache(block).is_some())
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        };
        let tool_use = |id: &str| ContentBlock::ToolUse {
            id: id.to_string(),
            name: "bash".to_string(),
            input: "{}".to_string(),
        };

        let earlier = vec![
            ConversationMessage::user_text("prompt"),
            ConversationMessage::assistant(vec![tool_use("tu-1")]),
            ConversationMessage::tool_result("tu-1", "bash", "one", false),
        ];
        let mut later = earlier.clone();
        later.push(ConversationMessage::assistant(vec![tool_use("tu-2")]));
        later.push(ConversationMessage::tool_result("tu-2", "bash", "two", false));

        let earlier_marks = marked_indices(&earlier);
        let later_marks = marked_indices(&later);

        assert_eq!(
            later_marks.first().copied(),
            earlier_marks.last().copied(),
            "the new anchor must be the position the previous request cached, \
             not fresh content the provider has never seen"
        );
        assert_eq!(later_marks.len(), 2, "still two message slots");
    }

    /// The same invariant held across a whole session rather than one step.
    ///
    /// One step can hold by accident. This walks a real shape — an opening
    /// prompt, four tool iterations, an assistant answer, the next human line,
    /// two more iterations — and requires at every request that the anchor is
    /// exactly the position the previous request's rolling marker wrote. That
    /// is the property the provider actually bills on: a marker that moved to
    /// content this request invented can never be an exact hit, and the local
    /// prompt-cache ledger measured what that costs (610 rows, 64.8M read
    /// tokens, uncorrelated with elapsed time).
    #[test]
    fn the_anchor_tracks_the_previous_rolling_marker_for_a_whole_session() {
        let marked_indices = |messages: &[ConversationMessage]| {
            let mut converted = convert_messages(messages);
            mark_conversation_cache_breakpoints(&mut converted);
            converted
                .iter()
                .enumerate()
                .filter(|(_, message)| {
                    message.content.iter().any(|block| block_cache(block).is_some())
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        };
        let tool_use = |id: &str| ContentBlock::ToolUse {
            id: id.to_string(),
            name: "bash".to_string(),
            input: "{}".to_string(),
        };

        let mut convo = vec![ConversationMessage::user_text("prompt")];
        let mut steps = vec![marked_indices(&convo)];
        let iterate = |convo: &mut Vec<ConversationMessage>, n: usize| {
            convo.push(ConversationMessage::assistant(vec![tool_use(&format!("t{n}"))]));
            convo.push(ConversationMessage::tool_result(
                format!("t{n}"),
                "bash",
                "out",
                false,
            ));
        };
        for n in 1..=4 {
            iterate(&mut convo, n);
            steps.push(marked_indices(&convo));
        }
        // The turn ends and the human types the next line — the transition the
        // pure tool loop never exercises.
        convo.push(ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "answer".to_string(),
        }]));
        convo.push(ConversationMessage::user_text("next prompt"));
        steps.push(marked_indices(&convo));
        for n in 5..=6 {
            iterate(&mut convo, n);
            steps.push(marked_indices(&convo));
        }

        for pair in steps.windows(2) {
            let (previous, current) = (&pair[0], &pair[1]);
            assert_eq!(
                current.first().copied(),
                previous.last().copied(),
                "the anchor must be the entry the previous request wrote \
                 (previous {previous:?}, this {current:?}); full walk: {steps:?}"
            );
        }
        assert!(
            steps.iter().skip(1).all(|marks| marks.len() == 2),
            "every request after the first carries an anchor and a roller: {steps:?}"
        );
    }

    /// An assistant-tail request (prefill, or a reminder message landing behind
    /// the last assistant turn): the anchor must be the user message that turn
    /// answers — the predecessor's tail — not the user message before it, which
    /// "second-to-last user message" would have picked.
    #[test]
    fn an_assistant_tail_anchors_on_the_user_message_it_answers() {
        let messages = vec![
            ConversationMessage::user_text("first prompt"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "first answer".to_string(),
            }]),
            ConversationMessage::user_text("second prompt"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "second answer".to_string(),
            }]),
        ];

        let mut converted = convert_messages(&messages);
        mark_conversation_cache_breakpoints(&mut converted);

        let marked = converted
            .iter()
            .enumerate()
            .filter(|(_, message)| {
                message.content.iter().any(|block| block_cache(block).is_some())
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(marked, vec![2, 3], "anchor on the newest user message behind the tail");
    }

    /// A completed isolated VERIFY leg is spliced back into the main transcript
    /// for persistence, resume, and transcript inspection. Its tool-heavy tail
    /// can add dozens of blocks, so keep one of the two message cache slots on
    /// the prefix immediately before the VERIFY prompt instead of relying on
    /// provider lookback to rediscover that boundary.
    #[test]
    fn verify_leg_keeps_the_pre_splice_prefix_cacheable() {
        let messages = vec![
            ConversationMessage::user_text("main task"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "implementation complete".to_string(),
            }]),
            ConversationMessage::user_text("[deep:VERIFY] inspect the isolated packet"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "verify-read".to_string(),
                name: "read_file".to_string(),
                input: "{}".to_string(),
            }]),
            ConversationMessage::tool_result(
                "verify-read",
                "read_file",
                "large verifier observation",
                false,
            ),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: r#"{"accepted":true,"issues":[]}"#.to_string(),
            }]),
            ConversationMessage::user_text("next foreground prompt"),
        ];

        let mut converted = convert_messages(&messages);
        mark_conversation_cache_breakpoints(&mut converted);

        assert_eq!(
            block_cache(&converted[1].content[0]),
            Some(&api::CacheControl::ephemeral()),
            "the pre-VERIFY prefix must keep an explicit cache boundary"
        );
        assert_eq!(
            block_cache(&converted[6].content[0]),
            Some(&api::CacheControl::ephemeral()),
            "the newest foreground prompt keeps the rolling tail boundary"
        );
        assert_eq!(
            converted
                .iter()
                .flat_map(|message| &message.content)
                .filter(|block| block_cache(block).is_some())
                .count(),
            2,
            "conversation history must stay within its two cache-control slots"
        );
    }

    /// The agentic-loop shape: the newest messages are a pure
    /// `tool_use`/`tool_result` exchange with no text anywhere near the tail.
    /// The markers must live in that tail — the rolling one on the newest
    /// result batch, the anchor on the result batch that ended the previous
    /// request — and NOT fall back to the last text-bearing message far behind
    /// it (the pre-fix behavior that re-billed the whole tool tail every
    /// iteration).
    #[test]
    fn tool_only_tail_carries_the_breakpoints() {
        let tool_use = |id: &str| ContentBlock::ToolUse {
            id: id.to_string(),
            name: "bash".to_string(),
            input: "{}".to_string(),
        };
        let messages = vec![
            ConversationMessage::user_text("prompt"),
            ConversationMessage::assistant(vec![tool_use("tu-1")]),
            ConversationMessage::tool_result("tu-1", "bash", "one", false),
            ConversationMessage::assistant(vec![tool_use("tu-2")]),
            ConversationMessage::tool_result("tu-2", "bash", "two", false),
        ];

        let mut converted = convert_messages(&messages);
        mark_conversation_cache_breakpoints(&mut converted);

        let last = converted.len() - 1;
        assert!(
            matches!(&converted[last].content[0], InputContentBlock::ToolResult { .. }),
            "fixture: tail is a tool_result message"
        );
        assert_eq!(
            block_cache(&converted[last].content[0]),
            Some(&api::CacheControl::ephemeral()),
            "newest tool_result carries a breakpoint"
        );
        assert!(
            block_cache(&converted[last - 1].content[0]).is_none(),
            "the tool_use turn between the two results never was a request tail"
        );
        assert_eq!(
            block_cache(&converted[last - 2].content[0]),
            Some(&api::CacheControl::ephemeral()),
            "the result batch the previous request ended on is the anchor"
        );
        assert!(
            block_cache(&converted[0].content[0]).is_none(),
            "the old text message no longer soaks up a marker"
        );
    }

    /// Thinking blocks take no `cache_control`; a thinking-led assistant turn
    /// must land its marker on the `tool_use` behind the thinking block.
    #[test]
    fn thinking_blocks_fall_through_to_the_tool_use() {
        let message = ConversationMessage::assistant(vec![
            ContentBlock::Thinking {
                thinking: "reasoning".to_string(),
                signature: "SIG".to_string(),
            },
            ContentBlock::ToolUse {
                id: "tu-1".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            },
        ]);
        let mut converted = convert_messages(&[message]);
        mark_conversation_cache_breakpoints(&mut converted);
        let content = &converted[0].content;
        assert!(matches!(&content[0], InputContentBlock::Thinking { .. }));
        assert_eq!(
            block_cache(&content[1]),
            Some(&api::CacheControl::ephemeral()),
            "marker lands on the tool_use, not the thinking block"
        );
    }

    #[test]
    fn empty_text_block_is_skipped_when_cache_marking() {
        // Anthropic 400s on `cache_control` over an empty text block. When a
        // message ends with a blank text block, the marker must fall through to
        // the previous non-empty block instead of landing on the empty one.
        let mut blocks = vec![
            InputContentBlock::Text {
                text: "real answer".into(),
                cache_control: None,
            },
            InputContentBlock::Text {
                text: "  \n".into(), // whitespace-only → treated as empty
                cache_control: None,
            },
        ];
        assert!(mark_last_cacheable_block(
            &mut blocks,
            &api::CacheControl::ephemeral_1h()
        ));
        let InputContentBlock::Text {
            cache_control: trailing,
            ..
        } = &blocks[1]
        else {
            panic!("expected a Text block");
        };
        assert!(
            trailing.is_none(),
            "blank text block must not be cache-marked"
        );
        let InputContentBlock::Text {
            cache_control: real,
            ..
        } = &blocks[0]
        else {
            panic!("expected a Text block");
        };
        assert!(
            real.is_some(),
            "the previous non-empty block must carry the marker"
        );
    }

    #[test]
    fn all_empty_blocks_mark_nothing() {
        let mut blocks = vec![InputContentBlock::Text {
            text: String::new(),
            cache_control: None,
        }];
        assert!(
            !mark_last_cacheable_block(&mut blocks, &api::CacheControl::ephemeral_1h()),
            "no non-empty cacheable block → nothing marked"
        );
    }

    /// Sub-agent variant: same anchor-plus-rolling placement, 5-minute TTL
    /// (`ttl: None` on the wire) — the spawn tail dies within minutes, so it
    /// must not pay the 1h write premium the foreground turn pays.
    /// Both ends of one decision: conversation breakpoints write at the
    /// 5-minute TTL, and the break classifier reads back the same window.
    ///
    /// They are separate constants in separate crates, so nothing but this
    /// test stops one from moving alone — and the failure would be silent in
    /// the direction that matters. Raise the marker to 1h without the config
    /// and every legitimate expiry between 5m and 1h still logs as an
    /// *unexpected* break; lower the config alone and real provider evictions
    /// get excused as expiries. Either way `breaks.jsonl` starts lying, which
    /// is the ledger every cache decision in this repo is made from.
    #[test]
    fn breakpoint_ttl_matches_break_classifier() {
        let messages = vec![
            ConversationMessage::user_text("first prompt"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "second response".to_string(),
            }]),
            ConversationMessage::user_text("third prompt"),
        ];

        let mut converted = convert_messages(&messages);
        super::mark_conversation_cache_breakpoints(&mut converted);

        // The anchor follows the declared policy (five minutes unless a host
        // declared otherwise); the ROLLING marker is the one the classifier's
        // window must mirror, under every policy.
        assert_eq!(
            block_cache(&converted[0].content[0]),
            Some(&super::conversation_anchor_ttl().cache_control()),
            "anchor carries the declared anchor TTL"
        );
        assert!(block_cache(&converted[1].content[0]).is_none());
        assert_eq!(
            block_cache(&converted[2].content[0]),
            Some(&api::CacheControl::ephemeral()),
            "rolling marker must carry the 5-minute TTL"
        );

        assert_eq!(
            api::PromptCacheConfig::default().prompt_ttl,
            std::time::Duration::from_secs(5 * 60),
            "break classification must mirror the breakpoint TTL"
        );

        // Third end of the same decision: the request ledger records the TTL
        // by reading these very blocks back (`RequestLedgerRow::ttl`), so a
        // marker change relabels every row written after it — which is what
        // lets a TTL change be judged from the ledger instead of from a deploy
        // timestamp the data does not carry. Asserted here, on the real output
        // of the marking function, because `api` cannot call back into
        // `runtime` to test the pairing from its own side.
        let expected_label = match super::conversation_anchor_ttl() {
            super::ConversationAnchorTtl::FiveMinutes => "5m",
            super::ConversationAnchorTtl::OneHour => "1h+5m",
        };
        assert_eq!(
            api::conversation_marker_ttl(&converted),
            expected_label,
            "the ledger must record the TTL these markers actually asked for"
        );
    }

    /// The slot labels the request ledger writes are the slots this function
    /// actually placed.
    ///
    /// `api::conversation_markers` derives `anchor` / `rolling` from POSITION
    /// (the highest-index message marker is the one the tail scan placed),
    /// because threading the policy's own choice through every
    /// `record_usage` / `record_response` call site to record it would be a
    /// wire change for a diagnostic. The derivation is only sound while this
    /// function keeps placing the anchor strictly BEHIND the rolling slot —
    /// which `previous_request_tail` does by construction, searching
    /// `messages[..rolling]`. This is the seam that says so out loud, on the
    /// real output of the marking function, from the side that owns the
    /// policy; `api` cannot call back into `runtime` to assert it from there.
    ///
    /// A round that splits the anchor and rolling TTLs (r20 §4) or moves the
    /// anchor forward would otherwise silently relabel every row written after
    /// it, and `docs/analysis/marker-instrument-r30.md`'s tables would be
    /// reading a word that no longer means what they say it means.
    /// The two markers do different jobs, so they may carry different
    /// lifetimes: a long anchor keeps the conversation prefix across a
    /// person's break while the rolling tail stays cheap; sub-agents keep
    /// both short. The ledger label tells the two policies apart, and the
    /// labels round-trip through the setting's own spelling.
    #[test]
    fn the_anchor_may_outlive_the_rolling_marker_and_a_sub_agent_never_takes_the_long_one() {
        let messages = vec![
            ConversationMessage::user_text("first prompt"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "second response".to_string(),
            }]),
            ConversationMessage::user_text("third prompt"),
        ];

        let mut long_anchor = convert_messages(&messages);
        super::mark_breakpoints_with_ttl(
            &mut long_anchor,
            &api::CacheControl::ephemeral_1h(),
            &api::CacheControl::ephemeral(),
        );
        assert_eq!(block_cache(&long_anchor[0].content[0]), Some(&api::CacheControl::ephemeral_1h()));
        assert!(block_cache(&long_anchor[1].content[0]).is_none());
        assert_eq!(block_cache(&long_anchor[2].content[0]), Some(&api::CacheControl::ephemeral()));
        assert_eq!(api::conversation_marker_ttl(&long_anchor), "1h+5m", "the ledger tells the policies apart");

        let mut short = convert_messages(&messages);
        super::mark_conversation_cache_breakpoints_short_lived(&mut short);
        assert_eq!(block_cache(&short[0].content[0]), Some(&api::CacheControl::ephemeral()));
        assert_eq!(block_cache(&short[2].content[0]), Some(&api::CacheControl::ephemeral()));
        assert_eq!(api::conversation_marker_ttl(&short), "5m");

        for ttl in super::ConversationAnchorTtl::ALL {
            assert_eq!(super::ConversationAnchorTtl::from_label(ttl.label()), Some(ttl));
            assert_eq!(super::ConversationAnchorTtl::from_label(&ttl.label().to_uppercase()), Some(ttl));
        }
        assert_eq!(super::ConversationAnchorTtl::from_label("2h"), None);
        assert_eq!(super::ConversationAnchorTtl::default(), super::ConversationAnchorTtl::FiveMinutes);
        assert_eq!(
            super::ConversationAnchorTtl::OneHour.cache_control(),
            api::CacheControl::ephemeral_1h()
        );
    }

    #[test]
    fn breakpoint_slots_match_the_placement_policy() {
        let messages = vec![
            ConversationMessage::user_text("first prompt"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "second response".to_string(),
            }]),
            ConversationMessage::user_text("third prompt"),
        ];

        let mut converted = convert_messages(&messages);
        super::mark_conversation_cache_breakpoints(&mut converted);

        let placed: Vec<usize> = converted
            .iter()
            .enumerate()
            .filter(|(_, message)| message.content.iter().any(|block| block_cache(block).is_some()))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(placed, vec![0, 2], "anchor at the previous tail, rolling at this one");

        let recorded = api::conversation_markers(&converted);
        assert_eq!(
            recorded
                .iter()
                .map(|marker| (marker.index, marker.slot.as_str(), marker.role.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (0, api::MARKER_SLOT_ANCHOR, "user"),
                (2, api::MARKER_SLOT_ROLLING, "user"),
            ],
            "the ledger must name the anchor the anchor and the rolling marker rolling"
        );
        assert_eq!(
            recorded.last().map(|marker| usize::try_from(marker.index).unwrap_or(usize::MAX)),
            placed.last().copied(),
            "`rolling` is the tail slot this function placed, not merely the last entry"
        );
    }
}

#[cfg(test)]
mod passport_label_tests {
    use super::{REASONING_PASSPORT_LABEL, strip_reasoning_passport_label};

    /// Only a LEADING label goes, with the newlines under it; a reply without
    /// one is untouched, and the label alone leaves nothing.
    #[test]
    fn a_leading_passport_label_is_taken_off_and_the_reply_kept() {
        let imitated = format!("{REASONING_PASSPORT_LABEL}\n\n**Refining scope**\n\nThe fix is small.");
        assert_eq!(
            strip_reasoning_passport_label(&imitated),
            "**Refining scope**\n\nThe fix is small."
        );
        assert_eq!(strip_reasoning_passport_label("plain reply"), "plain reply");
        assert_eq!(strip_reasoning_passport_label(REASONING_PASSPORT_LABEL), "");
        assert_eq!(
            strip_reasoning_passport_label("mid-text [earlier reasoning] stays"),
            "mid-text [earlier reasoning] stays"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_wire_reminders, convert_messages, convert_messages_for, ReasoningReplay,
        REASONING_PASSPORT_LABEL,
    };
    use crate::ConversationMessage;
    use api::{InputContentBlock, ToolResultContentBlock};

    fn wire_text_of(converted: &[api::InputMessage]) -> &str {
        let InputContentBlock::ToolResult { content, .. } = &converted[0].content[0] else {
            panic!("expected a ToolResult");
        };
        let ToolResultContentBlock::Text { text } = &content[0] else {
            panic!("expected a Text block");
        };
        text
    }

    /// Every `tool_use` on the wire, paired with the ids answered by the message
    /// that immediately follows it.
    ///
    /// This is the Anthropic rule stated as data: a `tool_use` is answered only
    /// by a `tool_result` in the *next* message, and that message must be a
    /// user turn. A result that arrives one message later is not an answer, and
    /// the request is rejected with `400 … tool_use ids were found without
    /// tool_result blocks`. (Block *order* inside that message is the separate
    /// job of [`enforce_tool_results_lead`], already pinned by
    /// `leading_reconcile_text_does_not_hide_the_results_behind_it`.)
    fn wire_orphans(converted: &[api::InputMessage]) -> Vec<String> {
        let mut orphans = Vec::new();
        for (index, message) in converted.iter().enumerate() {
            let uses = message.content.iter().filter_map(|block| match block {
                InputContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            });
            let next = converted.get(index + 1);
            let answered: Vec<&str> = next
                .map(|next| {
                    next.content
                        .iter()
                        .filter_map(|block| match block {
                            InputContentBlock::ToolResult { tool_use_id, .. } => {
                                Some(tool_use_id.as_str())
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let next_is_user = next.is_some_and(|next| next.role == "user");
            for id in uses {
                if !next_is_user || !answered.contains(&id.as_str()) {
                    orphans.push(id);
                }
            }
        }
        orphans
    }

    /// A transcript that was killed between a `tool_use` and its result must
    /// still lower to a request Anthropic accepts, after a full round trip
    /// through the file.
    ///
    /// This is the r24b scenario: the host window restarts, zo dies with the
    /// assistant's `tool_use` already appended to the JSONL and no result behind
    /// it, and the pane comes back as `zo --resume <id>`. If the reloaded
    /// history reached the wire verbatim the session would be bricked — the
    /// orphan is *in history*, so the 400 repeats on every later request, not
    /// just the first.
    ///
    /// The seal lives in [`core_types::session::Session::tool_consistent_messages`]
    /// (a per-request view; stored history is never rewritten) and the request
    /// builder composes the two. Pinning the composition here, on a session that
    /// really came back off disk, is what keeps a refactor from moving the call
    /// site and losing the property silently.
    #[test]
    fn a_transcript_killed_mid_tool_never_lowers_to_an_orphan() {
        use crate::session::{ContentBlock, Session};

        let directory = tempfile::tempdir().expect("temp session directory");
        let path = directory.path().join("r24b-killed.jsonl");

        let mut killed = Session::new();
        killed.push_user_text("run the long command").expect("user append");
        killed
            .push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "toolu_killed".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"sleep 600"}"#.to_string(),
            }]))
            .expect("assistant append");
        // SIGKILL lands here: the call is durable, the result never existed.
        killed.save_to_path(&path).expect("persist the killed transcript");

        let mut resumed = Session::load_from_path(&path).expect("resume the killed transcript");
        assert_eq!(resumed.messages.len(), 2, "the orphan survived the reload");
        resumed
            .push_user_text("never mind that; are you back?")
            .expect("the resumed prompt");

        let view = resumed.tool_consistent_messages();
        let converted = convert_messages(&view);
        assert!(
            wire_orphans(&converted).is_empty(),
            "a resumed session must not put an unanswered tool_use on the wire: {converted:#?}"
        );
        // The seal is a view, not a rewrite: the transcript still says what
        // actually happened, and a real result arriving later still appends.
        assert_eq!(resumed.messages.len(), 3, "stored history was not mutated");
        assert!(
            !std::fs::read_to_string(&path)
                .expect("re-read the transcript")
                .contains("tool_result"),
            "loading a killed transcript must not write a synthetic result into it"
        );
    }

    /// Re-deriving the request view twice is idempotent — one seal, not two.
    ///
    /// A resumed session assembles a request on every streaming iteration, so a
    /// seal that accumulated would grow an unanswered `tool_result` per turn and
    /// break the pairing from the other side.
    #[test]
    fn sealing_the_same_orphan_twice_adds_one_result_not_two() {
        use crate::session::{ContentBlock, Session};

        let mut session = Session::new();
        session.push_user_text("go").expect("user append");
        session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "toolu_once".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            }]))
            .expect("assistant append");

        let first = convert_messages(&session.tool_consistent_messages());
        let second = convert_messages(&session.tool_consistent_messages());
        assert_eq!(first.len(), second.len(), "the seal must not accumulate");
        let results = second
            .iter()
            .flat_map(|message| &message.content)
            .filter(|block| matches!(block, InputContentBlock::ToolResult { .. }))
            .count();
        assert_eq!(results, 1, "exactly one synthetic result: {second:#?}");
        assert!(wire_orphans(&second).is_empty());
    }

    /// The lowering is a lowering, not a repair — and that is on purpose.
    ///
    /// `convert_messages` faithfully lowers whatever it is handed; hand it an
    /// orphan and an orphan reaches the wire. The property the session depends
    /// on therefore belongs to the *caller*: every path to a provider must go
    /// through the sealed view. This test exists so that a future refactor which
    /// bypasses the seal fails a test that names the reason rather than
    /// producing a 400 in someone's overnight run.
    #[test]
    fn lowering_alone_does_not_invent_a_seal() {
        use crate::ContentBlock;

        let raw = vec![
            ConversationMessage::user_text("go"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "toolu_raw".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            }]),
        ];
        assert_eq!(
            wire_orphans(&convert_messages(&raw)),
            vec!["toolu_raw".to_string()],
            "sealing is the session view's job, not the lowering's"
        );
    }

    /// Freshness is a property of the trailing run of tool results, not of the
    /// single last message.
    ///
    /// Parallel tool calls come back as several adjacent `Tool` messages and all
    /// belong to the turn that just ran, so all of them must reach the model
    /// unelided. The moment anything else follows, they are history again.
    #[test]
    fn newest_tool_run_covers_every_trailing_tool_message() {
        use super::newest_tool_run_start;

        let tool = |id: &str| ConversationMessage::tool_result(id, "read_file", "{}", false);
        let user = || ConversationMessage::user_text("next");

        assert_eq!(newest_tool_run_start(&[]), 0);
        // Nothing trailing: every result is eligible for summarization.
        assert_eq!(newest_tool_run_start(&[tool("a"), user()]), 2);
        // One result, and it is the newest.
        assert_eq!(newest_tool_run_start(&[user(), tool("a")]), 1);
        // A parallel batch: the whole run is newest, not just its last member.
        assert_eq!(
            newest_tool_run_start(&[user(), tool("a"), tool("b"), tool("c")]),
            1
        );
        // An earlier run does not join the newest one across a user turn.
        assert_eq!(
            newest_tool_run_start(&[tool("old"), user(), tool("a"), tool("b")]),
            2
        );
    }

    /// The runtime persists per-request reminders as a trailing `System`
    /// message right after the newest tool batch, so an in-flight turn's tail
    /// is `[.., tool.., reminder]`. The scan must treat `System` as
    /// transparent: if the reminder ended the trailing run, the results the
    /// model asked for *this turn* would be handed back elided.
    #[test]
    fn newest_tool_run_sees_through_a_trailing_reminder_message() {
        use super::newest_tool_run_start;
        use crate::session::MessageRole;
        use crate::ContentBlock;

        let tool = |id: &str| ConversationMessage::tool_result(id, "read_file", "{}", false);
        let user = || ConversationMessage::user_text("next");
        let reminder = || ConversationMessage {
            role: MessageRole::System,
            blocks: vec![ContentBlock::Text {
                text: "<system-reminder>\nkeep going\n</system-reminder>".to_string(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        };

        // The exact request-build shape: reminder appended after the batch.
        assert_eq!(
            newest_tool_run_start(&[user(), tool("a"), tool("b"), reminder()]),
            1
        );
        // Turn start (no trailing tools): unchanged — everything is history.
        assert_eq!(
            newest_tool_run_start(&[tool("a"), reminder(), user()]),
            3
        );
    }

    #[test]
    fn tool_result_images_become_wire_image_blocks() {
        // A stored ToolResult carrying an image must convert to an
        // InputContentBlock::ToolResult whose content has a Text block followed
        // by a ToolResultContentBlock::Image — the model actually sees pixels.
        let message = ConversationMessage::tool_result_with_images(
            "tu-1",
            "read_image",
            "staged",
            false,
            vec![("image/png".to_string(), "QUJD".to_string())],
        );
        let converted = convert_messages(&[message]);
        assert_eq!(converted.len(), 1);
        let InputContentBlock::ToolResult { content, .. } = &converted[0].content[0] else {
            panic!("expected an InputContentBlock::ToolResult");
        };
        assert_eq!(content.len(), 2, "text block + one image block");
        assert!(matches!(content[0], ToolResultContentBlock::Text { .. }));
        match &content[1] {
            ToolResultContentBlock::Image { source } => {
                assert_eq!(source.kind, "base64");
                assert_eq!(source.media_type, "image/png");
                assert_eq!(source.data, "QUJD");
            }
            other => panic!("expected an Image block, got {other:?}"),
        }
    }

    #[test]
    fn oversized_stored_tool_result_image_is_downscaled_on_the_wire() {
        // The session-wedge regression: a full-page screenshot taller than
        // 8000px baked into a stored tool_result 400s every turn. Lowering must
        // downscale it (to PNG) so an already-poisoned history un-wedges without
        // surgery. Build a real oversized PNG so the guard actually fires.
        use base64::Engine as _;
        use image::{DynamicImage, ImageFormat, RgbImage};

        let mut buf = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::new(400, 12000))
            .write_to(&mut buf, ImageFormat::Png)
            .expect("encode oversized test PNG");
        let data = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());

        let message = ConversationMessage::tool_result_with_images(
            "tu-oversized",
            "read_image",
            "staged",
            false,
            vec![("image/png".to_string(), data.clone())],
        );
        let converted = convert_messages(&[message]);
        let InputContentBlock::ToolResult { content, .. } = &converted[0].content[0] else {
            panic!("expected a ToolResult");
        };
        let ToolResultContentBlock::Image { source } = &content[1] else {
            panic!("expected a downscaled image block, got {:?}", content[1]);
        };
        assert_eq!(source.media_type, "image/png", "downscale re-encodes to PNG");
        assert_ne!(source.data, data, "the oversized payload must be replaced");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(source.data.as_bytes())
            .expect("valid base64 out");
        let dims = crate::image_guard::guard_image_bytes(&decoded);
        assert_eq!(
            dims,
            crate::image_guard::ImageGuardOutcome::Keep,
            "the lowered image must now be within the cap"
        );
    }

    #[test]
    fn text_only_tool_result_has_no_image_block() {
        let message = ConversationMessage::tool_result("tu-2", "bash", "ok", false);
        let converted = convert_messages(&[message]);
        let InputContentBlock::ToolResult { content, .. } = &converted[0].content[0] else {
            panic!("expected a ToolResult");
        };
        assert_eq!(content.len(), 1, "text-only → exactly one Text block");
        assert!(matches!(content[0], ToolResultContentBlock::Text { .. }));
    }

    #[test]
    fn read_file_result_is_compressed_on_the_wire_only() {
        // The session block keeps the pretty-JSON envelope; the wire view the
        // model sees is the compact unwrapped form.
        let body = "fn main() {\n    println!(\"hello\");\n}\n".repeat(30);
        let envelope = serde_json::to_string_pretty(&serde_json::json!({
            "type": "text",
            "file": {
                "filePath": "/ws/src/main.rs",
                "content": body,
                "numLines": body.lines().count(),
                "startLine": 1,
                "totalLines": body.lines().count(),
            }
        }))
        .expect("serialize envelope");
        let message = ConversationMessage::tool_result("tu-3", "read_file", &envelope, false);
        let converted = convert_messages(std::slice::from_ref(&message));
        let wire = wire_text_of(&converted);
        assert!(
            wire.starts_with("[file] /ws/src/main.rs"),
            "wire view is unwrapped"
        );
        assert!(
            wire.contains("println!(\"hello\");"),
            "content preserved verbatim"
        );
        assert!(wire.len() < envelope.len(), "wire view is smaller");
        // And the stored session block still carries the original envelope.
        let crate::session::ContentBlock::ToolResult { output, .. } = &message.blocks[0] else {
            panic!("expected a session ToolResult");
        };
        assert_eq!(output, &envelope, "session history is untouched");
    }

    #[test]
    fn error_tool_result_passes_through_verbatim() {
        let error_text = "io error: old_string not found in file";
        let message = ConversationMessage::tool_result("tu-4", "edit_file", error_text, true);
        let converted = convert_messages(&[message]);
        assert_eq!(wire_text_of(&converted), error_text);
    }

    #[test]
    fn unknown_tool_result_passes_through_verbatim() {
        let payload = r#"{"anything": "goes", "even": ["json"]}"#;
        let message = ConversationMessage::tool_result("tu-5", "TodoWrite", payload, false);
        let converted = convert_messages(&[message]);
        assert_eq!(wire_text_of(&converted), payload);
    }

    #[test]
    fn parallel_tool_results_coalesce_into_one_wire_message() {
        // A parallel batch stores one Tool message per result, but the API
        // requires every tool_use id of the assistant turn to be answered in
        // THE single next message. Lowering coalesces the run so the
        // invariant holds in code instead of leaning on the API's
        // same-role-merge leniency (the 400 class: "tool_use ids were found
        // without tool_result blocks immediately after").
        let tool_use = |id: &str| crate::session::ContentBlock::ToolUse {
            id: id.to_string(),
            name: "bash".to_string(),
            input: "{}".to_string(),
        };
        let converted = convert_messages(&[
            ConversationMessage::user_text("prompt"),
            ConversationMessage::assistant(vec![
                tool_use("tu-1"),
                tool_use("tu-2"),
                tool_use("tu-3"),
            ]),
            ConversationMessage::tool_result("tu-1", "bash", "one", false),
            ConversationMessage::tool_result("tu-2", "bash", "two", false),
            ConversationMessage::tool_result("tu-3", "bash", "three", false),
        ]);
        assert_eq!(
            converted.len(),
            3,
            "user, assistant, ONE coalesced result message"
        );
        assert_eq!(converted[2].role, "user");
        let ids: Vec<&str> = converted[2]
            .content
            .iter()
            .map(|block| match block {
                InputContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.as_str(),
                other => panic!("coalesced message must be pure tool results, got {other:?}"),
            })
            .collect();
        assert_eq!(ids, ["tu-1", "tu-2", "tu-3"], "order preserved");
    }

    #[test]
    fn resolvable_deferred_tool_stays_linked_in_a_coalesced_batch() {
        // TaskStop is deferred from ordinary advertisement but remains
        // resolvable. Anthropic's accepted-name set includes it, so the stored
        // pair must survive reconciliation and conversion as a real linked
        // tool_use/tool_result rather than archived prose.
        let tool_use = |id: &str, name: &str| crate::session::ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: "{}".to_string(),
        };
        let stored = std::sync::Arc::new(vec![
            ConversationMessage::user_text("prompt"),
            ConversationMessage::assistant(vec![
                tool_use("tu-todo", "TodoWrite"),
                tool_use("tu-stop", "TaskStop"),
                tool_use("tu-grep", "grep_search"),
            ]),
            ConversationMessage::tool_result("tu-todo", "TodoWrite", "ok", false),
            ConversationMessage::tool_result("tu-stop", "TaskStop", "already terminal", true),
            ConversationMessage::tool_result("tu-grep", "grep_search", "3 matches", false),
            ConversationMessage::user_text("ggo"),
        ]);
        let known: std::collections::BTreeSet<String> =
            ["TodoWrite", "TaskStop", "grep_search"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let reconciled = crate::session::reconcile_tool_history(&stored, &known, &known);
        let converted = convert_messages(&reconciled);

        assert_eq!(converted.len(), 4, "user, assistant, batch, user");
        let batch = &converted[2].content;
        assert_eq!(batch.len(), 3, "all three linked results survive");
        let ids: Vec<&str> = batch
            .iter()
            .map(|block| match block {
                InputContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.as_str(),
                other => panic!("resolved batch must stay pure tool results, got {other:?}"),
            })
            .collect();
        assert_eq!(ids, ["tu-todo", "tu-stop", "tu-grep"], "order kept");
    }

    #[test]
    fn leading_reconcile_text_does_not_hide_the_results_behind_it() {
        // Variant: the FIRST tool of the batch is the unknown one, so the
        // rewrite text starts the coalesced message and pre-fix would hide
        // EVERY result behind it.
        let tool_use = |id: &str, name: &str| crate::session::ContentBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: "{}".to_string(),
        };
        let stored = std::sync::Arc::new(vec![
            ConversationMessage::assistant(vec![
                tool_use("tu-gone", "SendMessage"),
                tool_use("tu-a", "bash"),
                tool_use("tu-b", "read_file"),
            ]),
            ConversationMessage::tool_result("tu-gone", "SendMessage", "sent", false),
            ConversationMessage::tool_result("tu-a", "bash", "ok", false),
            ConversationMessage::tool_result("tu-b", "read_file", "body", false),
        ]);
        let known: std::collections::BTreeSet<String> = ["bash", "read_file"]
            .iter()
            .map(ToString::to_string)
            .collect();
        // `SendMessage` is absent from both sets here: this models a genuinely
        // removed tool, which must still become archived text.
        let reconciled = crate::session::reconcile_tool_history(&stored, &known, &known);
        let converted = convert_messages(&reconciled);
        let batch = &converted[1].content;
        assert!(
            matches!(&batch[0], InputContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "tu-a"),
            "first block must be a result, not the rewrite text"
        );
        assert!(
            matches!(&batch[1], InputContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "tu-b")
        );
        assert!(matches!(&batch[2], InputContentBlock::Text { .. }));
    }

    #[test]
    fn separate_tool_runs_do_not_cross_merge() {
        let converted = convert_messages(&[
            ConversationMessage::tool_result("tu-1", "bash", "one", false),
            ConversationMessage::assistant(vec![crate::session::ContentBlock::Text {
                text: "between".to_string(),
            }]),
            ConversationMessage::tool_result("tu-2", "bash", "two", false),
        ]);
        assert_eq!(converted.len(), 3, "an assistant turn ends the run");
    }

    #[test]
    fn reminders_land_after_the_whole_coalesced_batch() {
        let mut converted = convert_messages(&[
            ConversationMessage::user_text("prompt"),
            ConversationMessage::tool_result("tu-1", "bash", "one", false),
            ConversationMessage::tool_result("tu-2", "bash", "two", false),
        ]);
        append_wire_reminders(&mut converted, &["todo progress".to_string()]);
        let tail = &converted[1].content;
        assert_eq!(tail.len(), 3, "2 results + 1 reminder in one message");
        assert!(matches!(tail[0], InputContentBlock::ToolResult { .. }));
        assert!(matches!(tail[1], InputContentBlock::ToolResult { .. }));
        assert!(
            matches!(tail[2], InputContentBlock::Text { .. }),
            "reminder rides behind the batch, never inside it"
        );
    }

    #[test]
    fn wire_reminders_ride_the_newest_user_message_as_tagged_tail_blocks() {
        let mut converted = convert_messages(&[
            ConversationMessage::user_text("first prompt"),
            ConversationMessage::assistant(vec![crate::session::ContentBlock::Text {
                text: "answer".to_string(),
            }]),
            ConversationMessage::tool_result("tu-1", "bash", "ok", false),
        ]);
        append_wire_reminders(
            &mut converted,
            &["[zo:todo-progress] item 2 in progress".to_string()],
        );

        // Prior messages untouched — the prefix stays byte-identical.
        assert_eq!(converted[0].content.len(), 1);
        assert_eq!(converted[1].content.len(), 1);
        // Reminder is the tail block of the newest user message (after the
        // tool_result, satisfying the tool_result-first rule), tag-wrapped.
        let tail = &converted[2].content;
        assert_eq!(tail.len(), 2);
        assert!(matches!(tail[0], InputContentBlock::ToolResult { .. }));
        let InputContentBlock::Text { text, .. } = &tail[1] else {
            panic!("expected trailing reminder text block");
        };
        assert!(text.starts_with("<system-reminder>\n"));
        assert!(text.contains("[zo:todo-progress] item 2 in progress"));
        assert!(text.ends_with("\n</system-reminder>"));
    }

    #[test]
    fn wire_reminders_skip_empty_and_never_double_wrap() {
        let mut converted = convert_messages(&[ConversationMessage::user_text("prompt")]);
        let pre_tagged = "[zo:hook]\n<system-reminder>\nhook context\n</system-reminder>";
        append_wire_reminders(
            &mut converted,
            &["   ".to_string(), pre_tagged.to_string()],
        );

        let content = &converted[0].content;
        assert_eq!(content.len(), 2, "blank reminder contributes no block");
        let InputContentBlock::Text { text, .. } = &content[1] else {
            panic!("expected reminder text block");
        };
        assert_eq!(text, pre_tagged, "producer-supplied tags are kept as-is");
    }

    #[test]
    fn wire_reminders_no_op_when_empty() {
        let mut converted = convert_messages(&[ConversationMessage::user_text("prompt")]);
        let before = converted.clone();
        append_wire_reminders(&mut converted, &[]);
        assert_eq!(converted, before);
    }

    /// A stored, signed thinking block lowers to the wire VERBATIM (text +
    /// signature) and leads the assistant turn — before text and `tool_use` — which
    /// is the order the Anthropic API validates on replay. The Anthropic path
    /// serializes `InputContentBlock` directly, so the serde shape below is the
    /// exact wire payload.
    #[test]
    fn signed_thinking_replays_verbatim_before_tool_use_on_the_wire() {
        let message = ConversationMessage::assistant(vec![
            crate::session::ContentBlock::Thinking {
                thinking: "let me reason".to_string(),
                signature: "SIG-xyz".to_string(),
            },
            crate::session::ContentBlock::Text {
                text: "answer".to_string(),
            },
            crate::session::ContentBlock::ToolUse {
                id: "tu-1".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            },
        ]);
        let converted = convert_messages(&[message]);
        assert_eq!(converted.len(), 1);
        let content = &converted[0].content;
        assert!(
            matches!(
                &content[0],
                InputContentBlock::Thinking { thinking, signature }
                    if thinking == "let me reason" && signature == "SIG-xyz"
            ),
            "thinking must lead the turn: {content:?}"
        );
        assert!(matches!(&content[1], InputContentBlock::Text { .. }));
        assert!(matches!(&content[2], InputContentBlock::ToolUse { .. }));

        let wire = serde_json::to_value(&content[0]).expect("serialize thinking to wire");
        assert_eq!(wire["type"], "thinking");
        assert_eq!(wire["thinking"], "let me reason");
        assert_eq!(wire["signature"], "SIG-xyz");
    }

    /// An unsigned thinking block never reaches the wire AS a thinking block —
    /// the API 400s on one — but its content is no longer thrown away. GPT
    /// reasoning is stored exactly this way (empty signature), so dropping it
    /// silently deleted the reasoning of every non-Anthropic turn.
    #[test]
    fn unsigned_thinking_is_carried_as_text_never_sent_unsigned() {
        let message = ConversationMessage::assistant(vec![
            crate::session::ContentBlock::Thinking {
                thinking: "legacy reasoning".to_string(),
                signature: String::new(),
            },
            crate::session::ContentBlock::Text {
                text: "answer".to_string(),
            },
        ]);
        let converted = convert_messages(&[message]);
        let content = &converted[0].content;
        assert!(
            !content
                .iter()
                .any(|block| matches!(block, InputContentBlock::Thinking { .. })),
            "no unsigned thinking may reach the wire: {content:?}"
        );
        let carried = content
            .iter()
            .find_map(|block| match block {
                InputContentBlock::Text { text, .. }
                    if text.starts_with(REASONING_PASSPORT_LABEL) =>
                {
                    Some(text.clone())
                }
                _ => None,
            })
            .expect("the reasoning is carried as text, not deleted");
        assert!(carried.contains("legacy reasoning"), "{carried}");
        assert!(
            content.iter().any(|block| matches!(
                block,
                InputContentBlock::Text { text, .. } if text == "answer"
            )),
            "the answer still follows the carried reasoning"
        );
    }

    /// The whole point, stated as one test: switching providers mid-session
    /// must not delete the reasoning behind the conversation.
    ///
    /// Before this, an Anthropic-produced signed block was matched and dropped
    /// by every other encoder, so the model you switched TO saw the tool calls
    /// and the conclusions with every "why" removed — and nothing said so.
    #[test]
    fn switching_providers_carries_the_reasoning_instead_of_deleting_it() {
        let history = [ConversationMessage::assistant(vec![
            crate::session::ContentBlock::Thinking {
                thinking: "the lock is held across the await, which is the deadlock".to_string(),
                signature: "SIG-xyz".to_string(),
            },
            crate::session::ContentBlock::Text {
                text: "found it".to_string(),
            },
        ])];

        // Staying on Anthropic: verbatim signed replay, byte-for-byte.
        let native = convert_messages_for(&history, ReasoningReplay::Native);
        assert!(matches!(
            &native[0].content[0],
            InputContentBlock::Thinking { thinking, signature }
                if thinking.contains("deadlock") && signature == "SIG-xyz"
        ));

        // Switching away: the same reasoning, as text the new model can read.
        let switched = convert_messages_for(&history, ReasoningReplay::AsText);
        assert!(
            !switched[0]
                .content
                .iter()
                .any(|block| matches!(block, InputContentBlock::Thinking { .. })),
            "a signature the target cannot verify must not ride the wire"
        );
        let carried = match &switched[0].content[0] {
            InputContentBlock::Text { text, .. } => text.clone(),
            other => panic!("expected carried reasoning, got {other:?}"),
        };
        assert!(carried.starts_with(REASONING_PASSPORT_LABEL), "{carried}");
        assert!(
            carried.contains("the lock is held across the await"),
            "the finding itself must survive the switch: {carried}"
        );
    }

    /// A provider switch is where the cache is already lost, so carrying
    /// reasoning there is free — but the carried text must still be a pure
    /// function of the stored block, or it would break the prefix on every
    /// request AFTER the switch too.
    #[test]
    fn carried_reasoning_is_byte_stable_across_repeated_lowerings() {
        let long = (0..400)
            .map(|index| format!("step {index}: consider the {index}th branch"))
            .collect::<Vec<_>>()
            .join("\n");
        let history = [ConversationMessage::assistant(vec![
            crate::session::ContentBlock::Thinking {
                thinking: long,
                signature: String::new(),
            },
        ])];
        let first = convert_messages_for(&history, ReasoningReplay::AsText);
        for _ in 0..4 {
            assert_eq!(
                first,
                convert_messages_for(&history, ReasoningReplay::AsText),
                "the same history must lower to the same bytes every request"
            );
        }
        let text = match &first[0].content[0] {
            InputContentBlock::Text { text, .. } => text.clone(),
            other => panic!("expected carried reasoning, got {other:?}"),
        };
        assert!(
            text.chars().count() < 1_400,
            "carried reasoning must stay budgeted, got {} chars",
            text.chars().count()
        );
        assert!(
            text.contains("session_recall"),
            "a truncated carry must point at the full record: {text}"
        );
    }

    /// Redacted reasoning is sealed by the provider that made it. There is
    /// nothing to carry, so it is omitted rather than forwarded as a blob the
    /// next model would read as noise.
    #[test]
    fn redacted_reasoning_is_replayed_natively_and_omitted_elsewhere() {
        let history = [ConversationMessage::assistant(vec![
            crate::session::ContentBlock::RedactedThinking {
                data: "OPAQUE".to_string(),
            },
            crate::session::ContentBlock::Text {
                text: "answer".to_string(),
            },
        ])];
        let native = convert_messages_for(&history, ReasoningReplay::Native);
        assert!(matches!(
            &native[0].content[0],
            InputContentBlock::RedactedThinking { .. }
        ));
        let switched = convert_messages_for(&history, ReasoningReplay::AsText);
        assert_eq!(switched[0].content.len(), 1, "{:?}", switched[0].content);
        assert!(matches!(
            &switched[0].content[0],
            InputContentBlock::Text { text, .. } if text == "answer"
        ));
    }

    /// The handoff note has to be honest in both directions: it must not claim
    /// the reasoning changed shape on a same-provider swap (the model would go
    /// looking for markers that are not there), and it must say so when it did.
    #[test]
    fn the_handoff_note_only_mentions_carried_reasoning_when_the_provider_changed() {
        let same_provider =
            super::model_handoff_notice("claude-opus-5", "claude-sonnet-5").expect("a switch");
        assert!(same_provider.contains("claude-opus-5"));
        assert!(same_provider.contains("claude-sonnet-5"));
        assert!(
            !same_provider.contains(REASONING_PASSPORT_LABEL),
            "native replay both sides — nothing changed shape: {same_provider}"
        );

        let crossed =
            super::model_handoff_notice("claude-opus-5", "gpt-5.6-sol").expect("a switch");
        assert!(
            crossed.contains(REASONING_PASSPORT_LABEL) && crossed.contains("session_recall"),
            "a crossed switch must explain what the model is looking at: {crossed}"
        );

        assert!(
            super::model_handoff_notice("claude-opus-5", "claude-opus-5").is_none(),
            "no switch, no note"
        );
    }

    /// The replay mode is decided by the target model, not by a flag a caller
    /// might forget to set — every request lowers through this.
    #[test]
    fn the_replay_mode_follows_the_target_model() {
        for anthropic in ["claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5-20251001"] {
            assert_eq!(
                ReasoningReplay::for_model(anthropic),
                ReasoningReplay::Native,
                "{anthropic}"
            );
        }
        for other in ["gpt-5.6-sol", "gemini-3-pro", "grok-3"] {
            assert_eq!(
                ReasoningReplay::for_model(other),
                ReasoningReplay::AsText,
                "{other}"
            );
        }
    }

    /// Determinism pin: the smart-AUTO cache-collapse incident traced back to a
    /// suspicion that a provider-swap/history-lowering pass emitted different
    /// bytes on repeated calls for the same input, defeating Anthropic's prefix
    /// cache. `convert_messages` is the shared SSOT every provider request
    /// lowers through, so it must produce byte-identical (`PartialEq`) output
    /// for the same input every time. The fixture spans every branch that
    /// touches ordering or filtering: a signed thinking block (kept), an
    /// empty-signature thinking block — exactly how GPT-produced reasoning is
    /// stored in session history — (dropped), a parallel tool-use batch, and an
    /// image-bearing `tool_result`.
    #[test]
    fn convert_messages_is_deterministic_across_repeated_calls() {
        let messages = vec![
            ConversationMessage::user_text("prompt"),
            ConversationMessage::assistant(vec![
                crate::session::ContentBlock::Thinking {
                    thinking: "signed reasoning".to_string(),
                    signature: "SIG-1".to_string(),
                },
                crate::session::ContentBlock::Thinking {
                    thinking: "gpt reasoning, no signature".to_string(),
                    signature: String::new(),
                },
                crate::session::ContentBlock::Text {
                    text: "answer".to_string(),
                },
                crate::session::ContentBlock::ToolUse {
                    id: "tu-1".to_string(),
                    name: "bash".to_string(),
                    input: "{}".to_string(),
                },
                crate::session::ContentBlock::ToolUse {
                    id: "tu-2".to_string(),
                    name: "read_file".to_string(),
                    input: "{}".to_string(),
                },
            ]),
            ConversationMessage::tool_result("tu-1", "bash", "one", false),
            ConversationMessage::tool_result_with_images(
                "tu-2",
                "read_file",
                "staged",
                false,
                vec![("image/png".to_string(), "QUJD".to_string())],
            ),
        ];

        let first = convert_messages(&messages);
        let second = convert_messages(&messages);
        assert_eq!(
            first, second,
            "same &[ConversationMessage] must lower to byte-identical output every call"
        );

        // And confirm the empty-signature thinking never survives into either
        // run — it must never reach a provider wire (see anthropic.rs tests for
        // the follow-up trace of what would happen if it ever did).
        for converted in [&first, &second] {
            assert!(
                !converted
                    .iter()
                    .flat_map(|message| &message.content)
                    .any(|block| matches!(
                        block,
                        InputContentBlock::Thinking { signature, .. } if signature.is_empty()
                    )),
                "empty-signature thinking must never reach the wire"
            );
        }
    }

    /// `append_wire_reminders` mutates in place, so pin determinism by running
    /// it twice from independent clones of the same starting state.
    #[test]
    fn append_wire_reminders_is_deterministic() {
        let base = convert_messages(&[
            ConversationMessage::user_text("first"),
            ConversationMessage::assistant(vec![crate::session::ContentBlock::Text {
                text: "answer".to_string(),
            }]),
            ConversationMessage::tool_result("tu-1", "bash", "ok", false),
        ]);
        let reminders = vec![
            "[zo:todo-progress] item 2 in progress".to_string(),
            "   ".to_string(),
            "<system-reminder>\nalready tagged\n</system-reminder>".to_string(),
        ];

        let mut run_a = base.clone();
        append_wire_reminders(&mut run_a, &reminders);
        let mut run_b = base.clone();
        append_wire_reminders(&mut run_b, &reminders);

        assert_eq!(
            run_a, run_b,
            "same starting messages + reminders must append identically every call"
        );
    }

    /// The stronger form of "attaches to the newest user-role message": a
    /// trailing ASSISTANT turn must never receive it, even though it is the
    /// literal last message in the list. Only the last message whose *role* is
    /// `user` may change.
    #[test]
    fn append_wire_reminders_attaches_only_to_the_last_user_role_message_even_behind_an_assistant_tail(
    ) {
        let mut converted = convert_messages(&[
            ConversationMessage::user_text("first"),
            ConversationMessage::assistant(vec![crate::session::ContentBlock::Text {
                text: "a1".to_string(),
            }]),
            ConversationMessage::user_text("second"),
            ConversationMessage::assistant(vec![crate::session::ContentBlock::Text {
                text: "a2".to_string(),
            }]),
        ]);
        let before: Vec<usize> = converted.iter().map(|message| message.content.len()).collect();

        append_wire_reminders(&mut converted, &["reminder".to_string()]);

        assert_eq!(
            converted[0].content.len(),
            before[0],
            "earliest user message untouched"
        );
        assert_eq!(
            converted[1].content.len(),
            before[1],
            "first assistant turn untouched"
        );
        assert_eq!(converted[2].role, "user");
        assert_eq!(
            converted[2].content.len(),
            before[2] + 1,
            "the LAST user-role message gets the reminder"
        );
        assert_eq!(converted[3].role, "assistant");
        assert_eq!(
            converted[3].content.len(),
            before[3],
            "the trailing assistant turn — the actual last message — must NOT receive it"
        );
    }

    /// A `redacted_thinking` block carries no signature but is still replayed
    /// verbatim (its `data` is the encrypted reasoning the API returned).
    #[test]
    fn redacted_thinking_replays_on_the_wire() {
        let message = ConversationMessage::assistant(vec![
            crate::session::ContentBlock::RedactedThinking {
                data: "ENCRYPTED".to_string(),
            },
            crate::session::ContentBlock::Text {
                text: "answer".to_string(),
            },
        ]);
        let converted = convert_messages(&[message]);
        let content = &converted[0].content;
        assert!(matches!(
            &content[0],
            InputContentBlock::RedactedThinking { data } if data == "ENCRYPTED"
        ));
        let wire = serde_json::to_value(&content[0]).expect("serialize redacted to wire");
        assert_eq!(wire["type"], "redacted_thinking");
        assert_eq!(wire["data"], "ENCRYPTED");
    }
}
