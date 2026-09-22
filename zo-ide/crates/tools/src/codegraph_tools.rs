use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError, TryLockError};

use codegraph::{CodeGraph, IndexStatus, Symbol, SymbolKind, DEFAULT_CACHE_FILE_NAME};
use runtime::file_neighbours::MAX_NEIGHBOURS;
use runtime::{permission_enforcer::PermissionEnforcer, PermissionMode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};

pub(crate) const MAX_CODEGRAPH_RESULTS: usize = 200;
/// Defining files an `impact` refusal names when `file` was left out and the
/// name has several: enough to pick from, not an inventory.
const MAX_NAMED_DEFINERS: usize = 8;
const CODEGRAPH_CACHE_DIR_NAME: &str = "codegraph";

macro_rules! codegraph_description {
    ($specific:literal) => {
        concat!(
            $specific,
            " The index is tree-sitter based and covers Rust, TypeScript/TSX, JavaScript, Python, and Go. References are exact identifier-name matches, not semantic or scope-accurate resolution, and imports are not resolved across files. This complements an attached LSP; it does not replace LSP type-aware navigation."
        )
    };
}

#[derive(Debug, Deserialize)]
struct FindSymbolInput {
    name: String,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FindReferencesInput {
    name: String,
    /// The workspace-relative file defining `name`: narrows the answer to
    /// that definition's references (`CodeGraph::references_to`).
    #[serde(default)]
    file: Option<String>,
}

/// How a `find_references` answer was chosen — the output's `resolution`.
const RESOLUTION_EXACT_NAME: &str = "exact_name_match";
const RESOLUTION_DEFINITION_IN_FILE: &str = "definition_in_file";

#[derive(Debug, Deserialize)]
struct FileOutlineInput {
    path: String,
}

#[derive(Debug, Deserialize)]
struct ImpactInput {
    name: String,
    /// The workspace-relative file defining `name`; optional when only one
    /// file does.
    #[serde(default)]
    file: Option<String>,
}

#[derive(Debug, Serialize)]
struct ImpactFile {
    file: String,
    references: usize,
    test: bool,
}

#[derive(Debug, Serialize)]
struct ImpactOutput {
    name: String,
    file: String,
    definitions: Vec<SymbolMatch>,
    references: usize,
    referencing_files: usize,
    callers: usize,
    tests: usize,
    files: Vec<ImpactFile>,
    truncated: bool,
    resolution: &'static str,
    index: IndexStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_warning: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct SymbolMatch {
    file: String,
    line: usize,
    column: usize,
    kind: &'static str,
    container: Option<String>,
}

#[derive(Debug, Serialize)]
struct FindSymbolOutput {
    name: String,
    kind: Option<&'static str>,
    matches: Vec<SymbolMatch>,
    total_matches: usize,
    truncated: bool,
    index: IndexStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_warning: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct ReferenceMatch {
    line: usize,
    column: usize,
}

#[derive(Debug, Serialize)]
struct ReferenceGroup {
    file: String,
    references: Vec<ReferenceMatch>,
}

#[derive(Debug, Serialize)]
struct FindReferencesOutput {
    name: String,
    files: Vec<ReferenceGroup>,
    total_matches: usize,
    truncated: bool,
    resolution: &'static str,
    index: IndexStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_warning: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct OutlineNode {
    name: String,
    kind: &'static str,
    line: usize,
    column: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<String>,
    children: Vec<Self>,
}

#[derive(Debug, Serialize)]
struct FileOutlineOutput {
    path: String,
    indexed: bool,
    definitions: Vec<OutlineNode>,
    total_definitions: usize,
    index: IndexStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    index_warning: Option<&'static str>,
}

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "find_symbol",
            description: codegraph_description!(
                "Find definition names by exact spelling, optionally filtered by symbol kind. Returns file, one-based line/column, kind, and enclosing container; results are capped and report `truncated` honestly."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1 },
                    "kind": {
                        "type": "string",
                        "enum": ["fn", "struct", "trait", "class", "method", "const", "type", "mod", "interface", "enum", "union", "macro", "static"]
                    }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "find_references",
            description: codegraph_description!(
                "Find identifier occurrences with exactly the requested spelling, grouped by file. Pass `file` (the workspace-relative file that defines the name) to keep only that definition's references: occurrences in that file, in files whose imports name it, or all of them when no other file defines it. Results are capped and report `truncated` honestly."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1 },
                    "file": { "type": "string", "minLength": 1 }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "impact",
            description: codegraph_description!(
                "Measure what changing a definition reaches before changing it: the references of `name` as `file` defines it (narrowed as `find_references` with `file`), the files they sit in, how many are other files (callers) and how many are tests. `file` may be left out when only one file defines the name."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1 },
                    "file": { "type": "string", "minLength": 1 }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "file_outline",
            description: codegraph_description!(
                "Return the indexed definition tree for one workspace-relative source file, including symbol kinds and enclosing containers."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "minLength": 1 }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
    ]
}

pub(crate) fn dispatch(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<String, ToolError>> {
    match name {
        "find_symbol" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<FindSymbolInput>(input)
                    .and_then(|input| run_find_symbol(ctx, &input))
            }),
        ),
        "find_references" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<FindReferencesInput>(input)
                    .and_then(|input| run_find_references(ctx, &input))
            }),
        ),
        "file_outline" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<FileOutlineInput>(input)
                    .and_then(|input| run_file_outline(ctx, &input))
            }),
        ),
        "impact" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<ImpactInput>(input).and_then(|input| run_impact(ctx, &input))
            }),
        ),
        _ => None,
    }
}

