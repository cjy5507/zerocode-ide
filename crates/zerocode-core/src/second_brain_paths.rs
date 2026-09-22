//! Paths between two nodes of the vault graph (t-5966, G3).
//!
//! Graphify answers "how do these two concepts connect"; our graph had a
//! neighbourhood walk and a search. This is the one calculator: the window's
//! Shift-click, the `zo vault path` verb and anything a Jev seat may one day
//! rank all call [`VaultGraph::paths`] and count nothing of their own — the
//! window used to run a breadth-first search of its own over its lens's
//! subset, and two calculators over two pictures answered two things.
//!
//! The walk is undirected — a relation joins two pages whichever wrote it —
//! and every hop carries the edge it crossed with the road that wrote it
//! ([`crate::second_brain_graph::EdgeProvenance`]) and whether it was crossed
//! against its arrow. Paths come shortest first, then in node order, so the
//! same question answers the same twice. Every bound is in [`PATH_LIMITS`].
//!
//! The vault's home and journal are never an intermediate hop: `wiki/index.md`
//! links every page, so through it everything is two hops from everything and
//! the answer says nothing. Either may still be an endpoint.

use std::collections::VecDeque;
use std::fmt;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::second_brain::{WIKI_DIR, WIKI_INDEX_FILE, WIKI_LOG_FILE};
use crate::second_brain_graph::{
    EdgeKind, EdgeProvenance, GraphEdge, MARKDOWN_SUFFIX, NodeKind, VaultGraph,
};

/// The bounds of one path question, spelled once and sent with every answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathLimits {
    /// Hops one path may have. Past this a connection is not a path a
    /// reader follows; it is the graph being connected.
    pub max_hops: usize,
    /// Paths one answer may carry, whatever `k` was asked.
    pub k_max: usize,
    /// Search steps one question may spend. A hub-dense vault has more
    /// six-hop simple paths than a screen can show, and the walk stops at
    /// this many expansions and says so rather than finishing them.
    pub expansions_max: usize,
}

/// The table.
pub const PATH_LIMITS: PathLimits = PathLimits {
    max_hops: 6,
    k_max: 16,
    expansions_max: 200_000,
};

/// One step of a path: the line crossed, and whether it was crossed from its
/// `to` to its `from`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathHop {
    pub edge: GraphEdge,
    pub reversed: bool,
}

/// One path: the nodes in walking order (endpoints included) and the hop
/// between each consecutive pair — `hops.len() == nodes.len() - 1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphPath {
    pub nodes: Vec<u32>,
    pub hops: Vec<PathHop>,
}

impl GraphPath {
    /// Hops walked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hops.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hops.is_empty()
    }
}

/// The answer to one question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathAnswer {
    /// Shortest first, then in node order; at most `k` of them.
    pub paths: Vec<GraphPath>,
    /// Hops of the shortest path, or nothing when the two are not connected
    /// within [`PathLimits::max_hops`].
    pub shortest: Option<u32>,
    /// The walk spent [`PathLimits::expansions_max`] steps before finishing,
    /// so `paths` is what it found and not all there is.
    pub capped: bool,
    /// The bounds this answer was found under.
    pub limits: PathLimits,
}

/// Which of two lines between the same pair a hop crosses: a named relation
/// over a bare mention, a declared line over an inferred one, then the
/// earlier line. A path is read as a sentence and `implements` says more
/// than `mentions`.
fn hop_rank(edge: &GraphEdge, index: u32) -> (u8, u8, u32) {
    let named = u8::from(edge.kind == EdgeKind::Mentions);
    let road = match edge.provenance {
        EdgeProvenance::Declared => 0,
        EdgeProvenance::Inferred => 1,
        EdgeProvenance::Measured => 2,
    };
    (named, road, index)
}

