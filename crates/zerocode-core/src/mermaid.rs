//! Drawing a Mermaid diagram, in Rust.
//!
//! Orca renders these by bundling the `mermaid` library — 1,500 lines of core
//! plus twenty-odd chunks, loaded lazily and parsed on the first diagram, with
//! DOMPurify over the SVG it produces because that SVG comes out of a
//! general-purpose engine that can be asked to emit almost anything
//! (`MermaidBlock-O_45bwHQ.js`, `MermaidViewer-dUZcjW_i.js`, 1.4.164).
//!
//! This does not, for two reasons that point the same way:
//!
//! 1. **Shipping it is not available to us.** Third-party source does not go
//!    into this product, and a diagram engine is not a token we can measure and
//!    re-express — it is somebody's library.
//! 2. **It should not be JavaScript anyway.** A megabytes-large parse on the
//!    first diagram, per window, to draw a dozen boxes, is exactly the cost this
//!    project exists to not pay.
//!
//! ## No markup crosses the boundary
//!
//! This module emits no SVG. It answers with a [`Drawing`] — a flat list of
//! placed rectangles, polygons, polylines and texts — and the window turns each
//! mark into an element, putting every label in through `textContent`.
//!
//! That is why there is no sanitising pass on this path. Orca needs DOMPurify
//! because the library hands it an SVG STRING, and a string from a
//! general-purpose engine can contain anything. A label here never becomes
//! markup at any point, so there is nothing to launder: the injection Orca
//! defends against with a filter is not reachable, rather than filtered.
//!
//! ## What is drawn, and what is not
//!
//! Flowcharts (`graph` / `flowchart`) and sequence diagrams
//! (`sequenceDiagram`) — the two kinds that actually appear in a code
//! repository. Anything else parses to [`Diagram::Unsupported`], and the window
//! shows the SOURCE with a line saying the kind is not drawn yet. That is a
//! deliberate shape: a diagram nobody can read is better than an error where a
//! diagram should be, and better than a wrong picture.
//!
//! The remaining kinds — class, state, ER, gantt, pie, journey — are ledgered
//! (docs/reverse/orca-ui-inventory.md 1-bc), each a layout of its own.

use serde::{Deserialize, Serialize};

/// Layout numbers. Not measured off Orca — its diagrams are laid out by the
/// library it bundles — so these answer to legibility instead, and they are
/// gathered here rather than sprinkled through the emit code so the whole
/// diagram can be re-proportioned in one place.
mod metrics {
    /// Character advance for the label font at 13px, used to size boxes. An
    /// estimate on purpose: measuring text needs a font engine, and a box that
    /// is a few pixels wide of ideal is invisible next to a box that clips.
    pub const CHAR_WIDTH: f64 = 7.4;
    pub const NODE_PAD_X: f64 = 14.0;
    pub const NODE_HEIGHT: f64 = 38.0;
    pub const NODE_MIN_WIDTH: f64 = 56.0;
    /// Between two nodes in the same rank, and between two ranks.
    pub const GAP_WITHIN_RANK: f64 = 26.0;
    pub const GAP_BETWEEN_RANKS: f64 = 52.0;
    pub const MARGIN: f64 = 20.0;
    /// Sequence diagrams.
    pub const ACTOR_GAP: f64 = 40.0;
    pub const MESSAGE_GAP: f64 = 44.0;
    pub const ACTOR_TOP: f64 = 46.0;
}

/// The most nodes this window will lay out.
///
/// Not a taste limit — a bound on two loops that are quadratic in the node
/// count: interning scans the nodes it has, and ranking walks the edges once per
/// node. The file viewer caps a file at a megabyte, and a megabyte of `A1-->A2`
/// is a hundred thousand nodes, which would be 10^10 comparisons in a window
/// that is meant to be answering a click. Past this the diagram is reported
/// [`Diagram::TooLarge`] and the source is shown instead, which is the same
/// answer the viewer already gives for a file it will not render.
///
/// The number is well above any diagram a person reads: 400 boxes is already a
/// wall, and the largest in Orca's own docs is under thirty.
pub const MAX_NODES: usize = 400;

/// Which way a flowchart runs. Mermaid's four, with its own spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flow {
    Down,
    Up,
    Right,
    Left,
}

impl Flow {
    fn from_word(word: &str) -> Option<Self> {
        match word.trim().to_ascii_uppercase().as_str() {
            "TD" | "TB" => Some(Flow::Down),
            "BT" => Some(Flow::Up),
            "LR" => Some(Flow::Right),
            "RL" => Some(Flow::Left),
            _ => None,
        }
    }

    /// Do ranks advance along x rather than y?
    const fn horizontal(self) -> bool {
        matches!(self, Flow::Right | Flow::Left)
    }
}

/// A node's outline, from the brackets its label came in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    /// `[text]`
    Rect,
    /// `(text)`
    Round,
    /// `((text))`
    Circle,
    /// `{text}`
    Diamond,
    /// `[[text]]`
    Subroutine,
    /// `[(text)]`
    Cylinder,
    /// A bare id with no brackets.
    Plain,
}

/// How an edge is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Line {
    Solid,
    Dotted,
    Thick,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: Shape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    pub line: Line,
    pub arrow: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub from: usize,
    pub to: usize,
    pub text: String,
    pub line: Line,
    pub arrow: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Diagram {
    Flowchart {
        flow: Flow,
        nodes: Vec<Node>,
        edges: Vec<Edge>,
    },
    Sequence {
        actors: Vec<Node>,
        messages: Vec<Message>,
    },
    /// A kind this window does not draw. The word is carried so the window can
    /// name it rather than saying "unsupported".
    Unsupported { kind: String },
    /// More nodes than [`MAX_NODES`]. The count is the one reached before
    /// reading stopped, not the file's true total — past the limit the answer is
    /// the same either way, and counting the rest is the cost being refused.
    TooLarge { nodes: usize },
    /// Nothing usable in the text at all.
    Empty,
}

/// Strip a `%%` comment and surrounding space. Mermaid's comment marker, and
/// `%%{init: …}%%` directives take the same road out — this renderer has no
/// theme to configure, and a directive left in would parse as a node.
fn strip_comment(line: &str) -> &str {
    match line.find("%%") {
        Some(at) => line[..at].trim(),
        None => line.trim(),
    }
}

