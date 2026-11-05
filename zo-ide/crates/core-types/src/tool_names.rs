//! zo's core tools: every name they answer to, and the face a model sees
//! (t-96186).
//!
//! Inside zo a core tool keeps the name and fields its handler was written
//! under — `read_file` with `path`. Sessions on disk, permission rules, hooks,
//! the context compressor and every table keyed by a tool name were built on
//! them, and an older zo resumes what a newer one wrote. A model is shown the
//! general shape today's agent CLIs train it on instead — `Read` with
//! `file_path` — measured off the wire of Claude Code and codex
//! (`crates/tools/tests/fixtures/wire-tools/`). Every provider gets that one
//! face.
//!
//! This table is the only place the two meet:
//! - out to a model: [`wire_tool_name`], [`schema_to_wire`], [`input_to_wire`];
//! - in from a model, a hook or a saved session: [`CoreTool::of`] and
//!   [`input_from_wire`] bring any spelling a model sends back to the
//!   handler's, and [`handler_call`] does both for a stored call;
//! - one tool under two names: [`tool_key`], for permission rules, hook
//!   matchers and allow-lists.

use serde_json::{Map, Value};

/// Lower-case a tool name and replace `-` with `_`, so `Bash`, `BASH` and
/// `bash` are one name, and `read-file` is `read_file`. Dispatch resolves a
/// tool through it, and a permission rule meets a call through it, so a rule
/// holds for every spelling dispatch would run (t-81312).
#[must_use]
pub fn normalize_tool_name(value: &str) -> String {
    value.trim().replace('-', "_").to_ascii_lowercase()
}

/// Alias → handler, matched after [`normalize_tool_name`], so one entry covers
/// `Read`, `read` and `READ`. A name no entry matches is its own handler's.
pub const TOOL_NAME_ALIASES: &[(&str, &str)] = &[
    // The core tools under the names a model is shown (`CoreTool::wire`) and
    // their short forms.
    ("read", "read_file"),
    ("write", "write_file"),
    ("edit", "edit_file"),
    ("multi_edit", "MultiEdit"),
    ("glob", "glob_search"),
    ("grep", "grep_search"),
    ("bash", "bash"),
    // codex's shell command (`exec_command`, the codex-cli 0.166.0 baseline).
    ("exec_command", "bash"),
    // Task/todo/web/misc handlers are already PascalCase; these entries let
    // their snake_case spellings resolve too.
    ("todo_write", "TodoWrite"),
    ("notebook_edit", "NotebookEdit"),
    ("web_fetch", "WebFetch"),
    ("web_search", "WebSearch"),
    ("task", "TaskCreate"),
];

/// Alias keys that are zo's own shorthand rather than another agent's name for
/// the same tool: Claude Code's `Task` starts a sub-agent (zo's `Agent`),
/// while zo reads `task` as `TaskCreate`.
const ZO_SHORTHAND_ALIASES: &[&str] = &["task"];

/// Map any accepted tool alias to the canonical handler name used by the
/// dispatcher. Returns the input unchanged when no alias matches, so
/// already-canonical names (including `PascalCase` handlers like `TodoWrite`)
/// remain a no-op.
#[must_use]
pub fn canonical_tool_name(name: &str) -> String {
    let normalized = normalize_tool_name(name);
    TOOL_NAME_ALIASES
        .iter()
        .find(|(alias, _)| normalized == *alias)
        .map_or_else(|| name.to_string(), |(_, handler)| (*handler).to_string())
}

/// zo's tool for `name` when the alias table maps it to one and the name means
/// that tool elsewhere too (`Edit` is `edit_file`). `None` for a name the
/// table does not map and for zo's own shorthand.
fn same_tool_alias(name: &str) -> Option<&'static str> {
    let normalized = normalize_tool_name(name);
    TOOL_NAME_ALIASES
        .iter()
        .find(|(alias, _)| *alias == normalized && !ZO_SHORTHAND_ALIASES.contains(alias))
        .map(|(_, handler)| *handler)
}

/// The key two names of one tool share: `Edit(src/**)` and `edit_file(src/**)`
/// are one rule, `Bash` and `bash` one matcher. zo's own shorthand stays its
/// own (`Task` is not `TaskCreate`).
#[must_use]
pub fn tool_key(name: &str) -> String {
    normalize_tool_name(same_tool_alias(name).unwrap_or(name))
}