/// Neighbour lists, undirected: for each node, every (neighbour, edge index)
/// with one entry per neighbour — the best-ranked line — in node order.
fn adjacency(graph: &VaultGraph) -> Vec<Vec<(u32, u32)>> {
    let mut lists: Vec<Vec<(u32, u32)>> = vec![Vec::new(); graph.nodes.len()];
    for (at, edge) in graph.edges.iter().enumerate() {
        let index = u32::try_from(at).unwrap_or(u32::MAX);
        let (from, to) = (edge.from as usize, edge.to as usize);
        if from >= lists.len() || to >= lists.len() || from == to {
            continue;
        }
        lists[from].push((edge.to, index));
        lists[to].push((edge.from, index));
    }
    for list in &mut lists {
        list.sort_by_key(|(neighbour, index)| {
            (*neighbour, hop_rank(&graph.edges[*index as usize], *index))
        });
        list.dedup_by_key(|(neighbour, _)| *neighbour);
    }
    lists
}

impl VaultGraph {
    /// Up to `k` simple paths from `from` to `to`, shortest first — see the
    /// module's words for what a path is and is not.
    ///
    /// An index outside the graph, or the same index twice, answers with no
    /// paths rather than a panic: the ids come off the wire.
    #[must_use]
    pub fn paths(&self, from: u32, to: u32, k: usize) -> PathAnswer {
        self.paths_within(from, to, k, PATH_LIMITS)
    }

    /// [`VaultGraph::paths`] under bounds of the caller's own — a test's way
    /// of watching the walk stop at its budget without a vault that big.
    #[must_use]
    pub fn paths_within(&self, from: u32, to: u32, k: usize, limits: PathLimits) -> PathAnswer {
        let k = k.clamp(1, limits.k_max.max(1));
        let count = self.nodes.len();
        let (a, b) = (from as usize, to as usize);
        let empty = |capped: bool| PathAnswer {
            paths: Vec::new(),
            shortest: None,
            capped,
            limits,
        };
        if a >= count || b >= count || a == b {
            return empty(false);
        }
        let lists = adjacency(self);
        // The home and the journal are endpoints at most.
        let usable: Vec<bool> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(at, node)| {
                at == a || at == b || (node.id != WIKI_INDEX_FILE && node.id != WIKI_LOG_FILE)
            })
            .collect();

        // Distance to `b` over usable nodes, for pruning: a partial path may
        // only step where the rest can still reach `b` within the length.
        let mut to_b: Vec<u32> = vec![u32::MAX; count];
        let mut queue = VecDeque::from([b]);
        to_b[b] = 0;
        while let Some(u) = queue.pop_front() {
            for &(v, _) in &lists[u] {
                let v = v as usize;
                if usable[v] && to_b[v] == u32::MAX {
                    to_b[v] = to_b[u] + 1;
                    queue.push_back(v);
                }
            }
        }
        if to_b[a] == u32::MAX || to_b[a] as usize > limits.max_hops {
            return empty(false);
        }
        let shortest = to_b[a] as usize;

