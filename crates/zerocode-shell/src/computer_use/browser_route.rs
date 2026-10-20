//! Desktop-browser routing at the window, before any helper request.

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;

use serde_json::Value;
use zerocode_core::computer_use::{ComputerCommand, ComputerMethod};
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;

#[cfg(target_os = "macos")]
mod macos;

thread_local! {
    static CHROME: Cell<bool> = const { Cell::new(false) };
}

/// The explicit claim belongs to this request, including generated OCR and mark input.
pub(crate) struct Scope {
    previous: bool,
    thread_bound: PhantomData<Rc<()>>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        CHROME.with(|active| active.set(self.previous));
    }
}

pub(crate) fn command(command: &ComputerCommand) -> Result<Scope, ComputerUseError> {
    let previous = CHROME.with(|active| {
        let previous = active.get();
        active.set(previous || command.params.get("browserChrome") == Some(&Value::Bool(true)));
        previous
    });
    let scope = Scope { previous, thread_bound: PhantomData };
    check(command.method, &command.params)?;
    Ok(scope)
}

/// Reflex input can call the provider without passing through the CLI command handler.
pub(super) fn provider(method: &str, params: &Value) -> Result<(), ComputerUseError> {
    if let Some(method) = ComputerMethod::ALL.iter().copied().find(|candidate| candidate.provider_name() == Some(method)) {
        check(method, params)?;
    }
    Ok(())
}

fn check(method: ComputerMethod, params: &Value) -> Result<(), ComputerUseError> {
    if !method.acts()
        || matches!(method, ComputerMethod::Run | ComputerMethod::ClipboardWrite | ComputerMethod::Batch | ComputerMethod::RecipeRun)
        || params.get("pane").is_some()
        || params.get("platform").is_some()
        || params.get("browserChrome") == Some(&Value::Bool(true))
        || CHROME.with(Cell::get)
    {
        return Ok(());
    }
    // A mark already stores its app identity; resolving it reads memory, not the helper.
    let marked = params.get("mark").and_then(|_| super::marks::pinned_click(params).ok()).map(|mark| Value::Object(mark.params));
    let target = marked.as_ref().unwrap_or(params);
    if classify(method, target) == Some(true) {
        return Err(ComputerUseError::new(
            error_code::BROWSER_REQUIRES_WORK_BROWSER,
            "Web-page input belongs in the window's work browser: use zerocode-browser open <url>, then read, marks, click, type, and fields/fill. A working tab does not carry the person's external browser login; the person can take the tab over to sign in. Only when the person explicitly requested the browser application's own menus, tabs or extension UI, set browser_chrome: true (CLI --browser-chrome).",
        ));
    }
    Ok(())
}

fn classify(method: ComputerMethod, params: &Value) -> Option<bool> {
    #[cfg(test)]
    if let Some(classified) = tests::classification(params) {
        return classified;
    }
    #[cfg(target_os = "macos")]
    { macos::target_is_browser(method, params) }
    #[cfg(target_os = "windows")]
    { super::windows::target_is_browser(method, params) }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    { let _ = (method, params); None }
}

/// HTTP and HTTPS handlers must also view HTML and XHTML. A missing query is unknown.
#[cfg(any(target_os = "macos", test))]
fn registered_browser(bundle_id: &str, handlers: &[Vec<String>; 4]) -> Option<bool> {
    if bundle_id.is_empty() { return None; }
    Some(handlers.iter().all(|registered| registered.iter().any(|candidate| candidate.eq_ignore_ascii_case(bundle_id))))
}

/// Registered browser commands identify executable paths, not display names.
#[cfg(any(target_os = "windows", test))]
pub(super) fn registered_executable(executable: &str, commands: Option<&[String]>) -> Option<bool> {
    let commands = commands?;
    if commands.is_empty() { return None; }
    let mut readable = false;
    for command in commands {
        let text = command.trim();
        let program = if let Some(quoted) = text.strip_prefix('"') {
            quoted.split_once('"').map(|(program, _)| program)
        } else { text.split_whitespace().next().filter(|program| program.to_ascii_lowercase().ends_with(".exe")) };
        if let Some(program) = program {
            readable = true;
            if program.replace('/', "\\").eq_ignore_ascii_case(&executable.replace('/', "\\")) { return Some(true); }
        }
    }
    readable.then_some(false)
}

#[cfg(test)]
pub(super) mod tests;
