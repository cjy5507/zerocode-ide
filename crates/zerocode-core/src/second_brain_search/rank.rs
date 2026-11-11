//! Which files a few words point at, in one layer.
//!
//! The arithmetic is `second_brain_related`'s — a word few files hold counts
//! for more, a long file does not win by holding more words, and a match
//! chance would explain is left out. What this module adds is what it is
//! counted over: a file's name counts [`NAME_WEIGHT`] times its text, and the
//! one passage that holds the most of what was asked counts again at
//! [`PASSAGE_WEIGHT`], because words that meet in one passage say more than
//! the same words scattered over a file.
//!
//! Then the files the best answers declare a relation to are brought in
//! ([`NEIGHBOR_RULE`]): a page that answers in words nobody asked with is
//! invisible to the words, and the vault said so itself by writing the key.
//!
//! Ranking reads a [`Corpus`], not an index file, so the same code answers
//! from the index on disk and from files read whole — and a test holds the
//! two to the same answer.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;

use super::segment::Damaged;
use crate::second_brain_graph::EdgeKind;
use crate::second_brain_related::{chance_bar, field_score, rarity_of, surprise_of};

/// What a word in a file's name counts for beside the same word in its text.
/// Twice: the name was written to say what the file is about.
pub const NAME_WEIGHT: f64 = 2.0;

/// What a file's best passage adds, as a share of the words it holds. Chosen
/// on the questions set apart for choosing — the 136's `audit` and `heldout`
/// and the first half of the 49 — and only then read on the rest.
pub const PASSAGE_WEIGHT: f64 = 0.2;

/// How the files a best answer is related to are brought in beside it. One
/// table: zo's session recall reads the same numbers (`memory::recall`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeighborRule {
    /// How many of the best answers are widened from.
    pub seeds: usize,
    /// Neighbours one answer brings per relation; `contradicts` is unbounded.
    pub per_kind: usize,
    /// What share of an answer's score each relation carries to a neighbour,
    /// in thousandths, strongest first.
    pub decay: [(EdgeKind, i64); 4],
    /// What a plain link to or from the best answer adds to an equal score.
    pub mention_boost: i64,
    /// The most the links pointing at a page add to an equal score.
    pub hub_prior_max: i64,
}

/// The numbers zo's session recall settled on, measured on its 49-question
/// fixture vault. An answer holding five of the words asked scored 5,000
/// there, and each share is set by what the neighbour it brings must beat.
/// Every relation clears a page sharing two of the words (2,000).
/// `contradicts` comes in nearly beside its answer (4,500), because a reader
/// who misses a disagreement acts on half a fact; `related`, the claim
/// written most freely, only just clears the two-word page (2,250) and never
/// a page sharing three (3,000).
pub const NEIGHBOR_RULE: NeighborRule = NeighborRule {
    seeds: 3,
    per_kind: 2,
    decay: [
        (EdgeKind::Contradicts, 900),
        (EdgeKind::DependsOn, 700),
        (EdgeKind::Implements, 600),
        (EdgeKind::Related, 450),
    ],
    mention_boost: 1,
    hub_prior_max: 3,
};

/// What [`NeighborRule::decay`] is a share of.
const DECAY_WHOLE: f64 = 1_000.0;

/// One passage that holds a word: its file, its row, and whether it is the
/// file's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Held {
    pub doc: u32,
    pub passage: u32,
    pub name: bool,
}

/// One relation between two files of a layer, by their numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Edge {
    pub from: u32,
    pub to: u32,
    pub kind: EdgeKind,
}

/// What ranking reads of a layer.
pub(super) trait Corpus {
    /// File rows, standing or not: a file's number is below this.
    fn docs(&self) -> u32;
    /// Passage rows: a passage's number is below this.
    fn passages(&self) -> u32;
    /// Files that stand — what a word's rarity is counted against.
    fn standing(&self) -> u32;
    /// The different words of every standing file, summed.
    fn total_length(&self) -> u64;
    /// The different words of one file.
    fn length(&self, doc: u32) -> u32;
    /// One file's passages after its name.
    fn body_of(&self, doc: u32) -> Range<u32>;
    /// What two files with one score are ordered by.
    fn path(&self, doc: u32) -> &str;
    /// The relations between its files, in path order and then the order a
    /// file wrote them. Empty for a layer whose files declare none.
    fn edges(&self) -> &[Edge];
    /// The passages of standing files that hold `word`, a file's together and
    /// its name first.
    fn holders(&mut self, word: &str) -> Result<Vec<Held>, Damaged>;
}