        let mut paths: Vec<GraphPath> = Vec::new();
        let mut on_path = vec![false; count];
        let mut nodes: Vec<u32> = Vec::with_capacity(limits.max_hops + 1);
        let mut hops: Vec<PathHop> = Vec::with_capacity(limits.max_hops);
        let mut expansions = 0usize;
        let mut capped = false;
        'lengths: for length in shortest..=limits.max_hops {
            nodes.clear();
            hops.clear();
            nodes.push(from);
            on_path[a] = true;
            // An explicit stack of (node, next neighbour slot) — no recursion,
            // so a deep vault cannot ask the thread for a deeper stack.
            let mut stack: Vec<(usize, usize)> = vec![(a, 0)];
            while !stack.is_empty() {
                let depth = stack.len() - 1;
                let (u, slot) = {
                    let top = &mut stack[depth];
                    let held = *top;
                    top.1 += 1;
                    held
                };
                let Some(&(v, edge)) = lists[u].get(slot) else {
                    stack.pop();
                    if let Some(popped) = nodes.pop() {
                        on_path[popped as usize] = false;
                    }
                    hops.pop();
                    continue;
                };
                let v_at = v as usize;
                if !usable[v_at] || on_path[v_at] {
                    continue;
                }
                let remaining = length - depth - 1;
                if to_b[v_at] == u32::MAX || to_b[v_at] as usize > remaining {
                    continue;
                }
                expansions += 1;
                if expansions > limits.expansions_max {
                    capped = true;
                    break 'lengths;
                }
                let line = self.edges[edge as usize];
                let hop = PathHop {
                    edge: line,
                    reversed: line.to as usize == u && line.from == v,
                };
                if v_at == b {
                    if remaining == 0 {
                        let mut found_nodes = nodes.clone();
                        found_nodes.push(v);
                        let mut found_hops = hops.clone();
                        found_hops.push(hop);
                        paths.push(GraphPath {
                            nodes: found_nodes,
                            hops: found_hops,
                        });
                        if paths.len() >= k {
                            break 'lengths;
                        }
                    }
                    continue;
                }
                if remaining == 0 {
                    continue;
                }
                on_path[v_at] = true;
                nodes.push(v);
                hops.push(hop);
                stack.push((v_at, 0));
            }
            for held in &nodes {
                on_path[*held as usize] = false;
            }
        }
        PathAnswer {
            paths,
            shortest: Some(u32::try_from(shortest).unwrap_or(u32::MAX)),
            capped,
            limits,
        }
    }

    /// The node a person named: its id, its path below `wiki/`, its file
    /// stem, or its title — in that order, the first two exact and the last
    /// two case-insensitive — so a pane can ask `zo vault path 알파 Beta`.
    #[must_use]
    pub fn find_node(&self, name: &str) -> Option<u32> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let by_id = self.nodes.iter().position(|node| node.id == name);
        let below = format!("{WIKI_DIR}/{name}{MARKDOWN_SUFFIX}");
        let by_path = || self.nodes.iter().position(|node| node.id == below);
        let folded = name.to_lowercase();
        let by_stem = || {
            self.nodes.iter().position(|node| {
                node.id
                    .rsplit('/')
                    .next()
                    .and_then(|file| file.strip_suffix(MARKDOWN_SUFFIX))
                    .is_some_and(|stem| stem.to_lowercase() == folded)
            })
        };
        let by_title = || {
            self.nodes
                .iter()
                .position(|node| node.title.to_lowercase() == folded)
        };
        by_id
            .or_else(by_path)
            .or_else(by_stem)
            .or_else(by_title)
            .and_then(|at| u32::try_from(at).ok())
    }
}

/* ---- the answer by name ------------------------------------------------- */

/// One node as a reader names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedNode {
    pub id: String,
    pub title: String,
    pub kind: NodeKind,
}

/// One hop by page ids: the line as it was written (`from → to`), its kind
/// and road, and whether the path crossed it backwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedHop {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub provenance: EdgeProvenance,
    pub reversed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedPath {
    pub nodes: Vec<NamedNode>,
    pub hops: Vec<NamedHop>,
}

/// The answer the window and the `zo vault path` verb both carry: the two
/// endpoints as found, the paths by name, and how long the walk took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathReport {
    pub from: NamedNode,
    pub to: NamedNode,
    /// Paths asked for, after the table's clamp.
    pub k: usize,
    pub paths: Vec<NamedPath>,
    pub shortest: Option<u32>,
    pub capped: bool,
    pub limits: PathLimits,
    /// Microseconds [`VaultGraph::paths`] spent — the walk alone, never the
    /// scan; the number the report's p50/p95 are measured from.
    pub elapsed_us: u64,
}

/// Why a question could not be asked of this picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathRefusal {
    UnknownFrom(String),
    UnknownTo(String),
    SameNode(String),
}

