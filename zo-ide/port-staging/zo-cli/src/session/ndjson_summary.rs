/// NDJSON (`stream-json`) formatter for a completed `TurnSummary`.
///
/// Emits a typed event document mirroring the Claude Code / Cursor
/// `stream-json` contract: one JSON object per line, each with a `type`
/// tag. The sequence is
///
/// ```text
/// {"type":"system", …}        ← init: model / session start
/// {"type":"text_delta", …}    ← final assistant text (done=true)
/// {"type":"system", …}        ← auto-compaction notice (optional)
/// {"type":"result", …}        ← terminal metrics: duration / usage / cost
/// ```
///
/// The per-block events go through the shared [`Sink`] funnel (so they
/// reuse the `SerializableRenderBlock` schema), and the terminal
/// `result` event — which has no `RenderBlock` analogue — is written
/// directly after the sink flushes.
///
/// This function performs no IO beyond the supplied writer and has no
/// dependency on the network. `duration_ms` is supplied by the caller so
/// the output stays deterministic under test.
///
/// The production ndjson path now streams events live via [`drive_ndjson_stream`];
/// this post-hoc replay is retained only as the format/byte-equivalence contract
/// exercised by the fixture tests, hence `#[cfg(test)]`.
#[cfg(test)]
pub(crate) fn write_ndjson_summary<W: std::io::Write>(
    summary: &runtime::TurnSummary,
    model: &str,
    session_id: &str,
    duration_ms: u128,
    mut writer: W,
) -> Result<(), Box<dyn std::error::Error>> {
    use runtime::message_stream::{BlockIdGen, RenderBlock, SystemLevel};
    use zo_cli::sinks::{NdjsonSink, Sink};

    {
        // Borrow the writer for the per-block sink so the terminal
        // `result` line can be written to the same stream afterwards.
        let mut sink: Box<dyn Sink> = Box::new(NdjsonSink::new(&mut writer));
        let ids = BlockIdGen::default();

        // init — announce the model so a consumer can frame the stream.
        sink.emit(&RenderBlock::System {
            id: ids.next(),
            level: SystemLevel::Info,
            text: format!("session start · model {model}"),
        })?;

        let assistant_text = crate::final_assistant_text(summary);
        if !assistant_text.is_empty() {
            sink.emit(&RenderBlock::TextDelta {
                id: ids.next(),
                text: assistant_text,
                done: true,
            })?;
        }

        if let Some(event) = summary.auto_compaction.as_ref() {
            sink.emit(&RenderBlock::System {
                id: ids.next(),
                level: SystemLevel::Info,
                text: crate::formatting::format_auto_compaction_notice(event.removed_message_count),
            })?;
        }

        sink.finalize()?;
    }

    write_ndjson_result_event(
        summary,
        model,
        session_id,
        duration_ms,
        runtime::TokenUsage::default(),
        &mut writer,
    )
}

/// Build the **Claude-Code-SDK-compatible** terminal `result` object shared by
/// every headless output path (`--output-format json` and `stream-json`).
///
/// The SDK contract requires this exact key set, identically named and typed,
/// so an SDK consumer can parse Zo's `-p` output without special-casing:
///
/// ```text
/// {
///   "type": "result",          // always; the terminal frame tag
///   "subtype": "success",      // turn completed (a failed turn exits non-zero
///                              // upstream before this serializer runs)
///   "is_error": false,         // mirrors `subtype`; never true here
///   "result": "<assistant>",   // the final assistant text
///   "session_id": "<id>",      // the resumable session this turn belongs to
///   "num_turns": <n>,          // model round-trips this turn took
///   "duration_ms": <ms>,       // wall time, supplied by the caller
///   "total_cost_usd": <f64>,   // numeric so it is directly comparable
///   "usage": { … }             // the four provider token counters
/// }
/// ```
///
/// Returned as a `serde_json::Value` object so each path can layer **additive**
/// zo-specific extras (e.g. `model`, `tool_uses`) on top without dropping or
/// renaming an SDK key. Factoring this here is what stops the two paths from
/// drifting — they share one source of truth for the SDK key set.
pub(crate) fn sdk_result_object(
    summary: &runtime::TurnSummary,
    model: &str,
    session_id: &str,
    duration_ms: u128,
    usage_baseline: runtime::TokenUsage,
) -> serde_json::Value {
    // Per-RUN usage, not session-cumulative: `summary.usage` counts the whole
    // session including the history a resumed `--session-id` run rehydrated,
    // and Claude Code's result object bills per run — a consumer summing
    // stage reports (the bench, CI cost gates) double-counts otherwise. The
    // baseline is zero for a fresh session, so single-shot output is
    // unchanged.
    let run_usage = summary.usage.saturating_sub(usage_baseline);
    let total_cost_usd = run_usage
        .estimate_cost_usd_with_pricing(
            runtime::pricing_for_model(model)
                .unwrap_or_else(runtime::ModelPricing::default_sonnet_tier),
        )
        .total_cost_usd();
    serde_json::json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": crate::final_assistant_text(summary),
        "session_id": session_id,
        "num_turns": summary.iterations,
        "duration_ms": duration_ms,
        "total_cost_usd": total_cost_usd,
        "usage": {
            "input_tokens": run_usage.input_tokens,
            "output_tokens": run_usage.output_tokens,
            "cache_creation_input_tokens": run_usage.cache_creation_input_tokens,
            "cache_read_input_tokens": run_usage.cache_read_input_tokens,
        },
    })
}

