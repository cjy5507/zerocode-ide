use std::cell::RefCell;

use serde_json::{Value, json};
use zerocode_core::computer_use::ComputerCommand;
use zerocode_core::computer_use_protocol::{answer_envelope, error_code};

use crate::computer_use::{self, ComputerUseError};

const ROUTE_REFUSAL: &str = "browser_requires_work_browser";

struct Fixture {
    browser: Option<bool>,
    scene: bool,
    helper_calls: Vec<String>,
}

thread_local! {
    static FIXTURE: RefCell<Option<Fixture>> = const { RefCell::new(None) };
}

struct Installed;

impl Installed {
    fn new(browser: Option<bool>, scene: bool) -> Self {
        FIXTURE.with(|fixture| *fixture.borrow_mut() = Some(Fixture {
            browser, scene, helper_calls: Vec::new(),
        }));
        Self
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        FIXTURE.with(|fixture| *fixture.borrow_mut() = None);
    }
}

pub(super) fn classification(params: &Value) -> Option<Option<bool>> {
    FIXTURE.with(|installed| {
        let installed = installed.borrow();
        let installed = installed.as_ref()?;
        let Some(default_browser) = installed.browser else { return Some(None); };
        let fixture: Value = serde_json::from_str(include_str!("../../../tests/fixtures/browser-routing.json")).unwrap();
        let handlers: [Vec<String>; 4] = serde_json::from_value(fixture["handlers"].clone()).unwrap();
        let name = params.get("app").and_then(Value::as_str).unwrap_or(if default_browser { "Google Chrome" } else { "Calculator" });
        let app = fixture["apps"].as_array().unwrap().iter().find(|app| app["app"].as_str() == Some(name) || app["bundleId"].as_str() == Some(name));
        Some(app.and_then(|app| super::registered_browser(app["bundleId"].as_str().unwrap(), &handlers)))
    })
}

pub(crate) fn helper(method: &str, _params: &Value) -> Option<Result<Value, ComputerUseError>> {
    FIXTURE.with(|fixture| {
        let mut fixture = fixture.borrow_mut();
        let fixture = fixture.as_mut()?;
        fixture.helper_calls.push(method.to_string());
        Some(if fixture.scene && method == "click" {
            let (code, message) = if fixture.helper_calls.len() == 1 {
                (error_code::ELEMENT_NOT_FOUND, "element 194 changed since the last snapshot")
            } else {
                ("requires_foreground", "background input pauses while the person uses the target app")
            };
            Err(ComputerUseError::new(code, message))
        } else {
            Ok(json!({}))
        })
    })
}

fn calls() -> usize {
    FIXTURE.with(|fixture| fixture.borrow().as_ref().unwrap().helper_calls.len())
}

struct NoHandoff;

impl computer_use::enter::Typer for NoHandoff {
    fn computer(&mut self, _: &ComputerCommand) -> Result<Value, ComputerUseError> { unreachable!("no handoff") }
    fn browser(&mut self, _: &[String], _: Option<u64>) -> Result<(), ComputerUseError> { unreachable!("no handoff") }
    fn emulator(&mut self, _: &[String]) -> Result<(), ComputerUseError> { unreachable!("no handoff") }
}

fn answer(argv: &[&str]) -> Value {
    let argv = argv.iter().copied().chain(["--json"]).map(str::to_string).collect::<Vec<_>>();
    let answer = crate::agent_tools_runtime::answer_computer_command_with(
        &argv, None, computer_use::confirm::Asking::Person, None, &mut NoHandoff,
    );
    answer_envelope(&answer.stdout, &answer.stderr).unwrap_or(Value::Null)
}

fn one_hand() -> std::sync::MutexGuard<'static, ()> {
    let hand = crate::tests::computer_desktop_wait::ONE_HAND.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    computer_use::guard::lift();
    hand
}

