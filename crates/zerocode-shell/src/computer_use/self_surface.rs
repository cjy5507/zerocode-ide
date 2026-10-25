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
    Act, Facts, METHODS, OwnApp, judge, unsupported,
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
            // A secret field's value never leaves the page.
            value: facts
                .as_ref()
                .is_ok_and(|facts| !facts.secret)
                .then(|| control["value"].as_str().map(str::to_string))
                .flatten(),
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
