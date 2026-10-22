//! What a turn the person did not ask for — the window's mail pointer — and a
//! question the person asks between turns are told about the work the person
//! left unfinished in this session (t-72017).
//!
//! The window types its mail pointer into the composer as a person would, so
//! zo took it for a new request: a coordinator read four letters, reported
//! them and ended its turn with the person's verification still undone, as the
//! turn that answered the person's question had ended just before it. A line
//! the person types while a turn runs already says "answer, then carry on"
//! (the runtime's steering preamble); this is the word for the turns that
//! begin between turns.
//!
//! Only this session is read: the words the person typed in this process and
//! the open items of the session's own plan (its todo store, by session id).
//! Nothing is resumed that the person stopped, that an unattended controller
//! owns, or after the session was opened here and before the person wrote.

use std::path::Path;

/// Marks the reminder. Set or cleared on every turn by replace-by-prefix, so
/// it rides the one turn it was decided for.
pub(crate) const TASK_CONTINUITY_REMINDER_PREFIX: &str = "[zo:task-continuity]";

/// Who a turn's input came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnOrigin {
    /// The window's mail pointer — `orchestration::pointer_text` and nothing
    /// else.
    MailPointer,
    /// A goal, loop or cron turn the scheduler opened.
    Unattended,
    /// A background helper's result or notice, run as a turn of its own.
    HostNotice,
    /// Anything else typed into the composer.
    Person,
}

/// How the controller of the latest unattended turn stands at a turn's start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ControllerStanding {
    /// `loop-1`, `goal`, `cron <id>`.
    pub(crate) label: String,
    /// The controller's own word for its phase.
    pub(crate) phase: String,
    /// Why it stands so, when it says.
    pub(crate) reason: Option<String>,
    /// Paused or stopped: held until the person resumes it.
    pub(crate) held: bool,
}

/// What this process saw of the person's work in one session.
#[derive(Debug, Default)]
pub(crate) struct TaskContinuity {}

impl TaskContinuity {
    /// Begin a turn on `runtime`: say who `input` is from, put the turn's
    /// reminder on the runtime (or take the last one off), and give the one
    /// line the screen shows when the latest work stays held. `standing`
    /// answers how a controller stands, by its label.
    pub(crate) fn begin_turn<C, T>(
        &mut self,
        _runtime: &mut runtime::ConversationRuntime<C, T>,
        _input: &str,
        _cwd: &Path,
        _standing: impl FnOnce(&str) -> Option<ControllerStanding>,
    ) -> (TurnOrigin, Option<String>)
    where
        C: runtime::ApiClient,
        T: runtime::ToolExecutor,
    {
        (TurnOrigin::Person, None)
    }

    /// The scheduler is about to run `label`'s unattended turn.
    pub(crate) fn unattended_dispatched(&mut self, _label: String) {}

    /// A turn ended; `stopped_by_person` when the person interrupted it.
    pub(crate) fn end_turn(&mut self, _origin: TurnOrigin, _stopped_by_person: bool) {}
}

