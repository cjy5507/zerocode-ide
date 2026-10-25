//! One window shared with an agent, held by the window (t-73906; the rule is
//! core `computer_use_protocol::share`).
//!
//! A command is admitted here before it acts: the first act in an app asks
//! the person once through the question popup (`computer:share`, answered by
//! `computer_confirm_answer`), and the answer is held for the window's
//! lifetime. The band paints the share (`computer:share-state`) and offers
//! "take control" and "hand back"; a request admitted under one generation
//! is refused at its next provider step once the person took control.

use std::cell::Cell;
use std::collections::BTreeMap;
#[cfg(not(test))]
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;
use serde_json::{Value, json};
use zerocode_core::computer_use::ComputerMethod;
use zerocode_core::computer_use_protocol::share::{
    Answer, Gate, Reach, SHARE_ASK_MS, Share, app_key, took_control,
};

use super::ComputerUseError;
use super::confirm::Decision;

/// The card the window raises: which app an agent asks for, and how long the
/// window waits before the person's silence is the answer.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ShareAsk {
    pub(crate) id: String,
    pub(crate) app: String,
    #[serde(rename = "timeoutMs")]
    pub(crate) timeout_ms: u64,
}

pub(crate) type Asker = dyn Fn(&ShareAsk) -> Decision + Send + Sync;
pub(crate) type Notice = dyn Fn(&Value) + Send + Sync;

/// The window's verbs that are never held to a share: a stop, a resume, a
/// status, the person's turn.
const NEVER_HELD: [ComputerMethod; 4] = [
    ComputerMethod::Stop,
    ComputerMethod::Resume,
    ComputerMethod::Status,
    ComputerMethod::Handoff,
];

/// The share and the words a person knows each app by.
#[derive(Default)]
struct Held {
    share: Share,
    labels: BTreeMap<String, String>,
}

#[cfg(not(test))]
static HELD: Mutex<Option<Held>> = Mutex::new(None);
#[cfg(not(test))]
static ROAD: RwLock<Option<(Arc<Asker>, Arc<Notice>)>> = RwLock::new(None);
#[cfg(test)]
thread_local! {
    static HELD: std::cell::RefCell<Held> = std::cell::RefCell::new(Held::default());
}

thread_local! {
    /// The share's generation the request this thread runs was admitted under.
    static ADMITTED: Cell<Option<u64>> = const { Cell::new(None) };
}

fn with_held<T>(touch: impl FnOnce(&mut Held) -> T) -> T {
    #[cfg(not(test))]
    {
        touch(
            HELD.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_or_insert_with(Held::default),
        )
    }
    #[cfg(test)]
    {
        HELD.with(|held| touch(&mut held.borrow_mut()))
    }
}

/// While this lives, the command it was admitted for is held to the share's
/// generation it saw.
pub(crate) struct Scope {
    previous: Option<u64>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        ADMITTED.with(|admitted| admitted.set(self.previous));
    }
}

fn scope(generation: Option<u64>) -> Scope {
    Scope {
        previous: ADMITTED.with(|admitted| admitted.replace(generation)),
    }
}

/// The app a request reaches, as the share keys it, and the words it named
/// it by: its `--app`, or the app its mark was pinned in.
fn reached(params: &Value) -> Option<(String, String)> {
    let named = params
        .get("app")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            params.get("mark")?;
            let pinned = super::marks::pinned_click(params).ok()?;
            pinned.params.get("app")?.as_str().map(str::to_string)
        })?;
    let key = super::self_surface::own_app_key(&named).unwrap_or_else(|| app_key(&named));
    Some((key, named.trim().to_string()))
}

/// Whether a request looks at the whole screen rather than one app's window.
fn whole_screen(method: ComputerMethod, params: &Value) -> bool {
    let unnamed = ["app", "mark", "windowId"]
        .iter()
        .all(|key| params.get(*key).is_none());
    matches!(method, ComputerMethod::Screenshot | ComputerMethod::Zoom)
        || (unnamed && matches!(method, ComputerMethod::Observe | ComputerMethod::Watch))
}

