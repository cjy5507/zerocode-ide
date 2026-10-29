//! ZeroCode's own window as a Computer Use target — the window's half
//! (t-73906; the rule is core `computer_use_protocol::self_surface`).
//!
//! A request whose `--app` names this process never reaches the helper: the
//! page answers it. A look asks the page for its controls and the facts each
//! one carries; an act asks the page for one control's facts again, judges
//! them here, and only then asks the page to press it in its own document.
//! The page makes no OS event, so the person's pointer, focus and keys stay
//! theirs while an agent works in the same window.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::computer_use::ComputerMethod;
use zerocode_core::computer_use_protocol::render::{RenderNode, line, render_tree_text};
use zerocode_core::computer_use_protocol::self_surface::{
    Act, Facts, METHODS, OwnApp, judge, shown_value, unsupported,
};
use zerocode_core::computer_use_protocol::{AppIdentity, error_code};

use super::ComputerUseError;

/// The window's way of asking its page: one request, one answer.
pub(crate) type Page = dyn Fn(&Value) -> Result<Value, ComputerUseError> + Send + Sync;

static PAGE: RwLock<Option<(OwnApp, Arc<Page>)>> = RwLock::new(None);

/// The last look the page answered: which of its controls each index names.
/// An act presses only a control a look showed.
static LAST_LOOK: Mutex<Option<Look>> = Mutex::new(None);

#[derive(Clone)]
struct Look {
    id: Value,
    controls: Vec<Shown>,
}

/// One control as a look showed it: the page's id, and what it was.
#[derive(Clone)]
struct Shown {
    id: Value,
    role: String,
    name: String,
}

/// The window's commands only the person may run: an answer to a question,
/// Computer Use's own policy and stop, the coordinator's seat and landings,
/// a merge, an update and a restart, a credential written or dropped, an
/// account changed. Each runs only when the page says the person's own hand
/// made the call ([`HAND_HEADER`]) — the second wall behind the page's marks,
/// asked once for every command in the window's invoke handler
/// ([`person_only_gate`]): a press the marks missed still runs none of these,
/// whichever crate defines it, and no clock holds the person's own hand.
pub(crate) const PERSON_ONLY_COMMANDS: &[&str] = &[
    "answer_approval",
    "answer_ask",
    "answer_secret",
    "respond_permission",
    "wire_answer",
    "computer_confirm_answer",
    "computer_handoff_code",
    "set_computer_input_policy",
    "set_computer_share_required",
    "set_computer_live_reflex",
    "computer_resume",
    "claim_coordinator_seat",
    "set_coordinator_handover",
    "quota_wall_handover",
    "land_batch_promote",
    "land_batch_bisect",
    "land_batch_cancel",
    "land_batch_restack",
    // A letter acknowledged on the person's behalf; the desk and the inbox's marks are the first wall (t-74825).
    "desk_ack",
    "merge_and_remove_worktree",
    "github_merge_pr",
    "gitlab_merge_mr",
    "update_download",
    "update_install",
    // The person's hand on a window shared with an agent.
    "computer_take_control",
    "computer_give_back",
    "computer_share_end",
    "computer_share_hand_over",
    // The two ways out the page offers; both reach `relaunch_window`.
    "leave_now",
    "leave_when_idle",
    // A credential written, checked or dropped.
    "save_api_router",
    "remove_api_router",
    "adopt_legacy_judgment_key",
    "save_typesafe_key",
    "remove_typesafe_key",
    "move_settings_secrets",
    "check_typesafe_key",
    "save_type_value_key",
    "remove_type_value_key",
    "ssh_submit_credential",
    "save_ssh_host",
    "remove_ssh_host",
    "ssh_save_target",
    "ssh_remove_target",
    "save_remote_server",
    "remove_remote_server",
    "save_remote_workspace",
    "remove_remote_workspace",
    "set_opencode_cookie",
    "import_browser_cookies",
    "import_cookie_file",
    // A saved address written into a page's form — the person's pick (t-79331).
    "browser_address_fill",
    "jira_connect",
    "jira_disconnect",
    "linear_connect",
    "linear_disconnect",
    // An account added, removed, switched, signed in or out.
    "add_claude_account",
    "add_codex_account",
    "remove_claude_account",
    "remove_codex_account",
    "select_claude_account",
    "select_codex_account",
    "relogin_claude_account",
    "relogin_codex_account",
    "relogin_codex_login",
    "logout_codex_login",
    "use_system_claude_login",
    "resolve_claude_account_identity",
    "cli_login_start",
    "cli_login_logout",
    "google_login_start",
    "google_login_finish",
    "cancel_google_login",
    "github_login_intent",
    "github_select_account",
    // Work that cannot be had back (t-74975): the person's uncommitted
    // changes and a merge or rebase in progress, a push, a checkout, a
    // project, a lane or a terminal and the program in it, a browser's
    // sign-ins, Computer Use's own permissions, a saved launch, command,
    // automation, artifact, note or model — and the questions that guard
    // a removal or a close, turned off.
    "discard_paths",
    "delete_untracked",
    "abort_conflict_operation",
    "scm_push",
    "remove_worktree",
    "remove_project",
    "close_lane",
    "close_term",
    "delete_browser_profile",
    "reset_computer_use_permissions",
    "reset_agent_launch",
    "delete_quick_command",
    "delete_automation",
    "artifact_delete",
    "delete_diff_note",
    "clear_diff_notes",
    "managed_judgment_remove",
    "clear_model_connection",
    "set_skip_delete_worktree_confirm",
    "set_skip_delete_automation_confirm",
    "set_skip_close_terminal_with_running_process_confirm",
    "set_confirm_close_pinned",
    // What the session manager ends, a terminal and the program in it, and
    // Computer Use's own confirmations turned off (t-75153, R7b).
    "end_terminal_session",
    "end_all_terminal_sessions",
    "set_computer_confirm",
];