/// Split a label out of its brackets and say which shape they were.
///
/// Longest bracket pairs first: `[[x]]` must not be read as `[` + `[x]`, and
/// `((x))` must not be read as a round node whose label starts with a paren.
fn split_shape(text: &str) -> (String, Shape) {
    const PAIRS: &[(&str, &str, Shape)] = &[
        ("[[", "]]", Shape::Subroutine),
        ("((", "))", Shape::Circle),
        ("[(", ")]", Shape::Cylinder),
        ("[", "]", Shape::Rect),
        ("(", ")", Shape::Round),
        ("{", "}", Shape::Diamond),
    ];
    let text = text.trim();
    for (open, close, shape) in PAIRS {
        if let Some(inner) = text
            .strip_prefix(open)
            .and_then(|rest| rest.strip_suffix(close))
        {
            return (unquote(inner), *shape);
        }
    }
    (unquote(text), Shape::Plain)
}

/// Mermaid lets a label be quoted so it may contain brackets.
fn unquote(text: &str) -> String {
    let trimmed = text.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

/// The id a term declares, and its label. `A[Start]` is id `A` label `Start`;
/// a bare `A` is both.
fn parse_term(text: &str) -> Option<Node> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // The id runs until the first bracket. Everything after is the label.
    let split = text.find(['[', '(', '{']).unwrap_or(text.len());
    let id = text[..split].trim();
    if id.is_empty() {
        return None;
    }
    let (label, shape) = if split == text.len() {
        (id.to_string(), Shape::Plain)
    } else {
        split_shape(&text[split..])
    };
    Some(Node {
        id: id.to_string(),
        label: if label.is_empty() {
            id.to_string()
        } else {
            label
        },
        shape,
    })
}

/// The arrow shapes this renderer reads, longest first so `-.->` is not seen as
/// `-` then `.`, and `-->` is not seen as `--`.
const ARROWS: &[(&str, Line, bool)] = &[
    ("-.->", Line::Dotted, true),
    ("==>", Line::Thick, true),
    ("-->", Line::Solid, true),
    ("-.-", Line::Dotted, false),
    ("===", Line::Thick, false),
    ("---", Line::Solid, false),
    ("->", Line::Solid, true),
    ("--", Line::Solid, false),
];

/// Find the first arrow in a line: where it starts, how long, and what it means.
fn find_arrow(line: &str) -> Option<(usize, usize, Line, bool)> {
    let mut best: Option<(usize, usize, Line, bool)> = None;
    for (mark, kind, arrow) in ARROWS {
        if let Some(at) = line.find(mark) {
            // Earliest wins; at the same position the longest wins, which is
            // what keeps `-->` from being read as `--`.
            let better = match best {
                None => true,
                Some((held_at, held_len, _, _)) => {
                    at < held_at || (at == held_at && mark.len() > held_len)
                }
            };
            if better {
                best = Some((at, mark.len(), *kind, *arrow));
            }
        }
    }
    best
}

/// Add a node if it is new, and answer with its index either way.
///
/// A later declaration with a REAL label replaces a bare-id placeholder: mermaid
/// lets `A --> B` come before `B[Done]`, and the label is the thing a reader
/// needs.
///
/// `None` means the diagram is already at [`MAX_NODES`] and this id is a new
/// one. An id already held still resolves, so a big diagram's LATER edges
/// between nodes it already has are not what stops the read.
fn intern(nodes: &mut Vec<Node>, node: Node) -> Option<usize> {
    if let Some(at) = nodes.iter().position(|held| held.id == node.id) {
        if nodes[at].shape == Shape::Plain && node.shape != Shape::Plain {
            nodes[at] = node;
        }
        return Some(at);
    }
    if nodes.len() >= MAX_NODES {
        return None;
    }
    nodes.push(node);
    Some(nodes.len() - 1)
}

/// Read a diagram out of mermaid text.
///
/// Never fails. An unparseable line is skipped rather than refusing the whole
/// diagram — half a picture from a file somebody is still typing beats an error
/// where a picture should be, and the source is one keystroke away in the same
/// surface.
pub fn parse(source: &str) -> Diagram {
    let mut lines = source
        .lines()
        .map(strip_comment)
        .filter(|line| !line.is_empty());
    let Some(head) = lines.next() else {
        return Diagram::Empty;
    };
    let mut word = head.split_whitespace();
    let kind = word.next().unwrap_or_default();
    let rest: Vec<&str> = word.collect();

    match kind.to_ascii_lowercase().as_str() {
        "graph" | "flowchart" => {
            // The direction may be on the header (`graph TD`) or absent, in
            // which case mermaid's own default is top-down.
            let flow = rest
                .first()
                .and_then(|word| Flow::from_word(word))
                .unwrap_or(Flow::Down);
            parse_flowchart(flow, lines)
        }
        "sequencediagram" => parse_sequence(lines),
        other => Diagram::Unsupported {
            kind: other.to_string(),
        },
    }
}

fn parse_flowchart<'a>(flow: Flow, lines: impl Iterator<Item = &'a str>) -> Diagram {
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    for line in lines {
        // `subgraph`/`end` are containers this renderer does not draw. Skipped
        // rather than refused: their CONTENTS are ordinary nodes and edges, so
        // the diagram still says what connects to what.
        let lowered = line.to_ascii_lowercase();
        if lowered.starts_with("subgraph") || lowered == "end" {
            continue;
        }
        // Styling statements name nodes but declare no structure.
        if ["style", "classdef", "class", "linkstyle", "click"]
            .iter()
            .any(|word| lowered.starts_with(word))
        {
            continue;
        }
        let Some((at, len, line_kind, arrow)) = find_arrow(line) else {
            // No arrow: a lone declaration, which is how an isolated node gets
            // into the picture.
            if let Some(node) = parse_term(line)
                && intern(&mut nodes, node).is_none()
            {
                return Diagram::TooLarge { nodes: nodes.len() };
            }
            continue;
        };
        let left = &line[..at];
        let mut right = &line[at + len..];
        // `A -->|yes| B` and `A -- yes --> B` both label the edge. The first is
        // the common spelling; the second is read by taking the text before a
        // SECOND arrow on the same line.
        let mut label = None;
        if let Some(rest) = right.trim_start().strip_prefix('|')
            && let Some((inside, after)) = rest.split_once('|')
        {
            label = Some(unquote(inside));
            right = after;
        } else if let Some((inner_at, inner_len, _, _)) = find_arrow(right) {
            let text = right[..inner_at].trim();
            if !text.is_empty() {
                label = Some(unquote(text));
            }
            right = &right[inner_at + inner_len..];
        }
        let (Some(from), Some(to)) = (parse_term(left), parse_term(right)) else {
            continue;
        };
        let (Some(from), Some(to)) = (intern(&mut nodes, from), intern(&mut nodes, to)) else {
            return Diagram::TooLarge { nodes: nodes.len() };
        };
        edges.push(Edge {
            from,
            to,
            label: label.filter(|one| !one.is_empty()),
            line: line_kind,
            arrow,
        });
    }
    if nodes.is_empty() {
        return Diagram::Empty;
    }
    Diagram::Flowchart { flow, nodes, edges }
}

