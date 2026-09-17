use std::fs;

use tempfile::TempDir;

use super::*;

fn fixture_graph(files: &[(&str, &str)]) -> (TempDir, CodeGraph) {
    let workspace = tempfile::tempdir().expect("temp workspace");
    for (path, source) in files {
        let path = workspace.path().join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture parent");
        }
        fs::write(path, source).expect("fixture source");
    }
    let cache = workspace.path().join("state").join(DEFAULT_CACHE_FILE_NAME);
    let graph = CodeGraph::load_or_build(workspace.path(), cache).expect("build codegraph");
    (workspace, graph)
}

fn has_symbol(graph: &mut CodeGraph, name: &str, kind: SymbolKind) -> bool {
    !graph
        .find_symbols(name, Some(kind))
        .expect("query symbols")
        .is_empty()
}

#[test]
fn extracts_rust_definitions_and_references() {
    let source = r"
trait Speaker { fn speak(&self); }
struct Dog;
impl Speaker for Dog {
    fn speak(&self) { helper(); }
}
fn helper() {}
fn main() { helper(); }
";
    let (_workspace, mut graph) = fixture_graph(&[("src/lib.rs", source)]);
    assert!(has_symbol(&mut graph, "Dog", SymbolKind::Struct));
    assert!(has_symbol(&mut graph, "Speaker", SymbolKind::Trait));
    assert!(has_symbol(&mut graph, "helper", SymbolKind::Function));
    let methods = graph
        .find_symbols("speak", Some(SymbolKind::Method))
        .expect("method query");
    assert!(
        methods
            .iter()
            .any(|method| method.container.as_deref() == Some("Dog"))
    );
    assert_eq!(
        graph
            .find_references("helper")
            .expect("reference query")
            .len(),
        2
    );
}

#[test]
fn extracts_typescript_and_javascript_definitions() {
    let typescript = r"
interface Runner { run(): void; }
class Job implements Runner { run() { helper(); } }
function helper() {}
const make = () => new Job();
";
    let javascript = "export function jsHelper() {}\nconst build = () => jsHelper();\n";
    let (_workspace, mut graph) = fixture_graph(&[
        ("src/main.ts", typescript),
        ("src/helper.js", javascript),
    ]);
    assert!(has_symbol(&mut graph, "Runner", SymbolKind::Interface));
    assert!(has_symbol(&mut graph, "Job", SymbolKind::Class));
    assert!(has_symbol(&mut graph, "helper", SymbolKind::Function));
    assert!(has_symbol(&mut graph, "make", SymbolKind::Const));
    assert!(has_symbol(&mut graph, "jsHelper", SymbolKind::Function));
    assert!(has_symbol(&mut graph, "build", SymbolKind::Const));
    let methods = graph
        .find_symbols("run", Some(SymbolKind::Method))
        .expect("method query");
    assert!(methods.iter().any(|method| method.container.as_deref() == Some("Job")));
    assert!(!graph
        .find_references("helper")
        .expect("reference query")
        .is_empty());
}

#[test]
fn extracts_python_definitions_and_references() {
    let source = r"
def helper():
    pass

class Worker:
    def run(self):
        helper()
";
    let (_workspace, mut graph) = fixture_graph(&[("worker.py", source)]);
    assert!(has_symbol(&mut graph, "helper", SymbolKind::Function));
    assert!(has_symbol(&mut graph, "Worker", SymbolKind::Class));
    let methods = graph
        .find_symbols("run", Some(SymbolKind::Method))
        .expect("method query");
    assert_eq!(methods[0].container.as_deref(), Some("Worker"));
    assert_eq!(
        graph
            .find_references("helper")
            .expect("reference query")
            .len(),
        1
    );
}

#[test]
fn extracts_go_definitions_and_references() {
    let source = r"
package worker

type Worker struct{}
func helper() {}
func (worker Worker) Run() { helper() }
";
    let (_workspace, mut graph) = fixture_graph(&[("worker.go", source)]);
    assert!(has_symbol(&mut graph, "Worker", SymbolKind::Type));
    assert!(has_symbol(&mut graph, "helper", SymbolKind::Function));
    let methods = graph
        .find_symbols("Run", Some(SymbolKind::Method))
        .expect("method query");
    assert_eq!(methods[0].container.as_deref(), Some("Worker"));
    assert_eq!(
        graph
            .find_references("helper")
            .expect("reference query")
            .len(),
        1
    );
}

#[test]
fn workspace_scan_respects_gitignore() {
    let (workspace, mut graph) = fixture_graph(&[
        (".gitignore", "ignored.rs\n"),
        ("kept.rs", "fn kept() {}\n"),
        ("ignored.rs", "fn ignored() {}\n"),
    ]);
    assert!(has_symbol(&mut graph, "kept", SymbolKind::Function));
    assert!(!has_symbol(&mut graph, "ignored", SymbolKind::Function));
    assert!(workspace.path().join("ignored.rs").exists());
}

#[test]
fn oversized_file_is_skipped_with_marker() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    let oversized = workspace.path().join("oversized.rs");
    let file = fs::File::create(&oversized).expect("oversized fixture");
    file.set_len(MAX_INDEXABLE_FILE_SIZE + 1)
        .expect("grow sparse fixture");
    let cache = workspace.path().join("state").join(DEFAULT_CACHE_FILE_NAME);
    let graph = CodeGraph::load_or_build(workspace.path(), cache).expect("build codegraph");
    assert!(matches!(
        graph.skipped_files().as_slice(),
        [SkippedFile {
            reason: SkipReason::TooLarge { .. },
            ..
        }]
    ));
}

#[test]
fn binary_source_is_skipped_with_marker() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    fs::write(workspace.path().join("binary.py"), b"def valid():\0pass\n")
        .expect("binary fixture");
    let cache = workspace.path().join("state").join(DEFAULT_CACHE_FILE_NAME);
    let graph = CodeGraph::load_or_build(workspace.path(), cache).expect("build codegraph");
    assert!(matches!(
        graph.skipped_files().as_slice(),
        [SkippedFile {
            reason: SkipReason::Binary,
            ..
        }]
    ));
}

#[test]
fn query_refreshes_changed_file_from_fingerprint() {
    let (workspace, mut graph) = fixture_graph(&[("src/lib.rs", "fn before() {}\n")]);
    assert!(has_symbol(&mut graph, "before", SymbolKind::Function));
    fs::write(workspace.path().join("src/lib.rs"), "fn after_edit() {}\n")
        .expect("edit fixture");
    assert!(has_symbol(&mut graph, "after_edit", SymbolKind::Function));
    assert!(!has_symbol(&mut graph, "before", SymbolKind::Function));
}

#[test]
fn cache_roundtrip_and_corrupt_fallback() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    fs::write(workspace.path().join("lib.rs"), "fn cached() {}\n").expect("fixture source");
    let cache = workspace.path().join("state").join(DEFAULT_CACHE_FILE_NAME);
    let mut first = CodeGraph::load_or_build(workspace.path(), &cache).expect("first build");
    assert!(has_symbol(&mut first, "cached", SymbolKind::Function));
    drop(first);

    let mut roundtrip = CodeGraph::load_or_build(workspace.path(), &cache).expect("cache load");
    assert!(has_symbol(&mut roundtrip, "cached", SymbolKind::Function));
    drop(roundtrip);

    fs::write(&cache, b"not-json").expect("corrupt cache");
    let mut rebuilt = CodeGraph::load_or_build(workspace.path(), &cache).expect("fallback rebuild");
    assert!(has_symbol(&mut rebuilt, "cached", SymbolKind::Function));
}