impl fmt::Display for PathRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFrom(name) => write!(f, "no page named `{name}` (from)"),
            Self::UnknownTo(name) => write!(f, "no page named `{name}` (to)"),
            Self::SameNode(name) => write!(f, "`{name}` is both ends — a path needs two"),
        }
    }
}

impl std::error::Error for PathRefusal {}

fn named_node(graph: &VaultGraph, at: u32) -> NamedNode {
    let node = &graph.nodes[at as usize];
    NamedNode {
        id: node.id.clone(),
        title: node.title.clone(),
        kind: node.kind,
    }
}

/// The paths of `answer`, by name — one function so the window and the verb
/// spell a hop the same way.
#[must_use]
pub fn name_paths(graph: &VaultGraph, answer: &PathAnswer) -> Vec<NamedPath> {
    answer
        .paths
        .iter()
        .map(|path| NamedPath {
            nodes: path.nodes.iter().map(|at| named_node(graph, *at)).collect(),
            hops: path
                .hops
                .iter()
                .map(|hop| NamedHop {
                    from: graph.nodes[hop.edge.from as usize].id.clone(),
                    to: graph.nodes[hop.edge.to as usize].id.clone(),
                    kind: hop.edge.kind,
                    provenance: hop.edge.provenance,
                    reversed: hop.reversed,
                })
                .collect(),
        })
        .collect()
}

/// The question by name, answered by name: the endpoints are found the way
/// [`VaultGraph::find_node`] finds them, the walk is timed, and the answer
/// carries what a screen or a pane prints.
///
/// # Errors
///
/// An endpoint nothing answers to, or the same node twice.
pub fn report(
    graph: &VaultGraph,
    from: &str,
    to: &str,
    k: usize,
) -> Result<PathReport, PathRefusal> {
    let a = graph
        .find_node(from)
        .ok_or_else(|| PathRefusal::UnknownFrom(from.to_string()))?;
    let b = graph
        .find_node(to)
        .ok_or_else(|| PathRefusal::UnknownTo(to.to_string()))?;
    if a == b {
        return Err(PathRefusal::SameNode(graph.nodes[a as usize].id.clone()));
    }
    let began = Instant::now();
    let answer = graph.paths(a, b, k);
    let elapsed_us = u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX);
    Ok(PathReport {
        from: named_node(graph, a),
        to: named_node(graph, b),
        k: k.clamp(1, answer.limits.k_max),
        paths: name_paths(graph, &answer),
        shortest: answer.shortest,
        capped: answer.capped,
        limits: answer.limits,
        elapsed_us,
    })
}