fn run_find_symbol(ctx: &ToolContext, input: &FindSymbolInput) -> Result<String, ToolError> {
    let name = required_text("name", &input.name)?;
    let kind = input
        .kind
        .as_deref()
        .map(parse_symbol_kind)
        .transpose()?;
    with_codegraph(ctx, |graph| {
        let mut symbols = graph
            .find_symbols(name, kind)
            .map_err(|error| codegraph_error(&error))?;
        let total_matches = symbols.len();
        symbols.truncate(MAX_CODEGRAPH_RESULTS);
        let status = graph.status();
        let matches = symbols.into_iter().map(symbol_match).collect();
        to_pretty_json(FindSymbolOutput {
            name: name.to_string(),
            kind: kind.map(SymbolKind::as_str),
            matches,
            total_matches,
            truncated: total_matches > MAX_CODEGRAPH_RESULTS,
            index: status,
            index_warning: index_warning(status),
        })
    })
}

fn run_find_references(
    ctx: &ToolContext,
    input: &FindReferencesInput,
) -> Result<String, ToolError> {
    let name = required_text("name", &input.name)?;
    let file = input
        .file
        .as_deref()
        .map(|file| required_text("file", file))
        .transpose()?;
    with_codegraph(ctx, |graph| {
        let (mut references, resolution) = match file {
            Some(file) => (
                graph
                    .references_to(file, name)
                    .map_err(|error| codegraph_error(&error))?
                    .ok_or_else(|| {
                        ToolError::InvalidInput(format!(
                            "`{file}` defines no `{name}`; find_symbol names the files that do"
                        ))
                    })?,
                RESOLUTION_DEFINITION_IN_FILE,
            ),
            None => (
                graph
                    .find_references(name)
                    .map_err(|error| codegraph_error(&error))?,
                RESOLUTION_EXACT_NAME,
            ),
        };
        let total_matches = references.len();
        references.truncate(MAX_CODEGRAPH_RESULTS);
        let mut grouped = BTreeMap::<String, Vec<ReferenceMatch>>::new();
        for reference in references {
            grouped
                .entry(display_path(&reference.file))
                .or_default()
                .push(ReferenceMatch {
                    line: reference.range.start.row + 1,
                    column: reference.range.start.column + 1,
                });
        }
        let files = grouped
            .into_iter()
            .map(|(file, references)| ReferenceGroup { file, references })
            .collect();
        let status = graph.status();
        to_pretty_json(FindReferencesOutput {
            name: name.to_string(),
            files,
            total_matches,
            truncated: total_matches > MAX_CODEGRAPH_RESULTS,
            resolution,
            index: status,
            index_warning: index_warning(status),
        })
    })
}

