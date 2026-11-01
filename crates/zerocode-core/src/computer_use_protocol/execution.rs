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
const HAS_BACKGROUND_ROAD: bool = cfg!(target_os = "macos");

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
        let scoped = command
            .method
            .provider_name()
            .is_some_and(|name| METHODS.contains(&name));
        let composite = matches!(
            command.method,
            ComputerMethod::Batch
                | ComputerMethod::Parallel
                | ComputerMethod::RecipeRun
                | ComputerMethod::Walk
        );
        // Opening an app or a page takes no pointer and no focus: it comes up behind.
        if matches!(
            command.method,
            ComputerMethod::Launch | ComputerMethod::Open
        ) {
            params.insert(BACKGROUND.to_owned(), Value::Bool(true));
            return Ok(());
        }
        if command.method.acts() && !scoped && !composite {
            return Err(policy_refusal());
        }
        let has_target = params.contains_key("app") || params.contains_key("mark");
        if scoped && command.method.acts() && !has_target {
            return Err(policy_refusal());
        }
        if has_target
            && (scoped
                || matches!(
                    command.method,
                    ComputerMethod::Observe | ComputerMethod::WaitFor | ComputerMethod::Walk
                ))
        {
            params.insert(BACKGROUND.to_owned(), Value::Bool(true));
        }
        Ok(())
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
        let found = ComputerMethod::ALL
            .iter()
            .copied()
            .find(|candidate| candidate.provider_name() == Some(method));
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

/// The person lets the agent use the shared desktop. It still goes in the
/// background first, so it does not take the pointer from someone working, and
/// borrows the desktop only for what the background road cannot do. An agent that
/// says `--foreground` or `--restore-window` goes to the desktop at once, and one
/// that says `--background` stays there and is told when it cannot.
fn prepare_shared(
    command: &mut ComputerCommand,
    background_road: bool,
) -> Result<(), ProviderError> {
    if !background_road {
        return Ok(());
    }
    let method = command.method;
    let params = command
        .params
        .as_object_mut()
        .ok_or_else(|| ProviderError::invalid_argument("command parameters must be an object"))?;
    if matches!(method, ComputerMethod::Launch | ComputerMethod::Open) {
        if params.get(FOREGROUND) != Some(&Value::Bool(true)) {
            params.insert(BACKGROUND.to_owned(), Value::Bool(true));
        }
        return Ok(());
    }
    if params.get(FOREGROUND) == Some(&Value::Bool(true))
        || params.get("restoreWindow") == Some(&Value::Bool(true))
        || params.get(BACKGROUND) == Some(&Value::Bool(true))
    {
        return Ok(());
    }
    if !params.contains_key("app") && !params.contains_key("mark") {
        return Ok(());
    }
    let scoped = method
        .provider_name()
        .is_some_and(|name| METHODS.contains(&name));
    if scoped
        || matches!(
            method,
            ComputerMethod::Observe | ComputerMethod::WaitFor | ComputerMethod::Walk
        )
    {
        params.insert(BACKGROUND.to_owned(), Value::Bool(true));
    }
    // Only an act inside an app is given back afterwards. Bringing an app up or
    // moving a window is the point of the verb, and giving it back would undo it.
    if method.is_app_act() || method == ComputerMethod::Walk {
        params.insert(MAY_BORROW_DESKTOP.to_owned(), Value::Bool(true));
    }
    Ok(())
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

#[must_use]
pub fn unavailable() -> ProviderError {
    ProviderError::new(
        REQUIRES_FOREGROUND,
        "this operation has no supported background path; no desktop or clipboard fallback was attempted",
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
        for policy in [InputPolicy::Background, InputPolicy::SharedDesktop] {
            for words in [
                vec!["launch", "--app", "Fixture"],
                vec!["open", "--url", "https://example.test"],
            ] {
                let mut asked = command(&words);
                policy.prepare(&mut asked).unwrap();
                assert_eq!(asked.params[BACKGROUND], true, "{policy:?} {words:?}");
            }
        }
    }

    #[test]
    fn the_person_who_allows_the_desktop_gets_background_first_and_a_borrow_after() {
        let mut asked = command(&["click", "--app", "Fixture", "--element-index", "3"]);
        prepare_shared(&mut asked, true).unwrap();
        assert_eq!(asked.params[BACKGROUND], true);
        assert_eq!(asked.params[MAY_BORROW_DESKTOP], true);
    }

    #[test]
    fn a_verb_with_no_background_road_still_may_borrow_the_desktop() {
        let mut asked = command(&[
            "drag", "--app", "Fixture", "--from-x", "1", "--from-y", "1", "--to-x", "2", "--to-y",
            "2",
        ]);
        prepare_shared(&mut asked, true).unwrap();
        assert!(asked.params.get(BACKGROUND).is_none());
        assert_eq!(asked.params[MAY_BORROW_DESKTOP], true);
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

    #[test]
    fn bringing_an_app_up_is_not_given_back_afterwards() {
        let mut asked = command(&["activate", "--app", "Fixture"]);
        prepare_shared(&mut asked, true).unwrap();
        assert!(asked.params.get(MAY_BORROW_DESKTOP).is_none());
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