fn parse_sequence<'a>(lines: impl Iterator<Item = &'a str>) -> Diagram {
    let mut actors: Vec<Node> = Vec::new();
    let mut messages: Vec<Message> = Vec::new();
    for line in lines {
        let lowered = line.to_ascii_lowercase();
        // A declared participant, which is how an actor's ORDER is chosen —
        // mermaid places them in declaration order, not first-mention order.
        if let Some(rest) = lowered
            .strip_prefix("participant ")
            .or_else(|| lowered.strip_prefix("actor "))
        {
            let taken = line[line.len() - rest.len()..].trim();
            // `participant A as Alice` names the label separately.
            let node = match taken.split_once(" as ") {
                Some((id, label)) => Node {
                    id: id.trim().to_string(),
                    label: unquote(label),
                    shape: Shape::Rect,
                },
                None => Node {
                    id: taken.to_string(),
                    label: taken.to_string(),
                    shape: Shape::Rect,
                },
            };
            if intern(&mut actors, node).is_none() {
                return Diagram::TooLarge {
                    nodes: actors.len(),
                };
            }
            continue;
        }
        // Blocks this renderer does not draw. Their messages still do.
        if [
            "loop",
            "alt",
            "else",
            "opt",
            "par",
            "end",
            "note",
            "activate",
            "deactivate",
            "autonumber",
        ]
        .iter()
        .any(|word| lowered.starts_with(word))
        {
            continue;
        }
        // `A->>B: text`. The message text is after the first colon; the arrow
        // shapes are mermaid's sequence set, which overlap the flowchart ones.
        let Some((wire, text)) = line.split_once(':') else {
            continue;
        };
        let (line_kind, arrow, from_to) = sequence_arrow(wire);
        let Some((left, right)) = from_to else {
            continue;
        };
        if left.trim().is_empty() || right.trim().is_empty() {
            continue;
        }
        let named = |side: &str| Node {
            id: side.trim().to_string(),
            label: side.trim().to_string(),
            shape: Shape::Rect,
        };
        let (Some(from), Some(to)) = (
            intern(&mut actors, named(left)),
            intern(&mut actors, named(right)),
        ) else {
            return Diagram::TooLarge {
                nodes: actors.len(),
            };
        };
        messages.push(Message {
            from,
            to,
            text: text.trim().to_string(),
            line: line_kind,
            arrow,
        });
    }
    if actors.is_empty() {
        return Diagram::Empty;
    }
    Diagram::Sequence { actors, messages }
}

/// Sequence diagrams have their own arrow alphabet (`->>`, `-->>`, `-x`), and
/// `-->>` must be tried before `->>`.
fn sequence_arrow(wire: &str) -> (Line, bool, Option<(&str, &str)>) {
    const MARKS: &[(&str, Line, bool)] = &[
        ("-->>", Line::Dotted, true),
        ("--x", Line::Dotted, true),
        ("-->", Line::Dotted, false),
        ("->>", Line::Solid, true),
        ("-x", Line::Solid, true),
        ("->", Line::Solid, false),
        ("--", Line::Dotted, false),
    ];
    for (mark, kind, arrow) in MARKS {
        if let Some((left, right)) = wire.split_once(mark) {
            return (*kind, *arrow, Some((left, right)));
        }
    }
    (Line::Solid, true, None)
}

// ------------------------------------------------------------------- layout

/// One node, placed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Placed {
    pub node: Node,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

fn label_width(label: &str) -> f64 {
    // Counted in CHARS rather than bytes: a Hangul or CJK label is half the
    // characters of its UTF-8 length, and sizing by bytes would make every
    // Korean box three times too wide.
    let chars = label.chars().count() as f64;
    // Wide scripts advance about a full em where Latin advances about half.
    let wide = label
        .chars()
        .filter(|c| matches!(*c as u32, 0x1100..=0x11FF | 0x3000..=0x9FFF | 0xAC00..=0xD7AF))
        .count() as f64;
    let advance = (chars - wide) * metrics::CHAR_WIDTH + wide * metrics::CHAR_WIDTH * 2.0;
    (advance + metrics::NODE_PAD_X * 2.0).max(metrics::NODE_MIN_WIDTH)
}