fn run_impact(ctx: &ToolContext, input: &ImpactInput) -> Result<String, ToolError> {
    let name = required_text("name", &input.name)?;
    let file = input
        .file
        .as_deref()
        .map(|file| required_text("file", file))
        .transpose()?;
    with_codegraph(ctx, |graph| {
        let file = match file {
            Some(file) => std::path::PathBuf::from(file),
            None => sole_definer(graph, name)?,
        };
        let impact = graph
            .impact(&file, name)
            .map_err(|error| codegraph_error(&error))?
            .ok_or_else(|| {
                ToolError::InvalidInput(format!(
                    "`{}` defines no `{name}`; find_symbol names the files that do",
                    file.display()
                ))
            })?;
        let status = graph.status();
        let (referencing_files, callers, tests) =
            (impact.files.len(), impact.callers(), impact.tests());
        let files = impact
            .files
            .iter()
            .take(MAX_CODEGRAPH_RESULTS)
            .map(|linked| ImpactFile {
                file: display_path(&linked.file),
                references: linked.references,
                test: linked.test,
            })
            .collect();
        to_pretty_json(ImpactOutput {
            name: name.to_string(),
            file: display_path(&impact.file),
            definitions: impact.definitions.into_iter().map(symbol_match).collect(),
            references: impact.references,
            referencing_files,
            callers,
            tests,
            files,
            truncated: referencing_files > MAX_CODEGRAPH_RESULTS,
            resolution: RESOLUTION_DEFINITION_IN_FILE,
            index: status,
            index_warning: index_warning(status),
        })
    })
}

/// The one file defining `name`, or a refusal that says why there is none to
/// pick — nothing defines it, or several do (named, up to a handful).
fn sole_definer(graph: &mut CodeGraph, name: &str) -> Result<std::path::PathBuf, ToolError> {
    let mut definers = graph
        .find_symbols(name, None)
        .map_err(|error| codegraph_error(&error))?
        .into_iter()
        .map(|symbol| symbol.file)
        .collect::<Vec<_>>();
    definers.dedup();
    match definers.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(ToolError::InvalidInput(format!(
            "no indexed file defines `{name}`"
        ))),
        several => Err(ToolError::InvalidInput(format!(
            "{} files define `{name}` ({}{}); pass `file`",
            several.len(),
            several
                .iter()
                .take(MAX_NAMED_DEFINERS)
                .map(|file| display_path(file))
                .collect::<Vec<_>>()
                .join(", "),
            if several.len() > MAX_NAMED_DEFINERS { ", …" } else { "" }
        ))),
    }
}

fn run_file_outline(ctx: &ToolContext, input: &FileOutlineInput) -> Result<String, ToolError> {
    let path = required_text("path", &input.path)?;
    with_codegraph(ctx, |graph| {
        let symbols = graph
            .file_outline(path)
            .map_err(|error| codegraph_error(&error))?;
        let status = graph.status();
        let indexed = symbols.is_some();
        let symbols = symbols.unwrap_or_default();
        let total_definitions = symbols.len();
        to_pretty_json(FileOutlineOutput {
            path: path.to_string(),
            indexed,
            definitions: outline_tree(&symbols),
            total_definitions,
            index: status,
            index_warning: index_warning(status),
        })
    })
}

/// What the index adds to a file read's neighbour list: the files defining
/// names the read file uses, and the test files using names it defines,
/// absolute and most referenced first.
#[derive(Debug, Default)]
pub(crate) struct IndexNeighbours {
    pub(crate) uses: Vec<PathBuf>,
    pub(crate) tested_by: Vec<PathBuf>,
}

