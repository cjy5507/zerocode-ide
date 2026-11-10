//! Host-level proof that registered graphs and idle dreaming execute without
//! asking a parent model to decide whether to invoke a tool.
#![cfg(unix)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mock_anthropic_service::MockAnthropicService;
use runtime::message_stream::RenderBlock;
use runtime::{HookAbortSignal, PermissionMode};
use serde_json::{json, Value};

use crate::autonomy::runner::{BlockedReason, TurnReceipt};
use super::plain_session::{OpenOptions, PlainSession, TeammateHarness};

const CHECK: &str = "printf 'checked\\n' >> automatic-witness.txt";

struct World {
    root: tempfile::TempDir,
    cwd: PathBuf,
    store: PathBuf,
    previous_cwd: PathBuf,
    previous_env: Vec<(OsString, Option<OsString>)>,
}

impl World {
    fn new(base_url: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = |name: &str| {
            let path = root.path().join(name);
            fs::create_dir(&path).unwrap();
            fs::canonicalize(path).unwrap()
        };
        let cwd = directory("workspace");
        let home = directory("home");
        let state = directory("state");
        let store = directory("workflows");
        let sessions = directory("sessions");
        fs::write(home.join("settings.json"), r#"{"autoDreamEnabled":true,"smart":{"autoClassifier":"off","orchestration":"model"}}"#).unwrap();
        let values: Vec<(&str, OsString)> = vec![
            ("HOME", home.clone().into_os_string()),
            ("USERPROFILE", home.clone().into_os_string()),
            ("ZO_CONFIG_HOME", home.clone().into_os_string()),
            ("CLAUDE_CONFIG_DIR", home.join("claude").into_os_string()),
            ("CODEX_HOME", home.join("codex").into_os_string()),
            ("ZO_CODEX_HOME", home.join("codex").into_os_string()),
            ("ZO_STATE_DIR", state.clone().into_os_string()),
            ("ZO_AGENT_STORE", state.join("agents").into_os_string()),
            ("ZO_TODO_STORE", state.join("todos.json").into_os_string()),
            ("ZO_SESSION_ROOT", sessions.into_os_string()),
            ("ZO_WORKFLOW_STORE", store.clone().into_os_string()),
            ("ZO_TEAM_INBOX_STORE", state.join("inbox").into_os_string()),
            (tools::ARTIFACT_STORE_ENV, state.join("artifacts").into_os_string()),
            ("ANTHROPIC_BASE_URL", base_url.into()),
            ("ANTHROPIC_API_KEY", "test-dummy-key".into()),
            ("ZO_DISABLE_KEYCHAIN", "1".into()),
            ("ZO_DISABLE_EXTERNAL_CREDENTIALS", "1".into()),
            ("ZO_DISABLE_MODEL_DISCOVERY", "1".into()),
            ("ZO_TOOL_DIGEST", "off".into()),
            ("ZO_DREAM", "1".into()),
            ("ZO_AUTONOMY_IDLE_DREAM_DELAY_SECS", "1".into()),
            ("ZO_BASH_TIMEOUT_MS", "5000".into()),
        ];
        let mut keys: BTreeSet<OsString> = std::env::vars_os().map(|(key, _)| key)
            .filter(|key| {
                let key = key.to_string_lossy();
                key.starts_with("ZEROCODE_") || key.starts_with("ZO_EVENTS_")
                    || key.starts_with("ZO_SERVE_") || key.starts_with("ZO_MODEL_")
                    || key.starts_with("ZO_LAUNCH_")
            }).collect();
        keys.extend(values.iter().map(|(key, _)| OsString::from(key)));
        keys.extend([
            "ZO_HOME", "ANTHROPIC_AUTH_TOKEN", "OPENAI_API_KEY", "XAI_API_KEY", "GOOGLE_API_KEY",
            "GEMINI_API_KEY", "ZO_CUSTOM_PROVIDERS", "ZO_AUTO_VERIFY", "ZO_AUTO_VERIFY_CMD",
            "ZO_SUBAGENT_MODE", "TMUX", "TMUX_PANE",
        ].into_iter().map(OsString::from));
        let previous_env = keys.iter().map(|key| (key.clone(), std::env::var_os(key))).collect();
        let world = Self { root, cwd, store, previous_cwd:std::env::current_dir().unwrap(), previous_env };
        for key in keys { std::env::remove_var(key); }
        for (key, value) in values { std::env::set_var(key, value); }
        std::env::set_current_dir(&world.cwd).unwrap();
        api::managed_account::clear();
        api::invalidate_claude_code_keychain_cache();
        crate::runtime_support::forget_cached_claude_auth();
        world
    }

    fn open(&self, mode: PermissionMode) -> PlainSession {
        PlainSession::open(OpenOptions {
            person_model_pin: Some("claude-sonnet-4-6".to_string()),
            cwd:self.cwd.clone(), model:"claude-sonnet-4-6".to_string(),
            permission_mode:mode, named_permission_mode:Some(mode), allowed_tools:None,
            disable_spawn_family:false, resume:None, mcp_config:None,
            effort:Some(crate::effort::Effort::Off), headless:true, exact_selection:true,
            teammate_harness:Some(TeammateHarness {
                system_prompt:vec!["A hermetic automatic-work test.".to_string()],
                inspection_shell:false, permission_rules:None,
            }),
            parent_registry:None, remote_mcp:None,
        }).expect("open a real session without contacting a provider")
    }

    fn witness(&self) -> String {
        fs::read_to_string(self.cwd.join("automatic-witness.txt")).unwrap_or_default()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous_cwd);
        for (key, value) in &self.previous_env {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        crate::runtime_support::forget_cached_claude_auth();
        api::managed_account::clear();
    }
}

#[derive(Default)]
struct NoPrompt(AtomicUsize);

impl runtime::permission::PermissionPrompter for NoPrompt {
    fn decide<'a>(&'a self, _: runtime::permission::PermissionRequest) -> std::pin::Pin<Box<
        dyn std::future::Future<Output = Result<runtime::permission::PermissionDecision, runtime::permission::PermissionError>> + Send + 'a
    >> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(runtime::permission::PermissionDecision::Deny) })
    }
}

