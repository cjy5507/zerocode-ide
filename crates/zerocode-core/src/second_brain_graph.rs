//! The vault's `wiki/` as a graph: pages are nodes, `[[wikilinks]]` are edges.
//!
//! Obsidian's own graph view reads the same two facts out of the same folder,
//! so this scanner answers with what a person already recognises — a page, its
//! title, its tags, and the pages it actually points at. A link whose target
//! does not exist stays in the answer as a *ghost*, because "the page I keep
//! linking to and never wrote" is the one thing a graph can say that a folder
//! listing cannot.
//!
//! Every bound is a constant in this file and nowhere else: [`MAX_GRAPH_PAGES`]
//! caps how many pages a graph holds, [`MAX_PAGE_BYTES`] how much of one page
//! is read, [`MAX_PAGE_LINKS`] how many links one page contributes,
//! [`MAX_GRAPH_GHOSTS`] and [`MAX_GRAPH_EDGES`] how far a runaway file can grow
//! the picture. The scan is also incremental: [`GraphCache`] holds each page's
//! parsed facts against the modification time and length it read them at, so
//! reopening the view re-reads only what changed on disk. Bodies are never
//! kept — the facts are.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::second_brain::{RAW_DIR, WIKI_DIR};
use crate::second_brain_lint::{self, PageProvenance, VaultLint};
use crate::second_brain_live;

/// Pages one graph can hold. Past this the scan stops and says so.
pub const MAX_GRAPH_PAGES: usize = 5_000;
/// How much of one page is read. A page longer than this is parsed up to the
/// bound and reported as truncated rather than silently half-linked.
pub const MAX_PAGE_BYTES: usize = 256 * 1024;
/// Links one page contributes. A generated index linking a thousand pages is a
/// real page; one linking a hundred thousand is a runaway file.
pub const MAX_PAGE_LINKS: usize = 2_000;
/// Ghost nodes the picture carries. Ghosts come from text rather than from
/// disk, so they are the one part of this graph a single bad file could grow
/// without limit.
pub const MAX_GRAPH_GHOSTS: usize = 5_000;
/// Relations the picture carries.
pub const MAX_GRAPH_EDGES: usize = 100_000;
/// Directory entries the walk may visit before it gives up on being complete.
pub const MAX_GRAPH_ENTRIES: usize = 50_000;
/// Frontmatter lines read. The block is a header, not a document.
pub const MAX_FRONTMATTER_LINES: usize = 200;
/// Tags the catalog carries. The chips are a filter, not an inventory.
pub const MAX_GRAPH_TAGS: usize = 64;
/// Characters carried for a page summary in the graph payload.
pub const MAX_EXCERPT_CHARS: usize = 160;

/// Directory names the walk never enters: an application's own private state
/// and its wastebasket are not knowledge.
const SKIPPED_DIRS: [&str; 3] = [".obsidian", ".trash", ".git"];

pub(crate) const MARKDOWN_SUFFIX: &str = ".md";
/// A title lives in the head: the frontmatter of a wiki page is a few hundred
/// bytes, and reading one page whole for its title would make a twelve-title
/// answer cost twelve page bodies.
const TITLE_HEAD_BYTES: usize = 4096;

/// What one relation means. The variant order is the sort order and the
/// tie-break everywhere in this file, so a graph reads the same twice.
///
/// Every name but [`EdgeKind::Mentions`] is also the frontmatter key that
/// writes it: a page says `implements: [[wiki/adr/003]]` and gets an
/// `implements` edge. `mentions` is what a body `[[link]]` means and is never
/// spelled as a key — a page mentions another by linking to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Mentions,
    Related,
    Implements,
    DependsOn,
    Supersedes,
    Contradicts,
}

impl EdgeKind {
    /// The wire spelling, matching the serde rename and the frontmatter key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mentions => "mentions",
            Self::Related => "related",
            Self::Implements => "implements",
            Self::DependsOn => "depends_on",
            Self::Supersedes => "supersedes",
            Self::Contradicts => "contradicts",
        }
    }

    /// The kind a frontmatter key names, or nothing. `mentions` answers
    /// nothing on purpose: it is the body's own relation, not a key a page may
    /// declare.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "related" => Some(Self::Related),
            "implements" => Some(Self::Implements),
            "depends_on" => Some(Self::DependsOn),
            "supersedes" => Some(Self::Supersedes),
            "contradicts" => Some(Self::Contradicts),
            _ => None,
        }
    }

    /// Every typed kind, in enum order — the keys [`parse_page`] looks for.
    const TYPED: [Self; 5] = [
        Self::Related,
        Self::Implements,
        Self::DependsOn,
        Self::Supersedes,
        Self::Contradicts,
    ];
}

/// Where one relation's evidence comes from (t-5966, G1).
///
/// Graphify tags every edge EXTRACTED or INFERRED; this graph says which of
/// three roads wrote a line, because a reader — the lens, the path answer, a
/// Jev re-ranking, an exported picture — asks different things of a line a
/// machine measured and a line a person typed. The variant order is the sort
/// order everywhere a provenance is listed.
///
/// The wire spelling is the serde name; a source contract asks for the enum's
/// names rather than the strings, so the window never spells a road itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeProvenance {
    /// A machine measured it — a recall trace, a session, a lockfile, a
    /// dedupe pass over titles. Nobody wrote it in the vault.
    Measured,
    /// A frontmatter key declared it: `implements:`, `depends_on:`,
    /// `source:` … — a person or an agent stated the relation by name.
    Declared,
    /// Prose inferred it: a `[[link]]` in a body says the pages are about
    /// each other without saying how.
    Inferred,
}

impl EdgeProvenance {
    /// Every provenance, in enum order — the rows a lens and a report list.
    pub const ALL: [Self; 3] = [Self::Measured, Self::Declared, Self::Inferred];

    /// The wire spelling, matching the serde rename.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Declared => "declared",
            Self::Inferred => "inferred",
        }
    }
}

