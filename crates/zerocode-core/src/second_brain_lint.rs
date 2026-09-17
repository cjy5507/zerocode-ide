//! The vault's lint table — Karpathy's list, computed once and read twice.
//!
//! One producer: [`VaultLint`] is built by [`assess`] from the same
//! [`VaultGraph`] the window's knowledge graph draws, and the window's vault
//! health card and the `zerocode vault-lint` recipe both read THIS table.
//! Neither counts anything of its own — a count the card shows and a count the
//! recipe exits on come out of one function, so they cannot disagree.
//!
//! The deterministic findings, each a list of ids the graph already carries:
//!
//! - **index gaps** — pages `wiki/index.md` does not link to;
//! - **ghost links** — link targets no page answers to, and who points at them;
//! - **orphans** — pages no other page links to *and* the index does not list
//!   (`wiki/log.md` is a journal, not a place a reader navigates from, so a
//!   link from it rescues nothing);
//! - **missing frontmatter** — pages without `source:` or `ingested_at:`;
//! - **undeclared relations** — a page whose prose links a page that no
//!   relation key on that page names (the vault protocol says a relation the
//!   prose states should also be a key, so the graph sees it);
//! - **unlogged raw** — `raw/` items no page's `source:` names and no page
//!   (the log above all) mentions by path.
//!
//! Two more rows are facts rather than faults and never fail the recipe:
//! declared **contradictions** and **superseded** pages. And one row is a
//! seat kept for t-2931's dedupe lens — [`VaultLint::merge_candidates`] — which
//! this module leaves as "not computed" rather than as a zero it never measured.
//!
//! Every list is sorted, so the same vault lints the same twice.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::second_brain::{RAW_DIR, RAW_README_FILE, WIKI_INDEX_FILE, WIKI_LOG_FILE};
use crate::second_brain_graph::{EdgeKind, MAX_GRAPH_ENTRIES, NodeKind, VaultGraph};

/// A link target nothing on disk answers to, and the pages that wrote it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostLink {
    /// The target as written — the graph's `ghost:<target>` without the prefix.
    pub target: String,
    /// Page ids pointing at it, sorted.
    pub from: Vec<String>,
}

/// A page whose frontmatter is missing one of the two keys the protocol needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissingFrontmatter {
    pub page: String,
    /// The keys that are missing, in the order the protocol names them:
    /// `source`, then `ingested_at`.
    pub missing: Vec<String>,
}

/// A page whose prose links pages that no relation key on it names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndeclaredRelation {
    pub page: String,
    /// The linked page ids with no declared relation, sorted.
    pub targets: Vec<String>,
}

/// Two pages that look like one — t-2931's dedupe lens fills this seat.
///
/// Kept here so the recipe and the health card have the row the day the
/// producer exists, without a second definition of the pair.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeCandidate {
    pub left: String,
    pub right: String,
    /// Why the pair was proposed, in the producer's own words.
    pub reason: String,
}

/// The counts a card paints and a recipe prints — one per row, in row order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintCounts {
    pub index_gaps: u32,
    pub ghost_links: u32,
    pub orphans: u32,
    pub missing_frontmatter: u32,
    pub undeclared_relations: u32,
    pub unlogged_raw: u32,
    pub contradictions: u32,
    pub superseded: u32,
    /// `None` until the dedupe lens (t-2931) computes it — a row that says
    /// "not measured", never a zero nobody counted.
    pub merge_candidates: Option<u32>,
}

/// The table. Lists carry page ids exactly as the graph's nodes spell them,
/// so a reader can turn a row into a lens without a lookup of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultLint {
    pub index_gaps: Vec<String>,
    pub ghost_links: Vec<GhostLink>,
    pub orphans: Vec<String>,
    pub missing_frontmatter: Vec<MissingFrontmatter>,
    pub undeclared_relations: Vec<UndeclaredRelation>,
    /// Vault-relative `raw/…` paths, sorted.
    pub unlogged_raw: Vec<String>,
    /// Declared `contradicts` relations — a count, because the pair is already
    /// an edge the graph draws and the search word finds.
    pub contradictions: u32,
    /// Pages a `supersedes` relation points at, sorted; two pages superseding
    /// one target is one superseded page.
    pub superseded: Vec<String>,
    /// t-2931's seat. `None` means nobody computed it.
    pub merge_candidates: Option<Vec<MergeCandidate>>,
    pub counts: LintCounts,
    /// The rows a recipe fails on — the six deterministic findings, summed.
    /// Contradictions and superseded pages are facts the protocol asked for,
    /// not faults, and a merge candidate is a proposal.
    pub findings: u32,
}

