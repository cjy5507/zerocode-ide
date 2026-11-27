//! The tools of the headless core profile (`runtime::HeadlessProfile::Core`):
//! the six a core session is given, and the text three of them are shown in.
//!
//! A core run is sent a short manual that teaches no tool, so the shell,
//! `Read` and `Write` say how they are used in their own words, and the shell
//! shows no argument a run nobody attends should reach for: its description
//! and its sandbox switches. Every other tool keeps its own text, and every
//! tool runs as it always does — the read-before-write refusal, the kill
//! guard, the deny rules and the formatter stand behind the core texts as
//! they stand behind the others.
//!
//! The session decides once, where it opens, and says so here
//! ([`show_headless_core_tools`]): a run nobody attends opens one session, and
//! a model change within it keeps the list it opened on, so the texts are the
//! process's, like the tool digest's mode ([`crate::configure_tool_digest`]).

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::{PoisonError, RwLock};

use core_types::tool_names::{wire_tool_name, CoreTool};
use serde_json::Value;

use crate::registry::{core_tool_handlers, edit_form_of, edit_tool_handler, BACKGROUND_TASK_TOOLS};

/// The edit shape of the session this process shows the core texts to, or
/// `None` while it shows every tool its own.
static SHOWN: RwLock<Option<api::EditToolForm>> = RwLock::new(None);

/// Show this process's tools as the core profile shows them to a session
/// given `tools` — `Write` names the edit tool among them — or, with `None`,
/// each in its own text.
pub fn show_headless_core_tools(tools: Option<&BTreeSet<String>>) {
    let shown = tools.map(|tools| tools.iter().find_map(|name| edit_form_of(name)).unwrap_or_default());
    *SHOWN.write().unwrap_or_else(PoisonError::into_inner) = shown;
}

fn shown() -> Option<api::EditToolForm> {
    *SHOWN.read().unwrap_or_else(PoisonError::into_inner)
}

/// The tools a core session is given: the shell, `Read`, `Write` and the
/// model's own edit shape — the set a `core` tool set names
/// ([`crate::harness_tool_set`]) — and the two that reach a background task,
/// whose output and stop the shell cannot reach on its own.
#[must_use]
pub fn headless_core_tool_set(model: &str) -> BTreeSet<String> {
    core_tool_handlers(api::harness_profile_for_model(model).edit_tool)
        .into_iter()
        .chain(BACKGROUND_TASK_TOOLS)
        .map(str::to_string)
        .collect()
}

/// The shell, as a core run is told of it: a long job's output is read and
/// stopped with the two task tools a core session is given
/// ([`BACKGROUND_TASK_TOOLS`]).
fn shell_text() -> String {
    let [output, stop] = BACKGROUND_TASK_TOOLS;
    format!(
        "Run a bash command in the working directory; returns its output and exit code. For servers and long jobs set run_in_background, then read its output with {output} and stop it with {stop}."
    )
}

/// `Read`, as a core run is told of it.
const READ_TEXT: &str = "Read a workspace text file, image or PDF page. For a long file read the needed part with offset (first line, 1-based) and limit (lines). A whole code-file read ends with [neighbours].";

/// `Write`, as a core run is told of it: the edit tool it names is the
/// session's own edit shape.
fn write_text(edit_tool: api::EditToolForm) -> String {
    format!(
        "Create a file or replace a whole file; missing folders are created. Read an existing file before replacing it. Use {} to change part of a file.",
        wire_tool_name(edit_tool_handler(edit_tool))
    )
}

/// The shell's sandbox arguments, ignored on every profile: sandbox settings
/// come from user configuration only, never from model tool input.
const SANDBOX_SHELL_ARGUMENTS: [&str; 5] = [
    "dangerouslyDisableSandbox",
    "namespaceRestrictions",
    "isolateNetwork",
    "filesystemMode",
    "allowedMounts",
];

/// The shell's arguments the core profile does not show, as a call spells
/// them: its description and its sandbox switches. A call that carries one
/// runs without it — refusing it would cost the run a step — so a command
/// meets the sandbox the settings and the environment section say.
const HIDDEN_SHELL_ARGUMENTS: [&str; 6] = [
    "description",
    "dangerouslyDisableSandbox",
    "namespaceRestrictions",
    "isolateNetwork",
    "filesystemMode",
    "allowedMounts",
];

