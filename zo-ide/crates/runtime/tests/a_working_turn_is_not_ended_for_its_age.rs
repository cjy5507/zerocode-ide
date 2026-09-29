//! A turn that is working is not ended for its age (t-12076).
//!
//! The deadline of an unattended turn — a headless run, a summoned zo worker —
//! was a sixty-minute wall with two half-hour pushes for fresh progress: two
//! hours, and a worker still implementing was stopped with "Time budget
//! exhausted". The rule every road that could end a helper by its age now
//! answers to is the same one the pane's wait already follows: while there is
//! progress nothing cuts it, a tool call that is running is progress, and only
//! a limit somebody named is a wall clock. Whichever limit ends a turn, the
//! notice says which.
//!
//! The clocks here are milliseconds and the one env knob is a second, never
//! the hour the defaults are: the tests drive the same policy the host arms
//! (`env_deadline_extension`) with a base deadline made short by hand.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use runtime::message_stream::{RenderBlock, SystemLevel};
use runtime::permission::{
    PermissionDecision as AsyncPermissionDecision, PermissionError, PermissionPrompter,
    PermissionRequest as AsyncPermissionRequest,
};
use runtime::session::Session;
use runtime::{
    ApiClient, ApiRequest, Attendance, AssistantEvent, BudgetExhausted, ConversationRuntime,
    PermissionMode, PermissionPolicy, RuntimeError, StaticToolExecutor,
    DEFAULT_STREAMING_CHANNEL_CAPACITY,
};
use tokio::sync::mpsc;

/// The env knobs are the process's, and these tests read and set them.
static ENV: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    ENV.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One second is the shortest step the env can name.
const STEP_SECS: &str = "1";

/// A model that asks for one tool call per request until it has asked
/// `calls` times, and then answers. `thinks` is how long each request takes —
/// the model's time, which is nobody's progress.
struct ScriptedModel {
    asked: usize,
    calls: usize,
    tool: &'static str,
    thinks: Duration,
}

impl ApiClient for ScriptedModel {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        std::thread::sleep(self.thinks);
        self.asked += 1;
        if self.asked <= self.calls {
            // A different input each time: an identical call repeated is the
            // repetition guard's business, not this test's.
            return Ok(vec![
                AssistantEvent::ToolUse {
                    id: format!("call-{}", self.asked),
                    name: self.tool.to_string(),
                    input: format!(r#"{{"path":"file-{}.rs"}}"#, self.asked),
                },
                AssistantEvent::MessageStop,
            ]);
        }
        Ok(vec![
            AssistantEvent::TextDelta("done".to_string()),
            AssistantEvent::MessageStop,
        ])
    }
}

struct AllowPrompter;

impl PermissionPrompter for AllowPrompter {
    fn decide<'a>(
        &'a self,
        _request: AsyncPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AsyncPermissionDecision, PermissionError>> + Send + 'a>>
    {
        Box::pin(async { Ok(AsyncPermissionDecision::Allow) })
    }
}

/// What one turn came to: how it ended, how many requests the model got, and
/// what the turn said on the screen.
struct Ran {
    ended: Option<BudgetExhausted>,
    asked: usize,
    warnings: Vec<String>,
}

/// An unattended turn of `model` whose one tool takes `tool_takes`, under a
/// base deadline `base` from now and the policy the host arms from the env.
async fn run(model: ScriptedModel, tool_takes: Duration, base: Duration) -> Ran {
    let tool = model.tool;
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        model,
        StaticToolExecutor::new().register(tool, move |_| {
            std::thread::sleep(tool_takes);
            Ok("ok".to_string())
        }),
        PermissionPolicy::new(PermissionMode::DangerFullAccess),
        vec!["system".to_string()],
    );
    runtime.set_attendance(Attendance::Unattended);
    runtime.set_deadline(Instant::now() + base);
    runtime.set_deadline_extension(runtime::env_deadline_extension());

    let (tx, mut rx) = mpsc::channel(DEFAULT_STREAMING_CHANNEL_CAPACITY);
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(AllowPrompter);
    let drain = tokio::spawn(async move {
        let mut warnings = Vec::new();
        while let Some(block) = rx.recv().await {
            if let RenderBlock::System {
                level: SystemLevel::Warn,
                text,
                ..
            } = block
            {
                warnings.push(text);
            }
        }
        warnings
    });
    let summary = runtime
        .run_turn_streaming("go", tx, prompter)
        .await
        .expect("a turn that hits a budget completes, it does not error");
    let warnings = drain.await.expect("drain");
    Ran {
        ended: summary.budget_exhausted,
        asked: runtime_asked(&runtime),
        warnings,
    }
}

/// How many requests the model was given, read off the session: one assistant
/// message per request.
fn runtime_asked(runtime: &ConversationRuntime<ScriptedModel, StaticToolExecutor>) -> usize {
    runtime
        .session()
        .messages
        .iter()
        .filter(|message| message.role == runtime::session::MessageRole::Assistant)
        .count()
}

