//! Relevance compaction — the pure half (t-6039).
//!
//! Full compaction summarizes every message outside the preserved tail, and
//! the only trim before it is microcompact's, which clears the OLDEST tool
//! results first. Age is a poor reader of what the summary still needs: a
//! `grep` from ten turns ago whose finding was acted on is dead weight, and a
//! file read three turns ago that is still being edited is not. This module
//! asks one thing instead, per tool result in the compaction set: given the
//! person's last request and where the work stands, does the summary still
//! need to read this block? A closed keep/drop question over one shared
//! state, the way the skill seat asks about skills (`crate::skill_rank`).
//!
//! Nothing here calls anything. It picks the candidates out of a plan, builds
//! the state and the questions, cuts them into requests no larger than one
//! request should carry, checks a reply against the questions it asked, and
//! applies a judgment to the plan. The executor that puts the questions on
//! the wire, the door they pass, the setting that permits dropping and the
//! ledger that keeps the row are the tools crate's
//! (`smart_router::compaction_seat`); the runtime reaches it through the
//! [`CompactionSeat`] a host installs, so the runtime crate — which the tools
//! crate depends on — never has to know the wire.
//!
//! # A drop is a microcompact clear
//!
//! Dropping a block reuses the microcompact machinery whole rather than a
//! second placeholder: the ORIGINAL message is sealed to the session's vault
//! first (`seal_originals_of` in the parent module, the same seal the trim pays), the
//! summary's copy of the block carries [`MICROCOMPACT_PLACEHOLDER`], and the
//! eviction path heals every placeholder back from that seal before it
//! writes the raw record (`apply_compaction`'s `restore_microcompacted_bodies`).
//! So the summary reads only what was kept, the vault stays lossless, and a
//! dropped block is still readable through `session_recall`. A seal that
//! fails to write drops nothing, for the reason microcompact's does: the copy
//! in hand would be the last one.
//!
//! # Nothing gets worse for asking
//!
//! A judgment that was refused, failed, missed the wall, or broke the
//! contract keeps every block it covered, and a recording mode keeps every
//! block whatever it answered — the plan the summary reads is then
//! byte-identical to the plan `prepare_compaction` made, which is what a
//! compaction did before the seat existed.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use api::{SystemOneQuestion, SystemOneResponse};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use zerocode_core::jev::choice::{self, ChoiceRefusal};
use zerocode_core::jev::door::cut;
use zerocode_core::jev::{
    shard, Cap, COMPACTION_BLOCK_HEAD_BYTE_CAP, COMPACTION_DROP, COMPACTION_DROP_FLOOR_PERMILLE,
    COMPACTION_GOAL_CHAR_CAP, COMPACTION_INPUT_CHAR_CAP, COMPACTION_KEEP, COMPACTION_OPTIONS,
    COMPACTION_RECENT_CHAR_CAP, COMPACTION_SHARD_TARGET,
};

use super::{is_edit_result_tool, CompactionPlan, MICROCOMPACT_PLACEHOLDER};
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

/// Bumped whenever an option's meaning, the instructions or the state's
/// shape changes. A row carries it, so a judgment taken under other words is
/// never read as evidence about these ones.
pub const COMPACTION_RUBRIC_VERSION: u32 = 1;

/// The letter a question id opens with, so what comes back is named rather
/// than numbered. Spelled once: [`questions`] writes ids with it and
/// [`position_of`] reads them back through it.
const QUESTION_ID_PREFIX: &str = "b";

/// What `keep` means, in the words the judgment is shown.
pub const KEEP_MEANS: &str = "The work that remains will still need this result's own content: \
    the file is still being edited or compared, or a detail in it is not yet resolved or acted on.";

/// What `drop` means, in the words the judgment is shown.
pub const DROP_MEANS: &str = "This result has served its purpose: what mattered in it is already \
    reflected in later turns, or the work that remains does not depend on it.";

/// The instructions every block question carries, before its own place is
/// named: one sentence, so a reader of the ledger and a reader of the wire
/// see the same rubric.
pub const INSTRUCTIONS_HEAD: &str = "The conversation is about to be summarized. Given `goal` (the person's \
    last request) and `recent` (the assistant's newest words), will the summary still need to read";

/// One tool result the seat is asked about: where it sits in the plan, the
/// call that produced it, and the head of what it produced.
///
/// The call's input is carried WHOLE: the label that grades a drop reads a
/// path out of it later, and a path cut short is a path nothing matches. The
/// wire carries only its head ([`state`] cuts it, and the door cuts it again
/// at the row's cap).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHead {
    /// Its place among the candidates — the id its question answers under.
    pub position: usize,
    /// Where it sits in `plan.messages_to_compact`.
    pub message_index: usize,
    pub block_index: usize,
    pub tool_use_id: String,
    pub tool_name: String,
    /// The call's input, whole, as the assistant's `tool_use` carried it;
    /// empty when the call is not in the compaction set.
    pub input: String,
    /// The head of the result, cut at [`COMPACTION_BLOCK_HEAD_BYTE_CAP`].
    pub output_head: String,
    /// The whole result's size, in bytes — what a drop saves.
    pub output_bytes: usize,
}

