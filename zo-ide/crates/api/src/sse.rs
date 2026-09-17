use crate::error::ApiError;
use crate::types::StreamEvent;
use memchr::memmem;
use serde::Deserialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

/// Maximum bytes buffered for a partial SSE frame before rejecting the stream.
///
/// Providers should emit frame separators regularly; 16 MiB is far beyond normal
/// token/event payloads (it comfortably fits a large `reasoning.encrypted_content`
/// blob) but prevents unbounded memory growth if a stream never terminates a
/// frame. This is the single cap shared by every SSE parser in this crate — the
/// Anthropic parser here plus the OpenAI-compatible and Responses/Gemini parsers,
/// which call [`guard_sse_buffer_push`] with it — so the policy is defined once.
pub(crate) const MAX_SSE_BUFFER_BYTES: usize = 16 * 1024 * 1024;

/// Reject a chunk that would push the retained (unparsed) SSE buffer past
/// [`MAX_SSE_BUFFER_BYTES`]. `retained` is the number of bytes already held for
/// an in-progress frame; `incoming` is the chunk about to be appended. Shared by
/// every SSE parser in the crate so the overflow check is defined in one place
/// rather than re-derived per provider.
pub(crate) fn guard_sse_buffer_push(retained: usize, incoming: usize) -> Result<(), ApiError> {
    if incoming > MAX_SSE_BUFFER_BYTES.saturating_sub(retained) {
        return Err(ApiError::InvalidSseFrame(
            "sse stream buffered too many bytes without a frame separator",
        ));
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    cursor: usize,
}

impl SseParser {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<StreamEvent>, ApiError> {
        let mut events = Vec::new();
        self.push_into(chunk, &mut events)?;
        Ok(events)
    }

    /// Parse `chunk`, appending any completed events into `out` instead of
    /// allocating a fresh `Vec` per call. A hot streaming caller can reuse one
    /// scratch buffer across every chunk, removing the per-chunk allocation
    /// [`push`](Self::push) makes. Events are appended (the existing contents of
    /// `out` are preserved), matching `Vec::extend` semantics at the call site.
    pub fn push_into(
        &mut self,
        chunk: &[u8],
        out: &mut Vec<StreamEvent>,
    ) -> Result<(), ApiError> {
        let unparsed_len = self.buffer.len().saturating_sub(self.cursor);
        guard_sse_buffer_push(unparsed_len, chunk.len())?;

        self.buffer.extend_from_slice(chunk);

        while let Some(frame) = self.next_frame() {
            if let Some(event) = parse_frame(frame)? {
                out.push(event);
            }
        }

        // drain consumed bytes in one shot
        if self.cursor > 0 {
            self.buffer.drain(..self.cursor);
            self.cursor = 0;
        }

        Ok(())
    }

    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, ApiError> {
        if self.cursor >= self.buffer.len() {
            return Ok(Vec::new());
        }

        let remaining = &self.buffer[self.cursor..];
        // Fast path: valid UTF-8 -- borrow directly, no allocation
        let result = if let Ok(s) = std::str::from_utf8(remaining) {
            parse_frame(s)
        } else {
            // Fallback: lossy decode requires an owned String
            let owned = String::from_utf8_lossy(remaining).into_owned();
            parse_frame(&owned)
        };
        self.buffer.clear();
        self.cursor = 0;
        match result? {
            Some(event) => Ok(vec![event]),
            None => Ok(Vec::new()),
        }
    }

    /// Find the next complete frame using SIMD-accelerated `memmem::find`.
    /// Returns a `&str` borrowed from the internal buffer -- zero copy.
    /// Always advances the cursor past frame separators, skipping any
    /// non-UTF-8 frames so the parser never gets stuck.
    fn next_frame(&mut self) -> Option<&str> {
        loop {
            let start = self.cursor;
            let remaining = &self.buffer[start..];

            let (position, separator_len) = memmem::find(remaining, b"\n\n")
                .map(|p| (p, 2))
                .or_else(|| memmem::find(remaining, b"\r\n\r\n").map(|p| (p, 4)))?;

            self.cursor = start + position + separator_len;
            let frame_bytes = &self.buffer[start..start + position];
            if let Ok(frame) = std::str::from_utf8(frame_bytes) {
                return Some(frame);
            }
        }
    }
}

pub fn parse_frame(frame: &str) -> Result<Option<StreamEvent>, ApiError> {
    let trimmed = frame.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut first_data: Option<&str> = None;
    let mut extra_data: Option<Vec<&str>> = None;
    let mut event_name: Option<&str> = None;

    for line in trimmed.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(name) = line.strip_prefix("event:") {
            event_name = Some(name.trim());
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim_start();
            match first_data {
                None => first_data = Some(data),
                Some(first) => {
                    // Second+ data line -- lazily allocate the overflow vec
                    let extra = extra_data.get_or_insert_with(|| vec![first]);
                    extra.push(data);
                }
            }
        }
    }

    if matches!(event_name, Some("ping")) {
        return Ok(None);
    }

    let Some(first) = first_data else {
        return Ok(None);
    };

    // Fast path: single data line (the common case) -- no join allocation
    if extra_data.is_none() {
        if first == "[DONE]" {
            return Ok(None);
        }
        if let Some(error) = parse_error_payload(first)? {
            return Err(error);
        }
        return serde_json::from_str::<StreamEvent>(first)
            .map(|event| Some(note_input_transformations(first, event)))
            .map_err(ApiError::from);
    }

    // Slow path: multiple data lines -- must join
    let payload = extra_data
        .expect("fast path above returned whenever extra_data is None")
        .join("\n");
    if payload == "[DONE]" {
        return Ok(None);
    }

    if let Some(error) = parse_error_payload(&payload)? {
        return Err(error);
    }

    serde_json::from_str::<StreamEvent>(&payload)
        .map(|event| Some(note_input_transformations(&payload, event)))
        .map_err(ApiError::from)
}