/// Write the terminal `{"type":"result", …}` metrics event for a completed
/// turn. Shared by the post-hoc summary path ([`write_ndjson_summary`]) and the
/// live-streaming path ([`drive_ndjson_stream`]), which emits per-block events
/// itself and only needs this terminal line afterwards.
///
/// Emits the SDK key set from [`sdk_result_object`], plus the zo-specific
/// `model`, `iterations`, and `estimated_cost` (formatted) as ADDITIVE extras.
pub(crate) fn write_ndjson_result_event<W: std::io::Write>(
    summary: &runtime::TurnSummary,
    model: &str,
    session_id: &str,
    duration_ms: u128,
    usage_baseline: runtime::TokenUsage,
    mut writer: W,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut result = sdk_result_object(summary, model, session_id, duration_ms, usage_baseline);
    let total_cost_usd = result["total_cost_usd"].as_f64().unwrap_or(0.0);
    if let Some(object) = result.as_object_mut() {
        // Additive zo extras: keep the legacy `model`/`iterations` keys and a
        // human-formatted cost alongside the SDK fields so existing consumers
        // (and the ndjson byte-contract fixtures) keep working.
        object.insert("model".into(), serde_json::Value::from(model));
        object.insert("iterations".into(), serde_json::Value::from(summary.iterations));
        object.insert(
            "estimated_cost".into(),
            serde_json::Value::from(runtime::format_usd(total_cost_usd)),
        );
    }
    // ACP enables serde_json's insertion-ordered map backend transitively.
    // Keep Zo's existing byte contract independent of feature unification.
    sort_json_keys(&mut result);
    serde_json::to_writer(&mut writer, &result)?;
    writer.write_all(b"\n")?;
    Ok(())
}

fn sort_json_keys(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            let mut entries = std::mem::take(object).into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            for (_, child) in &mut entries {
                sort_json_keys(child);
            }
            object.extend(entries);
        }
        serde_json::Value::Array(values) => values.iter_mut().for_each(sort_json_keys),
        _ => {}
    }
}

/// Drive a turn through the **streaming** runtime path headlessly, forwarding
/// every [`RenderBlock`] (including the synthetic `session start` banner) into
/// `block_tx` *live* as it arrives. The caller owns the receiver and decides
/// where blocks go — stdout ndjson ([`drive_ndjson_stream`]), a `zo serve`
/// socket, or any other consumer.
///
/// Permission prompts that reach the prompter are auto-denied (no human is
/// attached) via [`runtime::permission::HeadlessPermissionPrompter`]. Returns
/// the turn summary so the caller can write any terminal result event.
///
/// The forwarding loop ends when the turn drops its internal sender; if the
/// caller's receiver is dropped first, the forwarder cancels the in-flight turn
/// so a disconnected output consumer cannot leave an expensive model request
/// running to completion. How a streamed turn resolves permission gates the
/// policy cannot decide.
///
/// `drive_render_stream` is shared by the non-interactive `-p` path and the
/// `zo serve` socket path, which differ only in *who answers* a prompt:
/// nobody (a fixed headless policy) vs. an attached client (forwarded over the
/// socket via [`super::socket_permission::SocketPermissionPrompter`]). The
/// socket prompter must bind to the turn's render channel, which only exists
/// inside `drive_render_stream`, so the caller passes a *kind* and the prompter
/// is built there.
pub(crate) enum StreamPrompter {
    /// Resolve residual prompts from a fixed policy (no human attached).
    Headless(runtime::permission::HeadlessDecision),
    /// Forward prompts to an attached client and await its socketed decision.
    Socket(super::socket_permission::SocketPrompterConfig),
    /// Delegate prompts to another protocol adapter.
    External(std::sync::Arc<dyn runtime::permission::PermissionPrompter>),
}