/// The index's neighbours for `file`, when the session already holds an
/// index of this workspace or one was already built on disk. A read never
/// builds an index, never walks the workspace, and never waits: if a
/// codegraph call holds the slot, the read goes without.
pub(crate) fn neighbours_for_read(
    slot: &Mutex<Option<CodeGraph>>,
    cwd: Option<&Path>,
    workspace_root: Option<&Path>,
    file: &Path,
) -> Option<IndexNeighbours> {
    let root = index_root(cwd, workspace_root).ok()?;
    let relative = file.strip_prefix(&root).ok()?;
    let mut slot = match slot.try_lock() {
        Ok(slot) => slot,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return None,
    };
    match slot.as_ref() {
        Some(graph) if graph.workspace_root() != root => return None,
        Some(_) => {}
        None => *slot = CodeGraph::open_existing(&root, codegraph_cache_path(&root)).ok()?,
    }
    let links = slot
        .as_mut()?
        .file_links(relative, MAX_NEIGHBOURS)
        .ok()??;
    Some(IndexNeighbours {
        uses: links.uses.iter().map(|linked| root.join(&linked.file)).collect(),
        tested_by: links
            .used_by
            .iter()
            .filter(|linked| linked.test)
            .map(|linked| root.join(&linked.file))
            .collect(),
    })
}

fn with_codegraph<T>(
    ctx: &ToolContext,
    operation: impl FnOnce(&mut CodeGraph) -> Result<T, ToolError>,
) -> Result<T, ToolError> {
    let root = index_root(ctx.cwd.as_deref(), ctx.workspace_root.as_deref())?;
    let mut slot = ctx
        .codegraph
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let must_replace = slot
        .as_ref()
        .is_some_and(|graph| graph.workspace_root() != root);
    if must_replace {
        *slot = None;
    }
    if slot.is_none() {
        *slot = Some(
            CodeGraph::load_or_build(&root, codegraph_cache_path(&root))
                .map_err(|error| codegraph_error(&error))?,
        );
    }
    operation(slot.as_mut().expect("codegraph initialized above"))
}

/// Where a workspace's index lives: the project's zo state, never the tree.
/// The codegraph tools, a read's neighbours and `zo vault code` open this one
/// file.
#[must_use]
pub fn codegraph_cache_path(root: &Path) -> PathBuf {
    runtime::zo_project_state_dir(root)
        .join(CODEGRAPH_CACHE_DIR_NAME)
        .join(DEFAULT_CACHE_FILE_NAME)
}

/// The workspace an index covers: the session's working directory, else its
/// workspace root, else the process's, canonicalized.
fn index_root(cwd: Option<&Path>, workspace_root: Option<&Path>) -> Result<PathBuf, ToolError> {
    let root = cwd
        .or(workspace_root)
        .map(Path::to_path_buf)
        .map_or_else(std::env::current_dir, Ok)?;
    fs::canonicalize(&root).map_err(|error| {
        ToolError::Execution(format!(
            "cannot resolve codegraph workspace {}: {error}",
            root.display()
        ))
    })
}

fn parse_symbol_kind(kind: &str) -> Result<SymbolKind, ToolError> {
    SymbolKind::from_label(kind.trim()).ok_or_else(|| {
        ToolError::InvalidInput(format!(
            "unknown symbol kind `{kind}`; expected fn, struct, trait, class, method, const, type, mod, interface, enum, union, macro, or static"
        ))
    })
}

fn required_text<'a>(field: &str, value: &'a str) -> Result<&'a str, ToolError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "`{field}` must not be empty"
        )));
    }
    Ok(value)
}

fn symbol_match(symbol: Symbol) -> SymbolMatch {
    SymbolMatch {
        file: display_path(&symbol.file),
        line: symbol.range.start.row + 1,
        column: symbol.range.start.column + 1,
        kind: symbol.kind.as_str(),
        container: symbol.container,
    }
}

