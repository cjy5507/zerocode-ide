use std::io::Write;

use api::{AnthropicClient, MessageResponse, OutputContentBlock};
use runtime::{AssistantEvent, ProviderStateBlob, RuntimeError};

use crate::render::TerminalRenderer;

pub(crate) fn push_output_block(
    block: OutputContentBlock,
    out: &mut (impl Write + ?Sized),
    events: &mut Vec<AssistantEvent>,
    pending_tool: &mut Option<(String, String, String)>,
    streaming_tool_input: bool,
) -> Result<(), RuntimeError> {
    match block {
        OutputContentBlock::Text { text } => {
            if !text.is_empty() {
                let rendered = TerminalRenderer::new().markdown_to_ansi(&text);
                write!(out, "{rendered}")
                    .and_then(|()| out.flush())
                    .map_err(|error| RuntimeError::new(error.to_string()))?;
                events.push(AssistantEvent::TextDelta(text));
            }
        }
        OutputContentBlock::ToolUse { id, name, input } => {
            let initial_input = if streaming_tool_input
                && input.is_object()
                && input.as_object().is_some_and(serde_json::Map::is_empty)
            {
                String::new()
            } else {
                input.to_string()
            };
            *pending_tool = Some((id, name, initial_input));
        }
        OutputContentBlock::Thinking { thinking, signature } => {
            // Streaming ships an empty placeholder (text/signature arrive as
            // deltas the caller accumulates); capture here only on the
            // non-streaming path, mirroring `runtime::push_output_block`.
            if !streaming_tool_input {
                events.push(AssistantEvent::Thinking { thinking, signature });
            }
        }
        OutputContentBlock::RedactedThinking { data } => {
            events.push(AssistantEvent::RedactedThinking {
                data: runtime::redacted_thinking_data_to_string(&data),
            });
        }
    }
    Ok(())
}

pub(crate) fn response_to_events(
    response: MessageResponse,
    out: &mut (impl Write + ?Sized),
) -> Result<Vec<AssistantEvent>, RuntimeError> {
    let mut events = Vec::new();
    let mut pending_tool = None;

    for block in response.content {
        push_output_block(block, out, &mut events, &mut pending_tool, false)?;
        if let Some((id, name, input)) = pending_tool.take() {
            events.push(AssistantEvent::ToolUse { id, name, input });
        }
    }

    events.push(AssistantEvent::Usage(response.usage.token_usage()));
    if let Some(signature) = &response.thought_signature {
        events.push(AssistantEvent::ProviderState(
            ProviderStateBlob::gemini_thought_signature(signature.clone()),
        ));
    }
    if let Some(replay) = &response.reasoning_replay {
        events.push(AssistantEvent::ReasoningReplay(replay.clone()));
    }
    // Surface the stop reason so the truncation-recovery logic engages on this
    // non-streaming fallback too (it fires during empty-stream recovery, which
    // re-requests with `stream:false`). Without this, a fallback response
    // truncated at the output-token limit would reach `build_assistant_message`
    // with `stop_reason = None` and the continuation would never run — a gap vs
    // the streaming path, which already emits it.
    if let Some(reason) = response
        .stop_reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
    {
        events.push(AssistantEvent::StopReason(reason.to_string()));
    }
    events.push(AssistantEvent::MessageStop);
    Ok(events)
}

pub(crate) fn push_prompt_cache_record(client: &AnthropicClient, events: &mut Vec<AssistantEvent>) {
    if let Some(record) = client.take_last_prompt_cache_record() {
        if let Some(event) = runtime::prompt_cache_record_to_event(record) {
            events.push(AssistantEvent::PromptCache(event));
        }
    }
}
