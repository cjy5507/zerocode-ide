use std::cell::RefCell;
use std::path::PathBuf;

use super::*;
use serde_json::json;

thread_local! {
    pub(super) static STATE_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

struct Root(tempfile::TempDir);

impl Root {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        STATE_ROOT.with(|state| *state.borrow_mut() = Some(root.path().canonicalize().unwrap()));
        Self(root)
    }

    fn read(&self) -> Value {
        std::fs::read(self.0.path().join("computer-use-tally.json")).ok()
            .and_then(|text| serde_json::from_slice(&text).ok()).unwrap_or_else(|| json!({}))
    }
}

impl Drop for Root {
    fn drop(&mut self) { STATE_ROOT.with(|state| *state.borrow_mut() = None); }
}

fn call(input: &Value, answer: &Value, error: Option<&str>) -> Result<ToolExecutionResult, ToolError> {
    let ctx = crate::tests::tool_context_outside_cwd_windows();
    run(input, &ctx, || {
        receipt(answer);
        match error {
            Some(error) => Err(ToolError::Execution(error.to_string())),
            None => Ok(ToolExecutionResult::from(answer.to_string())),
        }
    })
}

#[test]
fn the_photo_refusal_and_the_explicit_chrome_call_are_counted_once_each() {
    let root = Root::new();
    let refused = call(&json!({"action":"left_click", "app":"Google Chrome", "element_index":194}),
        &json!({"ok":false,"error":{"code":"browser_requires_work_browser","message":"use zerocode-browser open <url>"}}),
        Some("browser_requires_work_browser: use zerocode-browser open <url>"));
    assert!(refused.is_err());
    let after_refusal = root.read();
    eprintln!("photo tally fixture: {after_refusal}");
    assert_eq!(after_refusal["browserRouteRefusals"], 1, "the photo's route refusal is counted once");
    assert_eq!(after_refusal["refusals"]["browser_requires_work_browser"], 1);
    let accepted = call(&json!({"action":"left_click", "browser_chrome":true}),
        &json!({"ok":true,"result":{"snapshot":{"app":{"bundleId":"org.example.browser"}}}}), None);
    assert!(accepted.is_ok());
    let tally = root.read();
    eprintln!("chrome tally fixture: {tally}");
    assert_eq!(tally["calls"], 2);
    assert_eq!(tally["browserChromeCalls"], 1);
    assert_eq!(tally["byBundle"]["unknown"]["calls"], 1);
    assert_eq!(tally["byBundle"]["org.example.browser"]["calls"], 1);
}

#[test]
fn the_sixty_fifth_bundle_and_thirty_third_refusal_kind_use_other() {
    let root = Root::new();
    for number in 0..65 {
        let kind = format!("fixture_{number}");
        let _ = call(&json!({"action":"left_click"}),
            &json!({"ok":false, "result":{"app":{"bundleId":format!("org.example.browser{number}")}},"error":{"code":kind}}),
            Some(&format!("{kind}: fixture refusal")));
    }
    let tally = root.read();
    assert_eq!(tally["byBundle"]["other"]["calls"], 1, "the 65th bundle has one bounded overflow bucket");
    assert_eq!(tally["byBundle"].as_object().map(serde_json::Map::len), Some(65));
    assert_eq!(tally["refusals"]["other"], 33);
    assert_eq!(tally["refusals"].as_object().map(serde_json::Map::len), Some(33));
}

#[test]
fn an_unreadable_classification_passes_and_counts_unknown_once_for_input_only() {
    let root = Root::new();
    let input = json!({"action":"left_click"});
    let answer = json!({"ok":true,"result":{"browserClassificationUnknown":true}});
    assert!(call(&input, &answer, None).is_ok());
    assert_eq!(root.read()["browserClassificationUnknown"], 1, "a passed input with unreadable native declarations is counted once");
    assert!(call(&json!({"action":"screenshot"}), &answer, None).is_ok());
    assert_eq!(root.read()["browserClassificationUnknown"], 1, "read-only calls do not classify browsers");
}

