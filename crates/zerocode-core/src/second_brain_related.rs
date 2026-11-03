//! The pages worth putting in front of an agent, as one bounded block.
//!
//! Every agent in this product gets the same effect from the same vault: a
//! prompt arrives, the graph answers with the handful of pages that prompt is
//! about, and the block goes in ahead of the turn. That is why the choosing
//! lives here rather than in one runtime — a Claude pane and a Codex pane
//! reading the same vault must be shown the same pages.
//!
//! A page is chosen by BM25 over its own name and first paragraph, so a word
//! few pages use counts for more than an ending most of them share, and a long
//! name does not win by holding more words. The words that only say how
//! something is asked are not searched for, a Korean spelling also finds the
//! Latin name a page uses, and a page the words could match by chance is not
//! shown. A turn nobody asked for — a background task's notice, the mail
//! pointer — asks nothing at all ([`recall_words`]).
//!
//! The block is bounded on every axis a prompt hook cares about:
//! [`MAX_RELATED_ENTRIES`] pages, [`MAX_RELATED_RELATIONS_PER_LINE`] relations
//! on a line, [`MAX_RELATED_LINE_BYTES`] per line and
//! [`MAX_RELATED_BLOCK_BYTES`] for the whole thing. Nothing here reads the
//! disk: the caller hands over a graph it already scanned, so a warm vault
//! costs one pass over its nodes and a few over its edges.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};
use std::sync::OnceLock;

use crate::second_brain::{WIKI_INDEX_FILE, WIKI_LOG_FILE};
use crate::second_brain_graph::{EdgeKind, GraphNode, NodeKind, VaultGraph};

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
pub const RELATED_FOOTER: &str =
    "답에 근거로 쓴 페이지는 [[링크]]로 적는다. 더 찾을 때는 `zo vault search <말>`.";

/// Weight a typed relation adds to a neighbour's score. A page that declares
/// `implements` said something about the relation; a body link only mentioned.
const TYPED_WEIGHT: f64 = 2.0;
const MENTIONS_WEIGHT: f64 = 1.0;

/// BM25's two constants at their usual values; nothing about this vault chose
/// them. A word counts once in a field, so `BM25_K1` only sets how hard
/// `BM25_B` leans on the field's length.
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// What a page's first paragraph counts for beside its name. Half, because a
/// paragraph is about the page but was not written to name it.
const EXCERPT_WEIGHT: f64 = 0.5;

/// The vault size from which a match can be told from chance. Below it nearly
/// every word is common and nearly every page is worth naming, so no page is
/// held back.
const MIN_PAGES_TO_JUDGE_CHANCE: u32 = 64;

/// Pieces of Korean that say how something is asked and never what about:
/// question words, endings, connectives and the bare verbs of a request. They
/// are rare in the vault, because a page states things rather than asks, so
/// rarity alone would count them as the most telling words of a prompt. Written
/// as [`tokenize`] cuts them. zo's recall reads the same file.
fn asking_pieces() -> &'static BTreeSet<&'static str> {
    static PIECES: OnceLock<BTreeSet<&'static str>> = OnceLock::new();
    PIECES.get_or_init(|| {
        include_str!("second_brain_words/asking_pieces.txt")
            .split_whitespace()
            .collect()
    })
}

/// The Korean spelling of a name the vault writes in Latin letters. A person
/// types 윈도우 and the pages say Windows, and no page says both, so the vault
/// cannot teach this pairing. zo's recall reads the same file.
fn latin_names() -> &'static [(&'static str, &'static str)] {
    static NAMES: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    NAMES.get_or_init(|| {
        include_str!("second_brain_words/latin_names.txt")
            .lines()
            .filter_map(|line| line.split_once(' '))
            .collect()
    })
}

/// What may follow such a spelling and leave it that name: `윈도우에서` is
/// Windows, `맥락` is not macOS. zo's recall reads the same file.
fn name_endings() -> &'static BTreeSet<&'static str> {
    static ENDINGS: OnceLock<BTreeSet<&'static str>> = OnceLock::new();
    ENDINGS.get_or_init(|| {
        include_str!("second_brain_words/name_endings.txt")
            .split_whitespace()
            .collect()
    })
}

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
    is_hangul(ch)
        || matches!(u32::from(ch),
            0x4E00..=0x9FFF      // CJK Unified Ideographs
            | 0x3400..=0x4DBF    // CJK Unified Ideographs Extension A
            | 0x3040..=0x309F    // Hiragana
            | 0x30A0..=0x30FF) // Katakana
}