/// Whether a scanned relation's provenance is one a road of this scanner can
/// vouch for — the rule the lint's 「근거 없는 간선」 row counts against, in one
/// place, so the table and the scanner cannot drift.
///
/// Prose writes no key, so an inferred line is always a `mentions`; a key
/// writes a typed relation, or — as `source:` — a `mentions` line to a source
/// node; and the scanner measures nothing, so a measured line in the scanned
/// picture has no road behind it (measured lines ride beside the graph:
/// [`crate::second_brain_live::MergeCandidate`], the supply chain) — except
/// the code layer's, which the graft writes and names
/// ([`crate::second_brain_code::grafted_line`]).
#[must_use]
pub fn vouched(edge: &GraphEdge, target: NodeKind) -> bool {
    match edge.provenance {
        EdgeProvenance::Inferred => edge.kind == EdgeKind::Mentions && target != NodeKind::Source,
        EdgeProvenance::Declared => {
            if target == NodeKind::Source {
                edge.kind == EdgeKind::Mentions
            } else {
                edge.kind != EdgeKind::Mentions
            }
        }
        EdgeProvenance::Measured => crate::second_brain_code::grafted_line(edge.kind, target),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A Markdown page under `wiki/`.
    Page,
    /// A link target no page exists for.
    Ghost,
    /// A raw source a page's `source:` frontmatter names. Never walked and
    /// never stat'ed — the page's own words are the evidence it exists.
    Source,
    /// A file of the active project's code that a page names — never scanned
    /// here: grafted from the codegraph index's answer
    /// ([`crate::second_brain_code::graft`], t-5970).
    CodeFile,
    /// A definition in that code a page names by its symbol.
    CodeSymbol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNode {
    /// Vault-relative path with `/` separators, or `ghost:<target>` for a
    /// target nothing on disk answers to.
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub kind: NodeKind,
    /// Milliseconds since the epoch, or 0 for a node that is not a file.
    pub modified_ms: i64,
    pub out_links: u32,
    /// Counted per edge rather than per neighbour: a page that both mentions
    /// and `implements` the same target contributes two.
    pub in_links: u32,
    /// The `source:` frontmatter value, verbatim, when a page carries one.
    pub source: Option<String>,
    /// First readable body paragraph, with Markdown decoration removed.
    pub excerpt: String,
    /// First directory below `wiki/`, or empty for a page at its root.
    pub folder: String,
}

/// One relation, as a pair of indices into [`VaultGraph::nodes`] and what the
/// relation means.
///
/// Indices rather than ids: the webview turns these straight into a typed
/// array, and a graph of five thousand pages would otherwise ship every path
/// twice over. An edge's identity is the whole triple, so the same pair may
/// appear once per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: u32,
    pub to: u32,
    pub kind: EdgeKind,
    /// Which road wrote the line (t-5966). Part of what the window draws and
    /// the lens filters on, never part of the edge's identity: one pair and
    /// one kind is one line, and the first road to write it names it.
    pub provenance: EdgeProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindCount {
    pub kind: EdgeKind,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceCount {
    pub provenance: EdgeProvenance,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagCount {
    pub tag: String,
    pub count: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// Every tag in the graph, most used first, capped at [`MAX_GRAPH_TAGS`].
    pub tags: Vec<TagCount>,
    /// How many edges of each kind the graph holds, most used first.
    pub kinds: Vec<KindCount>,
    /// How many edges each road wrote, in enum order, every road listed even
    /// at zero — the lens's three toggles and the report's table read this
    /// and count nothing of their own (t-5966).
    #[serde(default)]
    pub provenances: Vec<ProvenanceCount>,
    pub pages: usize,
    pub ghosts: usize,
    /// Pages no other page links to and the index does not list — the same
    /// number as `lint.orphans.len()`, kept here as the tile the overview
    /// paints (`second_brain_lint` owns the definition).
    pub orphans: usize,
    /// The lint table — Karpathy's list over this very picture. The window's
    /// health card and the `zerocode vault-lint` recipe read this and count
    /// nothing of their own.
    #[serde(default)]
    pub lint: VaultLint,
    /// A bound was reached, so the picture is a part of the vault rather than
    /// all of it.
    pub capped: bool,
    /// Pages whose body was cut at [`MAX_PAGE_BYTES`].
    pub truncated: usize,
    /// How many pages this scan actually re-read. Zero on an unchanged vault:
    /// the number the incremental promise is measured by.
    pub reparsed: usize,
}

/// What one page contributes, with its body already forgotten.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PageFacts {
    title: String,
    tags: Vec<String>,
    source: Option<String>,
    /// The `ingested_at:` frontmatter value is there. The value itself is not
    /// a fact the picture uses; whether the protocol's key was written is.
    ingested_at: bool,
    /// Vault-relative `raw/…` paths the body names in prose — how the log and
    /// a page vouch that a raw item was read (`second_brain_lint`).
    raw_mentions: Vec<String>,
    /// Spans that may name the active project's code
    /// ([`crate::second_brain_code::code_mentions`]), for the code layer.
    code_mentions: Vec<String>,
    excerpt: String,
    /// Link targets as written, before resolution: the body's own links as
    /// [`EdgeKind::Mentions`] in written order, then each typed relation the
    /// frontmatter declared, in enum order. Each carries the road that wrote
    /// it — the body's links are inferred, a key's are declared.
    links: Vec<WrittenLink>,
    truncated: bool,
}

/// One link as a page wrote it: what it means, which road wrote it, and the
/// target before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WrittenLink {
    kind: EdgeKind,
    provenance: EdgeProvenance,
    target: String,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    modified_ms: i64,
    len: u64,
    facts: PageFacts,
}

/// A vault's parsed pages, held against the modification times they were read
/// at. Owned by the caller — one per vault path — and re-used on every scan.
#[derive(Debug, Default)]
pub struct GraphCache {
    entries: HashMap<String, CacheEntry>,
}

impl GraphCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many pages the cache is holding facts for.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Each held page's code mentions, by page id in id order — what the
    /// code layer asks the project's index about (t-5970). As of the last
    /// [`Self::scan`]; a page naming nothing is left out.
    #[must_use]
    pub fn code_mentions(&self) -> std::collections::BTreeMap<&str, &[String]> {
        self.entries
            .iter()
            .filter(|(_, entry)| !entry.facts.code_mentions.is_empty())
            .map(|(id, entry)| (id.as_str(), entry.facts.code_mentions.as_slice()))
            .collect()
    }

    /// Walk `root/wiki`, re-reading only what changed, and answer the graph.
    ///
    /// `sources` adds a faint node for each page's `source:` frontmatter. The
    /// `raw/` folder itself is never walked: a source is a claim a page makes,
    /// and honouring the claim costs no directory read.
    pub fn scan(&mut self, root: &Path, sources: bool) -> VaultGraph {
        let (found, capped) = walk_pages(&root.join(WIKI_DIR));
        let mut reparsed = 0;
        let mut kept: HashMap<String, CacheEntry> = HashMap::with_capacity(found.len());
        for page in &found {
            let entry = match self.entries.get(&page.id) {
                Some(held) if held.modified_ms == page.modified_ms && held.len == page.len => {
                    held.clone()
                }
                _ => {
                    reparsed += 1;
                    CacheEntry {
                        modified_ms: page.modified_ms,
                        len: page.len,
                        facts: read_page(&page.path, &page.stem),
                    }
                }
            };
            kept.insert(page.id.clone(), entry);
        }
        // Pages that vanished leave with the scan that stopped finding them.
        self.entries = kept;
        let mut graph = assemble(&found, &self.entries, sources);
        graph.capped |= capped;
        graph.reparsed = reparsed;
        // The lint table, off the same picture and the provenance the parse
        // kept beside it. The orphan tile is that table's row, not a second
        // count.
        let provenance: HashMap<String, PageProvenance> = self
            .entries
            .iter()
            .map(|(id, entry)| {
                (
                    id.clone(),
                    PageProvenance {
                        has_source: entry.facts.source.is_some(),
                        has_ingested_at: entry.facts.ingested_at,
                        raw_mentions: entry.facts.raw_mentions.clone(),
                    },
                )
            })
            .collect();
        graph.lint = second_brain_lint::assess(root, &graph, &provenance);
        // The lint table's merge seat, filled from the one function the
        // dedupe lens draws from (t-2931): the recipe prints the pairs the
        // graph shows, by page id. A proposal, not a finding — `settle`
        // leaves `findings` alone.
        let live_limits = second_brain_live::Limits::default();
        let pairs = second_brain_live::merge_candidates(&graph, &live_limits);
        let seated: Vec<second_brain_lint::MergeCandidate> = pairs
            .iter()
            .filter_map(|pair| {
                let left = graph.nodes.get(pair.left as usize)?;
                let right = graph.nodes.get(pair.right as usize)?;
                Some(second_brain_lint::MergeCandidate {
                    left: left.id.clone(),
                    right: right.id.clone(),
                    reason: pair.reason.as_str().to_string(),
                })
            })
            .collect();
        graph.lint = graph.lint.with_merge_candidates(seated);
        graph.orphans = graph.lint.orphans.len();
        graph
    }
}

/// One `.md` file the walk found, before its body is looked at.
#[derive(Debug, Clone)]
struct FoundPage {
    /// Vault-relative, `/` separated: `wiki/sub/Concept.md`.
    id: String,
    path: PathBuf,
    /// File name without the `.md` — the fallback title and a link target.
    stem: String,
    modified_ms: i64,
    len: u64,
}

/// Breadth-first, bounded, sorted. Sorted because the answer is a picture: a
/// graph whose node order followed `read_dir` would lay itself out differently
/// on two machines holding the same vault.
fn walk_pages(wiki: &Path) -> (Vec<FoundPage>, bool) {
    let mut found = Vec::new();
    let mut pending = VecDeque::from([wiki.to_path_buf()]);
    let mut visited = 0;
    let mut capped = false;
    while let Some(directory) = pending.pop_front() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_GRAPH_ENTRIES || found.len() >= MAX_GRAPH_PAGES {
                capped = true;
                pending.clear();
                break;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if kind.is_dir() {
                if !SKIPPED_DIRS.contains(&name) {
                    pending.push_back(entry.path());
                }
                continue;
            }
            if !kind.is_file() || !name.to_ascii_lowercase().ends_with(MARKDOWN_SUFFIX) {
                continue;
            }
            let path = entry.path();
            let Some(id) = relative_id(wiki, &path) else {
                continue;
            };
            let metadata = entry.metadata().ok();
            found.push(FoundPage {
                id,
                stem: name[..name.len() - MARKDOWN_SUFFIX.len()].to_string(),
                modified_ms: metadata.as_ref().and_then(modified_ms).unwrap_or(0),
                len: metadata.as_ref().map_or(0, fs::Metadata::len),
                path,
            });
        }
    }
    found.sort_by(|left, right| left.id.cmp(&right.id));
    (found, capped)
}

