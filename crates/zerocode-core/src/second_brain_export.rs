//! The knowledge graph as one self-contained HTML page (t-5966, G4).
//!
//! Graphify writes an HTML visualisation; the window has a GL picture and an
//! artifact store. This module joins the two: the window hands over what its
//! lens is showing — nodes at the positions they settled at, in the inks the
//! stylesheet computed for them, every line with its kind and the road that
//! wrote it — and this renders one page that draws the same picture in any
//! browser with nothing loaded from outside: no font, no script, no style
//! beyond the file. The page is [`TEMPLATE`], the window's own file
//! (`ui/knowledge-export.html`), with the data set into one mark.
//!
//! Every bound is [`EXPORT_LIMITS`]; the size is measured after rendering
//! and refused rather than trimmed, so a page in the store is always whole.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::second_brain_graph::{EdgeProvenance, ProvenanceCount};

/// The viewer, verbatim. Two marks are filled: the title and the data.
pub const TEMPLATE: &str = include_str!("../../../ui/knowledge-export.html");
/// Where the page's title goes — inside `<title>`, escaped.
pub const TITLE_MARK: &str = "<!--__KNOWLEDGE_EXPORT_TITLE__-->";
/// Where the data goes — the one JSON literal the page's script reads.
pub const DATA_MARK: &str = "/*__KNOWLEDGE_EXPORT_DATA__*/null";

/// The bounds of one export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportLimits {
    /// Bytes the rendered page may reach. The artifact store takes sixteen
    /// megabytes; a picture is a fraction of that or it is not a picture.
    pub max_bytes: usize,
    /// Nodes one page carries — the graph's own page bound.
    pub max_nodes: usize,
    /// Lines one page carries — the graph's own edge bound.
    pub max_edges: usize,
    /// Titles the page writes beside points at the fitted zoom; past this
    /// the words cover the picture. Zooming in shows every title.
    pub label_budget: usize,
}

/// The table.
pub const EXPORT_LIMITS: ExportLimits = ExportLimits {
    max_bytes: 4 * 1024 * 1024,
    max_nodes: crate::second_brain_graph::MAX_GRAPH_PAGES
        + crate::second_brain_graph::MAX_GRAPH_GHOSTS,
    max_edges: crate::second_brain_graph::MAX_GRAPH_EDGES,
    label_budget: 48,
};

/// The four inks the page's chrome wears, as the window computed them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportTheme {
    pub ground: String,
    pub ink: String,
    pub rule: String,
    pub highlight: String,
}

/// One point as the window drew it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportNode {
    pub id: String,
    pub title: String,
    /// The node's kind word as the window spells it (page, ghost, source,
    /// component, vulnerability …) — a word for the tooltip, not a rule.
    pub kind: String,
    pub x: f32,
    pub y: f32,
    /// Radius in page pixels.
    pub r: f32,
    pub fill: String,
    pub stroke: String,
    pub stroke_px: f32,
    /// A ghost's ring is dashed.
    #[serde(default)]
    pub dashed: bool,
    /// Whether the window was showing this point's title.
    #[serde(default)]
    pub named: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// One line as the window drew it, with the road that wrote it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportEdge {
    pub from: u32,
    pub to: u32,
    pub kind: String,
    pub provenance: EdgeProvenance,
    pub ink: String,
    pub width: f32,
    /// Dash length in page pixels; zero is a solid line.
    pub dash: f32,
    /// Whether the kind carries an arrowhead.
    pub directed: bool,
}

/// One legend row: a kind present in the picture and its ink.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportLegendRow {
    pub kind: String,
    pub ink: String,
    pub dash: f32,
}

/// What the window hands over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportInput {
    pub title: String,
    /// The vault's path, for the page's head.
    pub vault: String,
    /// The lenses that were on, in the window's words, for the head.
    #[serde(default)]
    pub lenses: Vec<String>,
    pub theme: ExportTheme,
    pub nodes: Vec<ExportNode>,
    pub edges: Vec<ExportEdge>,
    #[serde(default)]
    pub legend: Vec<ExportLegendRow>,
    /// When, as the window's clock spells it — a word for the head.
    #[serde(default)]
    pub exported_at: String,
}