/// Whether a character is Hangul — a syllable, a Jamo, or a compatibility
/// Jamo. One table for the tokenizer above and the Jev rows' language
/// column (`crate::jev::hangul_share_permille`).
pub(crate) fn is_hangul(ch: char) -> bool {
    matches!(u32::from(ch),
        0xAC00..=0xD7A3      // Hangul syllables
        | 0x1100..=0x11FF    // Hangul Jamo
        | 0x3130..=0x318F) // Hangul Compatibility Jamo
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
        // Evidence rather than pages to read: a raw file, and the project's
        // code the layer grafts beside the vault.
        NodeKind::Source | NodeKind::CodeFile | NodeKind::CodeSymbol => None,
    }
}

/// The vault's own pages — the home (`wiki/index.md`) and the journal
/// (`wiki/log.md`). They link everything and are about nothing, so they are
/// never a page the prompt is about, never reached as a neighbour, and never
/// shown as a relation: every slot they took was a slot a real page lost.
fn is_structural(node: &GraphNode) -> bool {
    node.id == WIKI_INDEX_FILE || node.id == WIKI_LOG_FILE
}

/// The words a prompt asks the vault with, or nothing when nobody asked.
///
/// Most of what reaches a prompt hook is not a question: a background task's
/// notice, the line this window types when mail waits, a nudge it types after a
/// restart. Those go first, then the plumbing around what is left
/// ([`crate::transcript::searchable_words`]). A worker's briefing is the same
/// words in every prompt, so only the task after it asks.
#[must_use]
pub fn recall_words(prompt: &str) -> Option<String> {
    let said = prompt
        .lines()
        .filter(|line| !crate::orchestration::is_pointer_text(line))
        .collect::<Vec<_>>()
        .join("\n");
    if crate::orchestration::is_window_note(said.trim_start()) {
        return None;
    }
    let words = crate::transcript::searchable_words(&said)?;
    let words = crate::jev::file_pick::task_words(&words);
    let words = words.trim();
    (!asked_tokens(words).is_empty()).then(|| words.to_string())
}

/// The tokens a prompt searches with: its own, without the pieces that only
/// say how it asks, and with the Latin name of any Korean spelling it holds.
/// `zo vault search` picks a page's best line by them.
#[must_use]
pub fn asked_tokens(words: &str) -> BTreeSet<String> {
    let mut tokens = tokenize(words);
    tokens.retain(|token| !asking_pieces().contains(token.as_str()));
    for run in words
        .split(|ch: char| !is_cjk(ch))
        .filter(|run| !run.is_empty())
    {
        for (spelling, name) in latin_names() {
            if run
                .strip_prefix(spelling)
                .is_some_and(|rest| rest.is_empty() || name_endings().contains(rest))
            {
                tokens.insert((*name).to_string());
            }
        }
    }
    tokens
}

/// One page the block will name.
struct Entry {
    at: u32,
    score: f64,
    slug: String,
}

/// Best first. An equal score goes to the page written last, because a name's
/// place in the alphabet says nothing about a prompt. The walk's order only
/// settles two pages written at the same moment, so the answer stays one answer.
fn ranked(graph: &VaultGraph, left: &Entry, right: &Entry) -> Ordering {
    let written = |entry: &Entry| {
        graph
            .nodes
            .get(entry.at as usize)
            .map_or(0, |node| node.modified_ms)
    };
    right
        .score
        .total_cmp(&left.score)
        .then_with(|| written(right).cmp(&written(left)))
        .then(left.at.cmp(&right.at))
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
    let tokens = asked_tokens(prompt);
    if tokens.is_empty() {
        return None;
    }

    let scores = page_scores(graph, &tokens);
    let mut seeds: Vec<Entry> = Vec::new();
    for (at, score) in scores.iter().enumerate() {
        if *score <= 0.0 {
            continue;
        }
        let Some(slug) = graph.nodes.get(at).and_then(slug_of) else {
            continue;
        };
        if !skip.contains(&slug) {
            seeds.push(Entry {
                at: index_of(at),
                score: *score,
                slug,
            });
        }
    }
    seeds.sort_by(|left, right| ranked(graph, left, right));
    seeds.truncate(MAX_RELATED_ENTRIES);
    if seeds.is_empty() {
        return None;
    }

    // The words fill the block first: a neighbour holds none of them, so it
    // takes only a line they left empty.
    let candidates = if seeds.len() < MAX_RELATED_ENTRIES {
        neighbours(graph, &seeds, skip)
    } else {
        Vec::new()
    };
    let entries = collect(seeds, candidates);
    if entries.is_empty() {
        return None;
    }
    Some(render(graph, &entries))
}

