use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use crate::extract::extract;
use crate::index::LEGACY_CACHE_FILE_NAMES;
use crate::language::spec_for_path;

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
        graph.skipped_files().expect("skipped files").as_slice(),
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
        graph.skipped_files().expect("skipped files").as_slice(),
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

/// Every language, with a multi-byte identifier and a multi-byte string ahead
/// of references on the same row: tree-sitter's columns are bytes, and the
/// index derives a reference's end from its start and the name's byte length.
const FIDELITY_FILES: [(&str, &str); 4] = [
    (
        "src/lib.rs",
        "use crate::model::{Symbol, SymbolKind};\nmod model;\ntrait Speaker { fn speak(&self); }\nstruct Dog { name: String }\nimpl Speaker for Dog {\n    fn speak(&self) { let label = \"멍멍\"; helper(label, &self.name); }\n}\nfn helper(_: &str, _: &String) {}\n",
    ),
    (
        "web/main.ts",
        "import { helper } from './helper';\nconst greeting = '안녕'; const answer = helper(greeting);\ninterface Runner { run(): void; }\nclass Job implements Runner { run() { helper(answer); } }\n",
    ),
    (
        "tools/계산.py",
        "from .shared import helper\n\ndef 계산(값):\n    return helper(값)\n\nclass Worker:\n    def run(self):\n        계산(1)\n",
    ),
    (
        "svc/main.go",
        "package svc\n\nimport (\n\tfmtAlias \"fmt\"\n\t\"strings\"\n)\n\ntype Worker struct{}\n\nfunc (w Worker) Run() { fmtAlias.Println(strings.ToUpper(\"é\")); helper() }\nfunc helper() {}\n",
    ),
];

#[test]
fn the_index_answers_exactly_what_extraction_found() {
    let (_workspace, mut graph) = fixture_graph(&FIDELITY_FILES);
    for (path, source) in FIDELITY_FILES {
        let spec = spec_for_path(Path::new(path)).expect("supported fixture");
        let extracted = extract(Path::new(path), source.as_bytes(), spec).expect("extract fixture");
        assert!(!extracted.references.is_empty(), "{path} has references");
        for reference in &extracted.references {
            let found = graph
                .find_references(&reference.name)
                .expect("reference query");
            assert!(found.contains(reference), "{path}: {reference:?} not answered");
        }
        for symbol in &extracted.symbols {
            let found = graph
                .find_symbols(&symbol.name, Some(symbol.kind))
                .expect("symbol query");
            assert!(found.contains(symbol), "{path}: {symbol:?} not answered");
        }
        assert_eq!(
            graph.file_outline(path).expect("outline query"),
            Some(extracted.symbols.clone())
        );
        assert_eq!(
            graph.file_imports(path).expect("import query"),
            Some(extracted.imports.clone())
        );
    }
}