#[test]
fn the_photo_scene_routes_once_before_either_helper_failure() {
    let _hand = one_hand();
    let _policy = computer_use::input_policy::enter(computer_use::input_policy::InputPolicy::SharedDesktop);
    let _fixture = Installed::new(Some(true), true);
    assert_eq!(classification(&json!({"app":"Google Chrome"})), Some(Some(true)));
    let mut road_refusals = 0;
    let mut said = Value::Null;
    for _ in 0..2 {
        said = answer(&["click", "--app", "Google Chrome", "--element-index", "194", "--background"]);
        if said.pointer("/error/code").and_then(Value::as_str) == Some(ROUTE_REFUSAL) {
            road_refusals += 1;
            break;
        }
    }
    eprintln!("photo fixture: helper failures={} calls; road refusals={} calls", calls(), road_refusals);
    assert_eq!((calls(), road_refusals), (0, 1), "the photo must receive the work-browser instructions before any helper request: {said}");
    let message = said.pointer("/error/message").and_then(Value::as_str).unwrap_or_default();
    for words in ["zerocode-browser open <url>", "read", "marks", "click", "type", "fill", "does not carry", "login", "take", "sign in", "browser_chrome", "menus", "tabs", "extension UI"] {
        assert!(message.contains(words), "the route refusal must explain {words:?}: {message}");
    }
}

#[test]
fn read_only_browser_looks_still_reach_the_helper() {
    let _hand = one_hand();
    let _policy = computer_use::input_policy::enter(computer_use::input_policy::InputPolicy::SharedDesktop);
    for argv in [vec!["screenshot"], vec!["observe", "--app", "Google Chrome"], vec!["list-windows", "--app", "Google Chrome"], vec!["zoom", "--region", "0,0,20,20"], vec!["read", "--app", "Google Chrome", "--ocr"]] {
        let _fixture = Installed::new(Some(true), false);
        let said = answer(&argv);
        assert!(calls() > 0, "a read-only look must reach the helper: {argv:?}: {said}");
        assert_ne!(said.pointer("/error/code").and_then(Value::as_str), Some(ROUTE_REFUSAL));
    }
}

#[test]
fn the_explicit_browser_chrome_claim_allows_a_chrome_click() {
    let _hand = one_hand();
    let _policy = computer_use::input_policy::enter(computer_use::input_policy::InputPolicy::SharedDesktop);
    let _fixture = Installed::new(Some(true), false);
    let said = answer(&["click", "--app", "Google Chrome", "--element-index", "194", "--browser-chrome"]);
    assert_eq!(said["ok"], true, "the explicit chrome claim must preserve the desktop path: {said}");
    assert_eq!(calls(), 1);
}

#[test]
fn calculator_and_iterm_input_keep_the_existing_path() {
    let _hand = one_hand();
    let _policy = computer_use::input_policy::enter(computer_use::input_policy::InputPolicy::SharedDesktop);
    for app in ["Calculator", "iTerm2"] {
        let _fixture = Installed::new(Some(false), false);
        let said = answer(&["click", "--app", app, "--element-index", "1"]);
        assert_eq!(said["ok"], true, "a non-browser must keep its existing path: {said}");
        assert_eq!(calls(), 1);
    }
}

#[test]
fn an_unreadable_browser_registration_preserves_the_existing_path() {
    let _hand = one_hand();
    let _policy = computer_use::input_policy::enter(computer_use::input_policy::InputPolicy::SharedDesktop);
    let _fixture = Installed::new(None, false);
    let said = answer(&["click", "--app", "Google Chrome", "--element-index", "194"]);
    assert_eq!(said["ok"], true, "a missing platform signal must preserve the previous behavior: {said}");
    assert_eq!(calls(), 1);
    assert_eq!(said["result"]["browserClassificationUnknown"], true, "the successful answer carries only the unknown-classification fact");
}

#[test]
fn provider_input_that_bypasses_the_command_handler_is_also_routed() {
    let _hand = one_hand();
    let _policy = computer_use::input_policy::enter(computer_use::input_policy::InputPolicy::SharedDesktop);
    let _fixture = Installed::new(Some(true), false);
    let said = computer_use::call("click", json!({"app":"Google Chrome", "elementIndex":194}));
    assert_eq!(calls(), 0, "direct provider input must be checked before its helper request: {said:?}");
    assert_eq!(said.err().map(|error| error.code), Some(ROUTE_REFUSAL.to_string()));
}

#[test]
fn macos_registration_needs_both_web_schemes_and_both_viewer_types() {
    let fixture: Value = serde_json::from_str(include_str!("../../../tests/fixtures/browser-routing.json")).unwrap();
    let handlers: [Vec<String>; 4] = serde_json::from_value(fixture["handlers"].clone()).unwrap();
    for app in fixture["apps"].as_array().unwrap() {
        let registered = super::registered_browser(app["bundleId"].as_str().unwrap(), &handlers);
        assert_eq!(registered, app["browser"].as_bool(), "classification must use native declarations, including unfamiliar browsers: {app}");
    }
}