/// `wiki/` plus the path below it, with `/` separators on every platform: the
/// id is wire-visible and a graph must not read differently on Windows.
fn relative_id(wiki: &Path, path: &Path) -> Option<String> {
    let below = path.strip_prefix(wiki).ok()?;
    let mut id = String::from(WIKI_DIR);
    for part in below.components() {
        id.push('/');
        id.push_str(part.as_os_str().to_str()?);
    }
    Some(id)
}

fn modified_ms(metadata: &fs::Metadata) -> Option<i64> {
    let at = metadata.modified().ok()?;
    let since = at.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

/// Read at most [`MAX_PAGE_BYTES`] of one page and keep only its facts.
fn read_page(path: &Path, stem: &str) -> PageFacts {
    let Ok(bytes) = fs::read(path) else {
        return PageFacts {
            title: stem.to_string(),
            ..PageFacts::default()
        };
    };
    let truncated = bytes.len() > MAX_PAGE_BYTES;
    // On a character boundary: cutting mid-sequence would turn the tail of a
    // 한글 title into replacement characters.
    let end = if truncated {
        floor_char_boundary(&bytes, MAX_PAGE_BYTES)
    } else {
        bytes.len()
    };
    let text = String::from_utf8_lossy(&bytes[..end]);
    let mut facts = parse_page(&text, stem);
    facts.truncated = truncated;
    facts
}

fn floor_char_boundary(bytes: &[u8], at: usize) -> usize {
    let mut end = at.min(bytes.len());
    while end > 0 && end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
        end -= 1;
    }
    end
}

/// The frontmatter block and the links in the body.
fn parse_page(text: &str, stem: &str) -> PageFacts {
    let (front, body) = split_frontmatter(text);
    let fields = parse_frontmatter(front);
    let title = fields
        .get("title")
        .map(|held| unquote(held.trim()))
        .filter(|held| !held.is_empty())
        .unwrap_or(stem)
        .to_string();
    let tags = fields
        .get("tags")
        .map(|held| split_scalar_list(held))
        .unwrap_or_default();
    let source = fields
        .get("source")
        .map(|held| unquote(held.trim()))
        .filter(|held| !held.is_empty())
        .map(str::to_string);
    let ingested_at = fields
        .get("ingested_at")
        .is_some_and(|held| !unquote(held.trim()).trim().is_empty());
    let raw_mentions = second_brain_lint::raw_mentions(body, MAX_PAGE_LINKS);
    let code_mentions = crate::second_brain_code::code_mentions(
        body,
        source.as_deref(),
        &crate::second_brain_code::CodeLimits::default(),
    );
    let excerpt = page_excerpt(body);
    // Body links first, in written order, then the declared relations in enum
    // order: [`MAX_PAGE_LINKS`] bounds what one page contributes in total, so a
    // runaway body cannot be joined by a runaway frontmatter.
    // The inferred road: prose links.
    let mut links: Vec<WrittenLink> = wiki_links(body)
        .into_iter()
        .map(|target| WrittenLink {
            kind: EdgeKind::Mentions,
            provenance: EdgeProvenance::Inferred,
            target,
        })
        .collect();
    // The declared road: relation keys.
    for kind in EdgeKind::TYPED {
        let Some(value) = fields.get(kind.as_str()) else {
            continue;
        };
        for item in split_relation_list(value) {
            if links.len() >= MAX_PAGE_LINKS {
                break;
            }
            if let Some(target) = relation_target(&item) {
                links.push(WrittenLink {
                    kind,
                    provenance: EdgeProvenance::Declared,
                    target,
                });
            }
        }
    }
    PageFacts {
        title,
        tags,
        source,
        ingested_at,
        raw_mentions,
        code_mentions,
        excerpt,
        links,
        truncated: false,
    }
}

