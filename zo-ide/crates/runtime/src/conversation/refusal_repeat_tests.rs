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

/// The premise of the tests below: on Opus 5.5 the provider routes
/// [`UNROUTED`] nowhere, and `cyber` somewhere.
#[test]
fn the_premise_one_category_is_routed_nowhere_and_one_somewhere() {
    assert!(::api::refusal_route_candidates("claude-opus-5-5", Some(UNROUTED)).is_empty());
    assert!(!::api::refusal_route_candidates("claude-opus-5-5", Some("cyber")).is_empty());
}

/// A provider whose classifier declines every request, naming `category`, and
/// keeps every request it was sent.
struct Declining {
    category: &'static str,
    requests: Mutex<Vec<ApiRequest>>,
}

impl Declining {
    fn new(category: &'static str) -> Arc<Self> {
        Arc::new(Self {
            category,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn decline(&self, request: ApiRequest) -> Vec<AssistantEvent> {
        self.requests.lock().expect("requests").push(request);
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
        Box::pin(async move { Ok(self.decline(request)) })
    }
}

/// The same provider behind the headless loop's client.
struct DecliningSync(Arc<Declining>);

impl ApiClient for DecliningSync {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        Ok(self.0.decline(request))
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
fn internal_leg(
    runtime: &mut ConversationRuntime<NotAsked, StaticToolExecutor>,
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

/// How many turn records were written under `cwd`.
fn turn_records(cwd: &Path) -> usize {
    let dir = crate::traces_base(cwd)
        .join(core_types::paths::ZO_DIR_NAME)
        .join("turns");
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
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
    let (ended, lines) = turn(&mut runtime, OTHER_WORDS, Vec::new());
    ended.expect("the next turn ends cleanly");
    assert_eq!(provider.sent() - before, 1, "the next turn asks once");
    assert!(
        lines.contains(&REFUSAL_STANDING_NOTICE.to_string()),
        "and stands in the words of a decline that stands: {lines:?}"
    );
}

/// Other words are another request: after a decline that stood, and after a
/// repeat of it, they are asked once, as before — and the classifier judges
/// them.
#[test]
fn other_words_after_a_decline_that_stood_are_asked_once() {
    let env = Hermetic::new("other-words");
    let provider = Declining::new(UNROUTED);
    let mut runtime = stood_runtime(&env, &provider);
    let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("the repeat ends cleanly");

    for words in [OTHER_WORDS, WORDS] {
        let before = provider.sent();
        let (ended, _) = turn(&mut runtime, words, Vec::new());
        ended.expect("the decline stands");
        assert_eq!(provider.sent() - before, 1, "{words:?} after other words: asked once");
        assert_eq!(
            provider.asked(before).as_ref().and_then(first_words),
            Some(words),
            "as the person wrote them"
        );
    }
}

/// The same words with a picture are another request: they are asked once.
#[test]
fn the_same_words_with_a_picture_are_asked_once() {
    let env = Hermetic::new("picture");
    let provider = Declining::new(UNROUTED);
    let mut runtime = stood_runtime(&env, &provider);

    let before = provider.sent();
    let picture = vec![("image/png".to_string(), "c2NyZWVuc2hvdA==".to_string())];
    let (ended, _) = turn(&mut runtime, WORDS, picture);
    ended.expect("the decline stands");

    assert_eq!(provider.sent() - before, 1, "asked once");
    let asked = provider.asked(before).expect("the request");
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
        assert!(runtime.repeats_a_standing_decline(WORDS), "{road}: on its own road the words would repeat");

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
        assert!(!runtime.repeats_a_standing_decline(WORDS), "so its repeat is never found");
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
        !runtime.repeats_a_standing_decline(WORDS),
        "a decline surfaced after the ladder has not stood yet: the next turn asks once"
    );
    let (ended, _) = turn(&mut runtime, WORDS, Vec::new());
    ended.expect("turn two stands");
    assert!(runtime.repeats_a_standing_decline(WORDS));
    assert!(!runtime.repeats_a_standing_decline(OTHER_WORDS), "other words");
    assert!(
        !runtime.repeats_a_standing_decline(&format!("{WORDS} ")),
        "other bytes are other words"
    );

    let mut ended_otherwise = runtime.surfaced_decline.clone().expect("the kept decline");
    ended_otherwise.renewed = false;
    let kept = runtime.surfaced_decline.replace(ended_otherwise);
    assert!(
        !runtime.repeats_a_standing_decline(WORDS),
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
        !runtime.repeats_a_standing_decline(WORDS),
        "the conversation changed by the drift, though it still ends on the same words and notice"
    );
    runtime.session.messages = messages;
    assert!(runtime.repeats_a_standing_decline(WORDS));

    runtime.quota_dry_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
    assert!(
        !runtime.repeats_a_standing_decline(WORDS),
        "a quota cooldown could carry the turn to another provider"
    );
    runtime.quota_dry_until = None;

    runtime.set_context_model("claude-opus-5");
    assert!(!runtime.repeats_a_standing_decline(WORDS), "another model");
}

/// The lifecycle hook contract, on every loop a turn runs on. A turn, or a
/// stop gate's follow-up, that repeats the words a decline just stood on runs
/// no hook: no `UserPromptSubmit`, and no `TurnEnd`, so no stop gate asks a
/// follow-up after it and the turn ends with it. It takes nothing from the
/// team inbox. Everything else runs them as before. The hooks are shell
/// commands, so these run where a POSIX shell does.
#[cfg(unix)]
mod hook_contract {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rusqlite::{params, Connection};

    use super::super::DeepGateConfig;
    use super::{
        first_words, standing_notices, turn, AsyncApiClient, ConversationRuntime, Declining,
        DecliningSync, Hermetic, PermissionMode, PermissionPolicy, StaticToolExecutor,
        TurnSummary, OTHER_WORDS, UNROUTED, WORDS,
    };
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

        /// A runtime's features with these hooks, loaded as zo loads settings.
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
                "hooks": { "UserPromptSubmit": [prompt_submit], "TurnEnd": [turn_end] }
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
        let left_open = runtime.team_inbox_turn.is_some()
            || runtime.file_pick_pending_turn.is_some()
            || runtime.skill_suggestion_turn_start.is_some();
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

    /// What one turn sent and ran: the words of each request it sent, in
    /// order, and how many times each hook ran.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Legs {
        asked: Vec<String>,
        prompt_submits: usize,
        turn_ends: usize,
    }

    /// One turn of [`OTHER_WORDS`] on `road` after the decline stood on
    /// [`WORDS`], beside a stop gate that answers with `follow_ups` and may
    /// be followed at most `stop_loops` times.
    fn follow_up_turn(road: Road, follow_ups: &[&str], stop_loops: usize) -> Legs {
        let env = Hermetic::new(&format!("follow-ups-{road:?}"));
        let provider = Declining::new(UNROUTED);
        let hooks = CountingHooks::new(&env);
        let mut runtime = road.runtime(&env, &provider, &hooks);
        runtime.set_max_stop_loops(stop_loops);
        road.stand(&env, &mut runtime);
        hooks.answer_with(follow_ups);

        let before = Mark::now(&provider, &hooks, &env.cwd);
        road.run(&mut runtime, OTHER_WORDS).expect("the turn ends cleanly");
        let after = Mark::now(&provider, &hooks, &env.cwd);
        Legs {
            asked: (before.requests..after.requests)
                .map(|at| {
                    provider.asked(at).as_ref().and_then(first_words).unwrap_or_default().to_string()
                })
                .collect(),
            prompt_submits: after.runs.prompt_submit - before.runs.prompt_submit,
            turn_ends: after.runs.turn_end - before.runs.turn_end,
        }
    }

    /// A stop gate that asks the same follow-up again and again while a
    /// decline stands. The follow-up is asked once and the decline stands on
    /// it; the next time it is the words the decline just stood on, so that
    /// leg sends nothing and runs no hook, and the turn ends: two legs on
    /// every loop, not one for each stop loop allowed.
    #[test]
    fn a_stop_gate_repeating_its_follow_up_while_a_decline_stands_is_asked_once() {
        let turns = ROADS.map(|road| (road, follow_up_turn(road, &[FOLLOW_UP; 6], 4)));
        eprintln!("t-57512 repeated follow-up | {turns:?}");

        for (road, legs) in &turns {
            assert_eq!(
                legs.asked,
                [OTHER_WORDS, FOLLOW_UP],
                "{road:?}: the turn's words, then the follow-up once"
            );
            assert_eq!(
                (legs.prompt_submits, legs.turn_ends),
                (2, 2),
                "{road:?}: each leg asked ran both hooks, and the leg that sent nothing ran neither"
            );
        }
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
                [OTHER_WORDS, FOLLOW_UP, OTHER_FOLLOW_UP],
                "{road:?}: every leg is asked"
            );
            assert_eq!(legs.turn_ends, 3, "{road:?}: and the stop gate runs after each");
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
}
