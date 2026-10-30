//! t-57512 — once a decline has stood, the same words are not asked again.
//!
//! 2026-10-15 19:27 → 19:28, a zo pane in a project folder: the window typed
//! the same 60-byte notice into the pane 18 times in 84 s. Each time zo ran a
//! turn, sent one request, met the same decline in a category the provider
//! routes nowhere, and recorded one more standing notice, so the session held
//! 18 of them. These pin that a turn whose words are the ones a decline stood
//! on, on the conversation it stood on, sends nothing and records nothing —
//! and that everything else is asked as before.

use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use super::fallback::{TurnLoop, REFUSAL_SURFACED_NOTICE};
use super::{
    ApiClient, ApiRequest, AssistantEvent, AsyncApiClient, ConversationRuntime, RuntimeError,
    StaticToolExecutor, StreamingTurnError, TurnSummary,
};
use crate::message_stream::types::{BlockId, RenderBlock};
use crate::permissions::{PermissionMode, PermissionPolicy};
use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};
use core_types::retry_signal::{
    REFUSAL_CONTEXT_CLEANED_WARN, REFUSAL_STANDING_NOTICE, REFUSAL_STANDING_REPEAT_NOTICE,
};

/// A category the catalog routes nowhere on Opus 5.5 (the premise below).
const UNROUTED: &str = "reasoning_extraction";

/// What the person — or a window — sends again and again.
const WORDS: &str = "list what the last turn left unfinished";

/// Words that are not those.
const OTHER_WORDS: &str = "only the file names, please";

/// Where every client here says it sends: one endpoint, so the same words go
/// where the request a decline stood on went.
const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// The premise of the tests below: on Opus 5.5 the provider routes
/// [`UNROUTED`] nowhere, and `cyber` somewhere.
#[test]
fn the_premise_one_category_is_routed_nowhere_and_one_somewhere() {
    assert!(::api::refusal_route_candidates("claude-opus-5-5", Some(UNROUTED)).is_empty());
    assert!(!::api::refusal_route_candidates("claude-opus-5-5", Some("cyber")).is_empty());
}

/// A provider whose classifier declines every request, naming `category`,
/// but those it was told to answer, and keeps every request it was sent.
struct Declining {
    category: &'static str,
    requests: Mutex<Vec<ApiRequest>>,
    /// The requests it answers: those whose last user words are the given
    /// ones — every request, for `None` — each with its answer.
    answers: Mutex<Vec<(Option<&'static str>, &'static str)>>,
}

impl Declining {
    fn new(category: &'static str) -> Arc<Self> {
        Arc::new(Self {
            category,
            requests: Mutex::new(Vec::new()),
            answers: Mutex::new(Vec::new()),
        })
    }

    /// From now on every request is answered.
    // These two are called only from the unix-only `hook_contract` module.
    #[cfg(unix)]
    fn answer_from_now_on(&self) {
        self.answer(None, "here is the list");
    }

    /// From now on a request whose last user words are `words` — every
    /// request, for `None` — is answered with `text`.
    #[cfg(unix)]
    fn answer(&self, words: Option<&'static str>, text: &'static str) {
        self.answers.lock().expect("answers").push((words, text));
    }

    fn reply(&self, request: ApiRequest) -> Vec<AssistantEvent> {
        let asked = request
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .and_then(first_words)
            .map(str::to_string);
        let answer = self
            .answers
            .lock()
            .expect("answers")
            .iter()
            .rev()
            .find(|(words, _)| words.is_none_or(|words| asked.as_deref() == Some(words)))
            .map(|(_, text)| *text);
        self.requests.lock().expect("requests").push(request);
        if let Some(text) = answer {
            return vec![AssistantEvent::TextDelta(text.to_string()), AssistantEvent::MessageStop];
        }
        vec![
            AssistantEvent::StopReason("refusal".to_string()),
            AssistantEvent::RefusalCategory(self.category.to_string()),
            AssistantEvent::MessageStop,
        ]
    }

    /// How many requests reached the provider.
    fn sent(&self) -> usize {
        self.requests.lock().expect("requests").len()
    }

    /// The last user message of request number `at`, as the provider read it.
    fn asked(&self, at: usize) -> Option<ConversationMessage> {
        let requests = self.requests.lock().expect("requests");
        requests
            .get(at)?
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .cloned()
    }
}

impl AsyncApiClient for Declining {
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        _render_tx: tokio::sync::mpsc::Sender<RenderBlock>,
        _text_block_id: BlockId,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>> {
        Box::pin(async move { Ok(self.reply(request)) })
    }

    fn destination(&self) -> Option<String> {
        Some(ENDPOINT.to_string())
    }
}

/// The same provider behind the headless loop's client.
struct DecliningSync(Arc<Declining>);

impl ApiClient for DecliningSync {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(self.0.reply(request))
    }

    fn destination(&self) -> Option<String> {
        Some(ENDPOINT.to_string())
    }
}

/// The sync client of a streaming runtime, which asks its async one.
struct NotAsked;

impl ApiClient for NotAsked {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        panic!("a streaming turn asks the async client");
    }
}

/// Answers no to every question.
struct No;

impl crate::permission::PermissionPrompter for No {
    fn decide<'a>(
        &'a self,
        _request: crate::permission::PermissionRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        crate::permission::PermissionDecision,
                        crate::permission::PermissionError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Ok(crate::permission::PermissionDecision::Deny) })
    }
}

/// The process environment held for a test's length — the todo store pinned
/// to a path that does not exist, as the other refusal tests pin it — and the
/// folder the session file and the turn records go to, removed afterwards.
struct Hermetic {
    _lock: std::sync::MutexGuard<'static, ()>,
    prior_todo_store: Option<std::ffi::OsString>,
    cwd: PathBuf,
}

impl Hermetic {
    fn new(label: &str) -> Self {
        let lock = crate::test_env_lock();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let cwd = std::env::temp_dir().join(format!(
            "zo-refusal-repeat-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&cwd).expect("cwd");
        let prior_todo_store = std::env::var_os("ZO_TODO_STORE");
        std::env::set_var("ZO_TODO_STORE", cwd.join("absent-todos.json"));
        Self {
            _lock: lock,
            prior_todo_store,
            cwd,
        }
    }

    /// The session file a runtime of this test writes.
    fn session_file(&self) -> PathBuf {
        self.cwd.join("session.jsonl")
    }

    /// A session that writes [`Self::session_file`].
    fn session(&self) -> Session {
        Session::new().with_persistence_path(self.session_file())
    }
}

impl Drop for Hermetic {
    fn drop(&mut self) {
        match self.prior_todo_store.take() {
            Some(value) => std::env::set_var("ZO_TODO_STORE", value),
            None => std::env::remove_var("ZO_TODO_STORE"),
        }
        let _ = std::fs::remove_dir_all(&self.cwd);
    }
}

/// Opus 5.5 on `session`, every request going to `provider`.
fn streaming_runtime(
    provider: &Arc<Declining>,
    cwd: &Path,
    session: Session,
) -> ConversationRuntime<NotAsked, StaticToolExecutor> {
    let mut runtime = ConversationRuntime::new(
        session,
        NotAsked,
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    )
    .with_async_api_client(Arc::clone(provider) as Arc<dyn AsyncApiClient>);
    runtime.set_context_model("claude-opus-5-5");
    runtime.set_workspace_cwd(cwd.to_path_buf());
    runtime
}

/// A streaming runtime whose decline has stood for [`WORDS`]: turn one
/// surfaced it after the ladder, turn two asked once and it stood.
fn stood_runtime(env: &Hermetic, provider: &Arc<Declining>) -> ConversationRuntime<NotAsked, StaticToolExecutor> {
    let mut runtime = streaming_runtime(provider, &env.cwd, env.session());
    for _ in 0..2 {
        let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
        ended.expect("the decline is surfaced, then stands");
    }
    assert_eq!(standing_notices(&runtime.session.messages), 1, "the decline stood once");
    runtime
}

/// One streaming turn of `words` and `images` through the dispatcher hosts
/// drive turns with: how it ended, and the lines the screen was told.
fn turn<C: ApiClient>(
    runtime: &mut ConversationRuntime<C, StaticToolExecutor>,
    words: &str,
    images: Vec<(String, String)>,
) -> (Result<TurnSummary, StreamingTurnError>, Vec<String>) {
    turn_through(runtime, Entry::Dispatcher, words, images)
}

/// The streaming entries a host can call.
#[derive(Debug, Clone, Copy)]
enum Entry {
    /// The dispatcher, with its stop loop and deep mode.
    Dispatcher,
    /// The ordinary turn without the dispatcher; it takes no pictures.
    Streaming,
    /// The same, with pictures.
    StreamingWithImages,
}

/// One streaming turn of `words` and `images` through `entry`: how it ended,
/// and the lines the screen was told — read to the end, which comes only when
/// the turn has let go of the screen.
fn turn_through<C: ApiClient>(
    runtime: &mut ConversationRuntime<C, StaticToolExecutor>,
    entry: Entry,
    words: &str,
    images: Vec<(String, String)>,
) -> (Result<TurnSummary, StreamingTurnError>, Vec<String>) {
    let prompter: Arc<dyn crate::permission::PermissionPrompter> = Arc::new(No);
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    executor.block_on(async {
        let (render_tx, mut render_rx) = tokio::sync::mpsc::channel(256);
        let screen = tokio::spawn(async move {
            let mut lines = Vec::new();
            while let Some(block) = render_rx.recv().await {
                if let RenderBlock::System { text, .. } = block {
                    lines.push(text);
                }
            }
            lines
        });
        let ended = match entry {
            Entry::Dispatcher => {
                runtime
                    .run_turn_streaming_maybe_deep(words, images, render_tx, prompter)
                    .await
            }
            Entry::Streaming => runtime.run_turn_streaming(words, render_tx, prompter).await,
            Entry::StreamingWithImages => {
                runtime
                    .run_turn_streaming_with_images(words, images, render_tx, prompter)
                    .await
            }
        };
        (ended, screen.await.expect("the screen task"))
    })
}

/// A road the same words can take after a decline stood: it moves the runtime
/// onto the road, runs the words there, and answers the round-trips made.
type Road = fn(&mut ConversationRuntime<NotAsked, StaticToolExecutor>) -> usize;

/// One internal leg of `words` — a deep-gate subturn's, inside a turn.
fn internal_leg<C: ApiClient>(
    runtime: &mut ConversationRuntime<C, StaticToolExecutor>,
    words: &str,
) -> Result<TurnSummary, StreamingTurnError> {
    let prompter: Arc<dyn crate::permission::PermissionPrompter> = Arc::new(No);
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    executor.block_on(async {
        let (render_tx, mut render_rx) = tokio::sync::mpsc::channel(256);
        let screen = tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
        let ended = runtime
            .run_internal_subturn_streaming_with_images(words, Vec::new(), render_tx, prompter)
            .await;
        screen.await.expect("the screen task");
        ended
    })
}

/// How many messages of `messages` are the notice of a decline that stood.
fn standing_notices(messages: &[ConversationMessage]) -> usize {
    messages
        .iter()
        .filter(|message| {
            message.role == MessageRole::Assistant
                && message.blocks.iter().any(
                    |block| matches!(block, ContentBlock::Text { text } if text == REFUSAL_STANDING_NOTICE),
                )
        })
        .count()
}

/// The words of a message, when words are all it holds.
fn words_of(message: &ConversationMessage) -> Option<&str> {
    match message.blocks.as_slice() {
        [ContentBlock::Text { text }] => Some(text.as_str()),
        _ => None,
    }
}

/// The first words of a message, as the tests of t-15890 read a request's.
fn first_words(message: &ConversationMessage) -> Option<&str> {
    message.blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    })
}

