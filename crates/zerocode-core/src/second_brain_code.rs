//! The code layer of the knowledge graph (t-5970, Graphify G2).
//!
//! A page that names code — a path in its `source:`, a file or a symbol in a
//! backtick span — is joined to that code as the active project's codegraph
//! index knows it: a file node, a symbol node, a `measured` `implements` line
//! from the code to the page, and the index's `depends_on` lines between the
//! files the layer holds. The index is zo's: `zo vault code` resolves the
//! mentions and answers the [`CodeLayer`]. This module is what the two sides
//! agree on — which spans are worth asking about ([`code_mentions`]), the
//! layer's shape, and the one [`graft`] — so the window draws what zo
//! measured and derives nothing.
//!
//! Every bound is in [`CodeLimits`].

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::second_brain_graph::{
    EdgeKind, EdgeProvenance, GraphEdge, GraphNode, NodeKind, VaultGraph, fence_mark, kind_counts,
    provenance_counts,
};

/// Every code node's id starts with this, so a vault id (`wiki/…`,
/// `ghost:…`, `raw/…`) and a code id can never be the same string.
pub const CODE_ID_PREFIX: &str = "code:";
/// Between a symbol's file and its name in the symbol's id.
const SYMBOL_ID_SEPARATOR: char = '#';
/// Between a symbol's name and the row it is defined on.
const SYMBOL_ROW_SEPARATOR: char = '@';
/// What splits a `source:` value into its items: prose separators, and the
/// middle dot pages use between sibling files (`store.rs·scan.rs`).
const SOURCE_SEPARATORS: [char; 8] = [' ', '\t', ',', ';', '(', ')', '·', '、'];
/// Between a Rust path's segments in a symbol mention (`Type::method`).
const PATH_SEPARATOR: &str = "::";
/// A call written as `name()` names `name`.
const CALL_SUFFIX: &str = "()";
/// Extensions are short: `rs`, `tsx`, `json`. A longer "extension" is a
/// sentence with a full stop in it.
const MAX_EXTENSION_CHARS: usize = 5;
/// Shorter than this, a span is a word — `id`, `rs` — not a name to look up.
const MIN_MENTION_CHARS: usize = 3;
/// A span opening with one of these is a location of its own, not a path
/// into the project.
const FOREIGN_PREFIXES: [&str; 4] = ["http://", "https://", "/", "~"];

/// Every number the code layer has, in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeLimits {
    /// Code mentions one page contributes: a page is about a handful of
    /// files, and one listing a thousand is a generated index.
    pub mentions_per_page: usize,
    /// Characters one mention may span: longer is prose in backticks.
    pub mention_chars_max: usize,
    /// Code nodes one layer may add to a picture.
    pub layer_nodes_max: usize,
    /// Lines one layer may add.
    pub layer_edges_max: usize,
}

impl Default for CodeLimits {
    fn default() -> Self {
        Self {
            mentions_per_page: 32,
            mention_chars_max: 160,
            layer_nodes_max: 600,
            layer_edges_max: 3_000,
        }
    }
}

/// A page's spans worth asking the index about, in the order written and
/// each once: the path-shaped items of its `source:` value, then every
/// backtick span outside a fence that is shaped like a path or a symbol.
///
/// This is a cheap sieve, not a resolution — the index decides what a span
/// names and drops what it cannot place, so a sieve that lets a word through
/// costs one lookup, and one that drops a real name loses the link.
#[must_use]
pub fn code_mentions(body: &str, source: Option<&str>, limits: &CodeLimits) -> Vec<String> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut take = |candidate: &str, symbols: bool| {
        if found.len() >= limits.mentions_per_page {
            return;
        }
        let Some(mention) = shaped(candidate, symbols, limits) else {
            return;
        };
        if seen.insert(mention.clone()) {
            found.push(mention);
        }
    };
    for item in source.unwrap_or_default().split(SOURCE_SEPARATORS) {
        take(item, false);
    }
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        if let Some(mark) = fence_mark(line.trim()) {
            match fence {
                Some((open, width)) if open == mark.0 && mark.1 >= width => fence = None,
                Some(_) => {}
                None => fence = Some(mark),
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let mut spans = line.split('`');
        // Odd pieces are inside a pair of backticks; an unpaired last one is
        // not a span.
        spans.next();
        while let (Some(inside), Some(_)) = (spans.next(), spans.next()) {
            take(inside, true);
        }
    }
    found
}