impl BlockHead {
    /// The id this block's question answers under.
    #[must_use]
    pub fn question_id(&self) -> String {
        format!("{QUESTION_ID_PREFIX}{}", self.position)
    }
}

/// The block a question id names, by its place among the candidates, or
/// `None` when the id is not one this module wrote.
fn position_of(question_id: &str) -> Option<usize> {
    question_id.strip_prefix(QUESTION_ID_PREFIX)?.parse().ok()
}

/// What one compaction asks the seat: the turn's identity, the goal, the
/// newest words and every candidate block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionAsk {
    /// The turn the compaction runs inside, as the runtime names it.
    pub attempt: String,
    /// The person's last request — the goal the remaining work serves.
    pub goal: String,
    /// The assistant's newest words — where the work stands.
    pub recent: String,
    pub blocks: Vec<BlockHead>,
}

/// What the seat answered: the candidates to drop, by position, and whether
/// the seat's mode acts on them. A seat that answered nothing, or that is
/// only recording, hands back an empty drop list or `applies: false`, and
/// either leaves the plan exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompactionJudgment {
    pub dropped: Vec<usize>,
    pub applies: bool,
}

/// A seat beside compaction: asked once per boundary, on the runtime's own
/// task, and answered inside the wall its row names. The runtime never
/// records anything itself; the seat files its own row and its own labels.
pub trait CompactionSeat: Send + Sync {
    fn judge<'a>(&'a self, ask: &'a CompactionAsk) -> BoxFuture<'a, CompactionJudgment>;
}

/// The tool results a plan's compaction set holds that are worth a question:
/// a body of at least `min_output_bytes` that is not already a placeholder
/// and not a mutation record ([`is_edit_result_tool`] — an applied diff is
/// the one body the model cannot re-read, so it is never dropped, as it is
/// never cleared).
///
/// The input beside each result is the assistant's `tool_use` with the same
/// id, found in the same compaction set; a result whose call sits outside it
/// carries an empty input.
#[must_use]
pub fn candidates(plan: &CompactionPlan, min_output_bytes: usize) -> Vec<BlockHead> {
    let mut inputs: HashMap<&str, &str> = HashMap::new();
    for message in &plan.messages_to_compact {
        for block in &message.blocks {
            if let ContentBlock::ToolUse { id, input, .. } = block {
                inputs.insert(id.as_str(), input.as_str());
            }
        }
    }
    let mut found = Vec::new();
    for (message_index, message) in plan.messages_to_compact.iter().enumerate() {
        for (block_index, block) in message.blocks.iter().enumerate() {
            let ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                output,
                ..
            } = block
            else {
                continue;
            };
            if output == MICROCOMPACT_PLACEHOLDER
                || is_edit_result_tool(tool_name)
                || output.len() < min_output_bytes
            {
                continue;
            }
            found.push(BlockHead {
                position: found.len(),
                message_index,
                block_index,
                tool_use_id: tool_use_id.clone(),
                tool_name: tool_name.clone(),
                input: inputs
                    .get(tool_use_id.as_str())
                    .map(|input| (*input).to_string())
                    .unwrap_or_default(),
                output_head: cut(output, Cap::Bytes(COMPACTION_BLOCK_HEAD_BYTE_CAP)),
                output_bytes: output.len(),
            });
        }
    }
    found
}

/// The text a message carries, its text blocks joined — what a request or a
/// reply said, with no tool traffic in it.
fn spoken(message: &ConversationMessage) -> String {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The newest message of `role` that spoke — a user message that is only a
/// tool result, or an assistant message that only called tools, is not one.
fn newest_words(session: &Session, role: MessageRole) -> String {
    session
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == role)
        .map(spoken)
        .find(|words| !words.trim().is_empty())
        .unwrap_or_default()
}

/// What to ask the seat about `plan`, or `None` when there is nothing to
/// ask: no candidate block, or no request to judge them against.
///
/// The goal and the newest words are read off the whole session — tail
/// included — because they are the present the summary is being made for,
/// not the past it is being made of.
#[must_use]
pub fn ask_for(
    session: &Session,
    plan: &CompactionPlan,
    min_output_bytes: usize,
    attempt: &str,
) -> Option<CompactionAsk> {
    let blocks = candidates(plan, min_output_bytes);
    if blocks.is_empty() {
        return None;
    }
    let goal = newest_words(session, MessageRole::User);
    if goal.trim().is_empty() {
        return None;
    }
    Some(CompactionAsk {
        attempt: attempt.to_string(),
        goal,
        recent: newest_words(session, MessageRole::Assistant),
        blocks,
    })
}