/// Commands whose names touch a key, a secret, a token, a password, a
/// credential, an account or a login that an agent pressing in the window may
/// still run: each reads, or is not a credential at all (the source contract
/// `a_command_named_for_a_credential_or_an_account_is_decided_for` asks every
/// such command to stand here or on [`PERSON_ONLY_COMMANDS`]).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "read by the source contract off this file's text, and by a unit test"
    )
)]
pub(crate) const AGENT_ALLOWED_SENSITIVE: &[&str] = &[
    // Whether a keychain keeps router keys, which accounts stand, what they
    // used, and whether their logins still hold: reads.
    "api_router_keys_kept",
    "claude_accounts",
    "claude_account_usage",
    "claude_token_usage",
    "codex_account_list",
    "codex_token_usage",
    "google_account",
    "type_value_keys",
    "verify_claude_accounts",
    "verify_codex_accounts",
    // A login the person started, watched or let go of: no credential moves.
    "cli_login_list",
    "cli_login_witness",
    "cli_login_wait",
    "cli_login_witness_drop",
    // The names of the secrets zo's settings file holds as plain text, never
    // a value (t-76698): a read. Moving them is the person's alone.
    "settings_secret_literals",
    // Keys as in keyboard keys, not credentials: a terminal's input, a
    // shortcut, the keyboard layout.
    "key_input",
    "term_key",
    "set_keybinding",
    "terminal_keyboard_layout",
];