/// The mention `candidate` makes, normalised, or nothing when it is shaped
/// like neither a path nor (when `symbols`) a symbol.
fn shaped(candidate: &str, symbols: bool, limits: &CodeLimits) -> Option<String> {
    let candidate = candidate.trim();
    let chars = candidate.chars().count();
    if !(MIN_MENTION_CHARS..=limits.mention_chars_max).contains(&chars)
        || candidate.chars().any(char::is_whitespace)
        || FOREIGN_PREFIXES
            .iter()
            .any(|prefix| candidate.starts_with(prefix))
    {
        return None;
    }
    if let Some(path) = path_shaped(candidate) {
        return Some(path);
    }
    symbols.then(|| symbol_shaped(candidate)).flatten()
}

/// A relative path: `a/b.rs`, `b.rs`, `a/b.rs:40` (the row is dropped), with
/// a leading `./` dropped.
fn path_shaped(candidate: &str) -> Option<String> {
    let path = candidate.strip_prefix("./").unwrap_or(candidate);
    let path = path
        .split_once(':')
        .filter(|(_, row)| {
            row.split(':')
                .all(|part| part.chars().all(|c| c.is_ascii_digit()))
        })
        .map_or(path, |(file, _)| file);
    let name = path.rsplit('/').next()?;
    let has_extension = name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && (1..=MAX_EXTENSION_CHARS).contains(&extension.len())
            && extension.chars().all(|c| c.is_ascii_alphanumeric())
    });
    let plausible = path
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'));
    (plausible && (has_extension || path.contains('/'))).then(|| path.to_string())
}

/// A symbol: identifier segments joined by `::`, optionally called (`()` is
/// dropped), and marked as a name rather than a word — an underscore, a
/// path, or a capital after the first letter.
fn symbol_shaped(candidate: &str) -> Option<String> {
    let symbol = candidate.strip_suffix(CALL_SUFFIX).unwrap_or(candidate);
    let segments = symbol.split(PATH_SEPARATOR).collect::<Vec<_>>();
    let identifier = |segment: &&str| {
        let mut chars = segment.chars();
        chars
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    if !segments.iter().all(identifier) {
        return None;
    }
    let named = segments.len() > 1
        || symbol.contains('_')
        || symbol.chars().skip(1).any(|c| c.is_ascii_uppercase());
    named.then(|| symbol.to_string())
}

/// The code a picture can be joined with, as zo resolved it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeLayer {
    /// The project whose index answered, as zo spelled its root.
    pub project: String,
    pub nodes: Vec<CodeNode>,
    pub edges: Vec<CodeEdge>,
    /// Distinct mentions asked, and how many the index could place.
    pub mentions: usize,
    pub resolved: usize,
    /// A bound of [`CodeLimits`] cut the layer.
    pub capped: bool,
}

/// One file or definition of the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeNode {
    /// [`file_id`] or [`symbol_id`].
    pub id: String,
    /// [`NodeKind::CodeFile`] or [`NodeKind::CodeSymbol`].
    pub kind: NodeKind,
    pub title: String,
    /// The file, relative to the project root, `/`-separated.
    pub file: String,
    /// One-based row of a definition.
    pub line: Option<u32>,
    /// What the card says under the title: the definition's kind and
    /// container, or the file's language.
    pub detail: String,
}

/// One line of the layer, by id: code → page (`implements`) or file → file
/// (`depends_on`). Its road is always [`EdgeProvenance::Measured`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

#[must_use]
pub fn file_id(file: &str) -> String {
    format!("{CODE_ID_PREFIX}{file}")
}

#[must_use]
pub fn symbol_id(file: &str, name: &str, line: u32) -> String {
    format!("{CODE_ID_PREFIX}{file}{SYMBOL_ID_SEPARATOR}{name}{SYMBOL_ROW_SEPARATOR}{line}")
}

/// What one [`graft`] added.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraftSummary {
    pub nodes: usize,
    pub edges: usize,
    /// A bound of [`CodeLimits`] stopped the graft before the layer ended.
    pub capped: bool,
}

