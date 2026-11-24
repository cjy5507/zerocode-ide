//! Background input stays in its target app and never falls back to the desktop.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{ProviderError, error_code};
use crate::computer_use::{ComputerCommand, ComputerMethod};

pub const BACKGROUND: &str = "background";
pub const REQUIRES_FOREGROUND: &str = "requires_foreground";
/// The agent asked for the shared desktop at once.
pub const FOREGROUND: &str = "foreground";
/// The person's setting lets this request borrow the shared desktop when the
/// background road cannot go.
pub const MAY_BORROW_DESKTOP: &str = "mayBorrowDesktop";
/// Whether this platform's provider can act in a window without the shared
/// desktop. Where it cannot, the person's shared desktop is the only road.
pub(crate) const HAS_BACKGROUND_ROAD: bool = cfg!(target_os = "macos");

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputPolicy {
    #[default]
    Background,
    SharedDesktop,
}

impl InputPolicy {
    #[must_use]
    pub const fn within(self, parent: Self) -> Self {
        match (self, parent) {
            (Self::SharedDesktop, Self::SharedDesktop) => Self::SharedDesktop,
            _ => Self::Background,
        }
    }

    pub fn prepare(self, command: &mut ComputerCommand) -> Result<(), ProviderError> {
        if self == Self::SharedDesktop {
            return prepare_shared(command, HAS_BACKGROUND_ROAD);
        }
        let params = command.params.as_object_mut().ok_or_else(|| {
            ProviderError::invalid_argument("command parameters must be an object")
        })?;
        if params.get("restoreWindow") == Some(&Value::Bool(true))
            || params.get(FOREGROUND) == Some(&Value::Bool(true))
        {
            return Err(policy_refusal());
        }
        if command.method == ComputerMethod::Permissions
            && (params.contains_key("id") || params.get("reset") == Some(&Value::Bool(true)))
        {
            return Err(policy_refusal());
        }
        stay_in_the_background(command, out_of_any_window, policy_refusal)
    }

    pub fn prepare_provider(self, method: &str, params: Value) -> Result<Value, ProviderError> {
        if matches!(
            method,
            "status"
                | "stop"
                | "resume"
                | super::eye::START_METHOD
                | super::eye::CHANGES_METHOD
                | super::eye::FRAME_METHOD
                | "reflexStatus"
                | "reflexStop"
                | "reflexReceipts"
                | "reflexAck"
        ) {
            return Ok(params);
        }
        // Several verbs share `windowAction`; only `focus` takes the front app.
        let found = if method == "windowAction" {
            Some(
                if params.get("action").and_then(Value::as_str) == Some("focus") {
                    ComputerMethod::WindowFocus
                } else {
                    ComputerMethod::WindowMove
                },
            )
        } else {
            ComputerMethod::ALL
                .iter()
                .copied()
                .find(|candidate| candidate.provider_name() == Some(method))
        };
        let Some(method) = found else {
            return if self == Self::SharedDesktop {
                Ok(params)
            } else {
                Err(policy_refusal())
            };
        };
        let mut command = ComputerCommand {
            method,
            params,
            json: true,
        };
        self.prepare(&mut command)?;
        Ok(command.params)
    }
}

fn in_its_window(method: ComputerMethod) -> bool {
    method
        .provider_name()
        .is_some_and(|name| METHODS.contains(&name))
}

/// What the background policy refuses outright: an act with no window of its own.
fn out_of_any_window(method: ComputerMethod) -> bool {
    let composite = matches!(
        method,
        ComputerMethod::Batch
            | ComputerMethod::Parallel
            | ComputerMethod::RecipeRun
            | ComputerMethod::Walk
    );
    // A table written to its file or through its app's script takes no
    // window, pointer, key or front app at all (t-102013).
    let deskless = method == ComputerMethod::SheetWrite;
    method.acts() && !in_its_window(method) && !composite && !deskless
}

fn takes_the_desk(method: ComputerMethod) -> bool {
    method.takes_the_desk()
}