/// Whether this process has already said that the API dropped replayed thinking.
static THINKING_DROP_SAID: AtomicBool = AtomicBool::new(false);

/// Preserved thinking (Claude Fable 5.1 / Mythos 5.1, beta
/// `thinking-binding-controls-2026-08-01`): a `message_start` whose message
/// carries `input_transformations` names the replayed thinking blocks the API
/// dropped before the model saw them — `prefix_binding_mismatch` when this
/// harness changed the history behind them (keep-tail compaction, wire-time
/// tool-result trimming), `model_binding_mismatch` after a model switch. The
/// request went through; what was lost is the reasoning those blocks carried,
/// so the model re-plans. Said once per process, like the context-editing
/// latch, so a long session hears it without a line per turn. The event is
/// handed back untouched.
fn note_input_transformations(payload: &str, event: StreamEvent) -> StreamEvent {
    if matches!(event, StreamEvent::MessageStart(_)) {
        if let Some(said) = dropped_thinking_summary(payload) {
            if !THINKING_DROP_SAID.swap(true, Ordering::Relaxed) {
                eprintln!("[zo] {said}");
            }
        }
    }
    event
}

/// One sentence on the thinking blocks a `message_start` payload says were
/// dropped, or `None` when it names none.
fn dropped_thinking_summary(payload: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Peek {
        message: PeekMessage,
    }
    #[derive(Deserialize)]
    struct PeekMessage {
        #[serde(default)]
        input_transformations: Vec<Value>,
    }
    memmem::find(payload.as_bytes(), b"input_transformations")?;
    let peek = serde_json::from_str::<Peek>(payload).ok()?;
    let dropped: Vec<&Value> = peek
        .message
        .input_transformations
        .iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("thinking_dropped"))
        .collect();
    if dropped.is_empty() {
        return None;
    }
    let mut reasons: Vec<&str> = dropped
        .iter()
        .filter_map(|entry| entry.get("reason").and_then(Value::as_str))
        .collect();
    reasons.sort_unstable();
    reasons.dedup();
    let reasons = if reasons.is_empty() {
        "no reason given".to_string()
    } else {
        reasons.join(", ")
    };
    Some(format!(
        "Anthropic dropped {} replayed thinking block(s) before the model saw them ({reasons}); \
         the model re-plans without that reasoning",
        dropped.len()
    ))
}