/// A turn's answer, when it gave exactly one, in words alone.
fn answer_of(summary: &TurnSummary) -> Option<&str> {
    match summary.assistant_messages.as_slice() {
        [answer] => words_of(answer),
        _ => None,
    }
}

/// How many turn records were written under `cwd`. Only the `.jsonl` logs: the
/// folder also holds the resumed reader's `.read-cursor.json`.
fn turn_records(cwd: &Path) -> usize {
    let dir = crate::traces_base(cwd)
        .join(core_types::paths::ZO_DIR_NAME)
        .join("turns");
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "jsonl"))
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default().lines().count())
        .sum()
}

/// Everything a turn can leave behind: what the provider was sent, the
/// conversation, its standing notices, the turn records and the session file.
#[derive(Debug, PartialEq, Eq)]
struct Trace {
    requests: usize,
    messages: usize,
    standing_notices: usize,
    turn_records: usize,
    session_file_bytes: usize,
    session_file_hash: u64,
}

impl Trace {
    fn of(provider: &Declining, messages: &[ConversationMessage], env: &Hermetic) -> Self {
        let session_file = std::fs::read(env.session_file()).unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        session_file.hash(&mut hasher);
        Self {
            requests: provider.sent(),
            messages: messages.len(),
            standing_notices: standing_notices(messages),
            turn_records: turn_records(&env.cwd),
            session_file_bytes: session_file.len(),
            session_file_hash: hasher.finish(),
        }
    }
}

/// The case seen. Turn one is surfaced after the ladder; turn two, the same
/// words, asks once and stands. Turns three and four, the same words again,
/// send nothing, leave nothing behind, and tell the screen why in one line.
#[test]
fn the_same_words_after_a_decline_that_stood_send_nothing_and_record_nothing() {
    let env = Hermetic::new("streaming");
    let provider = Declining::new(UNROUTED);
    let mut runtime = streaming_runtime(&provider, &env.cwd, env.session());

    let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("turn one surfaces the decline");
    let after_one = provider.sent();
    let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("turn two asks once and stands");
    let stood = Trace::of(&provider, &runtime.session.messages, &env);
    let mut repeats = Vec::new();
    for _ in 0..2 {
        let (ended, lines) = turn(&mut runtime, WORDS, Vec::new());
        repeats.push((ended.expect("the repeat ends cleanly"), lines));
    }

    assert_eq!(
        provider.sent() - after_one,
        1,
        "three more turns of the same words send one request: the one that found the decline standing"
    );
    assert_eq!(
        Trace::of(&provider, &runtime.session.messages, &env),
        stood,
        "the two repeats sent no request and added nothing to the conversation, its notices, the turn records or the session file"
    );
    assert_eq!(stood.standing_notices, 1, "the conversation holds one standing notice, not three");
    for (summary, lines) in &repeats {
        assert_eq!(
            lines,
            &[REFUSAL_STANDING_REPEAT_NOTICE.to_string()],
            "each repeat tells the screen one line"
        );
        assert_eq!(summary.iterations, 0, "and makes no round-trip");
        assert_eq!(answer_of(summary), Some(REFUSAL_STANDING_REPEAT_NOTICE), "and answers with that line");
    }
}

/// The headless loop keeps the same rule: the same words after a decline that
/// stood send nothing, leave nothing behind, and answer with the one line.
#[test]
fn the_headless_loop_sends_nothing_for_the_same_words_either() {
    let env = Hermetic::new("sync");
    let provider = Declining::new(UNROUTED);
    let mut runtime = ConversationRuntime::new(
        env.session(),
        DecliningSync(Arc::clone(&provider)),
        StaticToolExecutor::new(),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_context_model("claude-opus-5-5");
    runtime.set_workspace_cwd(env.cwd.clone());

    runtime.run_turn(WORDS, None).expect("turn one surfaces the decline");
    let after_one = provider.sent();
    runtime.run_turn(WORDS, None).expect("turn two asks once and stands");
    let stood = Trace::of(&provider, &runtime.session.messages, &env);
    let mut repeats = Vec::new();
    for _ in 0..2 {
        repeats.push(runtime.run_turn(WORDS, None).expect("the repeat ends cleanly"));
    }

    assert_eq!(
        provider.sent() - after_one,
        1,
        "three more turns of the same words send one request: the one that found the decline standing"
    );
    assert_eq!(
        Trace::of(&provider, &runtime.session.messages, &env),
        stood,
        "the two repeats sent no request and added nothing to the conversation, its notices, the turn records or the session file"
    );
    assert_eq!(stood.standing_notices, 1, "the conversation holds one standing notice, not three");
    for summary in &repeats {
        assert_eq!(summary.iterations, 0, "a repeat makes no round-trip");
        assert_eq!(
            answer_of(summary),
            Some(REFUSAL_STANDING_REPEAT_NOTICE),
            "and its headless result is the one line"
        );
    }
    let last = runtime.session.messages.last().expect("the standing notice");
    assert_eq!(words_of(last), Some(REFUSAL_STANDING_NOTICE), "the conversation still ends on it");
}

/// The streaming entries a host can call without the dispatcher keep the
/// rule too: the same words after a decline that stood send nothing through
/// either, leave nothing behind, and tell the screen why in one line.
#[test]
fn the_public_streaming_entries_send_nothing_for_the_same_words_either() {
    let env = Hermetic::new("public-entries");
    let provider = Declining::new(UNROUTED);
    let mut runtime = stood_runtime(&env, &provider);
    let stood = Trace::of(&provider, &runtime.session.messages, &env);

    for entry in [Entry::Streaming, Entry::StreamingWithImages] {
        let (ended, lines) = turn_through(&mut runtime, entry, WORDS, Vec::new());
        ended.expect("the repeat ends cleanly");
        assert_eq!(lines, [REFUSAL_STANDING_REPEAT_NOTICE.to_string()], "{entry:?}: one line");
    }
    assert_eq!(
        Trace::of(&provider, &runtime.session.messages, &env),
        stood,
        "neither entry sent a request or added anything to the conversation, the turn records or the session file"
    );
}

/// A turn that sends nothing writes nothing: the session file is, byte for
/// byte, what the turn that stood left.
#[test]
fn a_turn_that_repeats_a_standing_decline_adds_nothing_to_the_session_file() {
    let env = Hermetic::new("session-file");
    let provider = Declining::new(UNROUTED);
    let mut runtime = stood_runtime(&env, &provider);
    let stood = std::fs::read(env.session_file()).expect("the session file");

    let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("the repeat ends cleanly");
    let after = std::fs::read(env.session_file()).expect("the session file");

    assert!(
        after == stood,
        "the repeat changed the session file: {} bytes → {} bytes",
        stood.len(),
        after.len()
    );
}

/// What the window reads of each message line in the session file at `path`:
/// the line's `decline` name and the first words of its message.
fn message_lines(path: &Path) -> Vec<(Option<String>, Option<String>)> {
    std::fs::read_to_string(path)
        .expect("the session file")
        .lines()
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .filter(|record| record["type"] == "message")
        .map(|record| {
            (
                record["decline"].as_str().map(str::to_string),
                record["message"]["blocks"][0]["text"].as_str().map(str::to_string),
            )
        })
        .collect()
}

/// The `decline` name a message line is expected to carry, by its words: the
/// decline a notice was recorded as, and `None` for words no notice has.
fn decline_of_the_notice(words: Option<&str>) -> Option<String> {
    match words {
        Some(REFUSAL_SURFACED_NOTICE) => Some("surfaced".to_string()),
        Some(REFUSAL_STANDING_NOTICE) => Some("standing".to_string()),
        _ => None,
    }
}

/// The window and a later zo know a recorded decline by the name its line
/// carries (t-57445), not by its words: the line of the notice a decline was
/// surfaced with names `surfaced`, the line of the one it stood with names
/// `standing`, and no other line names a decline.
#[test]
fn a_recorded_decline_names_its_kind_on_its_line() {
    let env = Hermetic::new("decline-names");
    let provider = Declining::new(UNROUTED);
    let _runtime = stood_runtime(&env, &provider);

    let lines = message_lines(&env.session_file());
    let expected: Vec<_> = lines
        .iter()
        .map(|(_, words)| decline_of_the_notice(words.as_deref()))
        .collect();
    assert!(
        expected.iter().any(|decline| decline.as_deref() == Some("standing")),
        "the file holds the notice the decline stood with: {lines:?}"
    );
    let written: Vec<_> = lines.into_iter().map(|(decline, _)| decline).collect();
    assert_eq!(
        written, expected,
        "each notice's line names the decline it was recorded as, and no other line names one"
    );
}

/// The refusal ladder finds the declined exchange it drops before its
/// context-cleaning retry by the name the answer's record carries (t-57445):
/// a decline told in other words is still one. In history from before the
/// name, a notice's own words still mark one; other words under no name are an
/// answer like any other.
#[test]
fn a_decline_is_known_by_the_name_its_record_carries_before_its_words() {
    for (decline, words, known) in [
        (Some("standing"), "Reworded: still declined.", true),
        (Some("surfaced"), "Reworded: declined.", true),
        (None, REFUSAL_STANDING_NOTICE, true),
        (None, "Reworded: still declined.", false),
    ] {
        let env = Hermetic::new("decline-known");
        let mut answer = serde_json::json!({
            "type": "message",
            "turn_index": 0,
            "message": {"role": "assistant", "blocks": [{"type": "text", "text": words}]},
        });
        if let Some(decline) = decline {
            answer["decline"] = serde_json::Value::from(decline);
        }
        let records = [
            serde_json::json!({
                "type": "session_meta",
                "version": 1,
                "session_id": "session-decline-known",
                "created_at_ms": 1_790_000_000_000_u64,
                "updated_at_ms": 1_790_000_001_000_u64,
            }),
            serde_json::json!({
                "type": "message",
                "turn_index": 0,
                "message": {"role": "user", "blocks": [{"type": "text", "text": WORDS}]},
            }),
            answer,
        ];
        let contents = records.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        std::fs::write(env.session_file(), format!("{contents}\n")).expect("write the session file");
        let session = Session::load_from_path(env.session_file()).expect("load the session file");
        let provider = Declining::new(UNROUTED);
        let mut runtime = streaming_runtime(&provider, &env.cwd, session);

        assert_eq!(
            runtime.drop_last_declined_exchange(),
            known,
            "an answer told in {words:?} whose record names {decline:?}"
        );
        assert_eq!(
            runtime.session.messages.len(),
            if known { 0 } else { 2 },
            "a declined exchange is dropped whole, and any other answer stays"
        );
    }
}

/// A repeat ends its turn as any turn ends: the screen is let go, nothing a
/// turn start opens is left open, nothing waits for a request that will not
/// come — and the next turn starts and ends as any other.
#[test]
fn a_repeat_ends_its_turn_cleanly_and_leaves_nothing_pending() {
    let env = Hermetic::new("ends-cleanly");
    let provider = Declining::new(UNROUTED);
    let mut runtime = stood_runtime(&env, &provider);

    let (ended, lines) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("the repeat ends Ok");
    assert_eq!(lines, [REFUSAL_STANDING_REPEAT_NOTICE.to_string()], "one line, then the screen is let go");
    assert!(runtime.team_inbox_turn.is_none(), "no inbox delivery left open");
    assert!(runtime.file_pick_pending_turn.is_none(), "no file-pick turn left open");
    assert!(runtime.skill_suggestion_turn_start.is_none(), "no skill-suggestion turn left open");
    assert!(
        !runtime.quota_prearm_notice_pending && !runtime.refusal_prearm_notice_pending,
        "no notice left waiting for a request"
    );

    let before = provider.sent();
    let (ended, _) = turn(&mut runtime, OTHER_WORDS, Vec::new());
    ended.expect("the next turn ends cleanly");
    assert_eq!(
        provider.asked(before).as_ref().and_then(first_words),
        Some(OTHER_WORDS),
        "the next turn asks, with its words"
    );
}

/// Other words are another request: after a decline that stood, and after a
/// repeat of it, they are asked as the person wrote them, and the classifier
/// judges them, as before. The first of them walks the ladder's compaction, as
/// any decline does once the conversation is long enough to fold (t-16786);
/// after them, the words the decline stood on are asked once.
#[test]
fn other_words_after_a_decline_that_stood_are_asked() {
    let env = Hermetic::new("other-words");
    let provider = Declining::new(UNROUTED);
    let mut runtime = stood_runtime(&env, &provider);
    let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("the repeat ends cleanly");

    let before = provider.sent();
    let (ended, _) = turn(&mut runtime, OTHER_WORDS, Vec::new());
    ended.expect("the decline stands on other words too");
    assert_eq!(
        provider.asked(before).as_ref().and_then(first_words),
        Some(OTHER_WORDS),
        "other words are asked as the person wrote them"
    );

    let before = provider.sent();
    let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("the decline stands");
    assert_eq!(provider.sent() - before, 1, "after them, the words the decline stood on are asked once");
    assert_eq!(
        provider.asked(before).as_ref().and_then(first_words),
        Some(WORDS),
        "as the person wrote them"
    );
}

/// The same words with a picture are another request: they are asked, the
/// picture with them, as before.
#[test]
fn the_same_words_with_a_picture_are_asked() {
    let env = Hermetic::new("picture");
    let provider = Declining::new(UNROUTED);
    let mut runtime = stood_runtime(&env, &provider);

    let before = provider.sent();
    let picture = vec![("image/png".to_string(), "c2NyZWVuc2hvdA==".to_string())];
    let (ended, _) = turn(&mut runtime, WORDS, picture);
    ended.expect("the decline stands");

    let asked = provider.asked(before).expect("the request");
    assert_eq!(first_words(&asked), Some(WORDS), "asked with the words");
    assert!(
        asked
            .blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::Image { .. })),
        "with the picture: {asked:?}"
    );
}