/// Join `layer` to `graph`: its nodes after the picture's own, its lines
/// between nodes the picture now holds, each line once per pair and kind,
/// all of them measured. Counts, kinds and roads are recounted; the lint
/// stays the vault's own — code is not a page it could fault.
pub fn graft(graph: &mut VaultGraph, layer: &CodeLayer, limits: &CodeLimits) -> GraftSummary {
    let mut summary = GraftSummary {
        capped: layer.capped,
        ..GraftSummary::default()
    };
    let mut index: HashMap<String, u32> = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(at, node)| Some((node.id.clone(), u32::try_from(at).ok()?)))
        .collect();
    for node in &layer.nodes {
        if index.contains_key(&node.id) {
            continue;
        }
        if summary.nodes >= limits.layer_nodes_max {
            summary.capped = true;
            break;
        }
        let Ok(at) = u32::try_from(graph.nodes.len()) else {
            summary.capped = true;
            break;
        };
        index.insert(node.id.clone(), at);
        graph.nodes.push(GraphNode {
            id: node.id.clone(),
            title: node.title.clone(),
            tags: Vec::new(),
            kind: node.kind,
            modified_ms: 0,
            out_links: 0,
            in_links: 0,
            source: Some(node.file.clone()),
            excerpt: node.detail.clone(),
            folder: String::new(),
        });
        summary.nodes += 1;
    }
    let mut lines: HashSet<(u32, u32, EdgeKind)> = graph
        .edges
        .iter()
        .map(|edge| (edge.from, edge.to, edge.kind))
        .collect();
    for edge in &layer.edges {
        let (Some(&from), Some(&to)) = (index.get(&edge.from), index.get(&edge.to)) else {
            continue;
        };
        if from == to || !lines.insert((from, to, edge.kind)) {
            continue;
        }
        if summary.edges >= limits.layer_edges_max {
            summary.capped = true;
            break;
        }
        graph.edges.push(GraphEdge {
            from,
            to,
            kind: edge.kind,
            provenance: EdgeProvenance::Measured,
        });
        for (at, outgoing) in [(from, true), (to, false)] {
            if let Some(node) = graph.nodes.get_mut(at as usize) {
                if outgoing {
                    node.out_links += 1;
                } else {
                    node.in_links += 1;
                }
            }
        }
        summary.edges += 1;
    }
    graph.edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then_with(|| left.to.cmp(&right.to))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    graph.kinds = kind_counts(&graph.edges);
    graph.provenances = provenance_counts(&graph.edges);
    graph.capped |= summary.capped;
    summary
}