fn parse_error_payload(payload: &str) -> Result<Option<ApiError>, ApiError> {
    // 이 함수는 매 SSE 이벤트(=매 토큰 델타)마다 호출되는데, 정상 델타가
    // 압도적 다수이고 error envelope 은 사실상 스트림 종단 1회 미만이다.
    // error 페이로드는 반드시 `"error"` 토큰(`"type":"error"` 또는 envelope
    // 의 `"error"` 키)을 포함하므로, 값싼 SIMD substring prefilter 로
    // 비-error 페이로드는 full `Value` 파싱 없이 즉시 통과시킨다. false
    // positive(텍스트 델타에 "error" 가 우연히 포함)는 아래 full-parse 가
    // `type != "error"` 로 재확인하므로 동작은 완전히 보존된다.
    if memmem::find(payload.as_bytes(), b"error").is_none() {
        return Ok(None);
    }
    let value = serde_json::from_str::<Value>(payload)?;
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return Ok(None);
    };
    if kind != "error" {
        return Ok(None);
    }

    let parsed = serde_json::from_value::<StreamErrorEnvelope>(value).map_err(ApiError::from)?;
    let retryable = matches!(
        parsed.error.error_type.as_deref(),
        Some("rate_limit_error" | "overloaded_error" | "api_error")
    );
    Ok(Some(ApiError::StreamApi {
        error_type: parsed.error.error_type,
        message: parsed.error.message,
        body: payload.to_string(),
        retryable,
    }))
}

#[derive(Debug, Deserialize)]
struct StreamErrorEnvelope {
    error: StreamErrorBody,
}

#[derive(Debug, Deserialize)]
struct StreamErrorBody {
    #[serde(rename = "type")]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{dropped_thinking_summary, parse_frame, SseParser};
    use crate::error::ApiError;
    use crate::types::{ContentBlockDelta, MessageDelta, OutputContentBlock, StreamEvent, Usage};

    #[test]
    fn parses_single_frame() {
        let frame = concat!(
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Hi\"}}\n\n"
        );

        let event = parse_frame(frame).expect("frame should parse");
        assert_eq!(
            event,
            Some(StreamEvent::ContentBlockStart(
                crate::types::ContentBlockStartEvent {
                    index: 0,
                    content_block: OutputContentBlock::Text {
                        text: "Hi".to_string(),
                    },
                },
            ))
        );
    }

    #[test]
    fn parses_chunked_stream() {
        let mut parser = SseParser::new();
        let first = b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel";
        let second = b"lo\"}}\n\n";

        assert!(parser
            .push(first)
            .expect("first chunk should buffer")
            .is_empty());
        let events = parser.push(second).expect("second chunk should parse");

        assert_eq!(
            events,
            vec![StreamEvent::ContentBlockDelta(
                crate::types::ContentBlockDeltaEvent {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta {
                        text: "Hello".to_string(),
                    },
                }
            )]
        );
    }

    #[test]
    fn push_into_reuses_buffer_and_appends_across_chunks() {
        // A hot caller reuses one scratch Vec across chunks; `push_into` must
        // append (not clear) so a pre-seeded buffer keeps its contents, and a
        // partial-then-completed frame yields exactly one event in total.
        let mut parser = SseParser::new();
        let mut scratch: Vec<StreamEvent> = Vec::new();

        let first = b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel";
        let second = b"lo\"}}\n\n";

        parser
            .push_into(first, &mut scratch)
            .expect("first chunk should buffer");
        assert!(scratch.is_empty(), "partial frame yields no event yet");

        parser
            .push_into(second, &mut scratch)
            .expect("second chunk should parse");

        assert_eq!(
            scratch,
            vec![StreamEvent::ContentBlockDelta(
                crate::types::ContentBlockDeltaEvent {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta {
                        text: "Hello".to_string(),
                    },
                }
            )],
            "push_into must append the completed event into the reused buffer"
        );
    }