/// A request that goes no further than its own window. Opening an app or a page
/// takes no pointer and no focus: it comes up behind.
fn stay_in_the_background(
    command: &mut ComputerCommand,
    refused: fn(ComputerMethod) -> bool,
    refusal: fn() -> ProviderError,
) -> Result<(), ProviderError> {
    let method = command.method;
    let params = command
        .params
        .as_object_mut()
        .ok_or_else(|| ProviderError::invalid_argument("command parameters must be an object"))?;
    if matches!(method, ComputerMethod::Launch | ComputerMethod::Open) {
        params.insert(BACKGROUND.to_owned(), Value::Bool(true));
        return Ok(());
    }
    if refused(method) {
        return Err(refusal());
    }
    let scoped = in_its_window(method);
    let has_target = params.contains_key("app") || params.contains_key("mark");
    if scoped && method.acts() && !has_target {
        return Err(refusal());
    }
    if has_target
        && (scoped
            || matches!(
                method,
                ComputerMethod::Observe | ComputerMethod::WaitFor | ComputerMethod::Walk
            ))
    {
        params.insert(BACKGROUND.to_owned(), Value::Bool(true));
    }
    Ok(())
}

/// The person lets the agent use the shared desktop, and nothing takes it on its
/// own: several agents work at once and the person is working too. A request stays
/// in the background exactly as under the background policy, and is told when it
/// cannot. Only a call that says so goes to the desktop: `--foreground`,
/// `--restore-window`, or the press by words such a call turned into. The skill
/// tells an agent to say so only when the person asked for that work.
fn prepare_shared(
    command: &mut ComputerCommand,
    background_road: bool,
) -> Result<(), ProviderError> {
    if !background_road {
        return Ok(());
    }
    let params = command
        .params
        .as_object()
        .ok_or_else(|| ProviderError::invalid_argument("command parameters must be an object"))?;
    let says_so = |key: &str| params.get(key) == Some(&Value::Bool(true));
    if says_so(FOREGROUND) || says_so("restoreWindow") || says_so(MAY_BORROW_DESKTOP) {
        return Ok(());
    }
    stay_in_the_background(command, takes_the_desk, shared_refusal)
}

fn shared_refusal() -> ProviderError {
    ProviderError::new(
        REQUIRES_FOREGROUND,
        "this takes the person's pointer, keyboard or front app; nothing was done. Use an app-scoped background action, or hand it to them. Add --foreground only if they asked for this on their desktop; a caller without it hands the step to them",
    )
}

fn policy_refusal() -> ProviderError {
    ProviderError::new(
        REQUIRES_FOREGROUND,
        "the person's Computer Use setting permits background input only; use an app-scoped background action or an isolated browser; no shared pointer, focus, clipboard or process fallback was attempted",
    )
}
pub const METHODS: &[&str] = &[
    "getAppState",
    "readText",
    "findElements",
    "waitFor",
    "click",
    "performSecondaryAction",
    "setValue",
    "scroll",
    "typeText",
    "pressKey",
    "hotkey",
    "pasteText",
    "launchApp",
    "openTarget",
];

#[must_use]
pub fn requested(params: &Map<String, Value>) -> bool {
    params.get(BACKGROUND) == Some(&Value::Bool(true))
}

/// Whether a step's words send it down the background road (I-t123731-rc1):
/// the policy every request is prepared under sends a step that names its
/// app (or a pinned mark) to its window, and an app or a page it opens comes
/// up behind — the two policies alike — while a step that takes the desk
/// (`--foreground`, `--restore-window`) could run only under the shared one,
/// which sends it to the desktop. Where there is no background road, no step
/// goes down it. The words are what a step's evidence goes by only when no
/// act of the step was heard (`acted`): ZeroCode's own page pressed it, or
/// it pressed nothing — what ran is read off the act itself, since the words
/// may say `--foreground` of a mark click that went to the background.
#[must_use]
pub fn went_in_the_background(command: &ComputerCommand) -> bool {
    let mut prepared = command.clone();
    HAS_BACKGROUND_ROAD
        && InputPolicy::Background.prepare(&mut prepared).is_ok()
        && prepared.params.as_object().is_some_and(requested)
}