impl VaultLint {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.findings == 0
    }

    /// The table with its merge seat filled and the counts settled again —
    /// the scan calls this once the graph exists to compute the pairs from.
    #[must_use]
    pub fn with_merge_candidates(mut self, candidates: Vec<MergeCandidate>) -> Self {
        self.merge_candidates = Some(candidates);
        self.settle()
    }

    fn settle(mut self) -> Self {
        let count = |held: usize| u32::try_from(held).unwrap_or(u32::MAX);
        self.counts = LintCounts {
            index_gaps: count(self.index_gaps.len()),
            ghost_links: count(self.ghost_links.len()),
            orphans: count(self.orphans.len()),
            missing_frontmatter: count(self.missing_frontmatter.len()),
            undeclared_relations: count(self.undeclared_relations.len()),
            unlogged_raw: count(self.unlogged_raw.len()),
            contradictions: self.contradictions,
            superseded: count(self.superseded.len()),
            merge_candidates: self.merge_candidates.as_ref().map(|held| count(held.len())),
        };
        self.findings = [
            self.counts.index_gaps,
            self.counts.ghost_links,
            self.counts.orphans,
            self.counts.missing_frontmatter,
            self.counts.undeclared_relations,
            self.counts.unlogged_raw,
        ]
        .into_iter()
        .fold(0u32, u32::saturating_add);
        self
    }
}

/// What the scanner knows about one page beyond the node it became: the
/// frontmatter keys the protocol requires and the `raw/…` paths its body
/// names. Kept off the wire — a card does not need them, the lint does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageProvenance {
    pub has_source: bool,
    pub has_ingested_at: bool,
    /// Vault-relative `raw/…` paths the body mentions, as written.
    pub raw_mentions: Vec<String>,
}