/// The first prose paragraph a card can use as a summary.
///
/// A title or a fenced example is scaffolding rather than prose. Wrapped lines
/// belong to the same paragraph, and decoration is removed before applying the
/// wire-size bound so the payload spends its characters on words.
fn page_excerpt(body: &str) -> String {
    let mut paragraph = String::new();
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(mark) = fence_mark(trimmed) {
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
        if trimmed.is_empty() {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if markdown_heading(trimmed) {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        let cleaned = clean_excerpt_line(trimmed);
        if cleaned.is_empty() {
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(&cleaned);
    }
    truncate_excerpt(&paragraph)
}

fn markdown_heading(line: &str) -> bool {
    let marks = line.chars().take_while(|held| *held == '#').count();
    (1..=6).contains(&marks) && line.chars().nth(marks).is_some_and(char::is_whitespace)
}

fn clean_excerpt_line(line: &str) -> String {
    let mut held = line.trim_start();
    while let Some(rest) = held.strip_prefix('>') {
        held = rest.trim_start();
    }
    for mark in ["- ", "* ", "+ "] {
        if let Some(rest) = held.strip_prefix(mark) {
            held = rest;
            break;
        }
    }

    let chars: Vec<char> = held.chars().collect();
    let mut plain = String::with_capacity(held.len());
    let mut at = 0;
    while at < chars.len() {
        if chars[at] == '[' && at + 1 < chars.len() && chars[at + 1] == '[' {
            let mut end = at + 2;
            while end + 1 < chars.len() && !(chars[end] == ']' && chars[end + 1] == ']') {
                end += 1;
            }
            if end + 1 < chars.len() {
                let inside: String = chars[at + 2..end].iter().collect();
                let visible = inside
                    .split_once('|')
                    .map_or(inside.as_str(), |(_, alias)| alias)
                    .split('#')
                    .next()
                    .unwrap_or_default()
                    .trim();
                plain.push_str(visible);
                at = end + 2;
                continue;
            }
        }
        if !matches!(chars[at], '*' | '_' | '`' | '~') {
            plain.push(chars[at]);
        }
        at += 1;
    }
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_excerpt(excerpt: &str) -> String {
    let Some((end, _)) = excerpt.char_indices().nth(MAX_EXCERPT_CHARS) else {
        return excerpt.to_string();
    };
    let prefix = &excerpt[..end];
    let boundary = prefix
        .char_indices()
        .filter_map(|(at, held)| held.is_whitespace().then_some(at))
        .next_back()
        .unwrap_or(end);
    prefix[..boundary].trim_end().to_string()
}

/// One relation value's items, before normalisation.
///
/// A `[[link]]` is never split down the middle and its own brackets are never
/// eaten by the flow-list strip: `implements: [[wiki/adr/003]]` is one link
/// rather than a list holding a list, while `depends_on: [ [[wiki/a]], b ]` is
/// two items. The two are told apart by what stripping one bracket pair would
/// leave — a flow list of links still opens with `[[`, a lone link does not.
pub(crate) fn split_relation_list(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let inner = match trimmed.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        Some(held) if !trimmed.starts_with("[[") || held.trim_start().starts_with("[[") => held,
        _ => trimmed,
    };
    let chars: Vec<char> = inner.chars().collect();
    let mut items = Vec::new();
    let mut current = String::new();
    let mut at = 0;
    while at < chars.len() {
        if chars[at] == '[' && at + 1 < chars.len() && chars[at + 1] == '[' {
            let mut end = at + 2;
            while end + 1 < chars.len() && !(chars[end] == ']' && chars[end + 1] == ']') {
                end += 1;
            }
            if end + 1 >= chars.len() {
                // An unclosed `[[` is the rest of the value, as a renderer reads it.
                current.extend(&chars[at..]);
                break;
            }
            current.extend(&chars[at..=end + 1]);
            at = end + 2;
            continue;
        }
        if chars[at] == ',' {
            items.push(std::mem::take(&mut current));
        } else {
            current.push(chars[at]);
        }
        at += 1;
    }
    items.push(current);
    items
}

/// One declared relation's target: quotes off, one `[[…]]` pair off, then the
/// same `Target|alias` and `Target#heading` rule a body link answers to.
pub(crate) fn relation_target(item: &str) -> Option<String> {
    let held = unquote(item.trim()).trim();
    let bare = held
        .strip_prefix("[[")
        .and_then(|inner| inner.strip_suffix("]]"))
        .unwrap_or(held);
    link_target(bare)
}

/// A leading `---` block, and everything after it.
///
/// Byte offsets rather than `lines()`: that iterator drops a `\r` without
/// telling anyone how many bytes it consumed, and a vault written on Windows
/// would have its whole frontmatter read one byte out of step per line.
fn split_frontmatter(text: &str) -> (&str, &str) {
    let whole = text.strip_prefix('\u{feff}').unwrap_or(text);
    let after = match whole.strip_prefix("---") {
        Some(rest) if rest.starts_with('\n') => &rest[1..],
        Some(rest) if rest.starts_with("\r\n") => &rest[2..],
        _ => return ("", whole),
    };
    let mut at = 0;
    for _ in 0..MAX_FRONTMATTER_LINES {
        let newline = after[at..].find('\n').map(|held| at + held);
        let line = &after[at..newline.unwrap_or(after.len())];
        if matches!(line.trim_end(), "---" | "...") {
            return (&after[..at], newline.map_or("", |held| &after[held + 1..]));
        }
        match newline {
            Some(held) => at = held + 1,
            None => break,
        }
    }
    ("", whole)
}

/// A deliberately small YAML: `key: value`, `key: [a, b]`, and the block list
/// `key:` followed by `  - item` lines. That is the shape our own vault
/// protocol writes (`second_brain::AGENTS_GUIDE`), and a graph view is not the
/// place to grow a YAML engine — a key this reader cannot parse is simply not
/// a fact the picture uses.
fn parse_frontmatter(front: &str) -> BTreeMap<String, String> {
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    let mut open: Option<String> = None;
    for line in front.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        if trimmed.starts_with(' ') || trimmed.starts_with('\t') {
            let item = trimmed.trim_start();
            if let (Some(key), Some(value)) = (open.as_ref(), item.strip_prefix("- ")) {
                let held = fields.entry(key.clone()).or_default();
                if !held.is_empty() {
                    held.push(',');
                }
                held.push_str(value.trim());
            }
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            open = None;
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if value.is_empty() {
            open = Some(key.clone());
            fields.entry(key).or_default();
        } else {
            open = None;
            fields.insert(key, value.to_string());
        }
    }
    fields
}

/// `[a, b]`, `a, b`, or the block list this reader flattened to `a,b`.
fn split_scalar_list(value: &str) -> Vec<String> {
    let inner = value
        .trim()
        .strip_prefix('[')
        .and_then(|held| held.strip_suffix(']'))
        .unwrap_or(value);
    let mut held: Vec<String> = Vec::new();
    for part in inner.split(',') {
        let tag = unquote(part.trim()).trim_start_matches('#').trim();
        if !tag.is_empty() && !held.iter().any(|seen| seen == tag) {
            held.push(tag.to_string());
        }
    }
    held
}

fn unquote(value: &str) -> &str {
    for mark in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(mark)
            .and_then(|held| held.strip_suffix(mark))
        {
            return inner;
        }
    }
    value
}

/// Every `[[target]]` in the body, in the order it was written.
///
/// Fenced blocks and inline code spans are skipped: a page documenting this
/// very syntax must not be read as linking to its own example — and the vault
/// protocol page this integration writes is exactly such a page. `![[embed]]`
/// counts, because an embed is a relation a person can see on the page.
fn wiki_links(body: &str) -> Vec<String> {
    let mut held = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(mark) = fence_mark(trimmed) {
            match fence {
                // A fence closes on its own character at its own width or more.
                Some((open, width)) if open == mark.0 && mark.1 >= width => fence = None,
                Some(_) => {}
                None => fence = Some(mark),
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        scan_line_links(line, &mut held);
        if held.len() >= MAX_PAGE_LINKS {
            held.truncate(MAX_PAGE_LINKS);
            break;
        }
    }
    held
}

/// A ``` or ~~~ fence and its width, or nothing.
pub(crate) fn fence_mark(trimmed: &str) -> Option<(char, usize)> {
    for mark in ['`', '~'] {
        let width = trimmed.chars().take_while(|held| *held == mark).count();
        if width >= 3 {
            return Some((mark, width));
        }
    }
    None
}

fn scan_line_links(line: &str, into: &mut Vec<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        if chars[at] == '`' {
            // An inline span closes on a backtick run of the same width; an
            // unclosed one swallows the rest of the line, which is what a
            // renderer does with it too.
            let width = chars[at..].iter().take_while(|held| **held == '`').count();
            at += width;
            let mut run = 0;
            while at < chars.len() {
                run = if chars[at] == '`' { run + 1 } else { 0 };
                at += 1;
                if run == width {
                    break;
                }
            }
            continue;
        }
        if chars[at] == '[' && at + 1 < chars.len() && chars[at + 1] == '[' {
            let mut end = at + 2;
            while end + 1 < chars.len() && !(chars[end] == ']' && chars[end + 1] == ']') {
                end += 1;
            }
            if end + 1 >= chars.len() {
                break;
            }
            let inner: String = chars[at + 2..end].iter().collect();
            if let Some(target) = link_target(&inner) {
                into.push(target);
                if into.len() >= MAX_PAGE_LINKS {
                    return;
                }
            }
            at = end + 2;
            continue;
        }
        at += 1;
    }
}

/// `Target`, `Target|alias`, `Target#heading` and `Target#heading|alias`.
///
/// A bare `[[#heading]]` answers nothing: it points inside the page it is
/// written on, and a line from a node to itself is not a relation.
fn link_target(inner: &str) -> Option<String> {
    let before_alias = inner.split('|').next().unwrap_or(inner);
    let target = before_alias
        .split('#')
        .next()
        .unwrap_or(before_alias)
        .trim();
    (!target.is_empty()).then(|| target.to_string())
}

/// Nodes, edges and counts from the pages the walk found and the facts the
/// cache holds for them.
fn assemble(found: &[FoundPage], cache: &HashMap<String, CacheEntry>, sources: bool) -> VaultGraph {
    let mut capped = false;
    let mut nodes: Vec<GraphNode> = Vec::with_capacity(found.len());
    let mut index: HashMap<&str, u32> = HashMap::with_capacity(found.len());
    let mut truncated = 0;
    for page in found {
        let facts = cache.get(&page.id).map(|entry| &entry.facts);
        if facts.is_some_and(|held| held.truncated) {
            truncated += 1;
        }
        index.insert(page.id.as_str(), node_index(nodes.len()));
        nodes.push(GraphNode {
            id: page.id.clone(),
            title: facts
                .map(|held| held.title.clone())
                .unwrap_or_else(|| page.stem.clone()),
            tags: facts.map(|held| held.tags.clone()).unwrap_or_default(),
            kind: NodeKind::Page,
            modified_ms: page.modified_ms,
            out_links: 0,
            in_links: 0,
            source: facts.and_then(|held| held.source.clone()),
            excerpt: facts.map(|held| held.excerpt.clone()).unwrap_or_default(),
            folder: page_folder(&page.id),
        });
    }
    let pages = nodes.len();
    let lookup = LinkLookup::of(found);

    // Ghosts and sources are collected first and appended in sorted order, so
    // the node list is deterministic no matter which page named one first.
    let mut ghosts: BTreeMap<String, Vec<(u32, EdgeKind, EdgeProvenance)>> = BTreeMap::new();
    let mut sourced: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    // One line per pair and kind; the first road to write it names its
    // provenance (body links come before keys, so a page that both links and
    // declares the same relation keeps the declared line as its own kind).
    let mut triples: HashMap<(u32, u32, EdgeKind), EdgeProvenance> = HashMap::new();
    for (at, page) in found.iter().enumerate() {
        let from = node_index(at);
        let Some(entry) = cache.get(&page.id) else {
            continue;
        };
        for link in &entry.facts.links {
            match lookup.resolve(&link.target, &index) {
                Some(to) if triples.len() < MAX_GRAPH_EDGES => {
                    triples
                        .entry((from, to, link.kind))
                        .or_insert(link.provenance);
                }
                Some(_) => capped = true,
                // An unresolved target still means what the page said it
                // meant, so the ghost carries the kind rather than flattening.
                None if ghosts.len() < MAX_GRAPH_GHOSTS || ghosts.contains_key(&link.target) => {
                    ghosts.entry(link.target.clone()).or_default().push((
                        from,
                        link.kind,
                        link.provenance,
                    ));
                }
                None => capped = true,
            }
        }
        if sources && let Some(source) = &entry.facts.source {
            sourced.entry(source.clone()).or_default().push(from);
        }
    }

    let mut extra: Vec<(u32, u32, EdgeKind, EdgeProvenance)> = Vec::new();
    for (target, from) in &ghosts {
        let to = node_index(nodes.len());
        nodes.push(GraphNode {
            id: format!("ghost:{target}"),
            title: target.clone(),
            tags: Vec::new(),
            kind: NodeKind::Ghost,
            modified_ms: 0,
            out_links: 0,
            in_links: 0,
            source: None,
            excerpt: String::new(),
            folder: String::new(),
        });
        extra.extend(
            from.iter()
                .map(|(held, kind, provenance)| (*held, to, *kind, *provenance)),
        );
    }
    let ghost_count = nodes.len() - pages;
    for (source, from) in &sourced {
        let to = node_index(nodes.len());
        nodes.push(GraphNode {
            id: source.clone(),
            title: source.rsplit('/').next().unwrap_or(source).to_string(),
            tags: Vec::new(),
            kind: NodeKind::Source,
            modified_ms: 0,
            out_links: 0,
            in_links: 0,
            source: Some(source.clone()),
            excerpt: String::new(),
            folder: String::new(),
        });
        // A source is named by the page's own words, not by a link: naming it
        // is the faintest relation there is — and the `source:` key is the
        // declared road.
        extra.extend(
            from.iter()
                .map(|held| (*held, to, EdgeKind::Mentions, EdgeProvenance::Declared)),
        );
    }
    for (from, to, kind, provenance) in extra {
        triples.entry((from, to, kind)).or_insert(provenance);
    }

    let mut edges: Vec<GraphEdge> = triples
        .into_iter()
        .filter(|((from, to, _), _)| from != to)
        .map(|((from, to, kind), provenance)| GraphEdge {
            from,
            to,
            kind,
            provenance,
        })
        .collect();
    edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then_with(|| left.to.cmp(&right.to))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    for edge in &edges {
        if let Some(node) = nodes.get_mut(edge.from as usize) {
            node.out_links += 1;
        }
        if let Some(node) = nodes.get_mut(edge.to as usize) {
            node.in_links += 1;
        }
    }

    let mut counted: BTreeMap<&str, u32> = BTreeMap::new();
    for node in &nodes {
        for tag in &node.tags {
            *counted.entry(tag.as_str()).or_default() += 1;
        }
    }
    let mut tags: Vec<TagCount> = counted
        .into_iter()
        .map(|(tag, count)| TagCount {
            tag: tag.to_string(),
            count,
        })
        .collect();
    tags.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.tag.cmp(&right.tag))
    });
    tags.truncate(MAX_GRAPH_TAGS);

    let kinds = kind_counts(&edges);
    let provenances = provenance_counts(&edges);

    VaultGraph {
        nodes,
        edges,
        tags,
        kinds,
        provenances,
        pages,
        ghosts: ghost_count,
        // Settled by the caller from the lint table, once it exists.
        orphans: 0,
        lint: VaultLint::default(),
        capped,
        truncated,
        reparsed: 0,
    }
}