/// What an act did, as the helper answered it (I-t123731-rc1): whether it
/// went no further than one window, and that window as the helper found it
/// when it acted. A step's evidence is framed by this — not by its words read
/// again, which may say `--foreground` of a mark click the shared desktop
/// sent to the background, nor by the look after the act, which may land on
/// another window of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acted {
    /// The act touched its window alone: an app's act the helper ran in the
    /// background without borrowing the desktop, a window moved, sized,
    /// minimized, zoomed or closed by its number, or an app or a page
    /// opened behind.
    pub in_its_window: bool,
    /// The window it acted in, when the helper named one.
    pub window: Option<ActedWindow>,
}

/// A window as an act found it: its number, and the process that owned it
/// then — so a number the window server gave out again names no other
/// window. On the wire, as an act's answer names it: `{ "id", "pid" }`
/// ([`ACTED_WINDOW`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActedWindow {
    pub id: u64,
    pub pid: u64,
}

impl ActedWindow {
    /// This window, for the answer of an act aimed at it to name
    /// (`actedWindow`), when its provider saw this window take the act's
    /// input (`took_input`, the number of the window it saw take it) — and
    /// none when it saw another window take it, or did not see (review
    /// t-129021 §3). A step's evidence keeps a look of the window its answer
    /// names, so a window the input was only aimed at — a wheel or a drag
    /// sent at a point of the screen, which whatever window stands there
    /// takes; keys sent while another window of the app stood in front over
    /// it — is named by no answer, and its step keeps no picture.
    #[must_use]
    pub fn if_it_took(self, took_input: Option<u64>) -> Option<Self> {
        (took_input == Some(self.id)).then_some(self)
    }
}

/// The word an app act's answer names the window it acted in by, in its
/// `action` (`id`, `pid`): the window the helper checked the act against
/// when it acted, whatever window the look after the act lands on.
pub const ACTED_WINDOW: &str = "actedWindow";

/// What a request that acts did, read off what was sent and what the helper
/// answered (I-t123731-rc1); `None` for one that does not act. `background`:
/// whether it went to the helper asking for the background road. A window
/// moved, sized, minimized, zoomed or closed by its number touches that
/// window alone, and its answer names it and its owner (`windowId`, `pid`);
/// bringing one to the front takes the person's front app. An app's act
/// names the window it went to (`action.actedWindow`), and went no further
/// than it when it was sent for the background and the helper did not
/// borrow the desktop instead (`action.borrowedDesktop`).
#[must_use]
pub fn acted(method: &str, background: bool, answer: &Value) -> Option<Acted> {
    if method == "windowAction" {
        let action = answer.get("action")?.as_str()?;
        return Some(Acted {
            in_its_window: action != "focus",
            window: acted_window(answer.get("windowId"), answer.get("pid")),
        });
    }
    let acts = ComputerMethod::ALL
        .iter()
        .any(|known| known.provider_name() == Some(method) && known.acts());
    if !acts {
        return None;
    }
    let action = answer.get("action");
    let named = action.and_then(|action| action.get(ACTED_WINDOW));
    let borrowed =
        action.and_then(|action| action.get("borrowedDesktop")) == Some(&Value::Bool(true));
    Some(Acted {
        in_its_window: background && !borrowed,
        window: acted_window(
            named.and_then(|named| named.get("id")),
            named.and_then(|named| named.get("pid")),
        ),
    })
}

/// A window an answer names by its number and its owner, both positive.
fn acted_window(id: Option<&Value>, pid: Option<&Value>) -> Option<ActedWindow> {
    Some(ActedWindow {
        id: id?.as_u64().filter(|id| *id > 0)?,
        pid: pid?.as_u64().filter(|pid| *pid > 0)?,
    })
}

#[must_use]
pub fn unavailable() -> ProviderError {
    ProviderError::new(
        REQUIRES_FOREGROUND,
        "this operation has no supported background path; no desktop or clipboard fallback was attempted. Hand it to the person, or add --foreground only if they asked for this on their desktop; a caller without it hands the step to them",
    )
}

