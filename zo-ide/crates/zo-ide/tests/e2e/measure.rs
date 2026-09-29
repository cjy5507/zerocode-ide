//! A stateful loopback provider for the r23 performance measurements.
//!
//! [`super::scripted::ScriptedAnthropicService`] answers by *request ordinal*,
//! which is exactly wrong for the three things r23 has to drive: a long session
//! whose turns repeat, a compaction round that interleaves a summary request
//! among them, and sub-agents whose own requests arrive concurrently with the
//! parent's. This server answers by **what the request contains** instead, so
//! order and concurrency stop mattering:
//!
//! | the request's last message | the answer |
//! |---|---|
//! | carries the compaction prompt | an 8-section summary |
//! | is a `tool_result` | the final text of that turn |
//! | mentions `R23_SPAWN` | `children` × `Agent`, run in the foreground |
//! | mentions `R23_TOOLS` | `tools` × `bash`, in one message |
//! | mentions `R23_BIG` | one `bash` whose output is `payload_lines` long |
//! | mentions `R23_SLEEP` | one `bash` that sleeps `child_sleep_secs` |
//! | anything else | the final text |
//!
//! And the t-11354 conversation — a person talking to the main while its
//! background agent works — which reads the LAST message the same way:
//!
//! | the request | the answer |
//! |---|---|
//! | the child's (first message names [`TALK_CHILD`]) | `bash` steps of `child_sleep_secs`, `child_steps` of them, then [`TALK_CHILD_DONE`] — with `child_says_at`, one `SendMessage(to: "main")` of [`TALK_CHILD_SAYS`] at that step |
//! | the last message asks ([`TALK_ASK`] n) | [`TALK_ANSWER`] n for each n asked |
//! | the last message carries the child's result | [`TALK_NOTED`] |
//! | the turn's user messages carry the child's words | [`TALK_HEARD`] (and [`TALK_NOTED`] when its result came too) |
//! | the last message says [`TALK_DELEGATE`] | one background `Agent` |
//! | the last message says [`TALK_WAIT`] | one blocking `Agent` (`background: false`) |
//! | the last message says [`TALK_SLOW`] | one foreground `bash` of a few seconds — no wait |
//! | the last message answers that `Agent` call | a reply that ends on "I'll …" |
//! | the last message is the turn-end gate's | the poll the 2026-09-27 main ran: `bash` sleeping [`TALK_POLL_SECS`] |
//!
//! And the t-16031 stop — two background helpers beside one piece of the main's
//! own work, one helper the person stops by id while the other runs on. Read
//! from the FIRST message for a helper's own requests, and from the last for
//! the main's:
//!
//! | the request | the answer |
//! |---|---|
//! | the first helper's (first message names [`HSTOP_ALPHA`]) | its prose ([`HSTOP_ALPHA_PARTIAL`]) and one long `bash` step; [`HSTOP_ALPHA_DONE`] only if that step ever returns |
//! | the second helper's (first message names [`HSTOP_BETA`]) | one short `bash` step, then [`HSTOP_BETA_DONE`] |
//! | the last message carries a helper's notice | [`HSTOP_NOTED`] |
//! | the last message answers the main's own step | [`HSTOP_MAIN_DONE`] |
//! | the last message answers the two spawns | one foreground `bash` of a few seconds — the main's own work, which keeps its turn open |
//! | the last message says [`HSTOP_GO`] | two background `Agent`s |
//!
//! `latency_ms` holds every answer that long — the model's own latency, the
//! part of a reply no harness can take away.
//!
//! Every recorded body is kept with its arrival instant, because half of what
//! r23 asks is not about the screen at all — it is about what compaction does
//! to the *bytes on the wire*.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use super::scripted::COMPACTION_MARKER;

/// The text every scripted turn ends with, so the PTY driver can wait for the
/// turn to settle without matching anything the composer echoed.
pub const TURN_SETTLED: &str = "r23-turn-settled";

/// A CHILD's final text. Distinct from [`TURN_SETTLED`] because a child's
/// report is rendered into the parent's transcript: sharing one marker made
/// the driver stop the clock on the first child and report three serial
/// children as one 3-second turn.
pub const CHILD_SETTLED: &str = "r23-child-settled";

