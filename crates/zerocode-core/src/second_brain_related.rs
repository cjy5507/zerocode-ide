//! The pages worth putting in front of an agent, as one bounded block.
//!
//! Every agent in this product gets the same effect from the same vault: a
//! prompt arrives, the graph answers with the handful of pages that prompt is
//! about, and the block goes in ahead of the turn. That is why the choosing
//! lives here rather than in one runtime — a Claude pane and a Codex pane
//! reading the same vault must be shown the same pages.
//!
//! The block is bounded on every axis a prompt hook cares about:
//! [`MAX_RELATED_ENTRIES`] pages, [`MAX_RELATED_RELATIONS_PER_LINE`] relations
//! on a line, [`MAX_RELATED_LINE_BYTES`] per line and
//! [`MAX_RELATED_BLOCK_BYTES`] for the whole thing. Nothing here reads the
//! disk: the caller hands over a graph it already scanned, so a warm vault
//! costs one pass over its nodes and two over its edges.

use std::collections::{BTreeSet, HashSet};

use crate::second_brain::{WIKI_INDEX_FILE, WIKI_LOG_FILE};
use crate::second_brain_graph::{EdgeKind, GraphNode, NodeKind, VaultGraph};

/// Pages the prompt's own words may point at. More than a few seeds and the
/// block stops being about what was asked.
pub const MAX_RELATED_SEEDS: usize = 3;
/// Pages one block names. This is prepended to every prompt, so it is a
/// pointer list a person can read at a glance, not a search result page.
pub const MAX_RELATED_ENTRIES: usize = 5;
/// Relations shown per page. Enough to say how a page sits among its
/// neighbours; past that the line stops being scannable.
pub const MAX_RELATED_RELATIONS_PER_LINE: usize = 3;
/// One line's budget. A line longer than this wraps in every pane it is read
/// in, which costs more attention than the relations it carries are worth.
pub const MAX_RELATED_LINE_BYTES: usize = 240;
/// The whole block's budget — the tokens this feature spends on every single
/// turn, and the one number that keeps the cost predictable.
pub const MAX_RELATED_BLOCK_BYTES: usize = 1_536;
/// The heading an agent sees. Korean, like the vault protocol it belongs to.
pub const RELATED_HEADING: &str = "## 관련 지식 (second brain)";
/// What the block asks of the agent: knowledge is only worth surfacing if the
/// answer says which page it came from.
pub const RELATED_FOOTER: &str = "답에 근거로 쓴 페이지는 [[링크]]로 적는다.";

/// Weight a typed relation adds to a neighbour's score. A page that declares
/// `implements` said something about the relation; a body link only mentioned.
const TYPED_WEIGHT: u32 = 2;
const MENTIONS_WEIGHT: u32 = 1;

/// Said when the graph is a part of the vault rather than all of it, so an
/// agent does not read a short block as "the vault holds nothing else".
const CAPPED_NOTICE: &str = "(볼트 일부만 반영됨)";

const MARKDOWN_SUFFIX: &str = ".md";
const GHOST_PREFIX: &str = "ghost:";

/// The rendered block and the pages it named, in the order it named them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedBlock {
    pub text: String,
    pub slugs: Vec<String>,
}

/// Lower-case lexical tokens: Latin/digit characters group into
/// whitespace/punctuation-delimited runs, CJK characters into overlapping
/// bigrams. A script boundary always closes the current run, so `4-1트랙`
/// yields `{4, 1, 트랙}` rather than one opaque token.
///
/// This mirrors zo's own recall tokenizer (`memory::recall::tokenize`)
/// byte-for-byte in behaviour. It is duplicated rather than shared because zo
/// is a separate Cargo workspace — the same precedent as
/// `second_brain::corpus`, which duplicates this crate's frontmatter parsing.
/// Two vault features that score the same prompt must agree on what a word is,
/// and a copy of forty lines is a cheaper way to guarantee that than a
/// cross-workspace dependency.
#[must_use]
pub fn tokenize(text: &str) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    let mut ascii_run = String::new();
    let mut cjk_run: Vec<char> = Vec::new();

    for ch in text.chars().flat_map(char::to_lowercase) {
        if is_cjk(ch) {
            flush_ascii_run(&mut ascii_run, &mut tokens);
            cjk_run.push(ch);
        } else if ch.is_alphanumeric() {
            flush_cjk_run(&mut cjk_run, &mut tokens);
            ascii_run.push(ch);
        } else {
            flush_ascii_run(&mut ascii_run, &mut tokens);
            flush_cjk_run(&mut cjk_run, &mut tokens);
        }
    }
    flush_ascii_run(&mut ascii_run, &mut tokens);
    flush_cjk_run(&mut cjk_run, &mut tokens);
    tokens
}