/// The same words on another road are another attempt, and the decline that
/// stood does not stop it: an escalation the host installed for the turn, an
/// internal leg inside a turn, another model (`/model`) and a fresh session
/// (`/new`) are each asked, as before.
#[test]
fn a_decline_that_stood_does_not_stop_the_same_words_on_another_road() {
    let roads: [(&str, Road); 4] = [
        ("an escalation installed for the turn", |runtime| {
            runtime.set_escalation_model_override(Some("claude-opus-5".to_string()));
            let (ended, _) = turn(runtime, WORDS, Vec::new());
            ended.expect("the escalated turn ends cleanly").iterations
        }),
        ("an internal leg", |runtime| {
            internal_leg(runtime, WORDS).expect("the leg ends cleanly").iterations
        }),
        ("another model", |runtime| {
            runtime.set_context_model("claude-opus-5");
            let (ended, _) = turn(runtime, WORDS, Vec::new());
            ended.expect("the turn ends cleanly").iterations
        }),
        ("a fresh session", |runtime| {
            runtime.replace_session(Session::new());
            let (ended, _) = turn(runtime, WORDS, Vec::new());
            ended.expect("the turn ends cleanly").iterations
        }),
    ];
    for (road, take) in roads {
        let env = Hermetic::new("another-road");
        let provider = Declining::new(UNROUTED);
        let mut runtime = stood_runtime(&env, &provider);
        assert!(
            runtime.repeats_a_standing_decline(WORDS, TurnLoop::Streaming),
            "{road}: on its own road the words would repeat"
        );

        let before = provider.sent();
        let iterations = take(&mut runtime);
        assert!(provider.sent() > before, "{road}: the same words are asked");
        assert!(iterations > 0, "{road}: and the turn made its round-trips");
        assert_eq!(
            provider.asked(before).as_ref().and_then(first_words),
            Some(WORDS),
            "{road}: as the person wrote them"
        );
    }
}

/// A category the provider routes somewhere is never kept as a decline that
/// stands (t-15890), so the rule never finds its repeats: each one walks the
/// ladder as before — the cleaned retry included. The requests and cleaned
/// retries of each turn are printed, so a run before the change and a run
/// after it can be compared line for line.
#[test]
fn a_routed_decline_repeated_walks_the_ladder_every_turn() {
    let env = Hermetic::new("routed");
    let provider = Declining::new("cyber");
    let mut runtime = streaming_runtime(&provider, &env.cwd, env.session());
    runtime.set_classifier_fallback(crate::ClassifierFallback::Auto);

    let mut asked = Vec::new();
    let mut cleaned = Vec::new();
    for _ in 0..4 {
        let before = provider.sent();
        let (ended, lines) = turn(&mut runtime, WORDS, Vec::new());
        ended.expect("the turn surfaces the decline");
        asked.push(provider.sent() - before);
        cleaned.push(lines.iter().any(|line| line == REFUSAL_CONTEXT_CLEANED_WARN));
        assert!(runtime.surfaced_decline.is_none(), "a routed decline is not kept");
        assert!(
            !runtime.repeats_a_standing_decline(WORDS, TurnLoop::Streaming),
            "so its repeat is never found"
        );
    }
    eprintln!("t-57512 routed ladder | requests a turn {asked:?} | cleaned retry a turn {cleaned:?}");

    assert!(asked.iter().all(|sent| *sent > 0), "every turn asks: {asked:?}");
    assert!(cleaned.iter().any(|retried| *retried), "the cleaned retry is walked: {cleaned:?}");
    assert_eq!(standing_notices(&runtime.session.messages), 0);
}

/// The rule itself: each of its conditions is needed.
#[test]
fn a_repeat_needs_the_decline_kept_standing_and_the_same_words() {
    let env = Hermetic::new("rule");
    let provider = Declining::new(UNROUTED);
    let mut runtime = streaming_runtime(&provider, &env.cwd, env.session());

    let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("turn one surfaces the decline");
    assert!(
        !runtime.repeats_a_standing_decline(WORDS, TurnLoop::Streaming),
        "a decline surfaced after the ladder has not stood yet: the next turn asks once"
    );
    let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("turn two stands");
    assert!(runtime.repeats_a_standing_decline(WORDS, TurnLoop::Streaming));
    assert!(!runtime.repeats_a_standing_decline(OTHER_WORDS, TurnLoop::Streaming), "other words");
    assert!(
        !runtime.repeats_a_standing_decline(&format!("{WORDS} "), TurnLoop::Streaming),
        "other bytes are other words"
    );

    let mut ended_otherwise = runtime.surfaced_decline.clone().expect("the kept decline");
    ended_otherwise.renewed = false;
    let kept = runtime.surfaced_decline.replace(ended_otherwise);
    assert!(
        !runtime.repeats_a_standing_decline(WORDS, TurnLoop::Streaming),
        "the latest public turn did not surface it"
    );
    runtime.surfaced_decline = kept;

    let messages = Arc::clone(&runtime.session.messages);
    let mut drifted: Vec<ConversationMessage> = (0..super::fallback::STANDING_REFUSAL.message_drift)
        .map(|n| ConversationMessage::user_text(format!("earlier {n}")))
        .collect();
    drifted.extend(messages.iter().cloned());
    runtime.session.messages = Arc::new(drifted);
    assert!(
        !runtime.repeats_a_standing_decline(WORDS, TurnLoop::Streaming),
        "the conversation changed by the drift, though it still ends on the same words and notice"
    );
    runtime.session.messages = messages;
    assert!(runtime.repeats_a_standing_decline(WORDS, TurnLoop::Streaming));

    runtime.quota_dry_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
    assert!(
        !runtime.repeats_a_standing_decline(WORDS, TurnLoop::Streaming),
        "a quota cooldown could carry the turn to another provider"
    );
    runtime.quota_dry_until = None;

    runtime.set_context_model("claude-opus-5");
    assert!(!runtime.repeats_a_standing_decline(WORDS, TurnLoop::Streaming), "another model");
}

/// The lifecycle hook contract, on every loop a turn runs on. A turn, or a
/// stop gate's follow-up, that repeats the words a decline just stood on runs
/// no hook: no `UserPromptSubmit`, and no `TurnEnd`, so no stop gate asks a
/// follow-up after it and the turn ends with it, settling what the legs before
/// it took. It takes nothing from the team inbox. Everything else runs them as
/// before. The hooks are shell commands, so these run where a POSIX shell
/// does.
#[cfg(unix)]
mod hook_contract {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use rusqlite::{params, Connection};

    use super::super::DeepGateConfig;
    use super::{
        answer_of, first_words, internal_leg, standing_notices, turn, AsyncApiClient, ContentBlock,
        ConversationMessage, ConversationRuntime, Declining, DecliningSync, Hermetic, MessageRole,
        PermissionMode, PermissionPolicy, Session, StaticToolExecutor, Trace, TurnLoop, TurnSummary,
        OTHER_WORDS, REFUSAL_STANDING_NOTICE, REFUSAL_STANDING_REPEAT_NOTICE, UNROUTED, WORDS,
    };
    use crate::compact::{compact_session, CompactionConfig, COMPACTION_SYSTEM_PROMPT};
    use crate::config::{ConfigLoader, RuntimeFeatureConfig};