fn executor() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
}

/// Opening a real session marks this process as a session host permanently.
/// Run the assertion body in its own test process so that marker, provider
/// caches, and startup workers cannot affect unrelated library tests.
fn isolated_process(name: &str) -> bool {
    const CHILD: &str = "ZO_AUTOMATIC_WORK_TEST";
    if std::env::var(CHILD).ok().as_deref() == Some(name) { return false; }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("session::automatic_work_tests::{name}"), "--nocapture", "--test-threads=1"])
        .env(CHILD, name).output().expect("start isolated host integration test");
    assert!(output.status.success(), "isolated host test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    true
}

fn graph() -> Value {
    json!({"spec":{
        "name":"automatic-check", "mode":"graph",
        "graph":{"entry":"check", "max_steps":3, "nodes":[
            {"kind":"check", "id":"check", "command":CHECK, "on_pass":"learn", "on_fail":"failed"},
            {"kind":"dream", "id":"learn", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"},
            {"kind":"end", "id":"failed", "status":"failed"}
        ]}
    }, "input":null})
}

fn registry(session: &PlainSession) -> runtime::team_cron_registry::CronRegistry {
    session.runtime.tool_executor().tool_registry().context().crons.clone()
}

fn register_due(session: &PlainSession) -> runtime::team_cron_registry::CronEntry {
    let minute = crate::autonomy::scheduler::now_unix_ms() / 60_000 * 60;
    registry(session).create_workflow_at("* * * * *", &graph(), None, minute).unwrap()
}

async fn execute_scheduled(session: &mut PlainSession, prompt: &str) -> Result<runtime::TurnSummary, String> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
    let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    let prompter = Arc::new(NoPrompt::default());
    let result = tokio::time::timeout(Duration::from_secs(10), session.run_turn(
        prompt, sender, prompter.clone(), HookAbortSignal::new(), Arc::new(AtomicBool::new(false)),
    )).await.expect("scheduled work must finish without a model turn");
    drain.await.unwrap();
    assert_eq!(prompter.0.load(Ordering::SeqCst), 0, "the unattended host must not ask");
    result
}

fn receipt(summary: &runtime::TurnSummary, blocked: Option<BlockedReason>) -> TurnReceipt {
    TurnReceipt {
        workspace_before: "test-snapshot".to_string(),
        workspace_after: "test-snapshot".to_string(),
        gate: None,
        output_tokens: u64::from(summary.turn_output_tokens),
        active_millis: 1,
        blocked,
        todo_completed_delta: 0,
        loop_schedule: None,
    }
}