/// Whether a character belongs to a space-less CJK script (Hangul, CJK
/// ideographs incl. Extension A, and kana). These scripts write a whole word
/// with no separators, so the alphanumeric-run tokenizer would collapse an
/// entire word into one token that never overlaps a query phrased even
/// slightly differently.
fn is_cjk(ch: char) -> bool {
    matches!(u32::from(ch),
        0xAC00..=0xD7A3      // Hangul syllables
        | 0x1100..=0x11FF    // Hangul Jamo
        | 0x3130..=0x318F    // Hangul Compatibility Jamo
        | 0x4E00..=0x9FFF    // CJK Unified Ideographs
        | 0x3400..=0x4DBF    // CJK Unified Ideographs Extension A
        | 0x3040..=0x309F    // Hiragana
        | 0x30A0..=0x30FF) // Katakana
}

fn flush_ascii_run(run: &mut String, tokens: &mut BTreeSet<String>) {
    if !run.is_empty() {
        tokens.insert(std::mem::take(run));
    }
}

/// Bigrams let `트랙4` and `1트랙` share the `트랙` token, and `진행상태` and
/// `진행해` share `진행`. A length-1 run emits the single character.
fn flush_cjk_run(run: &mut Vec<char>, tokens: &mut BTreeSet<String>) {
    match run.as_slice() {
        [] => {}
        [only] => {
            tokens.insert(only.to_string());
        }
        chars => {
            for pair in chars.windows(2) {
                if let [a, b] = pair {
                    let mut bigram = String::with_capacity(a.len_utf8() + b.len_utf8());
                    bigram.push(*a);
                    bigram.push(*b);
                    tokens.insert(bigram);
                }
            }
        }
    }
    run.clear();
}

/// What a node is called in a `[[link]]`: `wiki/sub/Concept.md` answers
/// `wiki/sub/Concept`, a ghost answers the target it was named by, and a
/// source answers nothing — a raw file is evidence, not a page to read.
#[must_use]
pub fn slug_of(node: &GraphNode) -> Option<String> {
    match node.kind {
        NodeKind::Page => Some(
            node.id
                .strip_suffix(MARKDOWN_SUFFIX)
                .unwrap_or(&node.id)
                .to_string(),
        ),
        NodeKind::Ghost => Some(node.id.strip_prefix(GHOST_PREFIX)?.to_string()),
        NodeKind::Source => None,
    }
}

/// The vault's own pages — the home (`wiki/index.md`) and the journal
/// (`wiki/log.md`). They link everything and are about nothing, so they are
/// never a page the prompt is about, never reached as a neighbour, and never
/// shown as a relation: every slot they took was a slot a real page lost.
fn is_structural(node: &GraphNode) -> bool {
    node.id == WIKI_INDEX_FILE || node.id == WIKI_LOG_FILE
}

/// One page the block will name.
struct Entry {
    at: u32,
    score: u32,
    slug: String,
}