/// What one page holds of the words asked: each is an index into them.
struct Holding {
    at: usize,
    name_length: f64,
    excerpt_length: f64,
    in_name: Vec<usize>,
    in_excerpt: Vec<usize>,
    in_either: Vec<usize>,
}

/// How well each page answers the prompt, parallel to the nodes: BM25 over its
/// name, with its first paragraph at [`EXCERPT_WEIGHT`], each word counted once
/// in a field. A node the block never names, a page that holds none of the
/// words and a page chance would explain all score zero.
fn page_scores(graph: &VaultGraph, tokens: &BTreeSet<String>) -> Vec<f64> {
    let asked: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let mut name_holders = vec![0_u32; asked.len()];
    let mut excerpt_holders = vec![0_u32; asked.len()];
    let mut either_holders = vec![0_u32; asked.len()];
    let mut holdings: Vec<Holding> = Vec::new();
    let mut pages = 0_u32;
    let mut total_name_length = 0.0;
    let mut total_excerpt_length = 0.0;
    let held_of = |words: &BTreeSet<String>| -> Vec<usize> {
        words
            .iter()
            .filter_map(|word| asked.binary_search(&word.as_str()).ok())
            .collect()
    };
    for (at, node) in graph.nodes.iter().enumerate() {
        if node.kind != NodeKind::Page || is_structural(node) {
            continue;
        }
        let Some(slug) = slug_of(node) else {
            continue;
        };
        let name = name_words(node, &slug);
        let excerpt = tokenize(&node.excerpt);
        let name_length = length_of(&name);
        let excerpt_length = length_of(&excerpt);
        pages = pages.saturating_add(1);
        total_name_length += name_length;
        total_excerpt_length += excerpt_length;
        let in_name = held_of(&name);
        let in_excerpt = held_of(&excerpt);
        let mut in_either = in_name.clone();
        in_either.extend_from_slice(&in_excerpt);
        in_either.sort_unstable();
        in_either.dedup();
        for (holders, held) in [
            (&mut name_holders, &in_name),
            (&mut excerpt_holders, &in_excerpt),
            (&mut either_holders, &in_either),
        ] {
            for &word in held {
                holders[word] = holders[word].saturating_add(1);
            }
        }
        if !in_either.is_empty() {
            holdings.push(Holding {
                at,
                name_length,
                excerpt_length,
                in_name,
                in_excerpt,
                in_either,
            });
        }
    }

    let mut scores = vec![0.0; graph.nodes.len()];
    let vault = f64::from(pages.max(1));
    let average_name = (total_name_length / vault).max(1.0);
    let average_excerpt = (total_excerpt_length / vault).max(1.0);
    // How unlikely a page is to hold these words by chance must beat the number
    // of pages it could have happened in: past that, chance alone would match
    // fewer than one page.
    let chance_bar = if pages >= MIN_PAGES_TO_JUDGE_CHANCE {
        vault.ln()
    } else {
        0.0
    };
    for holding in holdings {
        let surprise: f64 = holding
            .in_either
            .iter()
            .map(|&word| (vault / f64::from(either_holders[word])).ln())
            .sum();
        if surprise < chance_bar {
            continue;
        }
        let name = field_score(
            &holding.in_name,
            &name_holders,
            pages,
            holding.name_length / average_name,
        );
        let excerpt = field_score(
            &holding.in_excerpt,
            &excerpt_holders,
            pages,
            holding.excerpt_length / average_excerpt,
        );
        scores[holding.at] = name + EXCERPT_WEIGHT * excerpt;
    }
    scores
}

fn length_of(words: &BTreeSet<String>) -> f64 {
    f64::from(u32::try_from(words.len()).unwrap_or(u32::MAX))
}

