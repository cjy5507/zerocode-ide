//! Conversation content value objects: the speaker [`MessageRole`], the
//! structured [`ContentBlock`] variants, and the [`ConversationMessage`] that
//! groups blocks with optional token-usage metadata — plus their JSON
//! (de)serialization.

use std::collections::BTreeMap;

use crate::json::JsonValue;
use crate::usage::{OutputTokensDetails, TokenUsage};

use super::SessionError;
use super::json_field::{required_string, required_u32};

/// The tag every harness reminder carries. Matched by CONTAINMENT rather than
/// prefix: most blocks get the wrapper from the runtime's `wrap_reminder`, but
/// some producers embed the tag after their own prefix line (the
/// `UserPromptSubmit` hook context).
pub const REMINDER_TAG_OPEN: &str = "<system-reminder>";

/// Replacement text microcompact leaves where a byte-identical later copy has
/// superseded a persisted reminder block.
///
/// Defined here rather than in the compaction tier that writes it because the
/// consumers that must recognize a cleared block live outside that tier — see
/// [`is_reminder_lineage_text`].
pub const CLEARED_REMINDER_PLACEHOLDER: &str = "[Superseded reminder cleared]";

/// Whether a `System` text block is reminder MACHINERY — a live reminder, or
/// the placeholder microcompact left where one was — rather than transcript
/// content a reader is meant to see.
///
/// Two questions used to share one containment test, and they pull in opposite
/// directions:
///
/// - *"May compaction clear this block?"* — tag only. The placeholder must NOT
///   answer yes, or a cleared block is a candidate again on the next pass and
///   clearing stops being idempotent. That is the runtime's
///   `is_persisted_reminder_text`, and it stays a bare tag test.
/// - *"Is this block reminder machinery?"* — this predicate, which the
///   placeholder MUST answer yes to. Reminders were never rendered live, so
///   clearing one is a context-reclamation detail that cannot be allowed to
///   promote invisible context into a visible transcript row on resume, nor to
///   make history surgery ([`super::Session::rewind_turns`]) stop treating the
///   block as part of the turn it annotates.
///
/// Hanging both on one predicate is what made them mutually exclusive: the
/// property bought for idempotence was the same property that hid the block.
#[must_use]
pub fn is_reminder_lineage_text(text: &str) -> bool {
    text.contains(REMINDER_TAG_OPEN) || text == CLEARED_REMINDER_PLACEHOLDER
}

/// Speaker role associated with a persisted conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// Structured message content stored inside a [`Session`](super::Session).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        tool_name: String,
        output: String,
        is_error: bool,
        /// Out-of-band images (`media_type`, base64) a tool produced for the
        /// model to *see*. Empty for text-only tools.
        images: Vec<(String, String)>,
    },
    Image {
        media_type: String,
        data: String,
    },
    /// A stored Anthropic reasoning block (extended / interleaved thinking),
    /// captured so it can be **replayed verbatim** on the next Anthropic
    /// request — the API rejects a modified thinking block and 400s if one is
    /// mis-ordered, but tolerates omission. `signature` is empty only for legacy
    /// data captured before signatures were stored; the lowering seam
    /// (`convert_blocks`) drops an unsigned block rather than send it. Provider-
    /// opaque like [`ConversationMessage::thought_signature`]: only the Anthropic
    /// wire encoder lowers it, so it can never leak into an OpenAI/Gemini request.
    Thinking {
        thinking: String,
        signature: String,
    },
    /// A stored Anthropic `redacted_thinking` block — encrypted reasoning the API
    /// returns in place of readable thinking. `data` is opaque (base64-ish) and
    /// replayed verbatim on the Anthropic path only; it carries no signature.
    RedactedThinking {
        data: String,
    },
}