    /// What a stop gate's `TurnEnd` hook asks for.
    const FOLLOW_UP: &str = "keep going until the list is complete";

    /// A follow-up that is not that.
    const OTHER_FOLLOW_UP: &str = "now say which of them is the oldest";

    /// The loops a turn runs on: the headless one, the streaming dispatcher,
    /// and the dispatcher in deep mode, whose legs run as internal turns.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Road {
        Headless,
        Streaming,
        Deep,
    }

    const ROADS: [Road; 3] = [Road::Headless, Road::Streaming, Road::Deep];

    /// The runtime every road drives.
    type Runtime = ConversationRuntime<DecliningSync, StaticToolExecutor>;

    impl Road {
        /// The loop this road's turns run on.
        fn turn_loop(self) -> TurnLoop {
            match self {
                Self::Headless => TurnLoop::Sync,
                Self::Streaming | Self::Deep => TurnLoop::Streaming,
            }
        }

        /// Opus 5.5 with `hooks`, every request going to `provider`.
        fn runtime(self, env: &Hermetic, provider: &Arc<Declining>, hooks: &CountingHooks) -> Runtime {
            let mut runtime = ConversationRuntime::new_with_features(
                env.session(),
                DecliningSync(Arc::clone(provider)),
                StaticToolExecutor::new(),
                PermissionPolicy::new(PermissionMode::DangerFullAccess),
                vec!["system".to_string()],
                &hooks.features(),
            );
            if self != Self::Headless {
                runtime = runtime
                    .with_async_api_client(Arc::clone(provider) as Arc<dyn AsyncApiClient>);
            }
            if self == Self::Deep {
                runtime.set_deep_gate(Some(DeepGateConfig::default()));
            }
            runtime.set_context_model("claude-opus-5-5");
            runtime.set_workspace_cwd(env.cwd.clone());
            runtime
        }

        /// One turn of `words` on this road.
        fn run(self, runtime: &mut Runtime, words: &str) -> Result<TurnSummary, String> {
            match self {
                Self::Headless => runtime.run_turn(words, None).map_err(|error| format!("{error:?}")),
                Self::Streaming | Self::Deep => {
                    turn(runtime, words, Vec::new()).0.map_err(|error| format!("{error:?}"))
                }
            }
        }

        /// Stands the decline on [`WORDS`] beside an empty team inbox: turn
        /// one surfaces it, turn two asks once and it stands.
        fn stand(self, env: &Hermetic, runtime: &mut Runtime) {
            open_team_inbox(&env.cwd, &runtime.session.session_id);
            for _ in 0..2 {
                self.run(runtime, WORDS).expect("the decline is surfaced, then stands");
            }
            assert_eq!(
                standing_notices(&runtime.session.messages),
                1,
                "{self:?}: the decline stood once"
            );
            assert!(
                runtime.repeats_a_standing_decline(WORDS, self.turn_loop()),
                "{self:?}: the same words again would only repeat it"
            );
        }
    }

    /// How many times each counted hook has run.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct HookRuns {
        prompt_submit: usize,
        turn_end: usize,
    }

    /// Hooks that keep count: `UserPromptSubmit` and `TurnEnd` each add a
    /// line to their own file, and the `TurnEnd` one, as a stop gate does,
    /// answers with the next follow-up it was given, if any. They sit in the
    /// person's own settings, the scope zo takes hooks from.
    struct CountingHooks {
        dir: PathBuf,
    }

    impl CountingHooks {
        fn new(env: &Hermetic) -> Self {
            let dir = env.cwd.join("hooks");
            std::fs::create_dir_all(&dir).expect("hooks folder");
            Self { dir }
        }

        fn path(&self, name: &str) -> String {
            self.dir.join(name).display().to_string()
        }

        /// A runtime's features with these hooks, loaded as zo loads settings,
        /// and a team inbox that gives a leg one update at most, so what each
        /// leg took can be told apart.
        fn features(&self) -> RuntimeFeatureConfig {
            let prompt_submit = format!("echo ran >> '{}'", self.path("prompt-submit"));
            let turn_end = format!(
                "echo ran >> '{runs}'; if [ -s '{queue}' ]; then \
                 next=$(head -n 1 '{queue}'); tail -n +2 '{queue}' > '{queue}.rest'; \
                 mv '{queue}.rest' '{queue}'; \
                 printf '{{\"hookSpecificOutput\":{{\"followupMessage\":\"%s\"}}}}' \"$next\"; fi",
                runs = self.path("turn-end"),
                queue = self.path("follow-ups"),
            );
            let project = self.dir.join("project");
            let home = self.dir.join("home").join(".zo");
            std::fs::create_dir_all(project.join(".zo")).expect("project settings folder");
            std::fs::create_dir_all(&home).expect("settings folder");
            let settings = serde_json::json!({
                "hooks": { "UserPromptSubmit": [prompt_submit], "TurnEnd": [turn_end] },
                "teamInboxDigestMaxUpdates": 1
            });
            std::fs::write(home.join("settings.json"), settings.to_string()).expect("settings");
            ConfigLoader::new(&project, &home)
                .load()
                .expect("the settings load")
                .feature_config()
                .clone()
        }

        /// From now on the `TurnEnd` hook answers with `follow_ups`, one per
        /// run and in order, then with nothing.
        fn answer_with(&self, follow_ups: &[&str]) {
            let queue: String = follow_ups.iter().flat_map(|words| [*words, "\n"]).collect();
            std::fs::write(self.dir.join("follow-ups"), queue).expect("the stop gate's follow-ups");
        }

        fn runs(&self) -> HookRuns {
            let count = |name: &str| {
                std::fs::read_to_string(self.dir.join(name)).map_or(0, |ran| ran.lines().count())
            };
            HookRuns {
                prompt_submit: count("prompt-submit"),
                turn_end: count("turn-end"),
            }
        }
    }

    fn team_inbox_file(cwd: &Path) -> PathBuf {
        cwd.join(".zo").join("team_inbox").join("team_inbox.sqlite3")
    }

    /// A team inbox for the session `session_id` in `cwd`, as the window keeps
    /// one: its tables, and a reader that has read nothing yet.
    fn open_team_inbox(cwd: &Path, session_id: &str) {
        let file = team_inbox_file(cwd);
        std::fs::create_dir_all(file.parent().expect("its folder")).expect("team inbox folder");
        let inbox = Connection::open(&file).expect("the team inbox");
        inbox
            .execute_batch(
                "CREATE TABLE updates (
                    seq INTEGER PRIMARY KEY AUTOINCREMENT,
                    id TEXT NOT NULL UNIQUE,
                    channel TEXT NOT NULL,
                    source TEXT NOT NULL,
                    created_at_unix INTEGER NOT NULL,
                    priority TEXT NOT NULL,
                    summary TEXT NOT NULL,
                    body_ref_json TEXT,
                    task_id TEXT,
                    status TEXT
                );
                CREATE TABLE cursors (
                    consumer_id TEXT NOT NULL,
                    channel TEXT NOT NULL,
                    last_seen_seq INTEGER NOT NULL,
                    PRIMARY KEY (consumer_id, channel)
                );
                CREATE TABLE deliveries (
                    update_id TEXT NOT NULL,
                    consumer_id TEXT NOT NULL,
                    state TEXT NOT NULL,
                    turn_id TEXT,
                    retry_count INTEGER NOT NULL DEFAULT 0,
                    updated_at_unix INTEGER NOT NULL,
                    PRIMARY KEY (update_id, consumer_id)
                );
                CREATE TABLE jsonl_outbox (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    event_json TEXT NOT NULL
                );",
            )
            .expect("the team inbox tables");
        inbox
            .execute(
                "INSERT INTO cursors (consumer_id, channel, last_seen_seq) VALUES (?1, 'ci', 0)",
                params![format!("session:{session_id}")],
            )
            .expect("its reader");
    }

    /// One update posted to the team inbox in `cwd`.
    fn post_to_team_inbox(cwd: &Path, id: &str) {
        Connection::open(team_inbox_file(cwd))
            .expect("the team inbox")
            .execute(
                "INSERT INTO updates (id, channel, source, created_at_unix, priority, summary)
                 VALUES (?1, 'ci', 'agent:reviewer', 1, 'high', 'the build is green again')",
                params![id],
            )
            .expect("the update");
    }

    /// The rows of the team inbox in `cwd` that its reader changes by taking
    /// an update: each delivery and its state, how far each cursor has read,
    /// and how many events wait in the outbox.
    fn team_inbox_taken(cwd: &Path) -> Vec<String> {
        let inbox = Connection::open(team_inbox_file(cwd)).expect("the team inbox");
        let mut taken = inbox
            .prepare(
                "SELECT 'delivery ' || update_id || ' ' || state FROM deliveries
                 UNION ALL SELECT 'cursor ' || channel || ' ' || last_seen_seq FROM cursors
                 UNION ALL SELECT 'outbox ' || count(*) FROM jsonl_outbox
                 ORDER BY 1",
            )
            .expect("read the team inbox");
        let rows = taken
            .query_map(params![], |row| row.get::<_, String>(0))
            .expect("its rows");
        rows.map(|row| row.expect("a row")).collect()
    }

    /// The deliveries of the team inbox in `cwd`, each with its state.
    fn team_inbox_deliveries(cwd: &Path) -> Vec<String> {
        team_inbox_taken(cwd)
            .into_iter()
            .filter(|row| row.starts_with("delivery "))
            .collect()
    }

    /// Whether a turn left its cleanup something to settle: a team inbox batch
    /// it took and did not settle, or a seat's turn it did not finish.
    fn cleanup_left_open(runtime: &Runtime) -> bool {
        runtime.team_inbox_turn.is_some()
            || runtime.file_pick_pending_turn.is_some()
            || runtime.skill_suggestion_turn_start.is_some()
    }

    /// How [`words_asked`] names the request a compaction sends for its
    /// summary. The first time a decline stands on a conversation long enough
    /// to fold, the ladder folds it once and asks again (t-16786).
    const SUMMARY: &str = "(the compaction's summary request)";

    /// The words of the requests `provider` was sent in `requests`, in order,
    /// a compaction's summary request named [`SUMMARY`].
    fn words_asked(provider: &Declining, requests: std::ops::Range<usize>) -> Vec<String> {
        requests
            .map(|at| {
                let words =
                    provider.asked(at).as_ref().and_then(first_words).unwrap_or_default().to_string();
                if words.starts_with(COMPACTION_SYSTEM_PROMPT) {
                    SUMMARY.to_string()
                } else {
                    words
                }
            })
            .collect()
    }

    /// What a turn did that a turn which repeats a standing decline must not:
    /// the requests it sent, the hooks it ran, and whether it took anything
    /// from the team inbox.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct TurnDid {
        requests: usize,
        prompt_submits: usize,
        turn_ends: usize,
        took_from_the_inbox: bool,
    }

    /// Where a runtime stands between turns.
    struct Mark {
        requests: usize,
        runs: HookRuns,
        inbox: Vec<String>,
    }

    impl Mark {
        fn now(provider: &Declining, hooks: &CountingHooks, cwd: &Path) -> Self {
            Self {
                requests: provider.sent(),
                runs: hooks.runs(),
                inbox: team_inbox_taken(cwd),
            }
        }

        /// What happened between this mark and `later`.
        fn until(&self, later: &Self) -> TurnDid {
            TurnDid {
                requests: later.requests - self.requests,
                prompt_submits: later.runs.prompt_submit - self.runs.prompt_submit,
                turn_ends: later.runs.turn_end - self.runs.turn_end,
                took_from_the_inbox: later.inbox != self.inbox,
            }
        }
    }

    /// One road's account: what the repeat did, whether it left a turn's
    /// cleanup anything to settle, what the next turn did, and whether that
    /// turn's stop gate follow-up was asked.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Account {
        repeat: TurnDid,
        left_open: bool,
        next: TurnDid,
        follow_up_asked: bool,
    }

    /// Stands the decline on [`WORDS`] on `road`, gives the stop gate one
    /// follow-up and posts an inbox update, then runs the repeat and a turn
    /// of other words.
    fn account(road: Road) -> Account {
        let env = Hermetic::new(&format!("hooks-{road:?}"));
        let provider = Declining::new(UNROUTED);
        let hooks = CountingHooks::new(&env);
        let mut runtime = road.runtime(&env, &provider, &hooks);
        runtime.set_max_stop_loops(1);
        road.stand(&env, &mut runtime);
        hooks.answer_with(&[FOLLOW_UP]);
        post_to_team_inbox(&env.cwd, "u1");

        let before = Mark::now(&provider, &hooks, &env.cwd);
        road.run(&mut runtime, WORDS).expect("the repeat ends cleanly");
        let repeated = Mark::now(&provider, &hooks, &env.cwd);
        let left_open = cleanup_left_open(&runtime);
        road.run(&mut runtime, OTHER_WORDS).expect("the next turn ends cleanly");
        let after = Mark::now(&provider, &hooks, &env.cwd);

        Account {
            repeat: before.until(&repeated),
            left_open,
            next: repeated.until(&after),
            follow_up_asked: (repeated.requests..after.requests)
                .any(|at| provider.asked(at).as_ref().and_then(first_words) == Some(FOLLOW_UP)),
        }
    }

    /// The case of a stop gate. The same words after a decline that stood run
    /// no `UserPromptSubmit` and no `TurnEnd` hook on the headless loop, the
    /// streaming dispatcher or its deep mode, so nothing asks a follow-up;
    /// they take nothing from the team inbox and leave a turn's cleanup
    /// nothing to settle. The next turn, other words, runs both hooks, takes
    /// the inbox update and asks the follow-up — the same on the headless and
    /// the streaming loop.
    #[test]
    fn a_repeat_runs_no_lifecycle_hook_on_any_loop_and_the_next_turn_runs_them() {
        let accounts = ROADS.map(|road| (road, account(road)));
        eprintln!("t-57512 hooks | {accounts:?}");

        for (road, seen) in accounts {
            assert_eq!(
                seen.repeat,
                TurnDid::default(),
                "{road:?}: the repeat sends nothing, runs no hook and takes nothing from the inbox"
            );
            assert!(!seen.left_open, "{road:?}: and leaves a turn's cleanup nothing to settle");
            assert!(
                seen.next.prompt_submits > 0 && seen.next.turn_ends > 0,
                "{road:?}: the next turn runs both hooks: {seen:?}"
            );
            assert!(seen.next.took_from_the_inbox, "{road:?}: takes the inbox update");
            assert!(seen.follow_up_asked, "{road:?}: and its stop gate's follow-up is asked");
        }
        assert_eq!(accounts[0].1, accounts[1].1, "the headless and the streaming loop do the same");
    }

    /// What one turn sent, ran and settled: the words of each request it
    /// sent, in order, how many times each hook ran, the team inbox's
    /// deliveries after it, and whether it left its cleanup anything to settle.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Legs {
        asked: Vec<String>,
        prompt_submits: usize,
        turn_ends: usize,
        inbox: Vec<String>,
        left_open: bool,
    }

    /// One turn of [`OTHER_WORDS`] on `road` after the decline stood on
    /// [`WORDS`], beside a stop gate that answers with `follow_ups` and may
    /// be followed at most `stop_loops` times, and four team inbox updates for
    /// the legs to take, one each.
    fn follow_up_turn(road: Road, follow_ups: &[&str], stop_loops: usize) -> Legs {
        let env = Hermetic::new(&format!("follow-ups-{road:?}"));
        let provider = Declining::new(UNROUTED);
        let hooks = CountingHooks::new(&env);
        let mut runtime = road.runtime(&env, &provider, &hooks);
        runtime.set_max_stop_loops(stop_loops);
        road.stand(&env, &mut runtime);
        hooks.answer_with(follow_ups);
        for id in ["u1", "u2", "u3", "u4"] {
            post_to_team_inbox(&env.cwd, id);
        }

        let before = Mark::now(&provider, &hooks, &env.cwd);
        road.run(&mut runtime, OTHER_WORDS).expect("the turn ends cleanly");
        let after = Mark::now(&provider, &hooks, &env.cwd);
        Legs {
            asked: words_asked(&provider, before.requests..after.requests),
            prompt_submits: after.runs.prompt_submit - before.runs.prompt_submit,
            turn_ends: after.runs.turn_end - before.runs.turn_end,
            inbox: team_inbox_deliveries(&env.cwd),
            left_open: cleanup_left_open(&runtime),
        }
    }

    /// A stop gate that asks the same follow-up again and again while a
    /// decline stands. The follow-up is asked once and the decline stands on
    /// it; the next time it is the words the decline just stood on, so that
    /// leg sends nothing and runs no hook, and the turn ends: two legs on
    /// every loop, not one for each stop loop allowed. The turn still settles
    /// the team inbox update the follow-up took.
    #[test]
    fn a_stop_gate_repeating_its_follow_up_while_a_decline_stands_is_asked_once() {
        let turns = ROADS.map(|road| (road, follow_up_turn(road, &[FOLLOW_UP; 6], 4)));
        eprintln!("t-57512 repeated follow-up | {turns:?}");

        for (road, legs) in &turns {
            assert_eq!(
                legs.asked,
                [OTHER_WORDS, SUMMARY, OTHER_WORDS, FOLLOW_UP],
                "{road:?}: the turn's words, folded once and asked again, then the follow-up once"
            );
            assert_eq!(
                (legs.prompt_submits, legs.turn_ends),
                (2, 2),
                "{road:?}: each leg asked ran both hooks, and the leg that sent nothing ran neither"
            );
            assert_eq!(
                legs.inbox,
                ["delivery u1 acked", "delivery u2 acked"],
                "{road:?}: each leg asked took one update, and the turn settled the follow-up's, \
                 though the leg after it sent nothing"
            );
            assert!(!legs.left_open, "{road:?}: and left its cleanup nothing to settle");
        }
    }

    /// A stop gate's follow-up is the person's side of the turn, in deep mode
    /// too, where the deep driver runs every leg as an internal one: given
    /// again once the decline stood on it, it sends nothing.
    #[test]
    fn a_stop_gate_follow_up_is_never_taken_for_an_internal_deep_leg() {
        let env = Hermetic::new("follow-up-in-deep-mode");
        let provider = Declining::new(UNROUTED);
        let hooks = CountingHooks::new(&env);
        let mut runtime = Road::Deep.runtime(&env, &provider, &hooks);
        runtime.set_max_stop_loops(4);
        Road::Deep.stand(&env, &mut runtime);
        hooks.answer_with(&[FOLLOW_UP, FOLLOW_UP]);

        let before = provider.sent();
        Road::Deep.run(&mut runtime, OTHER_WORDS).expect("the turn ends cleanly");
        assert_eq!(
            words_asked(&provider, before..provider.sent()),
            [OTHER_WORDS, SUMMARY, OTHER_WORDS, FOLLOW_UP],
            "the follow-up given again, once the decline stood on it, sends nothing"
        );
    }

    /// The contrast, on its own (m-57968): an internal leg is the deep
    /// driver's own. Once a stop gate's follow-up met the decline standing, so
    /// that the person's same words would only repeat it, an internal leg of
    /// those words, from the same place, is asked once, as before.
    #[test]
    fn an_internal_deep_leg_of_the_words_a_decline_stood_on_is_asked() {
        let env = Hermetic::new("internal-leg-in-deep-mode");
        let provider = Declining::new(UNROUTED);
        let hooks = CountingHooks::new(&env);
        let mut runtime = Road::Deep.runtime(&env, &provider, &hooks);
        runtime.set_max_stop_loops(4);
        Road::Deep.stand(&env, &mut runtime);
        hooks.answer_with(&[FOLLOW_UP]);
        Road::Deep.run(&mut runtime, OTHER_WORDS).expect("the turn ends cleanly");
        assert!(
            runtime.repeats_a_standing_decline(FOLLOW_UP, Road::Deep.turn_loop()),
            "the decline stood on the follow-up"
        );

        let before = provider.sent();
        internal_leg(&mut runtime, FOLLOW_UP).expect("the internal leg ends cleanly");
        assert_eq!(
            words_asked(&provider, before..provider.sent()),
            [FOLLOW_UP],
            "an internal leg of the same words is asked, once"
        );
    }

    /// A follow-up whose words are not the ones just declined is asked, on
    /// every loop, as before.
    #[test]
    fn a_different_follow_up_after_a_decline_is_still_asked() {
        let turns =
            ROADS.map(|road| (road, follow_up_turn(road, &[FOLLOW_UP, OTHER_FOLLOW_UP], 4)));
        eprintln!("t-57512 different follow-ups | {turns:?}");

        for (road, legs) in &turns {
            assert_eq!(
                legs.asked,
                [OTHER_WORDS, SUMMARY, OTHER_WORDS, FOLLOW_UP, OTHER_FOLLOW_UP],
                "{road:?}: every leg is asked"
            );
            assert_eq!(legs.turn_ends, 3, "{road:?}: and the stop gate runs after each");
            assert_eq!(
                legs.inbox,
                ["delivery u1 acked", "delivery u2 acked", "delivery u3 acked"],
                "{road:?}: each leg took one update, and the turn settled them all"
            );
            assert!(!legs.left_open, "{road:?}: and left its cleanup nothing to settle");
        }
    }

    /// Another model is another classifier. After `/model` to another model
    /// of the same provider, or to another provider's, the same words are
    /// asked on every loop, as before.
    #[test]
    fn a_model_or_provider_change_after_a_decline_that_stood_asks_the_same_words() {
        for road in ROADS {
            for model in ["claude-opus-5", "gpt-5.6-sol"] {
                let env = Hermetic::new(&format!("model-{road:?}"));
                let provider = Declining::new(UNROUTED);
                let hooks = CountingHooks::new(&env);
                let mut runtime = road.runtime(&env, &provider, &hooks);
                road.stand(&env, &mut runtime);
                runtime.set_context_model(model);

                let before = provider.sent();
                let _ = road.run(&mut runtime, WORDS);
                assert_eq!(
                    provider.asked(before).as_ref().and_then(first_words),
                    Some(WORDS),
                    "{road:?} on {model}: the same words are asked"
                );
            }
        }
    }

    /// Deep mode never folds the decline it keeps, because every leg it runs
    /// is internal, so a repeat is found only where the conversation ends on
    /// the notice of the decline that stood. Words answered after that — the
    /// same words on another model, or other words on the same one — are asked
    /// when they are sent again, and so are the words the decline stood on,
    /// on every loop.
    #[test]
    fn words_answered_after_a_decline_stood_are_asked_when_sent_again() {
        let cases: [(&str, &[&str]); 2] = [
            ("claude-opus-5", &[WORDS, WORDS]),
            ("claude-opus-5-5", &[OTHER_WORDS, OTHER_WORDS, WORDS]),
        ];
        for road in ROADS {
            for (model, turns) in cases {
                let env = Hermetic::new(&format!("answered-{road:?}"));
                let provider = Declining::new(UNROUTED);
                let hooks = CountingHooks::new(&env);
                let mut runtime = road.runtime(&env, &provider, &hooks);
                road.stand(&env, &mut runtime);
                runtime.set_context_model(model);
                provider.answer_from_now_on();

                for (at, words) in turns.iter().enumerate() {
                    let before = provider.sent();
                    road.run(&mut runtime, words).expect("the turn ends cleanly");
                    assert_eq!(
                        words_asked(&provider, before..provider.sent()),
                        [*words],
                        "{road:?} on {model}: turn {at} of {turns:?} is asked, once"
                    );
                }
            }
        }
    }

    /// An answer the model gave is not the notice of a decline that stood,
    /// even when it reads the same, byte for byte (Codex, m-57924). After a
    /// decline stood, other words answered in the notice's words are asked
    /// when they are sent again, and so are the words the decline stood on, on
    /// every loop — deep mode included, where the kept decline is never
    /// folded.
    #[test]
    fn an_answer_in_the_notices_words_never_makes_a_repeat() {
        for road in ROADS {
            let env = Hermetic::new(&format!("notice-words-{road:?}"));
            let provider = Declining::new(UNROUTED);
            let hooks = CountingHooks::new(&env);
            let mut runtime = road.runtime(&env, &provider, &hooks);
            road.stand(&env, &mut runtime);
            provider.answer(Some(OTHER_WORDS), REFUSAL_STANDING_NOTICE);

            for words in [OTHER_WORDS, OTHER_WORDS, WORDS] {
                let before = provider.sent();
                road.run(&mut runtime, words).expect("the turn ends cleanly");
                assert_eq!(
                    words_asked(&provider, before..provider.sent()).first().map(String::as_str),
                    Some(words),
                    "{road:?}: {words:?} is asked"
                );
            }
        }
    }

    /// The same for a stop gate's follow-up. After a leg the decline stood on,
    /// a follow-up the model answered in the notice's words is asked when the
    /// stop gate gives it again, on every loop: on the headless loop and in
    /// deep mode a follow-up leg folds nothing, so the decline kept is still
    /// the latest.
    #[test]
    fn a_follow_up_answered_in_the_notices_words_is_asked_again() {
        for road in ROADS {
            let env = Hermetic::new(&format!("notice-words-legs-{road:?}"));
            let provider = Declining::new(UNROUTED);
            let hooks = CountingHooks::new(&env);
            let mut runtime = road.runtime(&env, &provider, &hooks);
            runtime.set_max_stop_loops(4);
            road.stand(&env, &mut runtime);
            road.run(&mut runtime, OTHER_WORDS).expect("other words fold the conversation once");
            hooks.answer_with(&[FOLLOW_UP, FOLLOW_UP]);
            provider.answer(Some(FOLLOW_UP), REFUSAL_STANDING_NOTICE);

            let before = provider.sent();
            road.run(&mut runtime, OTHER_FOLLOW_UP).expect("the turn ends cleanly");
            assert_eq!(
                words_asked(&provider, before..provider.sent()),
                [OTHER_FOLLOW_UP, FOLLOW_UP, FOLLOW_UP],
                "{road:?}: the decline stands on the turn's words, and the follow-up the model \
                 answered in the notice's words is asked again"
            );
        }
    }

    /// A route armed for a turn, named, and how a test arms it.
    type Route = (&'static str, fn(&mut Runtime));

    /// A route armed for the turn takes it elsewhere, to another classifier:
    /// after a decline stood, the same words are asked on every loop with an
    /// escalation installed, a quota cooldown armed or a refusal cooldown
    /// armed, each its own case.
    #[test]
    fn a_routing_change_after_a_decline_that_stood_asks_the_same_words() {
        let routes: [Route; 3] = [
            ("an escalation installed", |runtime| {
                runtime.set_escalation_model_override(Some("claude-opus-5".to_string()));
            }),
            ("a quota cooldown armed", |runtime| {
                runtime.quota_dry_until = Some(Instant::now() + Duration::from_secs(60));
            }),
            ("a refusal cooldown armed", |runtime| {
                runtime.refusal_dry_until = Some(Instant::now() + Duration::from_secs(60));
            }),
        ];
        for road in ROADS {
            for (route, arm) in routes {
                let env = Hermetic::new(&format!("route-{road:?}"));
                let provider = Declining::new(UNROUTED);
                let hooks = CountingHooks::new(&env);
                let mut runtime = road.runtime(&env, &provider, &hooks);
                road.stand(&env, &mut runtime);
                arm(&mut runtime);
                assert!(
                    !runtime.repeats_a_standing_decline(WORDS, road.turn_loop()),
                    "{road:?}, {route}: not a repeat"
                );

                let before = provider.sent();
                let _ = road.run(&mut runtime, WORDS);
                assert_eq!(
                    provider.asked(before).as_ref().and_then(first_words),
                    Some(WORDS),
                    "{road:?}, {route}: the same words are asked"
                );
            }
        }
    }

    /// The same words with a picture are another request in deep mode too:
    /// they are asked, the picture with them.
    #[test]
    fn the_same_words_with_a_picture_are_asked_in_deep_mode() {
        let env = Hermetic::new("picture-deep");
        let provider = Declining::new(UNROUTED);
        let hooks = CountingHooks::new(&env);
        let mut runtime = Road::Deep.runtime(&env, &provider, &hooks);
        Road::Deep.stand(&env, &mut runtime);

        let before = provider.sent();
        let picture = vec![("image/png".to_string(), "c2NyZWVuc2hvdA==".to_string())];
        let (ended, _) = turn(&mut runtime, WORDS, picture);
        ended.expect("the decline stands");

        let asked = provider.asked(before).expect("the request");
        assert_eq!(first_words(&asked), Some(WORDS), "asked with the words");
        assert!(
            asked
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. })),
            "with the picture: {asked:?}"
        );
    }

    /// A change to the conversation between two turns, named, and how a test
    /// makes it.
    type Change = (&'static str, fn(&Hermetic, &mut Runtime));

    /// What can happen to the conversation between two turns once a decline
    /// stood that makes it another conversation than the one the notice was
    /// pushed onto, even one that holds as many messages and still ends on the
    /// same words and notice (Codex, m-57949, m-57981 and m-57999). `/clear`
    /// starts a fresh session, as `/new` does. The same-length replacement
    /// stands in for deep mode's cleaned retry, which drops the exchange that
    /// stood and pushes the model's answer.
    fn changes() -> [Change; 8] {
        [
            ("a compaction", |_, runtime| {
                let config = CompactionConfig {
                    preserve_recent_messages: 2,
                    max_estimated_tokens: 1,
                };
                runtime.apply_manual_compaction(compact_session(&runtime.session, config));
            }),
            ("a fold of one message into its summary, which keeps the length", |_, runtime| {
                let messages = runtime.session.messages.len();
                let config = CompactionConfig {
                    preserve_recent_messages: messages - 1,
                    max_estimated_tokens: 1,
                };
                runtime.apply_manual_compaction(compact_session(&runtime.session, config));
                assert_eq!(runtime.session.messages.len(), messages, "the fold kept the length");
            }),
            ("a rewind of the last turn", |_, runtime| {
                assert!(runtime.rewind_turns(1) > 0, "the rewind took the turn back");
            }),
            ("a clear", |_, runtime| runtime.replace_session(Session::new())),
            ("a resume of the session from its file", |env, runtime| {
                let resumed =
                    Session::load_from_path(env.session_file()).expect("the session file loads");
                runtime.replace_session(resumed);
            }),
            (
                "a same-length replacement ending in a model answer identical to the notice",
                |_, runtime| {
                    let messages = Arc::make_mut(&mut runtime.session.messages);
                    let notice = messages.pop().expect("the notice");
                    messages.push(ConversationMessage::assistant(notice.blocks));
                    runtime.session.mark_transcript_dirty();
                },
            ),
            ("a same-length removal and reinsertion of the exchange that stood", |_, runtime| {
                let messages = Arc::make_mut(&mut runtime.session.messages);
                let asked = messages
                    .iter()
                    .rposition(|message| message.role == MessageRole::User)
                    .expect("the words");
                let exchange = messages.split_off(asked);
                messages.extend(exchange);
                runtime.session.mark_transcript_dirty();
            }),
            ("a same-length context edit of an earlier message", |_, runtime| {
                let mut edited = runtime.session.messages.to_vec();
                edited[0].blocks = vec![ContentBlock::Text {
                    text: "(edited)".to_string(),
                }];
                runtime
                    .session
                    .replace_context_atomic(Arc::new(edited))
                    .expect("the edit is published");
            }),
        ]
    }

    /// How a case reads: the loop, the change, and whether a snapshot of the
    /// conversation was held elsewhere while it was made, so that a change in
    /// place copied the conversation rather than moved it.
    fn case(road: Road, change: &str, snapshot_held: bool) -> String {
        let held = if snapshot_held { ", a snapshot held elsewhere" } else { "" };
        format!("{road:?}, after {change}{held}")
    }

    /// After each of those changes the same words are asked, on every loop, as
    /// before, whether or not a snapshot of the conversation was held
    /// elsewhere while it was made.
    #[test]
    fn the_same_words_after_the_conversation_changed_are_asked() {
        for road in ROADS {
            for (change, make) in changes() {
                for snapshot_held in [false, true] {
                    let env = Hermetic::new(&format!("changed-{road:?}"));
                    let provider = Declining::new(UNROUTED);
                    let hooks = CountingHooks::new(&env);
                    let mut runtime = road.runtime(&env, &provider, &hooks);
                    road.stand(&env, &mut runtime);
                    let _snapshot = snapshot_held.then(|| Arc::clone(&runtime.session.messages));
                    make(&env, &mut runtime);

                    let before = provider.sent();
                    let _ = road.run(&mut runtime, WORDS);
                    assert_eq!(
                        provider.asked(before).as_ref().and_then(first_words),
                        Some(WORDS),
                        "{}: the same words are asked",
                        case(road, change, snapshot_held)
                    );
                }
            }
        }
    }

    /// The rule takes none of those changes for a repeat, on any loop: it
    /// counts on the very conversation the notice was pushed onto, not on one
    /// that holds as many messages and ends the same way. Every case runs
    /// before the assertion, which names each case the rule took for a repeat.
    #[test]
    fn the_rule_takes_no_changed_conversation_for_a_repeat() {
        let mut taken = Vec::new();
        for road in ROADS {
            for (change, make) in changes() {
                for snapshot_held in [false, true] {
                    let env = Hermetic::new(&format!("changed-rule-{road:?}"));
                    let provider = Declining::new(UNROUTED);
                    let hooks = CountingHooks::new(&env);
                    let mut runtime = road.runtime(&env, &provider, &hooks);
                    road.stand(&env, &mut runtime);
                    let _snapshot = snapshot_held.then(|| Arc::clone(&runtime.session.messages));
                    make(&env, &mut runtime);
                    if runtime.repeats_a_standing_decline(WORDS, road.turn_loop()) {
                        taken.push(case(road, change, snapshot_held));
                    }
                }
            }
        }
        eprintln!("t-57512 changed conversations taken for a repeat | {taken:?}");
        assert!(taken.is_empty(), "the rule took these for a repeat: {taken:?}");
    }

    /// The contrast (Codex, m-57999). A change whose write failed is rolled
    /// back to the conversation it captured, and that is the very one the
    /// notice was pushed onto, unchanged: the change copied it, because the
    /// capture held it. So the decline still stands, and the same words send
    /// nothing and record nothing, on every loop. The rollback is made here as
    /// the session makes it (capture, change, put the capture back); a real
    /// write failure would need the test's folder made read-only mid-test.
    #[test]
    fn a_change_rolled_back_leaves_the_decline_standing() {
        for road in ROADS {
            let env = Hermetic::new(&format!("rolled-back-{road:?}"));
            let provider = Declining::new(UNROUTED);
            let hooks = CountingHooks::new(&env);
            let mut runtime = road.runtime(&env, &provider, &hooks);
            road.stand(&env, &mut runtime);
            let captured = Arc::clone(&runtime.session.messages);
            Arc::make_mut(&mut runtime.session.messages)
                .push(ConversationMessage::user_text("a change whose write failed"));
            runtime.session.messages = captured;
            assert!(
                runtime.repeats_a_standing_decline(WORDS, road.turn_loop()),
                "{road:?}: rolled back, the decline still stands"
            );

            let before = Trace::of(&provider, &runtime.session.messages, &env);
            road.run(&mut runtime, WORDS).expect("the repeat ends cleanly");
            assert_eq!(
                Trace::of(&provider, &runtime.session.messages, &env),
                before,
                "{road:?}: the same words sent nothing and recorded nothing"
            );
        }
    }

    /// The real repeat on every loop, through each loop's public entry and
    /// with the hooks in place: the same words after the decline stood send
    /// nothing, record nothing and say so in one line. So what the rule counts
    /// on survived the ending of the turn that stood: its hooks, its settling
    /// and its save (m-58078).
    #[test]
    fn a_real_repeat_sends_and_records_nothing_on_every_loop() {
        for road in ROADS {
            let env = Hermetic::new(&format!("real-repeat-{road:?}"));
            let provider = Declining::new(UNROUTED);
            let hooks = CountingHooks::new(&env);
            let mut runtime = road.runtime(&env, &provider, &hooks);
            road.stand(&env, &mut runtime);
            let stood = Trace::of(&provider, &runtime.session.messages, &env);

            let said: Vec<String> = match road {
                Road::Headless => {
                    let summary = runtime.run_turn(WORDS, None).expect("the repeat ends cleanly");
                    answer_of(&summary).map(str::to_string).into_iter().collect()
                }
                Road::Streaming | Road::Deep => {
                    let (ended, lines) = turn(&mut runtime, WORDS, Vec::new());
                    ended.expect("the repeat ends cleanly");
                    lines
                }
            };
            assert_eq!(
                Trace::of(&provider, &runtime.session.messages, &env),
                stood,
                "{road:?}: the repeat sent nothing and recorded nothing"
            );
            assert_eq!(
                said,
                [REFUSAL_STANDING_REPEAT_NOTICE.to_string()],
                "{road:?}: and said so in one line"
            );
        }
    }
}

