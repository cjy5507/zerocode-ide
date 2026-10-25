//! One window shared with an agent, held by the window (t-73906; the rule is
//! core `computer_use_protocol::share`).
//!
//! A command is admitted here before it acts: the first act in an app asks
//! the person once through the question popup (`computer:share`, answered by
//! `computer_confirm_answer`), and the answer is held for the window's
//! lifetime. The band paints the share (`computer:share-state`) and offers
//! "take control" and "hand back"; a request admitted under one generation
//! is refused at its next provider step once the person took control. With
//! sharing required, an app whose card went unanswered waits (t-74826): the
//! band names it and offers the hand-over (`computer_share_hand_over`), and
//! the card is raised again only after the quiet.

use std::cell::RefCell;
use std::collections::BTreeMap;
#[cfg(not(test))]
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;
use serde_json::{Value, json};
use zerocode_core::computer_use::ComputerMethod;
#[cfg(test)]
pub(crate) use zerocode_core::computer_use_protocol::share::SHARE_ASK_AGAIN_MS;
use zerocode_core::computer_use_protocol::share::{
    Answer, Gate, Reach, SHARE_ASK_MS, Share, app_key, took_control,
};

use super::ComputerUseError;
use super::confirm::Decision;

/// The card the window raises: which app an agent asks for, how long the
/// window waits before the person's silence is the answer, and whether
/// sharing is required — then silence keeps the agent waiting rather than
/// lets it go on unshared, and the card's clock says so.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ShareAsk {
    pub(crate) id: String,
    pub(crate) app: String,
    #[serde(rename = "timeoutMs")]
    pub(crate) timeout_ms: u64,
    pub(crate) required: bool,
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
    panel_revision: u64,
    panel_first_revision: u64,
    panel_share: (Option<String>, u64),
    panel_label: String,
    panel_window: Value,
}

#[cfg(not(test))]
static HELD: Mutex<Option<Held>> = Mutex::new(None);
#[cfg(not(test))]
static REQUIRED: RwLock<Option<Arc<Required>>> = RwLock::new(None);

/// Reads the current policy from the settings repository.
pub(crate) type Required = dyn Fn() -> bool + Send + Sync;

pub(crate) fn install_required(read: Box<Required>) {
    #[cfg(not(test))]
    {
        *REQUIRED
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::from(read));
    }
    #[cfg(test)]
    stand_in::REQUIRED.with(|held| *held.borrow_mut() = Some(read));
}

fn refresh_required() {
    #[cfg(test)]
    stand_in::REQUIRED.with(|read| {
        if let Some(read) = read.borrow().as_ref() {
            with_held(|held| held.share.set_required(read()));
        }
    });
    #[cfg(not(test))]
    {
        let read = REQUIRED
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(read) = read {
            let required = read();
            with_held(|held| held.share.set_required(required));
        }
    }
}
#[cfg(not(test))]
static ROAD: RwLock<Option<(Arc<Asker>, Arc<Notice>)>> = RwLock::new(None);

/// The window's clock for the share, in ms: monotonic, from its first read.
fn now_ms() -> u64 {
    #[cfg(not(test))]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let start = START.get_or_init(std::time::Instant::now);
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
    #[cfg(test)]
    {
        stand_in::NOW.with(std::cell::Cell::get)
    }
}
#[cfg(test)]
thread_local! {
    static HELD: std::cell::RefCell<Held> = std::cell::RefCell::new(Held::default());
}

#[derive(Clone)]
struct Admitted {
    generation: u64,
    app: Option<String>,
    acts: bool,
    whole_screen: bool,
}

thread_local! {
    static ADMITTED: RefCell<Option<Admitted>> = const { RefCell::new(None) };
}