/// The person hands the main a task it delegates to a background agent.
pub const TALK_DELEGATE: &str = "TALK_DELEGATE";
/// What the delegated agent is told — and how its requests are known.
pub const TALK_CHILD: &str = "TALK_CHILD";
/// The delegated agent's final words.
pub const TALK_CHILD_DONE: &str = "TALK_CHILD_DONE";
/// The person asks the main something while the agent works: `TALK_ASK <n>`.
pub const TALK_ASK: &str = "TALK_ASK";
/// The main's answer to ask `<n>`: `TALK_ANSWER <n>`.
pub const TALK_ANSWER: &str = "TALK_ANSWER";
/// The main's words once the agent's result has reached it.
pub const TALK_NOTED: &str = "TALK_NOTED";
/// What the delegated agent tells the main mid-run, with `SendMessage`.
pub const TALK_CHILD_SAYS: &str = "TALK_CHILD_SAYS";
/// The main's words once the agent's mid-run message has reached it.
pub const TALK_HEARD: &str = "TALK_HEARD";
/// The person hands the main a task it runs in a blocking `Agent` call.
pub const TALK_WAIT: &str = "TALK_WAIT";
/// The id of the child's `SendMessage` call, to find its answer by.
pub const TALK_SAYS_ID: &str = "toolu_talk_says";
/// The person asks for something the main does itself, in the foreground.
pub const TALK_SLOW: &str = "TALK_SLOW";
/// The foreground command [`TALK_SLOW`] runs: a few seconds, not a wait.
pub const TALK_SLOW_COMMAND: &str = "sleep 5 && printf 'talk slow done'";
/// The end of the main's reply after delegating: a promise about the result.
pub const TALK_PROMISE: &str = "I'll report once it passes.";
/// How long the main's poll sleeps in all, in short sleeps inside a `for`
/// loop — the shape of the loops that held the person's words for hours.
pub const TALK_POLL_SECS: u64 = 60;
const TALK_POLL_STEP_SECS: u64 = 2;
const TALK_SPAWN_ID: &str = "toolu_talk_spawn";
const TALK_WAIT_ID: &str = "toolu_talk_wait";

/// The person hands the main two helpers to run beside each other, and asks
/// for one piece of work of its own while they run (t-16031).
pub const HSTOP_GO: &str = "HSTOP_GO";
/// What the first helper is told — and how its requests are known. It works a
/// little, then takes one long step that nobody lets it finish.
pub const HSTOP_ALPHA: &str = "HSTOP_ALPHA";
/// What the second helper is told. It takes a short step and finishes on its
/// own.
pub const HSTOP_BETA: &str = "HSTOP_BETA";
/// The first helper's prose before its long step: what it has done when the
/// person stops it.
pub const HSTOP_ALPHA_PARTIAL: &str = "HSTOP_ALPHA_PARTIAL";
/// The first helper's final words, which only a helper nobody stopped reaches.
pub const HSTOP_ALPHA_DONE: &str = "HSTOP_ALPHA_DONE";
/// The second helper's final words.
pub const HSTOP_BETA_DONE: &str = "HSTOP_BETA_DONE";
/// The main's words once a helper's notice has reached it.
pub const HSTOP_NOTED: &str = "HSTOP_NOTED";
/// The main's closing words after its own step.
pub const HSTOP_MAIN_DONE: &str = "HSTOP_MAIN_DONE";
/// What the main's own step prints, so its result is known by content.
const HSTOP_MAIN_WORK_OUTPUT: &str = "hstop main work";
/// Seconds of the main's own step: long enough to stop a helper while it runs,
/// short enough that no test waits on it.
const HSTOP_MAIN_WORK_SECS: u64 = 10;
/// Seconds of the second helper's step.
const HSTOP_BETA_STEP_SECS: u64 = 2;
/// Seconds of the first helper's long step — never waited for: the stop kills
/// it, and a run where the stop failed fails on the assertion, not on this.
const HSTOP_ALPHA_STEP_SECS: u64 = 120;
const HSTOP_ALPHA_SPAWN_ID: &str = "toolu_hstop_alpha";
const HSTOP_BETA_SPAWN_ID: &str = "toolu_hstop_beta";
const HSTOP_MAIN_STEP_ID: &str = "toolu_hstop_main";
/// The first words of the header a helper's notice opens with
/// (`background_agent_notification_header`) — how the main's request that
/// carries a notice is known.
pub const HELPER_NOTICE_HEAD: &str = "[task notification";

/// What this server should do with a turn request.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    /// `bash` calls emitted in ONE assistant message for an `R23_TOOLS` turn.
    pub tools: usize,
    /// `Agent` calls emitted in ONE assistant message for an `R23_SPAWN` turn.
    pub children: usize,
    /// Distinct lines a `R23_BIG` tool result carries. Distinct, because the
    /// context compressor collapses repeated runs — an 8k blob of the same
    /// line would arrive as three lines and the session would never grow.
    pub payload_lines: usize,
    /// Seconds an `R23_SPAWN` child's own tool sleeps, so three children
    /// overlap for long enough to be sampled.
    pub child_sleep_secs: u64,
    /// The `background` field on each spawned `Agent`.
    ///
    /// Not a detail: `false` blocks the parent's tool dispatch until the child
    /// finishes, and `Agent` is not in `is_concurrency_safe`, so a batch of
    /// blocking spawns runs one at a time. `true` is what the interactive
    /// session actually defaults to. The two answers are different by an
    /// order of magnitude, so the measurement has to name which it took.
    pub background: bool,
    /// How long every answer is held before it is written — the model's own
    /// latency. `0` answers at once, as the r23 measurements always did.
    pub latency_ms: u64,
    /// `bash` steps a [`TALK_CHILD`] agent takes before its final words.
    pub child_steps: usize,
    /// The step at which a [`TALK_CHILD`] agent sends [`TALK_CHILD_SAYS`] to
    /// the main with `SendMessage`, as one step more; `None` sends nothing.
    pub child_says_at: Option<usize>,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            tools: 8,
            children: 3,
            payload_lines: 90,
            child_sleep_secs: 3,
            background: false,
            latency_ms: 0,
            child_steps: 0,
            child_says_at: None,
        }
    }
}

