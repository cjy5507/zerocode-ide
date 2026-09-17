use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::index::CodeGraphError;
use crate::language::{node_text, LanguageSpec};
use crate::model::{ExtractedFile, Import, Reference, SourceRange, Symbol, SymbolKind};

struct QuerySet {
    definitions: Query,
    imports: Query,
    references: Query,
}

type QueryCache = HashMap<(&'static str, &'static str), Arc<QuerySet>>;

fn query_cache() -> &'static Mutex<QueryCache> {
    static CACHE: OnceLock<Mutex<QueryCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn query_set(
    spec: &'static dyn LanguageSpec,
    path: &Path,
) -> Result<Arc<QuerySet>, CodeGraphError> {
    let key = (spec.name(), spec.dialect(path));
    if let Some(queries) = query_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned()
    {
        return Ok(queries);
    }

    let language = spec.language(path);
    let compile = |kind, source| {
        Query::new(&language, source).map_err(|error| CodeGraphError::Query {
            language: spec.dialect(path),
            kind,
            message: error.to_string(),
        })
    };
    let queries = Arc::new(QuerySet {
        definitions: compile("definitions", spec.definitions_query(path))?,
        imports: compile("imports", spec.imports_query(path))?,
        references: compile("references", spec.references_query(path))?,
    });
    query_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, Arc::clone(&queries));
    Ok(queries)
}

pub(crate) fn extract(
    file: &Path,
    source: &[u8],
    spec: &'static dyn LanguageSpec,
) -> Result<ExtractedFile, CodeGraphError> {
    let mut parser = Parser::new();
    let language = spec.language(file);
    parser
        .set_language(&language)
        .map_err(|error| CodeGraphError::Language {
            language: spec.dialect(file),
            message: error.to_string(),
        })?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| CodeGraphError::Parse(file.to_path_buf()))?;
    let queries = query_set(spec, file)?;
    let symbols = extract_symbols(
        file,
        source,
        tree.root_node(),
        &queries.definitions,
        spec,
    );
    let imports = extract_imports(file, source, tree.root_node(), &queries.imports);
    let references = extract_references(
        file,
        source,
        tree.root_node(),
        &queries.references,
        &symbols,
    );
    Ok(ExtractedFile {
        language: spec.name().to_string(),
        symbols,
        imports,
        references,
    })
}

fn extract_symbols(
    file: &Path,
    source: &[u8],
    root: tree_sitter::Node<'_>,
    query: &Query,
    spec: &'static dyn LanguageSpec,
) -> Vec<Symbol> {
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    let mut symbols = BTreeMap::<(usize, usize), Symbol>::new();
    while let Some(query_match) = matches.next() {
        for capture in query_match.captures {
            let capture_name = capture_names[capture.index as usize];
            let Some(kind) = SymbolKind::from_capture(capture_name) else {
                continue;
            };
            let Some(name) = node_text(capture.node, source) else {
                continue;
            };
            let symbol = Symbol {
                name,
                kind,
                file: file.to_path_buf(),
                range: SourceRange::from_node(capture.node),
                container: spec.container_name(capture.node, source),
            };
            let key = (symbol.range.start_byte, symbol.range.end_byte);
            match symbols.get(&key) {
                Some(existing) if existing.kind.priority() >= kind.priority() => {}
                _ => {
                    symbols.insert(key, symbol);
                }
            }
        }
    }
    symbols.into_values().collect()
}

fn extract_imports(
    file: &Path,
    source: &[u8],
    root: tree_sitter::Node<'_>,
    query: &Query,
) -> Vec<Import> {
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    let mut imports = BTreeMap::<(usize, usize, Option<String>), Import>::new();
    while let Some(query_match) = matches.next() {
        let names = query_match
            .captures
            .iter()
            .filter(|capture| capture_names[capture.index as usize] == "import.name")
            .filter_map(|capture| node_text(capture.node, source))
            .collect::<Vec<_>>();
        for capture in query_match.captures {
            if capture_names[capture.index as usize] != "import.path" {
                continue;
            }
            let Some(path) = node_text(capture.node, source) else {
                continue;
            };
            let range = SourceRange::from_node(capture.node);
            let names = if names.is_empty() {
                vec![None]
            } else {
                names.iter().cloned().map(Some).collect()
            };
            for name in names {
                let key = (range.start_byte, range.end_byte, name.clone());
                imports.insert(
                    key,
                    Import {
                        path: path.clone(),
                        name,
                        file: file.to_path_buf(),
                        range,
                    },
                );
            }
        }
    }
    imports.into_values().collect()
}

fn extract_references(
    file: &Path,
    source: &[u8],
    root: tree_sitter::Node<'_>,
    query: &Query,
    symbols: &[Symbol],
) -> Vec<Reference> {
    let definition_ranges = symbols
        .iter()
        .map(|symbol| (symbol.range.start_byte, symbol.range.end_byte))
        .collect::<BTreeSet<_>>();
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, source);
    let mut references = BTreeMap::<(usize, usize, String), Reference>::new();
    while let Some(query_match) = matches.next() {
        for capture in query_match.captures {
            if capture_names[capture.index as usize] != "reference" {
                continue;
            }
            let range = SourceRange::from_node(capture.node);
            if definition_ranges.contains(&(range.start_byte, range.end_byte)) {
                continue;
            }
            let Some(name) = node_text(capture.node, source) else {
                continue;
            };
            references.insert(
                (range.start_byte, range.end_byte, name.clone()),
                Reference {
                    name,
                    file: file.to_path_buf(),
                    range,
                },
            );
        }
    }
    references.into_values().collect()
}
