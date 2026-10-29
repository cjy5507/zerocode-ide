//! The production graph adapter, with real shell checks and private temporary
//! stores. No test graph has an agent node, so no provider is contacted.
#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use runtime::memory::MemoryStore;
use serde_json::{json, Value};

const WITNESS_CHECK: &str = "printf 'checked\\n' >> graph-witness.txt";

/// The caller holds the crate-wide environment lock before construction and
/// until this fixture is dropped. Restoration also happens during unwinding.
struct Fixture {
    root: tempfile::TempDir,
    workspace: PathBuf,
    home: PathBuf,
    store: PathBuf,
    previous_cwd: PathBuf,
    previous_env: Vec<(&'static str, Option<OsString>)>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let directory = |name: &str| {
            let path = root.path().join(name);
            fs::create_dir(&path).unwrap();
            fs::canonicalize(path).unwrap()
        };
        let workspace = directory("workspace");
        let home = directory("home");
        let store = directory("workflows");
        let state = directory("state");
        fs::write(home.join("settings.json"), r#"{"autoDreamEnabled":true}"#).unwrap();
        let overrides: Vec<(&'static str, OsString)> = vec![
            ("HOME", home.clone().into_os_string()),
            ("ZO_HOME", home.clone().into_os_string()),
            ("ZO_CONFIG_HOME", home.clone().into_os_string()),
            ("ZO_STATE_DIR", state.clone().into_os_string()),
            ("ZO_WORKFLOW_STORE", store.clone().into_os_string()),
            ("ZO_TEAM_INBOX_STORE", state.join("inbox").into_os_string()),
            ("ZO_DREAM", "1".into()),
            ("ZO_BASH_TIMEOUT_MS", "5000".into()),
        ];
        let previous_env = overrides.iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        let fixture = Self {
            root, workspace, home, store,
            previous_cwd: std::env::current_dir().unwrap(),
            previous_env,
        };
        for (key, value) in overrides {
            std::env::set_var(key, value);
        }
        std::env::set_current_dir(&fixture.workspace).unwrap();
        fixture
    }

    fn witness(&self) -> String {
        fs::read_to_string(self.workspace.join("graph-witness.txt")).unwrap_or_default()
    }