fn with_held<T>(touch: impl FnOnce(&mut Held) -> T) -> T {
    let tracked = |held: &mut Held| {
        let result = touch(held);
        if held.panel_share.0.as_deref() != held.share.target()
            || held.panel_share.1 != held.share.generation()
        {
            // A target change is a presentation change, not a hand-back. A
            // pending physical press takes control of every app in this generation.
            held.panel_revision += 1;
            if held.panel_share.1 != held.share.generation() || held.panel_first_revision == 0 {
                held.panel_first_revision = held.panel_revision;
            }
            held.panel_share = (
                held.share.target().map(str::to_string),
                held.share.generation(),
            );
        }
        result
    };
    #[cfg(not(test))]
    {
        tracked(
            HELD.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_or_insert_with(Held::default),
        )
    }
    #[cfg(test)]
    {
        HELD.with(|held| tracked(&mut held.borrow_mut()))
    }
}

/// While this lives, the command it was admitted for is held to the share's
/// generation it saw.
pub(crate) struct Scope {
    previous: Option<Admitted>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        ADMITTED.with(|admitted| *admitted.borrow_mut() = self.previous.take());
    }
}

fn scope(generation: Option<Admitted>) -> Scope {
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
        refresh_required();
        let before = with_held(|held| held.share.target().map(str::to_string));
        let now = now_ms();
        let (gate, generation, after) = with_held(|held| {
            let gate = held.share.gate(
                Reach {
                    app: named.as_ref().map(|(key, _)| key.as_str()),
                    acts,
                    whole_screen: screen,
                },
                now,
            );
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
            Gate::Go => {
                if named.as_ref().is_some_and(|(key, _)| {
                    with_held(|held| held.share.target() == Some(key.as_str()))
                }) {
                    let pinned = params
                        .get("mark")
                        .and_then(|_| super::marks::pinned_click(params).ok());
                    let selected = pinned
                        .as_ref()
                        .map(|pinned| &pinned.params)
                        .or_else(|| params.as_object());
                    let window = json!({"windowId": selected.and_then(|p| p.get("windowId")), "windowIndex": selected.and_then(|p| p.get("windowIndex"))});
                    let changed = with_held(|held| {
                        if held.panel_window == window {
                            return false;
                        }
                        held.panel_window = window;
                        true
                    });
                    if changed {
                        tell();
                    }
                }
                return Ok(scope(Some(Admitted {
                    generation,
                    app: named.as_ref().map(|(key, _)| key.clone()),
                    acts,
                    whole_screen: screen,
                })));
            }
            Gate::Refuse(refused) => return Err(refused),
            Gate::Ask(key) => {
                let label = named
                    .as_ref()
                    .map_or_else(|| key.clone(), |(_, label)| label.clone());
                let required = with_held(|held| held.share.required());
                let answer = match ask(&ShareAsk {
                    id: super::confirm::next_id(),
                    app: label.clone(),
                    timeout_ms: SHARE_ASK_MS,
                    required,
                }) {
                    Decision::Allowed => Answer::Shared,
                    Decision::Refused => Answer::Declined,
                    Decision::TimedOut => Answer::Silent,
                };
                with_held(|held| {
                    held.labels.insert(key.clone(), label);
                    held.share.answered(&key, answer, now_ms());
                });
                tell();
            }
        }
    }
}

