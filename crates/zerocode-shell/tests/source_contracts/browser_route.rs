//! Composite Computer calls must carry the explicit browser-chrome claim into their steps.

const RUNTIME: &str = include_str!("../../src/agent_tools_runtime.rs");

#[test]
fn composite_computer_requests_scope_browser_routing_before_their_first_step() {
    let (_, composite) = RUNTIME
        .split_once("if let Some(mut command) = parsed")
        .expect("the composite command branch");
    let (admission, _) = composite
        .split_once("let answer = if command.method")
        .expect("the first composite dispatch");
    assert!(
        admission.contains("computer_use::browser_route::command(&command)"),
        "composite Computer calls must preflight the target and scope browser_chrome before observations or generated input"
    );
}