pub(crate) async fn drive_render_stream(
    rt: &mut runtime::ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>,
    live_client: std::sync::Arc<super::runtime_bridge::LiveAsyncApiClient>,
    input: String,
    model: &str,
    block_tx: tokio::sync::mpsc::Sender<runtime::message_stream::RenderBlock>,
    prompter_kind: StreamPrompter,
) -> Result<runtime::TurnSummary, String> {
    use runtime::message_stream::{BlockIdGen, RenderBlock, SystemLevel};

    rt.set_async_api_client(live_client);
    let (render_tx, mut render_rx) = tokio::sync::mpsc::channel::<RenderBlock>(64);
    let prompter: std::sync::Arc<dyn runtime::permission::PermissionPrompter> = match prompter_kind
    {
        StreamPrompter::Headless(decision) => std::sync::Arc::new(
            runtime::permission::HeadlessPermissionPrompter::new(decision),
        ),
        // The socket prompter emits its prompt frames on the same render
        // channel the turn writes to, so they stream to the client in order.
        StreamPrompter::Socket(config) => std::sync::Arc::new(
            super::socket_permission::SocketPermissionPrompter::new(render_tx.clone(), config),
        ),
        StreamPrompter::External(prompter) => prompter,
    };

    let init = RenderBlock::System {
        id: BlockIdGen::default().next(),
        level: SystemLevel::Info,
        text: format!("session start · model {model}"),
    };
    // Announce the model first so a consumer can frame the stream, mirroring the
    // legacy stdout path where the init banner was emitted before the turn ran.
    // If the consumer is already gone, do not start the expensive turn.
    block_tx
        .send(init)
        .await
        .map_err(|_| "render stream receiver dropped".to_string())?;
    // Route through the deep-gate-aware dispatcher: when a reactive/plan-first
    // `DeepGateConfig` is installed (headless coding turns, `/goal`, plan-first
    // automation) this runs the implement→verify→retry loop. With no gate it
    // falls back to `run_turn_streaming_with_images` — identical output for the
    // default config; it additionally honors any configured `TurnEnd` Stop hook
    // (a no-op unless `TurnEnd` rules exist), matching the sync `run_turn` path.
    // Shared by `-p` ndjson and serve.
    let turn = rt.run_turn_streaming_maybe_deep(input, Vec::new(), render_tx, prompter);
    // Forward blocks to the caller as they arrive; finishes when the turn drops
    // its sender. Runs concurrently with the turn.
    let forward = async {
        while let Some(block) = render_rx.recv().await {
            block_tx.send(block).await.map_err(|_| ())?;
        }
        Ok::<(), ()>(())
    };
    tokio::pin!(turn);
    tokio::pin!(forward);
    let turn_result = tokio::select! {
        turn_result = &mut turn => {
            let _ = (&mut forward).await;
            turn_result
        }
        forward_result = &mut forward => match forward_result {
            Ok(()) => (&mut turn).await,
            Err(()) => return Err("render stream receiver dropped".to_string()),
        },
    };
    turn_result.map_err(|error| error.to_string())
}

async fn drain_render_blocks<S>(
    mut block_rx: tokio::sync::mpsc::Receiver<runtime::message_stream::RenderBlock>,
    mut sink: S,
) -> Result<(), zo_cli::sinks::SinkError>
where
    S: zo_cli::sinks::Sink,
{
    while let Some(block) = block_rx.recv().await {
        sink.emit(&block)?;
    }
    Box::new(sink).finalize()
}