#[test]
fn real_session_dispatches_registered_graph_without_a_parent_provider_request() {
    if isolated_process("real_session_dispatches_registered_graph_without_a_parent_provider_request") { return; }
    let _lock = crate::test_env_lock();
    let executor = executor();
    let provider = executor.block_on(MockAnthropicService::spawn()).unwrap();
    let world = World::new(&provider.base_url());
    let mut session = world.open(PermissionMode::WorkspaceWrite);
    let entry = register_due(&session);
    let turn = session.dispatch_cron_turn().unwrap().expect("due graph becomes pending host work");
    let run_id = turn.workflow.as_ref().unwrap()["runId"].as_str().unwrap().to_string();
    let summary = executor.block_on(execute_scheduled(&mut session, &turn.prompt)).unwrap();
    assert_eq!(summary.iterations, 0, "no parent model iteration");
    assert_eq!(summary.turn_output_tokens, 0, "check-only graph spends no model tokens");
    assert!(summary.verification_issues.is_empty());
    assert_eq!(world.witness(), "checked\n");
    let checkpoint: Value = serde_json::from_str(&fs::read_to_string(world.store.join(format!("{run_id}.graph.json"))).unwrap()).unwrap();
    assert_eq!(checkpoint["session_id"], session.handle.id);
    assert_eq!(checkpoint["terminal"], "completed");
    assert_eq!(registry(&session).get(&entry.cron_id).unwrap().run_count, 1);
    assert_eq!(registry(&session).get(&entry.cron_id).unwrap().workflow_active_at, Some(turn.due_at_secs));
    assert!(registry(&session).due_at(turn.due_at_secs + 60).is_empty(),
        "a finished worker still owns the slot until its host records the receipt");
    let notice = session.record_cron_turn(&turn, &receipt(&summary, None)).unwrap();
    assert!(notice.contains("graph completed"), "{notice}");
    assert_eq!(registry(&session).get(&entry.cron_id).unwrap().workflow_active_at, None);
    // Ask at the same recorded instant so a wall-clock minute change cannot
    // turn the idempotency assertion into the next legitimate run.
    assert!(crate::autonomy::crons::dispatch_due(&registry(&session), turn.due_at_secs * 1000).is_none());
    let next = crate::autonomy::crons::dispatch_due(&registry(&session), (turn.due_at_secs + 60) * 1000).unwrap();
    assert_ne!(next.workflow.as_ref().unwrap()["runId"], run_id);
    assert_eq!(world.witness(), "checked\n", "dispatch alone does not run a graph");
    assert!(executor.block_on(provider.captured_requests()).is_empty());
}

#[test]
fn scheduled_graph_keeps_prompt_mode_ask_rules_without_calling_a_model() {
    if isolated_process("scheduled_graph_keeps_prompt_mode_ask_rules_without_calling_a_model") { return; }
    let _lock = crate::test_env_lock();
    let executor = executor();
    let provider = executor.block_on(MockAnthropicService::spawn()).unwrap();
    let world = World::new(&provider.base_url());
    let mut session = world.open(PermissionMode::Prompt);
    session.runtime.add_permission_rules(&runtime::RuntimePermissionRuleConfig::new(
        Vec::new(), Vec::new(), vec!["Workflow".to_string()],
    ));
    let entry = register_due(&session);
    let turn = session.dispatch_cron_turn().unwrap().unwrap();
    let summary = executor.block_on(execute_scheduled(&mut session, &turn.prompt)).unwrap();
    assert!(!summary.verification_issues.is_empty(), "permission refusal is a known blocked receipt");
    let notice = session.record_cron_turn(&turn, &receipt(&summary, Some(BlockedReason::Permission))).unwrap();
    assert!(notice.contains("graph blocked"), "{notice}");
    let held = registry(&session).get(&entry.cron_id).unwrap();
    assert!(!held.enabled, "blocked graph schedule is quarantined");
    assert_eq!(held.workflow_active_at, Some(turn.due_at_secs));
    assert!(held.workflow_blocked_reason.is_some());
    assert!(world.witness().is_empty());
    assert!(executor.block_on(provider.captured_requests()).is_empty());
}