/// The shards a compaction's blocks are asked in — even, none larger than
/// [`COMPACTION_SHARD_TARGET`] — as slices of the ask's own blocks.
#[must_use]
pub fn shards(blocks: &[BlockHead]) -> Vec<&[BlockHead]> {
    shard::even_shards(blocks.len(), COMPACTION_SHARD_TARGET)
        .into_iter()
        .map(|range| &blocks[range])
        .collect()
}

/// The one state every question in a shard reads: the heads and nothing
/// else. A result's body never leaves — the summary reads it, the judgment
/// only prices a line about it.
#[must_use]
pub fn state(ask: &CompactionAsk, shard: &[BlockHead]) -> Value {
    json!({
        "goal": cut(&ask.goal, Cap::Chars(COMPACTION_GOAL_CHAR_CAP)),
        "recent": cut(&ask.recent, Cap::Chars(COMPACTION_RECENT_CHAR_CAP)),
        "blocks": shard
            .iter()
            .map(|block| json!({
                "tool": block.tool_name,
                "input": cut(&block.input, Cap::Chars(COMPACTION_INPUT_CHAR_CAP)),
                "head": block.output_head,
            }))
            .collect::<Vec<_>>(),
    })
}

/// One question per block in the shard, by the id its answer comes back
/// under: a closed choice between [`COMPACTION_OPTIONS`].
///
/// The id is the block's place among ALL the candidates and the instructions
/// name its place in THIS shard's state — the offset a split batch needs, so
/// two shards' answers read into one judgment (`crate::skill_rank` keeps the
/// same discipline for the same reason).
#[must_use]
pub fn questions(shard: &[BlockHead]) -> BTreeMap<String, SystemOneQuestion> {
    let offset = shard.first().map_or(0, |first| first.position);
    shard
        .iter()
        .map(|block| {
            let at = block.position - offset;
            let instructions = format!("{INSTRUCTIONS_HEAD} `blocks[{at}]`?");
            (
                block.question_id(),
                SystemOneQuestion::choice(
                    &instructions,
                    [
                        (COMPACTION_KEEP, Some(KEEP_MEANS)),
                        (COMPACTION_DROP, Some(DROP_MEANS)),
                    ],
                ),
            )
        })
        .collect()
}

/// The words the rubric is made of — every sentence the judgment is shown —
/// in one string, so a fingerprint of it pins them.
#[must_use]
pub fn rubric_words() -> String {
    [
        INSTRUCTIONS_HEAD,
        COMPACTION_KEEP,
        KEEP_MEANS,
        COMPACTION_DROP,
        DROP_MEANS,
    ]
    .join("\n")
}

/// One block's reading, checked against the question that asked for it.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockReading {
    pub position: usize,
    /// The option the judgment chose.
    pub chosen: String,
    /// The probability it put on dropping.
    pub drop_probability: f64,
    pub confidence: f64,
}

impl BlockReading {
    /// Whether this reading takes the block out of the summary's input: the
    /// judgment chose `drop`, and put at least the table's lean on it
    /// ([`COMPACTION_DROP_FLOOR_PERMILLE`]). Compared in the line's own units,
    /// so the answer does not turn on a float's last bit.
    #[must_use]
    pub fn drops(&self) -> bool {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let permille = (self.drop_probability * 1000.0).floor() as u64;
        self.chosen == COMPACTION_DROP && permille >= u64::from(COMPACTION_DROP_FLOOR_PERMILLE)
    }
}

/// Why a reply was thrown away. Every one of these discards that SHARD's
/// judgment whole — a shard half-read would drop some blocks by the model
/// and keep the rest by nothing at all — and every one keeps its blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionRejection {
    /// An answer came back under an id nothing was asked under.
    UnknownAnswer(String),
    /// One of the closed choice's rules refused an answer.
    Answer(String, ChoiceRefusal),
}

impl CompactionRejection {
    /// The rule's own word, as a ledger row spells it.
    #[must_use]
    pub const fn rule(&self) -> &'static str {
        match self {
            Self::UnknownAnswer(_) => "unknown_answer",
            Self::Answer(_, refusal) => refusal.token(),
        }
    }

    /// Which block the rule broke on, by its place among the candidates. An
    /// unknown answer names none: what the reply called its answer is the
    /// model's word and names no block of ours.
    #[must_use]
    pub fn position(&self) -> Option<usize> {
        match self {
            Self::UnknownAnswer(_) => None,
            Self::Answer(id, _) => position_of(id),
        }
    }
}