/// The block for this prompt, or nothing when the vault has nothing to say.
///
/// `skip` holds slugs the caller already put in front of the agent this turn —
/// the same page named twice is noise, not emphasis.
#[must_use]
pub fn related_block(
    graph: &VaultGraph,
    prompt: &str,
    skip: &HashSet<String>,
) -> Option<RelatedBlock> {
    let tokens = tokenize(prompt);
    if tokens.is_empty() {
        return None;
    }

    // One pass over the nodes: a page scores by how much of the prompt its own
    // name, title and tags account for.
    let mut scores: Vec<u32> = vec![0; graph.nodes.len()];
    let mut seeds: Vec<Entry> = Vec::new();
    for (at, node) in graph.nodes.iter().enumerate() {
        if node.kind != NodeKind::Page || is_structural(node) {
            continue;
        }
        let Some(slug) = slug_of(node) else {
            continue;
        };
        let score = seed_score(&tokens, node, &slug);
        if score == 0 {
            continue;
        }
        let at = index_of(at);
        scores[at as usize] = score;
        if !skip.contains(&slug) {
            seeds.push(Entry { at, score, slug });
        }
    }
    // Score first, node order second: the answer must not depend on which page
    // the walk happened to read first.
    seeds.sort_by(|left, right| right.score.cmp(&left.score).then(left.at.cmp(&right.at)));
    seeds.truncate(MAX_RELATED_SEEDS);
    if seeds.is_empty() {
        return None;
    }

    let candidates = neighbours(graph, &seeds, &scores, skip);
    let entries = collect(seeds, candidates);
    if entries.is_empty() {
        return None;
    }
    Some(render(graph, &entries))
}

/// How many of the prompt's tokens this page's own words account for.
fn seed_score(tokens: &BTreeSet<String>, node: &GraphNode, slug: &str) -> u32 {
    let stem = slug.rsplit('/').next().unwrap_or(slug);
    let mut text = String::with_capacity(node.title.len() + stem.len() + 16);
    text.push_str(&node.title);
    for tag in &node.tags {
        text.push(' ');
        text.push_str(tag);
    }
    text.push(' ');
    text.push_str(stem);
    let held = tokenize(&text);
    u32::try_from(tokens.iter().filter(|token| held.contains(*token)).count()).unwrap_or(u32::MAX)
}

/// Pages one hop from a seed, in either direction, scored by how they were
/// reached. A page reachable from two seeds keeps its best reason.
fn neighbours(
    graph: &VaultGraph,
    seeds: &[Entry],
    scores: &[u32],
    skip: &HashSet<String>,
) -> Vec<Entry> {
    let mut best: Vec<(u32, u32)> = Vec::new();
    for edge in &graph.edges {
        let weight = match edge.kind {
            EdgeKind::Mentions => MENTIONS_WEIGHT,
            _ => TYPED_WEIGHT,
        };
        for (seed, other) in [(edge.from, edge.to), (edge.to, edge.from)] {
            let Some(from) = seeds.iter().find(|held| held.at == seed) else {
                continue;
            };
            if seeds.iter().any(|held| held.at == other) {
                continue;
            }
            let score = from.score + weight + scores.get(other as usize).copied().unwrap_or(0);
            match best.iter_mut().find(|(at, _)| *at == other) {
                Some(held) => held.1 = held.1.max(score),
                None => best.push((other, score)),
            }
        }
    }

    let mut held: Vec<Entry> = best
        .into_iter()
        .filter_map(|(at, score)| {
            let node = graph.nodes.get(at as usize)?;
            // Ghosts and sources are not pages an agent can read; the home
            // and the journal are not pages a prompt is about.
            if node.kind != NodeKind::Page || is_structural(node) {
                return None;
            }
            let slug = slug_of(node)?;
            (!skip.contains(&slug)).then_some(Entry { at, score, slug })
        })
        .collect();
    held.sort_by(|left, right| right.score.cmp(&left.score).then(left.at.cmp(&right.at)));
    held
}

/// Seeds first — the pages the prompt named — then what they reach.
fn collect(seeds: Vec<Entry>, candidates: Vec<Entry>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::with_capacity(MAX_RELATED_ENTRIES);
    for entry in seeds.into_iter().chain(candidates) {
        if entries.len() >= MAX_RELATED_ENTRIES {
            break;
        }
        if !entries.iter().any(|held| held.at == entry.at) {
            entries.push(entry);
        }
    }
    entries
}