/// One recorded provider request.
#[derive(Debug, Clone)]
pub struct Recorded {
    /// Milliseconds since the server started — a shared clock for lining
    /// requests up against the per-turn wall clock the driver keeps.
    pub at_ms: u128,
    /// The verbatim request body.
    pub body: String,
    /// Whether this was the compaction summary round-trip.
    pub compaction_summary: bool,
}

impl Recorded {
    /// The `messages` array, serialized exactly as it arrived.
    ///
    /// The prefix comparison this exists for is a byte question — a provider's
    /// cache matches serialized prefixes, not parsed trees — so it must keep
    /// the wire order rather than round-tripping through a map.
    #[must_use]
    pub fn messages_json(&self) -> String {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|value| value.get("messages").cloned())
            .map(|messages| messages.to_string())
            .unwrap_or_default()
    }

    /// Whether this request's FIRST message carries `needle` — the cheapest
    /// way to tell a sub-agent's own conversation from the parent's.
    #[must_use]
    pub fn opens_with(&self, needle: &str) -> bool {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|value| {
                value
                    .get("messages")
                    .and_then(Value::as_array)
                    .and_then(|messages| messages.first())
                    .map(Value::to_string)
            })
            .is_some_and(|first| first.contains(needle))
    }

    #[must_use]
    pub fn message_count(&self) -> usize {
        serde_json::from_str::<Value>(&self.body)
            .ok()
            .and_then(|value| {
                value
                    .get("messages")
                    .and_then(Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or_default()
    }
}

/// A deterministic loopback provider that answers by request CONTENT.
pub struct MeasureService {
    base_url: String,
    recorded: Arc<Mutex<Vec<Recorded>>>,
    shutdown: Option<oneshot::Sender<()>>,
    join_handle: JoinHandle<()>,
}

impl MeasureService {
    pub async fn start(plan: Plan) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let nonces = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let state = Arc::clone(&recorded);
        let started = Instant::now();
        let join_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break };
                        let state = Arc::clone(&state);
                        let nonces = Arc::clone(&nonces);
                        tokio::spawn(async move {
                            let _ = serve(socket, state, nonces, plan, started).await;
                        });
                    }
                }
            }
        });
        Ok(Self {
            base_url: format!("http://{address}"),
            recorded,
            shutdown: Some(shutdown_tx),
            join_handle,
        })
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn recorded(&self) -> Vec<Recorded> {
        self.recorded.lock().await.clone()
    }
}

impl Drop for MeasureService {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.join_handle.abort();
    }
}

async fn serve(
    mut socket: TcpStream,
    recorded: Arc<Mutex<Vec<Recorded>>>,
    nonces: Arc<AtomicUsize>,
    plan: Plan,
    started: Instant,
) -> io::Result<()> {
    let (method, body) = read_http_request(&mut socket).await?;
    if !method.eq_ignore_ascii_case("POST") {
        socket
            .write_all(http_response("text/plain", "").as_bytes())
            .await?;
        return Ok(());
    }
    let answer = answer_for(&body, plan);
    if plan.latency_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(plan.latency_ms)).await;
    }
    let nonce = nonces.fetch_add(1, Ordering::Relaxed);
    let input_tokens = approximate_input_tokens(&body);
    if std::env::var_os("MEASURE_TRACE").is_some() {
        eprintln!(
            "[r23-wire] {}ms answer={answer:?} body={}B messages={}",
            started.elapsed().as_millis(),
            body.len(),
            serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| value
                    .get("messages")
                    .and_then(Value::as_array)
                    .map(Vec::len))
                .unwrap_or_default()
        );
    }
    recorded.lock().await.push(Recorded {
        at_ms: started.elapsed().as_millis(),
        body,
        compaction_summary: matches!(answer, Answer::Summary),
    });
    socket
        .write_all(
            http_response(
                "text/event-stream",
                &answer.sse(plan, nonce, input_tokens),
            )
            .as_bytes(),
        )
        .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Summary,
    Tools,
    Spawn,
    Big,
    Sleep,
    ChildText,
    Text,
    TalkDelegate,
    TalkPromise,
    TalkPoll,
    TalkChildStep(usize),
    TalkChildSays,
    TalkChildDone,
    /// The child's words reached the main — with its result, when that came
    /// in the same turn.
    TalkHeard(bool),
    TalkWait,
    /// The asks to answer, and whether the child's result came with them.
    TalkReply(Vec<String>, bool),
    TalkNoted,
    TalkSlow,
    /// The two background helpers of the t-16031 stop.
    HstopSpawn,
    /// The main's own foreground step, which keeps its turn open.
    HstopMainWork,
    HstopMainDone,
    /// A helper's notice reached the main.
    HstopNoted,
    /// The first helper: its prose, then one long step.
    HstopAlphaStep,
    HstopAlphaDone,
    /// The second helper: one short step, then its words.
    HstopBetaStep,
    HstopBetaDone,
}