/// One field's BM25: the rarity of the words it holds, leaned on by how long
/// the field is against the average one.
fn field_score(held: &[usize], holders: &[u32], pages: u32, relative_length: f64) -> f64 {
    let rarity: f64 = held
        .iter()
        .map(|&word| rarity_of(pages, holders[word]))
        .sum();
    let stretch = 1.0 - BM25_B + BM25_B * relative_length;
    rarity * (BM25_K1 + 1.0) / (1.0 + BM25_K1 * stretch)
}

/// What one word is worth: more the fewer pages hold it, and never below zero,
/// so a word every page holds still counts for a little.
fn rarity_of(pages: u32, holding: u32) -> f64 {
    let pages = f64::from(pages);
    let holding = f64::from(holding);
    ((pages - holding + 0.5) / (holding + 0.5)).ln_1p()
}

/// The words a page answers to: its title, its tags and its file name.
fn name_words(node: &GraphNode, slug: &str) -> BTreeSet<String> {
    let stem = slug.rsplit('/').next().unwrap_or(slug);
    let mut text = String::with_capacity(node.title.len() + stem.len() + 16);
    text.push_str(&node.title);
    for tag in &node.tags {
        text.push(' ');
        text.push_str(tag);
    }
    text.push(' ');
    text.push_str(stem);
    tokenize(&text)
}