/// Where the request a decline stood on went (m-59387): the same words are
/// taken for its repeat only while the client their loop would send them on
/// names the endpoint that request went to when it went out, on the model it
/// went out on. Every runtime here has both clients installed, each at its own
/// endpoint, so each loop shows that what counts is the client that loop
/// really sends on: the sync one for the headless loop, the async one for the
/// streaming dispatcher and for deep mode.
mod destination {
    use std::sync::{Arc, Mutex};

    use super::super::DeepGateConfig;
    use super::{
        first_words, standing_notices, turn, ApiClient, ApiRequest, AssistantEvent, AsyncApiClient,
        BlockId, ConversationRuntime, Declining, Future, Hermetic, PermissionMode, PermissionPolicy,
        Pin, RenderBlock, RuntimeError, StaticToolExecutor, Trace, OTHER_WORDS, UNROUTED, WORDS,
    };
    use crate::message_stream::types::WireModelSource;

    /// Where the sync client sends.
    const SYNC_AT: &str = "https://api.anthropic.com/v1/messages";

    /// Where the async client sends: another endpoint, with a port and a path.
    const ASYNC_AT: &str = "https://gateway.example.com:8443/anthropic/v1/messages";

    /// Where a client that moved sends.
    const ELSEWHERE: &str = "https://proxy.example.com/v1/messages";

