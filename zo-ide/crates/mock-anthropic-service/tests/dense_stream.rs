//! `PARITY_SCENARIO:dense_stream` — a long answer of code blocks, tables, long
//! lines and Korean lines, streamed in many small text deltas, so a
//! measurement of the zo pane and of the window that draws it has something
//! heavy to draw without a model call (t-82949).
//!
//! The answer's shape is read from the words after the scenario name
//! (`bytes=`, `chunk=`, `width=`, `pace_ms=`). What a scene gets when it names
//! none is written in the tests below, not left to a constant a reader has to
//! find.

use std::time::{Duration, Instant};

use api::{ContentBlockDelta, SseParser, StreamEvent};
use mock_anthropic_service::{MockAnthropicService, DEFAULT_MODEL};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// What one streamed request brought back.
struct Streamed {
    /// The status line and headers; empty when the mock closed without one.
    head: String,
    /// Every text delta, in order.
    deltas: Vec<String>,
    /// From connecting to the last byte.
    elapsed: Duration,
}

impl Streamed {
    fn text(&self) -> String {
        self.deltas.concat()
    }
}

/// Send one Messages request for `prompt` and read the reply to its end.
async fn post(service: &MockAnthropicService, prompt: &str, stream: bool) -> (String, Vec<u8>) {
    let base_url = service.base_url();
    let address = base_url.trim_start_matches("http://");
    let body = json!({
        "model": DEFAULT_MODEL,
        "max_tokens": 64,
        "stream": stream,
        "messages": [{"role": "user", "content": [{"type": "text", "text": prompt}]}],
    })
    .to_string();
    let request = format!(
        "POST /v1/messages HTTP/1.1\r\nhost: {address}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut socket = TcpStream::connect(address)
        .await
        .expect("connect to the mock");
    socket
        .write_all(request.as_bytes())
        .await
        .expect("send the request");
    let mut raw = Vec::new();
    socket
        .read_to_end(&mut raw)
        .await
        .expect("read the reply");
    match raw.windows(4).position(|window| window == b"\r\n\r\n") {
        Some(end) => (
            String::from_utf8_lossy(&raw[..end]).into_owned(),
            raw[end + 4..].to_vec(),
        ),
        None => (String::new(), Vec::new()),
    }
}

async fn stream(service: &MockAnthropicService, prompt: &str) -> Streamed {
    let started = Instant::now();
    let (head, body) = post(service, prompt, true).await;
    let elapsed = started.elapsed();
    let mut parser = SseParser::new();
    let mut events = parser.push(&body).expect("the body parses as SSE");
    events.extend(parser.finish().expect("the stream ends on a frame"));
    let deltas = events
        .into_iter()
        .filter_map(|event| match event {
            StreamEvent::ContentBlockDelta(event) => match event.delta {
                ContentBlockDelta::TextDelta { text } => Some(text),
                _ => None,
            },
            _ => None,
        })
        .collect();
    Streamed {
        head,
        deltas,
        elapsed,
    }
}

fn is_hangul(character: char) -> bool {
    ('가'..='힣').contains(&character)
}

/// The four things that make the answer dense, each at least once.
fn assert_dense(text: &str, width: usize) {
    assert!(text.contains("\n```"), "no fenced code block");
    assert!(
        text.lines()
            .any(|line| line.len() > 1 && line.starts_with('|') && line.ends_with('|')),
        "no table row"
    );
    assert!(
        text.lines().any(|line| line.chars().any(is_hangul)),
        "no Korean line"
    );
    assert!(
        text.lines().any(|line| line.len() >= width),
        "no line of {width} bytes or more"
    );
}

#[tokio::test]
async fn dense_stream_brings_at_least_the_asked_bytes_in_many_small_text_deltas() {
    let service = MockAnthropicService::spawn().await.expect("start the mock");
    let answer = stream(
        &service,
        "PARITY_SCENARIO:dense_stream bytes=65536 chunk=40 width=600 draw a dense answer",
    )
    .await;

    assert!(answer.head.starts_with("HTTP/1.1 200"), "{}", answer.head);
    let text = answer.text();
    assert!(text.len() >= 65_536, "{} bytes", text.len());
    assert!(
        answer.deltas.len() >= 65_536 / 40,
        "{} deltas",
        answer.deltas.len()
    );
    assert!(
        answer
            .deltas
            .iter()
            .all(|delta| !delta.is_empty() && delta.len() <= 40),
        "a delta outside 1..=40 bytes"
    );
    assert_dense(&text, 600);

    let requests = service.captured_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].scenario, "dense_stream");
}