/// One conversation message with optional token-usage metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
    pub role: MessageRole,
    pub blocks: Vec<ContentBlock>,
    pub usage: Option<TokenUsage>,
    /// Opaque per-turn provider reasoning signature (e.g. Gemini 3's
    /// `thoughtSignature`). Stored on the assistant turn so a follow-up request
    /// to the *same* provider can echo it back — Gemini 3 rejects multi-turn
    /// tool calls whose prior `functionCall` is missing its signature. It is
    /// provider-opaque session state: only the originating provider's wire
    /// encoder reads it, so switching to Claude/GPT silently drops it (their
    /// encoders never look at this field) and it never leaks across providers.
    pub thought_signature: Option<String>,
    /// Provider-opaque reasoning-replay payload for this assistant turn (the
    /// ChatGPT/Codex Responses backend's reasoning items that must be echoed
    /// back before each `function_call` to keep multi-turn reasoning
    /// continuity — see `api::providers::chatgpt_backend`). Shape:
    /// `[{"call_id": "...", "items": [<reasoning item JSON>, ...]}, ...]`,
    /// one entry per tool call this turn made. Same isolation as
    /// [`Self::thought_signature`]: only the originating provider's wire
    /// encoder reads it, so it never leaks across providers.
    pub reasoning_replay: Option<serde_json::Value>,
    /// Wire model id that produced this assistant turn (as reported by the
    /// provider response, e.g. `claude-fable-5` / `gpt-5.6-sol`), for cost
    /// attribution. Smart-routed sessions interleave models turn-by-turn —
    /// and quota/refusal fallbacks can swap mid-turn — so without this stamp
    /// a session ledger cannot say which model billed which `usage` record
    /// (the exact forensic gap that slowed the 2026-07 cache-leak hunt).
    /// `None` on user/tool messages and on history from before this field.
    pub model: Option<String>,
}