/// Commands whose names discard, delete, remove, push, reset, close, clear or
/// abort that an agent pressing in the window may still run: each takes
/// nothing the person keeps, or only what is kept to be had back (the source
/// contract `a_command_that_cannot_be_taken_back_is_decided_for` asks every
/// such command to stand here or on [`PERSON_ONLY_COMMANDS`]).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "read by the source contract off this file's text, and by a unit test"
    )
)]
pub(crate) const AGENT_ALLOWED_DESTRUCTIVE_NAMED: &[&str] = &[
    // Kept to be had back: a decision the store keeps to restore for its
    // days, an untitled note removed only while it holds no bytes.
    "delete_custom_decision",
    "delete_untitled_markdown",
    // A view closed or a mark taken down; nothing the person keeps goes.
    "close_browser_pane",
    "close_onboarding",
    "browser_find_clear",
    "clear_finish_mark",
    // Only the review notes an agent was already handed, and only while
    // they still read as they were handed.
    "clear_delivered_diff_notes",
    // A preference for the close button, not a guard on a close.
    "set_minimize_to_tray_on_close",
    // Presets, not a reset.
    "api_router_presets",
    "github_preset_query",
    // A mirror pane the window opened over a run's log: a `tail -F` the run
    // goes on without (t-75153, R7b).
    "close_mirror_term",
    // The person's session regrant on a work browser turned off: the agent
    // loses a grant; the person keeps everything (t-66200).
    "end_browser_workspace_regrant",
];

/// The header in which the page's one `invoke` says whose hand made a call
/// (ui/shell-boot.js `HAND_HEADER`, t-74927): the hand of the flow that made
/// it, never the page's last input (t-75209) — `person` inside the person's
/// own pointer, touch or key and for the flow it started; `agent` inside an
/// agent's press in this window and for the flow it started, whatever the
/// person pressed meanwhile; `none` for a call outside any event that carries
/// no flow's hand; `program` for the one road no hand starts
/// ([`PROGRAM_COMMANDS`]).
pub(crate) const HAND_HEADER: &str = "zerocode-hand";

/// The person-only commands a road no hand starts still runs: the update
/// clock's quiet download and its staging. A `program` call of any other
/// person-only command is refused.
const PROGRAM_COMMANDS: &[&str] = &["update_download", "update_install"];

/// The refusal the window's invoke handler answers a command with, given the
/// hand the call said made it: `None` for a command not on
/// [`PERSON_ONLY_COMMANDS`], or for one the person's hand made.
pub(crate) fn person_only_refusal(command: &str, hand: Option<&str>) -> Option<String> {
    if !PERSON_ONLY_COMMANDS.contains(&command) {
        return None;
    }
    person_only(command, hand).err()
}

/// The window's invoke handler, asking the second wall first: a command on
/// [`PERSON_ONLY_COMMANDS`] is refused in words unless the call says the
/// person made it, and every other command goes to `handler` unasked.
pub(crate) fn person_only_gate<R: tauri::Runtime>(
    handler: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    move |invoke| {
        let hand = hand_said(invoke.message.headers());
        match person_only_refusal(invoke.message.command(), hand) {
            Some(refused) => {
                invoke.resolver.reject(refused);
                true
            }
            None => handler(invoke),
        }
    }
}

/// The hand a call's headers say made it, if they say one.
pub(crate) fn hand_said(headers: &tauri::http::HeaderMap) -> Option<&str> {
    headers.get(HAND_HEADER)?.to_str().ok()
}

/// Whether person-only `command` may run for a call `hand` made: the
/// person's, or the program road's own commands; never an agent's press,
/// however long ago it was, and never a call that names no hand.
pub(crate) fn person_only(command: &str, hand: Option<&str>) -> Result<(), String> {
    debug_assert!(
        PERSON_ONLY_COMMANDS.contains(&command),
        "{command} asks the second wall but is not on PERSON_ONLY_COMMANDS"
    );
    match hand {
        Some("person") => Ok(()),
        Some("program") if PROGRAM_COMMANDS.contains(&command) => Ok(()),
        Some("agent") => Err(format!(
            "{command} is the person's to run, and an agent's press in ZeroCode's window made this call; the person runs it with their own hand"
        )),
        Some("none") => Err(format!(
            "{command} is the person's to run, and this call came from no event of the person's and carries no hand its flow started with"
        )),
        _ => Err(format!(
            "{command} is the person's to run, and this call does not say the person made it"
        )),
    }
}