/// The page, and the numbers a receipt names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPage {
    pub html: String,
    pub nodes: usize,
    pub edges: usize,
    pub bytes: usize,
    pub provenances: Vec<ProvenanceCount>,
}

/// Why a picture was not rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportRefusal {
    NoNodes,
    TooManyNodes(usize),
    TooManyEdges(usize),
    EdgeOutOfRange { from: u32, to: u32 },
    TooLarge { bytes: usize, max_bytes: usize },
    Encode(String),
}

impl fmt::Display for ExportRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoNodes => write!(f, "nothing to export — the picture holds no node"),
            Self::TooManyNodes(count) => write!(f, "{count} nodes exceed the export bound"),
            Self::TooManyEdges(count) => write!(f, "{count} edges exceed the export bound"),
            Self::EdgeOutOfRange { from, to } => {
                write!(
                    f,
                    "an edge {from} → {to} names a node the picture does not hold"
                )
            }
            Self::TooLarge { bytes, max_bytes } => {
                write!(
                    f,
                    "the page would be {bytes} bytes, past the {max_bytes}-byte bound"
                )
            }
            Self::Encode(why) => write!(f, "the picture could not be encoded: {why}"),
        }
    }
}

impl std::error::Error for ExportRefusal {}

/// What the page's script reads: the input plus what this side counts.
#[derive(Serialize)]
struct PageData<'a> {
    title: &'a str,
    vault: &'a str,
    lenses: &'a [String],
    theme: &'a ExportTheme,
    nodes: &'a [ExportNode],
    edges: &'a [ExportEdge],
    legend: &'a [ExportLegendRow],
    exported_at: &'a str,
    provenances: &'a [ProvenanceCount],
    footer: String,
    label_budget: usize,
}

/// Lines per road, every road listed in enum order.
fn provenance_counts(edges: &[ExportEdge]) -> Vec<ProvenanceCount> {
    EdgeProvenance::ALL
        .into_iter()
        .map(|provenance| ProvenanceCount {
            provenance,
            count: u32::try_from(
                edges
                    .iter()
                    .filter(|edge| edge.provenance == provenance)
                    .count(),
            )
            .unwrap_or(u32::MAX),
        })
        .collect()
}