impl ConversationMessage {
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        }
    }

    /// Build a user message with optional image attachments followed by text.
    #[must_use]
    pub fn user_with_images(text: impl Into<String>, images: Vec<(String, String)>) -> Self {
        let mut blocks: Vec<ContentBlock> = images
            .into_iter()
            .map(|(media_type, data)| ContentBlock::Image { media_type, data })
            .collect();
        blocks.push(ContentBlock::Text { text: text.into() });
        Self {
            role: MessageRole::User,
            blocks,
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        }
    }

    #[must_use]
    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        }
    }

    #[must_use]
    pub fn assistant_with_usage(blocks: Vec<ContentBlock>, usage: Option<TokenUsage>) -> Self {
        Self {
            role: MessageRole::Assistant,
            blocks,
            usage,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        }
    }

    /// Attach a provider reasoning signature (e.g. Gemini `thoughtSignature`)
    /// to this turn. Builder form so the assistant constructors stay unchanged
    /// and callers without a signature pay nothing. See [`Self::thought_signature`].
    #[must_use]
    pub fn with_thought_signature(mut self, signature: Option<String>) -> Self {
        self.thought_signature = signature;
        self
    }

    /// Attach the ChatGPT/Codex reasoning-replay payload to this turn. Builder
    /// form so the assistant constructors stay unchanged and callers without a
    /// payload pay nothing. See [`Self::reasoning_replay`].
    #[must_use]
    pub fn with_reasoning_replay(mut self, reasoning_replay: Option<serde_json::Value>) -> Self {
        self.reasoning_replay = reasoning_replay;
        self
    }

    /// Attach the wire model id that produced this turn. Builder form like
    /// [`Self::with_thought_signature`]. See [`Self::model`].
    #[must_use]
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    /// Whether this message is a persisted harness-reminder annotation: a
    /// `System` message whose blocks are all reminder machinery
    /// ([`is_reminder_lineage_text`]). The runtime persists each request's
    /// reminder set in this shape so historical renders stay byte-identical
    /// (prefix-cache safety). Distinct from other `System` transcript content —
    /// compaction summaries are neither tagged nor cleared reminders — so
    /// history surgery (e.g. [`super::Session::rewind_turns`]) can treat
    /// reminders as part of the turn they annotate without touching summaries.
    ///
    /// It asks for LINEAGE, not the live tag: a microcompacted block is still
    /// the annotation it always was, and a rewind that stopped at one would
    /// burn a step removing nothing.
    #[must_use]
    pub fn is_reminder_annotation(&self) -> bool {
        self.role == MessageRole::System
            && !self.blocks.is_empty()
            && self.blocks.iter().all(|block| {
                matches!(block, ContentBlock::Text { text } if is_reminder_lineage_text(text))
            })
    }

    #[must_use]
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                tool_name: tool_name.into(),
                output: output.into(),
                is_error,
                images: Vec::new(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        }
    }

    /// Like [`Self::tool_result`] but carries out-of-band images the tool
    /// produced (e.g. `read_image`), so the model sees them on the next turn.
    #[must_use]
    pub fn tool_result_with_images(
        tool_use_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
        images: Vec<(String, String)>,
    ) -> Self {
        Self {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                tool_name: tool_name.into(),
                output: output.into(),
                is_error,
                images,
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        }
    }

    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        object.insert(
            "role".to_string(),
            JsonValue::String(
                match self.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                }
                .to_string(),
            ),
        );
        object.insert(
            "blocks".to_string(),
            JsonValue::Array(self.blocks.iter().map(ContentBlock::to_json).collect()),
        );
        if let Some(usage) = self.usage {
            object.insert("usage".to_string(), usage_to_json(usage));
        }
        if let Some(signature) = &self.thought_signature {
            object.insert(
                "thought_signature".to_string(),
                JsonValue::String(signature.clone()),
            );
        }
        // Stored as raw JSON text inside the hand-rolled session `JsonValue`
        // tree (same pattern as `ContentBlock::ToolUse.input`), since
        // `reasoning_replay` is an arbitrary provider JSON shape that the
        // session's minimal JSON model (integer-only numbers) cannot
        // losslessly represent as a decomposed tree.
        if let Some(reasoning_replay) = &self.reasoning_replay {
            object.insert(
                "reasoning_replay".to_string(),
                JsonValue::String(reasoning_replay.to_string()),
            );
        }
        if let Some(model) = &self.model {
            object.insert("model".to_string(), JsonValue::String(model.clone()));
        }
        JsonValue::Object(object)
    }

    pub(super) fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("message must be an object".to_string()))?;
        let role = match object
            .get("role")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| SessionError::Format("missing role".to_string()))?
        {
            "system" => MessageRole::System,
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "tool" => MessageRole::Tool,
            other => {
                return Err(SessionError::Format(format!(
                    "unsupported message role: {other}"
                )));
            }
        };
        let blocks = object
            .get("blocks")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| SessionError::Format("missing blocks".to_string()))?
            .iter()
            .map(ContentBlock::from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let usage = object.get("usage").map(usage_from_json).transpose()?;
        let thought_signature = object
            .get("thought_signature")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned);
        // Absent in sessions written before this field existed (back-compat);
        // a value that fails to parse as JSON is treated the same way rather
        // than failing the whole session load.
        let reasoning_replay = object
            .get("reasoning_replay")
            .and_then(JsonValue::as_str)
            .and_then(|raw| serde_json::from_str(raw).ok());
        // Absent in sessions written before this field existed (back-compat).
        let model = object
            .get("model")
            .and_then(JsonValue::as_str)
            .map(ToOwned::to_owned);
        Ok(Self {
            role,
            blocks,
            usage,
            thought_signature,
            reasoning_replay,
            model,
        })
    }
}