/// Requests the page has not answered yet, by id.
static WAITING: Mutex<Option<HashMap<u64, mpsc::Sender<Value>>>> = Mutex::new(None);
static NEXT_ASK: AtomicU64 = AtomicU64::new(0);

/// Ask the page once through `emit` and wait for its answer, at most
/// `timeout`. The page answers `{ "ok": … }` or `{ "error": { code, message } }`
/// by id ([`answered`]); silence is no answer, and a press it covered is
/// reported unknown, never as made.
pub(crate) fn ask_through(
    emit: &dyn Fn(u64, &Value),
    request: &Value,
    timeout: Duration,
) -> Result<Value, ComputerUseError> {
    let id = NEXT_ASK.fetch_add(1, Ordering::AcqRel) + 1;
    let (reply, receiver) = mpsc::channel();
    WAITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(id, reply);
    emit(id, request);
    let answer = receiver.recv_timeout(timeout);
    if let Some(waiting) = WAITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
    {
        waiting.remove(&id);
    }
    let Ok(mut answer) = answer else {
        return Err(ComputerUseError::new(
            error_code::ACTION_TIMEOUT,
            format!(
                "ZeroCode's page did not answer within {} ms; whether anything was pressed is unknown — look again",
                timeout.as_millis()
            ),
        ));
    };
    if let Some(error) = answer.get("error") {
        return Err(ComputerUseError::new(
            error["code"]
                .as_str()
                .unwrap_or(error_code::ACCESSIBILITY_ERROR),
            error["message"]
                .as_str()
                .unwrap_or("ZeroCode's page refused"),
        ));
    }
    Ok(answer.get_mut("ok").map(Value::take).unwrap_or(Value::Null))
}

/// The page's answer to request `id` — false when nothing waits for it.
pub(crate) fn answered(id: u64, answer: Value) -> bool {
    WAITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
        .and_then(|waiting| waiting.remove(&id))
        .is_some_and(|reply| reply.send(answer).is_ok())
}

/// Hand the window's own app and its page to the road — at boot, with the
/// page in hand.
pub(crate) fn install(own: OwnApp, page: Arc<Page>) {
    *PAGE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((own, page));
}