/// A builtin's wire `definition` (`handler` is the builtin's own name) as this
/// process shows it: in the core profile's text while a core session is shown
/// them ([`show_headless_core_tools`]), else as it is.
pub(crate) fn shown_definition(handler: &str, definition: api::ToolDefinition) -> api::ToolDefinition {
    match shown() {
        Some(edit_tool) => core_definition(handler, edit_tool, definition),
        None => definition,
    }
}

/// `definition` in the core profile's text, for a session whose edit shape is
/// `edit_tool`: the shell, `Read` and `Write` in the words above, the shell
/// without the arguments it does not show; every other tool as it is.
fn core_definition(handler: &str, edit_tool: api::EditToolForm, mut definition: api::ToolDefinition) -> api::ToolDefinition {
    if handler == CoreTool::Bash.handler() {
        definition.description = Some(shell_text());
        if let Some(properties) = definition.input_schema.get_mut("properties").and_then(Value::as_object_mut) {
            for argument in HIDDEN_SHELL_ARGUMENTS {
                properties.remove(argument);
            }
        }
    } else if handler == CoreTool::Read.handler() {
        definition.description = Some(READ_TEXT.to_string());
    } else if handler == CoreTool::Write.handler() {
        definition.description = Some(write_text(edit_tool));
    }
    definition
}

/// A call of the builtin `handler` as this process's session reads it: a shell
/// call drops model sandbox override arguments on every profile (and drops
/// `description` too in a core session), removed before anything reads the
/// call; any other tool's call passes through unchanged.
pub(crate) fn shown_shell_input<'a>(handler: &str, input: &'a Value) -> Cow<'a, Value> {
    if handler != CoreTool::Bash.handler() {
        return Cow::Borrowed(input);
    }
    let hidden: &[&str] = if shown().is_some() {
        &HIDDEN_SHELL_ARGUMENTS
    } else {
        &SANDBOX_SHELL_ARGUMENTS
    };
    without_shell_arguments(input, hidden)
}

#[cfg(test)]
fn without_hidden_shell_arguments(input: &Value) -> Cow<'_, Value> {
    without_shell_arguments(input, &HIDDEN_SHELL_ARGUMENTS)
}