fn outline_tree(symbols: &[Symbol]) -> Vec<OutlineNode> {
    let mut by_name = BTreeMap::<&str, Vec<usize>>::new();
    for (index, symbol) in symbols.iter().enumerate() {
        by_name.entry(&symbol.name).or_default().push(index);
    }
    let parents = symbols
        .iter()
        .enumerate()
        .map(|(index, symbol)| {
            symbol.container.as_deref().and_then(|container| {
                by_name.get(container).and_then(|candidates| {
                    candidates
                        .iter()
                        .copied()
                        .filter(|candidate| *candidate != index)
                        .min_by_key(|candidate| {
                            symbols[index]
                                .range
                                .start
                                .row
                                .abs_diff(symbols[*candidate].range.start.row)
                        })
                })
            })
        })
        .collect::<Vec<_>>();
    let mut children = vec![Vec::new(); symbols.len()];
    for (child, parent) in parents.iter().enumerate() {
        if let Some(parent) = parent {
            children[*parent].push(child);
        }
    }
    parents
        .iter()
        .enumerate()
        .filter(|(_, parent)| parent.is_none())
        .map(|(index, _)| build_outline_node(index, symbols, &children))
        .collect()
}

fn build_outline_node(index: usize, symbols: &[Symbol], children: &[Vec<usize>]) -> OutlineNode {
    let symbol = &symbols[index];
    OutlineNode {
        name: symbol.name.clone(),
        kind: symbol.kind.as_str(),
        line: symbol.range.start.row + 1,
        column: symbol.range.start.column + 1,
        container: symbol.container.clone(),
        children: children[index]
            .iter()
            .map(|child| build_outline_node(*child, symbols, children))
            .collect(),
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

const fn index_warning(status: IndexStatus) -> Option<&'static str> {
    if status.file_limit_reached {
        Some("workspace exceeded MAX_INDEXED_FILES; results exclude files beyond the index cap")
    } else {
        None
    }
}

fn codegraph_error(error: &codegraph::CodeGraphError) -> ToolError {
    ToolError::Execution(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;
    use crate::{mvp_tool_specs, GlobalToolRegistry};

    fn context_with_source(source: &str) -> (tempfile::TempDir, ToolContext) {
        let workspace = tempfile::tempdir().expect("temp workspace");
        fs::write(workspace.path().join("lib.rs"), source).expect("fixture source");
        let graph = CodeGraph::load_or_build(
            workspace.path(),
            workspace.path().join("cache").join(DEFAULT_CACHE_FILE_NAME),
        )
        .expect("fixture graph");
        let ctx = ToolContext::new().with_cwd(workspace.path());
        *ctx.codegraph
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(graph);
        (workspace, ctx)
    }

    #[test]
    fn codegraph_specs_are_read_only_deferred_and_honest() {
        let specs = mvp_tool_specs();
        for name in ["find_symbol", "find_references", "file_outline", "impact"] {
            let spec = specs
                .iter()
                .find(|spec| spec.name == name)
                .expect("registered codegraph spec");
            assert_eq!(spec.required_permission, PermissionMode::ReadOnly);
            assert!(spec.description.contains("tree-sitter"));
            assert!(spec.description.contains("exact identifier-name matches"));
            assert!(spec.description.contains("does not replace LSP"));
        }
        let advertised = GlobalToolRegistry::builtin()
            .definitions(None)
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        assert!(!advertised.iter().any(|name| name == "find_symbol"));
    }

    #[test]
    fn find_symbol_parses_and_queries_the_session_index() {
        let (_workspace, ctx) = context_with_source("struct Widget;\n");

        let output = dispatch(
            &ctx,
            None,
            "find_symbol",
            &json!({ "name": "Widget", "kind": "struct" }),
        )
        .expect("handled tool")
        .expect("successful query");
        let output: Value = serde_json::from_str(&output).expect("JSON output");
        assert_eq!(output["total_matches"], 1);
        assert_eq!(output["matches"][0]["file"], "lib.rs");
        assert_eq!(output["matches"][0]["line"], 1);
        assert_eq!(output["truncated"], false);
    }

    #[test]
    fn reference_cap_and_file_outline_are_explicit() {
        let calls = "target();\n".repeat(MAX_CODEGRAPH_RESULTS + 1);
        let source = format!(
            "struct Widget;\nimpl Widget {{ fn build() {{ target(); }} }}\nfn target() {{}}\nfn caller() {{ {calls} }}\n"
        );
        let (_workspace, ctx) = context_with_source(&source);

        let references = dispatch(
            &ctx,
            None,
            "find_references",
            &json!({ "name": "target" }),
        )
        .expect("handled references")
        .expect("successful references");
        let references: Value = serde_json::from_str(&references).expect("JSON references");
        assert_eq!(references["total_matches"], MAX_CODEGRAPH_RESULTS + 2);
        assert_eq!(references["truncated"], true);
        assert_eq!(
            references["files"][0]["references"]
                .as_array()
                .expect("reference array")
                .len(),
            MAX_CODEGRAPH_RESULTS
        );

        let outline = dispatch(
            &ctx,
            None,
            "file_outline",
            &json!({ "path": "lib.rs" }),
        )
        .expect("handled outline")
        .expect("successful outline");
        let outline: Value = serde_json::from_str(&outline).expect("JSON outline");
        let widget = outline["definitions"]
            .as_array()
            .expect("definition array")
            .iter()
            .find(|definition| definition["name"] == "Widget")
            .expect("Widget outline");
        assert!(widget["children"]
            .as_array()
            .expect("children")
            .iter()
            .any(|child| child["name"] == "build"));
    }

    /// A whole-file read lists what the index links the file to — the file
    /// defining a name it imports, the test importing a name it defines —
    /// beyond what its own text resolves (the test sits outside every naming
    /// convention); a window further in lists nothing.
    #[test]
    fn a_whole_file_read_lists_what_the_index_links_it_to() {
        let workspace = tempfile::tempdir().expect("temp workspace");
        for (path, source) in [
            (
                "src/graph.rs",
                "use crate::util::shared_util;\npub fn scan_graph() { shared_util(); }\n",
            ),
            ("src/util.rs", "pub fn shared_util() {}\n"),
            (
                "tests/graph_suite.rs",
                "use acme::graph::scan_graph;\nfn covers() { scan_graph(); }\n",
            ),
        ] {
            let path = workspace.path().join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("fixture dir");
            fs::write(path, source).expect("fixture source");
        }
        let graph = CodeGraph::load_or_build(
            workspace.path(),
            workspace.path().join("cache").join(DEFAULT_CACHE_FILE_NAME),
        )
        .expect("fixture graph");
        let root = graph.workspace_root().to_path_buf();
        let ctx = ToolContext::new().with_cwd(workspace.path());
        *ctx.codegraph
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(graph);

        let read = |input: Value| -> Value {
            let output = crate::file_tools::dispatch(&ctx, None, "read_file", &input)
                .expect("handled read")
                .expect("successful read");
            serde_json::from_str(&output).expect("JSON read")
        };
        let whole = read(json!({ "path": "src/graph.rs" }));
        let neighbours = whole["file"]["neighbours"]
            .as_array()
            .expect("neighbours")
            .iter()
            .map(|neighbour| {
                (
                    neighbour["relation"].as_str().expect("relation").to_owned(),
                    neighbour["path"].as_str().expect("path").to_owned(),
                )
            })
            .collect::<Vec<_>>();
        let at = |relative: &str| root.join(relative).to_string_lossy().into_owned();
        assert!(neighbours.contains(&("imports".to_owned(), at("src/util.rs"))));
        assert!(neighbours.contains(&("tests".to_owned(), at("tests/graph_suite.rs"))));

        let window = read(json!({ "path": "src/graph.rs", "offset": 1 }));
        assert!(window["file"]["neighbours"].is_null());
    }

    /// With `file`, the answer is that definition's references; a file that
    /// does not define the name is refused rather than answered empty.
    #[test]
    fn find_references_narrows_to_the_definition_a_file_holds() {
        let workspace = tempfile::tempdir().expect("temp workspace");
        for (path, source) in [
            ("a.rs", "pub fn build() {}\n"),
            ("b.rs", "pub fn build() {}\n"),
            ("c.rs", "use crate::a::build;\nfn c() { build(); }\n"),
            ("d.rs", "fn d(thing: Thing) { thing.build(); }\n"),
        ] {
            fs::write(workspace.path().join(path), source).expect("fixture source");
        }
        let graph = CodeGraph::load_or_build(
            workspace.path(),
            workspace.path().join("cache").join(DEFAULT_CACHE_FILE_NAME),
        )
        .expect("fixture graph");
        let ctx = ToolContext::new().with_cwd(workspace.path());
        *ctx.codegraph
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(graph);

        let narrowed = dispatch(
            &ctx,
            None,
            "find_references",
            &json!({ "name": "build", "file": "a.rs" }),
        )
        .expect("handled references")
        .expect("successful references");
        let narrowed: Value = serde_json::from_str(&narrowed).expect("JSON references");
        assert_eq!(narrowed["resolution"], RESOLUTION_DEFINITION_IN_FILE);
        assert_eq!(narrowed["total_matches"], 2);
        assert_eq!(narrowed["files"][0]["file"], "c.rs");

        let spelled = dispatch(&ctx, None, "find_references", &json!({ "name": "build" }))
            .expect("handled references")
            .expect("successful references");
        let spelled: Value = serde_json::from_str(&spelled).expect("JSON references");
        assert_eq!(spelled["resolution"], RESOLUTION_EXACT_NAME);
        assert_eq!(spelled["total_matches"], 3);

        let error = dispatch(
            &ctx,
            None,
            "find_references",
            &json!({ "name": "build", "file": "c.rs" }),
        )
        .expect("handled references")
        .expect_err("c.rs defines no build");
        assert!(matches!(error, ToolError::InvalidInput(_)));
    }

    /// `impact` counts one definition's callers and tests; without `file` it
    /// takes the only definer, and refuses a name several files define.
    #[test]
    fn impact_counts_callers_and_tests_and_asks_which_definition() {
        let workspace = tempfile::tempdir().expect("temp workspace");
        fs::create_dir_all(workspace.path().join("tests")).expect("tests dir");
        for (path, source) in [
            ("a.rs", "pub fn build() {}\npub fn only_here() {}\n"),
            ("b.rs", "pub fn build() {}\n"),
            ("c.rs", "use crate::a::build;\nfn c() { build(); only_here(); }\n"),
            ("tests/t.rs", "use acme::a::only_here;\nfn t() { only_here(); }\n"),
        ] {
            fs::write(workspace.path().join(path), source).expect("fixture source");
        }
        let graph = CodeGraph::load_or_build(
            workspace.path(),
            workspace.path().join("cache").join(DEFAULT_CACHE_FILE_NAME),
        )
        .expect("fixture graph");
        let ctx = ToolContext::new().with_cwd(workspace.path());
        *ctx.codegraph
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(graph);

        let answer = dispatch(&ctx, None, "impact", &json!({ "name": "only_here" }))
            .expect("handled impact")
            .expect("one definer: no file needed");
        let answer: Value = serde_json::from_str(&answer).expect("JSON impact");
        assert_eq!(answer["file"], "a.rs");
        assert_eq!(answer["callers"], 2);
        assert_eq!(answer["tests"], 1);
        assert_eq!(answer["resolution"], RESOLUTION_DEFINITION_IN_FILE);

        let narrowed = dispatch(
            &ctx,
            None,
            "impact",
            &json!({ "name": "build", "file": "a.rs" }),
        )
        .expect("handled impact")
        .expect("a.rs defines build");
        let narrowed: Value = serde_json::from_str(&narrowed).expect("JSON impact");
        assert_eq!(narrowed["references"], 2);
        assert_eq!(narrowed["files"][0]["file"], "c.rs");

        let error = dispatch(&ctx, None, "impact", &json!({ "name": "build" }))
            .expect("handled impact")
            .expect_err("two definers");
        assert!(
            matches!(&error, ToolError::InvalidInput(message) if message.contains("a.rs") && message.contains("b.rs")),
            "{error:?}"
        );
    }

    #[test]
    fn find_symbol_rejects_unknown_kind() {
        let ctx = ToolContext::new();
        let error = dispatch(
            &ctx,
            None,
            "find_symbol",
            &json!({ "name": "Widget", "kind": "banana" }),
        )
        .expect("handled tool")
        .expect_err("invalid kind");
        assert!(matches!(error, ToolError::InvalidInput(_)));
    }
}
