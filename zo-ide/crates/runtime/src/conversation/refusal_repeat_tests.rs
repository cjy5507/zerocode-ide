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

/// One streaming turn of `words` and `images`: how it ended, and the lines the
/// screen was told — read to the end, which comes only when the turn has let
/// go of the screen.
fn turn(
    runtime: &mut ConversationRuntime<NotAsked, StaticToolExecutor>,
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
        let ended = runtime
            .run_turn_streaming_maybe_deep(words, images, render_tx, prompter)
            .await;
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