/// The table for `graph`, read out of the picture plus the provenance the
/// scanner kept beside it and one bounded listing of `root/raw`.
///
/// `provenance` is keyed by page id; a page the map does not know is treated
/// as having neither key — the scanner could not read it, and a page whose
/// header cannot be read has no header a protocol can trust.
#[must_use]
pub fn assess(
    root: &Path,
    graph: &VaultGraph,
    provenance: &HashMap<String, PageProvenance>,
) -> VaultLint {
    let nodes = &graph.nodes;
    let index_at = nodes.iter().position(|node| node.id == WIKI_INDEX_FILE);
    let log_at = nodes.iter().position(|node| node.id == WIKI_LOG_FILE);
    let scaffold = |at: usize| Some(at) == index_at || Some(at) == log_at;
    let is_page = |at: usize| {
        nodes
            .get(at)
            .is_some_and(|node| node.kind == NodeKind::Page)
    };

    // Who the index lists, who links whom in prose, and which pairs a key
    // declares — three passes over the one edge list.
    let mut indexed: HashSet<u32> = HashSet::new();
    let mut inbound_from_pages: HashSet<u32> = HashSet::new();
    let mut mentioned: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut declared: HashSet<(u32, u32)> = HashSet::new();
    let mut ghosts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut contradictions = 0u32;
    let mut superseded: BTreeSet<String> = BTreeSet::new();
    for edge in &graph.edges {
        let (from, to) = (edge.from as usize, edge.to as usize);
        let Some(target) = nodes.get(to) else {
            continue;
        };
        match target.kind {
            NodeKind::Ghost => {
                let written = target.id.strip_prefix("ghost:").unwrap_or(&target.id);
                ghosts
                    .entry(written.to_string())
                    .or_default()
                    .insert(nodes[from].id.clone());
                continue;
            }
            NodeKind::Source => continue,
            NodeKind::Page => {}
        }
        if Some(from) == index_at {
            indexed.insert(edge.to);
        }
        if !scaffold(from) && is_page(from) {
            inbound_from_pages.insert(edge.to);
        }
        match edge.kind {
            EdgeKind::Mentions => {
                mentioned.entry(edge.from).or_default().insert(edge.to);
            }
            typed => {
                declared.insert((edge.from, edge.to));
                if typed == EdgeKind::Contradicts {
                    contradictions = contradictions.saturating_add(1);
                }
                if typed == EdgeKind::Supersedes {
                    superseded.insert(target.id.clone());
                }
            }
        }
    }

    let mut index_gaps = Vec::new();
    let mut orphans = Vec::new();
    let mut missing_frontmatter = Vec::new();
    let mut undeclared_relations = Vec::new();
    let mut named_sources: HashSet<String> = HashSet::new();
    for (at, node) in nodes.iter().enumerate() {
        if node.kind != NodeKind::Page || scaffold(at) {
            continue;
        }
        let seat = u32::try_from(at).unwrap_or(u32::MAX);
        let listed = indexed.contains(&seat);
        if !listed {
            index_gaps.push(node.id.clone());
            if !inbound_from_pages.contains(&seat) {
                orphans.push(node.id.clone());
            }
        }
        let known = provenance.get(&node.id);
        let mut missing = Vec::new();
        if !known.is_some_and(|held| held.has_source) {
            missing.push("source".to_string());
        }
        if !known.is_some_and(|held| held.has_ingested_at) {
            missing.push("ingested_at".to_string());
        }
        if !missing.is_empty() {
            missing_frontmatter.push(MissingFrontmatter {
                page: node.id.clone(),
                missing,
            });
        }
        if let Some(targets) = mentioned.get(&seat) {
            // A link to the index or the log is navigation, not a relation a
            // key could name.
            let undeclared: Vec<String> = targets
                .iter()
                .filter(|to| !scaffold(**to as usize) && !declared.contains(&(seat, **to)))
                .map(|to| nodes[*to as usize].id.clone())
                .collect();
            if !undeclared.is_empty() {
                undeclared_relations.push(UndeclaredRelation {
                    page: node.id.clone(),
                    targets: undeclared,
                });
            }
        }
        if let Some(source) = node.source.as_deref().map(normalize_raw_path) {
            named_sources.insert(source);
        }
    }
    for held in provenance.values() {
        for mention in &held.raw_mentions {
            named_sources.insert(normalize_raw_path(mention));
        }
    }
    let unlogged_raw: Vec<String> = walk_raw(root)
        .into_iter()
        .filter(|item| !named_sources.contains(item))
        .collect();

    VaultLint {
        index_gaps,
        ghost_links: ghosts
            .into_iter()
            .map(|(target, from)| GhostLink {
                target,
                from: from.into_iter().collect(),
            })
            .collect(),
        orphans,
        missing_frontmatter,
        undeclared_relations,
        unlogged_raw,
        contradictions,
        superseded: superseded.into_iter().collect(),
        merge_candidates: None,
        counts: LintCounts::default(),
        findings: 0,
    }
    .settle()
}

/// `raw/x.md`, however a page spelled it: `./raw/x.md`, `raw/x.md`, or with a
/// trailing sentence mark the body's punctuation left on it.
fn normalize_raw_path(written: &str) -> String {
    written
        .trim()
        .trim_start_matches("./")
        .trim_end_matches(['.', ',', ';', ':', ')', ']'])
        .to_string()
}