#[test]
fn unknown_scheduled_outcome_blocks_future_automation_after_reload() {
    if isolated_process("unknown_scheduled_outcome_blocks_future_automation_after_reload") { return; }
    let _lock = crate::test_env_lock();
    let executor = executor();
    let provider = executor.block_on(MockAnthropicService::spawn()).unwrap();
    let world = World::new(&provider.base_url());
    let mut session = world.open(PermissionMode::WorkspaceWrite);
    let entry = register_due(&session);
    let turn = session.dispatch_cron_turn().unwrap().unwrap();
    let (sender, receiver) = tokio::sync::mpsc::channel::<RenderBlock>(1);
    drop(receiver);
    let result = executor.block_on(session.run_turn(
        &turn.prompt, sender, Arc::new(NoPrompt::default()), HookAbortSignal::new(), Arc::new(AtomicBool::new(false)),
    ));
    assert!(result.is_err());
    assert!(world.witness().is_empty());
    let cron_registry = registry(&session);
    let reopened = runtime::team_cron_registry::CronRegistry::with_persistence_path(
        cron_registry.persistence_path().map(std::path::Path::to_path_buf),
    );
    assert_eq!(reopened.get(&entry.cron_id).unwrap().workflow_active_at, Some(turn.due_at_secs),
        "without a completion receipt the shared slot remains active");
    assert!(reopened.due_at(turn.due_at_secs + 60).is_empty());
    let restored = crate::autonomy::Autonomy::load(&session.handle.path, None, true);
    assert!(restored.status().budget.blocked.as_deref().is_some_and(|reason| reason.contains("unknown")));
    register_due(&session);
    assert!(session.dispatch_cron_turn().unwrap_err().contains("unknown"));
    assert!(executor.block_on(provider.captured_requests()).is_empty());
}

#[test]
fn real_session_idle_clock_promotes_existing_evidence_without_a_model_turn() {
    if isolated_process("real_session_idle_clock_promotes_existing_evidence_without_a_model_turn") { return; }
    let _lock = crate::test_env_lock();
    let executor = executor();
    let provider = executor.block_on(MockAnthropicService::spawn()).unwrap();
    let world = World::new(&provider.base_url());
    let session = world.open(PermissionMode::WorkspaceWrite);
    for source in ["verified-one", "verified-two"] {
        assert!(runtime::memory::record_verified_check(&world.cwd, source, Some("cargo test -p acme")));
    }
    let due = session.next_autonomy_wakeup().expect("idle dreaming participates in the host clock");
    let now = crate::autonomy::scheduler::now_unix_ms();
    assert!(due <= now + 1000, "test config uses a one-second idle delay");
    executor.block_on(async {
        tokio::time::sleep(Duration::from_millis(due.saturating_sub(now))).await;
        session.dream_when_idle();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let memory = runtime::memory::paths::memory_write_dir(&world.cwd, true);
                if memory.join("MEMORY.md").is_file()
                    && session.next_autonomy_wakeup().is_some_and(|next| next > crate::autonomy::scheduler::now_unix_ms() + 60_000) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("idle pass must write promoted memory");
    });
    assert!(executor.block_on(provider.captured_requests()).is_empty());
    // Keep the whole fixture alive through the detached pass's completed mark.
    assert!(world.root.path().exists());
}

fn session_with_recorded_edit(world: &World) -> (PlainSession, PathBuf) {
    use core_types::ContentBlock;
    use core_types::session::ConversationMessage;
    let mut session = world.open(PermissionMode::DangerFullAccess);
    let file = world.cwd.join("recovery.txt");
    fs::write(&file, "before").unwrap();
    {
        let rt = session.runtime.try_runtime_mut().unwrap();
        rt.session_mut().push_message(ConversationMessage::user_text("change the file")).unwrap();
        let context = rt.tool_executor().tool_registry().context();
        let checkpoint_dir = world.cwd.join("recovery-checkpoints");
        fs::create_dir_all(&checkpoint_dir).unwrap();
        let checkpoint = tools::WorkspaceCheckpoint {
            turn_index: 1, message_index: Some(0), message_end_index: Some(2),
            files: vec![tools::WorkspaceCheckpointFile {
                path: file.clone(),
                before: tools::WorkspaceFileSnapshot { content: Some(b"before".to_vec()), skipped: None },
                after: tools::WorkspaceFileSnapshot { content: Some(b"after".to_vec()), skipped: None },
            }], ..Default::default()
        };
        fs::write(checkpoint_dir.join("checkpoint-00000000000000000001.json"), serde_json::to_vec(&checkpoint).unwrap()).unwrap();
        fs::write(&file, "after").unwrap();
        context.reset_workspace_checkpoint_session(Some(checkpoint_dir)).unwrap();
        rt.session_mut().push_message(ConversationMessage::assistant(vec![ContentBlock::Text { text: "changed".into() }])).unwrap();
    }
    session.persist().unwrap();
    (session, file)
}