    /// Where the sync client sends once it moved to another port of its host.
    const SYNC_ON_ANOTHER_PORT: &str = "https://api.anthropic.com:8443/v1/messages";

    /// Where the sync client sends once it moved under another path of its host.
    const SYNC_UNDER_ANOTHER_PATH: &str = "https://api.anthropic.com/proxy/v1/messages";

    /// Where the async client sends once it moved to another port of its host.
    const ASYNC_ON_ANOTHER_PORT: &str = "https://gateway.example.com:9443/anthropic/v1/messages";

    /// Where the async client sends once it moved under another path of its host.
    const ASYNC_UNDER_ANOTHER_PATH: &str = "https://gateway.example.com:8443/proxy/v1/messages";

    /// Where a client says it sends — nothing when it cannot say — and where
    /// it will say so once the next request it sends is out.
    struct Place {
        now: Mutex<Option<&'static str>>,
        once_out: Mutex<Option<&'static str>>,
    }

    impl Place {
        fn at(endpoint: &'static str) -> Arc<Self> {
            Arc::new(Self {
                now: Mutex::new(Some(endpoint)),
                once_out: Mutex::new(None),
            })
        }

        /// From now on the client says it sends to `endpoint`, or cannot say.
        fn move_to(&self, endpoint: Option<&'static str>) {
            *self.now.lock().expect("the place") = endpoint;
        }