/// The relation a file was brought in by, and the answer at its other end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Arrival {
    pub kind: EdgeKind,
    pub seed: u32,
    /// Whether the answer declared the relation, or the file brought in did.
    pub from_seed: bool,
}

/// One file of the answer.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Ranked {
    pub doc: u32,
    pub score: f64,
    pub(super) boost: i64,
    /// The passage that holds the most of what was asked, when one holds any.
    pub passage: Option<u32>,
    pub arrival: Option<Arrival>,
}

/// What one file holds of the words asked, each an index into them.
#[derive(Default)]
struct Holding {
    in_name: Vec<usize>,
    /// Words its text holds and its name does not.
    in_text: Vec<usize>,
}

/// The `limit` files `asked` points at, best first.
pub(super) fn rank(
    corpus: &mut impl Corpus,
    asked: &BTreeSet<String>,
    limit: usize,
) -> Result<Vec<Ranked>, Damaged> {
    let pages = corpus.standing();
    if pages == 0 || asked.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let mut holders = Vec::with_capacity(asked.len());
    let mut held_by = Vec::with_capacity(asked.len());
    for word in asked {
        let held = corpus.holders(word)?;
        let mut files = 0_u32;
        let mut last = None;
        for one in &held {
            if last != Some(one.doc) {
                files += 1;
                last = Some(one.doc);
            }
        }
        holders.push(files);
        held_by.push(held);
    }

    let mut holdings: Vec<Holding> = (0..corpus.docs()).map(|_| Holding::default()).collect();
    let mut worth = vec![0.0_f64; corpus.passages() as usize];
    for (word, held) in held_by.iter().enumerate() {
        let rarity = rarity_of(pages, holders[word]);
        for one in held {
            let holding = &mut holdings[one.doc as usize];
            if one.name {
                holding.in_name.push(word);
            } else {
                worth[one.passage as usize] += rarity;
                // A file's passages come together and its name first, so the
                // word is at the end of a list when this file already has it.
                if holding.in_name.last() != Some(&word) && holding.in_text.last() != Some(&word) {
                    holding.in_text.push(word);
                }
            }
        }
    }

    let average = (corpus.total_length() as f64 / f64::from(pages)).max(1.0);
    let bar = chance_bar(pages);
    let mut found = Vec::new();
    for (doc, holding) in (0_u32..).zip(&holdings) {
        if holding.in_name.is_empty() && holding.in_text.is_empty() {
            continue;
        }
        let surprise: f64 = holding
            .in_name
            .iter()
            .chain(&holding.in_text)
            .map(|&word| surprise_of(pages, holders[word]))
            .sum();
        if surprise < bar {
            continue;
        }
        let relative = f64::from(corpus.length(doc)) / average;
        let (passage, best) = best_passage(corpus.body_of(doc), &worth);
        let score = NAME_WEIGHT * field_score(&holding.in_name, &holders, pages, relative)
            + field_score(&holding.in_text, &holders, pages, relative)
            + PASSAGE_WEIGHT * best;
        found.push(Ranked {
            doc,
            score,
            boost: 0,
            passage,
            arrival: None,
        });
    }

    sort(&mut found, corpus);
    if !corpus.edges().is_empty() {
        widen(&mut found, corpus.edges(), corpus.docs());
        sort(&mut found, corpus);
    }
    found.truncate(limit);
    Ok(found)
}

/// The first passage of `body` worth the most, and what it is worth.
fn best_passage(body: Range<u32>, worth: &[f64]) -> (Option<u32>, f64) {
    let mut best = (None, 0.0);
    for passage in body {
        let held = worth[passage as usize];
        if held > best.1 {
            best = (Some(passage), held);
        }
    }
    best
}