/// One of zo's core tools: the coding set every model is shown under today's
/// general names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreTool {
    Bash,
    Read,
    Write,
    Edit,
    MultiEdit,
    Glob,
    Grep,
}

/// A handler field the wire names differently, or that models also send under
/// other spellings.
struct Field {
    /// The handler's own name for it.
    handler: &'static str,
    /// The name the model is shown.
    wire: &'static str,
    /// Spellings a model sends besides the two above, taken in this order.
    received: &'static [&'static str],
}

/// A file tool's file: Claude Code's `file_path` on the wire, and the
/// camel-case `filePath` zo's own results and the window already read as it.
const FILE_PATH: Field = Field {
    handler: "path",
    wire: "file_path",
    received: &["filePath"],
};

/// The shell's command, also under codex's `cmd` (`exec_command`).
const COMMAND: Field = Field {
    handler: "command",
    wire: "command",
    received: &["cmd"],
};

impl CoreTool {
    pub const ALL: [Self; 7] = [
        Self::Bash,
        Self::Read,
        Self::Write,
        Self::Edit,
        Self::MultiEdit,
        Self::Glob,
        Self::Grep,
    ];

    /// The handler's name: what dispatch, sessions, rules and hooks inside zo
    /// key the tool on.
    #[must_use]
    pub const fn handler(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Read => "read_file",
            Self::Write => "write_file",
            Self::Edit => "edit_file",
            Self::MultiEdit => "MultiEdit",
            Self::Glob => "glob_search",
            Self::Grep => "grep_search",
        }
    }

    /// The name a model is shown and calls. `Glob` and `Grep` are in neither
    /// measured CLI today, and keep the names Claude Code gave them.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::Bash => "Bash",
            Self::Read => "Read",
            Self::Write => "Write",
            Self::Edit => "Edit",
            Self::MultiEdit => "MultiEdit",
            Self::Glob => "Glob",
            Self::Grep => "Grep",
        }
    }

    /// The core tool `name` means, in any spelling: `Read`, `read`,
    /// `read_file`, `READ-FILE`.
    #[must_use]
    pub fn of(name: &str) -> Option<Self> {
        let key = tool_key(name);
        Self::ALL
            .into_iter()
            .find(|tool| normalize_tool_name(tool.handler()) == key)
    }

    const fn fields(self) -> &'static [Field] {
        match self {
            Self::Read | Self::Write | Self::Edit | Self::MultiEdit => &[FILE_PATH],
            Self::Bash => &[COMMAND],
            Self::Glob | Self::Grep => &[],
        }
    }

    /// Fields the model may not set. The shell's sandbox knobs come from
    /// `settings.sandbox` (`runtime::SandboxConfig`); a call that still sends
    /// them — an old session, a model's habit — is not obeyed.
    const fn settings_only(self) -> &'static [&'static str] {
        match self {
            Self::Bash => &[
                "namespaceRestrictions",
                "isolateNetwork",
                "filesystemMode",
                "allowedMounts",
            ],
            _ => &[],
        }
    }
}

/// The name a model is shown for `name`: a core tool's wire name, any other
/// tool's own name.
#[must_use]
pub fn wire_tool_name(name: &str) -> &str {
    // A `match`, not `map_or(name, CoreTool::wire)`: the function item returns
    // `&'static str`, which would demand a `'static` name.
    match CoreTool::of(name) {
        Some(tool) => tool.wire(),
        None => name,
    }
}

/// The name the wire gives a handler field of `name`'s tool: `path` is
/// `file_path` on a file tool, every other field is itself.
#[must_use]
pub fn field_to_wire<'a>(name: &str, field: &'a str) -> &'a str {
    CoreTool::of(name)
        .and_then(|tool| tool.fields().iter().find(|known| known.handler == field))
        .map_or(field, |known| known.wire)
}

/// A handler's input schema as the model is shown it: each renamed field under
/// its wire name in `properties` and `required`. `None` when nothing is
/// renamed, so the caller keeps the schema it holds.
#[must_use]
pub fn schema_to_wire(name: &str, schema: &Value) -> Option<Value> {
    let renamed = renamed_fields(name)?;
    let mut schema = schema.clone();
    let mut changed = false;
    if let Some(Value::Object(properties)) = schema.get_mut("properties") {
        for field in &renamed {
            changed |= rename(properties, field.handler, field.wire);
        }
    }
    if let Some(Value::Array(required)) = schema.get_mut("required") {
        for entry in required {
            if let Some(field) = renamed.iter().find(|field| *entry == field.handler) {
                *entry = Value::from(field.wire);
                changed = true;
            }
        }
    }
    changed.then_some(schema)
}