impl Answer {
    fn sse(&self, plan: Plan, nonce: usize, input_tokens: u32) -> String {
        match self {
            Self::TalkDelegate => talk_delegate_sse(plan, input_tokens),
            Self::TalkPromise => text_sse(
                "msg_talk_promise",
                &format!("The agent is working in the background; its result comes back here when it lands.\n\n{TALK_PROMISE}"),
                input_tokens,
            ),
            Self::TalkPoll => talk_bash_sse(
                "toolu_talk_poll",
                &format!(
                    "for i in $(seq 1 {}); do sleep {TALK_POLL_STEP_SECS}; done; echo polled",
                    TALK_POLL_SECS / TALK_POLL_STEP_SECS
                ),
                TALK_POLL_SECS * 2 * 1000,
                input_tokens,
            ),
            Self::TalkChildStep(step) => talk_bash_sse(
                &format!("toolu_talk_child_{step}"),
                &format!("sleep {} && printf 'talk child step {step}'", plan.child_sleep_secs),
                60_000,
                input_tokens,
            ),
            Self::TalkSlow => talk_bash_sse("toolu_talk_slow", TALK_SLOW_COMMAND, 20_000, input_tokens),
            Self::HstopSpawn => hstop_spawn_sse(input_tokens),
            Self::HstopMainWork => talk_bash_sse(
                HSTOP_MAIN_STEP_ID,
                &format!("sleep {HSTOP_MAIN_WORK_SECS} && printf '{HSTOP_MAIN_WORK_OUTPUT}'"),
                60_000,
                input_tokens,
            ),
            Self::HstopMainDone => text_sse(
                "msg_hstop_main_done",
                &format!("### Main\n\n- {HSTOP_MAIN_DONE}\n"),
                input_tokens,
            ),
            Self::HstopNoted => text_sse(
                "msg_hstop_noted",
                &format!("### Notice\n\n- {HSTOP_NOTED}\n"),
                input_tokens,
            ),
            Self::HstopAlphaStep => hstop_alpha_step_sse(input_tokens),
            Self::HstopAlphaDone => text_sse(
                "msg_hstop_alpha_done",
                &format!("### Done\n\n- {HSTOP_ALPHA_DONE}\n"),
                input_tokens,
            ),
            Self::HstopBetaStep => talk_bash_sse(
                "toolu_hstop_beta_step",
                &format!("sleep {HSTOP_BETA_STEP_SECS} && printf 'hstop beta step'"),
                60_000,
                input_tokens,
            ),
            Self::HstopBetaDone => text_sse(
                "msg_hstop_beta_done",
                &format!("### Done\n\n- {HSTOP_BETA_DONE}\n"),
                input_tokens,
            ),
            Self::TalkChildSays => talk_says_sse(input_tokens),
            Self::TalkHeard(noted) => text_sse(
                "msg_talk_heard",
                &if *noted {
                    format!("### Heard\n\n- {TALK_HEARD}\n- {TALK_NOTED}\n")
                } else {
                    format!("### Heard\n\n- {TALK_HEARD}\n")
                },
                input_tokens,
            ),
            Self::TalkWait => talk_wait_sse(plan, input_tokens),
            Self::TalkChildDone => text_sse(
                "msg_talk_child_done",
                &format!("### Done\n\n- {TALK_CHILD_DONE}\n"),
                input_tokens,
            ),
            Self::TalkReply(asks, noted) => talk_reply_sse(asks, *noted, input_tokens),
            Self::TalkNoted => text_sse(
                "msg_talk_noted",
                &format!("### Result\n\n- {TALK_NOTED}\n"),
                input_tokens,
            ),
            Self::Summary => text_sse("msg_r23_summary", &summary_body(), input_tokens),
            Self::Tools => tools_sse(plan.tools, input_tokens),
            Self::Spawn => spawn_sse(plan, input_tokens),
            Self::Big => big_output_sse(plan.payload_lines, nonce, input_tokens),
            Self::Sleep => sleep_sse(plan.child_sleep_secs, input_tokens),
            Self::ChildText => text_sse(
                "msg_r23_child",
                &format!("### Done\n\n- {CHILD_SETTLED}\n"),
                input_tokens,
            ),
            Self::Text => text_sse(
                "msg_r23_text",
                &format!("### Done\n\n- {TURN_SETTLED}\n"),
                input_tokens,
            ),
        }
    }
}

