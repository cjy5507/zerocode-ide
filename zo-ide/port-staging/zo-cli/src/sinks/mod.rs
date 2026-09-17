//! Output sinks for non-interactive CLI runs.
//!
//! A [`Sink`] consumes [`runtime::RenderBlock`] values and writes them
//! to an output surface. Sinks are the L4 boundary between the
//! conversation runtime (which produces blocks) and the user's terminal
//! or a machine-readable stream.
//!
//! ## Living standard (L1 mirror)
//!
//! 1. Module layout — this directory has `mod.rs` plus one file per
//!    concrete sink (`ndjson.rs`, `json.rs`, `text.rs`).
//! 2. Errors — one [`SinkError`] `thiserror` enum for the whole module,
//!    no `anyhow`.
//! 3. Async — the [`Sink`] trait is synchronous; all writes go through
//!    `std::io::Write` so callers pick the concrete runtime.
//! 4. Tests — integration tests live at
//!    `tests/ndjson_sink.rs` and follow the `<area>_<scenario>` naming
//!    rule.
//! 5. Docs — every public item carries a `///` doc comment.

use std::io;

use runtime::message_stream::RenderBlock;

pub mod json;
pub mod ndjson;
pub mod text;

pub use json::JsonSink;
pub use ndjson::NdjsonSink;
/// Decode a wire permission-decision tag (`allow_once` / … / `deny_always`)
/// into a render-block [`runtime::message_stream::PermissionDecision`]. Shared
/// by the wire serializer and the `zo serve` `permission.respond` handler.
pub use serializable::permission_decision_from_tag;
/// Encode a render-block [`runtime::message_stream::PermissionDecision`] into
/// its wire tag. Shared by the wire serializer and the `zo attach` client's
/// `permission.respond` sender — one decision vocabulary across the socket.
pub use serializable::permission_decision_tag;
/// The wire projection of a render frame — the exact JSON `zo serve` streams
/// over the socket. Re-exported so the `zo serve` `session.run_turn_detached`
/// job buffer can hold projected frames (`from_block`) and replay them through
/// `session.job_result`, byte-identical to the live stream.
pub use serializable::SerializableRenderBlock;
pub use text::TextSink;