fn standing() -> Option<(OwnApp, Arc<Page>)> {
    #[cfg(test)]
    {
        stand_in::STANDING.with(|standing| standing.borrow().clone())
    }
    #[cfg(not(test))]
    {
        PAGE.read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// The one key ZeroCode's own app is held under, whatever name, bundle or
/// pid a request gave it; `None` for another app.
pub(crate) fn own_app_key(query: &str) -> Option<String> {
    let (own, _) = standing()?;
    own.named_by(query)
        .then(|| own.names.first().unwrap_or(&own.bundle_id).to_lowercase())
}

/// The answer to a request that names ZeroCode's own app, or `None` when the
/// request is the helper's: another app, no app, or a look the page does not
/// make (a picture, the window list).
pub(super) fn route(method: &str, params: &Value) -> Option<Result<Value, ComputerUseError>> {
    let query = params.get("app")?.as_str()?;
    let (own, page) = standing()?;
    if !own.named_by(query) {
        return None;
    }
    if !METHODS.contains(&method) {
        let acts = ComputerMethod::ALL
            .iter()
            .find(|candidate| candidate.provider_name() == Some(method))
            .is_none_or(|candidate| candidate.acts());
        // A wait would answer the helper's indexes, which no press here reads.
        return (acts || method == "waitFor").then(|| Err(unsupported(method)));
    }
    if let Some(reason) = super::guard::stopped_reason() {
        return Some(Err(super::guard::refusal(&reason)));
    }
    if super::confirm::asking() {
        return Some(Err(ComputerUseError::new(
            error_code::PERSON_ASKED,
            "the person is being asked about a step; nothing in ZeroCode's window is pressed until they answer",
        )));
    }
    let act = Act::of_method(method)?;
    Some(match act {
        Act::Look => look(&own, page.as_ref(), method, params),
        _ => press(&own, page.as_ref(), act, method, params),
    })
}

fn look(
    own: &OwnApp,
    page: &Page,
    method: &str,
    params: &Value,
) -> Result<Value, ComputerUseError> {
    let answered = page(&json!({ "kind": "look" }))?;
    let mut snapshot = snapshot(own, &answered)?;
    if method == "readText" {
        return Ok(json!({ "text": snapshot["treeText"].take() }));
    }
    let mut answer = json!({
        "snapshot": snapshot,
        "screenshot": null,
        "screenshotStatus": { "status": "not_taken", "reason": "self_surface" },
    });
    if method == "findElements" {
        let wanted = params
            .get("query")
            .or_else(|| params.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let found: Vec<usize> = answer["snapshot"]["treeText"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .filter_map(|row| {
                let (index, rest) = row.split_once(' ')?;
                (!wanted.is_empty() && rest.to_lowercase().contains(&wanted))
                    .then(|| index.parse().ok())?
            })
            .collect();
        answer["matches"] = json!(found);
    }
    Ok(answer)
}

/// The page's look as the helper's snapshot reads: the same tree grammar and
/// indexes, each control the window would not press saying so.
fn snapshot(own: &OwnApp, answered: &Value) -> Result<Value, ComputerUseError> {
    let controls = answered
        .get("controls")
        .and_then(Value::as_array)
        .ok_or_else(|| broken("the page's look carried no controls"))?;
    let mut shown = Vec::with_capacity(controls.len());
    let mut lines = Vec::with_capacity(controls.len());
    for (at, control) in controls.iter().enumerate() {
        // A control whose facts do not read is listed, and refused closed.
        let facts = facts_of(control);
        let role = control["role"].as_str().unwrap_or("AXUnknown").to_string();
        let name = control["name"].as_str().unwrap_or_default().to_string();
        let node = RenderNode {
            role: role.clone(),
            title: (!name.is_empty()).then(|| name.clone()),
            // A secret field's value never leaves the page, and the
            // person's text leaves it as its length only.
            value: facts.as_ref().ok().and_then(|facts| {
                shown_value(
                    facts,
                    &role,
                    control["value"].as_str(),
                    control["withheld"].as_u64(),
                )
            }),
            ..RenderNode::default()
        };
        let mut row = line(at + 1, &node);
        if let Err(refused) = facts.and_then(|facts| judge(&facts, Act::Click)) {
            row.push_str(&format!(" [{}]", refused.code));
        }
        lines.push(row);
        shown.push(Shown {
            id: control["id"].clone(),
            role,
            name,
        });
    }
    let id = answered.get("lookId").cloned().unwrap_or(Value::Null);
    let title = answered["title"].as_str().unwrap_or("ZeroCode");
    let app = AppIdentity {
        name: own.names.first().cloned().unwrap_or_default(),
        bundle_id: Some(own.bundle_id.clone()),
        pid: own.pid,
    };
    let focused = answered["focused"]
        .as_u64()
        .map(|index| format!("element {index}"));
    let tree = render_tree_text(&app, title, &lines, focused.as_deref());
    *LAST_LOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Look {
        id: id.clone(),
        controls: shown,
    });
    Ok(json!({
        "id": id,
        "app": { "name": app.name, "bundleId": app.bundle_id, "pid": app.pid },
        "window": { "id": 0, "title": title },
        "coordinateSpace": "window",
        "treeText": tree,
        "elementCount": lines.len(),
        "selfSurface": true,
    }))
}

fn press(
    own: &OwnApp,
    page: &Page,
    act: Act,
    method: &str,
    params: &Value,
) -> Result<Value, ComputerUseError> {
    let index = params
        .get("elementIndex")
        .and_then(Value::as_u64)
        .filter(|index| *index > 0)
        .ok_or_else(|| {
            ComputerUseError::new(
                error_code::INVALID_ARGUMENT,
                "ZeroCode's own window is pressed by an element index from `get-app-state --app ZeroCode`",
            )
        })?;
    let look = LAST_LOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or_else(|| not_found("no look of ZeroCode's window stands; look first"))?;
    let shown = usize::try_from(index - 1)
        .ok()
        .and_then(|at| look.controls.get(at))
        .ok_or_else(|| not_found("the last look of ZeroCode's window showed no such element"))?;
    let asked = json!({ "kind": "facts", "lookId": look.id, "id": shown.id });
    let fresh = page(&asked)?;
    if fresh.get("gone") == Some(&Value::Bool(true))
        || fresh["role"].as_str() != Some(shown.role.as_str())
        || fresh["name"].as_str().unwrap_or_default() != shown.name
    {
        return Err(not_found(
            "that element of ZeroCode's window changed since the look; look again",
        ));
    }
    let facts = facts_of(&fresh)?;
    judge(&facts, act)?;
    let mut request = json!({
        "kind": "do",
        "lookId": look.id,
        "id": shown.id,
        "act": act,
        "facts": facts,
    });
    match act {
        Act::SetValue => {
            let value = params.get("value").and_then(Value::as_str).ok_or_else(|| {
                ComputerUseError::new(error_code::INVALID_ARGUMENT, "set-value needs --value")
            })?;
            request["value"] = json!(value);
        }
        Act::Scroll => {
            request["direction"] = params.get("direction").cloned().unwrap_or(json!("down"));
            request["pages"] = params.get("pages").cloned().unwrap_or(json!(1));
        }
        Act::Click | Act::Look => {}
    }
    // The page presses as the agent's hand, so a person-only command the press
    // leads to says so and the second wall refuses it ([`HAND_HEADER`]).
    let done = page(&request)?;
    if done.get("done") != Some(&Value::Bool(true)) {
        return Err(not_found(
            "that element of ZeroCode's window changed before the press; nothing was pressed — look again",
        ));
    }
    let after = page(&json!({ "kind": "look" }))?;
    Ok(json!({
        "action": {
            "path": "self",
            "actionName": method,
            "background": { "requested": true, "foregroundUnchanged": true },
        },
        "snapshot": snapshot(own, &after)?,
    }))
}

/// The facts a page answered for a control; facts that do not read refuse
/// the press closed.
fn facts_of(control: &Value) -> Result<Facts, ComputerUseError> {
    serde_json::from_value(control.get("facts").cloned().unwrap_or(Value::Null)).map_err(|_| {
        ComputerUseError::new(
            error_code::SELF_SENSITIVE,
            "not pressed: the page did not say what that control of ZeroCode's window is",
        )
    })
}

fn not_found(message: &str) -> ComputerUseError {
    ComputerUseError::new(error_code::ELEMENT_NOT_FOUND, message)
}

fn broken(message: &str) -> ComputerUseError {
    ComputerUseError::new(error_code::ACCESSIBILITY_ERROR, message)
}

/// The page stood in for by a test, on this thread.
#[cfg(test)]
pub(crate) mod stand_in {
    use std::cell::RefCell;
    use std::sync::Arc;

    use super::{OwnApp, Page};

    thread_local! {
        pub(super) static STANDING: RefCell<Option<(OwnApp, Arc<Page>)>> = const { RefCell::new(None) };
    }

    /// The page stands while this lives.
    pub(crate) struct Standing(());

    impl Drop for Standing {
        fn drop(&mut self) {
            STANDING.with(|standing| standing.borrow_mut().take());
        }
    }

    pub(crate) fn stand(own: OwnApp, page: Arc<Page>) -> Standing {
        STANDING.with(|standing| *standing.borrow_mut() = Some((own, page)));
        Standing(())
    }
}

#[cfg(test)]
mod tests;