/// `&`, `<`, `>` and `"` as text: the title goes into markup.
fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for held in text.chars() {
        match held {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// JSON safe inside a `<script>`: `<` never appears bare, so `</script>` and
/// `<!--` inside a title cannot end the script early. Outside a string JSON
/// never holds `<`, so escaping every one is escaping only the strings'.
fn script_safe(json: &str) -> String {
    json.replace('<', "\\u003c")
}

/// The page, under [`EXPORT_LIMITS`].
///
/// # Errors
///
/// An empty picture, a bound crossed, an edge pointing outside the nodes,
/// or a page past the byte bound once rendered.
pub fn render(input: &ExportInput) -> Result<ExportPage, ExportRefusal> {
    render_within(input, EXPORT_LIMITS)
}

/// [`render`] under bounds of the caller's own — a test's way of watching
/// the byte bound refuse without a vault that big.
///
/// # Errors
///
/// As [`render`].
pub fn render_within(
    input: &ExportInput,
    limits: ExportLimits,
) -> Result<ExportPage, ExportRefusal> {
    if input.nodes.is_empty() {
        return Err(ExportRefusal::NoNodes);
    }
    if input.nodes.len() > limits.max_nodes {
        return Err(ExportRefusal::TooManyNodes(input.nodes.len()));
    }
    if input.edges.len() > limits.max_edges {
        return Err(ExportRefusal::TooManyEdges(input.edges.len()));
    }
    let count = u32::try_from(input.nodes.len()).unwrap_or(u32::MAX);
    if let Some(edge) = input
        .edges
        .iter()
        .find(|edge| edge.from >= count || edge.to >= count)
    {
        return Err(ExportRefusal::EdgeOutOfRange {
            from: edge.from,
            to: edge.to,
        });
    }
    let provenances = provenance_counts(&input.edges);
    let data = PageData {
        title: &input.title,
        vault: &input.vault,
        lenses: &input.lenses,
        theme: &input.theme,
        nodes: &input.nodes,
        edges: &input.edges,
        legend: &input.legend,
        exported_at: &input.exported_at,
        provenances: &provenances,
        footer: format!(
            "ZeroCode knowledge graph · {} nodes · {} edges{}",
            input.nodes.len(),
            input.edges.len(),
            if input.exported_at.is_empty() {
                String::new()
            } else {
                format!(" · {}", input.exported_at)
            }
        ),
        label_budget: limits.label_budget,
    };
    let json =
        serde_json::to_string(&data).map_err(|error| ExportRefusal::Encode(error.to_string()))?;
    let html = TEMPLATE
        .replacen(TITLE_MARK, &escape_html(&input.title), 1)
        .replacen(DATA_MARK, &script_safe(&json), 1);
    let bytes = html.len();
    if bytes > limits.max_bytes {
        return Err(ExportRefusal::TooLarge {
            bytes,
            max_bytes: limits.max_bytes,
        });
    }
    Ok(ExportPage {
        html,
        nodes: input.nodes.len(),
        edges: input.edges.len(),
        bytes,
        provenances,
    })
}

/// Whether a page loads nothing from outside itself: no `src=`, `href=`,
/// `url(` or `@import` that names a scheme or a protocol-relative path.
/// The promise the gallery makes for this kind of page — checked on the
/// rendered text rather than assumed of the template.
#[must_use]
pub fn is_self_contained(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    for opener in ["src=\"", "src='", "href=\"", "href='", "url(", "@import "] {
        let mut rest = lower.as_str();
        while let Some(at) = rest.find(opener) {
            let after = rest[at + opener.len()..].trim_start_matches(['"', '\'', ' ']);
            if after.starts_with("http:")
                || after.starts_with("https:")
                || after.starts_with("//")
                || after.starts_with("file:")
            {
                return false;
            }
            rest = &rest[at + opener.len()..];
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> ExportTheme {
        ExportTheme {
            ground: "rgb(20, 20, 24)".into(),
            ink: "rgb(230, 230, 235)".into(),
            rule: "rgba(255, 255, 255, 0.15)".into(),
            highlight: "rgb(120, 180, 255)".into(),
        }
    }

    fn node(id: &str, title: &str) -> ExportNode {
        ExportNode {
            id: id.into(),
            title: title.into(),
            kind: "page".into(),
            x: 1.0,
            y: 2.0,
            r: 4.0,
            fill: "rgba(1, 2, 3, 1)".into(),
            stroke: "rgba(4, 5, 6, 1)".into(),
            stroke_px: 1.0,
            dashed: false,
            named: true,
            tags: vec!["core".into()],
        }
    }

    fn edge(from: u32, to: u32, kind: &str, provenance: EdgeProvenance) -> ExportEdge {
        ExportEdge {
            from,
            to,
            kind: kind.into(),
            provenance,
            ink: "rgba(7, 8, 9, 0.5)".into(),
            width: 1.0,
            dash: 0.0,
            directed: kind == "implements",
        }
    }

    fn picture() -> ExportInput {
        ExportInput {
            title: "볼트 <그림> & \"인용\"".into(),
            vault: "/Users/dev/vault".into(),
            lenses: vec!["typed".into()],
            theme: theme(),
            nodes: vec![node("wiki/a.md", "A"), node("wiki/b.md", "B </script><!--")],
            edges: vec![
                edge(0, 1, "implements", EdgeProvenance::Declared),
                edge(1, 0, "mentions", EdgeProvenance::Inferred),
            ],
            legend: vec![ExportLegendRow {
                kind: "implements".into(),
                ink: "rgb(1, 1, 1)".into(),
                dash: 0.0,
            }],
            exported_at: "2026-09-22T10:00:00Z".into(),
        }
    }

    #[test]
    fn the_page_is_the_template_with_the_title_escaped_and_the_data_set_in_one_script() {
        let page = render(&picture()).unwrap();
        assert!(
            page.html
                .contains("<title>볼트 &lt;그림&gt; &amp; &quot;인용&quot;</title>")
        );
        assert!(!page.html.contains(TITLE_MARK));
        assert!(!page.html.contains(DATA_MARK));
        // The data is one JSON literal, and a `</script>` inside a title
        // cannot end the script: no bare `<` survives in the literal.
        let at = page.html.find("const DATA = ").unwrap();
        let literal = &page.html[at + "const DATA = ".len()..];
        let literal = &literal[..literal.find(";\n").unwrap()];
        assert!(!literal.contains('<'), "{literal}");
        let read: serde_json::Value = serde_json::from_str(literal).unwrap();
        assert_eq!(read["nodes"][1]["title"], "B </script><!--");
        assert_eq!(read["edges"][0]["provenance"], "declared");
        assert_eq!(
            read["provenances"][1],
            serde_json::json!({ "provenance": "declared", "count": 1 })
        );
        assert_eq!(read["label_budget"], EXPORT_LIMITS.label_budget);
        assert!(
            read["footer"]
                .as_str()
                .unwrap()
                .contains("2 nodes · 2 edges · 2026-09-22T10:00:00Z")
        );
        assert_eq!(page.html.matches("</script>").count(), 1);
        assert_eq!(page.bytes, page.html.len());
        assert_eq!((page.nodes, page.edges), (2, 2));
        assert!(is_self_contained(&page.html));
    }

    #[test]
    fn the_template_loads_nothing_from_outside_and_names_no_colour_of_its_own() {
        assert!(is_self_contained(TEMPLATE));
        assert!(TEMPLATE.contains(TITLE_MARK) && TEMPLATE.contains(DATA_MARK));
        // Inks come from the window's computed palette, never from the file:
        // no `#rgb`/`#rrggbb` literal in the stylesheet or the script.
        let mut rest = TEMPLATE;
        while let Some(at) = rest.find('#') {
            let hex = rest[at + 1..]
                .chars()
                .take_while(char::is_ascii_hexdigit)
                .count();
            let word = rest[at + 1..].chars().take(8).collect::<String>();
            assert!(
                !(3..=8).contains(&hex)
                    || rest[at + 1 + hex..]
                        .starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-'),
                "a colour literal in the export template: #{word}"
            );
            rest = &rest[at + 1..];
        }
    }

    #[test]
    fn the_self_contained_check_refuses_every_road_out() {
        for bad in [
            "<script src=\"https://cdn.example.com/x.js\"></script>",
            "<script src='//cdn.example.com/x.js'></script>",
            "<link rel=\"stylesheet\" href=\"http://example.com/x.css\">",
            "<style>body { background: url(https://example.com/x.png); }</style>",
            "<style>@import 'https://example.com/x.css';</style>",
            "<img src=\"file:///Users/dev/x.png\">",
        ] {
            assert!(!is_self_contained(bad), "{bad}");
        }
        assert!(is_self_contained(
            "<img src=\"data:image/png;base64,AAAA\"><a href=\"#top\">"
        ));
    }

    #[test]
    fn an_empty_or_oversized_picture_and_a_stray_edge_are_refused_and_say_why() {
        let mut empty = picture();
        empty.nodes.clear();
        empty.edges.clear();
        assert_eq!(render(&empty), Err(ExportRefusal::NoNodes));
        let mut stray = picture();
        stray
            .edges
            .push(edge(0, 9, "mentions", EdgeProvenance::Inferred));
        assert_eq!(
            render(&stray),
            Err(ExportRefusal::EdgeOutOfRange { from: 0, to: 9 })
        );
        let tight = ExportLimits {
            max_bytes: 100,
            ..EXPORT_LIMITS
        };
        let refused = render_within(&picture(), tight).unwrap_err();
        assert!(
            matches!(refused, ExportRefusal::TooLarge { max_bytes: 100, .. }),
            "{refused}"
        );
        assert!(refused.to_string().contains("past the 100-byte bound"));
        let one_node = ExportLimits {
            max_nodes: 1,
            ..EXPORT_LIMITS
        };
        assert_eq!(
            render_within(&picture(), one_node),
            Err(ExportRefusal::TooManyNodes(2))
        );
        let one_edge = ExportLimits {
            max_edges: 1,
            ..EXPORT_LIMITS
        };
        assert_eq!(
            render_within(&picture(), one_edge),
            Err(ExportRefusal::TooManyEdges(2))
        );
    }

    /// The report's size number: `cargo test --release -p zerocode-core --lib
    /// second_brain_export::tests::a_real_vault -- --ignored --nocapture`
    /// with `ZEROCODE_SECOND_BRAIN` naming a vault. Read-only. Positions
    /// and inks are synthetic and of the window's shape, so the byte count
    /// is the page's shape at that many nodes and lines, not a pixel-true
    /// picture.
    #[test]
    #[ignore = "reads the vault ZEROCODE_SECOND_BRAIN names; a measurement, not a gate"]
    fn a_real_vault_renders_within_the_bound_and_says_its_size() {
        let Ok(root) = std::env::var(crate::second_brain::VAULT_ENV) else {
            return;
        };
        let graph =
            crate::second_brain_graph::GraphCache::new().scan(std::path::Path::new(&root), false);
        let nodes: Vec<ExportNode> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(at, held)| ExportNode {
                id: held.id.clone(),
                title: held.title.clone(),
                kind: format!("{:?}", held.kind).to_lowercase(),
                x: (at as f32) * 1.2345,
                y: (at as f32) * -0.9876,
                r: 4.5,
                fill: "rgba(120, 130, 140, 1)".into(),
                stroke: "rgba(20, 30, 40, 0.8)".into(),
                stroke_px: 1.0,
                dashed: held.kind == crate::second_brain_graph::NodeKind::Ghost,
                named: at % 20 == 0,
                tags: held.tags.clone(),
            })
            .collect();
        let edges: Vec<ExportEdge> = graph
            .edges
            .iter()
            .map(|edge| ExportEdge {
                from: edge.from,
                to: edge.to,
                kind: edge.kind.as_str().into(),
                provenance: edge.provenance,
                ink: "rgba(100, 110, 120, 0.34)".into(),
                width: 1.0,
                dash: 0.0,
                directed: edge.kind != crate::second_brain_graph::EdgeKind::Mentions,
            })
            .collect();
        let input = ExportInput {
            title: "vault knowledge graph".into(),
            vault: root.clone(),
            lenses: Vec::new(),
            theme: theme(),
            nodes,
            edges,
            legend: Vec::new(),
            exported_at: "2026-09-22T00:00:00Z".into(),
        };
        let page = render(&input).expect("within the bound");
        assert!(is_self_contained(&page.html));
        println!(
            "vault {root}: nodes {} edges {} → {} bytes ({} KB), bound {} KB",
            page.nodes,
            page.edges,
            page.bytes,
            page.bytes / 1024,
            EXPORT_LIMITS.max_bytes / 1024
        );
    }

    #[test]
    fn the_input_reads_the_wire_the_window_writes_with_its_defaults() {
        let wire = serde_json::json!({
            "title": "t", "vault": "/v", "theme": theme(),
            "nodes": [{ "id": "wiki/a.md", "title": "A", "kind": "page", "x": 0, "y": 0, "r": 3,
                        "fill": "rgb(0, 0, 0)", "stroke": "rgb(0, 0, 0)", "stroke_px": 1 }],
            "edges": [],
        });
        let input: ExportInput = serde_json::from_value(wire).unwrap();
        assert!(input.lenses.is_empty() && input.legend.is_empty() && input.exported_at.is_empty());
        assert!(!input.nodes[0].dashed && !input.nodes[0].named && input.nodes[0].tags.is_empty());
        let page = render(&input).unwrap();
        assert_eq!(page.provenances.iter().map(|row| row.count).sum::<u32>(), 0);
    }
}
