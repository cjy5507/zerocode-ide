//! Candidate and proposal rules for the vault's record-only pair review.
//! Selection and interpretation are pure; the Jev door and ledger live in zo.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::second_brain::{WIKI_INDEX_FILE, WIKI_LOG_FILE};
use crate::second_brain_graph::{EdgeKind, GraphNode, NodeKind, VaultGraph};
use crate::second_brain_related::tokenize;

/// The number of summary neighbours retained per page before deduplication.
pub const BM25_NEIGHBOURS: usize = 5;
/// BM25's term-frequency saturation (`k1`) and length normalisation (`b`),
/// the Robertson defaults; a summary is a token SET here, so every present
/// term counts once ([`BM25_BINARY_TF`]).
pub const BM25_K1: f64 = 1.2;
pub const BM25_B: f64 = 0.75;
pub const BM25_BINARY_TF: f64 = 1.0;
/// Bound one review run to the estimated first-run budget.
pub const PAIR_LIMIT: usize = 2_800;
/// New pairs a routine weekly review will pay to classify.
pub const WEEKLY_PAIR_LIMIT: usize = 500;
/// A title candidate needs this much Jaccard overlap, in percent.
pub const TITLE_OVERLAP_PERCENT: usize = 30;
/// Private derived suggestions beside a vault, never under wiki/ or raw/.
pub const PROPOSALS_FILE: &str = ".zo/vault-pair-proposals.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pair {
    pub left: String,
    pub right: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalRecord {
    pub left: String,
    pub right: String,
    pub reason: String,
    pub left_modified_ms: i64,
    pub right_modified_ms: i64,
    pub suggestion: Suggestion,
}

/// The latest response is usable only while both source pages are unchanged.
#[must_use]
pub fn valid_proposals(root: &Path, graph: &VaultGraph) -> Vec<ProposalRecord> {
    let path = root.join(PROPOSALS_FILE);
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let Ok(held) = serde_json::from_slice::<Vec<ProposalRecord>>(&bytes) else {
        return Vec::new();
    };
    held.into_iter()
        .filter(|record| {
            let left = graph.nodes.iter().find(|node| node.id == record.left);
            let right = graph.nodes.iter().find(|node| node.id == record.right);
            matches!((left, right), (Some(a), Some(b))
            if page(a) && page(b)
                && a.modified_ms == record.left_modified_ms
                && b.modified_ms == record.right_modified_ms)
        })
        .collect()
}

/// Replace only this derived snapshot, leaving every page and raw source as it
/// was. The caller folds old valid answers with the newly answered pairs.
pub fn save_proposals(root: &Path, rows: &[ProposalRecord]) -> std::io::Result<()> {
    let path = root.join(PROPOSALS_FILE);
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::other("no parent"));
    };
    fs::create_dir_all(parent)?;
    // The derived cache must not be redirected into wiki/ or raw/.
    if fs::symlink_metadata(parent)?.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "proposal directory is a symbolic link",
        ));
    }
    crate::second_brain_live::write_atomically(
        &path,
        &serde_json::to_vec_pretty(rows).map_err(std::io::Error::other)?,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Suggestion {
    Merge,
    Related,
    Supersedes,
    Contradicts,
    None,
}

impl Suggestion {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Related => "related",
            Self::Supersedes => "supersedes",
            Self::Contradicts => "contradicts",
            Self::None => "none",
        }
    }

    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        [
            Self::Merge,
            Self::Related,
            Self::Supersedes,
            Self::Contradicts,
            Self::None,
        ]
        .into_iter()
        .find(|suggestion| suggestion.word() == word)
    }
}

/// The response remains a proposal. The human decides whether either page
/// or its frontmatter should change.
#[must_use]
pub fn suggest(score: f64, same_claim: f64, opposite_claim: f64, replaces: f64) -> Suggestion {
    use crate::jev::{
        VAULT_PAIR_OPPOSITE_FLOOR_PERMILLE, VAULT_PAIR_REPLACES_FLOOR_PERMILLE,
        VAULT_PAIR_SAME_FLOOR_PERMILLE,
    };
    let above = |probability: f64, floor: u16| {
        (0.0..=1.0).contains(&probability) && probability > f64::from(floor) / 1_000.0
    };
    if above(opposite_claim, VAULT_PAIR_OPPOSITE_FLOOR_PERMILLE) {
        Suggestion::Contradicts
    } else if above(replaces, VAULT_PAIR_REPLACES_FLOOR_PERMILLE) {
        Suggestion::Supersedes
    } else if score.round() >= 2.0 && above(same_claim, VAULT_PAIR_SAME_FLOOR_PERMILLE) {
        Suggestion::Merge
    } else if score.round() >= 1.0 {
        Suggestion::Related
    } else {
        Suggestion::None
    }
}