    #[test]
    fn rejects_unparsed_buffer_over_cap_without_frame_separator() {
        let mut parser = SseParser::new();
        let chunk = vec![b'a'; super::MAX_SSE_BUFFER_BYTES + 1];

        let error = parser
            .push(&chunk)
            .expect_err("over-cap partial frame should fail");
        match error {
            ApiError::InvalidSseFrame(message) => assert_eq!(
                message,
                "sse stream buffered too many bytes without a frame separator"
            ),
            other => panic!("expected invalid sse frame, got {other:?}"),
        }
    }

    #[test]
    fn parses_small_data_frame_after_buffer_cap_guard() {
        let mut parser = SseParser::new();
        let events = parser
            .push(b"data: {\"type\":\"message_stop\"}\n\n")
            .expect("small data frame should parse");

        assert_eq!(
            events,
            vec![StreamEvent::MessageStop(crate::types::MessageStopEvent {})]
        );
    }

    #[test]
    fn ignores_ping_and_done() {
        let mut parser = SseParser::new();
        let payload = concat!(
            ": keepalive\n",
            "event: ping\n",
            "data: {\"type\":\"ping\"}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
            "data: [DONE]\n\n"
        );

        let events = parser
            .push(payload.as_bytes())
            .expect("parser should succeed");
        assert_eq!(
            events,
            vec![
                StreamEvent::MessageDelta(crate::types::MessageDeltaEvent {
                    delta: MessageDelta {
                        stop_reason: Some("tool_use".to_string()),
                        stop_sequence: None,
                        thought_signature: None,
                        reasoning_replay: None,
                    },
                    usage: Usage {
                        input_tokens: 1,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        output_tokens: 2,
                        output_tokens_details: None,
                    },
                    context_management: None,
                }),
                StreamEvent::MessageStop(crate::types::MessageStopEvent {}),
            ]
        );
    }

    #[test]
    fn ignores_data_less_event_frames() {
        let frame = "event: ping\n\n";
        let event = parse_frame(frame).expect("frame without data should be ignored");
        assert_eq!(event, None);
    }

    #[test]
    fn parses_split_json_across_data_lines() {
        let frame = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\n",
            "data: \"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n"
        );