/// Longest-path ranking: a node's rank is one past the deepest thing pointing
/// at it.
///
/// Cycles are the reason this is written as an iteration with a bound rather
/// than a recursion: `A --> B --> A` is a diagram people draw, and a recursive
/// depth would not return from it. The bound is the node count, which is the
/// longest a rank can legitimately be.
fn rank_nodes(count: usize, edges: &[Edge]) -> Vec<usize> {
    let mut rank = vec![0usize; count];
    for _ in 0..count {
        let mut moved = false;
        for edge in edges {
            if edge.from == edge.to {
                continue;
            }
            let wanted = rank[edge.from] + 1;
            if rank[edge.to] < wanted {
                rank[edge.to] = wanted;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    rank
}

/// Place a flowchart's nodes.
///
/// Layered: rank decides the position along the flow axis, order of first
/// appearance decides the position across it. Each rank is centred against the
/// widest one, which is what stops a diagram with one long rank from looking
/// like a staircase.
pub fn layout_flowchart(flow: Flow, nodes: &[Node], edges: &[Edge]) -> (Vec<Placed>, f64, f64) {
    let rank = rank_nodes(nodes.len(), edges);
    let ranks = rank.iter().copied().max().map_or(0, |top| top + 1);
    // Nodes per rank, in first-appearance order.
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); ranks];
    for (at, &row) in rank.iter().enumerate() {
        rows[row].push(at);
    }

    let widths: Vec<f64> = nodes.iter().map(|node| label_width(&node.label)).collect();
    let horizontal = flow.horizontal();
    // The extent of each rank ACROSS the flow, and the widest of them.
    let cross_extent = |row: &Vec<usize>| -> f64 {
        if horizontal {
            // Across a left-to-right flow is vertical, so each node costs a
            // height.
            row.len() as f64 * metrics::NODE_HEIGHT
                + (row.len().saturating_sub(1)) as f64 * metrics::GAP_WITHIN_RANK
        } else {
            row.iter().map(|&at| widths[at]).sum::<f64>()
                + (row.len().saturating_sub(1)) as f64 * metrics::GAP_WITHIN_RANK
        }
    };
    let widest = rows.iter().map(cross_extent).fold(0.0f64, f64::max);
    // The extent ALONG the flow: the deepest node in each rank.
    let along_of = |row: &Vec<usize>| -> f64 {
        if horizontal {
            row.iter().map(|&at| widths[at]).fold(0.0f64, f64::max)
        } else {
            metrics::NODE_HEIGHT
        }
    };
    let along_total: f64 = rows.iter().map(along_of).sum::<f64>()
        + (ranks.saturating_sub(1)) as f64 * metrics::GAP_BETWEEN_RANKS;

    let mut placed: Vec<Option<Placed>> = vec![None; nodes.len()];
    let mut along = metrics::MARGIN;
    for row in &rows {
        let extent = cross_extent(row);
        let mut cross = metrics::MARGIN + (widest - extent) / 2.0;
        let depth = along_of(row);
        for &at in row {
            let (x, y, width, height) = if horizontal {
                (along, cross, widths[at], metrics::NODE_HEIGHT)
            } else {
                (cross, along, widths[at], metrics::NODE_HEIGHT)
            };
            // A reversed flow is the same layout read from the far end, which
            // keeps one placement pass rather than two.
            let (x, y) = match flow {
                Flow::Left => (metrics::MARGIN * 2.0 + along_total - x - width, y),
                Flow::Up => (x, metrics::MARGIN * 2.0 + along_total - y - height),
                _ => (x, y),
            };
            placed[at] = Some(Placed {
                node: nodes[at].clone(),
                x,
                y,
                width,
                height,
            });
            cross += if horizontal {
                metrics::NODE_HEIGHT + metrics::GAP_WITHIN_RANK
            } else {
                widths[at] + metrics::GAP_WITHIN_RANK
            };
        }
        along += depth + metrics::GAP_BETWEEN_RANKS;
    }

    let (width, height) = if horizontal {
        (
            along_total + metrics::MARGIN * 2.0,
            widest + metrics::MARGIN * 2.0,
        )
    } else {
        (
            widest + metrics::MARGIN * 2.0,
            along_total + metrics::MARGIN * 2.0,
        )
    };
    // Every index was ranked, so every slot is filled; `flatten` rather than
    // `unwrap` so a future ranking bug is a missing box instead of a panic in a
    // file viewer.
    (placed.into_iter().flatten().collect(), width, height)
}

// ------------------------------------------------------------------ marks

/// Where a text mark sits relative to its x.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Middle,
    Start,
}

/// One thing to draw. The window's whole vocabulary — four shapes, and every
/// label arrives as `text` for `textContent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mark", rename_all = "snake_case")]
pub enum Mark {
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        radius: f64,
        class: String,
    },
    Polygon {
        points: Vec<[f64; 2]>,
        class: String,
    },
    /// A run of two or more points. A straight edge is two of them; a message to
    /// oneself is four.
    Polyline {
        points: Vec<[f64; 2]>,
        class: String,
        arrow: bool,
    },
    Text {
        x: f64,
        y: f64,
        text: String,
        class: String,
        anchor: Anchor,
    },
}

/// A diagram, placed and ready to draw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub width: f64,
    pub height: f64,
    pub marks: Vec<Mark>,
}

/// Why a diagram was not drawn.
///
/// Two reasons, and the window says something different for each: one is "this
/// kind is not built yet", the other is "this file is bigger than we will lay
/// out". A single flag would make the window guess, and it would guess wrong
/// half the time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "why", rename_all = "snake_case")]
pub enum Undrawn {
    /// A diagram kind with no layout here. `name` is the source's own word.
    Kind { name: String },
    /// More than [`MAX_NODES`].
    TooLarge { nodes: usize, limit: usize },
}

/// What the window gets back for a piece of mermaid text.
///
/// Both fields empty means there was nothing in the source. Classes carry the
/// palette rather than literal colours, so one drawing is correct in both
/// treatments — Orca instead initialises the library with
/// `theme: isDark ? "dark" : "default"` and re-renders the whole diagram on
/// every theme change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MermaidRender {
    /// The marks, when this is a kind we draw.
    pub drawing: Option<Drawing>,
    /// Why not, when there are none.
    pub undrawn: Option<Undrawn>,
}

fn edge_class(line: Line) -> String {
    match line {
        Line::Solid => "mmd-edge".into(),
        Line::Dotted => "mmd-edge mmd-edge--dotted".into(),
        Line::Thick => "mmd-edge mmd-edge--thick".into(),
    }
}

/// The arrowhead, in the size it is drawn at.
const HEAD_LENGTH: f64 = 9.0;
const HEAD_HALF_WIDTH: f64 = 4.0;

/// Where a line from `from` toward the box `at` meets that box's border.
///
/// Edges are computed centre to centre, because a centre is the one point a box
/// of any shape has. Drawn that way they run UNDER the box and put the arrowhead
/// in the middle of the label, so each end is pulled back to the border here.
///
/// The box is treated as its bounding rectangle even when the node is a diamond,
/// which leaves a hair of daylight on a diagonal approach — cheap, and invisible
/// next to an arrow buried in the text.
fn clip_to_box(from: [f64; 2], at: &Placed, gap: f64) -> [f64; 2] {
    let centre = [at.x + at.width / 2.0, at.y + at.height / 2.0];
    let (dx, dy) = (from[0] - centre[0], from[1] - centre[1]);
    let span = (dx * dx + dy * dy).sqrt();
    // Two boxes on the same centre have no direction to pull back along.
    if span < f64::EPSILON {
        return centre;
    }
    let along = |half: f64, delta: f64| {
        if delta.abs() < f64::EPSILON {
            f64::INFINITY
        } else {
            half / delta.abs()
        }
    };
    // The nearer of the two border crossings, plus the gap expressed in the same
    // units as the direction vector.
    let scale = along(at.width / 2.0, dx).min(along(at.height / 2.0, dy)) + gap / span;
    [centre[0] + dx * scale, centre[1] + dy * scale]
}

/// The three points of an arrowhead whose tip is `tip`, pointing away from
/// `from`.
fn arrow_head(from: [f64; 2], tip: [f64; 2]) -> Vec<[f64; 2]> {
    let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
    let span = (dx * dx + dy * dy).sqrt();
    if span < f64::EPSILON {
        return Vec::new();
    }
    let (ux, uy) = (dx / span, dy / span);
    let base = [tip[0] - ux * HEAD_LENGTH, tip[1] - uy * HEAD_LENGTH];
    // The perpendicular, for the two back corners.
    let (px, py) = (-uy * HEAD_HALF_WIDTH, ux * HEAD_HALF_WIDTH);
    vec![
        tip,
        [base[0] + px, base[1] + py],
        [base[0] - px, base[1] - py],
    ]
}