fn page(node: &GraphNode) -> bool {
    node.kind == NodeKind::Page && node.id != WIKI_INDEX_FILE && node.id != WIKI_LOG_FILE
}

fn shared_tag(a: &GraphNode, b: &GraphNode) -> bool {
    a.tags.iter().any(|tag| b.tags.contains(tag))
}

fn title_overlap(a: &GraphNode, b: &GraphNode) -> bool {
    let a = tokenize(&a.title);
    let b = tokenize(&b.title);
    let union = a.union(&b).count();
    union != 0 && a.intersection(&b).count() * 100 >= union * TITLE_OVERLAP_PERCENT
}

/// Title and BM25 summary neighbours, sorted and bounded. Existing declared
/// relations are removed before any page text is prepared for a request.
#[must_use]
pub fn candidates(graph: &VaultGraph) -> Vec<Pair> {
    let pages: Vec<usize> = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| page(node))
        .map(|(i, _)| i)
        .collect();
    let linked: HashSet<(usize, usize)> = graph
        .edges
        .iter()
        .filter(|edge| {
            matches!(
                edge.kind,
                EdgeKind::Related | EdgeKind::Supersedes | EdgeKind::Contradicts
            )
        })
        .map(|edge| {
            (
                (edge.from as usize).min(edge.to as usize),
                (edge.from as usize).max(edge.to as usize),
            )
        })
        .collect();
    let tokens: Vec<BTreeSet<String>> = pages
        .iter()
        .map(|&i| tokenize(&graph.nodes[i].excerpt))
        .collect();
    let lengths: Vec<f64> = tokens.iter().map(|tokens| tokens.len() as f64).collect();
    let average = (lengths.iter().sum::<f64>() / lengths.len().max(1) as f64).max(1.0);
    let mut document_frequency: HashMap<&str, usize> = HashMap::new();
    for doc in &tokens {
        for token in doc {
            *document_frequency.entry(token).or_default() += 1;
        }
    }
    let mut found: BTreeMap<(usize, usize), &str> = BTreeMap::new();
    for (i, &left) in pages.iter().enumerate() {
        for &right in pages.iter().skip(i + 1) {
            let key = (left, right);
            if !linked.contains(&key)
                && shared_tag(&graph.nodes[left], &graph.nodes[right])
                && title_overlap(&graph.nodes[left], &graph.nodes[right])
            {
                found.insert(key, "title");
            }
        }
    }
    for (i, &left) in pages.iter().enumerate() {
        let mut neighbours = Vec::new();
        for (j, &right) in pages.iter().enumerate() {
            if i == j || linked.contains(&(left.min(right), left.max(right))) {
                continue;
            }
            let score = tokens[i]
                .intersection(&tokens[j])
                .map(|term| {
                    let count = *document_frequency.get(term.as_str()).unwrap_or(&0) as f64;
                    let idf = ((pages.len() as f64 - count + 0.5) / (count + 0.5) + 1.0).ln();
                    let tf = BM25_BINARY_TF;
                    idf * tf * (BM25_K1 + 1.0)
                        / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * lengths[j] / average))
                })
                .sum::<f64>();
            if score > 0.0 {
                neighbours.push((right, score));
            }
        }
        neighbours.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (right, _) in neighbours.into_iter().take(BM25_NEIGHBOURS) {
            found
                .entry((left.min(right), left.max(right)))
                .or_insert("bm25");
        }
    }
    found
        .into_iter()
        .take(PAIR_LIMIT)
        .map(|((left, right), reason)| Pair {
            left: graph.nodes[left].id.clone(),
            right: graph.nodes[right].id.clone(),
            reason: reason.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::second_brain_graph::GraphCache;

    #[test]
    fn score_and_nouls_leave_a_none_outlet_and_make_only_proposals() {
        assert_eq!(suggest(0.0, 0.0, 0.0, 0.0), Suggestion::None);
        assert_eq!(suggest(2.0, 0.9, 0.1, 0.1), Suggestion::Merge);
        assert_eq!(suggest(1.0, 0.1, 0.9, 0.1), Suggestion::Contradicts);
        assert_eq!(suggest(1.0, 0.1, 0.1, 0.9), Suggestion::Supersedes);
    }

    #[test]
    fn candidates_exclude_declared_pairs_and_a_snapshot_never_edits_pages() {
        let vault = tempfile::tempdir().expect("vault");
        let wiki = vault.path().join("wiki");
        fs::create_dir(&wiki).expect("wiki");
        let raw = vault.path().join("raw");
        fs::create_dir(&raw).expect("raw");
        let source = raw.join("source.md");
        fs::write(&source, "An original source.\n").expect("source");
        fs::write(wiki.join("index.md"), "# Index\n").expect("index");
        fs::write(wiki.join("log.md"), "# Log\n").expect("log");
        let first = wiki.join("topic-one.md");
        let second = wiki.join("topic-two.md");
        let third = wiki.join("different.md");
        let a = "---\ntitle: Topic One\ntags: [example]\n---\n\nA shared measurement of the same idea.\n";
        let b = "---\ntitle: Topic Two\ntags: [example]\n---\n\nA shared measurement of the same idea.\n";
        fs::write(&first, a).expect("first");
        fs::write(&second, b).expect("second");
        fs::write(&third, "---\ntitle: Different\nrelated: [[topic-one]]\n---\n\nA shared measurement of the same idea.\n").expect("third");
        let mut cache = GraphCache::new();
        let graph = cache.scan(vault.path(), false);
        let pairs = candidates(&graph);
        assert!(pairs.iter().any(
            |pair| pair.left.ends_with("topic-one.md") && pair.right.ends_with("topic-two.md")
        ));
        assert!(!pairs.iter().any(
            |pair| pair.left.ends_with("different.md") && pair.right.ends_with("topic-one.md")
        ));
        let pair = pairs
            .iter()
            .find(|pair| {
                pair.left.ends_with("topic-one.md") && pair.right.ends_with("topic-two.md")
            })
            .expect("pair");
        let left = graph
            .nodes
            .iter()
            .find(|node| node.id == pair.left)
            .expect("left");
        let right = graph
            .nodes
            .iter()
            .find(|node| node.id == pair.right)
            .expect("right");
        save_proposals(
            vault.path(),
            &[ProposalRecord {
                left: pair.left.clone(),
                right: pair.right.clone(),
                reason: pair.reason.clone(),
                left_modified_ms: left.modified_ms,
                right_modified_ms: right.modified_ms,
                suggestion: Suggestion::Merge,
            }],
        )
        .expect("snapshot");
        assert_eq!(fs::read_to_string(&first).expect("first remains"), a);
        assert_eq!(fs::read_to_string(&second).expect("second remains"), b);
        assert_eq!(
            fs::read_to_string(&source).expect("raw remains"),
            "An original source.\n"
        );
        let rescanned = cache.scan(vault.path(), false);
        assert!(
            rescanned
                .lint
                .merge_candidates
                .as_ref()
                .is_some_and(|pairs| pairs
                    .iter()
                    .any(|pair| pair.proposal.as_deref() == Some("merge")))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_proposal_snapshot_cannot_follow_a_temporary_link_into_raw() {
        let vault = tempfile::tempdir().expect("vault");
        let source = vault.path().join("source.md");
        fs::write(&source, "immutable source").expect("source");
        let path = vault.path().join(PROPOSALS_FILE);
        fs::create_dir_all(path.parent().expect("parent")).expect("cache directory");
        std::os::unix::fs::symlink(&source, path.with_extension("tmp")).expect("link");
        let _ = save_proposals(vault.path(), &[]);
        assert_eq!(
            fs::read_to_string(source).expect("source remains"),
            "immutable source"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_proposal_snapshot_refuses_a_cache_directory_linked_into_raw() {
        let vault = tempfile::tempdir().expect("vault");
        let raw = vault.path().join("raw");
        fs::create_dir(&raw).expect("raw");
        let parent = vault
            .path()
            .join(PROPOSALS_FILE)
            .parent()
            .expect("parent")
            .to_path_buf();
        std::os::unix::fs::symlink(&raw, parent).expect("linked directory");
        assert!(save_proposals(vault.path(), &[]).is_err());
        assert_eq!(fs::read_dir(raw).expect("raw unchanged").count(), 0);
    }

    #[test]
    #[ignore = "reads the vault named by ZEROCODE_SECOND_BRAIN for a local measurement"]
    fn measure_candidates_on_this_vault() {
        let root =
            std::env::var_os(crate::second_brain::VAULT_ENV).expect("set ZEROCODE_SECOND_BRAIN");
        let graph = GraphCache::new().scan(Path::new(&root), false);
        let pairs = candidates(&graph);
        let title = pairs.iter().filter(|pair| pair.reason == "title").count();
        let bm25 = pairs.len() - title;
        eprintln!(
            "pages={} candidates={} title={} bm25={}",
            graph.pages,
            pairs.len(),
            title,
            bm25
        );
        assert!(pairs.len() <= PAIR_LIMIT);
    }
}