/// What a provider would report as this request's input size.
///
/// A mock that always answers `input_tokens: 12` is not merely imprecise — the
/// live footer reads the provider's number (`RenderBlock::Usage.ctx_tokens`,
/// deliberately NOT the local estimate), so a constant makes the screen read
/// `100% context left` for a session of any length and makes
/// `effective_context_tokens` fall back to the estimate alone. Four characters
/// per token over the whole serialized request is the same conversion the
/// runtime's own estimator uses, applied to the same bytes the provider sees.
fn approximate_input_tokens(body: &str) -> u32 {
    u32::try_from(body.len().div_ceil(4)).unwrap_or(u32::MAX)
}

/// Decide the answer from the request body alone.
///
/// Deliberately reads only the LAST message: the trigger words stay in the
/// transcript forever once typed, so a whole-body search would keep re-firing
/// the first turn's tool burst on every later turn of the same session.
fn answer_for(body: &str, plan: Plan) -> Answer {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Answer::Text;
    };
    let Some(last) = value
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.last())
    else {
        return Answer::Text;
    };
    if let Some(stop) = hstop_answer(&value, last) {
        return stop;
    }
    if let Some(talk) = talk_answer(&value, last, plan) {
        return talk;
    }
    let child = value
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| messages.first())
        .is_some_and(|first| first.to_string().contains("R23_SLEEP"));
    let blocks = last.get("content").and_then(Value::as_array);
    if blocks.is_some_and(|blocks| {
        blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
    }) {
        return if child { Answer::ChildText } else { Answer::Text };
    }
    let text = last.to_string();
    if text.contains(COMPACTION_MARKER) {
        return Answer::Summary;
    }
    if text.contains("R23_SPAWN") {
        return Answer::Spawn;
    }
    if text.contains("R23_TOOLS") {
        return Answer::Tools;
    }
    if text.contains("R23_BIG") {
        return Answer::Big;
    }
    if text.contains("R23_SLEEP") {
        return Answer::Sleep;
    }
    Answer::Text
}

/// The one background `Agent` the main hands its task to.
fn talk_delegate_sse(plan: Plan, input_tokens: u32) -> String {
    let mut body = message_start("msg_talk_delegate", input_tokens);
    append_tool_use(
        &mut body,
        0,
        TALK_SPAWN_ID,
        "Agent",
        &json!({
            "description": "talk child",
            "subagent_type": "general-purpose",
            "background": true,
            "prompt": format!("{TALK_CHILD}: take {} steps and report", plan.child_steps),
        })
        .to_string(),
    );
    finish_tool_message(&mut body, input_tokens);
    body
}

/// The child's word to the main while it works (t-11459).
fn talk_says_sse(input_tokens: u32) -> String {
    let mut body = message_start("msg_talk_says", input_tokens);
    append_tool_use(
        &mut body,
        0,
        TALK_SAYS_ID,
        "SendMessage",
        &json!({
            "to": "main",
            "message": format!("{TALK_CHILD_SAYS}: the flag lives in config.rs"),
        })
        .to_string(),
    );
    finish_tool_message(&mut body, input_tokens);
    body
}

/// The one blocking `Agent` the main runs its task in (t-11460).
fn talk_wait_sse(plan: Plan, input_tokens: u32) -> String {
    let mut body = message_start("msg_talk_wait", input_tokens);
    append_tool_use(
        &mut body,
        0,
        TALK_WAIT_ID,
        "Agent",
        &json!({
            "description": "talk child",
            "subagent_type": "general-purpose",
            "background": false,
            "prompt": format!("{TALK_CHILD}: take {} steps and report", plan.child_steps),
        })
        .to_string(),
    );
    finish_tool_message(&mut body, input_tokens);
    body
}

/// One `bash` call of the t-11354 conversation.
fn talk_bash_sse(id: &str, command: &str, timeout_ms: u64, input_tokens: u32) -> String {
    let mut body = message_start("msg_talk_bash", input_tokens);
    append_tool_use(
        &mut body,
        0,
        id,
        "bash",
        &json!({"command": command, "timeout": timeout_ms}).to_string(),
    );
    finish_tool_message(&mut body, input_tokens);
    body
}

/// The main's answer to every ask the last message carried — and to the
/// agent's result, when it came with them.
fn talk_reply_sse(asks: &[String], noted: bool, input_tokens: u32) -> String {
    let mut text = asks
        .iter()
        .map(|ask| format!("- {TALK_ANSWER} {ask}"))
        .collect::<Vec<_>>()
        .join("\n");
    if noted {
        text.push_str("\n- ");
        text.push_str(TALK_NOTED);
    }
    text_sse("msg_talk_answer", &format!("### Status\n\n{text}\n"), input_tokens)
}