/// Errors produced by [`Sink`] implementations.
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    /// The underlying writer returned an I/O error.
    #[error("sink io error: {0}")]
    Io(#[from] io::Error),
    /// JSON serialization failed (only produced by JSON-shaped sinks).
    #[error("sink json error: {0}")]
    Json(#[from] serde_json::Error),
    /// The sink has already been finalized and cannot accept more data.
    #[error("sink already finalized")]
    AlreadyFinalized,
}

/// Trait implemented by every non-interactive output sink.
///
/// Sinks receive [`RenderBlock`]s one at a time via [`Sink::emit`] and
/// flush any buffered state when [`Sink::finalize`] is called. Dropping
/// a sink without calling `finalize` is allowed but may leave buffered
/// state unflushed.
pub trait Sink {
    /// Emit a single [`RenderBlock`].
    ///
    /// # Errors
    /// Returns [`SinkError::Io`] if the underlying writer fails, or
    /// [`SinkError::Json`] if serialization fails for JSON-shaped sinks.
    fn emit(&mut self, block: &RenderBlock) -> Result<(), SinkError>;

    /// Flush and finalize the sink, consuming it.
    ///
    /// After `finalize` returns, the sink is done and cannot be reused.
    ///
    /// # Errors
    /// Returns [`SinkError::Io`] if the final flush fails, or
    /// [`SinkError::Json`] if buffered JSON cannot be serialized.
    fn finalize(self: Box<Self>) -> Result<(), SinkError>;
}

/// Serializable projection of a [`RenderBlock`] used by JSON-shaped
/// sinks.
///
/// Kept in the sinks module (not in `runtime::message_stream::types`)
/// so runtime does not gain a serde contract it does not own. The shape
/// mirrors the Claude Code `stream-json` contract: one object per line
/// with a `type` tag and variant-specific fields.
pub(crate) mod serializable {
    use runtime::message_stream::{
        AgentResultStatus, BlockId, PermissionChoice, PermissionDecision, PermissionPrompt,
        ProjectedRenderBlock, RenderBlock, SystemLevel, ToolCallId, ToolCallStatus, ToolPreview,
        ToolResultBody, project_render_block,
    };
    use runtime::usage::{RateLimitSnapshot, RateLimitWindow, TokenUsage};
    use serde::{Deserialize, Serialize};

    /// Owned, (de)serializable snapshot of a [`RenderBlock`].
    ///
    /// `Deserialize` makes this the symmetric wire vocabulary for `zo serve`
    /// / `zo attach`: the server serializes blocks with [`Self::from_block`],
    /// the client reconstructs renderable blocks with [`to_render_block`]. The
    /// projection is intentionally lossy (a *presentation* form, not a clone),
    /// so reconstruction rebuilds the minimal variant that renders correctly.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum SerializableRenderBlock {
        /// Assistant text delta.
        TextDelta {
            /// Stable block id.
            id: u64,
            /// Text chunk.
            text: String,
            /// Whether the block has finished streaming.
            done: bool,
        },
        /// Assistant reasoning / chain-of-thought delta.
        Reasoning {
            /// Stable block id.
            id: u64,
            /// Reasoning chunk.
            text: String,
            /// Optional opaque signature.
            signature: Option<String>,
            /// Whether the block has finished streaming.
            done: bool,
        },
        /// Tool call lifecycle event.
        ToolCall {
            /// Stable block id.
            id: u64,
            /// Provider-neutral tool call id.
            tool_call_id: String,
            /// Canonical tool name.
            name: String,
            /// One-line summary of the tool input.
            summary: String,
            /// Lifecycle status tag (`pending`/`running`/`ok`/`errored`/`cancelled`).
            status: String,
        },
        /// Tool result event.
        ToolResult {
            /// Stable block id.
            id: u64,
            /// Provider-neutral tool call id.
            tool_call_id: String,
            /// Whether the tool returned an error.
            is_error: bool,
            /// Best-effort textual preview of the result body.
            content: String,
        },
        /// System notice (banner, divider, error).
        System {
            /// Stable block id.
            id: u64,
            /// Severity tag (`info`, `warn`, `error`, or `user` for a folded
            /// user message).
            level: String,
            /// Display text.
            text: String,
        },
        /// Finished background sub-agent result — a first-class wire frame so an
        /// attached `zo attach` client renders the same collapsible
        /// agent-result card as the local TUI, instead of a raw system wall.
        AgentResult {
            /// Stable block id.
            id: u64,
            /// Sub-agent display label.
            label: String,
            /// Completion status tag (`completed` | `failed`).
            status: String,
            /// The agent's raw result markdown (shown when expanded).
            body: String,
        },
        /// Inline image attachment.
        Image {
            /// Stable block id.
            id: u64,
            /// MIME type, e.g. `"image/png"`.
            media_type: String,
            /// Byte length of the raw image data.
            byte_len: usize,
        },
        /// Permission prompt marker.
        ///
        /// Carries enough to drive a **live** modal on an attached `zo
        /// attach` client (F2): `prompt_id` routes the client's
        /// `permission.respond` back to the server-side responder, and
        /// `choices` lets the modal render the same options a local prompt
        /// would. The live oneshot responder itself never crosses the wire.
        PermissionPrompt {
            /// Stable block id (the modal's display id).
            id: u64,
            /// Server-global id the client echoes in `permission.respond` so
            /// the server resolves the right in-flight prompt. Defaults to
            /// `u64::MAX` for frames from a pre-F2 server (decoded as
            /// unroutable → the client falls back to a passive notice).
            #[serde(default = "default_prompt_id")]
            prompt_id: u64,
            /// Canonical tool name the prompt gates.
            tool_name: String,
            /// Human-readable justification.
            reasoning: String,
            /// Short audit line explaining risk and explicit unblock action.
            #[serde(default)]
            audit_hint: Option<String>,
            /// Selectable options in display order. Empty for a pre-F2 frame.
            #[serde(default)]
            choices: Vec<SerializablePermissionChoice>,
        },
        /// Blocking user question prompt.
        UserQuestionPrompt {
            /// Stable block id.
            id: u64,
            /// Human-readable question.
            question: String,
            /// Fixed choice labels, empty for free-form prompts.
            options: Vec<String>,
            /// Whether several options may be checked at once. Defaults to
            /// single-select for records written before multi-select existed.
            #[serde(default)]
            multi_select: bool,
        },
        /// Real mid-turn usage snapshot — accurate token/cost telemetry for
        /// machine consumers (the live-ledger HUD counterpart).
        Usage {
            /// Absolute estimated context tokens after the latest response.
            ctx_tokens: u64,
            /// Session-cumulative input tokens.
            input_tokens: u32,
            /// Session-cumulative output tokens.
            output_tokens: u32,
            /// Session-cumulative cache-read input tokens.
            cache_read_tokens: u32,
            /// Session-cumulative cache-creation input tokens.
            cache_creation_tokens: u32,
        },
        /// Unified 5h/7d rate-limit utilization (subscription / OAuth only).
        RateLimit {
            /// 5-hour rolling window utilization, `0.0..=1.0`, if present.
            five_hour_utilization: Option<f64>,
            /// 7-day rolling window utilization, `0.0..=1.0`, if present.
            seven_day_utilization: Option<f64>,
        },
    }

    /// One selectable option on a [`SerializableRenderBlock::PermissionPrompt`].
    /// The mirror of [`runtime::message_stream::PermissionChoice`] with the
    /// decision lowered to a wire tag.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SerializablePermissionChoice {
        /// Single keyboard key (as a one-char string for JSON friendliness).
        pub key: String,
        /// Human-readable label.
        pub label: String,
        /// Decision tag: `allow_once` | `allow_always` | `deny` | `deny_always`.
        pub decision: String,
    }

    /// Default `prompt_id` for a frame decoded without one (pre-F2 server):
    /// `u64::MAX` marks it unroutable so the client shows a passive notice
    /// rather than a modal wired to a server that cannot answer.
    const fn default_prompt_id() -> u64 {
        u64::MAX
    }

    /// Lower a render-block [`PermissionDecision`] to its wire tag.
    ///
    /// Re-exported at [`crate::sinks`] so the `zo attach` client encodes its
    /// modal decision through the same table the server decodes with.
    #[must_use]
    pub fn permission_decision_tag(decision: PermissionDecision) -> &'static str {
        match decision {
            PermissionDecision::AllowOnce => "allow_once",
            PermissionDecision::AllowAlways => "allow_always",
            PermissionDecision::Deny => "deny",
            PermissionDecision::DenyAlways => "deny_always",
        }
    }

    /// Inverse of [`permission_decision_tag`]. Unknown tags fall back to the
    /// safe `Deny`, so a malformed wire decision never silently allows.
    ///
    /// Re-exported at [`crate::sinks`] so the `zo serve` dispatcher decodes a
    /// client's `permission.respond` tag through the *same* table the wire
    /// frame is built from — one source of truth for the decision vocabulary.
    #[must_use]
    pub fn permission_decision_from_tag(tag: &str) -> PermissionDecision {
        match tag {
            "allow_once" => PermissionDecision::AllowOnce,
            "allow_always" => PermissionDecision::AllowAlways,
            "deny_always" => PermissionDecision::DenyAlways,
            _ => PermissionDecision::Deny,
        }
    }

    impl SerializableRenderBlock {
        /// Project a borrowed [`RenderBlock`] into an owned serializable
        /// form.
        // A flat 1:1 exhaustive match over every variant; splitting it would
        // scatter the projection across helpers and hurt readability.
        #[allow(clippy::too_many_lines)] // flat RenderBlock to serializable mapping, one arm per block
        #[must_use]
        pub fn from_block(block: &RenderBlock) -> Self {
            match project_render_block(block) {
                ProjectedRenderBlock::TextDelta {
                    id,
                    text,
                    done,
                } => {
                    return Self::TextDelta {
                        id,
                        text: text.to_string(),
                        done,
                    };
                }
                ProjectedRenderBlock::ToolCall {
                    id,
                    tool_call_id,
                    name,
                    summary,
                    status,
                } => {
                    return Self::ToolCall {
                        id,
                        tool_call_id: tool_call_id.to_string(),
                        name: name.to_string(),
                        summary: summary.to_string(),
                        status: status_tag(status).to_string(),
                    };
                }
                ProjectedRenderBlock::ToolResult {
                    id,
                    tool_call_id,
                    is_error,
                    body,
                } => {
                    return Self::ToolResult {
                        id,
                        tool_call_id: tool_call_id.to_string(),
                        is_error,
                        content: super::text::tool_result_preview(body),
                    };
                }
                ProjectedRenderBlock::Other => {}
            }

            match block {
                RenderBlock::TextDelta { .. }
                | RenderBlock::ToolCall { .. }
                | RenderBlock::ToolResult { .. } => {
                    unreachable!("projected render block must return above")
                }
                RenderBlock::Reasoning {
                    id,
                    text,
                    signature,
                    done,
                } => Self::Reasoning {
                    id: id.0,
                    text: text.clone(),
                    signature: signature.clone(),
                    done: *done,
                },
                RenderBlock::Image {
                    id,
                    data,
                    media_type,
                } => Self::Image {
                    id: id.0,
                    media_type: media_type.clone(),
                    byte_len: data.len(),
                },
                RenderBlock::UserMessage { id, text } => Self::System {
                    id: id.0,
                    level: "user".to_string(),
                    text: text.clone(),
                },
                // A `send_to_user` push has no live payload, so — like `Card`
                // and folded user messages — it projects to a `System` notice on
                // the wire; a `zo attach` client sees the verbatim text.
                RenderBlock::UserNotice { id, message } => Self::System {
                    id: id.0,
                    level: "info".to_string(),
                    text: message.clone(),
                },
                RenderBlock::AgentResult {
                    id,
                    label,
                    status,
                    body,
                } => Self::AgentResult {
                    id: id.0,
                    label: label.clone(),
                    status: agent_status_tag(*status).to_string(),
                    body: body.clone(),
                },
                RenderBlock::System { id, level, text } => Self::System {
                    id: id.0,
                    level: level_tag(*level).to_string(),
                    text: text.clone(),
                },
                RenderBlock::PermissionPrompt(prompt) => Self::PermissionPrompt {
                    id: prompt.id.0,
                    // The socket prompter encodes the routing id in the block
                    // id, so the two coincide; carry it explicitly for clarity
                    // and forward compatibility.
                    prompt_id: prompt.id.0,
                    tool_name: prompt.tool_name.clone(),
                    reasoning: prompt.reasoning.clone(),
                    audit_hint: prompt.audit_hint.clone(),
                    choices: prompt
                        .choices
                        .iter()
                        .map(|c| SerializablePermissionChoice {
                            key: c.key.to_string(),
                            label: c.label.clone(),
                            decision: permission_decision_tag(c.decision).to_string(),
                        })
                        .collect(),
                },
                RenderBlock::UserQuestionPrompt(prompt) => Self::UserQuestionPrompt {
                    id: prompt.id.0,
                    question: prompt.question.clone(),
                    // Labels only: descriptions/header are modal presentation
                    // sugar; the durable record is the question + choices.
                    options: prompt.options.iter().map(|opt| opt.label.clone()).collect(),
                    multi_select: prompt.multi_select,
                },
                RenderBlock::Card { id, card } => Self::System {
                    id: id.0,
                    level: "info".to_string(),
                    text: card.plain_text(),
                },
                // Live-spinner-only heartbeat: attach/headless sinks have no
                // spinner, so surface it as a plain info row (id 0 = ledger
                // block, same convention the layout uses for Usage).
                RenderBlock::CompactionProgress { streamed_chars } => Self::System {
                    id: 0,
                    level: "info".to_string(),
                    text: format!("Compacting conversation… {streamed_chars} chars streamed"),
                },
                RenderBlock::Usage {
                    ctx_tokens,
                    cumulative,
                    // Machine telemetry stays session-cumulative; the live ctx
                    // breakdown (`current`) is a TUI-only presentation concern.
                    current: _,
                } => Self::Usage {
                    ctx_tokens: *ctx_tokens,
                    input_tokens: cumulative.input_tokens,
                    output_tokens: cumulative.output_tokens,
                    cache_read_tokens: cumulative.cache_read_input_tokens,
                    cache_creation_tokens: cumulative.cache_creation_input_tokens,
                },
                RenderBlock::RateLimit(rl) => Self::RateLimit {
                    five_hour_utilization: rl.five_hour.map(|w| w.utilization),
                    seven_day_utilization: rl.seven_day.map(|w| w.utilization),
                },
            }
        }
    }

    const fn status_tag(status: ToolCallStatus) -> &'static str {
        match status {
            ToolCallStatus::Pending => "pending",
            ToolCallStatus::Running => "running",
            ToolCallStatus::Ok => "ok",
            ToolCallStatus::Errored => "errored",
            ToolCallStatus::Cancelled => "cancelled",
        }
    }

    const fn level_tag(level: SystemLevel) -> &'static str {
        match level {
            SystemLevel::Info => "info",
            SystemLevel::Success => "success",
            SystemLevel::Warn => "warn",
            SystemLevel::Error => "error",
        }
    }

    /// Wire tag for an [`AgentResultStatus`].
    const fn agent_status_tag(status: AgentResultStatus) -> &'static str {
        match status {
            AgentResultStatus::Completed => "completed",
            AgentResultStatus::Failed => "failed",
        }
    }

    /// Reconstruct a renderable [`RenderBlock`] from a wire frame — the inverse
    /// of [`SerializableRenderBlock::from_block`].
    ///
    /// Lossy by contract: the wire form is a presentation projection, not a
    /// clone. Variants whose live payload cannot cross a socket — `Image` bytes
    /// and the oneshot responders behind `PermissionPrompt` /
    /// `UserQuestionPrompt` — are surfaced as `System` notices rather than
    /// half-built variants that would render broken (an empty image, a modal
    /// wired to a dead channel). Structured tool previews/results collapse to
    /// their `Generic`/`Text` forms, which render correctly in the transcript.
    /// Every reversible variant carries its own `id`, so no id generator is
    /// needed.
    // A flat 1:1 inverse of `from_block`'s exhaustive match — splitting it would
    // scatter the round-trip across helpers and hurt readability.
    #[allow(clippy::too_many_lines)] // flat serializable to RenderBlock mapping, one arm per block
    #[must_use]
    pub fn to_render_block(frame: &SerializableRenderBlock) -> RenderBlock {
        match frame {
            SerializableRenderBlock::TextDelta { id, text, done } => RenderBlock::TextDelta {
                id: BlockId(*id),
                text: text.clone(),
                done: *done,
            },
            SerializableRenderBlock::Reasoning {
                id,
                text,
                signature,
                done,
            } => RenderBlock::Reasoning {
                id: BlockId(*id),
                text: text.clone(),
                signature: signature.clone(),
                done: *done,
            },
            SerializableRenderBlock::ToolCall {
                id,
                tool_call_id,
                name,
                summary,
                status,
            } => RenderBlock::ToolCall {
                id: BlockId(*id),
                tool_call_id: ToolCallId(tool_call_id.clone()),
                name: name.clone(),
                summary: summary.clone(),
                preview: ToolPreview::Generic {
                    name: name.clone(),
                    input_summary: summary.clone(),
                },
                status: status_from_tag(status),
            },
            SerializableRenderBlock::ToolResult {
                id,
                tool_call_id,
                is_error,
                content,
            } => RenderBlock::ToolResult {
                id: BlockId(*id),
                tool_call_id: ToolCallId(tool_call_id.clone()),
                is_error: *is_error,
                body: ToolResultBody::Text {
                    content: content.clone(),
                    truncated: false,
                },
            },
            SerializableRenderBlock::System { id, level, text } => {
                if level == "user" {
                    RenderBlock::UserMessage {
                        id: BlockId(*id),
                        text: text.clone(),
                    }
                } else {
                    RenderBlock::System {
                        id: BlockId(*id),
                        level: level_from_tag(level),
                        text: text.clone(),
                    }
                }
            }
            SerializableRenderBlock::AgentResult {
                id,
                label,
                status,
                body,
            } => RenderBlock::AgentResult {
                id: BlockId(*id),
                label: label.clone(),
                status: agent_status_from_tag(status),
                body: body.clone(),
            },
            SerializableRenderBlock::Image {
                id,
                media_type,
                byte_len,
            } => RenderBlock::System {
                id: BlockId(*id),
                level: SystemLevel::Info,
                text: format!("[image: {media_type}, {byte_len} bytes]"),
            },
            SerializableRenderBlock::PermissionPrompt {
                id,
                prompt_id,
                tool_name,
                reasoning,
                audit_hint,
                choices,
            } => {
                if *prompt_id == default_prompt_id() {
                    // Pre-F2 server: no route home, so render a passive notice
                    // instead of a modal wired to a channel nobody answers.
                    RenderBlock::System {
                        id: BlockId(*id),
                        level: SystemLevel::Warn,
                        text: audit_hint.as_ref().map_or_else(
                            || format!("permission: {tool_name} — {reasoning} (auto-handled by server)"),
                            |hint| format!("permission: {tool_name} — {reasoning}. {hint} (auto-handled by server)"),
                        ),
                    }
                } else {
                    // F2: a live, answerable prompt. The block id carries the
                    // server's routing id (`prompt_id`); the embedded responder
                    // is vestigial on the client, which answers over the socket
                    // via `permission.respond`, not this oneshot.
                    let (responder, _unused) = tokio::sync::oneshot::channel();
                    RenderBlock::PermissionPrompt(PermissionPrompt {
                        id: BlockId(*prompt_id),
                        tool_call_id: ToolCallId(String::new()),
                        tool_name: tool_name.clone(),
                        reasoning: reasoning.clone(),
                        audit_hint: audit_hint.clone(),
                        choices: choices
                            .iter()
                            .map(|c| PermissionChoice {
                                key: c.key.chars().next().unwrap_or('?'),
                                label: c.label.clone(),
                                decision: permission_decision_from_tag(&c.decision),
                            })
                            .collect(),
                        responder,
                    })
                }
            }
            SerializableRenderBlock::UserQuestionPrompt {
                id,
                question,
                options,
                multi_select,
            } => RenderBlock::System {
                id: BlockId(*id),
                level: SystemLevel::Info,
                text: if options.is_empty() {
                    format!("question: {question}")
                } else if *multi_select {
                    format!(
                        "question: {question} [{}] (multi-select)",
                        options.join(", ")
                    )
                } else {
                    format!("question: {question} [{}]", options.join(", "))
                },
            },
            SerializableRenderBlock::Usage {
                ctx_tokens,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            } => RenderBlock::Usage {
                ctx_tokens: *ctx_tokens,
                cumulative: TokenUsage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    cache_read_input_tokens: *cache_read_tokens,
                    cache_creation_input_tokens: *cache_creation_tokens,
                },
                // The per-turn `new`/`cached` split is a TUI-only field the wire
                // drops; default keeps the headline ctx/cost figures correct.
                current: TokenUsage::default(),
            },
            SerializableRenderBlock::RateLimit {
                five_hour_utilization,
                seven_day_utilization,
            } => RenderBlock::RateLimit(RateLimitSnapshot {
                five_hour: five_hour_utilization.map(|utilization| RateLimitWindow {
                    utilization,
                    resets_at_unix: None,
                }),
                seven_day: seven_day_utilization.map(|utilization| RateLimitWindow {
                    utilization,
                    resets_at_unix: None,
                }),
                // The "which window binds" hint is not on the wire; the sidebar
                // simply shows both gauges.
                representative: None,
            }),
        }
    }

    fn status_from_tag(tag: &str) -> ToolCallStatus {
        match tag {
            "pending" => ToolCallStatus::Pending,
            "running" => ToolCallStatus::Running,
            "errored" => ToolCallStatus::Errored,
            "cancelled" => ToolCallStatus::Cancelled,
            // "ok" and any unknown tag both render as a completed call.
            _ => ToolCallStatus::Ok,
        }
    }

    fn level_from_tag(tag: &str) -> SystemLevel {
        match tag {
            "success" => SystemLevel::Success,
            "warn" => SystemLevel::Warn,
            "error" => SystemLevel::Error,
            // Unknown tags — including a level minted by a newer peer — read as
            // a plain notice instead of dropping the line.
            _ => SystemLevel::Info,
        }
    }

    /// Parse an [`AgentResultStatus`] wire tag; unknown tags default to
    /// `Completed` (a re-injected result that reached the client is, by
    /// definition, a result).
    fn agent_status_from_tag(tag: &str) -> AgentResultStatus {
        match tag {
            "failed" => AgentResultStatus::Failed,
            _ => AgentResultStatus::Completed,
        }
    }
}