/// One path as a line of text: titles joined by the hop's kind and road,
/// with the arrow pointing the way the line was written —
/// `알파 →(implements·declared) 베타 ←(mentions·inferred) 감마`.
#[must_use]
pub fn render_chain(path: &NamedPath) -> String {
    let mut out = String::new();
    for (at, node) in path.nodes.iter().enumerate() {
        if at > 0 {
            let hop = &path.hops[at - 1];
            let (open, close) = if hop.reversed {
                ("←(", ")")
            } else {
                ("→(", ")")
            };
            out.push(' ');
            out.push_str(open);
            out.push_str(hop.kind.as_str());
            out.push('·');
            out.push_str(hop.provenance.as_str());
            out.push_str(close);
            out.push(' ');
        }
        out.push_str(if node.title.is_empty() {
            &node.id
        } else {
            &node.title
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::second_brain_graph::GraphNode;

    fn page(id: &str, title: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            title: title.to_string(),
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

    fn line(from: u32, to: u32, kind: EdgeKind) -> GraphEdge {
        GraphEdge {
            from,
            to,
            kind,
            provenance: if kind == EdgeKind::Mentions {
                EdgeProvenance::Inferred
            } else {
                EdgeProvenance::Declared
            },
        }
    }

    fn graph(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> VaultGraph {
        VaultGraph {
            pages: nodes.len(),
            nodes,
            edges,
            ..VaultGraph::default()
        }
    }

    /// `a → b`, `a → c`, `b → d`, `c → d` (typed), and a longer `a → e → f → d`.
    fn diamond() -> VaultGraph {
        graph(
            vec![
                page("wiki/a.md", "A"),
                page("wiki/b.md", "B"),
                page("wiki/c.md", "C"),
                page("wiki/d.md", "D"),
                page("wiki/e.md", "E"),
                page("wiki/f.md", "F"),
            ],
            vec![
                line(0, 1, EdgeKind::Mentions),
                line(0, 2, EdgeKind::Mentions),
                line(1, 3, EdgeKind::Mentions),
                line(2, 3, EdgeKind::Implements),
                line(0, 4, EdgeKind::Mentions),
                line(4, 5, EdgeKind::Mentions),
                line(5, 3, EdgeKind::Mentions),
            ],
        )
    }

    fn walked(answer: &PathAnswer) -> Vec<Vec<u32>> {
        answer.paths.iter().map(|path| path.nodes.clone()).collect()
    }

    #[test]
    fn paths_come_shortest_first_then_in_node_order_and_stop_at_k() {
        let held = diamond();
        let answer = held.paths(0, 3, 5);
        assert_eq!(
            walked(&answer),
            [vec![0, 1, 3], vec![0, 2, 3], vec![0, 4, 5, 3]]
        );
        assert_eq!(answer.shortest, Some(2));
        assert!(!answer.capped);
        assert_eq!(answer.limits, PATH_LIMITS);
        for path in &answer.paths {
            assert_eq!(path.len(), path.nodes.len() - 1);
        }
        // Each hop carries the line and its road.
        let typed = &answer.paths[1].hops[1];
        assert_eq!(typed.edge.kind, EdgeKind::Implements);
        assert_eq!(typed.edge.provenance, EdgeProvenance::Declared);
        assert!(!typed.reversed);
        assert_eq!(walked(&held.paths(0, 3, 2)), [vec![0, 1, 3], vec![0, 2, 3]]);
        assert_eq!(walked(&held.paths(0, 3, 0)).len(), 1, "k is at least one");
        assert!(held.paths(0, 3, 1_000).paths.len() <= PATH_LIMITS.k_max);
    }

    #[test]
    fn the_walk_is_undirected_and_a_hop_says_when_it_crossed_against_the_arrow() {
        let held = diamond();
        let answer = held.paths(3, 0, 1);
        assert_eq!(walked(&answer), [vec![3, 1, 0]]);
        assert!(answer.paths[0].hops.iter().all(|hop| hop.reversed));
        let forward = held.paths(0, 3, 1);
        assert!(forward.paths[0].hops.iter().all(|hop| !hop.reversed));
    }

    #[test]
    fn a_hop_prefers_the_named_and_declared_line_between_the_same_pair() {
        let held = graph(
            vec![page("wiki/a.md", "A"), page("wiki/b.md", "B")],
            vec![
                line(0, 1, EdgeKind::Mentions),
                line(1, 0, EdgeKind::DependsOn),
            ],
        );
        let answer = held.paths(0, 1, 1);
        let hop = answer.paths[0].hops[0];
        assert_eq!(hop.edge.kind, EdgeKind::DependsOn);
        assert!(
            hop.reversed,
            "the named line runs b → a and was crossed backwards"
        );
    }

    #[test]
    fn the_home_and_the_journal_are_endpoints_at_most() {
        let held = graph(
            vec![
                page(WIKI_INDEX_FILE, "home"),
                page("wiki/a.md", "A"),
                page("wiki/b.md", "B"),
                page(WIKI_LOG_FILE, "log"),
            ],
            vec![
                line(0, 1, EdgeKind::Mentions),
                line(0, 2, EdgeKind::Mentions),
                line(3, 1, EdgeKind::Mentions),
                line(3, 2, EdgeKind::Mentions),
            ],
        );
        // a and b connect only through the home and the journal: no path.
        let none = held.paths(1, 2, 3);
        assert!(none.paths.is_empty());
        assert_eq!(none.shortest, None);
        // The home itself may be asked about.
        assert_eq!(walked(&held.paths(0, 1, 1)), [vec![0, 1]]);
    }

    #[test]
    fn a_bad_index_a_disconnected_pair_and_a_pair_too_far_apart_answer_nothing() {
        let held = diamond();
        assert!(held.paths(0, 99, 1).paths.is_empty());
        assert!(held.paths(2, 2, 1).paths.is_empty());
        let apart = graph(
            vec![page("wiki/a.md", "A"), page("wiki/b.md", "B")],
            Vec::new(),
        );
        assert_eq!(apart.paths(0, 1, 1).shortest, None);
        // Beyond the hop bound is "not connected" for a reader.
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for at in 0..=PATH_LIMITS.max_hops + 1 {
            nodes.push(page(&format!("wiki/n{at}.md"), &format!("N{at}")));
            if at > 0 {
                edges.push(line(at as u32 - 1, at as u32, EdgeKind::Mentions));
            }
        }
        let chain = graph(nodes, edges);
        let last = PATH_LIMITS.max_hops as u32 + 1;
        assert!(chain.paths(0, last, 1).paths.is_empty());
        assert_eq!(walked(&chain.paths(0, last - 1, 1)).len(), 1);
    }

    #[test]
    fn a_dense_picture_stops_at_the_expansion_budget_and_says_so() {
        // A complete graph on n pages has (n-2)!·… simple paths between two
        // of them; the walk must stop rather than list them.
        let n = 40u32;
        let nodes = (0..n)
            .map(|at| page(&format!("wiki/n{at}.md"), &format!("N{at}")))
            .collect();
        let mut edges = Vec::new();
        for from in 0..n {
            for to in from + 1..n {
                edges.push(line(from, to, EdgeKind::Mentions));
            }
        }
        let held = graph(nodes, edges);
        let answer = held.paths(0, n - 1, PATH_LIMITS.k_max);
        assert_eq!(answer.paths.len(), PATH_LIMITS.k_max);
        assert!(!answer.capped, "k is reached long before the budget");
        // Under a budget of a few steps the same question stops early,
        // keeps what it found, and says it was cut.
        let tight = PathLimits {
            expansions_max: 3,
            ..PATH_LIMITS
        };
        let cut = held.paths_within(0, n - 1, PATH_LIMITS.k_max, tight);
        assert!(cut.capped);
        assert!(cut.paths.len() < PATH_LIMITS.k_max);
        assert_eq!(cut.shortest, Some(1));
        assert_eq!(cut.limits, tight);
    }

    #[test]
    fn the_report_names_both_ends_every_hop_and_its_own_time_and_refuses_what_it_cannot_find() {
        let held = diamond();
        let answer = report(&held, "A", "wiki/d.md", 2).unwrap();
        assert_eq!(answer.from.id, "wiki/a.md");
        assert_eq!(answer.to.title, "D");
        assert_eq!(answer.k, 2);
        assert_eq!(answer.paths.len(), 2);
        assert_eq!(answer.shortest, Some(2));
        let typed = &answer.paths[1].hops[1];
        assert_eq!(
            typed,
            &NamedHop {
                from: "wiki/c.md".into(),
                to: "wiki/d.md".into(),
                kind: EdgeKind::Implements,
                provenance: EdgeProvenance::Declared,
                reversed: false,
            }
        );
        assert_eq!(
            render_chain(&answer.paths[1]),
            "A →(mentions·inferred) C →(implements·declared) D"
        );
        let back = report(&held, "d", "a", 1).unwrap();
        assert_eq!(
            render_chain(&back.paths[0]),
            "D ←(mentions·inferred) B ←(mentions·inferred) A"
        );
        assert_eq!(
            report(&held, "nothing", "a", 1),
            Err(PathRefusal::UnknownFrom("nothing".into()))
        );
        assert_eq!(
            report(&held, "a", "nothing", 1),
            Err(PathRefusal::UnknownTo("nothing".into()))
        );
        assert_eq!(
            report(&held, "a", "A", 1),
            Err(PathRefusal::SameNode("wiki/a.md".into()))
        );
        // The wire carries the enum names, and the hop count is the table's.
        let wire = serde_json::to_value(&answer).unwrap();
        assert_eq!(wire["paths"][1]["hops"][1]["provenance"], "declared");
        assert_eq!(wire["limits"]["max_hops"], PATH_LIMITS.max_hops);
    }

    /// The report's numbers: `cargo test --release -p zerocode-core --lib
    /// second_brain_paths::tests::paths_on_a_real_vault -- --ignored --nocapture`
    /// with `ZEROCODE_SECOND_BRAIN` naming a vault. Read-only; prints the
    /// per-road line counts and the walk's p50/p95 over page pairs picked
    /// by a fixed stride, k = 5.
    #[test]
    #[ignore = "reads the vault ZEROCODE_SECOND_BRAIN names; a measurement, not a gate"]
    fn paths_on_a_real_vault_report_p50_p95() {
        let Ok(root) = std::env::var(crate::second_brain::VAULT_ENV) else {
            return;
        };
        let graph =
            crate::second_brain_graph::GraphCache::new().scan(std::path::Path::new(&root), false);
        let pages: Vec<u32> = graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.kind == NodeKind::Page)
            .map(|(at, _)| at as u32)
            .collect();
        let mut samples: Vec<u128> = Vec::new();
        let mut found = 0usize;
        let mut asked = 0usize;
        let stride = pages.len() / 8 + 1;
        for (i, from) in pages.iter().step_by(stride.max(1)).enumerate() {
            for to in pages.iter().skip(i * 3 + 1).step_by(stride * 2 + 1).take(6) {
                let began = Instant::now();
                let answer = graph.paths(*from, *to, 5);
                samples.push(began.elapsed().as_micros());
                asked += 1;
                if !answer.paths.is_empty() {
                    found += 1;
                }
            }
        }
        samples.sort_unstable();
        let at = |q: f64| samples[((samples.len() as f64 - 1.0) * q).round() as usize];
        println!(
            "vault {root}: pages {} edges {} provenances {:?}",
            graph.pages,
            graph.edges.len(),
            graph
                .provenances
                .iter()
                .map(|row| (row.provenance.as_str(), row.count))
                .collect::<Vec<_>>()
        );
        println!(
            "paths k=5: asked {asked} connected {found} p50 {} µs p95 {} µs max {} µs",
            at(0.5),
            at(0.95),
            samples.last().copied().unwrap_or(0)
        );
    }

    #[test]
    fn a_node_is_found_by_id_path_stem_or_title_in_that_order() {
        let held = graph(
            vec![
                page("wiki/topics/Alpha.md", "알파"),
                page("wiki/Beta.md", "beta"),
                page("wiki/alpha.md", "Alpha the second"),
            ],
            Vec::new(),
        );
        assert_eq!(held.find_node("wiki/Beta.md"), Some(1));
        assert_eq!(held.find_node("Beta"), Some(1), "the path below wiki/");
        assert_eq!(
            held.find_node("alpha"),
            Some(2),
            "an exact path wins over a stem"
        );
        assert_eq!(
            held.find_node("ALPHA the SECOND"),
            Some(2),
            "a title, any case"
        );
        assert_eq!(held.find_node("알파"), Some(0));
        assert_eq!(held.find_node("topics/Alpha"), Some(0));
        assert_eq!(held.find_node(""), None);
        assert_eq!(held.find_node("nothing"), None);
    }
}