/// Draw one connector: the line, and its head if it has one.
///
/// The head is a polygon rather than an SVG `<marker>` because a marker needs a
/// document-unique id, and a markdown file with three diagrams in it would have
/// three of the same id — with the third one's arrows quietly resolving to the
/// first one's definition, and dangling the moment that diagram is repainted.
fn connector(points: Vec<[f64; 2]>, class: String, arrow: bool, into: &mut Vec<Mark>) {
    if arrow && points.len() >= 2 {
        let tip = points[points.len() - 1];
        let head = arrow_head(points[points.len() - 2], tip);
        if !head.is_empty() {
            into.push(Mark::Polyline {
                points,
                class,
                arrow: true,
            });
            into.push(Mark::Polygon {
                points: head,
                class: "mmd-head".into(),
            });
            return;
        }
    }
    into.push(Mark::Polyline {
        points,
        class,
        arrow: false,
    });
}

/// Every mark that draws one node's outline. More than one for a subroutine,
/// whose two inner rules are what tell it from a plain box.
fn node_marks(placed: &Placed, into: &mut Vec<Mark>) {
    let (x, y, w, h) = (placed.x, placed.y, placed.width, placed.height);
    let box_of = |radius: f64| Mark::Rect {
        x,
        y,
        width: w,
        height: h,
        radius,
        class: "mmd-node".into(),
    };
    match placed.node.shape {
        Shape::Diamond => {
            let (cx, cy) = (x + w / 2.0, y + h / 2.0);
            into.push(Mark::Polygon {
                points: vec![[cx, y], [x + w, cy], [cx, y + h], [x, cy]],
                class: "mmd-node".into(),
            });
        }
        // A circle in a layered layout is a box with its ends rounded off: the
        // label decides the width, so a true circle would either clip the text
        // or dwarf every neighbour.
        Shape::Circle => into.push(box_of(h / 2.0)),
        Shape::Round => into.push(box_of(8.0)),
        Shape::Cylinder => into.push(box_of(w / 2.0)),
        Shape::Subroutine => {
            into.push(box_of(0.0));
            for at in [x + 6.0, x + w - 6.0] {
                into.push(Mark::Polyline {
                    points: vec![[at, y], [at, y + h]],
                    class: "mmd-node-rule".into(),
                    arrow: false,
                });
            }
        }
        Shape::Rect | Shape::Plain => into.push(box_of(4.0)),
    }
}