fn render(graph: &VaultGraph, entries: &[Entry]) -> RelatedBlock {
    let mut lines: Vec<String> = Vec::with_capacity(entries.len());
    let mut slugs: Vec<String> = Vec::with_capacity(entries.len());
    let mut used = RELATED_HEADING.len() + RELATED_FOOTER.len() + 1;
    if graph.capped {
        used += CAPPED_NOTICE.len() + 1;
    }
    for entry in entries {
        let line = line_for(graph, entry);
        // Trailing entries go rather than a cut line: half a slug is a link to
        // nothing, and an agent would follow it.
        if used + line.len() + 1 > MAX_RELATED_BLOCK_BYTES {
            break;
        }
        used += line.len() + 1;
        lines.push(line);
        slugs.push(entry.slug.clone());
    }

    let mut text = String::with_capacity(used);
    text.push_str(RELATED_HEADING);
    for line in &lines {
        text.push('\n');
        text.push_str(line);
    }
    if graph.capped {
        text.push('\n');
        text.push_str(CAPPED_NOTICE);
    }
    text.push('\n');
    text.push_str(RELATED_FOOTER);
    RelatedBlock { text, slugs }
}

/// `- [[slug]] — title`, then the relations that still fit.
fn line_for(graph: &VaultGraph, entry: &Entry) -> String {
    let title = graph
        .nodes
        .get(entry.at as usize)
        .map_or("", |node| node.title.as_str());
    let head = format!("- [[{}]] — ", entry.slug);
    let mut line = head.clone();
    if head.len() + title.len() > MAX_RELATED_LINE_BYTES {
        // The one thing this renderer may cut: a title carries no link, so a
        // reader loses words rather than a destination.
        line.push_str(&clipped(
            title,
            MAX_RELATED_LINE_BYTES.saturating_sub(head.len()),
        ));
    } else {
        line.push_str(title);
    }
    for suffix in suffixes(graph, entry.at) {
        if line.len() + suffix.len() > MAX_RELATED_LINE_BYTES {
            break;
        }
        line.push_str(&suffix);
    }
    line
}

/// `text` cut to fit `budget` bytes with an ellipsis, on a character boundary.
fn clipped(text: &str, budget: usize) -> String {
    const ELLIPSIS: &str = "…";
    let Some(room) = budget.checked_sub(ELLIPSIS.len()) else {
        return String::new();
    };
    let mut end = room.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELLIPSIS}", &text[..end])
}

/// The relations one page shows, most telling first: what it declared, then
/// what was declared about it, then the plain links either way. A relation
/// both pages declared is one relation, shown as this page's own; the home
/// and the journal are not shown at all.
fn suffixes(graph: &VaultGraph, at: u32) -> Vec<String> {
    // (rank, incoming, kind, slug) — rank orders, the rest renders.
    let mut found: Vec<(u8, bool, EdgeKind, String)> = Vec::new();
    for edge in &graph.edges {
        let (incoming, other) = if edge.from == at {
            (false, edge.to)
        } else if edge.to == at {
            (true, edge.from)
        } else {
            continue;
        };
        let Some(node) = graph.nodes.get(other as usize) else {
            continue;
        };
        if is_structural(node) {
            continue;
        }
        let Some(slug) = slug_of(node) else {
            continue;
        };
        if let Some(held) = found
            .iter_mut()
            .find(|(_, _, kind, held)| *kind == edge.kind && *held == slug)
        {
            // Seen from the other end already: keep the page's own statement.
            if held.1 && !incoming {
                held.0 = rank_of(edge.kind, incoming);
                held.1 = false;
            }
            continue;
        }
        found.push((rank_of(edge.kind, incoming), incoming, edge.kind, slug));
    }
    // Stable: within a rank, edge order (which is sorted) decides.
    found.sort_by_key(|(rank, ..)| *rank);
    found.truncate(MAX_RELATED_RELATIONS_PER_LINE);
    found
        .into_iter()
        .map(|(_, incoming, kind, slug)| {
            let arrow = if incoming { "← " } else { "" };
            format!(" · {arrow}{}: [[{slug}]]", kind.as_str())
        })
        .collect()
}