/// How many edges of each kind, most used first.
pub(crate) fn kind_counts(edges: &[GraphEdge]) -> Vec<KindCount> {
    let mut per_kind: BTreeMap<EdgeKind, u32> = BTreeMap::new();
    for edge in edges {
        *per_kind.entry(edge.kind).or_default() += 1;
    }
    let mut kinds: Vec<KindCount> = per_kind
        .into_iter()
        .map(|(kind, count)| KindCount { kind, count })
        .collect();
    kinds.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    kinds
}

/// How many lines each road wrote, every road listed in enum order — a zero
/// is a row that says "none", not a row that is missing.
#[must_use]
pub fn provenance_counts(edges: &[GraphEdge]) -> Vec<ProvenanceCount> {
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

fn page_folder(id: &str) -> String {
    let below = id.strip_prefix("wiki/").unwrap_or(id);
    below
        .split_once('/')
        .map_or_else(String::new, |(folder, _)| folder.to_string())
}

/// The bounds above keep a graph far below `u32::MAX` nodes; saturating is the
/// answer that cannot forge an index into somebody else's node.
fn node_index(at: usize) -> u32 {
    u32::try_from(at).unwrap_or(u32::MAX)
}

/// The four ways a link resolves, in the order Obsidian tries them: the whole
/// vault-relative path, the path below `wiki/`, the bare file name, and the
/// same name folded to lower case.
struct LinkLookup {
    by_path: HashMap<String, String>,
    by_stem: HashMap<String, String>,
    by_folded: HashMap<String, String>,
}

impl LinkLookup {
    fn of(found: &[FoundPage]) -> Self {
        let mut by_path = HashMap::with_capacity(found.len() * 2);
        let mut by_stem = HashMap::with_capacity(found.len());
        let mut by_folded = HashMap::with_capacity(found.len());
        let below_wiki = format!("{WIKI_DIR}/");
        // Sorted input, and `or_insert`: two pages with the same file name in
        // different folders resolve to the first one by path, every time.
        for page in found {
            let without = page
                .id
                .strip_suffix(MARKDOWN_SUFFIX)
                .unwrap_or(&page.id)
                .to_string();
            if let Some(below) = without.strip_prefix(&below_wiki) {
                by_path.entry(below.to_string()).or_insert(page.id.clone());
            }
            by_path.entry(without).or_insert(page.id.clone());
            by_stem.entry(page.stem.clone()).or_insert(page.id.clone());
            by_folded
                .entry(page.stem.to_lowercase())
                .or_insert(page.id.clone());
        }
        Self {
            by_path,
            by_stem,
            by_folded,
        }
    }

    fn resolve(&self, target: &str, index: &HashMap<&str, u32>) -> Option<u32> {
        let trimmed = target.trim().trim_start_matches("./");
        let cleaned = trimmed.strip_suffix(MARKDOWN_SUFFIX).unwrap_or(trimmed);
        let id = self
            .by_path
            .get(cleaned)
            .or_else(|| self.by_stem.get(cleaned))
            .or_else(|| self.by_folded.get(&cleaned.to_lowercase()))?;
        index.get(id.as_str()).copied()
    }
}

/// Where a page lives on disk, for the window's "open this page" door.
///
/// The id is this scanner's own output, so the check is cheap and total: an id
/// that leaves the vault, or names anything but a file under `wiki/` or
/// `raw/`, is refused rather than resolved.
#[must_use]
pub fn page_path(root: &Path, id: &str) -> Option<PathBuf> {
    let mut parts = id.split('/');
    let head = parts.next()?;
    if head != WIKI_DIR && head != RAW_DIR {
        return None;
    }
    let mut path = root.to_path_buf();
    path.push(head);
    let mut named = false;
    for part in parts {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        path.push(part);
        named = true;
    }
    named.then_some(path)
}

/// One page's title — its frontmatter `title:`, else its stem — read from the
/// head of the file only. `None` when `id` names no file inside the vault
/// (the same join as [`page_path`]: a crafted id answers nothing).
#[must_use]
pub fn page_title(root: &Path, id: &str) -> Option<String> {
    let path = page_path(root, id)?;
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let mut file = fs::File::open(&path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut head = vec![0u8; TITLE_HEAD_BYTES];
    let mut read = 0;
    loop {
        let got = std::io::Read::read(&mut file, &mut head[read..]).ok()?;
        if got == 0 {
            break;
        }
        read += got;
        if read == head.len() {
            break;
        }
    }
    let end = floor_char_boundary(&head, read);
    let text = String::from_utf8_lossy(&head[..end]);
    let (front, _) = split_frontmatter(&text);
    let title = parse_frontmatter(front)
        .get("title")
        .map(|held| unquote(held.trim()))
        .filter(|held| !held.is_empty())
        .map_or(stem, str::to_string);
    Some(title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_page_title_comes_from_its_head_or_its_stem_and_never_from_outside_the_vault() {
        let vault = tempfile::tempdir().unwrap();
        let wiki = vault.path().join(WIKI_DIR);
        fs::create_dir_all(wiki.join("dir.md")).unwrap();
        fs::write(
            wiki.join("titled.md"),
            "---\ntitle: \"창\"\ntags: [a]\n---\nbody",
        )
        .unwrap();
        fs::write(wiki.join("plain.md"), "just text").unwrap();
        fs::write(wiki.join("blank.md"), "---\ntitle: \"\"\n---\n").unwrap();
        assert_eq!(
            page_title(vault.path(), "wiki/titled.md").as_deref(),
            Some("창")
        );
        assert_eq!(
            page_title(vault.path(), "wiki/plain.md").as_deref(),
            Some("plain")
        );
        assert_eq!(
            page_title(vault.path(), "wiki/blank.md").as_deref(),
            Some("blank")
        );
        assert_eq!(page_title(vault.path(), "wiki/missing.md"), None);
        assert_eq!(page_title(vault.path(), "wiki/dir.md"), None);
        assert_eq!(page_title(vault.path(), "../wiki/titled.md"), None);
        assert_eq!(page_title(vault.path(), "wiki/../titled.md"), None);
    }

    /// A vault whose `wiki/` holds exactly the pages named, plus the scaffolding
    /// `second_brain::setup` would have written.
    fn vault(pages: &[(&str, &str)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        crate::second_brain::setup(root.path()).unwrap();
        for (relative, body) in pages {
            let path = root.path().join(WIKI_DIR).join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        root
    }

    fn node<'a>(graph: &'a VaultGraph, id: &str) -> &'a GraphNode {
        graph
            .nodes
            .iter()
            .find(|held| held.id == id)
            .unwrap_or_else(|| panic!("no node {id} in {:?}", ids(graph)))
    }

    fn ids(graph: &VaultGraph) -> Vec<&str> {
        graph.nodes.iter().map(|held| held.id.as_str()).collect()
    }

    /// Every edge as `from -> to` by node id, so a failure names pages rather
    /// than indices.
    fn relations(graph: &VaultGraph) -> Vec<String> {
        graph
            .edges
            .iter()
            .map(|edge| {
                format!(
                    "{} -> {}",
                    graph.nodes[edge.from as usize].id, graph.nodes[edge.to as usize].id
                )
            })
            .collect()
    }

    #[test]
    fn pages_are_nodes_and_wikilinks_are_edges_in_all_four_spellings() {
        let root = vault(&[
            (
                "Alpha.md",
                "---\ntitle: 알파\ntags: [core, reading]\n---\n\nsee [[Beta]] and [[Gamma|third]] \
                 and [[Delta#heading]] and [[Beta#top|again]].\n",
            ),
            ("Beta.md", "plain\n"),
            ("Gamma.md", "plain\n"),
            ("Delta.md", "plain\n"),
        ]);

        let graph = GraphCache::new().scan(root.path(), false);

        let alpha = node(&graph, "wiki/Alpha.md");
        assert_eq!(alpha.title, "알파");
        assert_eq!(alpha.tags, ["core", "reading"]);
        assert_eq!(alpha.kind, NodeKind::Page);
        // The alias and the heading name the same page twice — one relation.
        assert_eq!(alpha.out_links, 3);
        assert_eq!(
            relations(&graph),
            [
                "wiki/Alpha.md -> wiki/Beta.md",
                "wiki/Alpha.md -> wiki/Delta.md",
                "wiki/Alpha.md -> wiki/Gamma.md",
            ]
        );
        assert_eq!(node(&graph, "wiki/Beta.md").in_links, 1);
        assert_eq!(graph.ghosts, 0);
        assert!(!graph.capped);
        // index.md and log.md are pages of the wiki like any other.
        assert_eq!(graph.pages, 6);
    }

    #[test]
    fn a_link_to_nothing_becomes_one_ghost_every_page_pointing_at_it_shares() {
        let root = vault(&[
            ("One.md", "[[Missing]] and [[Missing|again]]\n"),
            ("Two.md", "also [[Missing]]\n"),
        ]);

        let graph = GraphCache::new().scan(root.path(), false);

        let ghost = node(&graph, "ghost:Missing");
        assert_eq!(ghost.kind, NodeKind::Ghost);
        assert_eq!(ghost.title, "Missing");
        assert_eq!(ghost.in_links, 2);
        assert_eq!(graph.ghosts, 1);
        assert_eq!(
            relations(&graph),
            [
                "wiki/One.md -> ghost:Missing",
                "wiki/Two.md -> ghost:Missing"
            ]
        );
    }

    #[test]
    fn a_link_inside_code_is_documentation_rather_than_a_relation() {
        let root = vault(&[
            (
                "Guide.md",
                "write `[[Example]]` inline.\n\n```md\n[[Fenced]]\n```\n\n~~~\n[[Tilde]]\n~~~\n\n\
                 and a real [[target]].\n",
            ),
            ("Target.md", "plain\n"),
        ]);

        let graph = GraphCache::new().scan(root.path(), false);

        assert_eq!(graph.ghosts, 0, "{:?}", ids(&graph));
        assert_eq!(relations(&graph), ["wiki/Guide.md -> wiki/Target.md"]);
    }

    #[test]
    fn excerpt_skips_frontmatter_headings_and_fences_then_cleans_the_first_paragraph() {
        let root = vault(&[(
            "topics/Guide.md",
            concat!(
                "---\n",
                "title: Guide\n",
                "tags: [core]\n",
                "---\n\n",
                "# Guide\n\n",
                "```md\n",
                "This fenced example is not the summary.\n",
                "```\n\n",
                "> **Start** with [[Deep Notes|the useful note]] and `inline code`.\n",
                "This wrapped line belongs to the same paragraph.\n\n",
                "The second paragraph is not included.\n",
            ),
        )]);

        let graph = GraphCache::new().scan(root.path(), false);
        let guide = node(&graph, "wiki/topics/Guide.md");

        assert_eq!(guide.folder, "topics");
        assert_eq!(
            guide.excerpt,
            "Start with the useful note and inline code. This wrapped line belongs to the same paragraph."
        );
        assert_eq!(node(&graph, "wiki/index.md").folder, "");
    }

    #[test]
    fn excerpt_stops_at_a_word_boundary_and_non_pages_carry_no_page_metadata() {
        let words = (0..80)
            .map(|at| format!("word{at:02}"))
            .collect::<Vec<_>>()
            .join(" ");
        let page = format!("{words}\n\n[[Missing]]\n");
        let root = vault(&[(
            "Page.md",
            &format!("---\nsource: raw/article.md\n---\n\n{page}"),
        )]);

        let graph = GraphCache::new().scan(root.path(), true);
        let held = node(&graph, "wiki/Page.md");
        let ghost = node(&graph, "ghost:Missing");
        let source = node(&graph, "raw/article.md");

        assert!(held.excerpt.chars().count() <= 160, "{}", held.excerpt);
        assert!(!held.excerpt.ends_with(char::is_whitespace));
        assert!(words.starts_with(&held.excerpt));
        assert_eq!(ghost.excerpt, "");
        assert_eq!(ghost.folder, "");
        assert_eq!(source.excerpt, "");
        assert_eq!(source.folder, "");
    }

    #[test]
    fn a_link_resolves_by_path_by_name_and_by_case_and_never_to_itself() {
        let root = vault(&[
            (
                "Hub.md",
                "[[topics/Deep]] · [[wiki/topics/Deep]] · [[Deep]] · [[deep]] · [[Deep.md]] · \
                 [[#own-heading]] · [[Hub]]\n",
            ),
            ("topics/Deep.md", "plain\n"),
        ]);

        let graph = GraphCache::new().scan(root.path(), false);

        assert_eq!(graph.ghosts, 0, "{:?}", ids(&graph));
        assert_eq!(relations(&graph), ["wiki/Hub.md -> wiki/topics/Deep.md"]);
    }

    #[test]
    fn frontmatter_reads_block_lists_crlf_and_a_quoted_source() {
        let root = vault(&[
            (
                "Block.md",
                "---\ntags:\n  - alpha\n  - \"beta\"\nsource: 'raw/article.md'\n---\nbody\n",
            ),
            (
                "Windows.md",
                "---\r\ntitle: 창\r\ntags: [alpha]\r\n---\r\nbody [[Block]]\r\n",
            ),
            ("Bare.md", "no frontmatter, [[Block]]\n"),
        ]);

        let graph = GraphCache::new().scan(root.path(), true);

        let block = node(&graph, "wiki/Block.md");
        assert_eq!(block.tags, ["alpha", "beta"]);
        assert_eq!(block.source.as_deref(), Some("raw/article.md"));
        assert_eq!(node(&graph, "wiki/Windows.md").title, "창");
        assert_eq!(node(&graph, "wiki/Windows.md").tags, ["alpha"]);
        // No frontmatter, so the file name is the title.
        assert_eq!(node(&graph, "wiki/Bare.md").title, "Bare");
        // `raw/` is never walked; the page's own words put this node here.
        let source = node(&graph, "raw/article.md");
        assert_eq!(source.kind, NodeKind::Source);
        assert_eq!(source.title, "article.md");
        assert_eq!(source.in_links, 1);
        assert_eq!(
            graph.tags,
            [
                TagCount {
                    tag: "alpha".to_string(),
                    count: 2
                },
                TagCount {
                    tag: "beta".to_string(),
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn sources_stay_out_of_the_picture_unless_they_are_asked_for() {
        let root = vault(&[("Page.md", "---\nsource: raw/article.md\n---\nbody\n")]);

        let without = GraphCache::new().scan(root.path(), false);

        assert!(!ids(&without).contains(&"raw/article.md"));
        // The fact is still on the node, so the inspector can name it.
        assert_eq!(
            node(&without, "wiki/Page.md").source.as_deref(),
            Some("raw/article.md")
        );
    }

    #[test]
    fn a_second_scan_re_reads_only_what_changed_on_disk() {
        let root = vault(&[("One.md", "[[Two]]\n"), ("Two.md", "plain\n")]);
        let mut cache = GraphCache::new();

        let first = cache.scan(root.path(), false);
        assert_eq!(first.reparsed, first.pages);
        assert_eq!(cache.len(), first.pages);

        let again = cache.scan(root.path(), false);
        assert_eq!(again.reparsed, 0, "an unchanged vault must read no page");
        assert_eq!(relations(&again), relations(&first));

        // Length alone moves here; a same-length rewrite is caught by mtime,
        // which the filesystem advances on its own.
        let path = root.path().join("wiki/Two.md");
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"now with [[One]]\n").unwrap();
        drop(file);
        let third = cache.scan(root.path(), false);
        assert_eq!(third.reparsed, 1);
        assert_eq!(
            relations(&third),
            ["wiki/One.md -> wiki/Two.md", "wiki/Two.md -> wiki/One.md"]
        );

        fs::remove_file(&path).unwrap();
        let fourth = cache.scan(root.path(), false);
        assert_eq!(fourth.pages, third.pages - 1);
        assert_eq!(cache.len(), fourth.pages, "a deleted page leaves the cache");
    }

    #[test]
    fn orphans_are_pages_no_page_links_to_and_the_index_does_not_list() {
        let root = vault(&[
            ("Linked.md", "[[Other]]\n"),
            ("Other.md", "plain\n"),
            ("Alone.md", "plain\n"),
        ]);

        let graph = GraphCache::new().scan(root.path(), false);

        // Linked.md and Alone.md: nothing points at them and the index (the
        // scaffolding one, listing nothing yet) does not either. Other.md is
        // reached from Linked.md. The scaffolding pages are never orphans.
        assert_eq!(graph.orphans, 2);
        assert_eq!(graph.lint.orphans, ["wiki/Alone.md", "wiki/Linked.md"]);
        assert_eq!(node(&graph, "wiki/Alone.md").in_links, 0);
        assert_eq!(node(&graph, "wiki/Alone.md").out_links, 0);
    }

    #[test]
    fn a_page_longer_than_the_bound_is_read_to_the_bound_and_says_so() {
        let mut body = String::from("[[Early]]\n");
        while body.len() < MAX_PAGE_BYTES {
            body.push_str("한글 채우기 줄\n");
        }
        body.push_str("[[Late]]\n");
        let root = vault(&[("Long.md", &body), ("Early.md", "e\n"), ("Late.md", "l\n")]);

        let graph = GraphCache::new().scan(root.path(), false);

        assert_eq!(graph.truncated, 1);
        assert_eq!(relations(&graph), ["wiki/Long.md -> wiki/Early.md"]);
        // Cut on a character boundary: a title read out of the tail is words,
        // not replacement marks.
        assert!(
            !graph
                .nodes
                .iter()
                .any(|held| held.title.contains('\u{fffd}'))
        );
    }

    #[test]
    fn one_page_contributes_no_more_links_than_the_bound_allows() {
        let mut body = String::new();
        for at in 0..(MAX_PAGE_LINKS + 50) {
            body.push_str(&format!("[[T{at}]]\n"));
        }
        let root = vault(&[("Runaway.md", &body)]);

        let graph = GraphCache::new().scan(root.path(), false);

        assert_eq!(
            node(&graph, "wiki/Runaway.md").out_links as usize,
            MAX_PAGE_LINKS
        );
    }

    #[test]
    fn the_walk_skips_obsidian_private_folders_and_anything_that_is_not_markdown() {
        let root = vault(&[("Real.md", "plain\n")]);
        fs::create_dir_all(root.path().join("wiki/.obsidian/plugins")).unwrap();
        fs::write(root.path().join("wiki/.obsidian/plugins/note.md"), "x").unwrap();
        fs::write(root.path().join("wiki/image.png"), "x").unwrap();

        let graph = GraphCache::new().scan(root.path(), false);

        assert!(ids(&graph).contains(&"wiki/Real.md"));
        assert!(!ids(&graph).iter().any(|held| held.contains(".obsidian")));
        assert!(!ids(&graph).iter().any(|held| held.ends_with(".png")));
    }

    /// Every edge as `from -kind-> to` by node id, so a failure names pages
    /// and relations rather than indices.
    fn typed(graph: &VaultGraph) -> Vec<String> {
        graph
            .edges
            .iter()
            .map(|edge| {
                format!(
                    "{} -{}-> {}",
                    graph.nodes[edge.from as usize].id,
                    edge.kind.as_str(),
                    graph.nodes[edge.to as usize].id
                )
            })
            .collect()
    }

    #[test]
    fn a_frontmatter_key_names_the_relation_it_writes() {
        let root = vault(&[
            (
                "Decision.md",
                concat!(
                    "---\n",
                    "implements: [[wiki/adr/003]]\n",
                    "supersedes: \"[[wiki/Old|옛 결정]]\"\n",
                    "depends_on: [ [[wiki/Alpha]], Beta ]\n",
                    "related:\n",
                    "  - [[wiki/Alpha#section]]\n",
                    "  - Beta\n",
                    "---\n",
                ),
            ),
            ("adr/003.md", "an adr\n"),
            ("Old.md", "the old one\n"),
            ("Alpha.md", "alpha\n"),
            ("Beta.md", "beta\n"),
        ]);
        let graph = GraphCache::new().scan(root.path(), false);
        let held = typed(&graph);
        // A `[[link]]` inside a flow list keeps its own brackets, and an alias
        // or a heading is trimmed exactly as a body link's would be.
        for want in [
            "wiki/Decision.md -related-> wiki/Alpha.md",
            "wiki/Decision.md -related-> wiki/Beta.md",
            "wiki/Decision.md -implements-> wiki/adr/003.md",
            "wiki/Decision.md -depends_on-> wiki/Alpha.md",
            "wiki/Decision.md -depends_on-> wiki/Beta.md",
            "wiki/Decision.md -supersedes-> wiki/Old.md",
        ] {
            assert!(
                held.contains(&want.to_string()),
                "{want} missing from {held:?}"
            );
        }
        // Frontmatter alone, so nothing was read as a mention.
        assert!(
            !held.iter().any(|edge| edge.contains("-mentions->")),
            "{held:?}"
        );
    }

    #[test]
    fn mentioning_and_implementing_one_page_is_two_relations() {
        let root = vault(&[
            (
                "Alpha.md",
                "---\nimplements: [[Beta]]\n---\n\nsee [[Beta]] and [[Beta]] again\n",
            ),
            ("Beta.md", "beta\n"),
        ]);
        let graph = GraphCache::new().scan(root.path(), false);
        // The repeated body link is one edge; the declared relation is another.
        assert_eq!(
            typed(&graph),
            vec![
                "wiki/Alpha.md -mentions-> wiki/Beta.md",
                "wiki/Alpha.md -implements-> wiki/Beta.md",
            ]
        );
        // Both edges are counted, so a pair may weigh more than one link.
        assert_eq!(node(&graph, "wiki/Alpha.md").out_links, 2);
        assert_eq!(node(&graph, "wiki/Beta.md").in_links, 2);
        assert_eq!(
            graph.kinds,
            vec![
                KindCount {
                    kind: EdgeKind::Mentions,
                    count: 1
                },
                KindCount {
                    kind: EdgeKind::Implements,
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn a_declared_relation_to_nothing_is_a_ghost_that_kept_its_kind() {
        let root = vault(&[(
            "Alpha.md",
            "---\ncontradicts: [[wiki/미래]]\n---\n\nno body links\n",
        )]);
        let graph = GraphCache::new().scan(root.path(), false);
        assert_eq!(graph.ghosts, 1);
        assert_eq!(
            typed(&graph),
            vec!["wiki/Alpha.md -contradicts-> ghost:wiki/미래"]
        );
        assert_eq!(node(&graph, "ghost:wiki/미래").kind, NodeKind::Ghost);
    }

    #[test]
    fn kinds_are_counted_most_used_first_and_edges_sort_by_the_whole_triple() {
        let root = vault(&[
            (
                "Alpha.md",
                "---\nrelated: [[Beta]], [[Gamma]]\nimplements: [[Beta]]\n---\n\n[[Gamma]]\n",
            ),
            ("Beta.md", "---\nrelated: [[Gamma]]\n---\n"),
            ("Gamma.md", "gamma\n"),
        ]);
        let graph = GraphCache::new().scan(root.path(), false);
        assert_eq!(
            graph.kinds,
            vec![
                KindCount {
                    kind: EdgeKind::Related,
                    count: 3
                },
                KindCount {
                    kind: EdgeKind::Mentions,
                    count: 1
                },
                KindCount {
                    kind: EdgeKind::Implements,
                    count: 1
                },
            ]
        );
        // Sorted by from, then to, then the kind's own order: `related` before
        // `implements` on the same pair.
        assert_eq!(
            typed(&graph),
            [
                "wiki/Alpha.md -related-> wiki/Beta.md",
                "wiki/Alpha.md -implements-> wiki/Beta.md",
                "wiki/Alpha.md -mentions-> wiki/Gamma.md",
                "wiki/Alpha.md -related-> wiki/Gamma.md",
                "wiki/Beta.md -related-> wiki/Gamma.md",
            ]
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
        );
        // `implements` sorts after `mentions`, yet its edge comes first: the
        // pair decides before the kind does.
        assert_eq!(
            graph.edges[1].kind.cmp(&graph.edges[2].kind),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn a_declared_relation_leaves_the_incremental_promise_alone() {
        let root = vault(&[
            ("Alpha.md", "---\ndepends_on: [[Beta]]\n---\n\n[[Beta]]\n"),
            ("Beta.md", "beta\n"),
        ]);
        let mut cache = GraphCache::new();
        let first = cache.scan(root.path(), false);
        // Every page the walk found, scaffolding included, was read once.
        assert_eq!(first.reparsed, first.pages);
        let second = cache.scan(root.path(), false);
        assert_eq!(second.reparsed, 0);
        // The pair is two relations, and the second scan says so from cache.
        assert_eq!(
            typed(&second),
            vec![
                "wiki/Alpha.md -mentions-> wiki/Beta.md",
                "wiki/Alpha.md -depends_on-> wiki/Beta.md",
            ]
        );
    }

    /// t-5966 G1: the three roads each name their own provenance, and the
    /// names go out on the wire as the enum spells them.
    #[test]
    fn each_road_names_the_provenance_it_writes() {
        let root = vault(&[
            (
                "Alpha.md",
                "---\ntitle: 알파\nsource: raw/alpha.md\nimplements: [[Beta]]\n---\n\n\
                 see [[Gamma]] and [[Beta]].\n",
            ),
            ("Beta.md", "plain\n"),
            ("Gamma.md", "plain\n"),
        ]);
        let graph = GraphCache::new().scan(root.path(), true);
        let of = |from: &str, to: &str, kind: EdgeKind| {
            graph
                .edges
                .iter()
                .find(|edge| {
                    graph.nodes[edge.from as usize].id == from
                        && graph.nodes[edge.to as usize].id == to
                        && edge.kind == kind
                })
                .map(|edge| edge.provenance)
        };
        // Prose is the inferred road …
        assert_eq!(
            of("wiki/Alpha.md", "wiki/Gamma.md", EdgeKind::Mentions),
            Some(EdgeProvenance::Inferred)
        );
        assert_eq!(
            of("wiki/Alpha.md", "wiki/Beta.md", EdgeKind::Mentions),
            Some(EdgeProvenance::Inferred)
        );
        // … a key is the declared road, and so is `source:` …
        assert_eq!(
            of("wiki/Alpha.md", "wiki/Beta.md", EdgeKind::Implements),
            Some(EdgeProvenance::Declared)
        );
        assert_eq!(
            of("wiki/Alpha.md", "raw/alpha.md", EdgeKind::Mentions),
            Some(EdgeProvenance::Declared)
        );
        // … and the measured road never writes into the scanned picture:
        // every line is vouched for, so the lint's row is zero.
        assert!(
            graph
                .edges
                .iter()
                .all(|edge| vouched(edge, graph.nodes[edge.to as usize].kind))
        );
        assert_eq!(graph.lint.counts.unsourced_edges, 0);
        assert_eq!(
            graph.provenances,
            [
                ProvenanceCount {
                    provenance: EdgeProvenance::Measured,
                    count: 0
                },
                ProvenanceCount {
                    provenance: EdgeProvenance::Declared,
                    count: 2
                },
                ProvenanceCount {
                    provenance: EdgeProvenance::Inferred,
                    count: 2
                },
            ]
        );
        // The wire spelling is the enum's own.
        for edge in &graph.edges {
            let wire = serde_json::to_value(edge).unwrap();
            assert_eq!(wire["provenance"], edge.provenance.as_str());
        }
        for provenance in EdgeProvenance::ALL {
            assert_eq!(
                serde_json::to_value(provenance).unwrap(),
                provenance.as_str()
            );
        }
    }

    /// The measured road rides beside the picture: a pair the dedupe pass
    /// finds carries `measured`, written by that producer and nobody else.
    #[test]
    fn the_measured_road_is_the_dedupe_pass_and_says_so_on_the_wire() {
        let root = vault(&[
            ("설계 노트 첫째.md", "[[Target]]\n"),
            ("설계 노트 둘째.md", "[[Target]]\n"),
            ("Target.md", "plain\n"),
        ]);
        let graph = GraphCache::new().scan(root.path(), false);
        let limits = crate::second_brain_live::Limits::default();
        let pairs = crate::second_brain_live::merge_candidates(&graph, &limits);
        assert!(!pairs.is_empty(), "{:?}", ids(&graph));
        assert!(
            pairs
                .iter()
                .all(|pair| pair.provenance == EdgeProvenance::Measured)
        );
        assert_eq!(
            serde_json::to_value(&pairs[0]).unwrap()["provenance"],
            EdgeProvenance::Measured.as_str()
        );
    }

    /// The one rule behind the lint row, asked directly.
    #[test]
    fn a_line_is_vouched_for_only_on_the_road_that_could_have_written_it() {
        let line = |kind, provenance| GraphEdge {
            from: 0,
            to: 1,
            kind,
            provenance,
        };
        assert!(vouched(
            &line(EdgeKind::Mentions, EdgeProvenance::Inferred),
            NodeKind::Page
        ));
        assert!(vouched(
            &line(EdgeKind::Mentions, EdgeProvenance::Inferred),
            NodeKind::Ghost
        ));
        assert!(vouched(
            &line(EdgeKind::Related, EdgeProvenance::Declared),
            NodeKind::Page
        ));
        assert!(vouched(
            &line(EdgeKind::Supersedes, EdgeProvenance::Declared),
            NodeKind::Ghost
        ));
        assert!(vouched(
            &line(EdgeKind::Mentions, EdgeProvenance::Declared),
            NodeKind::Source
        ));
        // Prose writes no key; a key writes no bare mention to a page; the
        // scanner measures nothing; a source is named by a key alone.
        assert!(!vouched(
            &line(EdgeKind::Related, EdgeProvenance::Inferred),
            NodeKind::Page
        ));
        assert!(!vouched(
            &line(EdgeKind::Mentions, EdgeProvenance::Declared),
            NodeKind::Page
        ));
        assert!(!vouched(
            &line(EdgeKind::Mentions, EdgeProvenance::Measured),
            NodeKind::Page
        ));
        assert!(!vouched(
            &line(EdgeKind::Mentions, EdgeProvenance::Inferred),
            NodeKind::Source
        ));
        assert!(!vouched(
            &line(EdgeKind::Implements, EdgeProvenance::Declared),
            NodeKind::Source
        ));
    }

    #[test]
    fn a_node_id_only_opens_a_file_inside_the_vault() {
        let root = Path::new("/vault");

        assert_eq!(
            page_path(root, "wiki/topics/Deep.md"),
            Some(PathBuf::from("/vault/wiki/topics/Deep.md"))
        );
        assert_eq!(
            page_path(root, "raw/article.md"),
            Some(PathBuf::from("/vault/raw/article.md"))
        );
        assert_eq!(page_path(root, "ghost:Missing"), None);
        assert_eq!(page_path(root, "wiki/../../etc/passwd"), None);
        assert_eq!(page_path(root, "wiki"), None);
        assert_eq!(page_path(root, "/etc/passwd"), None);
    }
}