/// Drive a turn through the streaming runtime path headlessly and DISCARD
/// every [`RenderBlock`](runtime::message_stream::RenderBlock): the
/// `--output-format json` contract is a single terminal result object on
/// stdout, but the turn itself must still cross the deep-gate-aware
/// dispatcher (`run_turn_streaming_maybe_deep`) — the sync `run_turn` loop
/// ignores an installed `DeepGateConfig` entirely, so a format driven through
/// it silently loses reactive auto-verify and the plan-first automation gate
/// (the exact defect that kept headless json coding turns un-verified while
/// their gate-install predicate tested green). Residual permission prompts
/// deny, same as the ndjson path: machine stdout has no human attached.
pub(crate) async fn drive_discarding_stream(
    rt: &mut runtime::ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>,
    live_client: std::sync::Arc<super::runtime_bridge::LiveAsyncApiClient>,
    input: String,
    model: &str,
) -> Result<runtime::TurnSummary, String> {
    use runtime::message_stream::RenderBlock;

    let (block_tx, mut block_rx) = tokio::sync::mpsc::channel::<RenderBlock>(64);
    // The receiver must be drained (not dropped): `drive_render_stream` treats
    // a closed receiver as "consumer gone" and aborts the turn.
    let drain = async move { while block_rx.recv().await.is_some() {} };
    let (turn_result, ()) = tokio::join!(
        drive_render_stream(
            rt,
            live_client,
            input,
            model,
            block_tx,
            // Stdout `-p` has no human attached: deny residual prompts.
            StreamPrompter::Headless(runtime::permission::HeadlessDecision::DenyAll),
        ),
        drain
    );
    turn_result
}