/// Progress is a file written: the model keeps editing, each edit taking a
/// quarter of a second, for far more looks than the two pushes a turn used to
/// get. Nothing ends it for its age — it ends when the model has nothing left
/// to do.
#[tokio::test]
async fn a_turn_that_keeps_writing_is_never_ended_for_its_age() {
    let _env = env_lock();
    std::env::set_var("ZO_DEADLINE_EXTENSION_SECS", STEP_SECS);
    std::env::remove_var("ZO_DEADLINE_EXTENSIONS");
    std::env::remove_var("ZO_TURN_DEADLINE_SECS");

    // Fourteen edits of a quarter second: about three and a half seconds under
    // a base deadline of a tenth and a step of one — the third look after the
    // base is where the second push used to be the last.
    let ran = run(
        ScriptedModel {
            asked: 0,
            calls: 14,
            tool: "edit_file",
            thinks: Duration::ZERO,
        },
        Duration::from_millis(250),
        Duration::from_millis(100),
    )
    .await;

    assert_eq!(
        ran.ended, None,
        "a turn still writing files was ended for its age after {} requests: {:?}",
        ran.asked, ran.warnings
    );
    assert_eq!(ran.asked, 15, "every edit and the closing answer");
}

/// A tool call that is running is progress: the clock does not run while it
/// does, so a build that takes longer than the whole window does not end the
/// turn that started it. A probe is not a file written, and the turn had none
/// — only the call's own time kept it alive.
#[tokio::test]
async fn a_tool_call_running_past_the_deadline_is_progress() {
    let _env = env_lock();
    std::env::set_var("ZO_DEADLINE_EXTENSION_SECS", STEP_SECS);
    std::env::remove_var("ZO_DEADLINE_EXTENSIONS");
    std::env::remove_var("ZO_TURN_DEADLINE_SECS");

    let ran = run(
        ScriptedModel {
            asked: 0,
            calls: 1,
            tool: "read_file",
            thinks: Duration::ZERO,
        },
        Duration::from_millis(300),
        Duration::from_millis(100),
    )
    .await;

    assert_eq!(
        ran.ended, None,
        "a turn was ended at the deadline the moment its tool call returned: {:?}",
        ran.warnings
    );
    assert_eq!(ran.asked, 2);
}

/// What ends a turn says which limit did it: no progress for a window — and
/// how long the window was — when nobody named a limit.
#[tokio::test]
async fn a_turn_ended_for_no_progress_says_so() {
    let _env = env_lock();
    std::env::set_var("ZO_DEADLINE_EXTENSION_SECS", STEP_SECS);
    std::env::remove_var("ZO_DEADLINE_EXTENSIONS");
    std::env::remove_var("ZO_TURN_DEADLINE_SECS");

    // The model's own time is nobody's progress: a request that takes longer
    // than the window, and a probe after it, and no file written.
    let ran = run(
        ScriptedModel {
            asked: 0,
            calls: 1,
            tool: "read_file",
            thinks: Duration::from_millis(150),
        },
        Duration::ZERO,
        Duration::from_millis(50),
    )
    .await;

    assert_eq!(ran.ended, Some(BudgetExhausted::Deadline));
    assert!(
        ran.warnings
            .iter()
            .any(|text| text.contains("no progress for")),
        "the notice does not say it was progress that ran out: {:?}",
        ran.warnings
    );
}

/// A limit somebody named is a wall clock, and says so: the person who sets
/// `ZO_TURN_DEADLINE_SECS` is promised that limit, working or not.
#[tokio::test]
async fn a_deadline_somebody_named_is_a_wall_clock_that_says_so() {
    let _env = env_lock();
    std::env::set_var("ZO_DEADLINE_EXTENSION_SECS", STEP_SECS);
    std::env::remove_var("ZO_DEADLINE_EXTENSIONS");
    std::env::set_var("ZO_TURN_DEADLINE_SECS", "1");

    let named = runtime::env_turn_budgets(Attendance::Unattended)
        .0
        .expect("the named deadline is read");
    assert_eq!(named, Duration::from_secs(1));
    let ran = run(
        ScriptedModel {
            asked: 0,
            calls: 20,
            tool: "edit_file",
            thinks: Duration::ZERO,
        },
        Duration::from_millis(250),
        named,
    )
    .await;
    std::env::remove_var("ZO_TURN_DEADLINE_SECS");

    assert_eq!(
        ran.ended,
        Some(BudgetExhausted::Deadline),
        "a working turn outlived the limit somebody named ({} requests)",
        ran.asked
    );
    // Four edits fit in the second; a wall that is pushed on for progress lets
    // it run for three or four times that.
    assert!(
        ran.asked <= 6,
        "the named second was pushed on for a turn that kept writing: {} requests",
        ran.asked
    );
    assert!(
        ran.warnings
            .iter()
            .any(|text| text.contains("its limit of 1s")),
        "the notice does not name the limit: {:?}",
        ran.warnings
    );
}