/// The t-11354 conversation's answer to this request, when it is part of it.
fn talk_answer(value: &Value, last: &Value, plan: Plan) -> Option<Answer> {
    let messages = value.get("messages").and_then(Value::as_array)?;
    let first = messages.first().map(Value::to_string).unwrap_or_default();
    if first.contains(TALK_CHILD) && !first.contains(TALK_DELEGATE) {
        let results = messages
            .iter()
            .filter_map(|message| message.get("content").and_then(Value::as_array))
            .flatten()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            .count();
        let steps = plan.child_steps + usize::from(plan.child_says_at.is_some());
        return Some(if plan.child_says_at == Some(results) {
            Answer::TalkChildSays
        } else if results < steps {
            Answer::TalkChildStep(results)
        } else {
            Answer::TalkChildDone
        });
    }
    let text = last.to_string();
    if text.contains(COMPACTION_MARKER) {
        return None;
    }
    // The host may put a reminder of its own after the words that opened the
    // turn (a recall hint, for one), so the child's words are looked for in
    // every user message since the main last spoke.
    let turn = messages
        .iter()
        .rev()
        .take_while(|message| message.get("role").and_then(Value::as_str) != Some("assistant"))
        .map(Value::to_string)
        .collect::<String>();
    if turn.contains(TALK_CHILD_SAYS) {
        return Some(Answer::TalkHeard(turn.contains(TALK_CHILD_DONE)));
    }
    let noted = text.contains(TALK_CHILD_DONE);
    let asks: Vec<String> = text
        .match_indices(TALK_ASK)
        .filter_map(|(at, _)| {
            text[at + TALK_ASK.len()..]
                .split(|ch: char| !ch.is_ascii_digit() && ch != ' ')
                .next()
                .map(str::trim)
                .filter(|number| !number.is_empty())
                .map(str::to_string)
        })
        .collect();
    if !asks.is_empty() {
        return Some(Answer::TalkReply(asks, noted));
    }
    if noted {
        return Some(Answer::TalkNoted);
    }
    if text.contains(TALK_DELEGATE) {
        return Some(Answer::TalkDelegate);
    }
    if text.contains(TALK_WAIT) {
        return Some(Answer::TalkWait);
    }
    if text.contains(TALK_SLOW) && !text.contains("tool_result") {
        return Some(Answer::TalkSlow);
    }
    if text.contains(TALK_SPAWN_ID) {
        return Some(Answer::TalkPromise);
    }
    if text.contains("[zo:turn-end-gate]") && first.contains(TALK_DELEGATE) {
        return Some(Answer::TalkPoll);
    }
    None
}

/// The two background `Agent`s the main hands its work to in one message: the
/// first is the one the person will stop, the second the one that runs on.
fn hstop_spawn_sse(input_tokens: u32) -> String {
    let mut body = message_start("msg_hstop_spawn", input_tokens);
    for (index, (id, description, marker)) in [
        (HSTOP_ALPHA_SPAWN_ID, "hstop alpha", HSTOP_ALPHA),
        (HSTOP_BETA_SPAWN_ID, "hstop beta", HSTOP_BETA),
    ]
    .into_iter()
    .enumerate()
    {
        append_tool_use(
            &mut body,
            index,
            id,
            "Agent",
            &json!({
                "description": description,
                "subagent_type": "general-purpose",
                "background": true,
                "prompt": format!("{marker}: take your step and report"),
            })
            .to_string(),
        );
    }
    finish_tool_message(&mut body, input_tokens);
    body
}

/// The first helper's request: prose first (the work it has done), then one
/// long `bash` step in the same message — the state a stop finds it in.
fn hstop_alpha_step_sse(input_tokens: u32) -> String {
    let mut body = message_start("msg_hstop_alpha_step", input_tokens);
    append_sse(
        &mut body,
        "content_block_start",
        &json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
    );
    append_sse(
        &mut body,
        "content_block_delta",
        &json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "text_delta",
                "text": format!("{HSTOP_ALPHA_PARTIAL}: read the first two files, now running the long check.\n")
            }
        }),
    );
    append_sse(
        &mut body,
        "content_block_stop",
        &json!({"type": "content_block_stop", "index": 0}),
    );
    append_tool_use(
        &mut body,
        1,
        "toolu_hstop_alpha_step",
        "bash",
        &json!({
            "command": format!("sleep {HSTOP_ALPHA_STEP_SECS} && printf 'hstop alpha long step'"),
            "timeout": (HSTOP_ALPHA_STEP_SECS + 30) * 1000,
        })
        .to_string(),
    );
    finish_tool_message(&mut body, input_tokens);
    body
}

/// How many tool results a request's conversation already carries — how far
/// along a helper is.
fn tool_results_in(messages: &[Value]) -> usize {
    messages
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .count()
}

