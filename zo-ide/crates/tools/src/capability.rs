//! `CapabilityInvoke` — addressing a tool without advertising it.
//!
//! The wire tool list sits at the very front of the cached prompt prefix, so
//! adding one schema mid-session re-bills the entire conversation behind it.
//! The live ledger measured that at 191 breaks and 67.5M tokens — 18.6% of
//! every cache read lost on this machine — and the cause was structural: a
//! `ToolSearch` had to ADD what it found, because a strict function-calling
//! provider can only emit a call for a tool advertised on that very request.
//!
//! `CapabilityInvoke` satisfies that constraint with a schema that is always
//! advertised. The model names the tool it wants; the executor re-enters
//! itself with that name. So the wire list becomes a function of configuration
//! alone — byte-identical on every request of a session.
//!
//! **It is an address, not a privilege.** Unwrapping happens at the TOP of an
//! executor's `execute`, and the inner name goes back through the same
//! entrypoint: allow-lists, deny-lists, the permission enforcer, the plan
//! gate, disabled-tool toggles and the audit ledger all see the inner tool
//! exactly as if the model had called it directly. Nothing here re-implements
//! a check, which is the only way a wrapper like this stays safe as those
//! checks change.

use serde_json::Value;

/// The always-advertised indirection tool.
pub const CAPABILITY_INVOKE: &str = "CapabilityInvoke";

/// What an executor should run instead of a `CapabilityInvoke` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityCall {
    pub name: String,
    /// The inner tool's arguments, already serialized — executors take input
    /// as a JSON string, and re-serializing here keeps that seam single.
    pub input: String,
}

/// Read `{name, input}` off a `CapabilityInvoke` call.
///
/// Errors are the model's to act on, so they say what to do: a missing name,
/// a non-object `input`, or an attempt to nest the wrapper in itself.
///
/// # Errors
/// When `name` is missing/empty/not a string, when `input` is present but not
/// an object, or when `name` is `CapabilityInvoke` itself.
pub fn unwrap_capability_invoke(input: &Value) -> Result<CapabilityCall, String> {
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            "CapabilityInvoke needs `name`: the exact name of the tool to run. Find it with \
             ToolSearch."
                .to_string()
        })?;
    if name == CAPABILITY_INVOKE {
        // Not a stack-overflow guard alone — a nested call is always a
        // misunderstanding, and saying so is more useful than unwrapping it.
        return Err(format!(
            "{CAPABILITY_INVOKE} cannot invoke itself; pass the name of the tool you want to run."
        ));
    }
    let inner = match input.get("input") {
        None | Some(Value::Null) => Value::Object(serde_json::Map::new()),
        Some(value @ Value::Object(_)) => value.clone(),
        Some(other) => {
            return Err(format!(
                "CapabilityInvoke `input` must be an object shaped by `{name}`'s own schema, got \
                 {}.",
                match other {
                    Value::Array(_) => "an array",
                    Value::String(_) => "a string",
                    Value::Number(_) => "a number",
                    Value::Bool(_) => "a boolean",
                    _ => "a non-object",
                }
            ))
        }
    };
    Ok(CapabilityCall {
        name: name.to_string(),
        input: inner.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{unwrap_capability_invoke, CAPABILITY_INVOKE};
    use serde_json::json;

    #[test]
    fn a_named_tool_and_its_arguments_come_back_serialized() {
        let call = unwrap_capability_invoke(&json!({
            "name": "WebSearch",
            "input": {"query": "rust"}
        }))
        .expect("unwrap");
        assert_eq!(call.name, "WebSearch");
        assert_eq!(call.input, r#"{"query":"rust"}"#);
    }

    /// A tool that takes no arguments is the common case for the diagnostic
    /// families (`Audit` takes none), so omitting `input` must work rather
    /// than force the model to type `{}`.
    #[test]
    fn a_tool_without_arguments_needs_no_input_field() {
        for payload in [json!({"name": "Audit"}), json!({"name": "Audit", "input": null})] {
            let call = unwrap_capability_invoke(&payload).expect("unwrap");
            assert_eq!(call.input, "{}");
        }
    }

    /// Every error is something the model can fix on the next call, so each
    /// one names the fix rather than the schema.
    #[test]
    fn every_rejection_says_what_to_do_instead() {
        let missing = unwrap_capability_invoke(&json!({})).expect_err("no name");
        assert!(missing.contains("ToolSearch"), "{missing}");
        let blank = unwrap_capability_invoke(&json!({"name": "   "})).expect_err("blank name");
        assert!(blank.contains("`name`"), "{blank}");
        let nested = unwrap_capability_invoke(&json!({"name": CAPABILITY_INVOKE}))
            .expect_err("self-invocation");
        assert!(nested.contains("cannot invoke itself"), "{nested}");
        let shaped = unwrap_capability_invoke(&json!({"name": "bash", "input": "ls"}))
            .expect_err("string input");
        assert!(shaped.contains("must be an object"), "{shaped}");
        assert!(shaped.contains("`bash`"), "names the tool: {shaped}");
    }

    /// Whitespace around a name is a formatting slip, not a different tool.
    #[test]
    fn a_padded_name_resolves_to_the_same_tool() {
        let call = unwrap_capability_invoke(&json!({"name": " WebSearch "})).expect("unwrap");
        assert_eq!(call.name, "WebSearch");
    }
}