/// Best first; an equal score goes to the page the vault points at more, and
/// then to the earlier path, so one question has one answer.
fn sort(found: &mut [Ranked], corpus: &impl Corpus) {
    found.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.boost.cmp(&left.boost))
            .then_with(|| corpus.path(left.doc).cmp(corpus.path(right.doc)))
    });
}

/// Brings in what the best answers are related to ([`NEIGHBOR_RULE`]).
///
/// A neighbour's score is the answer's, at the share its relation carries,
/// on top of whatever its own words earned. Over several answers it is the
/// largest, not the sum: a page three answers point at is one answer reached
/// three ways. A relation counts from either end, the answer's own first.
fn widen(found: &mut Vec<Ranked>, edges: &[Edge], docs: u32) {
    let rule = &NEIGHBOR_RULE;
    let seeds: Vec<(u32, f64)> = found
        .iter()
        .take(rule.seeds)
        .map(|one| (one.doc, one.score))
        .collect();
    let mut arrivals: HashMap<u32, (f64, Arrival)> = HashMap::new();
    for &(seed, score) in &seeds {
        let mut named: HashSet<u32> = HashSet::new();
        for (kind, share) in rule.decay {
            let own = edges
                .iter()
                .filter(|edge| edge.kind == kind && edge.from == seed && edge.to != seed)
                .map(|edge| (edge.to, true));
            let others = edges
                .iter()
                .filter(|edge| edge.kind == kind && edge.to == seed && edge.from != seed)
                .map(|edge| (edge.from, false));
            let mut taken = 0;
            for (neighbor, from_seed) in own.chain(others) {
                if !named.insert(neighbor) {
                    continue;
                }
                // A page that says the answer is wrong is the one neighbour a
                // reader cannot afford to have crowded out.
                if kind != EdgeKind::Contradicts {
                    if taken >= rule.per_kind {
                        continue;
                    }
                    taken += 1;
                }
                let value = score * share as f64 / DECAY_WHOLE;
                if arrivals
                    .get(&neighbor)
                    .is_none_or(|(best, _)| *best < value)
                {
                    arrivals.insert(
                        neighbor,
                        (
                            value,
                            Arrival {
                                kind,
                                seed,
                                from_seed,
                            },
                        ),
                    );
                }
            }
        }
    }

    // Plain links count from the best answer alone, and only between pages
    // the words already found.
    let mentioned: HashSet<u32> = seeds.first().map_or_else(HashSet::new, |&(best, _)| {
        edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Mentions)
            .filter_map(|edge| match (edge.from == best, edge.to == best) {
                (true, false) => Some(edge.to),
                (false, true) => Some(edge.from),
                _ => None,
            })
            .collect()
    });
    let pointed_at = pointed_at(edges, docs);
    let hub = |doc: u32| match pointed_at[doc as usize] {
        0 => 0,
        pages => i64::from(pages.ilog2()).min(rule.hub_prior_max),
    };
    for one in found.iter_mut() {
        if let Some((value, _)) = arrivals.remove(&one.doc) {
            one.score += value;
        }
        one.boost = hub(one.doc)
            + if mentioned.contains(&one.doc) {
                rule.mention_boost
            } else {
                0
            };
    }
    for (doc, (score, arrival)) in arrivals {
        found.push(Ranked {
            doc,
            score,
            boost: hub(doc),
            passage: None,
            arrival: Some(arrival),
        });
    }
}