/// The t-16031 stop's answer to this request, when it is part of it.
fn hstop_answer(value: &Value, last: &Value) -> Option<Answer> {
    let messages = value.get("messages").and_then(Value::as_array)?;
    let first = messages.first().map(Value::to_string).unwrap_or_default();
    // A helper's own conversation opens with the brief it was handed; the
    // main's opens with the person's words, which name neither helper.
    if !first.contains(HSTOP_GO) {
        for (marker, step, done) in [
            (HSTOP_ALPHA, Answer::HstopAlphaStep, Answer::HstopAlphaDone),
            (HSTOP_BETA, Answer::HstopBetaStep, Answer::HstopBetaDone),
        ] {
            if first.contains(marker) {
                return Some(if tool_results_in(messages) == 0 { step } else { done });
            }
        }
        return None;
    }
    let text = last.to_string();
    if text.contains(COMPACTION_MARKER) {
        return None;
    }
    if text.contains(HSTOP_MAIN_WORK_OUTPUT) {
        return Some(Answer::HstopMainDone);
    }
    if text.contains(HELPER_NOTICE_HEAD) {
        return Some(Answer::HstopNoted);
    }
    if text.contains(HSTOP_ALPHA_SPAWN_ID) && text.contains(HSTOP_BETA_SPAWN_ID) {
        return Some(Answer::HstopMainWork);
    }
    if text.contains(HSTOP_GO) {
        return Some(Answer::HstopSpawn);
    }
    None
}

/// An eight-section summary with no backtick spans and no path-like tokens.
///
/// Both omissions are load-bearing: `summary_fabricates_identifiers` rejects a
/// summary whose cited identifiers are mostly ungrounded, and a rejected
/// summary silently falls back to the local extractor — the measurement would
/// then be of a code path the real provider never takes.
fn summary_body() -> String {
    "<analysis>\nThe session repeated one measured turn.\n</analysis>\n\n<summary>\n\
     1. Primary Request and Intent: repeat one scripted measurement turn until compaction fires.\n\
     2. Key Technical Concepts: repeated turns, a scripted provider, a growing transcript.\n\
     3. Files and Code Sections: none were edited during this session.\n\
     4. Errors and fixes: none occurred.\n\
     5. Problem Solving: the session simply repeated the same request.\n\
     6. All user messages: the user asked for the same measured turn each time.\n\
     7. Pending Tasks: continue repeating the measured turn.\n\
     8. Current Work: repeating the measured turn.\n</summary>\n"
        .to_string()
}

fn tools_sse(tools: usize, input_tokens: u32) -> String {
    let mut body = message_start("msg_r23_tools", input_tokens);
    for index in 0..tools {
        append_tool_use(
            &mut body,
            index,
            &format!("toolu_r23_tool_{index}"),
            "bash",
            &format!(r#"{{"command":"printf 'r23 probe {index}'","timeout":5000}}"#),
        );
    }
    finish_tool_message(&mut body, input_tokens);
    body
}

fn spawn_sse(plan: Plan, input_tokens: u32) -> String {
    let Plan {
        children,
        child_sleep_secs: sleep_secs,
        background,
        ..
    } = plan;
    let mut body = message_start("msg_r23_spawn", input_tokens);
    for index in 0..children {
        append_tool_use(
            &mut body,
            index,
            &format!("toolu_r23_child_{index}"),
            "Agent",
            &format!(
                r#"{{"description":"r23 child {index}","subagent_type":"general-purpose","background":{background},"prompt":"R23_SLEEP for {sleep_secs} seconds and report"}}"#
            ),
        );
    }
    finish_tool_message(&mut body, input_tokens);
    body
}

/// One tool call that occupies a child for a known, overlapping window.
///
/// `sleep` and not a busy loop on purpose: the question is whether three
/// children RUN at once, and a sleeping child answers it without adding CPU
/// that would then have to be subtracted from the parent's profile.
fn sleep_sse(seconds: u64, input_tokens: u32) -> String {
    let mut body = message_start("msg_r23_sleep", input_tokens);
    append_tool_use(
        &mut body,
        0,
        "toolu_r23_sleep",
        "bash",
        &json!({"command": format!("sleep {seconds} && printf 'r23 child awake'"), "timeout": 60000})
            .to_string(),
    );
    finish_tool_message(&mut body, input_tokens);
    body
}

/// One big tool result, stamped with the request's ordinal.
///
/// The stamp is not decoration. A full compaction can land mid-turn and leave
/// the preserved tail ending at the user's prompt with the `tool_use` that
/// answered it summarized away — so the runtime asks again, and a mock that
/// answers by content answers identically. zo then (correctly) stops the turn
/// with its tool-repetition guard: "`bash` has been called with identical
/// input 4 times without making progress". Stamping the command makes every
/// call distinct, so the measurement observes compaction instead of the guard.
fn big_output_sse(payload_lines: usize, nonce: usize, input_tokens: u32) -> String {
    let mut body = message_start("msg_r23_big", input_tokens);
    // `seq` output is distinct per line and long enough per line to move the
    // estimate, and `awk` keeps it one process instead of a shell loop whose
    // own start-up would dominate the tool's wall clock.
    let command = format!(
        "seq 1 {payload_lines} | awk '{{printf \"r23 payload {nonce} row %s carries a deterministic sentence of filler so the transcript grows\\n\", $1}}'"
    );
    append_tool_use(
        &mut body,
        0,
        "toolu_r23_big",
        "bash",
        &json!({"command": command, "timeout": 20000}).to_string(),
    );
    finish_tool_message(&mut body, input_tokens);
    body
}

fn message_start(id: &str, input_tokens: u32) -> String {
    let mut body = String::new();
    append_sse(
        &mut body,
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": "claude-sonnet-4-6",
                "stop_reason": null,
                "stop_sequence": null,
                "usage": usage_json(input_tokens, 0)
            }
        }),
    );
    body
}

