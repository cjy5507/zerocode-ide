//! The tools every model is shown, and every older name and field they still
//! answer to (t-96186).
//!
//! The yardstick is measured, not remembered: what Claude Code puts on its
//! wire, captured by `tools/cli-tool-baseline/capture.py` into
//! `tests/fixtures/wire-tools/`. When the CLI changes, capture again and these
//! tests say what moved.

use super::*;

fn claude_code_baseline() -> serde_json::Value {
    serde_json::from_str(include_str!("../../tests/fixtures/wire-tools/claude-code.json"))
        .expect("the Claude Code baseline is JSON")
}

/// A baseline tool's parameter names.
fn baseline_parameters(baseline: &serde_json::Value, name: &str) -> BTreeSet<String> {
    baseline["tools"]
        .as_array()
        .expect("the baseline lists tools")
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("the baseline has no {name}"))["parameters"]
        .as_array()
        .expect("a baseline tool lists its parameters")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The parameter names of each tool on zo's wire.
fn wire_parameters() -> BTreeMap<String, BTreeSet<String>> {
    builtin_registry_outside_cwd_windows()
        .definitions(None)
        .into_iter()
        .map(|definition| {
            let fields = definition.input_schema["properties"]
                .as_object()
                .map(|properties| properties.keys().cloned().collect())
                .unwrap_or_default();
            (definition.name, fields)
        })
        .collect()
}

/// The bytes the wire list costs on every request, whole and per tool, for
/// the before-and-after the task reports (`--nocapture`).
#[test]
fn the_wire_bytes_are_reported() {
    let definitions = builtin_registry_outside_cwd_windows().definitions(None);
    for definition in &definitions {
        let bytes = serde_json::to_string(definition).expect("a definition serializes").len();
        eprintln!("wire tool {} {bytes} bytes", definition.name);
    }
    let total = serde_json::to_string(&definitions).expect("definitions serialize").len();
    eprintln!("wire tools {} schemas {total} bytes", definitions.len());
    assert!(total > 0);
}

/// Where Claude Code carries a tool zo has, zo shows it under the same name
/// and fields — but for a field whose feature zo's handler lacks, which it
/// would be a lie to advertise. In neither measured CLI today, `Glob` and
/// `Grep` keep the names and fields Claude Code gave them, because a third of
/// zo's sessions search with them.
#[test]
fn the_wire_shows_todays_general_names_and_fields() {
    let wire = wire_parameters();
    let baseline = claude_code_baseline();
    // `Read`'s PDF page range and its over-size read: zo's reader has neither.
    let not_in_zo: &[(&str, &[&str])] = &[("Read", &["pages", "allow_large"])];
    for name in ["Bash", "Read", "Write", "Edit"] {
        let skipped = not_in_zo
            .iter()
            .find(|(tool, _)| *tool == name)
            .map_or(&[][..], |(_, fields)| *fields);
        let expected: BTreeSet<String> = baseline_parameters(&baseline, name)
            .into_iter()
            .filter(|field| !skipped.contains(&field.as_str()))
            .collect();
        let shown = wire
            .get(name)
            .unwrap_or_else(|| panic!("{name} is not on the wire: {:?}", wire.keys()));
        assert_eq!(shown, &expected, "{name}'s fields");
    }
    assert_eq!(
        wire.get("Glob"),
        Some(&["path", "pattern"].map(str::to_string).into_iter().collect()),
        "Glob's fields"
    );
    let grep = wire.get("Grep").expect("Grep is on the wire");
    for field in ["pattern", "path", "glob", "output_mode", "-i", "-n", "head_limit"] {
        assert!(grep.contains(field), "Grep takes {field}: {grep:?}");
    }
    for old in ["bash", "read_file", "write_file", "edit_file", "glob_search", "grep_search"] {
        assert!(!wire.contains_key(old), "{old} still reaches the model");
    }
}

/// An older session, a model's habit, a hook or an allow-list may name a tool
/// or a field the old way, or the way another CLI does: every one of them still
/// reaches the same handler.
#[test]
fn every_old_and_new_name_and_field_reaches_the_same_tool() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("wire-names");
    fs::create_dir_all(&root).expect("create root");
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&root).expect("set cwd");

    run_tool("Write", &json!({ "file_path": "a.txt", "content": "one\n" }))
        .expect("Write with file_path");
    for (name, input) in [
        ("Read", json!({ "file_path": "a.txt" })),
        ("read_file", json!({ "path": "a.txt" })),
        ("read", json!({ "filePath": "a.txt" })),
    ] {
        let read = run_tool(name, &input).unwrap_or_else(|error| panic!("{name} {input}: {error}"));
        assert!(read.contains("one"), "{name}: {read}");
    }
    run_tool(
        "Edit",
        &json!({ "file_path": "a.txt", "old_string": "one", "new_string": "two" }),
    )
    .expect("Edit with file_path");
    run_tool(
        "edit_file",
        &json!({ "path": "a.txt", "old_string": "two", "new_string": "three" }),
    )
    .expect("edit_file with path");
    run_tool(
        "MultiEdit",
        &json!({ "file_path": "a.txt", "edits": [{ "old_string": "three", "new_string": "four" }] }),
    )
    .expect("MultiEdit with file_path");
    assert_eq!(fs::read_to_string(root.join("a.txt")).expect("read back"), "four\n");
    let shell = run_tool("Bash", &json!({ "cmd": "printf from-cmd" })).expect("Bash with cmd");
    assert!(shell.contains("from-cmd"), "{shell}");

    std::env::set_current_dir(original_dir).expect("restore cwd");
    let _ = fs::remove_dir_all(root);
}

/// A call the table cannot fix says what is missing and the shape to send,
/// under the names the model is shown, so the next call is right.
#[test]
fn a_call_missing_its_file_is_told_the_shape_to_send() {
    let error = run_tool("Read", &json!({ "filename": "a.txt" }))
        .expect_err("no file named")
        .to_string();
    assert!(error.contains("missing `file_path`"), "{error}");
    assert!(error.contains("Read has no `filename`"), "{error}");
    assert!(error.contains(r#"Read takes {"file_path": string"#), "{error}");
}

/// The wire list is a function of configuration alone: the same bytes on
/// every request of a session, so the prompt cache behind it holds.
#[test]
fn the_wire_is_the_same_bytes_on_every_request() {
    let bytes = || {
        serde_json::to_string(&builtin_registry_outside_cwd_windows().definitions(None))
            .expect("definitions serialize")
    };
    assert_eq!(bytes(), bytes());
}

/// One list for every provider: each wire schema uses only the keywords every
/// provider's serializer is proved to carry as written
/// (`api::PORTABLE_SCHEMA_KEYWORDS`), and each name is one every provider
/// accepts as a function name — a letter, then letters, digits, `_` or `-`,
/// 64 at most.
#[test]
fn every_wire_tool_reaches_every_provider_as_written() {
    let portable: BTreeSet<String> = api::PORTABLE_SCHEMA_KEYWORDS
        .iter()
        .map(|keyword| (*keyword).to_string())
        .collect();
    for definition in builtin_registry_outside_cwd_windows().definitions(None) {
        let strangers: Vec<String> = api::schema_keywords(&definition.input_schema)
            .difference(&portable)
            .cloned()
            .collect();
        assert!(strangers.is_empty(), "{} uses {strangers:?}", definition.name);
        let name = definition.name.as_str();
        assert!(
            name.len() <= 64
                && name.starts_with(|first: char| first.is_ascii_alphabetic())
                && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'),
            "{name} is not a name every provider takes"
        );
    }
}