/// A stored call's input as the model is shown it: each renamed field under
/// its wire name. `None` when nothing needed changing.
#[must_use]
pub fn input_to_wire(name: &str, input: &Value) -> Option<Value> {
    let renamed = renamed_fields(name)?;
    let Value::Object(object) = input else {
        return None;
    };
    let mut object = object.clone();
    let mut changed = false;
    for field in &renamed {
        changed |= rename(&mut object, field.handler, field.wire);
    }
    changed.then_some(Value::Object(object))
}

/// [`input_to_wire`] over the JSON text a session stores. `None` when nothing
/// needed changing or the text is not JSON.
#[must_use]
pub fn input_text_to_wire(name: &str, input: &str) -> Option<String> {
    renamed_fields(name)?;
    let value = serde_json::from_str::<Value>(input).ok()?;
    input_to_wire(name, &value).map(|value| value.to_string())
}

/// A call's input as its handler reads it, whatever spelling the model, a hook
/// or a saved session used: a field's wire name or another spelling models
/// send becomes the handler's, and settings-only fields are dropped. A field
/// the call already names the handler's way wins over its other spellings.
/// `None` when nothing needed changing, so a caller keeps the bytes it holds.
#[must_use]
pub fn input_from_wire(name: &str, input: &Value) -> Option<Value> {
    let tool = CoreTool::of(name)?;
    let Value::Object(object) = input else {
        return None;
    };
    let mut object = object.clone();
    let mut changed = false;
    for field in tool.fields() {
        let other_names = std::iter::once(field.wire)
            .chain(field.received.iter().copied())
            .filter(|name| *name != field.handler);
        for sent in other_names {
            changed |= rename(&mut object, sent, field.handler);
        }
    }
    for knob in tool.settings_only() {
        changed |= object.remove(*knob).is_some();
    }
    changed.then_some(Value::Object(object))
}

/// A call as its handler stores it, from the name and JSON text a model sent:
/// the handler's name, and the input in the handler's fields when they
/// differed (`None` keeps the text as sent). `None` for a tool outside the
/// core.
#[must_use]
pub fn handler_call(name: &str, input: &str) -> Option<(&'static str, Option<String>)> {
    let tool = CoreTool::of(name)?;
    let input = serde_json::from_str::<Value>(input)
        .ok()
        .and_then(|value| input_from_wire(tool.handler(), &value))
        .map(|value| value.to_string());
    Some((tool.handler(), input))
}

/// The renamed fields of `name`'s tool; `None` when it has none.
fn renamed_fields(name: &str) -> Option<Vec<&'static Field>> {
    let renamed: Vec<_> = CoreTool::of(name)?
        .fields()
        .iter()
        .filter(|field| field.handler != field.wire)
        .collect();
    (!renamed.is_empty()).then_some(renamed)
}