fn finish_tool_message(body: &mut String, input_tokens: u32) {
    append_sse(
        body,
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": null},
            "usage": usage_json(input_tokens, 4)
        }),
    );
    append_sse(body, "message_stop", &json!({"type": "message_stop"}));
}

fn text_sse(message_id: &str, text: &str, input_tokens: u32) -> String {
    let mut body = message_start(message_id, input_tokens);
    append_sse(
        &mut body,
        "content_block_start",
        &json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
    );
    append_sse(
        &mut body,
        "content_block_delta",
        &json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": text}
        }),
    );
    append_sse(
        &mut body,
        "content_block_stop",
        &json!({"type": "content_block_stop", "index": 0}),
    );
    append_sse(
        &mut body,
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": null},
            "usage": usage_json(input_tokens, 8)
        }),
    );
    append_sse(&mut body, "message_stop", &json!({"type": "message_stop"}));
    body
}

fn append_tool_use(buffer: &mut String, index: usize, id: &str, name: &str, input: &str) {
    append_sse(
        buffer,
        "content_block_start",
        &json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}
        }),
    );
    append_sse(
        buffer,
        "content_block_delta",
        &json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "input_json_delta", "partial_json": input}
        }),
    );
    append_sse(
        buffer,
        "content_block_stop",
        &json!({"type": "content_block_stop", "index": index}),
    );
}

fn append_sse(buffer: &mut String, event: &str, payload: &Value) {
    use std::fmt::Write as _;
    writeln!(buffer, "event: {event}").expect("event write should succeed");
    writeln!(buffer, "data: {payload}").expect("payload write should succeed");
    buffer.push('\n');
}

fn usage_json(input_tokens: u32, output_tokens: u32) -> Value {
    json!({
        "input_tokens": input_tokens,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
        "output_tokens": output_tokens
    })
}

fn http_response(content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

async fn read_http_request(socket: &mut TcpStream) -> io::Result<(String, String)> {
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request closed before headers",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position;
        }
    };
    let header_text = String::from_utf8(buffer[..header_end].to_vec())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?;
    let method = request_line
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing method"))?
        .to_string();
    let mut content_length = 0_usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid content-length: {error}"),
                    )
                })?;
            }
        }
    }
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < content_length {
        let mut chunk = vec![0_u8; content_length - body.len()];
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request closed before body",
            ));
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    let body = String::from_utf8(body)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    Ok((method, body))
}

/// Length of the longest shared PREFIX of two serialized message arrays.
///
/// This is the only honest cache question a mock can answer. A mock invents
/// its own `cache_creation` / `cache_read` numbers, so reading them back would
/// be circular; what a provider actually matches is the serialized prefix, and
/// that is real in every request the driver captured.
#[must_use]
pub fn shared_prefix_len(left: &str, right: &str) -> usize {
    left.as_bytes()
        .iter()
        .zip(right.as_bytes())
        .take_while(|(left, right)| left == right)
        .count()
}

#[cfg(test)]
mod tests {
    use super::{answer_for, shared_prefix_len, Answer, Plan};

    #[test]
    fn only_the_last_message_decides_the_answer() {
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "R23_TOOLS go"}]},
                {"role": "user", "content": [{"type": "text", "text": "say something"}]}
            ]
        })
        .to_string();
        assert_eq!(answer_for(&body, Plan::default()), Answer::Text);
    }

    #[test]
    fn a_tool_result_ends_the_turn() {
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": "x"}]}
            ]
        })
        .to_string();
        assert_eq!(answer_for(&body, Plan::default()), Answer::Text);
    }

    #[test]
    fn the_compaction_prompt_is_recognized() {
        let body = serde_json::json!({
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "You are summarizing a coding conversation that is about to run out of context window."}]}
            ]
        })
        .to_string();
        assert_eq!(answer_for(&body, Plan::default()), Answer::Summary);
    }

    #[test]
    fn shared_prefix_counts_bytes_not_elements() {
        assert_eq!(shared_prefix_len("[1,2,3]", "[1,2,9]"), 5);
        assert_eq!(shared_prefix_len("[]", "[1]"), 1);
    }
}