/// Every `raw/…` path a body mentions, bounded by `limit` — normalised the
/// way a `source:` value is, so the two compare. A mention is the path as prose writes
/// it: after a space, a bracket, a quote or a backtick, up to the next space
/// or closing mark.
#[must_use]
pub fn raw_mentions(body: &str, limit: usize) -> Vec<String> {
    let needle = format!("{RAW_DIR}/");
    let mut held: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(at) = rest.find(&needle) {
        // `./raw/x.md` is the same path with a dot in front of it.
        let start = if rest[..at].ends_with("./") {
            at - 2
        } else {
            at
        };
        let before = rest[..start].chars().next_back();
        let tail = &rest[at..];
        let end = tail
            .find(|held: char| {
                held.is_whitespace()
                    || matches!(held, ')' | ']' | '`' | '"' | '\'' | '<' | '>' | '|')
            })
            .unwrap_or(tail.len());
        let opens = before.is_none_or(|held| {
            held.is_whitespace() || matches!(held, '(' | '[' | '`' | '"' | '\'' | '→' | '←')
        });
        if opens {
            let path = normalize_raw_path(&tail[..end]);
            if path.len() > needle.len() && !held.contains(&path) {
                held.push(path);
                if held.len() >= limit {
                    break;
                }
            }
        }
        rest = &tail[end.max(needle.len())..];
    }
    held
}

/// Directory names never entered under `raw/` — the same private folders the
/// wiki walk skips.
const SKIPPED_DIRS: [&str; 3] = [".obsidian", ".trash", ".git"];