/// Whether the request this thread runs is still under the share it was
/// admitted to: refused once the person took control since, and otherwise
/// judged as admitting it now would be — outside the share the person made
/// since, or required sharing for an app they did not hand over. Judged
/// without moving the share (t-74826): a step in flight never takes it back
/// from the app the person handed over since.
pub(crate) fn ensure_current() -> Result<(), ComputerUseError> {
    let Some(admitted) = ADMITTED.with(|held| held.borrow().clone()) else {
        return Ok(());
    };
    refresh_required();
    let now = now_ms();
    with_held(|held| {
        if held.share.generation() != admitted.generation {
            return Err(took_control());
        }
        held.share.still_holds(
            Reach {
                app: admitted.app.as_deref(),
                acts: admitted.acts,
                whole_screen: admitted.whole_screen,
            },
            now,
        )
    })
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

/// The person hands over, from the band, the app that waits for them under
/// required sharing — no card. `app` is the key the band was told it by
/// (`waiting`), taken as it is. Whether it was the app that waits.
pub(crate) fn hand_over(app: &str) -> bool {
    let handed = with_held(|held| held.share.hand_over(app));
    if handed {
        tell();
    }
    handed
}

/// The share as the band paints it: the shared app's key and the words the
/// person knows it by, whether the person took control, and the app that
/// waits for them to hand it over.
pub(crate) fn state() -> Value {
    with_held(|held| {
        let target = held.share.target();
        let waiting = held.share.waiting();
        json!({
            "target": target,
            "label": target.and_then(|key| held.labels.get(key)),
            "waiting": waiting,
            "waitingLabel": waiting.and_then(|key| held.labels.get(key)),
            "takenBack": held.share.taken_back(),
            "generation": held.share.generation(),
            "revision": held.panel_revision,
            "buttonLabel": held.panel_label,
            "windowId": held.panel_window.get("windowId"),
            "windowIndex": held.panel_window.get("windowIndex"),
        })
    })
}

/// Reuse the renderer's existing translated words on the native panel.
pub(crate) fn panel_label(label: Option<String>) {
    // This read command can carry only the existing catalog's control name.
    // A caller cannot rename the human control to an arbitrary instruction.
    const LABELS: [&str; 5] = [
        "제어권 가져오기",
        "Take control",
        "操作を取り戻す",
        "收回控制权",
        "Tomar el control",
    ];
    if let Some(label) = label.filter(|label| LABELS.contains(&label.as_str())) {
        with_held(|held| held.panel_label = label);
        #[cfg(all(target_os = "macos", not(test)))]
        super::macos::sync_share_panel(&state());
    }
}

/// A physical panel press survives target changes within the same generation.
/// A press from before a hand-back cannot take a new generation.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) fn hear_panel(answer: &Value) {
    let Some(panel) = answer.get("shareControl") else {
        return;
    };
    if panel.get("takenBack").and_then(Value::as_bool) != Some(true) {
        return;
    }
    let Some(revision) = panel.get("revision").and_then(Value::as_u64) else {
        return;
    };
    let took = with_held(|held| {
        if held.share.taken_back()
            || held.share.target().is_none()
            || panel.get("target").and_then(Value::as_str).is_none()
            || panel.get("generation").and_then(Value::as_u64) != Some(held.share.generation())
            || revision < held.panel_first_revision
            || revision > held.panel_revision
        {
            return false;
        }
        held.share.take_back();
        true
    });
    if took {
        tell();
    }
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
    with_held(|held| held.panel_revision += 1);
    let said = state();
    #[cfg(all(target_os = "macos", not(test)))]
    {
        super::macos::sync_share_panel(&said);
    }
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

    use super::{Decision, Required, ShareAsk};

    /// The person, as a test answers for them.
    pub(super) type Person = Rc<dyn Fn(&ShareAsk) -> Decision>;

    thread_local! {
        pub(super) static REQUIRED: RefCell<Option<Box<Required>>> = const { RefCell::new(None) };
        pub(super) static ASKED: RefCell<Option<Person>> = const { RefCell::new(None) };
        pub(super) static NOTICES: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
        pub(super) static NOW: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    /// Move this thread's share clock on by `ms`.
    pub(crate) fn advance(ms: u64) {
        NOW.with(|now| now.set(now.get() + ms));
    }

    /// The person stood in on this thread: answers every card with `answer`
    /// and counts the cards.
    pub(crate) struct Standing(());

    impl Drop for Standing {
        fn drop(&mut self) {
            ASKED.with(|asked| asked.borrow_mut().take());
            REQUIRED.with(|read| read.borrow_mut().take());
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
