//! The guards with the judgment provider cut off (t-88166): an address nothing
//! listens on, and a server that takes the request and never answers. An
//! acting guard goes on by the product's own rule, at the cost of its wall and
//! no more, and what the permission rule decides about a command is what it
//! decides with the provider up: it still asks, and it still refuses.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use runtime::tool_guard::text_ask;
use runtime::{
    ApiRequest, AssistantEvent, ContentBlock, ConversationMessage, ConversationRuntime, PermissionMode, PermissionPolicy,
    PermissionPromptDecision, PermissionPrompter, PermissionRequest, RuntimeError, RuntimePermissionRuleConfig, Session,
    StaticToolExecutor,
};
use zerocode_core::jev::{JevMode, ROUTE_USE_FALLBACK};

use super::super::jev_mock::{machine, Mock};
use super::tests::{rows_of, ORDER};
use super::*;

/// An address nothing listens on: the port was free a moment ago.
fn closed_address() -> String {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a free port");
    let port = listener.local_addr().expect("its address").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

/// Run `body` before each way a provider is cut off.
fn on_each_cut_wire(mut body: impl FnMut(&'static str, &str)) {
    body("closed", &closed_address());
    let silent = Mock::silent();
    body("silent", &silent.base_url);
}

const DESTROYS: &str = "rm -rf build";

fn destroying_ask(cwd: &Path, id: &str) -> CommandAsk {
    CommandAsk {
        attempt: "turn-1".to_string(),
        owner: "turn-1".to_string(),
        tool_use_id: id.to_string(),
        command: DESTROYS.to_string(),
        cwd: cwd.to_path_buf(),
        task: "clean the build folder".to_string(),
    }
}

/// The command guard, acting: no line, the row says the call fell back and
/// keeps what today's rule read of the command.
#[test]
fn an_acting_command_guard_on_a_cut_wire_decides_by_the_rule_within_its_wall() {
    on_each_cut_wire(|wire, base_url| {
        machine(&COMMAND_GUARD, JevMode::On.key(), base_url, |cwd| {
            forget_waiting(cwd);
            let judge = ToolGuardJudge::at(cwd);
            let started = Instant::now();
            judge.command(destroying_ask(cwd, "shell-1"));
            let ran = CommandRan {
                owner: "turn-1".to_string(),
                tool_use_id: "shell-1".to_string(),
                failed: false,
                cancelled: false,
            };
            assert_eq!(api::sync_bridge::run_blocking(judge.command_ran(ran)), None, "{wire}");
            let took = started.elapsed();
            println!("command_guard {wire}: {} ms", took.as_millis());
            assert!(took < COMMAND.deadline * 2, "{wire}: {took:?}");
            let row: CommandGuardRow =
                serde_json::from_value(rows_of(&command_guard_path(), 1)[0].clone()).expect("a command row");
            assert_eq!(row.verdict, Verdict::Unavailable.word(), "{wire}");
            assert_eq!(row.rule, Verdict::Flagged.word(), "{wire}: today's rule reads this command as flagged");
            assert_eq!(
                (row.asked.route_use.as_str(), row.asked.applied, row.noted),
                (ROUTE_USE_FALLBACK, false, false),
                "{wire}"
            );
        });
    });
}

/// The text guard, acting: it waits on the wire, and hands the block back
/// unfenced and unmarked at its wall.
#[test]
fn an_acting_text_guard_on_a_cut_wire_hands_the_text_back_within_its_wall() {
    on_each_cut_wire(|wire, base_url| {
        machine(&TOOL_TEXT_GUARD, JevMode::On.key(), base_url, |cwd| {
            forget_waiting(cwd);
            let judge = ToolGuardJudge::at(cwd);
            let ask = text_ask("turn-1", "read-1", "read_file", ORDER).expect("a file is read");
            let started = Instant::now();
            let guard = api::sync_bridge::run_blocking(judge.text(ask));
            let took = started.elapsed();
            println!("tool_text_guard {wire}: {} ms", took.as_millis());
            assert_eq!(guard, TextGuard::default(), "{wire}");
            assert!(took < TEXT.deadline * 2, "{wire}: {took:?}");
            let row: ToolTextGuardRow =
                serde_json::from_value(rows_of(&tool_text_guard_path(), 1)[0].clone()).expect("a text row");
            assert_eq!(row.verdict, Verdict::Unavailable.word(), "{wire}");
            assert_eq!(
                (row.asked.route_use.as_str(), row.asked.applied, row.fenced, row.noted),
                (ROUTE_USE_FALLBACK, false, false, false),
                "{wire}"
            );
        });
    });
}

/* ---- what the permission rule decides ---------------------------------------- */

const SHELL_OUTPUT: &str = r#"{"stdout":"removed","stderr":"","interrupted":false}"#;

struct OneCommand {
    calls: usize,
}

impl runtime::ApiClient for OneCommand {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        self.calls += 1;
        Ok(if self.calls == 1 {
            vec![
                AssistantEvent::ToolUse {
                    id: "shell-1".to_string(),
                    name: SHELL_TOOL.to_string(),
                    input: serde_json::json!({ "command": DESTROYS }).to_string(),
                },
                AssistantEvent::MessageStop,
            ]
        } else {
            vec![AssistantEvent::TextDelta("done".to_string()), AssistantEvent::MessageStop]
        })
    }
}

/// A person at the prompt: answers `allow`, and counts what they were asked.
struct AtThePrompt {
    allow: bool,
    asked: Arc<Mutex<Vec<String>>>,
}

impl PermissionPrompter for AtThePrompt {
    fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
        self.asked.lock().expect("asked").push(request.tool_name.clone());
        if self.allow {
            PermissionPromptDecision::Allow
        } else {
            PermissionPromptDecision::Deny { reason: "the person said no".to_string() }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Decided {
    prompted: usize,
    ran: usize,
    output: String,
}

/// One command through the runtime under `policy`, with `seat` installed:
/// whether the person was asked, whether it ran, what the model read.
fn decided(policy: PermissionPolicy, seat: Option<Arc<dyn runtime::ToolGuardSeat>>, allow: bool) -> Decided {
    let ran = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ran);
    let mut runtime = ConversationRuntime::new(
        Session::new(),
        OneCommand { calls: 0 },
        StaticToolExecutor::new().register(SHELL_TOOL, move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(SHELL_OUTPUT.to_string())
        }),
        policy,
        vec!["system".to_string()],
    );
    runtime.set_tool_guard_seat(seat);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut prompter = AtThePrompt { allow, asked: Arc::clone(&asked) };
    runtime.run_turn("clean the build folder", Some(&mut prompter)).expect("the turn");
    let output = tool_outputs(&runtime.session().messages);
    let times_asked = asked.lock().expect("asked").len();
    Decided { prompted: times_asked, ran: ran.load(Ordering::SeqCst), output }
}

fn tool_outputs(messages: &[ConversationMessage]) -> String {
    messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult { output, .. } => Some(output.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn asking_policy() -> PermissionPolicy {
    PermissionPolicy::new(PermissionMode::Prompt).with_tool_requirement(SHELL_TOOL, PermissionMode::DangerFullAccess)
}

fn refusing_policy() -> PermissionPolicy {
    PermissionPolicy::new(PermissionMode::DangerFullAccess).with_permission_rules(&RuntimePermissionRuleConfig::new(
        Vec::new(),
        vec!["bash(rm *)".to_string()],
        Vec::new(),
    ))
}

/// What the rule decides with no guard at all is what it decides with an
/// acting guard whose provider is cut off, and the turn pays its wall at most.
#[test]
fn the_permission_rule_still_asks_and_refuses_with_the_provider_cut_off() {
    let (asked_and_allowed, asked_and_denied, refused) = machine(&COMMAND_GUARD, JevMode::Off.key(), &closed_address(), |_| {
        (decided(asking_policy(), None, true), decided(asking_policy(), None, false), decided(refusing_policy(), None, true))
    });
    assert_eq!(
        (asked_and_allowed.prompted, asked_and_allowed.ran, asked_and_allowed.output.as_str()),
        (1, 1, SHELL_OUTPUT),
        "the rule asks, and an allowed command runs"
    );
    assert_eq!((asked_and_denied.prompted, asked_and_denied.ran), (1, 0), "a denied command does not run");
    assert_eq!((refused.prompted, refused.ran), (0, 0), "a deny rule refuses without asking");

    on_each_cut_wire(|wire, base_url| {
        machine(&COMMAND_GUARD, JevMode::On.key(), base_url, |cwd| {
            forget_waiting(cwd);
            let seat = || Some(Arc::new(ToolGuardJudge::at(cwd)) as Arc<dyn runtime::ToolGuardSeat>);
            for (name, policy, allow, want) in [
                ("asked, allowed", asking_policy as fn() -> PermissionPolicy, true, &asked_and_allowed),
                ("asked, denied", asking_policy, false, &asked_and_denied),
                ("refused", refusing_policy, true, &refused),
            ] {
                let started = Instant::now();
                let got = decided(policy(), seat(), allow);
                let took = started.elapsed();
                println!("permission rule {wire} {name}: {} ms", took.as_millis());
                assert_eq!(&got, want, "{wire}: {name}");
                assert!(took < COMMAND.deadline * 2, "{wire}: {name}: {took:?}");
            }
        });
    });
}