pub fn validate(method: &str, params: &Map<String, Value>) -> Result<(), ProviderError> {
    if params
        .get(BACKGROUND)
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(ProviderError::invalid_argument(
            "background must be a boolean",
        ));
    }
    if !requested(params) {
        return Ok(());
    }
    if params.get("restoreWindow") == Some(&Value::Bool(true)) {
        return Err(ProviderError::invalid_argument(
            "background cannot restore or activate a window",
        ));
    }
    if !METHODS.contains(&method) {
        return Err(unavailable());
    }
    if method != "openTarget"
        && params
            .get("app")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(ProviderError::invalid_argument(
            "background requires an explicit target app",
        ));
    }
    Ok(())
}

pub fn require_support(supported: bool, params: &Map<String, Value>) -> Result<(), ProviderError> {
    if requested(params) && !supported {
        return Err(ProviderError::new(
            error_code::UNSUPPORTED_CAPABILITY,
            "the provider has not advertised background input; update it or choose an explicit foreground workflow",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command(words: &[&str]) -> ComputerCommand {
        crate::computer_use::parse_command(
            &words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    #[test]
    fn omitted_background_cannot_grant_an_agent_shared_desktop_access() {
        for words in [
            vec!["click", "--app", "Fixture", "--element-index", "3"],
            vec![
                "set-value",
                "--app",
                "Fixture",
                "--element-index",
                "3",
                "--value",
                "fixture",
            ],
            vec!["click", "--mark", "1", "--look", "fixture"],
            vec!["observe", "--app", "Fixture", "--marks"],
            vec!["get-app-state", "--app", "Fixture"],
            vec!["walk", "--app", "Fixture", "--goal", "Read a label"],
        ] {
            let mut asked = command(&words);
            InputPolicy::default().prepare(&mut asked).unwrap();
            assert_eq!(asked.params[BACKGROUND], true, "{words:?}");
        }
    }

    #[test]
    fn background_policy_stops_preparation_and_indirect_shared_input() {
        for words in [
            vec!["get-app-state", "--app", "Fixture", "--restore-window"],
            vec!["window-focus", "--id", "1"],
            vec!["activate", "--app", "Fixture"],
            vec!["run", "--program", "fixture"],
            vec!["mouse-click", "--x", "1", "--y", "1"],
            vec!["key", "--key", "return"],
            vec!["clipboard-write", "--text", "fixture"],
            vec!["permissions", "--id", "accessibility"],
        ] {
            let mut asked = command(&words);
            assert_eq!(
                InputPolicy::Background
                    .prepare(&mut asked)
                    .unwrap_err()
                    .code,
                REQUIRES_FOREGROUND,
                "{words:?}"
            );
        }
    }

    #[test]
    fn provider_admission_cannot_bypass_the_command_policy() {
        for method in [
            "mouseClick",
            "activateApp",
            "windowAction",
            "clipboardWrite",
            "reflexStart",
            "unknown",
        ] {
            assert_eq!(
                InputPolicy::Background
                    .prepare_provider(method, json!({}))
                    .unwrap_err()
                    .code,
                REQUIRES_FOREGROUND
            );
        }
        let params =
            json!({"app":"Fixture", "elementIndex":3, "background":false, "pin":{"value":"kept"}});
        let prepared = InputPolicy::Background
            .prepare_provider("click", params)
            .unwrap();
        assert_eq!(prepared[BACKGROUND], true);
        assert_eq!(prepared["pin"], json!({"value":"kept"}));
    }

    #[test]
    fn read_only_and_stop_paths_remain_available_without_shared_access() {
        for method in [
            "handshake",
            "listApps",
            "listAllWindows",
            "screenshotDesktop",
            "status",
            "stop",
            "reflexStop",
            super::super::eye::START_METHOD,
            super::super::eye::CHANGES_METHOD,
            super::super::eye::FRAME_METHOD,
        ] {
            assert_eq!(
                InputPolicy::Background
                    .prepare_provider(method, json!({}))
                    .unwrap(),
                json!({})
            );
        }
        for words in [
            vec!["status"],
            vec!["stop"],
            vec!["permissions"],
            vec!["wait", "--ms", "1"],
        ] {
            InputPolicy::Background
                .prepare(&mut command(&words))
                .unwrap();
        }
    }

    #[test]
    fn only_explicit_person_policy_keeps_the_shared_desktop_road() {
        let original = command(&["get-app-state", "--app", "Fixture", "--restore-window"]);
        let mut allowed = original.clone();
        InputPolicy::SharedDesktop.prepare(&mut allowed).unwrap();
        assert_eq!(allowed, original);
        assert_eq!(
            InputPolicy::SharedDesktop.within(InputPolicy::Background),
            InputPolicy::Background
        );
        assert_eq!(
            serde_json::to_value(InputPolicy::default()).unwrap(),
            "background"
        );
        assert!(serde_json::from_value::<InputPolicy>(json!("unknown")).is_err());
    }

    #[test]
    fn opening_an_app_or_a_page_comes_up_behind_under_either_policy() {
        for words in [
            vec!["launch", "--app", "Fixture"],
            vec!["open", "--url", "https://example.test"],
        ] {
            let mut background = command(&words);
            InputPolicy::Background.prepare(&mut background).unwrap();
            assert_eq!(background.params[BACKGROUND], true, "background {words:?}");
            // The shared policy's two roads are named, not read off this host: Windows has
            // no background road, so there the request goes on untouched to the shared desktop.
            let mut shared = command(&words);
            prepare_shared(&mut shared, true).unwrap();
            assert_eq!(shared.params[BACKGROUND], true, "shared {words:?}");
            let mut roadless = command(&words);
            prepare_shared(&mut roadless, false).unwrap();
            assert_eq!(roadless, command(&words), "no background road {words:?}");
        }
    }

    #[test]
    fn the_person_who_allows_the_desktop_still_gets_the_background_and_no_borrow() {
        let mut asked = command(&["click", "--app", "Fixture", "--element-index", "3"]);
        prepare_shared(&mut asked, true).unwrap();
        assert_eq!(asked.params[BACKGROUND], true);
        assert!(asked.params.get(MAY_BORROW_DESKTOP).is_none());
    }

    /// An app's own action on one of its controls — closing the menu it
    /// opened, confirming a field, stepping a value — takes nothing of the
    /// person's: it goes to its window under either policy, and the helper
    /// refuses the actions that raise a window or open a menu (t-125705).
    #[test]
    fn an_apps_own_action_on_its_control_stays_in_its_window_under_either_policy() {
        let words = [
            "perform-secondary-action",
            "--app",
            "Fixture",
            "--element-index",
            "1",
            "--action",
            "cancel",
        ];
        let mut background = command(&words);
        InputPolicy::Background.prepare(&mut background).unwrap();
        assert_eq!(background.params[BACKGROUND], true);
        let mut shared = command(&words);
        prepare_shared(&mut shared, true).unwrap();
        assert_eq!(shared.params[BACKGROUND], true);
        assert!(shared.params.get(MAY_BORROW_DESKTOP).is_none());
        // Asked of the desktop, it goes there as it did.
        let mut said = command(&[&words[..], &["--foreground"]].concat());
        let before = said.params.clone();
        prepare_shared(&mut said, true).unwrap();
        assert_eq!(said.params, before);
        assert!(said.params.get(BACKGROUND).is_none());
        // It names its app, as every act in one app's window does.
        assert!(
            crate::computer_use::parse_command(
                &[words[0], words[3], words[4], words[5], words[6]]
                    .iter()
                    .map(|word| (*word).to_owned())
                    .collect::<Vec<_>>()
            )
            .is_err()
        );
    }

    #[test]
    fn nothing_takes_the_desktop_unless_the_call_says_so() {
        for words in [
            vec![
                "drag", "--app", "Fixture", "--from-x", "1", "--from-y", "1", "--to-x", "2",
                "--to-y", "2",
            ],
            vec!["mouse-click", "--x", "1", "--y", "1"],
            vec!["mouse-move", "--x", "1", "--y", "1"],
            vec!["key", "--key", "tab"],
            vec!["type", "--text", "hello"],
            vec!["activate", "--app", "Fixture"],
            vec!["hold-key", "--key", "shift", "--ms", "10"],
            vec!["clipboard-write", "--text", "x"],
        ] {
            let error = prepare_shared(&mut command(&words), true).unwrap_err();
            assert_eq!(error.code, REQUIRES_FOREGROUND, "{words:?}");
            assert!(
                error.message.contains("--foreground only if they asked"),
                "{words:?}: {}",
                error.message
            );
            // The flag the refusal names is one the verb takes.
            let mut said_words = words.clone();
            said_words.push("--foreground");
            let mut said = command(&said_words);
            assert_eq!(said.params["foreground"], true, "{words:?}");
            let before = said.params.clone();
            prepare_shared(&mut said, true).unwrap();
            assert_eq!(said.params, before, "{words:?}");
        }
    }

    #[test]
    fn moving_a_window_takes_nothing_of_the_persons_but_focusing_one_does() {
        for action in ["move", "resize", "minimize", "zoom", "close"] {
            let params = json!({"action": action, "windowId": 1, "x": 0, "y": 0});
            InputPolicy::SharedDesktop
                .prepare_provider("windowAction", params)
                .unwrap_or_else(|error| panic!("{action}: {error:?}"));
        }
        let focus = InputPolicy::SharedDesktop
            .prepare_provider("windowAction", json!({"action": "focus", "windowId": 1}));
        // Without a background road the request goes on to the shared desktop as asked, as `prepare_shared` says.
        if HAS_BACKGROUND_ROAD {
            assert_eq!(focus.unwrap_err().code, REQUIRES_FOREGROUND);
        } else {
            focus.unwrap();
        }
    }

    #[test]
    fn a_press_by_words_the_call_asked_for_goes_on() {
        let mut press = ComputerCommand {
            method: ComputerMethod::MouseClick,
            params: json!({"x": 1, "y": 1, "mayBorrowDesktop": true, "borrowApp": "Fixture"}),
            json: true,
        };
        let before = press.params.clone();
        prepare_shared(&mut press, true).unwrap();
        assert_eq!(press.params, before);
    }

    #[test]
    fn what_takes_nothing_of_the_persons_goes_on_as_it_did() {
        for words in [
            vec!["observe", "--app", "Fixture"],
            vec!["screenshot"],
            vec!["run", "--program", "/bin/echo"],
            vec!["quit", "--app", "Fixture"],
            vec!["batch", "--commands", "[[\"wait\",\"--ms\",\"1\"]]"],
        ] {
            prepare_shared(&mut command(&words), true).unwrap();
        }
    }

    #[test]
    fn an_agent_can_ask_for_the_desktop_at_once_or_for_the_background_only() {
        let mut foreground = command(&[
            "click",
            "--app",
            "Fixture",
            "--element-index",
            "3",
            "--foreground",
        ]);
        prepare_shared(&mut foreground, true).unwrap();
        assert!(foreground.params.get(BACKGROUND).is_none());
        assert!(foreground.params.get(MAY_BORROW_DESKTOP).is_none());
        let mut background = command(&[
            "click",
            "--app",
            "Fixture",
            "--element-index",
            "3",
            "--background",
        ]);
        prepare_shared(&mut background, true).unwrap();
        assert_eq!(background.params[BACKGROUND], true);
        assert!(background.params.get(MAY_BORROW_DESKTOP).is_none());
    }

    /// The road a step's words send it down — what its evidence goes by
    /// when no act of it was heard (I-t123731-rc1). An app's act and an app
    /// it opens go to the background under either policy; a step that takes
    /// the desk does not, nor a look at the screen; and where there is no
    /// background road, nothing does.
    #[test]
    fn a_step_that_ran_says_whether_it_went_down_the_background_road() {
        for words in [
            vec!["click", "--app", "Fixture", "--element-index", "3"],
            vec!["type-text", "--app", "Fixture", "--text", "hello"],
            vec!["press-key", "--app", "Fixture", "--key", "Return"],
            vec!["click", "--mark", "1", "--look", "fixture"],
            vec!["launch", "--app", "Fixture"],
        ] {
            let ran = command(&words);
            assert_eq!(
                went_in_the_background(&ran),
                HAS_BACKGROUND_ROAD,
                "{words:?}"
            );
            // The shared policy marks it the same way.
            let mut shared = ran.clone();
            prepare_shared(&mut shared, HAS_BACKGROUND_ROAD).unwrap();
            assert_eq!(
                requested(shared.params.as_object().unwrap()),
                HAS_BACKGROUND_ROAD,
                "shared {words:?}"
            );
        }
        for words in [
            vec![
                "click",
                "--app",
                "Fixture",
                "--element-index",
                "3",
                "--foreground",
            ],
            vec!["get-app-state", "--app", "Fixture", "--restore-window"],
            vec!["mouse-click", "--x", "1", "--y", "1", "--foreground"],
            vec!["key", "--key", "tab", "--foreground"],
            vec!["quit", "--app", "Fixture"],
            vec!["screenshot"],
        ] {
            assert!(!went_in_the_background(&command(&words)), "{words:?}");
        }
    }

    /// Why a step's evidence goes by its act, not its words (I-t123731-rc1,
    /// review t-128744 §1): under the shared desktop a mark's press is sent
    /// without the `--foreground` its words carried — the mark gives the
    /// press its app and its window, and nothing of the desk — so it goes to
    /// the background; and a click asked `--background --foreground` keeps
    /// the background road it asked for, the one the helper reads.
    #[test]
    fn a_press_may_go_to_the_background_whatever_its_words_say_of_the_desk() {
        let press = json!({
            "app": "pid:42",
            "windowId": 7,
            "elementIndex": 3,
            "noScreenshot": true,
            "deferObservation": true,
        });
        let sent = InputPolicy::SharedDesktop
            .prepare_provider("click", press)
            .unwrap();
        assert_eq!(
            requested(sent.as_object().unwrap()),
            HAS_BACKGROUND_ROAD,
            "a mark's press: {sent}"
        );
        let mut both = command(&[
            "click",
            "--app",
            "Fixture",
            "--element-index",
            "3",
            "--background",
            "--foreground",
        ]);
        InputPolicy::SharedDesktop.prepare(&mut both).unwrap();
        assert_eq!(both.params[BACKGROUND], true);
        assert!(!went_in_the_background(&both), "its words say the desk");
    }

    /// What an act did is read off what was sent and what the helper
    /// answered (I-t123731-rc1): an app's act sent for the background went
    /// no further than the window it names — the one it was checked against
    /// when it acted, not the one the look after it shows — unless the
    /// helper borrowed the desktop instead, and one sent without it took the
    /// desk; a window moved by its number touched that window alone, while
    /// bringing one to the front took the person's front app; an app opened
    /// behind names no window; and a look is not an act.
    #[test]
    fn what_an_act_did_is_read_off_what_was_sent_and_what_the_helper_answered() {
        let pressed = json!({
            "action": {
                "targetWindowId": 19,
                ACTED_WINDOW: { "id": 7, "pid": 501 },
                "background": { "requested": true },
            },
            "snapshot": { "window": { "id": 19 } },
        });
        let seven = Some(ActedWindow { id: 7, pid: 501 });
        assert_eq!(
            acted("click", true, &pressed),
            Some(Acted {
                in_its_window: true,
                window: seven,
            }),
            "the window the act went to, not the one the look after it shows"
        );
        assert_eq!(
            acted("click", false, &pressed),
            Some(Acted {
                in_its_window: false,
                window: seven,
            }),
            "sent without the background road: it took the desk"
        );
        let borrowed = json!({
            "action": { ACTED_WINDOW: { "id": 7, "pid": 501 }, "borrowedDesktop": true },
        });
        assert_eq!(
            acted("click", true, &borrowed).map(|acted| acted.in_its_window),
            Some(false),
            "the helper borrowed the desktop instead"
        );
        let window = |action: &str| json!({ "windowId": 17, "pid": 501, "action": action });
        assert_eq!(
            acted("windowAction", false, &window("move")),
            Some(Acted {
                in_its_window: true,
                window: Some(ActedWindow { id: 17, pid: 501 }),
            }),
            "a window moved by its number, with or without the background road"
        );
        assert_eq!(
            acted("windowAction", false, &window("focus")).map(|acted| acted.in_its_window),
            Some(false),
            "a window brought to the front"
        );
        assert_eq!(
            acted("launchApp", true, &json!({ "launched": true })),
            Some(Acted {
                in_its_window: true,
                window: None,
            }),
            "an app opened behind names no window"
        );
        assert_eq!(acted("getAppState", true, &pressed), None, "a look");
        let unnumbered = json!({ "action": { ACTED_WINDOW: { "id": 0, "pid": 501 } } });
        assert_eq!(
            acted("click", true, &unnumbered).and_then(|acted| acted.window),
            None,
            "no window is numbered 0"
        );
    }

    #[test]
    fn a_provider_with_no_background_road_goes_on_the_shared_desktop() {
        let mut asked = command(&["click", "--app", "Fixture", "--element-index", "3"]);
        let before = asked.params.clone();
        prepare_shared(&mut asked, false).unwrap();
        assert_eq!(asked.params, before);
    }

    #[test]
    fn several_apps_at_once_are_admitted_step_by_step_under_background_only() {
        let mut asked = command(&[
            "parallel",
            "--commands",
            r#"[[["click","--app","A","--element-index","1"]],[["click","--app","B","--element-index","2"]]]"#,
        ]);
        InputPolicy::Background.prepare(&mut asked).unwrap();
    }

    #[test]
    fn background_only_refuses_an_agent_asking_for_the_desktop() {
        let mut asked = command(&[
            "click",
            "--app",
            "Fixture",
            "--element-index",
            "3",
            "--foreground",
        ]);
        assert_eq!(
            InputPolicy::Background
                .prepare(&mut asked)
                .unwrap_err()
                .code,
            REQUIRES_FOREGROUND
        );
    }

    #[test]
    fn native_providers_share_the_background_admission_table() {
        for line in include_str!("cases/background.tsv").lines() {
            let row = line.split('\t').collect::<Vec<_>>();
            let mut params = json!({"background": true, "restoreWindow": row[1] == "true"});
            if row[2] == "true" {
                params["app"] = "Fixture".into();
            }
            let result = validate(row[0], params.as_object().unwrap());
            assert_eq!(
                result.err().map_or("ok".to_owned(), |error| error.code),
                row[3],
                "{line}"
            );
        }
    }

    #[test]
    fn background_cannot_escape_to_global_input_or_activation() {
        let params = json!({"background": true, "app": "Fixture"});
        for method in [
            "drag",
            "activateApp",
            "windowAction",
            "mouseClick",
            "mouseMove",
            "clipboardWrite",
            "reflexStart",
            "unknown",
        ] {
            assert_eq!(
                validate(method, params.as_object().unwrap())
                    .unwrap_err()
                    .code,
                REQUIRES_FOREGROUND
            );
        }
        for method in METHODS {
            assert!(validate(method, params.as_object().unwrap()).is_ok());
        }
    }

    #[test]
    fn background_is_explicit_scoped_and_fail_closed_on_old_providers() {
        for params in [
            json!({"background": "true"}),
            json!({"background": true}),
            json!({"background": true, "app": "Fixture", "restoreWindow": true}),
        ] {
            assert!(validate("getAppState", params.as_object().unwrap()).is_err());
        }
        let params = json!({"background": true, "app": "Fixture"});
        assert!(require_support(false, params.as_object().unwrap()).is_err());
        assert!(require_support(true, params.as_object().unwrap()).is_ok());
        assert!(validate("mouseClick", &Map::new()).is_ok());
        assert!(require_support(false, &Map::new()).is_ok());
    }
}