#[test]
fn references_come_in_path_order_then_source_order() {
    let (_workspace, mut graph) = fixture_graph(&[
        ("src/z.rs", "fn z() { target(); target(); }\n"),
        ("src/a.rs", "fn a() { target(); }\nfn target() {}\n"),
        ("src/m/n.rs", "fn n() { target(); }\n"),
    ]);
    let found = graph.find_references("target").expect("reference query");
    let spots = found
        .iter()
        .map(|reference| {
            (
                reference.file.clone(),
                reference.range.start.row,
                reference.range.start.column,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        spots,
        vec![
            (PathBuf::from("src/a.rs"), 0, 9),
            (PathBuf::from("src/m/n.rs"), 0, 9),
            (PathBuf::from("src/z.rs"), 0, 9),
            (PathBuf::from("src/z.rs"), 0, 19),
        ]
    );
}

#[test]
fn a_refresh_writes_only_what_changed() {
    let (workspace, mut graph) = fixture_graph(&[
        ("src/a.rs", "fn alpha() {}\n"),
        ("src/b.rs", "fn beta() { alpha(); }\n"),
    ]);
    assert_eq!(
        graph.refresh().expect("unchanged refresh"),
        RefreshSummary::default()
    );

    fs::write(workspace.path().join("src/c.rs"), "fn gamma() { alpha(); }\n")
        .expect("add a file");
    assert_eq!(
        graph.refresh().expect("refresh after an add"),
        RefreshSummary {
            written: 1,
            removed: 0
        }
    );
    assert_eq!(graph.find_references("alpha").expect("references").len(), 2);

    fs::remove_file(workspace.path().join("src/b.rs")).expect("remove a file");
    assert_eq!(
        graph.refresh().expect("refresh after a removal"),
        RefreshSummary {
            written: 0,
            removed: 1
        }
    );
    let files = graph
        .find_references("alpha")
        .expect("references")
        .into_iter()
        .map(|reference| reference.file)
        .collect::<Vec<_>>();
    assert_eq!(files, vec![PathBuf::from("src/c.rs")]);
    assert!(!has_symbol(&mut graph, "beta", SymbolKind::Function));
    assert_eq!(graph.status().indexed_files, 2);
}

#[test]
fn two_sessions_share_one_index() {
    let (workspace, mut first) = fixture_graph(&[("lib.rs", "fn shared() {}\n")]);
    let cache = workspace.path().join("state").join(DEFAULT_CACHE_FILE_NAME);
    let mut second = CodeGraph::load_or_build(workspace.path(), &cache).expect("second session");

    fs::write(workspace.path().join("lib.rs"), "fn edited() {}\n").expect("edit");
    assert_eq!(first.refresh().expect("first session refresh").written, 1);
    // The second session reads the rows the first one wrote instead of
    // parsing the file again.
    assert_eq!(
        second.refresh().expect("second session refresh"),
        RefreshSummary::default()
    );
    assert!(has_symbol(&mut second, "edited", SymbolKind::Function));
    assert!(!has_symbol(&mut second, "shared", SymbolKind::Function));
}

#[test]
fn an_old_cache_is_removed_and_another_workspace_rebuilds() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    fs::write(workspace.path().join("lib.rs"), "fn first_root() {}\n").expect("fixture");
    let state = tempfile::tempdir().expect("temp state");
    for legacy in LEGACY_CACHE_FILE_NAMES {
        fs::write(state.path().join(legacy), b"{}").expect("legacy cache");
    }
    let cache = state.path().join(DEFAULT_CACHE_FILE_NAME);
    let mut graph = CodeGraph::load_or_build(workspace.path(), &cache).expect("build");
    assert!(has_symbol(&mut graph, "first_root", SymbolKind::Function));
    for legacy in LEGACY_CACHE_FILE_NAMES {
        assert!(!state.path().join(legacy).exists(), "{legacy} removed");
    }
    drop(graph);

    let other = tempfile::tempdir().expect("other workspace");
    fs::write(other.path().join("lib.rs"), "fn second_root() {}\n").expect("fixture");
    let mut reused = CodeGraph::load_or_build(other.path(), &cache).expect("rebuild");
    assert!(has_symbol(&mut reused, "second_root", SymbolKind::Function));
    assert!(!has_symbol(&mut reused, "first_root", SymbolKind::Function));
}

#[test]
fn file_links_follow_names_only_one_file_defines_and_the_user_imports() {
    let (_workspace, mut graph) = fixture_graph(&[
        (
            "src/graph.rs",
            "use crate::util::shared_util;\npub fn scan_graph() {}\npub fn new() {}\nfn helper() { shared_util(); shared_util(); lonely(); }\n",
        ),
        (
            "src/util.rs",
            "pub fn shared_util() {}\npub fn new() {}\npub fn lonely() {}\n",
        ),
        ("src/app.rs", "use crate::graph::{new, scan_graph};\nfn run() { scan_graph(); new(); }\n"),
        (
            "tests/graph.rs",
            "use zo::graph::scan_graph;\nfn covers() { scan_graph(); scan_graph(); }\n",
        ),
        ("src/unimported.rs", "fn call() { scan_graph(); }\n"),
    ]);
    let links = graph
        .file_links("src/graph.rs", MAX_INDEXED_FILES)
        .expect("links query")
        .expect("current file");
    // `shared_util` has one definer and graph.rs imports it (the `use` line
    // is one of its three occurrences). `lonely` has one definer but no
    // import spells it; `new` has two definers. Neither links.
    assert_eq!(
        links.uses,
        vec![LinkedFile {
            file: PathBuf::from("src/util.rs"),
            references: 3,
            test: false,
        }]
    );
    // unimported.rs spells `scan_graph` without importing it: not a link.
    assert_eq!(
        links.used_by,
        vec![
            LinkedFile {
                file: PathBuf::from("tests/graph.rs"),
                references: 3,
                test: true,
            },
            LinkedFile {
                file: PathBuf::from("src/app.rs"),
                references: 2,
                test: false,
            },
        ]
    );
    let first = graph
        .file_links("src/graph.rs", 1)
        .expect("links query")
        .expect("current file");
    assert_eq!(first.used_by.len(), 1, "cut at the limit, most referenced kept");
    assert_eq!(first.used_by[0].file, PathBuf::from("tests/graph.rs"));
}

#[test]
fn references_to_keep_what_the_defining_file_or_an_import_vouches_for() {
    let (_workspace, mut graph) = fixture_graph(&[
        ("src/a.rs", "pub fn build() {}\nfn local() { build(); }\n"),
        ("src/b.rs", "pub fn build() {}\n"),
        ("src/c.rs", "use crate::a::build;\nfn c() { build(); }\n"),
        ("src/d.rs", "fn d(thing: Thing) { thing.build(); }\n"),
        ("src/e.rs", "pub fn only_here() {}\n"),
        ("src/f.rs", "fn f() { only_here(); }\n"),
    ]);
    let files = |references: Vec<Reference>| {
        references
            .into_iter()
            .map(|reference| reference.file)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        graph.find_references("build").expect("spelling").len(),
        4,
        "the spelling alone answers every file"
    );
    // Two files define `build`: only a.rs itself and the file whose import
    // names it are a.rs's; d.rs's method call is not.
    assert_eq!(
        files(
            graph
                .references_to("src/a.rs", "build")
                .expect("query")
                .expect("a.rs defines build")
        ),
        vec![
            PathBuf::from("src/a.rs"),
            PathBuf::from("src/c.rs"),
            PathBuf::from("src/c.rs"),
        ]
    );
    // One file defines `only_here`: every occurrence is its.
    assert_eq!(
        files(
            graph
                .references_to("src/e.rs", "only_here")
                .expect("query")
                .expect("e.rs defines only_here")
        ),
        vec![PathBuf::from("src/f.rs")]
    );
    assert_eq!(graph.references_to("src/f.rs", "only_here").expect("query"), None);
}

#[test]
fn impact_counts_the_callers_and_tests_one_definition_reaches() {
    let (_workspace, mut graph) = fixture_graph(&[
        ("src/a.rs", "pub fn build() {}\nfn local() { build(); }\n"),
        ("src/b.rs", "pub fn build() {}\n"),
        ("src/c.rs", "use crate::a::build;\nfn c() { build(); }\n"),
        ("src/d.rs", "fn d(thing: Thing) { thing.build(); }\n"),
        ("tests/build.rs", "use acme::a::build;\nfn t() { build(); build(); }\n"),
    ]);
    let impact = graph
        .impact("src/a.rs", "build")
        .expect("impact query")
        .expect("a.rs defines build");
    assert_eq!(impact.file, PathBuf::from("src/a.rs"));
    assert_eq!(impact.definitions.len(), 1);
    assert_eq!(impact.references, 6);
    assert_eq!(
        impact
            .files
            .iter()
            .map(|linked| (linked.file.clone(), linked.references, linked.test))
            .collect::<Vec<_>>(),
        vec![
            (PathBuf::from("tests/build.rs"), 3, true),
            (PathBuf::from("src/c.rs"), 2, false),
            (PathBuf::from("src/a.rs"), 1, false),
        ]
    );
    assert_eq!((impact.callers(), impact.tests()), (2, 1));
    assert_eq!(graph.impact("src/d.rs", "build").expect("impact query"), None);
}

#[test]
fn a_mention_resolves_only_to_code_the_index_can_place() {
    let (_workspace, mut graph) = fixture_graph(&[
        ("crates/core/src/scan.rs", "pub fn scan_workspace() {}\npub fn new() {}\n"),
        ("crates/core/src/graph.rs", "pub struct GraphCache;\nimpl GraphCache { pub fn scan() {} }\npub fn new() {}\n"),
        ("crates/other/src/scan.rs", "fn other() {}\n"),
        ("crates/core/src/lone.rs", "fn lone() {}\n"),
    ]);
    let mentions = [
        "crates/core/src/scan.rs",
        "core/src/scan.rs",
        "scan.rs",
        "lone.rs",
        "scan_workspace",
        "GraphCache::scan",
        "graph::GraphCache",
        "new",
        "crates/core",
        "nowhere_at_all",
    ]
    .map(str::to_string);
    let resolved = graph.resolve_mentions(&mentions).expect("resolve");
    let file = |path: &str| Some(Resolved::File(PathBuf::from(path)));
    assert_eq!(resolved[0], file("crates/core/src/scan.rs"), "exact path");
    assert_eq!(resolved[1], file("crates/core/src/scan.rs"), "the one path ending so");
    assert_eq!(resolved[2], None, "two files are called scan.rs");
    assert_eq!(resolved[3], file("crates/core/src/lone.rs"));
    let symbol = |at: usize| match &resolved[at] {
        Some(Resolved::Symbol(symbol)) => Some((symbol.name.clone(), symbol.file.clone())),
        _ => None,
    };
    assert_eq!(
        symbol(4),
        Some(("scan_workspace".to_string(), PathBuf::from("crates/core/src/scan.rs")))
    );
    assert_eq!(
        symbol(5),
        Some(("scan".to_string(), PathBuf::from("crates/core/src/graph.rs")))
    );
    assert_eq!(
        symbol(6),
        Some(("GraphCache".to_string(), PathBuf::from("crates/core/src/graph.rs")))
    );
    assert_eq!(resolved[7], None, "two files define `new`");
    assert_eq!(resolved[8], None, "a folder is not a file");
    assert_eq!(resolved[9], None);
}

#[test]
fn an_import_spells_a_name_as_a_whole_identifier_or_as_what_it_binds() {
    let import = |path: &str, name: Option<&str>| Import {
        path: path.to_string(),
        name: name.map(str::to_string),
        file: PathBuf::from("lib.rs"),
        range: SourceRange {
            start: Position { row: 0, column: 0 },
            end: Position { row: 0, column: 0 },
            start_byte: 0,
            end_byte: 0,
        },
    };
    let scan = import("crate::scan::{fingerprint, scan_workspace}", None);
    assert!(scan.spells("scan_workspace") && scan.spells("fingerprint"));
    // Every whole identifier of the path, the module's own name included…
    assert!(scan.spells("scan"));
    // …and never a part of one.
    assert!(!scan.spells("scan_work") && !import("crate::scanner::Scan", None).spells("scan"));
    assert!(import(".shared", Some("helper")).spells("helper"));
}

#[test]
fn file_links_never_describe_a_file_saved_since_the_last_refresh() {
    let (workspace, mut graph) = fixture_graph(&[
        ("src/a.rs", "fn alpha() {}\n"),
        ("src/b.rs", "fn beta() { alpha(); }\n"),
    ]);
    assert!(graph
        .file_links("src/b.rs", MAX_INDEXED_FILES)
        .expect("links query")
        .is_some());
    fs::write(workspace.path().join("src/b.rs"), "fn beta() {}\n").expect("save");
    assert_eq!(
        graph
            .file_links("src/b.rs", MAX_INDEXED_FILES)
            .expect("links query"),
        None
    );
    graph.refresh().expect("refresh");
    assert_eq!(
        graph
            .file_links("src/b.rs", MAX_INDEXED_FILES)
            .expect("links query"),
        Some(FileLinks::default())
    );
}

#[test]
fn open_existing_reads_a_built_index_and_never_builds_one() {
    let workspace = tempfile::tempdir().expect("temp workspace");
    fs::write(workspace.path().join("lib.rs"), "fn built() {}\n").expect("fixture");
    let cache = workspace.path().join("state").join(DEFAULT_CACHE_FILE_NAME);
    assert!(CodeGraph::open_existing(workspace.path(), &cache)
        .expect("open")
        .is_none());
    assert!(!cache.exists(), "a missing index stays missing");

    drop(CodeGraph::load_or_build(workspace.path(), &cache).expect("build"));
    let mut opened = CodeGraph::open_existing(workspace.path(), &cache)
        .expect("open")
        .expect("built index");
    assert!(has_symbol(&mut opened, "built", SymbolKind::Function));

    let other = tempfile::tempdir().expect("other workspace");
    assert!(CodeGraph::open_existing(other.path(), &cache)
        .expect("open")
        .is_none());
}