/// Reconstruct a [`RenderBlock`] from one wire render frame — the canonical
/// `SerializableRenderBlock` JSON that `zo serve` streams. Used by
/// `zo attach` to feed the ratatui `App` from a socket. Encapsulates the
/// crate-private projection type so callers only deal in [`RenderBlock`].
///
/// # Errors
/// Returns [`serde_json::Error`] if `value` is not a valid render frame.
pub fn render_block_from_value(
    value: &serde_json::Value,
) -> Result<RenderBlock, serde_json::Error> {
    let frame: serializable::SerializableRenderBlock = serde_json::from_value(value.clone())?;
    Ok(serializable::to_render_block(&frame))
}

#[cfg(test)]
mod agent_result_wire_tests {
    use super::render_block_from_value;
    use super::serializable::SerializableRenderBlock;
    use runtime::message_stream::{AgentResultStatus, BlockId, RenderBlock, SystemLevel};

    /// An agent-result card must survive the `zo attach` wire as a first-class
    /// `AgentResult` frame — NOT be downcast to a raw System wall — so a remote
    /// client renders the same collapsible card. This is the regression the
    /// cross-model review caught.
    #[test]
    fn agent_result_round_trips_over_the_wire() {
        let block = RenderBlock::AgentResult {
            id: BlockId(42),
            label: "runtime-scout".to_string(),
            status: AgentResultStatus::Failed,
            body: "line one\nline two".to_string(),
        };
        let frame = SerializableRenderBlock::from_block(&block);
        // It serializes as its own variant, not a System fallback.
        assert!(
            matches!(frame, SerializableRenderBlock::AgentResult { .. }),
            "must serialize as a first-class AgentResult frame"
        );
        let json = serde_json::to_value(&frame).expect("serialize");
        let restored = render_block_from_value(&json).expect("deserialize");
        match restored {
            RenderBlock::AgentResult {
                id,
                label,
                status,
                body,
            } => {
                assert_eq!(id, BlockId(42));
                assert_eq!(label, "runtime-scout");
                assert_eq!(status, AgentResultStatus::Failed);
                assert_eq!(body, "line one\nline two");
            }
            other => panic!("round-trip lost the card: {other:?}"),
        }
    }