fn without_shell_arguments<'a>(input: &'a Value, hidden: &[&str]) -> Cow<'a, Value> {
    match input.as_object() {
        Some(fields) if hidden.iter().any(|argument| fields.contains_key(*argument)) => {
            let mut fields = fields.clone();
            for argument in hidden {
                fields.remove(*argument);
            }
            Cow::Owned(Value::Object(fields))
        }
        _ => Cow::Borrowed(input),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{core_definition, headless_core_tool_set, without_hidden_shell_arguments, HIDDEN_SHELL_ARGUMENTS};

    /// The builtin `handler`'s wire definition, as every session but a core one
    /// is shown it.
    fn own_definition(handler: &str) -> api::ToolDefinition {
        crate::mvp_tool_specs()
            .iter()
            .find(|spec| spec.name == handler)
            .unwrap_or_else(|| panic!("no builtin {handler}"))
            .wire_definition()
    }

    #[test]
    fn the_core_shell_read_and_write_say_how_they_are_used_and_the_rest_keep_their_text() {
        let shell = core_definition("bash", api::EditToolForm::ApplyPatch, own_definition("bash"));
        assert_eq!(shell.name, "Bash");
        assert!(shell.description.as_deref().is_some_and(|text| text.contains("read its output with TaskOutput and stop it with TaskStop")));
        let mut arguments: Vec<&String> = shell.input_schema["properties"].as_object().expect("properties").keys().collect();
        arguments.sort();
        assert_eq!(arguments, ["command", "run_in_background", "timeout"], "the core shell shows three arguments");
        assert_eq!(shell.input_schema["required"], json!(["command"]));

        let write = core_definition("write_file", api::EditToolForm::ApplyPatch, own_definition("write_file"));
        assert!(write.description.as_deref().is_some_and(|text| text.ends_with("Use apply_patch to change part of a file.")));
        let write = core_definition("write_file", api::EditToolForm::Edit, own_definition("write_file"));
        assert!(write.description.as_deref().is_some_and(|text| text.ends_with("Use Edit to change part of a file.")), "the session's own edit shape");
        let read = core_definition("read_file", api::EditToolForm::ApplyPatch, own_definition("read_file"));
        assert_eq!(read.input_schema, own_definition("read_file").input_schema, "Read keeps its arguments");

        for handler in ["apply_patch", "TaskOutput", "TaskStop"] {
            assert_eq!(core_definition(handler, api::EditToolForm::ApplyPatch, own_definition(handler)), own_definition(handler), "{handler} keeps its own text");
        }
    }

    #[test]
    fn a_core_shell_call_is_read_without_the_arguments_it_does_not_show() {
        let call = json!({"command": "echo hi", "description": 7, "dangerouslyDisableSandbox": true, "namespaceRestrictions": false,
                          "isolateNetwork": "no", "filesystemMode": "off", "allowedMounts": ["/"], "run_in_background": true,
                          "timeout": 5000, "__zo_tool_call_id": "call_0"});
        let read = without_hidden_shell_arguments(&call);
        for argument in HIDDEN_SHELL_ARGUMENTS {
            assert!(read.get(argument).is_none(), "{argument} is not read");
        }
        assert_eq!(*read, json!({"command": "echo hi", "run_in_background": true, "timeout": 5000, "__zo_tool_call_id": "call_0"}));
        let plain = json!({"command": "echo hi"});
        assert!(matches!(without_hidden_shell_arguments(&plain), std::borrow::Cow::Borrowed(_)), "a call without one is read as it came");
    }

    #[test]
    fn non_core_shell_call_ignores_sandbox_arguments_while_keeping_description() {
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        super::show_headless_core_tools(None);
        let call = json!({
            "command": "echo hi",
            "description": "list files",
            "dangerouslyDisableSandbox": true,
            "namespaceRestrictions": false,
            "isolateNetwork": "no",
            "filesystemMode": "off",
            "allowedMounts": ["/"],
            "run_in_background": true,
            "timeout": 5000
        });
        let read = super::shown_shell_input("bash", &call);
        for argument in [
            "dangerouslyDisableSandbox",
            "namespaceRestrictions",
            "isolateNetwork",
            "filesystemMode",
            "allowedMounts",
        ] {
            assert!(read.get(argument).is_none(), "{argument} must be stripped for non-core profiles");
        }
        assert_eq!(
            *read,
            json!({
                "command": "echo hi",
                "description": "list files",
                "run_in_background": true,
                "timeout": 5000
            })
        );
    }

    #[test]
    fn a_core_session_is_given_the_core_four_and_the_two_background_task_tools() {
        let (patcher, editor) = {
            let _guard = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = std::env::var_os(api::MODEL_CONTEXT_WINDOWS_ENV);
            std::env::set_var(
                api::MODEL_CONTEXT_WINDOWS_ENV,
                r#"{"models":[
                    {"ids":["acme-patcher-1"],"harness":{"edit_tool":"apply_patch"}},
                    {"ids":["acme-editor-1"],"harness":{"tool_set":"core"}}]}"#,
            );
            let sets = (headless_core_tool_set("acme-patcher-1"), headless_core_tool_set("acme-editor-1"));
            match previous {
                Some(value) => std::env::set_var(api::MODEL_CONTEXT_WINDOWS_ENV, value),
                None => std::env::remove_var(api::MODEL_CONTEXT_WINDOWS_ENV),
            }
            sets
        };
        let names = |list: [&str; 6]| list.into_iter().map(str::to_string).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(patcher, names(["bash", "read_file", "write_file", "apply_patch", "TaskOutput", "TaskStop"]));
        assert_eq!(editor, names(["bash", "read_file", "write_file", "edit_file", "TaskOutput", "TaskStop"]), "the model's own edit shape");
    }
}