impl ContentBlock {
    #[must_use]
    pub fn to_json(&self) -> JsonValue {
        let mut object = BTreeMap::new();
        match self {
            Self::Text { text } => {
                object.insert("type".to_string(), JsonValue::String("text".to_string()));
                object.insert("text".to_string(), JsonValue::String(text.clone()));
            }
            Self::ToolUse { id, name, input } => {
                object.insert(
                    "type".to_string(),
                    JsonValue::String("tool_use".to_string()),
                );
                object.insert("id".to_string(), JsonValue::String(id.clone()));
                object.insert("name".to_string(), JsonValue::String(name.clone()));
                object.insert("input".to_string(), JsonValue::String(input.clone()));
            }
            Self::ToolResult {
                tool_use_id,
                tool_name,
                output,
                is_error,
                images,
            } => {
                object.insert(
                    "type".to_string(),
                    JsonValue::String("tool_result".to_string()),
                );
                object.insert(
                    "tool_use_id".to_string(),
                    JsonValue::String(tool_use_id.clone()),
                );
                object.insert(
                    "tool_name".to_string(),
                    JsonValue::String(tool_name.clone()),
                );
                object.insert("output".to_string(), JsonValue::String(output.clone()));
                object.insert("is_error".to_string(), JsonValue::Bool(*is_error));
                // Only emit `images` when present, so text-only tool results
                // keep their existing on-disk JSON byte-for-byte.
                if !images.is_empty() {
                    let array = images
                        .iter()
                        .map(|(media_type, data)| {
                            let mut image = BTreeMap::new();
                            image.insert(
                                "media_type".to_string(),
                                JsonValue::String(media_type.clone()),
                            );
                            image.insert("data".to_string(), JsonValue::String(data.clone()));
                            JsonValue::Object(image)
                        })
                        .collect();
                    object.insert("images".to_string(), JsonValue::Array(array));
                }
            }
            Self::Image { media_type, data } => {
                object.insert("type".to_string(), JsonValue::String("image".to_string()));
                let mut source = BTreeMap::new();
                source.insert("type".to_string(), JsonValue::String("base64".to_string()));
                source.insert(
                    "media_type".to_string(),
                    JsonValue::String(media_type.clone()),
                );
                source.insert("data".to_string(), JsonValue::String(data.clone()));
                object.insert("source".to_string(), JsonValue::Object(source));
            }
            Self::Thinking { thinking, signature } => {
                object.insert(
                    "type".to_string(),
                    JsonValue::String("thinking".to_string()),
                );
                object.insert("thinking".to_string(), JsonValue::String(thinking.clone()));
                object.insert(
                    "signature".to_string(),
                    JsonValue::String(signature.clone()),
                );
            }
            Self::RedactedThinking { data } => {
                object.insert(
                    "type".to_string(),
                    JsonValue::String("redacted_thinking".to_string()),
                );
                object.insert("data".to_string(), JsonValue::String(data.clone()));
            }
        }
        JsonValue::Object(object)
    }