/// How the controller `label` names stands now.
pub(crate) fn controller_standing(
    _autonomy: &crate::autonomy::Autonomy,
    _label: &str,
) -> Option<ControllerStanding> {
    None
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use core_types::session::{ContentBlock, ConversationMessage, MessageRole};
    use runtime::{
        ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, PermissionMode,
        PermissionPolicy, RuntimeError, RuntimeFeatureConfig, StaticToolExecutor,
    };

    use super::{
        controller_standing, ControllerStanding, TaskContinuity, TASK_CONTINUITY_REMINDER_PREFIX,
    };
    use crate::autonomy::loops::LoopPhase;

    /// The line the window types, as the composer hands it to a turn.
    fn pointer(count: usize) -> String {
        zerocode_core::orchestration::pointer_text(count)
            .trim()
            .to_string()
    }

    /// A provider that answers every request with one sentence and keeps the
    /// messages it was sent.
    #[derive(Clone, Default)]
    struct Provider {
        sent: Arc<Mutex<Vec<Arc<Vec<ConversationMessage>>>>>,
    }

    impl ApiClient for Provider {
        fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            self.sent
                .lock()
                .expect("the provider's record")
                .push(Arc::clone(&request.messages));
            Ok(vec![
                AssistantEvent::TextDelta("Noted.".to_string()),
                AssistantEvent::MessageStop,
            ])
        }
    }

    /// A project folder of the test's own, with every plan store under it:
    /// the session state root is moved there and no shared plan file is
    /// named, so nothing reaches a store a person's sessions use.
    struct Stores {
        root: tempfile::TempDir,
        state: Option<std::ffi::OsString>,
        shared: Option<std::ffi::OsString>,
    }

    impl Stores {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("a test folder");
            let state = std::env::var_os(core_types::paths::ZO_STATE_DIR_ENV);
            let shared = std::env::var_os("ZO_TODO_STORE");
            std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, root.path().join("state"));
            std::env::remove_var("ZO_TODO_STORE");
            Self {
                root,
                state,
                shared,
            }
        }

        fn project(&self, name: &str) -> PathBuf {
            let project = self.root.path().join(name);
            std::fs::create_dir_all(&project).expect("a project folder");
            project
        }
    }

    impl Drop for Stores {
        fn drop(&mut self) {
            match self.state.take() {
                Some(value) => std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, value),
                None => std::env::remove_var(core_types::paths::ZO_STATE_DIR_ENV),
            }
            if let Some(value) = self.shared.take() {
                std::env::set_var("ZO_TODO_STORE", value);
            }
        }
    }

    /// What one turn was told, and what the screen was given.
    struct Told {
        reminder: Option<String>,
        held: Option<String>,
    }

    /// One synthetic session: its runtime, what the provider was sent, and the
    /// host's continuity, kept as `PlainSession` keeps it.
    struct Synthetic {
        cwd: PathBuf,
        runtime: ConversationRuntime<Provider, StaticToolExecutor>,
        provider: Provider,
        continuity: TaskContinuity,
    }

    impl Synthetic {
        fn open(cwd: &Path, session_id: &str) -> Self {
            let provider = Provider::default();
            let mut session = core_types::Session::new();
            session.session_id = session_id.to_string();
            let features = RuntimeFeatureConfig::default().with_auto_dream_enabled(false);
            let mut runtime = ConversationRuntime::new_with_features(
                session,
                provider.clone(),
                StaticToolExecutor::new(),
                PermissionPolicy::new(PermissionMode::ReadOnly),
                vec!["system".to_string()],
                &features,
            );
            runtime.set_workspace_cwd(cwd.to_path_buf());
            Self {
                cwd: cwd.to_path_buf(),
                runtime,
                provider,
                continuity: TaskContinuity::default(),
            }
        }

        /// The plan the model keeps for the person's work, in this session's
        /// own store, as `TodoWrite` leaves it.
        fn plan(&self, items: &[(&str, &str)]) {
            let store = runtime::todo_store::primary_store(
                &self.cwd,
                Some(self.runtime.session().session_id.as_str()),
            )
            .expect("a session's store");
            std::fs::create_dir_all(store.parent().expect("the store's folder"))
                .expect("the store's folder");
            let todos: Vec<serde_json::Value> = items
                .iter()
                .map(|(content, status)| {
                    serde_json::json!({"content": content, "activeForm": content, "status": status})
                })
                .collect();
            std::fs::write(&store, serde_json::to_vec(&todos).expect("a plan")).expect("a plan");
        }

        /// One turn as `PlainSession::run_turn_with_images` runs it: begin it
        /// (who it is from, what the model is told), run it, end it. What the
        /// turn was told is the reminder it carried, and the provider's
        /// request must hold it: after the turn's words, or — the same words
        /// a turn or two before — where the runtime leaves one copy.
        fn turn(
            &mut self,
            input: &str,
            standing: impl FnOnce(&str) -> Option<ControllerStanding>,
            stopped_by_person: bool,
        ) -> Told {
            let (origin, held) =
                self.continuity
                    .begin_turn(&mut self.runtime, input, &self.cwd, standing);
            let reminder = self
                .runtime
                .transient_reminders()
                .iter()
                .find(|reminder| reminder.starts_with(TASK_CONTINUITY_REMINDER_PREFIX))
                .cloned();
            let asked = self.provider.sent.lock().expect("the provider's record").len();
            self.runtime.run_turn(input, None).expect("the turn runs");
            self.continuity.end_turn(origin, stopped_by_person);
            let sent = self.provider.sent.lock().expect("the provider's record");
            let request = sent.get(asked).expect("the turn asked the provider");
            if let Some(reminder) = reminder.as_deref() {
                assert!(carried(request, reminder), "the model's request carries the reminder: {reminder}");
            }
            Told { reminder, held }
        }

        fn person(&mut self, words: &str) -> Told {
            self.turn(words, |_| None, false)
        }
    }

    /// Whether a request carries `reminder` as a reminder message of its own.
    fn carried(messages: &[ConversationMessage], reminder: &str) -> bool {
        messages
            .iter()
            .filter(|message| message.role == MessageRole::System)
            .flat_map(|message| message.blocks.iter())
            .any(|block| matches!(block, ContentBlock::Text { text } if text == reminder))
    }

    /// (a) The window's mail line in the middle of the person's work: the
    /// model is told the line is not the person's, and to go back to the work
    /// after the mail — the person's words and the plan's open items named.
    #[test]
    fn a_mail_line_in_the_middle_of_the_persons_work_is_told_to_go_back_to_it_after_the_mail() {
        let _env = crate::test_env_lock();
        let stores = Stores::new();
        let mut session = Synthetic::open(&stores.project("work"), "session-a");
        session.person("Fix the parser and run its tests until they pass.");
        session.plan(&[("Fix the parser", "completed"), ("Run the parser tests", "in_progress")]);

        let told = session.turn(&pointer(2), |_| None, false);

        let said = told
            .reminder
            .expect("the mail line's turn tells the model what the line is and what comes after the mail");
        assert!(said.contains("not a request from the person"), "{said}");
        assert!(said.contains("continue it in this same turn"), "{said}");
        assert!(said.contains("\"Fix the parser and run its tests until they pass.\""), "{said}");
        assert!(said.contains("Run the parser tests (in_progress)"), "{said}");
        assert!(!said.contains("Fix the parser (completed)"), "a finished item is not open: {said}");
        assert_eq!(told.held, None, "nothing is held");
    }

    /// (b) A question between turns while the person's work is open: answer,
    /// then go on — and a question with nothing open is left alone.
    #[test]
    fn a_question_between_turns_while_the_work_is_open_is_told_to_answer_and_go_on() {
        let _env = crate::test_env_lock();
        let stores = Stores::new();
        let mut session = Synthetic::open(&stores.project("work"), "session-b");
        session.person("Rename the config loader and update every caller.");
        session.plan(&[("Rename the loader", "completed"), ("Update the callers", "pending")]);

        let told = session.person("Why did you pick this model?");

        let said = told
            .reminder
            .expect("the question's turn tells the model the work is open and to go on after the answer");
        assert!(said.contains("answer it, then continue the work in this same turn"), "{said}");
        assert!(said.contains("\"Rename the config loader and update every caller.\""), "{said}");
        assert!(said.contains("Update the callers (pending)"), "{said}");
        assert!(!said.contains("Why did you pick this model?"), "the question is not its own earlier words: {said}");

        session.plan(&[("Rename the loader", "completed"), ("Update the callers", "completed")]);
        let finished = session.person("And which provider is it on?");
        assert_eq!(finished.reminder, None, "a question with nothing open is told nothing");
    }

    /// (c) The person stopped the work: the window's mail lines do not restart
    /// it, until the person writes again.
    #[test]
    fn a_mail_line_after_the_person_stopped_the_work_is_told_not_to_restart_it() {
        let _env = crate::test_env_lock();
        let stores = Stores::new();
        let mut session = Synthetic::open(&stores.project("work"), "session-c");
        session.plan(&[("Run the full verification", "in_progress")]);
        // The person interrupts the turn doing the work (Esc, or the window's stop).
        session.turn("Run the full verification and write the report.", |_| None, true);

        let told = session.turn(&pointer(1), |_| None, false);

        let said = told
            .reminder
            .expect("the mail line's turn says the person stopped the work");
        assert!(said.contains("The person stopped the latest work"), "{said}");
        assert!(said.contains("Do not restart it"), "{said}");
        assert!(!said.contains("continue it in this same turn"), "{said}");
        let again = session.turn(&pointer(2), |_| None, false).reminder.unwrap_or_default();
        assert!(again.contains("Do not restart it"), "a second mail line holds the stop too: {again}");

        let next = session.person("What was left?").reminder.unwrap_or_default();
        assert!(
            next.contains("treat that work as stopped unless this message asks to go on"),
            "the person's next words are told of the stop: {next}"
        );
        let after = session.turn(&pointer(3), |_| None, false).reminder.unwrap_or_default();
        assert!(after.contains("continue it in this same turn"), "the person wrote since: {after}");
    }

    /// (d) A loop its budget paused: the window's mail line does not resume
    /// it, and the screen says why in one line.
    #[test]
    fn a_mail_line_after_a_loop_its_budget_paused_leaves_it_paused_and_says_why_in_one_line() {
        let _env = crate::test_env_lock();
        let stores = Stores::new();
        let cwd = stores.project("work");
        let mut autonomy = crate::autonomy::Autonomy::load(&cwd.join("session-d.jsonl"), None, false);
        autonomy
            .loops
            .apply(
                commands::LoopCommand::StartDynamic {
                    prompt: "Re-run the nightly verification".to_string(),
                },
                1_000,
                &cwd,
            )
            .expect("a loop");
        let mut session = Synthetic::open(&cwd, "session-d");
        session.person("Keep the nightly verification going.");
        // The loop's own turn runs, then the session's budget pauses it.
        session.continuity.unattended_dispatched("loop-1".to_string());
        session.runtime.set_autonomous_surface(true);
        session.turn("[zo:loop-iteration id=loop-1]\nRe-run the nightly verification", |_| None, false);
        session.runtime.set_autonomous_surface(false);
        autonomy
            .loops
            .pause_for_session_limit("session output-token limit reached (58624/32000)")
            .expect("paused");

        let told = session.turn(&pointer(4), |label| controller_standing(&autonomy, label), false);

        let held = told
            .held
            .expect("the screen says in one line why the loop is not resumed");
        assert_eq!(
            held,
            "loop-1 is paused: session output-token limit reached (58624/32000); the mail line does not resume it"
        );
        let said = told.reminder.unwrap_or_default();
        assert!(said.contains("loop-1"), "{said}");
        assert!(said.contains("does not resume it"), "{said}");
        assert!(!said.contains("continue it in this same turn"), "{said}");
        assert_eq!(autonomy.loops.phase("loop-1"), Some(LoopPhase::Paused));
        assert_eq!(
            autonomy.loops.dispatch_due(u64::MAX, &cwd).expect("the loop state"),
            None,
            "a paused loop is not dispatched"
        );
    }

    /// (e) Another session in the same project, and a session of the same id
    /// in another project: neither one's words nor plan reach this session.
    #[test]
    fn a_mail_line_names_only_its_own_sessions_words_and_plan() {
        let _env = crate::test_env_lock();
        let stores = Stores::new();
        let project = stores.project("work");
        let mut mine = Synthetic::open(&project, "session-mine");
        let mut neighbour = Synthetic::open(&project, "session-neighbour");
        let mut elsewhere = Synthetic::open(&stores.project("other"), "session-mine");
        neighbour.person("Neighbour: migrate the billing tables.");
        neighbour.plan(&[("Migrate the billing tables", "in_progress")]);
        elsewhere.person("Elsewhere: translate the help pages.");
        elsewhere.plan(&[("Translate the help pages", "pending")]);
        mine.person("Mine: tidy the release notes.");
        mine.plan(&[("Tidy the release notes", "in_progress")]);

        let said = mine
            .turn(&pointer(3), |_| None, false)
            .reminder
            .expect("the mail line's turn names this session's own work");

        assert!(said.contains("\"Mine: tidy the release notes.\""), "{said}");
        assert!(said.contains("Tidy the release notes (in_progress)"), "{said}");
        for foreign in ["Neighbour", "billing", "Elsewhere", "help pages"] {
            assert!(!said.contains(foreign), "{foreign} reached this session: {said}");
        }
    }

    /// A session opened here before the person wrote: the window's mail line
    /// resumes nothing, as a reopened session's loops come back paused.
    #[test]
    fn a_mail_line_before_the_person_has_written_here_resumes_nothing() {
        let _env = crate::test_env_lock();
        let stores = Stores::new();
        let mut session = Synthetic::open(&stores.project("work"), "session-f");
        session.plan(&[("Write the hand-in", "pending")]);

        let said = session
            .turn(&pointer(1), |_| None, false)
            .reminder
            .expect("the mail line's turn says nothing earlier is resumed from it");

        assert!(said.contains("has not written since this session was opened here"), "{said}");
        assert!(!said.contains("continue it in this same turn"), "{said}");
    }

    /// A background helper's result that runs as a turn is the host's line,
    /// not the person's: it is not quoted as their words.
    #[test]
    fn a_helpers_result_run_as_a_turn_is_not_quoted_as_the_persons_words() {
        let _env = crate::test_env_lock();
        let stores = Stores::new();
        let mut session = Synthetic::open(&stores.project("work"), "session-g");
        session.person("Profile the importer.");
        session.plan(&[("Profile the importer", "in_progress")]);
        session.person(
            "[task notification — background agent `profiler` (id: agent-3) finished. To follow \
             up without losing its context, use SendMessage with that id/name as `to` to continue \
             this agent]\n\nThe importer spends its time parsing dates.",
        );

        let said = session
            .turn(&pointer(1), |_| None, false)
            .reminder
            .expect("the mail line's turn names the person's own words");

        assert!(said.contains("\"Profile the importer.\""), "{said}");
        assert!(!said.contains("task notification"), "{said}");
        assert!(!said.contains("parsing dates"), "{said}");
    }
}