#[test]
fn a_corrupt_tally_does_not_change_the_computer_result() {
    let root = Root::new();
    std::fs::write(root.0.path().join("computer-use-tally.json"), "not JSON").unwrap();
    let answer = json!({"ok":true,"result":{"done":true}});
    let result = call(&json!({"action":"left_click"}), &answer, None).unwrap();
    assert_eq!(result.text, answer.to_string());
    assert_eq!(root.read()["calls"], 1, "a corrupt tally restarts without altering the call");
}

#[test]
fn the_saved_keys_and_values_exclude_request_text_and_unbounded_error_words() {
    let root = Root::new();
    let _ = call(&json!({"action":"type","app":"Unstored app name", "text":"Unstored page text", "url":"https://example.invalid/private"}),
        &json!({"ok":false,"error":{"code":"NOT A CODE","message":"Unstored error words"}, "result":{"app":{"name":"Unstored app name","bundleId":"/Users/dev/not-a-bundle"}}}),
        Some("NOT A CODE: Unstored error words"));
    let tally = root.read();
    assert_eq!(tally["refusals"]["other"], 1, "only bounded refusal-code tokens reach the file");
    let mut keys = tally.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();
    let mut expected = vec!["since", "calls", "refusals", "byBundle", "browserRouteRefusals", "browserChromeCalls", "browserClassificationUnknown"];
    expected.sort_unstable();
    assert_eq!(keys, expected);
    let text = tally.to_string();
    for forbidden in ["Unstored", "https://", "/Users/"] { assert!(!text.contains(forbidden), "request data reached the tally: {text}"); }
}

#[cfg(unix)]
#[test]
fn a_tally_symlink_does_not_change_the_call_or_its_target() {
    let root = Root::new();
    let target = root.0.path().join("unchanged.txt");
    std::fs::write(&target, "unchanged").unwrap();
    std::os::unix::fs::symlink(&target, root.0.path().join("computer-use-tally.json")).unwrap();
    let result = call(&json!({"action":"left_click"}), &json!({"ok":true}), None);
    assert!(result.is_ok(), "tally failures cannot change the Computer result");
    assert_eq!(std::fs::read_to_string(target).unwrap(), "unchanged");
}

#[test]
fn normalized_input_actions_keep_the_unknown_classification_count() {
    let root = Root::new();
    let answer = json!({"ok":true,"result":{"browserClassificationUnknown":true}});
    assert!(call(&json!({"action":"left_click"}), &answer, None).is_ok());
    assert!(call(&json!({"action":" left_click "}), &answer, None).is_ok());
    assert_eq!(root.read()["browserClassificationUnknown"], 2, "the tally uses the same normalized action as Computer execution");
}

#[cfg(unix)]
#[test]
fn error_envelopes_with_or_without_the_optional_fact_keep_the_same_tool_error() {
    use std::os::unix::fs::PermissionsExt;
    let root = Root::new();
    let shim = root.0.path().join("zerocode-computer");
    let road = crate::computer_tools::ComputerRoad { program: shim.clone() };
    let ctx = crate::tests::tool_context_outside_cwd_windows();
    let old = r#"#!/bin/sh
printf '%s\n' '{"ok":false,"error":{"code":"element_not_found","message":"fixture helper refusal"}}'
exit 1
"#;
    let new = r#"#!/bin/sh
printf '%s\n' '{"ok":false,"error":{"code":"element_not_found","message":"fixture helper refusal"},"browserClassificationUnknown":true}'
exit 1
"#;
    for body in [old, new] {
        std::fs::write(&shim, body).unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = crate::computer_tools::run_computer_execution(
            &json!({"action":"left_click", "app":"Fixture", "element_index":194}), &ctx, &road);
        assert!(matches!(&result, Err(ToolError::Execution(message)) if message == "element_not_found: fixture helper refusal"),
            "the optional reply field must not change parsing or the original refusal");
    }
    let tally = root.read();
    assert_eq!(tally["calls"], 2);
    assert_eq!(tally["refusals"]["element_not_found"], 2);
    assert_eq!(tally["browserClassificationUnknown"], 1, "the failed input counts the fact once, before the tool converts its error");
}