    pub(super) fn from_json(value: &JsonValue) -> Result<Self, SessionError> {
        let object = value
            .as_object()
            .ok_or_else(|| SessionError::Format("block must be an object".to_string()))?;
        match object
            .get("type")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| SessionError::Format("missing block type".to_string()))?
        {
            "text" => Ok(Self::Text {
                text: required_string(object, "text")?,
            }),
            "tool_use" => Ok(Self::ToolUse {
                id: required_string(object, "id")?,
                name: required_string(object, "name")?,
                input: required_string(object, "input")?,
            }),
            "tool_result" => Ok(Self::ToolResult {
                tool_use_id: required_string(object, "tool_use_id")?,
                tool_name: required_string(object, "tool_name")?,
                output: required_string(object, "output")?,
                is_error: object
                    .get("is_error")
                    .and_then(JsonValue::as_bool)
                    .ok_or_else(|| SessionError::Format("missing is_error".to_string()))?,
                // Absent in older sessions → empty (back-compat).
                images: object
                    .get("images")
                    .and_then(JsonValue::as_array)
                    .map(|array| {
                        array
                            .iter()
                            .filter_map(|entry| {
                                let image = entry.as_object()?;
                                Some((
                                    image
                                        .get("media_type")
                                        .and_then(JsonValue::as_str)?
                                        .to_string(),
                                    image.get("data").and_then(JsonValue::as_str)?.to_string(),
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            }),
            "image" => {
                let source = object
                    .get("source")
                    .and_then(JsonValue::as_object)
                    .ok_or_else(|| SessionError::Format("missing image source".to_string()))?;
                Ok(Self::Image {
                    media_type: source
                        .get("media_type")
                        .and_then(JsonValue::as_str)
                        .ok_or_else(|| {
                            SessionError::Format("missing image media_type".to_string())
                        })?
                        .to_string(),
                    data: source
                        .get("data")
                        .and_then(JsonValue::as_str)
                        .ok_or_else(|| SessionError::Format("missing image data".to_string()))?
                        .to_string(),
                })
            }
            // Additive: sessions written before thinking was stored simply carry
            // no `thinking`/`redacted_thinking` blocks, so older on-disk data
            // still deserializes unchanged.
            "thinking" => Ok(Self::Thinking {
                thinking: required_string(object, "thinking")?,
                signature: required_string(object, "signature")?,
            }),
            "redacted_thinking" => Ok(Self::RedactedThinking {
                data: required_string(object, "data")?,
            }),
            other => Err(SessionError::Format(format!(
                "unsupported block type: {other}"
            ))),
        }
    }
}

fn usage_to_json(usage: TokenUsage) -> JsonValue {
    let mut object = BTreeMap::new();
    object.insert(
        "input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.input_tokens)),
    );
    object.insert(
        "output_tokens".to_string(),
        JsonValue::Number(i64::from(usage.output_tokens)),
    );
    object.insert(
        "cache_creation_input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.cache_creation_input_tokens)),
    );
    object.insert(
        "cache_read_input_tokens".to_string(),
        JsonValue::Number(i64::from(usage.cache_read_input_tokens)),
    );
    if let Some(details) = usage.output_tokens_details {
        let mut details_object = BTreeMap::new();
        details_object.insert(
            "thinking_tokens".to_string(),
            JsonValue::Number(i64::from(details.thinking_tokens)),
        );
        object.insert(
            "output_tokens_details".to_string(),
            JsonValue::Object(details_object),
        );
    }
    JsonValue::Object(object)
}

fn usage_from_json(value: &JsonValue) -> Result<TokenUsage, SessionError> {
    let object = value
        .as_object()
        .ok_or_else(|| SessionError::Format("usage must be an object".to_string()))?;
    let output_tokens_details = object
        .get("output_tokens_details")
        .map(|value| -> Result<OutputTokensDetails, SessionError> {
            let details = value.as_object().ok_or_else(|| {
                SessionError::Format("output_tokens_details must be an object".to_string())
            })?;
            Ok(OutputTokensDetails {
                thinking_tokens: required_u32(details, "thinking_tokens")?,
            })
        })
        .transpose()?;
    Ok(TokenUsage {
        input_tokens: required_u32(object, "input_tokens")?,
        output_tokens: required_u32(object, "output_tokens")?,
        cache_creation_input_tokens: required_u32(object, "cache_creation_input_tokens")?,
        cache_read_input_tokens: required_u32(object, "cache_read_input_tokens")?,
        output_tokens_details,
    })
}

#[cfg(test)]
mod thinking_block_tests {
    use super::{ContentBlock, ConversationMessage};
    use crate::json::JsonValue;

    #[test]
    fn thinking_and_redacted_blocks_round_trip_through_json() {
        let thinking = ContentBlock::Thinking {
            thinking: "step-by-step reasoning".to_string(),
            signature: "SIG-abc123".to_string(),
        };
        assert_eq!(
            ContentBlock::from_json(&thinking.to_json()).expect("parse thinking"),
            thinking
        );

        let redacted = ContentBlock::RedactedThinking {
            data: "ENCRYPTED_BLOB".to_string(),
        };
        assert_eq!(
            ContentBlock::from_json(&redacted.to_json()).expect("parse redacted"),
            redacted
        );
    }

    #[test]
    fn thinking_block_json_uses_anthropic_type_tags() {
        let JsonValue::Object(object) = (ContentBlock::Thinking {
            thinking: "t".to_string(),
            signature: "s".to_string(),
        })
        .to_json() else {
            panic!("expected a JSON object");
        };
        assert_eq!(
            object.get("type").and_then(JsonValue::as_str),
            Some("thinking")
        );
        assert_eq!(
            object.get("signature").and_then(JsonValue::as_str),
            Some("s")
        );
    }

    /// A session written before thinking was stored (text + `tool_use` only) still
    /// deserializes under the new schema — the additive `type` arms don't disturb
    /// existing block types (serde/back-compat).
    #[test]
    fn legacy_message_without_thinking_still_deserializes() {
        let legacy = ConversationMessage::assistant(vec![
            ContentBlock::Text {
                text: "hi".to_string(),
            },
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            },
        ]);
        let restored =
            ConversationMessage::from_json(&legacy.to_json()).expect("parse legacy message");
        assert_eq!(restored, legacy);
        assert!(restored.blocks.iter().all(|block| !matches!(
            block,
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }
        )));
    }

    /// A full assistant turn stores thinking BEFORE text/`tool_use` and survives a
    /// JSON round-trip in that order — the order the Anthropic API validates when
    /// the block is replayed.
    #[test]
    fn thinking_leads_turn_and_survives_round_trip() {
        let message = ConversationMessage::assistant(vec![
            ContentBlock::Thinking {
                thinking: "reason".to_string(),
                signature: "SIG".to_string(),
            },
            ContentBlock::Text {
                text: "answer".to_string(),
            },
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            },
        ]);
        let restored = ConversationMessage::from_json(&message.to_json()).expect("round trip");
        assert_eq!(restored, message);
        assert!(matches!(restored.blocks[0], ContentBlock::Thinking { .. }));
    }
}

#[cfg(test)]
mod reasoning_replay_tests {
    use super::{ContentBlock, ConversationMessage};
    use crate::json::JsonValue;

    /// A `reasoning_replay` payload survives a JSON round-trip byte-for-byte
    /// (as a decoded value, since the session's hand-rolled `JsonValue` stores
    /// it as embedded JSON text — see `ConversationMessage::to_json`).
    #[test]
    fn reasoning_replay_round_trips_through_json() {
        let message = ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "bash".to_string(),
            input: "{}".to_string(),
        }])
        .with_reasoning_replay(Some(serde_json::json!([
            {"call_id": "t1", "items": [
                {"type": "reasoning", "id": "rs_1", "encrypted_content": "OPAQUE"}
            ]}
        ])));