    /// A `send_to_user` push has no live payload, so — like a `Card` — it folds
    /// to a `System` notice on the wire rather than a first-class frame. Mirrors
    /// the `Card`/user-message fold precedent; a `zo attach` client still sees
    /// the verbatim text.
    #[test]
    fn user_notice_folds_to_a_system_frame_over_the_wire() {
        let block = RenderBlock::UserNotice {
            id: BlockId(7),
            message: "verbatim finding".to_string(),
        };
        let frame = SerializableRenderBlock::from_block(&block);
        match &frame {
            SerializableRenderBlock::System { id, level, text } => {
                assert_eq!(*id, 7);
                assert_eq!(level, "info");
                assert_eq!(text, "verbatim finding");
            }
            other => panic!("UserNotice must fold to a System frame, got {other:?}"),
        }
        // And the folded frame still round-trips as a System block.
        let json = serde_json::to_value(&frame).expect("serialize");
        let restored = render_block_from_value(&json).expect("deserialize");
        assert!(matches!(restored, RenderBlock::System { .. }));
    }

    /// A success notice must keep its level across the wire. The producer
    /// (`level_tag`) and the consumer (`level_from_tag`) have to stay in step:
    /// when only one of them knew about `success`, a `zo attach` client silently
    /// re-rendered the green check as a plain info notice.
    #[test]
    fn success_level_round_trips_over_the_wire() {
        let block = RenderBlock::System {
            id: BlockId(11),
            level: SystemLevel::Success,
            text: "claude OAuth login successful!".to_string(),
        };
        let frame = SerializableRenderBlock::from_block(&block);
        match &frame {
            SerializableRenderBlock::System { level, .. } => assert_eq!(level, "success"),
            other => panic!("a System block must serialize as a System frame, got {other:?}"),
        }
        let json = serde_json::to_value(&frame).expect("serialize");
        match render_block_from_value(&json).expect("deserialize") {
            RenderBlock::System { id, level, text } => {
                assert_eq!(id, BlockId(11));
                assert_eq!(
                    level,
                    SystemLevel::Success,
                    "the wire must not downgrade Success to Info"
                );
                assert_eq!(text, "claude OAuth login successful!");
            }
            other => panic!("round-trip lost the notice: {other:?}"),
        }
    }
}