/// Pages one hop from a seed, in either direction, scored by how they were
/// reached. A page reachable from two seeds keeps its best reason. Asked only
/// when the words left room, so none of these holds a word of the prompt.
fn neighbours(graph: &VaultGraph, seeds: &[Entry], skip: &HashSet<String>) -> Vec<Entry> {
    let mut best: Vec<(u32, f64)> = Vec::new();
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
            let score = from.score + weight;
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
    held.sort_by(|left, right| ranked(graph, left, right));
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
    use crate::second_brain_graph::{EdgeProvenance, GraphEdge, GraphNode};

    fn page(id: &str, title: &str, tags: &[&str]) -> GraphNode {
        GraphNode {
            page_type: None,
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
            page_type: None,
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
                    provenance: if *kind == EdgeKind::Mentions {
                        EdgeProvenance::Inferred
                    } else {
                        EdgeProvenance::Declared
                    },
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
            page_type: None,
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

    fn written_at(mut node: GraphNode, modified_ms: i64) -> GraphNode {
        node.modified_ms = modified_ms;
        node
    }

    /// A word few pages use says what the prompt is about. An ending most
    /// pages share says nothing, however many of them one page holds: a
    /// prompt about a rare name was answered with pages that happened to
    /// share two of its grammar words.
    #[test]
    fn a_rare_word_outweighs_the_endings_most_pages_share() {
        let mut nodes: Vec<GraphNode> = (0..20)
            .map(|at| {
                page(
                    &format!("wiki/C{at:02}.md"),
                    "이 일은 대로 하고 끝낸다",
                    &[],
                )
            })
            .collect();
        nodes.push(page("wiki/Rare.md", "카파 정리", &[]));
        let held = graph(nodes, &[]);
        let block = related_block(&held, "카파 대로 하고", &nothing()).unwrap();
        assert_eq!(block.slugs[0], "wiki/Rare", "{:?}", block.slugs);
    }

    /// Two pages hold the one word asked. The one whose whole name is about
    /// it comes first, whichever the walk reached first: a page does not win
    /// by holding more words.
    #[test]
    fn a_long_name_does_not_win_by_holding_more_words() {
        let long = (0..40).map(|at| format!("word{at}")).collect::<Vec<_>>();
        let held = graph(
            vec![
                page("wiki/Long.md", &format!("parser {}", long.join(" ")), &[]),
                page("wiki/Short.md", "parser notes", &[]),
            ],
            &[],
        );
        let block = related_block(&held, "parser", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Short", "wiki/Long"]);
    }

    /// A name's place in the alphabet says nothing about a prompt. The page
    /// written last takes an equal score, and only a page written at the same
    /// moment falls back on the walk's order, so the answer stays one answer.
    #[test]
    fn a_tie_goes_to_the_newer_page_and_not_to_the_earlier_name() {
        let held = graph(
            vec![
                written_at(page("wiki/Alpha-note.md", "같은 제목", &[]), 100),
                written_at(page("wiki/Zeta-note.md", "같은 제목", &[]), 200),
            ],
            &[],
        );
        let block = related_block(&held, "note", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Zeta-note", "wiki/Alpha-note"]);

        let held = graph(
            vec![
                written_at(page("wiki/Alpha-note.md", "같은 제목", &[]), 100),
                written_at(page("wiki/Zeta-note.md", "같은 제목", &[]), 100),
            ],
            &[],
        );
        let block = related_block(&held, "note", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Alpha-note", "wiki/Zeta-note"]);
    }

    /// The words fill the block first. A neighbour named nothing the prompt
    /// said, so it only takes a line the words left empty.
    #[test]
    fn five_pages_the_words_found_come_before_any_neighbour() {
        let mut nodes: Vec<GraphNode> = (0..6)
            .map(|at| page(&format!("wiki/P{at}.md"), "alpha", &[]))
            .collect();
        nodes.push(page("wiki/Next.md", "이웃", &[]));
        let held = graph(nodes, &[(0, 6, EdgeKind::Implements)]);
        let block = related_block(&held, "alpha", &nothing()).unwrap();
        assert_eq!(block.slugs.len(), MAX_RELATED_ENTRIES);
        assert!(
            block.slugs.iter().all(|slug| slug != "wiki/Next"),
            "{:?}",
            block.slugs
        );
    }

    /// A page answers to its first paragraph too, for less than its name: a
    /// question can use a word the title never did.
    #[test]
    fn a_first_paragraph_finds_a_page_its_name_would_not_and_counts_for_less() {
        let mut in_paragraph = page("wiki/Paragraph.md", "다른 제목", &[]);
        in_paragraph.excerpt = "마우스를 뺏어 간다는 이야기".to_string();
        let held = graph(
            vec![in_paragraph, page("wiki/Named.md", "마우스 정리", &[])],
            &[],
        );
        let block = related_block(&held, "마우스", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Named", "wiki/Paragraph"]);
    }

    /// The words of asking are rare in a vault that states things, so rarity
    /// alone would call them the most telling words of the prompt. They are
    /// not searched for, and a prompt made of nothing else gets no block.
    #[test]
    fn the_words_of_asking_are_not_searched_for() {
        let held = graph(
            vec![
                page("wiki/Asking.md", "그렇게 어떻게 됐어", &[]),
                page("wiki/Topic.md", "볼트 정리", &[]),
            ],
            &[],
        );
        assert!(related_block(&held, "그렇게 어떻게 됐어", &nothing()).is_none());
        let block = related_block(&held, "볼트 어떻게 됐어", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Topic"]);
    }

    /// Every piece is written the way the tokenizer cuts a word, or it would
    /// never be taken out of a prompt, and every name is one a page could hold.
    #[test]
    fn the_word_lists_are_written_the_way_the_tokenizer_cuts() {
        assert!(asking_pieces().len() > 50);
        for piece in asking_pieces() {
            assert!(tokenize(piece).contains(*piece), "{piece}");
        }
        assert!(latin_names().len() > 10);
        for (spelling, name) in latin_names() {
            assert!(spelling.chars().all(is_cjk), "{spelling}");
            assert_eq!(tokenize(name).into_iter().collect::<Vec<_>>(), [*name]);
        }
        assert!(
            name_endings()
                .iter()
                .all(|ending| ending.chars().all(is_cjk))
        );
    }

    /// A person types the Korean spelling and the page says the Latin name.
    /// With an ending it is still that name; as the start of another word it
    /// is not.
    #[test]
    fn a_korean_spelling_finds_the_latin_name_a_page_uses() {
        let held = graph(
            vec![
                page("wiki/Windows-build.md", "Windows build", &[]),
                page("wiki/Macos-note.md", "macOS note", &[]),
            ],
            &[],
        );
        let block = related_block(&held, "윈도우에서", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Windows-build"]);
        let block = related_block(&held, "맥에서", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Macos-note"]);
        assert!(related_block(&held, "맥락", &nothing()).is_none());
    }

    /// In a vault large enough to tell, one common word on a page is what
    /// chance would give many pages. Nothing is shown for it; a word only that
    /// page holds is shown, and so are two ordinary words that meet on one page.
    #[test]
    fn a_match_chance_would_explain_is_not_shown() {
        let mut nodes: Vec<GraphNode> = (0..80)
            .map(|at| {
                let word = if at < 7 { "common" } else { "filler" };
                page(&format!("wiki/P{at:02}.md"), &format!("{word} note"), &[])
            })
            .collect();
        nodes.push(page("wiki/Unique.md", "singular", &[]));
        nodes.push(page("wiki/Pair.md", "common uncommon", &[]));
        nodes.push(page("wiki/Other.md", "uncommon", &[]));
        for at in 0..4 {
            nodes.push(page(&format!("wiki/U{at}.md"), "uncommon", &[]));
        }
        let held = graph(nodes, &[]);
        assert!(related_block(&held, "common", &nothing()).is_none());
        let block = related_block(&held, "singular", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Unique"]);
        let block = related_block(&held, "common uncommon", &nothing()).unwrap();
        assert_eq!(block.slugs, vec!["wiki/Pair"]);
    }

    /// What a CLI wraps around a turn, the lines this window types when mail
    /// waits and after a restart, and a command with nothing after it: nobody
    /// asked the vault anything.
    #[test]
    fn a_turn_nobody_asked_for_has_no_words_to_recall_with() {
        let two_pointers = format!(
            "{}{}",
            crate::orchestration::pointer_text(1),
            crate::orchestration::pointer_text(2)
        );
        let nudge = format!(
            "{} Worktree now: HEAD abc1234.",
            crate::orchestration::RESTART_NUDGE
        );
        let cut = format!("{} `cargo test`", crate::orchestration::CUT_COMMANDS_NUDGE);
        for said in [
            "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n</task-notification>",
            "<command-name>/effort</command-name>\n<command-args></command-args>",
            "<local-command-stdout>Set effort level</local-command-stdout>",
            crate::orchestration::pointer_text(1).as_str(),
            crate::orchestration::pointer_text(12).as_str(),
            two_pointers.as_str(),
            nudge.as_str(),
            cut.as_str(),
            crate::orchestration::COORDINATOR_NUDGE,
            "/effort",
            "계속",
            "시작",
            "그렇게 해줘",
            "   ",
        ] {
            assert_eq!(recall_words(said), None, "{said:?}");
        }
    }

    /// A prompt says what it says past the plumbing around it. The envelope
    /// and the frame go; the words stay, and they are what the vault is asked.
    /// A short topic stays too: 「보고」 and 「남은과업」 ask about something.
    #[test]
    fn a_prompt_asks_with_its_own_words_and_not_the_plumbing_around_them() {
        for (said, words) in [
            ("볼트 lint 왜 부풀어", "볼트 lint 왜 부풀어"),
            ("계속 진행하되 시험부터", "계속 진행하되 시험부터"),
            ("보고", "보고"),
            ("남은과업", "남은과업"),
            ("진행상황", "진행상황"),
            ("진행?", ""),
            ("R", "R"),
            ("next", "next"),
            (
                "<system-reminder>a note</system-reminder>\nfix the parser",
                "fix the parser",
            ),
            (
                "<pasted_content id=\"8a76\">the briefing</pasted_content id=\"8a76\">",
                "the briefing",
            ),
            ("<pasted_content id=\"8a76\">the briefing", "the briefing"),
            // A tag that never closes is one a person typed.
            (
                "<system-reminder> 이 태그 안 닫혀",
                "<system-reminder> 이 태그 안 닫혀",
            ),
            // A command's arguments are what its author wrote.
            ("/review 회상 블록", "회상 블록"),
            (
                "<command-name>/review</command-name>\n<command-args>회상 블록</command-args>",
                "회상 블록",
            ),
            ("/Users/dev/notes 이거 읽어", "/Users/dev/notes 이거 읽어"),
            // A pointer beside what somebody wrote leaves the words.
            (
                "You have 2 orchestration messages. Run `zerocode-orc check`.\nfix the parser",
                "fix the parser",
            ),
        ] {
            let want = (!words.is_empty()).then(|| words.to_string());
            assert_eq!(recall_words(said), want, "{said:?}");
        }
    }

    /// A worker's briefing is the same long words in every prompt. Its task
    /// comes after them, past the bound the hook reads to, and only the task
    /// asks the vault.
    #[test]
    fn a_workers_briefing_asks_with_its_task_and_not_its_preamble() {
        let briefing = crate::orchestration::worker_briefing("t-1", "a title", "");
        let said = format!(
            "<pasted_content id=\"1\">{briefing}\nfix the parser\n</pasted_content id=\"1\">"
        );
        assert!(said.len() > 4_096, "{} bytes", said.len());
        assert_eq!(recall_words(&said).as_deref(), Some("fix the parser"));
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