/// Declared by the page, declared about it, merely linked.
fn rank_of(kind: EdgeKind, incoming: bool) -> u8 {
    match (kind, incoming) {
        (EdgeKind::Mentions, _) => 2,
        (_, false) => 0,
        (_, true) => 1,
    }
}

/// The bounds in `second_brain_graph` keep a graph far below `u32::MAX` nodes;
/// saturating is the answer that cannot forge an index into another node.
fn index_of(at: usize) -> u32 {
    u32::try_from(at).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::second_brain_graph::{GraphEdge, GraphNode};

    fn page(id: &str, title: &str, tags: &[&str]) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            title: title.to_string(),
            tags: tags.iter().map(|held| (*held).to_string()).collect(),
            kind: NodeKind::Page,
            modified_ms: 0,
            out_links: 0,
            in_links: 0,
            source: None,
            excerpt: String::new(),
            folder: String::new(),
        }
    }

    fn ghost(target: &str) -> GraphNode {
        GraphNode {
            id: format!("ghost:{target}"),
            title: target.to_string(),
            kind: NodeKind::Ghost,
            ..page("", target, &[])
        }
    }

    fn graph(nodes: Vec<GraphNode>, edges: &[(u32, u32, EdgeKind)]) -> VaultGraph {
        VaultGraph {
            pages: nodes.iter().filter(|h| h.kind == NodeKind::Page).count(),
            nodes,
            edges: edges
                .iter()
                .map(|(from, to, kind)| GraphEdge {
                    from: *from,
                    to: *to,
                    kind: *kind,
                })
                .collect(),
            ..VaultGraph::default()
        }
    }

    fn nothing() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn the_tokenizer_answers_what_zos_recall_answers() {
        let held = tokenize("Hook Bridge 훅 브리지 v2");
        let want: BTreeSet<String> = ["hook", "bridge", "v2", "훅", "브리", "리지"]
            .iter()
            .map(|held| (*held).to_string())
            .collect();
        assert_eq!(held, want);
        // A script boundary closes the run rather than fusing the two.
        assert_eq!(
            tokenize("4-1트랙"),
            ["1", "4", "트랙"]
                .iter()
                .map(|held| (*held).to_string())
                .collect::<BTreeSet<_>>()
        );
        assert!(tokenize("   ,.  ").is_empty());
    }

    #[test]
    fn a_slug_is_what_a_link_would_have_written() {
        assert_eq!(
            slug_of(&page("wiki/sub/Concept.md", "개념", &[])).as_deref(),
            Some("wiki/sub/Concept")
        );
        assert_eq!(slug_of(&ghost("wiki/미래")).as_deref(), Some("wiki/미래"));
        let source = GraphNode {
            kind: NodeKind::Source,
            ..page("raw/paper.pdf", "paper.pdf", &[])
        };
        assert_eq!(slug_of(&source), None);
    }

    #[test]
    fn a_prompt_that_names_nothing_gets_no_block() {
        let held = graph(vec![page("wiki/Alpha.md", "알파", &[])], &[]);
        assert!(related_block(&held, "", &nothing()).is_none());
        assert!(related_block(&held, "quantum chromodynamics", &nothing()).is_none());
    }

    #[test]
    fn the_page_the_prompt_named_comes_first_and_its_neighbour_follows() {
        let held = graph(
            vec![
                page("wiki/Hook.md", "Hook Bridge", &["runtime"]),
                page("wiki/Bridge.md", "브리지 설계", &[]),
                page("wiki/Unrelated.md", "관계 없음", &[]),
            ],
            &[(0, 1, EdgeKind::Implements)],
        );
        let block = related_block(&held, "hook 브리지 어떻게 붙였지", &nothing()).unwrap();
        // Bridge scores higher: the prompt's `브리`/`리지` bigrams are both its
        // own, while Hook only answers `hook`. Score decides, not node order.
        assert_eq!(block.slugs, vec!["wiki/Bridge", "wiki/Hook"]);
        assert!(block.text.starts_with(RELATED_HEADING), "{}", block.text);
        assert!(block.text.ends_with(RELATED_FOOTER), "{}", block.text);
        assert!(
            block
                .text
                .contains("- [[wiki/Hook]] — Hook Bridge · implements: [[wiki/Bridge]]"),
            "{}",
            block.text
        );
        // The neighbour says the same relation from the other end.
        assert!(
            block
                .text
                .contains("- [[wiki/Bridge]] — 브리지 설계 · ← implements: [[wiki/Hook]]"),
            "{}",
            block.text
        );
    }

    /// The home and the journal link every page and are about none; a block
    /// that names them spends its lines on nothing, and their links are the
    /// loudest relation of every page for the same reason.
    #[test]
    fn the_home_and_the_journal_are_never_named_as_pages_or_relations() {
        let held = graph(
            vec![
                page("wiki/index.md", "index", &[]),
                page("wiki/log.md", "log", &[]),
                page("wiki/Hook.md", "Hook Bridge", &[]),
                page("wiki/Bridge.md", "브리지 설계", &[]),
            ],
            &[
                (0, 2, EdgeKind::Mentions),
                (0, 3, EdgeKind::Mentions),
                (1, 2, EdgeKind::Mentions),
                (2, 3, EdgeKind::Implements),
            ],
        );
        let block = related_block(&held, "hook 브리지", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Bridge", "wiki/Hook"]);
        assert!(
            !block.text.contains("wiki/index") && !block.text.contains("wiki/log"),
            "{}",
            block.text
        );
        assert!(
            related_block(&held, "index log", &nothing()).is_none(),
            "a prompt that names only the home is a prompt the vault has nothing for"
        );
    }

    /// `related` written on both pages is one relation. It is shown once, as
    /// the page's own statement, whichever end the edge list reached first.
    #[test]
    fn a_relation_declared_from_both_ends_is_shown_once_as_the_pages_own() {
        let held = graph(
            vec![
                page("wiki/Hook.md", "Hook Bridge", &[]),
                page("wiki/Bridge.md", "브리지 설계", &[]),
            ],
            &[(0, 1, EdgeKind::Related), (1, 0, EdgeKind::Related)],
        );
        let block = related_block(&held, "hook", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Hook", "wiki/Bridge"]);
        assert!(
            block
                .text
                .contains("- [[wiki/Hook]] — Hook Bridge · related: [[wiki/Bridge]]\n"),
            "{}",
            block.text
        );
        assert!(
            block
                .text
                .contains("- [[wiki/Bridge]] — 브리지 설계 · related: [[wiki/Hook]]\n"),
            "{}",
            block.text
        );
        assert!(!block.text.contains("← related"), "{}", block.text);
    }

    #[test]
    fn an_ascii_prompt_reaches_a_neighbour_over_an_incoming_edge() {
        let held = graph(
            vec![
                page("wiki/Caller.md", "Caller", &[]),
                page("wiki/Parser.md", "Parser", &[]),
            ],
            &[(0, 1, EdgeKind::Mentions)],
        );
        let block = related_block(&held, "parser bug", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Parser", "wiki/Caller"]);
    }

    #[test]
    fn a_ghost_is_a_relation_to_show_and_never_a_page_to_read() {
        let held = graph(
            vec![page("wiki/Alpha.md", "Alpha", &[]), ghost("wiki/미래")],
            &[(0, 1, EdgeKind::DependsOn)],
        );
        let block = related_block(&held, "alpha", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Alpha"]);
        assert!(
            block.text.contains("· depends_on: [[wiki/미래]]"),
            "{}",
            block.text
        );
    }

    #[test]
    fn a_page_the_caller_already_showed_is_left_out_of_both_roles() {
        let nodes = vec![
            page("wiki/Alpha.md", "Alpha", &[]),
            page("wiki/Beta.md", "Beta", &[]),
        ];
        let held = graph(nodes, &[(0, 1, EdgeKind::Related)]);
        let skip: HashSet<String> = ["wiki/Alpha".to_string()].into_iter().collect();
        // Skipped as a seed, and the prompt's other word still finds Beta.
        let block = related_block(&held, "alpha beta", &skip).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Beta"]);
        // Skipped as a neighbour too.
        let skip: HashSet<String> = ["wiki/Beta".to_string()].into_iter().collect();
        let block = related_block(&held, "alpha", &skip).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Alpha"]);
    }

    #[test]
    fn no_more_entries_than_the_bound_and_seeds_stay_ahead() {
        let mut nodes = vec![page("wiki/Core.md", "Core", &[])];
        let mut edges = Vec::new();
        for at in 1..=10u32 {
            nodes.push(page(&format!("wiki/N{at}.md"), &format!("N{at}"), &[]));
            edges.push((0, at, EdgeKind::Mentions));
        }
        let held = graph(nodes, &edges);
        let block = related_block(&held, "core", &nothing()).unwrap();
        assert_eq!(block.slugs.len(), MAX_RELATED_ENTRIES);
        assert_eq!(block.slugs[0], "wiki/Core");
    }

    #[test]
    fn a_line_keeps_its_budget_by_dropping_whole_relations() {
        let mut nodes = vec![page("wiki/Hub.md", "Hub", &[])];
        let mut edges = Vec::new();
        for at in 1..=6u32 {
            let long = "칠판".repeat(20);
            nodes.push(page(&format!("wiki/T{at}-{long}.md"), "T", &[]));
            edges.push((0, at, EdgeKind::Related));
        }
        let held = graph(nodes, &edges);
        let block = related_block(&held, "hub", &nothing()).unwrap();
        for line in block.text.lines() {
            // Every `[[` on a line still has its `]]`: suffixes went whole.
            assert_eq!(
                line.matches("[[").count(),
                line.matches("]]").count(),
                "{line}"
            );
        }
        // One long relation fits on the hub's line; the rest were dropped
        // rather than cutting the line at the bound.
        let hub = block
            .text
            .lines()
            .find(|line| line.starts_with("- [[wiki/Hub]]"))
            .unwrap();
        assert!(hub.len() <= MAX_RELATED_LINE_BYTES, "{} bytes", hub.len());
        assert_eq!(hub.matches(" · ").count(), 1, "{hub}");
    }

    #[test]
    fn a_title_longer_than_a_line_is_the_one_thing_that_gets_cut() {
        let long = "긴 제목 ".repeat(60);
        let held = graph(vec![page("wiki/Long.md", &long, &[])], &[]);
        let block = related_block(&held, "long", &nothing()).unwrap();
        let line = block.text.lines().nth(1).unwrap();
        assert!(line.len() <= MAX_RELATED_LINE_BYTES, "{} bytes", line.len());
        assert!(line.ends_with('…'), "{line}");
        assert!(line.starts_with("- [[wiki/Long]] — "), "{line}");
    }

    #[test]
    fn the_whole_block_keeps_its_budget_by_dropping_trailing_pages() {
        // Long slugs rather than long titles: a title is clipped on its own
        // line, so only a line the renderer may not cut can reach the block
        // bound.
        let long = "칠판".repeat(80);
        let mut nodes = vec![page("wiki/Core.md", "Core", &[])];
        let mut edges = Vec::new();
        for at in 1..=5u32 {
            nodes.push(page(&format!("wiki/L{at}-{long}.md"), "T", &[]));
            edges.push((0, at, EdgeKind::Related));
        }
        let held = graph(nodes, &edges);
        let block = related_block(&held, "core", &nothing()).unwrap();
        assert!(
            block.text.len() <= MAX_RELATED_BLOCK_BYTES,
            "{} bytes",
            block.text.len()
        );
        assert!(block.slugs.len() < MAX_RELATED_ENTRIES);
        assert_eq!(block.slugs.len(), block.text.lines().count() - 2);
    }

    #[test]
    fn a_partial_scan_says_so_above_the_footer() {
        let mut held = graph(vec![page("wiki/Alpha.md", "Alpha", &[])], &[]);
        held.capped = true;
        let block = related_block(&held, "alpha", &nothing()).unwrap();
        let lines: Vec<&str> = block.text.lines().collect();
        assert_eq!(lines[lines.len() - 2], CAPPED_NOTICE);
        assert_eq!(lines[lines.len() - 1], RELATED_FOOTER);
    }
}