        let restored = ConversationMessage::from_json(&message.to_json()).expect("round trip");
        assert_eq!(restored, message);
        assert_eq!(restored.reasoning_replay, message.reasoning_replay);
    }

    /// The turn-boundary extension of the payload — an extra entry keyed
    /// `turn_final` instead of `call_id`, carrying the reasoning that preceded
    /// the turn's final text — round-trips through the session file unchanged,
    /// so a resumed session still replays it (the ChatGPT encoder's
    /// `turn_final_reasoning_replay` reads exactly this entry). Shape-specific
    /// on purpose: the schema is additive and this pins the additive entry.
    #[test]
    fn turn_final_reasoning_replay_entry_round_trips_through_json() {
        let payload = serde_json::json!([
            {"call_id": "t1", "items": [{"type": "reasoning", "id": "rs_1"}]},
            {"turn_final": true, "items": [
                {"type": "reasoning", "id": "rs_2", "encrypted_content": "OPAQUE"}
            ]}
        ]);
        let message = ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "here is why".to_string(),
        }])
        .with_reasoning_replay(Some(payload.clone()));

        let restored = ConversationMessage::from_json(&message.to_json()).expect("round trip");
        assert_eq!(restored.reasoning_replay, Some(payload));
    }

    /// A session written before `reasoning_replay` existed carries no such key
    /// at all in its on-disk JSON; it must still load, with the field
    /// defaulting to `None` (back-compat, mirrors the `thinking`/`images`
    /// additive fields above).
    #[test]
    fn legacy_message_without_reasoning_replay_still_deserializes() {
        let legacy = ConversationMessage::assistant(vec![ContentBlock::Text {
            text: "hi".to_string(),
        }]);
        let legacy_json = legacy.to_json();
        let JsonValue::Object(object) = &legacy_json else {
            panic!("expected a JSON object");
        };
        assert!(
            !object.contains_key("reasoning_replay"),
            "a message with no payload must not emit the key at all"
        );

        let restored =
            ConversationMessage::from_json(&legacy_json).expect("legacy message still loads");
        assert_eq!(restored, legacy);
        assert!(restored.reasoning_replay.is_none());
    }

    /// A `reasoning_replay` value that fails to parse as JSON (corrupt/foreign
    /// data) degrades to `None` rather than failing the whole session load —
    /// the same tolerance the rest of this loader gives malformed optional
    /// fields.
    #[test]
    fn unparseable_reasoning_replay_value_degrades_to_none() {
        use std::collections::BTreeMap;
        let mut object = BTreeMap::new();
        object.insert("role".to_string(), JsonValue::String("assistant".to_string()));
        object.insert("blocks".to_string(), JsonValue::Array(vec![]));
        object.insert(
            "reasoning_replay".to_string(),
            JsonValue::String("not valid json{".to_string()),
        );
        let restored = ConversationMessage::from_json(&JsonValue::Object(object))
            .expect("malformed reasoning_replay must not fail the whole message");
        assert!(restored.reasoning_replay.is_none());
    }
}