#[test]
fn conversation_recovery_uses_live_revision_and_guarded_files() {
    use core_types::Session;
    use core_types::session::ConversationMessage;
    use zerocode_core::conversation_control::{Action, RestoreScope};
    let _lock = crate::test_env_lock();
    let world = World::new("http://127.0.0.1:1");
    let (mut session, file) = session_with_recorded_edit(&world);
    let id = session.handle.id.clone();
    let preview = session.conversation_action(&id, &Action::Preview { message_index: 0 }).unwrap().response;
    assert_eq!(preview["files_available"], true);
    let revision = preview["revision"].as_str().unwrap().to_string();
    let fork = session.conversation_action(&id, &Action::Fork { message_index: 0, revision: revision.clone() }).unwrap();
    let child = Session::load_from_path(fork.fork.unwrap()).unwrap();
    assert_eq!(child.fork.unwrap().parent_session_id, id);
    assert_eq!(session.handle.id, id);
    assert_eq!(fs::read_to_string(&file).unwrap(), "after");
    let restore = Action::Restore { message_index: 0, revision: revision.clone(), scope: RestoreScope::Both };
    fs::write(&file, "person's edit").unwrap();
    assert!(session.conversation_action(&id, &restore).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "person's edit");
    assert_eq!(session.runtime.session().messages.len(), 2);
    fs::write(&file, "after").unwrap();
    session.permission_mode = PermissionMode::ReadOnly;
    assert!(session.conversation_action(&id, &restore).is_err());
    session.permission_mode = PermissionMode::DangerFullAccess;
    assert!(session.conversation_action("another-session", &restore).is_err());
    let restored = session.conversation_action(&id, &restore).unwrap();
    assert_eq!(restored.response["kind"], "restored");
    assert!(restored.replay);
    assert_eq!(fs::read_to_string(&file).unwrap(), "before");
    let persisted = Session::load_from_path(&session.handle.path).unwrap();
    assert!(!persisted.messages.iter().any(ConversationMessage::is_user_prompt));
    assert!(session.conversation_action(&id, &restore).is_err(), "the accepted revision cannot execute twice");
}

#[test]
fn conversation_recovery_keeps_the_unselected_scope_and_retires_old_anchors() {
    use core_types::session::ConversationMessage;
    use zerocode_core::conversation_control::{Action, RestoreScope};
    let _lock = crate::test_env_lock();
    for scope in [RestoreScope::Conversation, RestoreScope::Files] {
        let world = World::new("http://127.0.0.1:1");
        let (mut session, file) = session_with_recorded_edit(&world);
        let id = session.handle.id.clone();
        let preview = session.conversation_action(&id, &Action::Preview { message_index: 0 }).unwrap().response;
        let action = Action::Restore { message_index: 0, revision: preview["revision"].as_str().unwrap().into(), scope };
        let result = session.conversation_action(&id, &action).unwrap();
        assert_eq!(result.response["kind"], "restored");
        assert_eq!(fs::read_to_string(file).unwrap(), if scope == RestoreScope::Files { "before" } else { "after" });
        let saved = core_types::Session::load_from_path(&session.handle.path).unwrap();
        assert_eq!(saved.messages.iter().any(ConversationMessage::is_user_prompt), scope == RestoreScope::Files);
        assert!(session.conversation_action(&id, &action).is_err(), "the old preview cannot be replayed");
        if scope == RestoreScope::Conversation {
            let retired: tools::WorkspaceCheckpoint = serde_json::from_slice(&fs::read(world.cwd.join("recovery-checkpoints/checkpoint-00000000000000000001.json")).unwrap()).unwrap();
            assert_eq!(retired.message_index, None, "retired anchors stay retired after a restart");
            let rt = session.runtime.try_runtime_mut().unwrap();
            rt.session_mut().push_message(ConversationMessage::user_text("a new prompt at the reused index")).unwrap();
            assert!(rt.tool_executor().tool_registry().context().workspace_checkpoints().iter().all(|checkpoint| checkpoint.message_index.is_none()));
            let preview = session.conversation_action(&id, &Action::Preview { message_index: 0 }).unwrap().response;
            assert_eq!(preview["files_available"], false, "discarded history must not be mistaken for this new prompt's edits");
        }
    }
}