/// Move `from` to `to`, unless `to` is already there: a call that names a field
/// both ways keeps the one it gave under `to`, and `from` stays as sent.
fn rename(object: &mut Map<String, Value>, from: &str, to: &str) -> bool {
    if object.contains_key(to) {
        return false;
    }
    let Some(value) = object.remove(from) else {
        return false;
    };
    object.insert(to.to_string(), value);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_spelling_of_a_core_tool_names_one_handler_and_one_wire_name() {
        for tool in CoreTool::ALL {
            let lower = tool.wire().to_ascii_lowercase();
            let upper = tool.handler().to_ascii_uppercase();
            let dashed = tool.handler().replace('_', "-");
            for spelling in [tool.handler(), tool.wire(), lower.as_str(), upper.as_str(), dashed.as_str()] {
                assert_eq!(CoreTool::of(spelling), Some(tool), "{spelling}");
                assert_eq!(wire_tool_name(spelling), tool.wire(), "{spelling}");
                assert_eq!(tool_key(spelling), tool_key(tool.handler()), "{spelling}");
            }
        }
        assert_eq!(CoreTool::of("multi_edit"), Some(CoreTool::MultiEdit));
        assert_eq!(CoreTool::of("exec_command"), Some(CoreTool::Bash));
    }

    #[test]
    fn a_tool_outside_the_core_keeps_its_own_name() {
        for name in ["TodoWrite", "Agent", "AskUserQuestion", "mcp__x__y", "TaskCreate", "Task"] {
            assert_eq!(CoreTool::of(name), None, "{name}");
            assert_eq!(wire_tool_name(name), name, "{name}");
        }
        // zo's own `task` shorthand is not Claude Code's `Task` in a rule.
        assert_ne!(tool_key("Task"), tool_key("TaskCreate"));
        assert_eq!(tool_key("todo_write"), tool_key("TodoWrite"));
    }

    #[test]
    fn a_file_tool_reads_its_file_from_every_spelling_a_model_sends() {
        for key in ["file_path", "filePath"] {
            let sent = json!({ key: "src/a.rs", "old_string": "a", "new_string": "b" });
            assert_eq!(
                input_from_wire("Edit", &sent),
                Some(json!({"path": "src/a.rs", "old_string": "a", "new_string": "b"})),
                "{key}"
            );
        }
        // The handler's own spelling needs nothing, under either name.
        assert_eq!(input_from_wire("edit_file", &json!({"path": "a"})), None);
        assert_eq!(input_from_wire("Read", &json!({"path": "a"})), None);
        // Grep's `path` is its own directory word, left alone.
        assert_eq!(input_from_wire("Grep", &json!({"pattern": "x", "path": "src"})), None);
        // A call naming its file both ways keeps the handler's spelling.
        assert_eq!(input_from_wire("Read", &json!({"path": "a", "file_path": "b"})), None);
    }

    #[test]
    fn the_shell_takes_cmd_and_never_takes_sandbox_knobs_from_the_model() {
        let sent = json!({
            "cmd": "ls",
            "filesystemMode": "off",
            "namespaceRestrictions": false,
            "isolateNetwork": false,
            "allowedMounts": ["/"],
            "timeout": 1000
        });
        assert_eq!(
            input_from_wire("Bash", &sent),
            Some(json!({"command": "ls", "timeout": 1000}))
        );
        assert_eq!(input_from_wire("bash", &json!({"command": "ls"})), None);
    }

    #[test]
    fn a_stored_call_goes_out_in_the_wire_shape_and_comes_back_unchanged() {
        let stored = json!({"path": "a.rs", "offset": 3});
        let wire = input_to_wire("read_file", &stored).expect("renamed");
        assert_eq!(wire, json!({"file_path": "a.rs", "offset": 3}));
        assert_eq!(input_from_wire("Read", &wire), Some(stored));
        assert_eq!(input_to_wire("bash", &json!({"command": "ls"})), None);
        assert_eq!(input_to_wire("glob_search", &json!({"path": "src"})), None);
        assert_eq!(input_to_wire("TodoWrite", &json!({"path": "x"})), None);
    }

    #[test]
    fn a_schema_is_shown_under_the_wire_fields() {
        let handler = json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
            "required": ["path", "content"],
            "additionalProperties": false
        });
        assert_eq!(
            schema_to_wire("write_file", &handler),
            Some(json!({
                "type": "object",
                "properties": {"file_path": {"type": "string"}, "content": {"type": "string"}},
                "required": ["file_path", "content"],
                "additionalProperties": false
            }))
        );
        assert_eq!(schema_to_wire("grep_search", &handler), None);
        assert_eq!(field_to_wire("edit_file", "path"), "file_path");
        assert_eq!(field_to_wire("edit_file", "old_string"), "old_string");
        assert_eq!(field_to_wire("grep_search", "path"), "path");
    }

    #[test]
    fn a_call_is_stored_under_its_handler() {
        assert_eq!(
            handler_call("Read", r#"{"file_path":"a.rs"}"#),
            Some(("read_file", Some(r#"{"path":"a.rs"}"#.to_string())))
        );
        assert_eq!(handler_call("Bash", r#"{"command":"ls"}"#), Some(("bash", None)));
        assert_eq!(handler_call("read_file", r#"{"path":"a.rs"}"#), Some(("read_file", None)));
        assert_eq!(handler_call("Agent", "{}"), None);
    }
}