/// How many different pages point at each page.
fn pointed_at(edges: &[Edge], docs: u32) -> Vec<u32> {
    let mut pairs: Vec<(u32, u32)> = edges.iter().map(|edge| (edge.to, edge.from)).collect();
    pairs.sort_unstable();
    pairs.dedup();
    let mut count = vec![0_u32; docs as usize];
    for (to, _) in pairs {
        count[to as usize] += 1;
    }
    count
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Files held in memory and read whole on every question: no index, no
    /// hash, nothing remembered. What the index's answers are held to.
    pub(in super::super) struct Whole {
        /// Per file: its path, the words of its name, the words of each passage.
        pub files: Vec<(String, BTreeSet<String>, Vec<BTreeSet<String>>)>,
        pub edges: Vec<Edge>,
    }

    impl Whole {
        fn first_passage(&self, doc: u32) -> u32 {
            self.files[..doc as usize]
                .iter()
                .map(|(_, _, passages)| u32::try_from(passages.len()).unwrap() + 1)
                .sum()
        }
    }

    impl Corpus for Whole {
        fn docs(&self) -> u32 {
            u32::try_from(self.files.len()).unwrap()
        }
        fn passages(&self) -> u32 {
            self.first_passage(self.docs())
        }
        fn standing(&self) -> u32 {
            self.docs()
        }
        fn total_length(&self) -> u64 {
            (0..self.docs())
                .map(|doc| u64::from(self.length(doc)))
                .sum()
        }
        fn length(&self, doc: u32) -> u32 {
            let (_, name, passages) = &self.files[doc as usize];
            let all: BTreeSet<&String> = name.iter().chain(passages.iter().flatten()).collect();
            u32::try_from(all.len()).unwrap()
        }
        fn body_of(&self, doc: u32) -> Range<u32> {
            self.first_passage(doc) + 1..self.first_passage(doc + 1)
        }
        fn path(&self, doc: u32) -> &str {
            &self.files[doc as usize].0
        }
        fn edges(&self) -> &[Edge] {
            &self.edges
        }
        fn holders(&mut self, word: &str) -> Result<Vec<Held>, Damaged> {
            let mut held = Vec::new();
            for (doc, (_, name, passages)) in (0_u32..).zip(&self.files) {
                let first = self.first_passage(doc);
                if name.contains(word) {
                    held.push(Held {
                        doc,
                        passage: first,
                        name: true,
                    });
                }
                for (at, passage) in (1_u32..).zip(passages) {
                    if passage.contains(word) {
                        held.push(Held {
                            doc,
                            passage: first + at,
                            name: false,
                        });
                    }
                }
            }
            Ok(held)
        }
    }

    fn words(text: &str) -> BTreeSet<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    fn whole(files: &[(&str, &str, &[&str])]) -> Whole {
        Whole {
            files: files
                .iter()
                .map(|(path, name, passages)| {
                    (
                        (*path).to_string(),
                        words(name),
                        passages.iter().map(|text| words(text)).collect(),
                    )
                })
                .collect(),
            edges: Vec::new(),
        }
    }

    fn paths(corpus: &mut Whole, edges: &[Edge], asked: &str, limit: usize) -> Vec<String> {
        corpus.edges = edges.to_vec();
        rank(corpus, &words(asked), limit)
            .unwrap()
            .iter()
            .map(|one| corpus.path(one.doc).to_string())
            .collect()
    }

    /// The same word in a name outranks it in a text, and a file that holds
    /// none of the words is not an answer.
    #[test]
    fn a_word_in_the_name_counts_for_more_than_the_same_word_in_the_text() {
        let mut held = whole(&[
            ("text.md", "notes", &["the crane lifts cargo"]),
            ("name.md", "crane", &["a short note"]),
            ("other.md", "tide", &["the table by the door"]),
        ]);
        assert_eq!(paths(&mut held, &[], "crane", 5), ["name.md", "text.md"]);
    }

    /// Two files hold both words. The one where they meet in one passage
    /// comes first, and that passage is the one it answers with.
    #[test]
    fn words_that_meet_in_one_passage_outrank_the_same_words_apart() {
        let mut held = whole(&[
            ("apart.md", "a", &["the crane stands", "cargo waits here"]),
            ("meet.md", "b", &["an empty line", "the crane lifts cargo"]),
        ]);
        let found = rank(&mut held, &words("crane cargo"), 5).unwrap();
        assert_eq!(held.path(found[0].doc), "meet.md");
        // meet.md is the second file: name 3, passages 4 and 5.
        assert_eq!(found[0].passage, Some(5));
        assert_eq!(held.path(found[1].doc), "apart.md");
    }

    /// A file found by its name alone has no passage to answer with.
    #[test]
    fn a_file_found_by_its_name_alone_answers_with_no_passage() {
        let mut held = whole(&[("name.md", "crane", &["a short note"])]);
        let found = rank(&mut held, &words("crane"), 5).unwrap();
        assert_eq!(found[0].passage, None);
    }

    /// In a layer large enough to tell, one common word is what chance would
    /// give many files; a word one file holds is an answer.
    #[test]
    fn a_match_chance_would_explain_is_left_out() {
        let mut files: Vec<(String, BTreeSet<String>, Vec<BTreeSet<String>>)> = (0..80)
            .map(|at| {
                let word = if at < 7 { "common" } else { "filler" };
                (format!("p{at:02}.md"), words(word), vec![])
            })
            .collect();
        files.push(("unique.md".to_string(), words("singular"), vec![]));
        let mut held = Whole {
            files,
            edges: Vec::new(),
        };
        assert!(paths(&mut held, &[], "common", 5).is_empty());
        assert_eq!(paths(&mut held, &[], "singular", 5), ["unique.md"]);
    }

    /// The answer's `depends_on` page arrives with none of the words, behind
    /// the answer and ahead of a weaker match, and says how it came.
    #[test]
    fn a_page_the_best_answer_depends_on_arrives_without_the_words() {
        let mut held = whole(&[
            ("answer.md", "crane cargo", &["the crane lifts cargo"]),
            ("base.md", "foundation", &["concrete and steel"]),
            (
                "weak.md",
                "notes",
                &["a crane far away and much else besides in a long text"],
            ),
        ]);
        held.edges = vec![Edge {
            from: 0,
            to: 1,
            kind: EdgeKind::DependsOn,
        }];
        let found = rank(&mut held, &words("crane cargo"), 5).unwrap();
        let order: Vec<&str> = found.iter().map(|one| held.path(one.doc)).collect();
        assert_eq!(order, ["answer.md", "base.md", "weak.md"]);
        assert_eq!(
            found[1].arrival,
            Some(Arrival {
                kind: EdgeKind::DependsOn,
                seed: 0,
                from_seed: true,
            })
        );
        assert_eq!(found[0].arrival, None);
    }

    /// One relation brings two pages at most; `contradicts` brings every one.
    #[test]
    fn a_relation_brings_two_pages_and_contradicts_brings_all() {
        let mut held = whole(&[
            ("answer.md", "crane", &[]),
            ("r1.md", "one", &[]),
            ("r2.md", "two", &[]),
            ("r3.md", "three", &[]),
            ("c1.md", "four", &[]),
            ("c2.md", "five", &[]),
            ("c3.md", "six", &[]),
        ]);
        let edge = |to, kind| Edge { from: 0, to, kind };
        let edges = [
            edge(1, EdgeKind::Related),
            edge(2, EdgeKind::Related),
            edge(3, EdgeKind::Related),
            edge(4, EdgeKind::Contradicts),
            edge(5, EdgeKind::Contradicts),
            edge(6, EdgeKind::Contradicts),
        ];
        assert_eq!(
            paths(&mut held, &edges, "crane", 10),
            ["answer.md", "c1.md", "c2.md", "c3.md", "r1.md", "r2.md"]
        );
    }

    /// Two neighbours the relation scores alike: the one more pages point at
    /// comes first.
    #[test]
    fn equal_neighbours_are_ordered_by_how_many_pages_point_at_them() {
        let mut held = whole(&[
            ("answer.md", "crane", &[]),
            ("a-quiet.md", "one", &[]),
            ("b-cited.md", "two", &[]),
            ("x.md", "three", &[]),
            ("y.md", "four", &[]),
        ]);
        let edges = [
            Edge {
                from: 0,
                to: 1,
                kind: EdgeKind::Related,
            },
            Edge {
                from: 0,
                to: 2,
                kind: EdgeKind::Related,
            },
            Edge {
                from: 3,
                to: 2,
                kind: EdgeKind::Mentions,
            },
            Edge {
                from: 4,
                to: 2,
                kind: EdgeKind::Mentions,
            },
        ];
        assert_eq!(
            paths(&mut held, &edges, "crane", 3),
            ["answer.md", "b-cited.md", "a-quiet.md"]
        );
    }
}