/// Whether a measured line is one [`graft`] writes: code implementing a page,
/// or one code file depending on another.
#[must_use]
pub fn grafted_line(kind: EdgeKind, target: NodeKind) -> bool {
    matches!(
        (kind, target),
        (EdgeKind::Implements, NodeKind::Page) | (EdgeKind::DependsOn, NodeKind::CodeFile)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        CodeEdge, CodeLayer, CodeLimits, CodeNode, code_mentions, file_id, graft, grafted_line,
        symbol_id,
    };
    use crate::second_brain_graph::{
        EdgeKind, EdgeProvenance, GraphEdge, GraphNode, NodeKind, VaultGraph, vouched,
    };

    #[test]
    fn a_page_names_code_in_its_source_and_its_backtick_spans() {
        let body = "Reads `crates/core/src/graph.rs:120` and `./ui/shell.js`, calls \
                    `GraphCache::scan()` and `load_or_build`, not `store` or `a b` or \
                    `https://example.com/x.rs`.\n\n```rust\nlet hidden = `fenced::name`;\n```\n\
                    After the fence: `CodeGraph`.\n";
        let mentions = code_mentions(
            body,
            Some("zo-ide/crates/codegraph (store.rs·scan.rs), 사용자 대화"),
            &CodeLimits::default(),
        );
        assert_eq!(
            mentions,
            [
                "zo-ide/crates/codegraph",
                "store.rs",
                "scan.rs",
                "crates/core/src/graph.rs",
                "ui/shell.js",
                "GraphCache::scan",
                "load_or_build",
                "CodeGraph",
            ]
        );
    }

    #[test]
    fn a_page_contributes_a_bounded_number_of_mentions_once_each() {
        let body = (0..100)
            .map(|n| format!("`file_{n}.rs` `file_{n}.rs`\n"))
            .collect::<String>();
        let limits = CodeLimits::default();
        let mentions = code_mentions(&body, None, &limits);
        assert_eq!(mentions.len(), limits.mentions_per_page);
        assert_eq!(mentions[0], "file_0.rs");
        assert_eq!(mentions[1], "file_1.rs");
    }

    fn page(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            title: id.to_string(),
            tags: Vec::new(),
            kind: NodeKind::Page,
            modified_ms: 0,
            out_links: 0,
            in_links: 0,
            source: None,
            excerpt: String::new(),
            folder: String::new(),
        }
    }

    fn code(id: String, kind: NodeKind) -> CodeNode {
        CodeNode {
            title: id.clone(),
            id,
            kind,
            file: "src/graph.rs".to_string(),
            line: None,
            detail: String::new(),
        }
    }

    #[test]
    fn a_layer_joins_the_picture_with_measured_lines_once_each() {
        let mut graph = VaultGraph {
            nodes: vec![page("wiki/Graph.md"), page("wiki/Scan.md")],
            edges: vec![GraphEdge {
                from: 0,
                to: 1,
                kind: EdgeKind::Mentions,
                provenance: EdgeProvenance::Inferred,
            }],
            ..VaultGraph::default()
        };
        let graph_file = file_id("src/graph.rs");
        let scan_file = file_id("src/scan.rs");
        let scan_fn = symbol_id("src/scan.rs", "scan_workspace", 37);
        let layer = CodeLayer {
            project: "acme".to_string(),
            nodes: vec![
                code(graph_file.clone(), NodeKind::CodeFile),
                code(scan_file.clone(), NodeKind::CodeFile),
                code(scan_fn.clone(), NodeKind::CodeSymbol),
            ],
            edges: vec![
                CodeEdge {
                    from: graph_file.clone(),
                    to: "wiki/Graph.md".to_string(),
                    kind: EdgeKind::Implements,
                },
                CodeEdge {
                    from: scan_fn.clone(),
                    to: "wiki/Scan.md".to_string(),
                    kind: EdgeKind::Implements,
                },
                CodeEdge {
                    from: scan_fn.clone(),
                    to: "wiki/Scan.md".to_string(),
                    kind: EdgeKind::Implements,
                },
                CodeEdge {
                    from: graph_file.clone(),
                    to: scan_file.clone(),
                    kind: EdgeKind::DependsOn,
                },
                CodeEdge {
                    from: graph_file,
                    to: "wiki/Gone.md".to_string(),
                    kind: EdgeKind::Implements,
                },
            ],
            mentions: 3,
            resolved: 3,
            capped: false,
        };
        let summary = graft(&mut graph, &layer, &CodeLimits::default());
        assert_eq!(
            (summary.nodes, summary.edges, summary.capped),
            (3, 3, false)
        );
        assert_eq!(graph.nodes.len(), 5);
        assert!(graph.edges.iter().skip(1).all(|edge| {
            edge.provenance == EdgeProvenance::Measured
                && vouched(edge, graph.nodes[edge.to as usize].kind)
        }));
        assert_eq!(
            graph.nodes[1].in_links, 1,
            "the scan page is implemented once"
        );
        let measured = graph
            .provenances
            .iter()
            .find(|count| count.provenance == EdgeProvenance::Measured)
            .map(|count| count.count);
        assert_eq!(measured, Some(3));
        // Grafting the same layer again adds nothing.
        let again = graft(&mut graph, &layer, &CodeLimits::default());
        assert_eq!((again.nodes, again.edges), (0, 0));
    }

    #[test]
    fn only_the_grafts_own_lines_are_measured_roads() {
        assert!(grafted_line(EdgeKind::Implements, NodeKind::Page));
        assert!(grafted_line(EdgeKind::DependsOn, NodeKind::CodeFile));
        assert!(!grafted_line(EdgeKind::Mentions, NodeKind::Page));
        assert!(!grafted_line(EdgeKind::Implements, NodeKind::CodeSymbol));
    }
}
