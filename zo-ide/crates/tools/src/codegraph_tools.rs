use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use codegraph::{CodeGraph, IndexStatus, Symbol, SymbolKind, DEFAULT_CACHE_FILE_NAME};
use runtime::{permission_enforcer::PermissionEnforcer, PermissionMode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};

pub(crate) const MAX_CODEGRAPH_RESULTS: usize = 200;
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
}

#[derive(Debug, Deserialize)]
struct FileOutlineInput {
    path: String,
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
                "Find identifier occurrences with exactly the requested spelling, grouped by file. Results are capped and report `truncated` honestly."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1 }
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
    with_codegraph(ctx, |graph| {
        let mut references = graph
            .find_references(name)
            .map_err(|error| codegraph_error(&error))?;
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
            resolution: "exact_name_match",
            index: status,
            index_warning: index_warning(status),
        })
    })
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

fn with_codegraph<T>(
    ctx: &ToolContext,
    operation: impl FnOnce(&mut CodeGraph) -> Result<T, ToolError>,
) -> Result<T, ToolError> {
    let root = workspace_root(ctx)?;
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
        let cache_path = runtime::zo_project_state_dir(&root)
            .join(CODEGRAPH_CACHE_DIR_NAME)
            .join(DEFAULT_CACHE_FILE_NAME);
        *slot = Some(
            CodeGraph::load_or_build(&root, cache_path)
                .map_err(|error| codegraph_error(&error))?,
        );
    }
    operation(slot.as_mut().expect("codegraph initialized above"))
}

fn workspace_root(ctx: &ToolContext) -> Result<PathBuf, ToolError> {
    let root = ctx
        .cwd
        .as_deref()
        .or(ctx.workspace_root.as_deref())
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
        for name in ["find_symbol", "find_references", "file_outline"] {
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