    fn memory_slugs(&self) -> Vec<String> {
        runtime::memory::FsMemoryStore::local_at_cwd(&self.workspace).existing_slugs()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Restore before TempDir removes the current directory.
        let _ = std::env::set_current_dir(&self.previous_cwd);
        for (key, value) in &self.previous_env {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn check_graph() -> Value {
    json!({
        "name":"recorded-check", "mode":"graph",
        "graph":{"entry":"verify", "max_steps":4, "nodes":[
            {"kind":"check", "id":"verify", "command":WITNESS_CHECK, "on_pass":"learn", "on_fail":"failed"},
            {"kind":"dream", "id":"learn", "next":"done"},
            {"kind":"end", "id":"done", "status":"completed"},
            {"kind":"end", "id":"failed", "status":"failed"}
        ]}
    })
}

fn run(input: &Value, session: &str) -> Value {
    let output = super::run_workflow(
        input,
        None,
        false,
        &runtime::RuntimeHookConfig::default(),
        Some(session),
        None,
        None,
        None,
        Some(runtime::PermissionMode::WorkspaceWrite),
    ).expect("the production adapter accepts the graph");
    serde_json::from_str(&output).unwrap()
}

fn assert_completed(result: &Value) {
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(result["agents_spawned"], 0);
    assert_eq!(result["graph"]["steps"], 3);
    assert_eq!(result["graph"]["outputs"]["verify"]["exit_code"], 0);
}

#[test]
fn production_graph_checks_dreams_and_replays_without_repeating_effects() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let first_input = json!({"spec":check_graph(), "runId":"nightly-one"});
    let first = run(&first_input, "source-one");
    assert_completed(&first);
    assert_eq!(first["graph"]["outputs"]["learn"]["promoted"], json!([]));
    assert_eq!(fixture.witness(), "checked\n");
    assert!(fixture.memory_slugs().is_empty());

    // A caller's new session does not turn replayed evidence into a new source.
    let replay = run(&json!({"spec":check_graph(), "resumeFromRunId":"nightly-one"}), "replay-session");
    assert_completed(&replay);
    assert_eq!(replay["graph"]["resumed"], true);
    assert_eq!(replay["graph"]["visits"], first["graph"]["visits"]);
    assert_eq!(replay["graph"]["outputs"], first["graph"]["outputs"]);
    assert_eq!(fixture.witness(), "checked\n");
    assert!(fixture.memory_slugs().is_empty());

    let second = run(&json!({"spec":check_graph(), "runId":"nightly-two"}), "source-two");
    assert_completed(&second);
    assert_eq!(fixture.witness(), "checked\nchecked\n");
    let promoted = second["graph"]["outputs"]["learn"]["promoted"].as_array().unwrap();
    assert_eq!(promoted.len(), 1, "{second}");
    let slug = promoted[0].as_str().unwrap();
    assert_eq!(fixture.memory_slugs(), [slug]);
    let memory = runtime::memory::paths::memory_write_dir(&fixture.workspace, true);
    assert!(memory.starts_with(&fixture.home));
    let entry = fs::read_to_string(memory.join(format!("{slug}.md"))).unwrap();
    assert!(entry.contains(WITNESS_CHECK));
    assert!(fs::read_to_string(memory.join("MEMORY.md")).unwrap().contains(&format!("]({slug}.md)")));

    let second_replay = run(&json!({"spec":check_graph(), "runId":"nightly-two"}), "source-three");
    assert_completed(&second_replay);
    assert_eq!(fixture.witness(), "checked\nchecked\n");
    assert_eq!(fixture.memory_slugs(), [slug]);
    assert_eq!(fs::read_to_string(memory.join(format!("{slug}.md"))).unwrap(), entry);
}

#[test]
fn production_graph_rejects_changed_resume_before_running_the_check() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let original = run(&json!({"spec":check_graph(), "input":"first", "runId":"fixed-run"}), "source-one");
    assert_completed(&original);
    let mut changed = check_graph();
    changed["graph"]["nodes"][0]["command"] = json!("printf 'unexpected\\n' >> graph-witness.txt");
    for request in [
        json!({"spec":changed, "input":"first", "resumeFromRunId":"fixed-run"}),
        json!({"spec":check_graph(), "input":"changed", "resumeFromRunId":"fixed-run"}),
    ] {
        let refused = run(&request, "source-two");
        assert_eq!(refused["status"], "blocked", "{refused}");
        assert!(refused["notes"].as_array().unwrap().iter()
            .any(|note| note.as_str().is_some_and(|text| text.contains("does not match"))));
        assert_eq!(fixture.witness(), "checked\n");
    }
}

#[test]
fn production_bare_graph_ignores_fresh_dispatch_ids_for_resume_identity() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let mut request = check_graph();
    request["__zo_tool_call_id"] = json!("dispatch-one");
    let first = run(&request, "source-one");
    assert_completed(&first);
    request["__zo_tool_call_id"] = json!("dispatch-two");
    let replay = run(&request, "source-two");
    assert_completed(&replay);
    assert_eq!(first["graph"]["run_id"], replay["graph"]["run_id"]);
    assert_eq!(replay["graph"]["resumed"], true);
    assert_eq!(fixture.witness(), "checked\n");
    assert!(fixture.memory_slugs().is_empty());
}

fn tree_entries(root: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, directory: &Path, entries: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            entries.push(entry.path().strip_prefix(root).unwrap().to_path_buf());
            if entry.file_type().unwrap().is_dir() {
                walk(root, &entry.path(), entries);
            }
        }
    }
    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    entries.sort();
    entries
}

#[test]
fn shipped_graph_example_validates_without_touching_execution_state() {
    let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let before = tree_entries(fixture.root.path());
    let spec: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"), "/../../examples/workflows/verified-repair-graph.json"
    ))).unwrap();
    let preview: Value = serde_json::from_str(&super::validate_workflow(&spec).unwrap()).unwrap();
    assert_eq!(preview["valid"], true);
    assert_eq!(preview["mode"], "graph");
    assert_eq!(preview["graph"]["entry"], "verify");
    assert_eq!(tree_entries(fixture.root.path()), before);
    assert_eq!(fs::read_dir(&fixture.store).unwrap().count(), 0);
    assert!(fixture.witness().is_empty());
    assert!(fixture.memory_slugs().is_empty());
}