/// Admit `method` with `params` under the share: ask the person when it is
/// the first act in an app, refuse what lies outside the shared window.
pub(crate) fn admit(method: ComputerMethod, params: &Value) -> Result<Scope, ComputerUseError> {
    if NEVER_HELD.contains(&method) {
        return Ok(scope(None));
    }
    let named = reached(params);
    let acts = method.acts();
    let screen = whole_screen(method, params);
    loop {
        let before = with_held(|held| held.share.target().map(str::to_string));
        let (gate, generation, after) = with_held(|held| {
            let gate = held.share.gate(Reach {
                app: named.as_ref().map(|(key, _)| key.as_str()),
                acts,
                whole_screen: screen,
            });
            (
                gate,
                held.share.generation(),
                held.share.target().map(str::to_string),
            )
        });
        if before != after {
            tell();
        }
        match gate {
            Gate::Go => return Ok(scope(Some(generation))),
            Gate::Refuse(refused) => return Err(refused),
            Gate::Ask(key) => {
                let label = named
                    .as_ref()
                    .map_or_else(|| key.clone(), |(_, label)| label.clone());
                let answer = match ask(&ShareAsk {
                    id: super::confirm::next_id(),
                    app: label.clone(),
                    timeout_ms: SHARE_ASK_MS,
                }) {
                    Decision::Allowed => Answer::Shared,
                    Decision::Refused => Answer::Declined,
                    Decision::TimedOut => Answer::Silent,
                };
                with_held(|held| {
                    held.labels.insert(key.clone(), label);
                    held.share.answered(&key, answer);
                });
                tell();
            }
        }
    }
}

/// Whether the request this thread runs is still under the share it was
/// admitted to: refused once the person took control since.
pub(crate) fn ensure_current() -> Result<(), ComputerUseError> {
    let Some(admitted) = ADMITTED.with(Cell::get) else {
        return Ok(());
    };
    if with_held(|held| held.share.generation()) == admitted {
        Ok(())
    } else {
        Err(took_control())
    }
}

/// The person takes control of the shared window.
pub(crate) fn take_control() {
    with_held(|held| held.share.take_back());
    tell();
}

/// The person hands the shared window back.
pub(crate) fn give_back() {
    with_held(|held| held.share.give_back());
    tell();
}

/// The share as the band paints it: the shared app's key and the words the
/// person knows it by, and whether the person took control.
pub(crate) fn state() -> Value {
    with_held(|held| {
        let target = held.share.target();
        json!({
            "target": target,
            "label": target.and_then(|key| held.labels.get(key)),
            "takenBack": held.share.taken_back(),
        })
    })
}

/// Hand the window's card and band to the share — at boot.
pub(crate) fn install(asker: Box<Asker>, notice: Box<Notice>) {
    #[cfg(not(test))]
    {
        *ROAD
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((Arc::from(asker), Arc::from(notice)));
    }
    #[cfg(test)]
    {
        let _ = (asker, notice);
    }
}

/// Ask the person about an app; with nobody to ask, the answer is silence and
/// the agent works as before.
fn ask(card: &ShareAsk) -> Decision {
    #[cfg(not(test))]
    {
        let road = ROAD
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        road.map_or(Decision::TimedOut, |(asker, _)| asker(card))
    }
    #[cfg(test)]
    {
        stand_in::ASKED
            .with(|asked| asked.borrow().clone())
            .map_or(Decision::TimedOut, |answer| answer(card))
    }
}

/// Tell the band what is shared now.
fn tell() {
    let said = state();
    #[cfg(not(test))]
    {
        let road = ROAD
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some((_, notice)) = road {
            notice(&said);
        }
    }
    #[cfg(test)]
    {
        stand_in::NOTICES.with(|notices| notices.borrow_mut().push(said));
    }
}

#[cfg(test)]
pub(crate) mod stand_in {
    use std::cell::RefCell;
    use std::rc::Rc;

    use serde_json::Value;

    use super::{Decision, ShareAsk};

    /// The person, as a test answers for them.
    pub(super) type Person = Rc<dyn Fn(&ShareAsk) -> Decision>;

    thread_local! {
        pub(super) static ASKED: RefCell<Option<Person>> = const { RefCell::new(None) };
        pub(super) static NOTICES: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
    }

    /// The person stood in on this thread: answers every card with `answer`
    /// and counts the cards.
    pub(crate) struct Standing(());

    impl Drop for Standing {
        fn drop(&mut self) {
            ASKED.with(|asked| asked.borrow_mut().take());
            NOTICES.with(|notices| notices.borrow_mut().clear());
        }
    }

    pub(crate) fn stand(answer: impl Fn(&ShareAsk) -> Decision + 'static) -> Standing {
        ASKED.with(|asked| *asked.borrow_mut() = Some(Rc::new(answer)));
        Standing(())
    }

    /// What the band was told so far on this thread.
    pub(crate) fn notices() -> Vec<Value> {
        NOTICES.with(|notices| notices.borrow().clone())
    }
}

#[cfg(test)]
mod tests;