/// One tenth of a pixel, which is finer than a display can show.
///
/// Trigonometry leaves tails like `81.77555555555556`, and every one of those
/// becomes a 17-character attribute string in the document. Snapped once here
/// rather than at each of the twenty places a coordinate is computed.
fn snap(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn snap_points(points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points
        .into_iter()
        .map(|[x, y]| [snap(x), snap(y)])
        .collect()
}

fn snapped(mark: Mark) -> Mark {
    match mark {
        Mark::Rect {
            x,
            y,
            width,
            height,
            radius,
            class,
        } => Mark::Rect {
            x: snap(x),
            y: snap(y),
            width: snap(width),
            height: snap(height),
            radius: snap(radius),
            class,
        },
        Mark::Polygon { points, class } => Mark::Polygon {
            points: snap_points(points),
            class,
        },
        Mark::Polyline {
            points,
            class,
            arrow,
        } => Mark::Polyline {
            points: snap_points(points),
            class,
            arrow,
        },
        Mark::Text {
            x,
            y,
            text,
            class,
            anchor,
        } => Mark::Text {
            x: snap(x),
            y: snap(y),
            text,
            class,
            anchor,
        },
    }
}

fn snap_drawing(drawing: Drawing) -> Drawing {
    Drawing {
        width: snap(drawing.width),
        height: snap(drawing.height),
        marks: drawing.marks.into_iter().map(snapped).collect(),
    }
}

/// Place and draw a parsed diagram.
pub fn to_drawing(diagram: &Diagram) -> MermaidRender {
    match diagram {
        Diagram::Flowchart { flow, nodes, edges } => MermaidRender {
            drawing: Some(snap_drawing(flowchart_marks(*flow, nodes, edges))),
            undrawn: None,
        },
        Diagram::Sequence { actors, messages } => MermaidRender {
            drawing: Some(snap_drawing(sequence_marks(actors, messages))),
            undrawn: None,
        },
        Diagram::Unsupported { kind } => MermaidRender {
            drawing: None,
            undrawn: Some(Undrawn::Kind { name: kind.clone() }),
        },
        Diagram::TooLarge { nodes } => MermaidRender {
            drawing: None,
            undrawn: Some(Undrawn::TooLarge {
                nodes: *nodes,
                limit: MAX_NODES,
            }),
        },
        Diagram::Empty => MermaidRender::default(),
    }
}

fn flowchart_marks(flow: Flow, nodes: &[Node], edges: &[Edge]) -> Drawing {
    let (placed, width, height) = layout_flowchart(flow, nodes, edges);
    let mut marks = Vec::new();
    // Edges first, so a line never draws over a box's label.
    for edge in edges {
        let (Some(from), Some(to)) = (placed.get(edge.from), placed.get(edge.to)) else {
            continue;
        };
        let centre_from = [from.x + from.width / 2.0, from.y + from.height / 2.0];
        let centre_to = [to.x + to.width / 2.0, to.y + to.height / 2.0];
        // Both ends pulled back to their borders. A self-edge has one box and no
        // direction, so it keeps its centres and draws as a stub — a loop for it
        // is the one flowchart shape this layout does not have room for.
        let (tail, tip) = if edge.from == edge.to {
            (centre_from, centre_to)
        } else {
            (
                clip_to_box(centre_to, from, 0.0),
                clip_to_box(centre_from, to, 2.0),
            )
        };
        connector(
            vec![tail, tip],
            edge_class(edge.line),
            edge.arrow,
            &mut marks,
        );
        if let Some(label) = &edge.label {
            let (mx, my) = (
                (centre_from[0] + centre_to[0]) / 2.0,
                (centre_from[1] + centre_to[1]) / 2.0,
            );
            // A plate under the label, because an edge label sits ON its line.
            let plate = label_width(label) * 0.7;
            marks.push(Mark::Rect {
                x: mx - plate / 2.0,
                y: my - 8.0,
                width: plate,
                height: 16.0,
                radius: 3.0,
                class: "mmd-edge-plate".into(),
            });
            marks.push(Mark::Text {
                x: mx,
                y: my + 4.0,
                text: label.clone(),
                class: "mmd-edge-label".into(),
                anchor: Anchor::Middle,
            });
        }
    }
    for one in &placed {
        node_marks(one, &mut marks);
        marks.push(Mark::Text {
            x: one.x + one.width / 2.0,
            y: one.y + one.height / 2.0 + 4.0,
            text: one.node.label.clone(),
            class: "mmd-label".into(),
            anchor: Anchor::Middle,
        });
    }
    Drawing {
        width,
        height,
        marks,
    }
}

fn sequence_marks(actors: &[Node], messages: &[Message]) -> Drawing {
    let widths: Vec<f64> = actors.iter().map(|one| label_width(&one.label)).collect();
    // Each actor's centre, laid left to right in declaration order.
    let mut centres = Vec::with_capacity(actors.len());
    let mut x = metrics::MARGIN;
    for width in &widths {
        centres.push(x + width / 2.0);
        x += width + metrics::ACTOR_GAP;
    }
    let width = x - metrics::ACTOR_GAP + metrics::MARGIN;
    let height =
        metrics::ACTOR_TOP + metrics::MESSAGE_GAP * (messages.len() as f64 + 1.0) + metrics::MARGIN;

    let mut marks = Vec::new();
    // The lifelines, behind everything.
    for centre in &centres {
        marks.push(Mark::Polyline {
            points: vec![
                [*centre, metrics::ACTOR_TOP],
                [*centre, height - metrics::MARGIN],
            ],
            class: "mmd-lifeline".into(),
            arrow: false,
        });
    }
    // A message ends ON its target's lifeline, so its head needs no clipping —
    // only the two pixels that keep the tip off the line itself.
    let toward = |from: f64, to: f64| if to > from { to - 2.0 } else { to + 2.0 };
    for (at, actor) in actors.iter().enumerate() {
        marks.push(Mark::Rect {
            x: centres[at] - widths[at] / 2.0,
            y: metrics::MARGIN,
            width: widths[at],
            height: metrics::NODE_HEIGHT - 12.0,
            radius: 4.0,
            class: "mmd-node".into(),
        });
        marks.push(Mark::Text {
            x: centres[at],
            y: metrics::MARGIN + 17.0,
            text: actor.label.clone(),
            class: "mmd-label".into(),
            anchor: Anchor::Middle,
        });
    }
    for (row, message) in messages.iter().enumerate() {
        let y = metrics::ACTOR_TOP + metrics::MESSAGE_GAP * (row as f64 + 1.0);
        let (Some(&from), Some(&to)) = (centres.get(message.from), centres.get(message.to)) else {
            continue;
        };
        // A message to oneself is a loop out and back, not a zero-length line.
        if message.from == message.to {
            let out_x = from + 34.0;
            connector(
                vec![
                    [from, y],
                    [out_x, y],
                    [out_x, y + 18.0],
                    [from + 2.0, y + 18.0],
                ],
                edge_class(message.line),
                message.arrow,
                &mut marks,
            );
            marks.push(Mark::Text {
                x: out_x + 8.0,
                y: y + 2.0,
                text: message.text.clone(),
                class: "mmd-msg".into(),
                anchor: Anchor::Start,
            });
            continue;
        }
        connector(
            vec![[from, y], [toward(from, to), y]],
            edge_class(message.line),
            message.arrow,
            &mut marks,
        );
        marks.push(Mark::Text {
            x: (from + to) / 2.0,
            y: y - 7.0,
            text: message.text.clone(),
            class: "mmd-msg".into(),
            anchor: Anchor::Middle,
        });
    }
    Drawing {
        width,
        height,
        marks,
    }
}

/// Parse and place in one call — what the window asks for.
pub fn render(source: &str) -> MermaidRender {
    to_drawing(&parse(source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flowchart_reads_its_nodes_shapes_edges_and_labels() {
        let Diagram::Flowchart { flow, nodes, edges } = parse(
            "graph LR\n  A[Start] --> B{Ok?}\n  B -->|yes| C((Done))\n  B -.->|no| D[(Store)]\n",
        ) else {
            panic!("not read as a flowchart");
        };
        assert_eq!(flow, Flow::Right);
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[0].label, "Start");
        assert_eq!(nodes[1].shape, Shape::Diamond);
        assert_eq!(nodes[2].shape, Shape::Circle);
        assert_eq!(nodes[3].shape, Shape::Cylinder);
        assert_eq!(edges.len(), 3);
        assert_eq!(edges[1].label.as_deref(), Some("yes"));
        assert_eq!(edges[2].line, Line::Dotted);
        assert!(edges[2].arrow);
    }

    /// The longest-arrow-first rule. Read shortest-first, `-->` becomes `--`
    /// with a stray `>` glued to the next node's id.
    #[test]
    fn the_longest_arrow_wins_at_the_same_position() {
        let Diagram::Flowchart { nodes, edges, .. } = parse("graph TD\nA-->B\n") else {
            panic!("not a flowchart");
        };
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].id, "B", "the arrow was read short: {nodes:?}");
        assert_eq!(edges.len(), 1);
        assert!(edges[0].arrow);

        // And the dotted one, whose first character is also a plain dash.
        let Diagram::Flowchart { edges, .. } = parse("graph TD\nA-.->B\n") else {
            panic!("not a flowchart");
        };
        assert_eq!(edges[0].line, Line::Dotted);
    }

    /// A label declared AFTER its first mention still replaces the placeholder —
    /// mermaid allows it, and the label is the part a reader needs.
    #[test]
    fn a_later_declaration_gives_a_placeholder_its_real_label() {
        let Diagram::Flowchart { nodes, .. } = parse("graph TD\nA --> B\nB[Finished]\n") else {
            panic!("not a flowchart");
        };
        assert_eq!(nodes.len(), 2, "B was added twice: {nodes:?}");
        assert_eq!(nodes[1].label, "Finished");
        assert_eq!(nodes[1].shape, Shape::Rect);
    }

    /// `%%` comments and `%%{init}%%` directives leave, and what is left still
    /// parses. A directive read as a node used to put `{init: …}` in a box.
    #[test]
    fn comments_and_directives_do_not_become_nodes() {
        let Diagram::Flowchart { nodes, .. } = parse(
            "%%{init: {'theme':'dark'}}%%\ngraph TD\n  %% the happy path\n  A --> B %% trailing\n",
        ) else {
            panic!("not a flowchart");
        };
        assert_eq!(nodes.len(), 2, "{nodes:?}");
        assert!(nodes.iter().all(|one| !one.label.contains("init")));
    }

    /// A cycle is a diagram people draw. Ranking it must terminate.
    #[test]
    fn a_cycle_is_ranked_without_running_forever() {
        let Diagram::Flowchart { flow, nodes, edges } = parse("graph TD\nA-->B\nB-->C\nC-->A\n")
        else {
            panic!("not a flowchart");
        };
        let (placed, width, height) = layout_flowchart(flow, &nodes, &edges);
        assert_eq!(placed.len(), 3);
        assert!(width > 0.0 && height > 0.0);
        // And a self-edge, which would be an infinite rank bump.
        let Diagram::Flowchart { flow, nodes, edges } = parse("graph TD\nA-->A\n") else {
            panic!("not a flowchart");
        };
        assert_eq!(layout_flowchart(flow, &nodes, &edges).0.len(), 1);
    }

    #[test]
    fn a_sequence_diagram_reads_participants_in_declaration_order() {
        let Diagram::Sequence { actors, messages } = parse(
            "sequenceDiagram\n  participant W as Window\n  participant A as Agent\n  \
             W->>A: prompt\n  A-->>W: done\n  A->>A: think\n",
        ) else {
            panic!("not a sequence diagram");
        };
        // Declaration order, not first-mention: mermaid places them as declared.
        assert_eq!(actors.len(), 2);
        assert_eq!(actors[0].label, "Window");
        assert_eq!(actors[1].label, "Agent");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].text, "prompt");
        // `-->>` is dotted and must not be read as `->>`.
        assert_eq!(messages[1].line, Line::Dotted);
        // A message to oneself.
        assert_eq!(messages[2].from, messages[2].to);
    }

    /// Text leaves here as TEXT. Nothing on this path escapes anything, because
    /// nothing on it builds markup — a label that looks like a tag arrives
    /// verbatim in a [`Mark::Text`] and the window sets it as `textContent`.
    #[test]
    fn a_label_leaves_as_text_and_never_as_markup() {
        let source = "graph TD\n  A[\"</text><script>alert(1)</script>\"] --> B[\"a & b < c\"]\n";
        let drawing = render(source).drawing.expect("a flowchart draws");
        let texts: Vec<&str> = drawing
            .marks
            .iter()
            .filter_map(|mark| match mark {
                Mark::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            texts.contains(&"</text><script>alert(1)</script>"),
            "the label was mangled on the way out: {texts:?}"
        );
        assert!(texts.contains(&"a & b < c"), "{texts:?}");
        // No mark carries markup, because no mark carries a string that will be
        // parsed as markup.
        assert!(matches!(
            drawing.marks.first(),
            Some(Mark::Polyline { .. } | Mark::Rect { .. })
        ));
    }

    /// A kind this renderer does not draw says WHICH, so the window can name it
    /// instead of showing an error where a picture should be.
    #[test]
    fn an_undrawn_kind_names_itself_and_an_empty_source_is_empty() {
        assert_eq!(
            parse("gantt\n  title A\n"),
            Diagram::Unsupported {
                kind: "gantt".into()
            }
        );
        let undrawn = render("gantt\ntitle A\n");
        assert!(undrawn.drawing.is_none());
        assert_eq!(
            undrawn.undrawn,
            Some(Undrawn::Kind {
                name: "gantt".into()
            })
        );

        assert_eq!(parse(""), Diagram::Empty);
        assert_eq!(parse("%% only a comment\n"), Diagram::Empty);
        // A header with nothing under it is empty rather than a blank canvas.
        assert_eq!(parse("graph TD\n"), Diagram::Empty);
        // Empty is BOTH empty: no drawing, and no kind to name either, so the
        // window says nothing rather than "we do not draw ''".
        assert_eq!(render(""), MermaidRender::default());
    }

    /// An arrow stops AT its target, not in the middle of its label.
    ///
    /// Drawn centre to centre the head lands on top of the text — the whole
    /// reason [`clip_to_box`] exists — so this measures the tip against the
    /// target's rectangle and insists it is outside it.
    #[test]
    fn an_arrow_stops_outside_the_box_it_points_at() {
        for source in ["graph TD\nA[Start] --> B[Finish]\n", "graph LR\nA-->B\n"] {
            let Diagram::Flowchart { flow, nodes, edges } = parse(source) else {
                panic!("not a flowchart");
            };
            let (placed, _, _) = layout_flowchart(flow, &nodes, &edges);
            let target = &placed[1];
            let drawing = render_of(source);
            let head = drawing
                .marks
                .iter()
                .find_map(|mark| match mark {
                    Mark::Polygon { points, class } if class == "mmd-head" => Some(points.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no arrowhead in {source:?}"));
            let tip = head[0];
            let inside = tip[0] > target.x
                && tip[0] < target.x + target.width
                && tip[1] > target.y
                && tip[1] < target.y + target.height;
            assert!(
                !inside,
                "the arrow tip {tip:?} is inside the target box \
                 ({}, {}, {}×{}) for {source:?}",
                target.x, target.y, target.width, target.height
            );
            // And it points the right way: the tip is the point of the three
            // nearest the target.
            let centre = [
                target.x + target.width / 2.0,
                target.y + target.height / 2.0,
            ];
            let reach = |point: &[f64; 2]| (point[0] - centre[0]).hypot(point[1] - centre[1]);
            assert!(
                head[1..].iter().all(|back| reach(back) > reach(&tip)),
                "the arrowhead points backwards in {source:?}: {head:?}"
            );
        }
    }

    /// An edge with no arrow gets no head. `A --- B` in mermaid is a plain link,
    /// and an unconditional head would turn every one of them into a direction
    /// the author did not write.
    #[test]
    fn a_plain_link_has_no_head() {
        let drawing = render_of("graph TD\nA --- B\n");
        assert!(
            !drawing
                .marks
                .iter()
                .any(|mark| matches!(mark, Mark::Polygon { class, .. } if class == "mmd-head")),
            "a link with no arrow grew one"
        );
    }

    /// A file bigger than we lay out is refused, quickly, and says so with a
    /// reason of its own rather than the same word an undrawn KIND gets.
    ///
    /// The point of this test is the clock: interning and ranking are both
    /// quadratic in the node count, so without the bound a megabyte of chained
    /// edges — well inside the file viewer's own cap — is 10^10 comparisons in
    /// the middle of a click.
    #[test]
    fn a_diagram_past_the_node_bound_is_refused_rather_than_ground_through() {
        let mut source = String::from("graph TD\n");
        for at in 0..(MAX_NODES * 6) {
            source.push_str(&format!("  N{at} --> N{}\n", at + 1));
        }
        let started = std::time::Instant::now();
        let render = render(&source);
        // Generous by three orders of magnitude against the unbounded cost; the
        // assertion is "it returned", not a benchmark.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "the bound did not stop the read: {:?}",
            started.elapsed()
        );
        assert!(render.drawing.is_none());
        assert_eq!(
            render.undrawn,
            Some(Undrawn::TooLarge {
                nodes: MAX_NODES,
                limit: MAX_NODES
            })
        );

        // And the bound does not fire on a diagram AT the limit, which is the
        // off-by-one that would refuse a legitimate 400-box file.
        let mut fits = String::from("graph TD\n");
        for at in 0..(MAX_NODES - 1) {
            fits.push_str(&format!("  N{at} --> N{}\n", at + 1));
        }
        let drawing = render_of(&fits);
        assert_eq!(
            drawing
                .marks
                .iter()
                .filter(|one| matches!(one, Mark::Text { .. }))
                .count(),
            MAX_NODES
        );
    }

    fn render_of(source: &str) -> Drawing {
        render(source).drawing.expect("this source draws")
    }

    /// A diagram whose LATER edges only connect nodes it already has is not
    /// refused: the bound is on distinct nodes, and an id already held resolves
    /// without adding one.
    #[test]
    fn edges_between_nodes_already_held_do_not_trip_the_bound() {
        let mut source = String::from("graph LR\n  A --> B\n");
        for _ in 0..5_000 {
            source.push_str("  A --> B\n");
        }
        let drawing = render_of(&source);
        assert_eq!(
            drawing
                .marks
                .iter()
                .filter(|one| matches!(one, Mark::Text { .. }))
                .count(),
            2,
            "the repeated edge added nodes"
        );
    }

    /// The mark list is what crosses to the window, so its field names are wire
    /// names — snake_case, with the variant under `mark`.
    #[test]
    fn marks_serialise_under_the_names_the_window_reads() {
        let drawing = render("sequenceDiagram\n  A->>B: hi\n")
            .drawing
            .expect("a sequence draws");
        let json = serde_json::to_value(&drawing).expect("a drawing serialises");
        assert!(json["width"].is_number() && json["height"].is_number());
        let marks = json["marks"].as_array().expect("marks is a list");
        let kinds: Vec<&str> = marks
            .iter()
            .filter_map(|one| one["mark"].as_str())
            .collect();
        assert!(kinds.contains(&"polyline"), "{kinds:?}");
        assert!(kinds.contains(&"rect"), "{kinds:?}");
        assert!(kinds.contains(&"text"), "{kinds:?}");
        let text = marks
            .iter()
            .find(|one| one["mark"] == "text" && one["text"] == "hi")
            .expect("the message text is a mark");
        assert_eq!(text["anchor"], "middle");
        assert!(text["class"].as_str().unwrap_or_default().contains("mmd-"));
    }

    /// Coordinates arrive snapped. Every one becomes an attribute string, and
    /// `81.77555555555556` is seventeen characters of accuracy no display has.
    #[test]
    fn coordinates_are_snapped_before_they_become_attributes() {
        // A diagonal edge, which is where the tails come from.
        let drawing = render_of("graph TD\n  A --> B\n  A --> C\n");
        let long = |value: f64| {
            let printed = format!("{value}");
            printed
                .split_once('.')
                .is_some_and(|(_, tail)| tail.len() > 1)
        };
        assert!(!long(drawing.width) && !long(drawing.height));
        for mark in &drawing.marks {
            let numbers: Vec<f64> = match mark {
                Mark::Rect {
                    x,
                    y,
                    width,
                    height,
                    radius,
                    ..
                } => vec![*x, *y, *width, *height, *radius],
                Mark::Polygon { points, .. } | Mark::Polyline { points, .. } => {
                    points.iter().flat_map(|point| point.to_vec()).collect()
                }
                Mark::Text { x, y, .. } => vec![*x, *y],
            };
            assert!(
                !numbers.iter().copied().any(long),
                "a coordinate came through unsnapped: {mark:?}"
            );
        }
    }

    /// Wide scripts are sized by character, not by UTF-8 length — a Korean
    /// label measured in bytes makes a box three times too wide.
    #[test]
    fn a_wide_label_is_sized_by_its_characters() {
        let hangul = label_width("에이전트");
        let latin = label_width("agent");
        assert!(
            hangul > latin,
            "a four-character wide label is not wider than a five-character latin one"
        );
        // And nowhere near what its 12 UTF-8 bytes would give.
        assert!(
            hangul < label_width("aaaaaaaaaaaa"),
            "sized by bytes: {hangul}"
        );
    }

    /// The flow direction actually moves the boxes, and a reversed flow is the
    /// same layout read from the far end.
    #[test]
    fn direction_decides_which_axis_ranks_advance_along() {
        let read = |text: &str| {
            let Diagram::Flowchart { flow, nodes, edges } = parse(text) else {
                panic!("not a flowchart");
            };
            layout_flowchart(flow, &nodes, &edges).0
        };
        let down = read("graph TD\nA-->B\n");
        assert!(down[1].y > down[0].y, "TD did not advance downward");
        assert!((down[0].x - down[1].x).abs() < 1.0, "TD drifted sideways");

        let right = read("graph LR\nA-->B\n");
        assert!(right[1].x > right[0].x, "LR did not advance rightward");

        // Reversed: the second rank is ABOVE the first.
        let up = read("graph BT\nA-->B\n");
        assert!(up[1].y < up[0].y, "BT did not advance upward");
        let left = read("graph RL\nA-->B\n");
        assert!(left[1].x < left[0].x, "RL did not advance leftward");
    }

    /// Containers and styling statements are skipped, and their contents still
    /// draw — a `subgraph` used to swallow the nodes inside it.
    #[test]
    fn subgraphs_and_styling_are_skipped_but_their_contents_are_not() {
        let Diagram::Flowchart { nodes, edges, .. } = parse(
            "flowchart TD\n  subgraph one [Group]\n    A --> B\n  end\n  \
             style A fill:#f00\n  classDef big font-size:20px\n  B --> C\n",
        ) else {
            panic!("not a flowchart");
        };
        assert_eq!(nodes.len(), 3, "{nodes:?}");
        assert_eq!(edges.len(), 2);
        assert!(
            nodes.iter().all(|one| one.id != "one" && one.id != "style"),
            "a container or a style statement became a node: {nodes:?}"
        );
    }
}