/// Drive a turn through the streaming runtime path headlessly, rendering the
/// blocks a text one-shot shows: assistant text through the shared markdown
/// streamer (the same `push`/`flush` cadence the retired sync-client stdout
/// emission used, ANSI-stripped for `NO_COLOR`/non-TTY) and tool-call start
/// cards from the structured preview. Tool RESULTS are deliberately not
/// rendered here — the executor prints them at execution time
/// (`emit_output`), byte-identical to the sync path. Reasoning, system notes
/// and usage stay off stdout, as before. The prompter is the caller's choice:
/// the text path passes a terminal y/N prompter so interactive approvals keep
/// working.
pub(crate) async fn drive_text_stream(
    rt: &mut runtime::ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>,
    live_client: std::sync::Arc<super::runtime_bridge::LiveAsyncApiClient>,
    input: String,
    model: &str,
    prompter: StreamPrompter,
) -> Result<runtime::TurnSummary, String> {
    use std::io::IsTerminal as _;
    use std::io::Write as _;

    use runtime::message_stream::{RenderBlock, ToolCallStatus};

    let (block_tx, mut block_rx) = tokio::sync::mpsc::channel::<RenderBlock>(64);
    let drain = async move {
        let renderer = crate::render::TerminalRenderer::new();
        let mut markdown = crate::render::MarkdownStreamState::default();
        let strip = crate::render::no_color_env() || !std::io::stdout().is_terminal();
        let mut out = crate::runtime_support::StripAnsiWriter::new(std::io::stdout(), strip);
        // One call streams as SEVERAL ToolCall blocks with the same
        // tool_call_id (the wire parser announces it, the dispatch loop
        // re-sends it with its permission-settled status) — the TUI coalesces
        // them by id in place, an append-only transcript must print the card
        // exactly once. The map also names each call's ToolResult, which
        // carries only the id.
        let mut announced_calls: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        while let Some(block) = block_rx.recv().await {
            match block {
                RenderBlock::TextDelta { text, done, .. } => {
                    if !text.is_empty() {
                        if let Some(rendered) = markdown.push(&renderer, &text) {
                            let _ = write!(out, "{rendered}").and_then(|()| out.flush());
                        }
                    }
                    if done {
                        if let Some(rendered) = markdown.flush(&renderer) {
                            let _ = write!(out, "{rendered}").and_then(|()| out.flush());
                        }
                    }
                }
                RenderBlock::ToolCall {
                    tool_call_id,
                    name,
                    summary,
                    preview,
                    status,
                    ..
                } => {
                    if status == ToolCallStatus::Running
                        && !announced_calls.contains_key(&tool_call_id.0)
                    {
                        let card = crate::tool_formatting::format_tool_call_start_from_preview(
                            &name, &preview, &summary,
                        );
                        announced_calls.insert(tool_call_id.0.clone(), name);
                        let _ = writeln!(out, "\n{card}").and_then(|()| out.flush());
                    }
                }
                RenderBlock::ToolResult {
                    tool_call_id,
                    is_error,
                    body,
                    ..
                } => {
                    // The concurrent dispatch closure the streaming loop
                    // executes tools through never writes to stdout (the
                    // serial executor's `emit_output` branch is sync-loop
                    // machinery), so the result renders here — from the same
                    // structured body the TUI shows.
                    let name = announced_calls
                        .get(&tool_call_id.0)
                        .cloned()
                        .or_else(|| match &body {
                            runtime::message_stream::ToolResultBody::Generic {
                                name, ..
                            } => Some(name.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| "tool".to_string());
                    let result_text = crate::tool_formatting::format_tool_result_from_body(
                        &name, is_error, &body,
                    );
                    let _ = writeln!(out, "{result_text}").and_then(|()| out.flush());
                }
                _ => {}
            }
        }
        // Channel closed mid-message (error paths): flush any buffered tail so
        // partial assistant text is not silently dropped.
        if let Some(rendered) = markdown.flush(&renderer) {
            let _ = write!(out, "{rendered}").and_then(|()| out.flush());
        }
    };
    let (turn_result, ()) = tokio::join!(
        drive_render_stream(rt, live_client, input, model, block_tx, prompter),
        drain
    );
    turn_result
}

/// Drive a turn through the streaming runtime path headlessly, emitting each
/// [`RenderBlock`] as a typed ndjson line *live* to stdout (text deltas, tool
/// calls/results, usage) instead of replaying a post-hoc summary. Thin stdout
/// adapter over [`drive_render_stream`]: it owns the receiver and funnels every
/// block through an [`NdjsonSink`] under a single locked stdout. Returns the
/// turn summary so the caller can write the terminal result event.
pub(crate) async fn drive_ndjson_stream(
    rt: &mut runtime::ConversationRuntime<crate::AnthropicRuntimeClient, crate::CliToolExecutor>,
    live_client: std::sync::Arc<super::runtime_bridge::LiveAsyncApiClient>,
    input: String,
    model: &str,
) -> Result<runtime::TurnSummary, String> {
    use runtime::message_stream::RenderBlock;
    use zo_cli::sinks::NdjsonSink;

    let (block_tx, block_rx) = tokio::sync::mpsc::channel::<RenderBlock>(64);
    let drain = async {
        let stdout = std::io::stdout();
        drain_render_blocks(block_rx, NdjsonSink::new(stdout.lock())).await
    };
    let (turn_result, drain_result) = tokio::join!(
        drive_render_stream(
            rt,
            live_client,
            input,
            model,
            block_tx,
            // Stdout `-p` has no human attached: deny residual prompts.
            StreamPrompter::Headless(runtime::permission::HeadlessDecision::DenyAll),
        ),
        drain
    );
    drain_result.map_err(|error| error.to_string())?;
    turn_result
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use runtime::message_stream::{BlockIdGen, RenderBlock, SystemLevel};
    use zo_cli::sinks::NdjsonSink;

    use super::drain_render_blocks;

    struct FailAfterEventsWriter {
        allowed_events: usize,
        attempted_events: Arc<AtomicUsize>,
        at_event_start: bool,
    }

    impl io::Write for FailAfterEventsWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.is_empty() {
                return Ok(0);
            }
            if self.at_event_start {
                let attempted = self.attempted_events.fetch_add(1, Ordering::SeqCst) + 1;
                if attempted > self.allowed_events {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "ndjson writer closed",
                    ));
                }
                self.at_event_start = false;
            }
            if bytes.ends_with(b"\n") {
                self.at_event_start = true;
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn ndjson_sink_error_stops_drain_after_failing_block() {
        let attempted_events = Arc::new(AtomicUsize::new(0));
        let writer = FailAfterEventsWriter {
            allowed_events: 1,
            attempted_events: Arc::clone(&attempted_events),
            at_event_start: true,
        };
        let (block_tx, block_rx) = tokio::sync::mpsc::channel(3);
        let ids = BlockIdGen::default();
        for text in ["first", "second", "must not be consumed"] {
            block_tx
                .send(RenderBlock::System {
                    id: ids.next(),
                    level: SystemLevel::Info,
                    text: text.to_string(),
                })
                .await
                .expect("queue render block");
        }
        drop(block_tx);

        let result = drain_render_blocks(block_rx, NdjsonSink::new(writer)).await;

        assert!(result.is_err(), "the first sink error must be returned");
        assert_eq!(
            attempted_events.load(Ordering::SeqCst),
            2,
            "the drain must drop its receiver without consuming later blocks"
        );
    }
}