#[test]
fn conversation_resume_first_paint_is_a_recent_page() {
    use core_types::session::ConversationMessage;
    use super::plain_session::ReplayItem;
    let _lock = crate::test_env_lock();
    let world = World::new("http://127.0.0.1:1");
    let mut session = world.open(PermissionMode::DangerFullAccess);
    for index in 0..300 {
        session.runtime.try_runtime_mut().unwrap().session_mut()
            .push_message(ConversationMessage::user_text(format!("request {index}"))).unwrap();
    }
    // Nothing to cut at: the page comes as one part.
    let (pages, earlier) = session.recent_replay_pages(&[]);
    assert!(earlier);
    assert_eq!(pages.len(), 1);
    let page = &pages[0].0;
    assert_eq!(page.len(), 128);
    assert_eq!(page.first(), Some(&ReplayItem::User("request 172".into())));
    assert_eq!(page.last(), Some(&ReplayItem::User("request 299".into())));
    assert_eq!(session.runtime.session().messages.len(), 300, "paging never drops saved history");
}

#[test]
fn conversation_recovery_does_not_split_a_steered_file_checkpoint() {
    use core_types::session::ConversationMessage;
    use zerocode_core::conversation_control::{Action, RestoreScope};
    let _lock = crate::test_env_lock();
    let world = World::new("http://127.0.0.1:1");
    let (mut session, file) = session_with_recorded_edit(&world);
    let id = session.handle.id.clone();
    let folder = world.cwd.join("recovery-checkpoints");
    let path = folder.join("checkpoint-00000000000000000001.json");
    let mut checkpoint: tools::WorkspaceCheckpoint = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    checkpoint.message_end_index = Some(3);
    fs::write(&path, serde_json::to_vec(&checkpoint).unwrap()).unwrap();
    let rt = session.runtime.try_runtime_mut().unwrap();
    rt.session_mut().push_message(ConversationMessage::user_text("a steer inside the same model turn")).unwrap();
    rt.tool_executor().tool_registry().context().reset_workspace_checkpoint_session(Some(folder)).unwrap();
    let preview = session.conversation_action(&id, &Action::Preview { message_index: 2 }).unwrap().response;
    assert_eq!(preview["conversation_available"], true);
    assert_eq!(preview["files_available"], false);
    let revision = preview["revision"].as_str().unwrap().to_string();
    assert!(session.conversation_action(&id, &Action::Restore { message_index: 2, revision: revision.clone(), scope: RestoreScope::Files }).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "after");
    let outcome = session.conversation_action(&id, &Action::Restore { message_index: 2, revision, scope: RestoreScope::Conversation }).unwrap();
    assert_eq!(outcome.response["kind"], "restored");
    assert_eq!(session.runtime.session().messages.len(), 2);
    assert_eq!(fs::read_to_string(file).unwrap(), "after");
    let retired: tools::WorkspaceCheckpoint = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!((retired.message_index, retired.message_end_index), (None, None));
}

#[test]
fn conversation_legacy_rewind_retires_the_same_message_anchors() {
    use core_types::session::ConversationMessage;
    use super::plain_session::RewindTurn;
    use zerocode_core::conversation_control::Action;
    let _lock = crate::test_env_lock();
    let world = World::new("http://127.0.0.1:1");
    let (mut session, file) = session_with_recorded_edit(&world);
    assert!(matches!(session.rewind_last_turn().unwrap(), RewindTurn::Removed(2)));
    session.runtime.try_runtime_mut().unwrap().session_mut()
        .push_message(ConversationMessage::user_text("new request at the old index")).unwrap();
    let id = session.handle.id.clone();
    let preview = session.conversation_action(&id, &Action::Preview { message_index: 0 }).unwrap().response;
    assert_eq!(preview["files_available"], false);
    assert_eq!(fs::read_to_string(file).unwrap(), "after", "the legacy conversation rewind keeps files");
}