/// Check one shard's reply against the questions [`questions`] asked.
///
/// # Errors
/// The first rule the reply breaks.
pub fn validate(
    shard: &[BlockHead],
    response: &SystemOneResponse,
) -> Result<Vec<BlockReading>, CompactionRejection> {
    let asked: BTreeSet<String> = shard.iter().map(BlockHead::question_id).collect();
    if let Some(stray) = response.answers.keys().find(|id| !asked.contains(*id)) {
        return Err(CompactionRejection::UnknownAnswer(stray.clone()));
    }
    let offered: BTreeSet<String> = COMPACTION_OPTIONS.iter().map(|word| (*word).to_string()).collect();
    let answers = serde_json::to_value(&response.answers).unwrap_or(Value::Null);
    shard
        .iter()
        .map(|block| {
            let id = block.question_id();
            let read = choice::read(&answers, &id, &offered)
                .map_err(|refusal| CompactionRejection::Answer(id, refusal))?;
            Ok(BlockReading {
                position: block.position,
                drop_probability: read.probabilities.get(COMPACTION_DROP).copied().unwrap_or(0.0),
                chosen: read.chosen,
                confidence: read.confidence,
            })
        })
        .collect()
}

/// Take `dropped` — positions into `blocks` — out of the plan's compaction
/// set: seal each touched message's original to the vault, then put the
/// microcompact placeholder where the block's body was. Answers how many
/// blocks left; zero when the seal did not land, in which case the plan is
/// untouched, because a body whose only copy is in hand is not one to blank.
///
/// `session` is the LIVE session the plan was prepared from: its
/// `first_message_index` and the plan's `cache_prefix` length are what put a
/// plan index on the vault's seq axis — the same arithmetic
/// `apply_compaction` uses to heal the placeholders back before it seals the
/// evicted originals.
pub fn drop_blocks(
    plan: &mut CompactionPlan,
    session: &Session,
    blocks: &[BlockHead],
    dropped: &[usize],
) -> usize {
    let chosen: Vec<&BlockHead> = dropped
        .iter()
        .filter_map(|position| blocks.iter().find(|block| block.position == *position))
        .collect();
    if chosen.is_empty() {
        return 0;
    }
    let mut touched: Vec<usize> = chosen.iter().map(|block| block.message_index).collect();
    touched.sort_unstable();
    touched.dedup();
    let base_seq = session
        .first_message_index()
        .saturating_add(u32::try_from(plan.cache_prefix.len()).unwrap_or(u32::MAX));
    if !super::seal_originals_of(session, &plan.messages_to_compact, base_seq, &touched) {
        return 0;
    }
    let mut left = 0;
    for block in chosen {
        let Some(message) = plan.messages_to_compact.get_mut(block.message_index) else {
            continue;
        };
        if let Some(ContentBlock::ToolResult {
            tool_use_id,
            output,
            images,
            ..
        }) = message.blocks.get_mut(block.block_index)
        {
            if *tool_use_id == block.tool_use_id && *output != MICROCOMPACT_PLACEHOLDER {
                *output = MICROCOMPACT_PLACEHOLDER.to_string();
                images.clear();
                left += 1;
            }
        }
    }
    left
}

/// The tokens the summary request reads for `messages`, as the request
/// pre-trims them (`pretrim_messages_for_summary`) and the
/// transcript estimate counts them — the one unit a before and an after are
/// quoted in.
#[must_use]
pub fn summary_input_tokens(messages: &[ConversationMessage]) -> usize {
    super::pretrim_messages_for_summary(messages)
        .iter()
        .map(super::estimate_message_tokens)
        .sum()
}

/// What a judged compaction did to its plan, for a caller that reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Judged {
    /// Blocks the seat was asked about.
    pub asked: usize,
    /// Blocks that left the summary's input.
    pub dropped: usize,
}

/// Put `plan` to `seat` and hand back the plan the summary reads: the same
/// plan when there was nothing to ask, when the seat answered nothing, when
/// its mode only records, or when the seal did not land; otherwise the plan
/// with the dropped blocks' bodies replaced by the placeholder.
pub async fn judge_plan(
    seat: &dyn CompactionSeat,
    session: &Session,
    mut plan: CompactionPlan,
    min_output_bytes: usize,
    attempt: &str,
) -> (CompactionPlan, Judged) {
    let Some(ask) = ask_for(session, &plan, min_output_bytes, attempt) else {
        return (plan, Judged::default());
    };
    let asked = ask.blocks.len();
    let judgment = seat.judge(&ask).await;
    let dropped = if judgment.applies {
        drop_blocks(&mut plan, session, &ask.blocks, &judgment.dropped)
    } else {
        0
    };
    (plan, Judged { asked, dropped })
}

#[cfg(test)]
mod tests;