/// A scene that names nothing gets 256 KiB in deltas of at most 32 bytes,
/// long lines of at least 480 bytes, all sent at once.
#[tokio::test]
async fn dense_stream_without_arguments_sends_256_kib_in_deltas_of_at_most_32_bytes() {
    let service = MockAnthropicService::spawn().await.expect("start the mock");
    let answer = stream(&service, "PARITY_SCENARIO:dense_stream").await;

    let text = answer.text();
    assert!(text.len() >= 256 * 1024, "{} bytes", text.len());
    assert!(
        answer.deltas.len() >= 256 * 1024 / 32,
        "{} deltas",
        answer.deltas.len()
    );
    assert!(
        answer.deltas.iter().all(|delta| delta.len() <= 32),
        "a delta over 32 bytes"
    );
    assert_dense(&text, 480);
}

/// A pace spreads the same deltas over time — the scene a frame-rate
/// measurement needs — and changes nothing else.
#[tokio::test]
async fn dense_stream_with_a_pace_sends_the_same_deltas_spread_over_time() {
    let service = MockAnthropicService::spawn().await.expect("start the mock");
    let at_once = stream(&service, "PARITY_SCENARIO:dense_stream bytes=2048 chunk=128").await;
    let paced = stream(
        &service,
        "PARITY_SCENARIO:dense_stream bytes=2048 chunk=128 pace_ms=5",
    )
    .await;

    assert!(at_once.text().len() >= 2048, "{} bytes", at_once.text().len());
    assert_eq!(paced.deltas, at_once.deltas);
    let deltas = u32::try_from(paced.deltas.len()).expect("a small count");
    let elapsed = paced.elapsed;
    assert!(
        elapsed >= Duration::from_millis(5) * deltas,
        "{deltas} deltas in {elapsed:?}"
    );
}

/// The whole answer without streaming is the streamed deltas joined.
#[tokio::test]
async fn dense_stream_without_streaming_answers_the_same_text_in_one_block() {
    let service = MockAnthropicService::spawn().await.expect("start the mock");
    let prompt = "PARITY_SCENARIO:dense_stream bytes=4096 chunk=16";
    let streamed = stream(&service, prompt).await;
    let (head, body) = post(&service, prompt, false).await;

    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let message: Value = serde_json::from_slice(&body).expect("a JSON message");
    assert_eq!(
        message["content"][0]["text"].as_str(),
        Some(streamed.text().as_str())
    );
}

/// A shape the mock cannot honour is refused like an unknown scenario — the
/// connection closes without a reply — instead of being quietly replaced.
#[tokio::test]
async fn dense_stream_refuses_a_shape_it_cannot_honour() {
    let service = MockAnthropicService::spawn().await.expect("start the mock");
    let honoured = stream(&service, "PARITY_SCENARIO:dense_stream bytes=64 chunk=8").await;
    assert!(honoured.head.starts_with("HTTP/1.1 200"), "{}", honoured.head);

    for prompt in [
        "PARITY_SCENARIO:dense_stream chunk=0",
        "PARITY_SCENARIO:dense_stream bytes=0",
        "PARITY_SCENARIO:dense_stream width=0",
        "PARITY_SCENARIO:dense_stream bytes=many",
        "PARITY_SCENARIO:dense_stream pace_ms=-1",
        "PARITY_SCENARIO:dense_stream size=64",
    ] {
        let refused = stream(&service, prompt).await;
        assert!(refused.head.is_empty(), "{prompt}: {}", refused.head);
    }
}