/// Every file under `root/raw`, vault-relative with `/` separators, sorted,
/// without the README the setup wrote. Bounded by [`MAX_GRAPH_ENTRIES`] like
/// the wiki walk; past the bound the listing is a part of the inbox.
fn walk_raw(root: &Path) -> Vec<String> {
    let raw = root.join(RAW_DIR);
    let mut found = Vec::new();
    let mut pending = VecDeque::from([raw.clone()]);
    let mut visited = 0;
    'walk: while let Some(directory) = pending.pop_front() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_GRAPH_ENTRIES {
                break 'walk;
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
            if !kind.is_file() {
                continue;
            }
            let path = entry.path();
            let Ok(below) = path.strip_prefix(&raw) else {
                continue;
            };
            let mut id = String::from(RAW_DIR);
            for part in below.components() {
                id.push('/');
                let Some(part) = part.as_os_str().to_str() else {
                    continue;
                };
                id.push_str(part);
            }
            if id == RAW_README_FILE {
                continue;
            }
            found.push(id);
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::second_brain::WIKI_DIR;
    use crate::second_brain_graph::GraphCache;

    /// A vault set up the way the window sets one up, plus the pages and raw
    /// items named. Paths are vault-relative.
    fn vault(files: &[(&str, &str)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        crate::second_brain::setup(root.path()).unwrap();
        for (relative, body) in files {
            let path = root.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        root
    }

    fn lint_of(root: &Path) -> VaultLint {
        GraphCache::new().scan(root, false).lint
    }

    const FULL: &str = "---\nsource: raw/a.md\ningested_at: 2026-09-07T00:00:00+09:00\n---\n";

    #[test]
    fn a_set_up_vault_with_nothing_in_it_is_clean() {
        let root = vault(&[]);
        let lint = lint_of(root.path());
        assert!(lint.is_clean(), "{lint:?}");
        // The scan seats the merge candidates even in an empty vault: zero of
        // them, never "nobody computed it".
        assert_eq!(
            lint.counts,
            LintCounts {
                merge_candidates: Some(0),
                ..LintCounts::default()
            }
        );
        assert_eq!(lint.merge_candidates, Some(Vec::new()));
    }

    #[test]
    fn a_page_the_index_does_not_list_is_a_gap_and_an_unlinked_one_is_an_orphan() {
        let root = vault(&[
            ("wiki/index.md", "# Wiki\n\n- [[listed]]\n- [[hub]]\n"),
            ("wiki/listed.md", &format!("{FULL}listed\n")),
            (
                "wiki/hub.md",
                &format!("{FULL}reaches [[reached]] in prose\n"),
            ),
            ("wiki/reached.md", &format!("{FULL}reached\n")),
            ("wiki/alone.md", &format!("{FULL}alone\n")),
            // A journal line is not a reader's path to a page.
            ("wiki/log.md", "- 2026-09-07 00:00 — raw/a.md → [[alone]]\n"),
        ]);
        let lint = lint_of(root.path());
        assert_eq!(lint.index_gaps, ["wiki/alone.md", "wiki/reached.md"]);
        // `reached` has a page pointing at it; `alone` has only the log.
        assert_eq!(lint.orphans, ["wiki/alone.md"]);
        assert_eq!(lint.counts.index_gaps, 2);
        assert_eq!(lint.counts.orphans, 1);
    }

    #[test]
    fn a_ghost_names_every_page_that_wrote_it() {
        let root = vault(&[
            ("wiki/index.md", "- [[one]]\n- [[two]]\n"),
            ("wiki/one.md", &format!("{FULL}[[never]]\n")),
            (
                "wiki/two.md",
                &format!("{FULL}[[never|again]] and [[also-never]]\n"),
            ),
        ]);
        let lint = lint_of(root.path());
        assert_eq!(
            lint.ghost_links,
            [
                GhostLink {
                    target: "also-never".to_string(),
                    from: vec!["wiki/two.md".to_string()],
                },
                GhostLink {
                    target: "never".to_string(),
                    from: vec!["wiki/one.md".to_string(), "wiki/two.md".to_string()],
                },
            ]
        );
        assert_eq!(lint.counts.ghost_links, 2);
    }

    #[test]
    fn a_missing_key_names_itself_and_the_scaffolding_pages_are_never_asked_for_one() {
        let root = vault(&[
            ("wiki/index.md", "- [[bare]]\n- [[half]]\n- [[full]]\n"),
            ("wiki/bare.md", "no frontmatter\n"),
            ("wiki/half.md", "---\nsource: raw/a.md\n---\nhalf\n"),
            ("wiki/full.md", &format!("{FULL}full\n")),
        ]);
        let lint = lint_of(root.path());
        assert_eq!(
            lint.missing_frontmatter,
            [
                MissingFrontmatter {
                    page: "wiki/bare.md".to_string(),
                    missing: vec!["source".to_string(), "ingested_at".to_string()],
                },
                MissingFrontmatter {
                    page: "wiki/half.md".to_string(),
                    missing: vec!["ingested_at".to_string()],
                },
            ]
        );
    }

    #[test]
    fn a_prose_link_with_no_key_naming_it_is_undeclared_and_a_declared_one_is_not() {
        let root = vault(&[
            ("wiki/index.md", "- [[a]]\n- [[b]]\n- [[c]]\n"),
            (
                "wiki/a.md",
                "---\nsource: raw/a.md\ningested_at: 2026-09-07\nrelated: [[b]]\n---\n\
                 says [[b]] (declared) and [[c]] (not) and [[ghost]]; back to [[index]]\n",
            ),
            ("wiki/b.md", &format!("{FULL}b\n")),
            ("wiki/c.md", &format!("{FULL}c\n")),
        ]);
        let lint = lint_of(root.path());
        assert_eq!(
            lint.undeclared_relations,
            [UndeclaredRelation {
                page: "wiki/a.md".to_string(),
                targets: vec!["wiki/c.md".to_string()],
            }]
        );
        // The index links every page in prose and declares nothing; it is a
        // catalog, not a claim — and a page's link back to it is navigation.
        assert!(
            !lint
                .undeclared_relations
                .iter()
                .any(|held| held.page == WIKI_INDEX_FILE)
        );
        assert!(
            !lint
                .undeclared_relations
                .iter()
                .any(|held| held.targets.iter().any(|to| to == WIKI_INDEX_FILE))
        );
    }

    #[test]
    fn a_raw_item_is_logged_by_a_source_key_or_by_any_page_naming_its_path() {
        let root = vault(&[
            ("raw/by-source.md", "s\n"),
            ("raw/by-log.md", "l\n"),
            ("raw/by-prose.md", "p\n"),
            ("raw/nested/never.md", "n\n"),
            ("wiki/index.md", "- [[page]]\n"),
            (
                "wiki/page.md",
                "---\nsource: raw/by-source.md\ningested_at: 2026-09-07\n---\n\
                 the notes came from `raw/by-prose.md`.\n",
            ),
            (
                "wiki/log.md",
                "- 2026-09-07 00:00 — raw/by-log.md → [[page]]\n",
            ),
        ]);
        let lint = lint_of(root.path());
        assert_eq!(lint.unlogged_raw, ["raw/nested/never.md"]);
        assert_eq!(lint.counts.unlogged_raw, 1);
    }

    #[test]
    fn contradictions_and_superseded_pages_are_rows_that_never_fail_the_recipe() {
        let root = vault(&[
            ("wiki/index.md", "- [[new]]\n- [[old]]\n- [[other]]\n"),
            (
                "wiki/new.md",
                "---\nsource: raw/a.md\ningested_at: 2026-09-07\nsupersedes: [[old]]\ncontradicts: [[other]]\n---\n",
            ),
            (
                "wiki/other.md",
                "---\nsource: raw/a.md\ningested_at: 2026-09-07\nsupersedes: [[old]]\n---\n",
            ),
            ("wiki/old.md", &format!("{FULL}old\n")),
        ]);
        let lint = lint_of(root.path());
        assert_eq!(lint.contradictions, 1);
        // Two pages supersede one target: one superseded page.
        assert_eq!(lint.superseded, ["wiki/old.md"]);
        assert_eq!(lint.counts.superseded, 1);
        assert!(lint.is_clean(), "{lint:?}");
    }

    #[test]
    fn raw_mentions_read_paths_out_of_prose_and_leave_punctuation_behind() {
        assert_eq!(
            raw_mentions(
                "from raw/a.md, then (raw/b.md) and `raw/c.md`; see ./raw/d.md.\n\
                 not-a-mention:foo/raw/e.md — 2026-09-07 — raw/f.md → [[page]]",
                10
            ),
            ["raw/a.md", "raw/b.md", "raw/c.md", "raw/d.md", "raw/f.md"]
        );
        assert_eq!(
            raw_mentions("raw/ alone is not a path", 10),
            Vec::<String>::new()
        );
        assert_eq!(raw_mentions("raw/a.md raw/b.md raw/c.md", 2).len(), 2);
    }

    #[test]
    fn the_orphan_count_on_the_graph_is_the_table_s_orphan_count() {
        let root = vault(&[
            ("wiki/index.md", "- [[listed]]\n"),
            ("wiki/listed.md", &format!("{FULL}[[other]]\n")),
            ("wiki/other.md", &format!("{FULL}other\n")),
            ("wiki/alone.md", &format!("{FULL}alone\n")),
        ]);
        let graph = GraphCache::new().scan(root.path(), false);
        assert_eq!(graph.orphans, graph.lint.orphans.len());
        assert_eq!(graph.lint.orphans, ["wiki/alone.md"]);
    }

    /// The fixture the recipe and the window's test both read: one of each
    /// finding, and the expected table beside it. The Rust side pins the
    /// table; the browser side feeds the same file to the health card.
    #[test]
    fn the_fixture_vault_lints_to_exactly_its_expected_table() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/vault-lint");
        let text = fs::read_to_string(fixture.join("expected.json")).expect("expected.json");
        let expected: VaultLint =
            serde_json::from_str(&text).expect("expected.json parses as the lint table");
        let lint = lint_of(&fixture.join("vault"));
        assert_eq!(
            serde_json::to_value(&lint).unwrap(),
            serde_json::to_value(&expected).unwrap(),
            "the fixture vault and fixtures/vault-lint/expected.json disagree"
        );
        assert_eq!(lint.findings, 7);
        assert_eq!(lint.counts.contradictions, 1);
        assert_eq!(lint.counts.superseded, 1);
        // The merge seat is filled by the scan (`second_brain_live`), and a
        // candidate is a proposal: the findings do not move with it.
        assert!(
            lint.counts.merge_candidates.is_some(),
            "the scan seats the merge candidates"
        );
    }

    #[test]
    fn the_wiki_dir_constant_is_the_one_the_ids_carry() {
        // A regression guard for the id spelling every list depends on.
        assert!(WIKI_INDEX_FILE.starts_with(WIKI_DIR));
    }
}