        let event = parse_frame(frame).expect("frame should parse");
        assert_eq!(
            event,
            Some(StreamEvent::ContentBlockDelta(
                crate::types::ContentBlockDeltaEvent {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta {
                        text: "Hello".to_string(),
                    },
                }
            ))
        );
    }

    #[test]
    fn next_frame_preserves_remaining_buffer_for_lf_and_crlf() {
        // LF separator: verify both frames are extracted in order
        let mut parser = SseParser::new();
        parser.buffer = b"data: one\n\ndata: two\n\n".to_vec();
        let first = parser.next_frame().map(std::borrow::ToOwned::to_owned);
        assert_eq!(first.as_deref(), Some("data: one"));
        let second = parser.next_frame().map(std::borrow::ToOwned::to_owned);
        assert_eq!(second.as_deref(), Some("data: two"));

        // CRLF separator: same behavior
        let mut parser = SseParser::new();
        parser.buffer = b"data: one\r\n\r\ndata: two\r\n\r\n".to_vec();
        let first = parser.next_frame().map(std::borrow::ToOwned::to_owned);
        assert_eq!(first.as_deref(), Some("data: one"));
        let second = parser.next_frame().map(std::borrow::ToOwned::to_owned);
        assert_eq!(second.as_deref(), Some("data: two"));
    }

    #[test]
    fn parses_thinking_content_block_start() {
        let frame = concat!(
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":null}}\n\n"
        );

        let event = parse_frame(frame).expect("frame should parse");
        assert_eq!(
            event,
            Some(StreamEvent::ContentBlockStart(
                crate::types::ContentBlockStartEvent {
                    index: 0,
                    content_block: OutputContentBlock::Thinking {
                        thinking: String::new(),
                        signature: None,
                    },
                },
            ))
        );
    }

    #[test]
    fn parses_thinking_related_deltas() {
        let thinking = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"step 1\"}}\n\n"
        );
        let signature = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig_123\"}}\n\n"
        );

        let thinking_event = parse_frame(thinking).expect("thinking delta should parse");
        let signature_event = parse_frame(signature).expect("signature delta should parse");

        assert_eq!(
            thinking_event,
            Some(StreamEvent::ContentBlockDelta(
                crate::types::ContentBlockDeltaEvent {
                    index: 0,
                    delta: ContentBlockDelta::ThinkingDelta {
                        thinking: "step 1".to_string(),
                    },
                }
            ))
        );
        assert_eq!(
            signature_event,
            Some(StreamEvent::ContentBlockDelta(
                crate::types::ContentBlockDeltaEvent {
                    index: 0,
                    delta: ContentBlockDelta::SignatureDelta {
                        signature: "sig_123".to_string(),
                    },
                }
            ))
        );
    }

    #[test]
    fn surfaces_stream_error_payloads_as_api_errors() {
        let frame = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\"}}\n\n"
        );

        let error = parse_frame(frame).expect_err("error frame should become api error");
        match error {
            ApiError::StreamApi {
                error_type,
                message,
                retryable,
                ..
            } => {
                assert_eq!(error_type.as_deref(), Some("overloaded_error"));
                assert_eq!(message.as_deref(), Some("busy"));
                assert!(retryable);
            }
            other => panic!("expected stream api error, got {other:?}"),
        }
    }

    #[test]
    fn text_delta_containing_error_word_is_not_misclassified() {
        // prefilter 회귀 가드: 텍스트에 "error" 가 우연히 포함된 정상 델타는
        // error envelope 으로 오인되면 안 된다. prefilter 가 매칭하더라도
        // full-parse 의 `type != "error"` 재확인으로 정상 TextDelta 가 된다.
        let frame = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"error: not really\"}}\n\n"
        );
        let event = parse_frame(frame).expect("delta containing 'error' must parse, not error out");
        assert_eq!(
            event,
            Some(StreamEvent::ContentBlockDelta(
                crate::types::ContentBlockDeltaEvent {
                    index: 0,
                    delta: ContentBlockDelta::TextDelta {
                        text: "error: not really".to_string(),
                    },
                }
            ))
        );
    }

    /// A `message_start` that names dropped thinking blocks is summarised —
    /// count and distinct reasons — and one that names none says nothing.
    #[test]
    fn a_message_start_that_dropped_thinking_is_summarised() {
        let payload = r#"{"type":"message_start","message":{"id":"m","type":"message","role":"assistant","content":[],"model":"claude-fable-5-1","input_transformations":[{"type":"thinking_dropped","path":"messages.1.content.0","reason":"prefix_binding_mismatch"},{"type":"thinking_dropped","path":"messages.3.content.0","reason":"prefix_binding_mismatch"}]}}"#;
        assert_eq!(
            dropped_thinking_summary(payload).as_deref(),
            Some(
                "Anthropic dropped 2 replayed thinking block(s) before the model saw them \
                 (prefix_binding_mismatch); the model re-plans without that reasoning"
            )
        );
        assert!(dropped_thinking_summary(
            r#"{"type":"message_start","message":{"input_transformations":[]}}"#
        )
        .is_none());
        assert!(dropped_thinking_summary(r#"{"type":"content_block_delta"}"#).is_none());
    }
}