        /// The client says it sends to `endpoint` from while its next request
        /// is out.
        fn move_while_the_next_request_is_out(&self, endpoint: &'static str) {
            *self.once_out.lock().expect("the place") = Some(endpoint);
        }

        fn now(&self) -> Option<&'static str> {
            *self.now.lock().expect("the place")
        }

        /// A request is out.
        fn sending(&self) {
            let moved = self.once_out.lock().expect("the place").take();
            if let Some(endpoint) = moved {
                self.move_to(Some(endpoint));
            }
        }
    }

    /// A client of `provider` at `place`, for either loop.
    struct Located {
        provider: Arc<Declining>,
        place: Arc<Place>,
    }

    impl Located {
        fn new(provider: &Arc<Declining>, place: &Arc<Place>) -> Self {
            Self {
                provider: Arc::clone(provider),
                place: Arc::clone(place),
            }
        }

        fn reply(&self, request: ApiRequest) -> Vec<AssistantEvent> {
            self.place.sending();
            self.provider.reply(request)
        }
    }

    impl ApiClient for Located {
        fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(self.reply(request))
        }

        fn destination(&self) -> Option<String> {
            self.place.now().map(str::to_string)
        }
    }

    impl AsyncApiClient for Located {
        fn stream_async<'a>(
            &'a self,
            request: ApiRequest,
            _render_tx: tokio::sync::mpsc::Sender<RenderBlock>,
            _text_block_id: BlockId,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a>>
        {
            Box::pin(async move { Ok(self.reply(request)) })
        }

        fn destination(&self) -> Option<String> {
            self.place.now().map(str::to_string)
        }
    }

    /// The places of a runtime's two clients.
    struct Clients {
        sync: Arc<Place>,
        streaming: Arc<Place>,
    }

    impl Clients {
        /// The sync client at [`SYNC_AT`], the async one at [`ASYNC_AT`].
        fn new() -> Self {
            Self {
                sync: Place::at(SYNC_AT),
                streaming: Place::at(ASYNC_AT),
            }
        }

        /// The place of the client `turn_loop` sends its requests on.
        fn sent_on(&self, turn_loop: Loop) -> &Arc<Place> {
            match turn_loop {
                Loop::Headless => &self.sync,
                Loop::Streaming | Loop::Deep => &self.streaming,
            }
        }

        /// The place of the client `turn_loop` never sends on.
        fn unused_by(&self, turn_loop: Loop) -> &Arc<Place> {
            match turn_loop {
                Loop::Headless => &self.streaming,
                Loop::Streaming | Loop::Deep => &self.sync,
            }
        }
    }

    /// The loops a turn runs on: the headless one, the streaming dispatcher,
    /// and the dispatcher in deep mode, whose legs run as internal turns.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Loop {
        Headless,
        Streaming,
        Deep,
    }

    const LOOPS: [Loop; 3] = [Loop::Headless, Loop::Streaming, Loop::Deep];

    /// The runtime every loop drives.
    type Runtime = ConversationRuntime<Located, StaticToolExecutor>;

    impl Loop {
        /// Opus 5.5 on this loop with both clients installed, each a client of
        /// `provider` at its place in `clients`.
        fn runtime(self, env: &Hermetic, provider: &Arc<Declining>, clients: &Clients) -> Runtime {
            let mut runtime = ConversationRuntime::new(
                env.session(),
                Located::new(provider, &clients.sync),
                StaticToolExecutor::new(),
                PermissionPolicy::new(PermissionMode::DangerFullAccess),
                vec!["system".to_string()],
            )
            .with_async_api_client(Arc::new(Located::new(provider, &clients.streaming)));
            if self == Self::Deep {
                runtime.set_deep_gate(Some(DeepGateConfig::default()));
            }
            runtime.set_context_model("claude-opus-5-5");
            runtime.set_workspace_cwd(env.cwd.clone());
            runtime
        }

        /// One turn of `words` on this loop.
        fn run(self, runtime: &mut Runtime, words: &str) {
            match self {
                Self::Headless => {
                    runtime.run_turn(words, None).expect("the turn ends cleanly");
                }
                Self::Streaming | Self::Deep => {
                    turn(runtime, words, Vec::new()).0.expect("the turn ends cleanly");
                }
            }
        }

        /// Turn one surfaces the decline on [`WORDS`]; then `between` runs, and
        /// turn two asks once and the decline stands.
        fn stand(self, runtime: &mut Runtime, between: impl FnOnce()) {
            self.run(runtime, WORDS);
            between();
            self.run(runtime, WORDS);
            assert_eq!(
                standing_notices(&runtime.session.messages),
                1,
                "{self:?}: the decline stood once"
            );
        }

        /// [`WORDS`] once more on this loop: the words of the first request the
        /// turn made, nothing when it made none, and how many it made.
        fn again(self, runtime: &mut Runtime, provider: &Declining) -> (Option<String>, usize) {
            let before = provider.sent();
            self.run(runtime, WORDS);
            let words = provider.asked(before).as_ref().and_then(first_words).map(str::to_string);
            (words, provider.sent() - before)
        }
    }

    /// The same words on every loop, asked as any turn is once the decline
    /// stood: as the person wrote them, in as many requests as other words
    /// make there — main's compaction walk (t-16786) and all — so that a send
    /// made twice fails as surely as one not made.
    fn asked_as_any_turn() -> Vec<(Loop, Option<String>, usize)> {
        LOOPS
            .into_iter()
            .map(|turn_loop| {
                let env = Hermetic::new(&format!("destination-any-turn-{turn_loop:?}"));
                let provider = Declining::new(UNROUTED);
                let clients = Clients::new();
                let mut runtime = turn_loop.runtime(&env, &provider, &clients);
                turn_loop.stand(&mut runtime, || {});
                let before = provider.sent();
                turn_loop.run(&mut runtime, OTHER_WORDS);
                (turn_loop, Some(WORDS.to_string()), provider.sent() - before)
            })
            .collect()
    }

    /// The same words on every loop after `change`, once the decline stood:
    /// what [`Loop::again`] answered, with the loop it ran on.
    fn asked_after(change: fn(&Clients, Loop)) -> Vec<(Loop, Option<String>, usize)> {
        let mut asked = Vec::new();
        for turn_loop in LOOPS {
            let env = Hermetic::new(&format!("destination-{turn_loop:?}"));
            let provider = Declining::new(UNROUTED);
            let clients = Clients::new();
            let mut runtime = turn_loop.runtime(&env, &provider, &clients);
            turn_loop.stand(&mut runtime, || {});
            change(&clients, turn_loop);
            assert_eq!(
                runtime.wire_model(),
                Some(("claude-opus-5-5", WireModelSource::Session)),
                "{turn_loop:?}: the model the decline stood on"
            );
            let (words, requests) = turn_loop.again(&mut runtime, &provider);
            asked.push((turn_loop, words, requests));
        }
        println!("asked, and the requests each turn made: {asked:?}");
        asked
    }

    /// The client a loop sends on names another endpoint now, on the model
    /// the decline stood on: the same words would go somewhere else, so they
    /// are asked, on every loop.
    #[test]
    fn the_same_words_are_asked_once_the_client_their_loop_sends_on_has_moved() {
        let asked = asked_after(|clients, turn_loop| {
            clients.sent_on(turn_loop).move_to(Some(ELSEWHERE));
        });
        assert_eq!(asked, asked_as_any_turn(), "the same words are asked on every loop");
    }

    /// The client a loop sends on cannot say where it sends any more, so
    /// nothing proves the same words would go where the decline stood: they
    /// are asked, on every loop.
    #[test]
    fn the_same_words_are_asked_once_the_client_their_loop_sends_on_cannot_say_where() {
        let asked = asked_after(|clients, turn_loop| clients.sent_on(turn_loop).move_to(None));
        assert_eq!(asked, asked_as_any_turn(), "the same words are asked on every loop");
    }

    /// The client a loop sends on never could say where it sends, neither for
    /// the request the decline stood on nor now. Two places nobody knows are
    /// never the same place: the same words are asked, on every loop.
    #[test]
    fn the_same_words_are_asked_when_no_client_ever_said_where_their_loop_sends() {
        let mut asked = Vec::new();
        for turn_loop in LOOPS {
            let env = Hermetic::new(&format!("destination-unknown-{turn_loop:?}"));
            let provider = Declining::new(UNROUTED);
            let clients = Clients::new();
            clients.sent_on(turn_loop).move_to(None);
            let mut runtime = turn_loop.runtime(&env, &provider, &clients);
            turn_loop.stand(&mut runtime, || {});
            let (words, requests) = turn_loop.again(&mut runtime, &provider);
            asked.push((turn_loop, words, requests));
        }
        println!("asked, and the requests each turn made: {asked:?}");
        assert_eq!(asked, asked_as_any_turn(), "the same words are asked on every loop");
    }

    /// The client a loop sends on names the same host on another port, or
    /// under another path: another endpoint, so the same words are asked, on
    /// every loop.
    #[test]
    fn the_same_words_are_asked_once_the_client_their_loop_sends_on_names_another_port_or_path() {
        let on_another_port = asked_after(|clients, turn_loop| {
            let moved = match turn_loop {
                Loop::Headless => SYNC_ON_ANOTHER_PORT,
                Loop::Streaming | Loop::Deep => ASYNC_ON_ANOTHER_PORT,
            };
            clients.sent_on(turn_loop).move_to(Some(moved));
        });
        let under_another_path = asked_after(|clients, turn_loop| {
            let moved = match turn_loop {
                Loop::Headless => SYNC_UNDER_ANOTHER_PATH,
                Loop::Streaming | Loop::Deep => ASYNC_UNDER_ANOTHER_PATH,
            };
            clients.sent_on(turn_loop).move_to(Some(moved));
        });
        let any_turn = asked_as_any_turn();
        assert_eq!(on_another_port, any_turn, "another port: asked on every loop");
        assert_eq!(under_another_path, any_turn, "another path: asked on every loop");
    }

    /// The client a loop sends on could not say where it sends when the
    /// request the decline stood on went out, and can now. Nothing proves the
    /// same words would go where that request went: they are asked, on every
    /// loop.
    #[test]
    fn the_same_words_are_asked_once_the_client_their_loop_sends_on_can_say_where_again() {
        let mut asked = Vec::new();
        for turn_loop in LOOPS {
            let env = Hermetic::new(&format!("destination-unknown-to-known-{turn_loop:?}"));
            let provider = Declining::new(UNROUTED);
            let clients = Clients::new();
            let place = clients.sent_on(turn_loop);
            let known = place.now();
            place.move_to(None);
            let mut runtime = turn_loop.runtime(&env, &provider, &clients);
            turn_loop.stand(&mut runtime, || {});
            place.move_to(known);
            let (words, requests) = turn_loop.again(&mut runtime, &provider);
            asked.push((turn_loop, words, requests));
        }
        println!("asked, and the requests each turn made: {asked:?}");
        assert_eq!(asked, asked_as_any_turn(), "the same words are asked on every loop");
    }

    /// The client a loop sends on moved while the request the decline stood
    /// on was out. Where that request went is where the client said it sends
    /// when the request went out, not where it says so once the decline is
    /// kept: the same words would go somewhere else, so they are asked.
    #[test]
    fn the_same_words_are_asked_when_the_client_moved_while_the_declined_request_was_out() {
        let mut asked = Vec::new();
        for turn_loop in LOOPS {
            let env = Hermetic::new(&format!("destination-in-flight-{turn_loop:?}"));
            let provider = Declining::new(UNROUTED);
            let clients = Clients::new();
            let mut runtime = turn_loop.runtime(&env, &provider, &clients);
            turn_loop.stand(&mut runtime, || {
                clients.sent_on(turn_loop).move_while_the_next_request_is_out(ELSEWHERE);
            });
            assert_eq!(
                clients.sent_on(turn_loop).now(),
                Some(ELSEWHERE),
                "{turn_loop:?}: the client moved while the request was out"
            );
            let (words, requests) = turn_loop.again(&mut runtime, &provider);
            asked.push((turn_loop, words, requests));
        }
        println!("asked, and the requests each turn made: {asked:?}");
        assert_eq!(asked, asked_as_any_turn(), "the same words are asked on every loop");
    }

    /// Only the client a loop never sends on moved: the same words would go
    /// where the decline stood, so they send nothing and record nothing. What
    /// counts is the client the loop really sends on.
    #[test]
    fn the_same_words_send_nothing_when_only_a_client_their_loop_never_uses_moved() {
        for turn_loop in LOOPS {
            let env = Hermetic::new(&format!("destination-unused-{turn_loop:?}"));
            let provider = Declining::new(UNROUTED);
            let clients = Clients::new();
            let mut runtime = turn_loop.runtime(&env, &provider, &clients);
            turn_loop.stand(&mut runtime, || {});
            let stood = Trace::of(&provider, &runtime.session.messages, &env);
            clients.unused_by(turn_loop).move_to(Some(ELSEWHERE));

            turn_loop.run(&mut runtime, WORDS);
            assert_eq!(
                Trace::of(&provider, &runtime.session.messages, &env),
                stood,
                "{turn_loop:?}: the repeat sent nothing and recorded nothing"
            );
        }
    }

    /// A host installs a fresh client every turn. A fresh client at the same
    /// endpoint sends the same words where the decline stood, so they send
    /// nothing and record nothing.
    #[test]
    fn the_same_words_send_nothing_through_a_fresh_client_at_the_same_endpoint() {
        for turn_loop in LOOPS {
            let env = Hermetic::new(&format!("destination-fresh-{turn_loop:?}"));
            let provider = Declining::new(UNROUTED);
            let clients = Clients::new();
            let mut runtime = turn_loop.runtime(&env, &provider, &clients);
            turn_loop.stand(&mut runtime, || {});
            let stood = Trace::of(&provider, &runtime.session.messages, &env);
            match turn_loop {
                Loop::Headless => {
                    *runtime.api_client_mut() = Located::new(&provider, &clients.sync);
                }
                Loop::Streaming | Loop::Deep => {
                    let fresh = Located::new(&provider, &clients.streaming);
                    runtime.set_async_api_client(Arc::new(fresh));
                }
            }

            turn_loop.run(&mut runtime, WORDS);
            assert_eq!(
                Trace::of(&provider, &runtime.session.messages, &env),
                stood,
                "{turn_loop:?}: the repeat sent nothing and recorded nothing"
            );
        }
    }
}
