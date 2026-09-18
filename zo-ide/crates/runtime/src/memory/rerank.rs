//! Graph-safe reranking of recalled memory — the pure half.
//!
//! [`crate::memory::recall`] already answers a turn with the entries and vault
//! pages the query's words and the vault's own graph chose, in an order those
//! two settled. This module asks a System One model one more thing about that
//! answer — how much each recalled note helps with the request — and folds the
//! reply into a THIRD ordering axis behind the two recall already spent.
//!
//! Nothing here calls anything. It builds the state and the questions, checks a
//! reply against the questions it asked, reports what that reply would have
//! reordered ([`compare`]), and folds a reported order back into recall's hits
//! ([`apply_order`]) for the one mode allowed to act on it. The executor that
//! puts them on the wire, the setting that permits acting, and the ledger that
//! keeps the comparison are the tools crate's
//! (`docs/design/typesafe-judgment-expansion-20260917.md` §6).
//!
//! # The graph outranks the judgment
//!
//! Recall does not hand back a list of independent facts. It hands back a
//! reading of the vault's graph: a page another page replaced is folded to one
//! line naming its successor, two pages that disagree are marked on both lines
//! and placed side by side, and a page admitted by an edge carries the edge it
//! arrived on. Those are decisions code made from what a person wrote down, and
//! a probability does not overturn them:
//!
//! * a superseded page never rises above the page that replaced it;
//! * a contradicting pair is never split, because two answers to one question
//!   read as one open question only while they are adjacent;
//! * no id is invented and none is dropped — a judgment may only permute what
//!   recall already admitted;
//! * ties fall back to recall's own order, which already spells lexical
//!   evidence, then the boost axis, then the slug.
//!
//! A reply that breaks any rule of the contract is not partly used: the whole
//! judgment is discarded and recall's order stands. The two orders are reported
//! side by side ([`RerankComparison`]) so a reader can see how often the
//! judgment wanted what the graph refused. Under `smart.rerankShadow: on` the
//! graph-safe order is also what the turn reads — and [`apply_order`] folds it
//! only when it can prove the order a permutation of what recall admitted, so
//! the same discard rule holds at the moment it would change something.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use api::{SystemOneQuestion, SystemOneQuestionKind, SystemOneResponse, SystemOneScoreAnswer};
use core_types::text::truncate_on_char_boundary;
use core_types::MemoryHit;
use serde_json::{json, Value};

use super::recall::{wikilink_target, RECALL_CONTRADICTS_MARK, RECALL_SUPERSEDED_PREFIX};
use crate::model_router::PROBABILITY_SUM_TOLERANCE;

/// Bumped whenever a level description, the instructions or the state's shape
/// changes. A comparison row carries it, so a reading taken under other words
/// is never read as evidence about these ones.
pub const RERANK_RUBRIC_VERSION: u32 = 1;

/// The ordered levels of the one question asked about each note, lowest first.
///
/// Situations rather than degrees, because each level is judged on its own
/// against the state: the model is never shown a level's number or its
/// neighbours, so "more relevant than the last one" would say nothing.
pub const RERANK_LEVELS: [&str; 4] = [
    "The note is about some other subject; nothing in it bears on the request.",
    "The note shares background or vocabulary with the request but answers no part of it.",
    "The note answers part of the request, or names a file, symbol or decision the request needs.",
    "The note answers the request directly, or states a constraint or decision the work must follow.",
];

/// The top level's number: the upper end of the scale, and the divisor that
/// puts a reading on 0 to 1.
///
/// Spelled rather than derived, because deriving it means converting a length
/// to a float, and this scale is never worth a lossy conversion. The assert is
/// what keeps the two in step.
pub const RERANK_TOP_LEVEL: f64 = 3.0;
const _: () = assert!(
    RERANK_LEVELS.len() == 4,
    "RERANK_TOP_LEVEL is the last level's number"
);

/// Notes one judgment may be asked about. Recall renders at most
/// [`super::recall::MAX_RECALLED_ENTRIES`] and asks for a few more behind them
/// for the reminder block; past this the tail is not worth a question. This
/// and the two caps below are the Jev use table's recall row
/// (`zerocode_core::jev::RECALL`), the one place what each use sends is capped.
pub const MAX_RERANK_CANDIDATES: usize = zerocode_core::jev::RECALL_NOTE_CAP;

/// Bytes of one note's summary the state carries.
pub const RERANK_SUMMARY_MAX_BYTES: usize = zerocode_core::jev::RECALL_SUMMARY_BYTE_CAP;

/// Characters of the request the state carries, on a character boundary.
pub const RERANK_REQUEST_CHAR_CAP: usize = zerocode_core::jev::RECALL_REQUEST_CHAR_CAP;

/// How far a score may sit from the probability-weighted mean of its levels
/// before the answer is refused. The contract says the two are the same number;
/// this is the room two decimal places leave.
pub const SCORE_MEAN_TOLERANCE: f64 = 0.02;

/// Marker the state's truncations leave, so a clipped summary is visibly one —
/// the table's own, so the door's byte cap re-cutting a summary this already
/// cut leaves it byte for byte as it was.
const TRUNCATION_MARKER: &str = zerocode_core::jev::CUT_MARK;

/// One note put to the judgment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerankCandidate {
    /// The key this note's question and answer share. Never sent to the model —
    /// the contract says a question id is for code — so the instructions name
    /// the note by its path in the state instead.
    pub question_id: String,
    /// Where this note sits in the state's `notes` array, and in the recall
    /// order the comparison is against.
    pub position: usize,
    pub slug: String,
    /// The pointer summary as recall rendered it, capped. It already carries
    /// whatever the graph said about this note — the fold naming a successor,
    /// the contradiction mark, the edge a page arrived on — so the model reads
    /// the vault's own words rather than a second rendering of them.
    pub summary: String,
}

/// The notes recall's answer puts to a judgment, in recall's order.
///
/// A collapsed pointer — the line recall leaves where a body already sits in
/// the transcript — is dropped: it carries no claim to rate, and asking about
/// one would spend a question on the absence of text.
#[must_use]
pub fn rerank_candidates(hits: &[MemoryHit]) -> Vec<RerankCandidate> {
    hits.iter()
        .enumerate()
        .take(MAX_RERANK_CANDIDATES)
        .map(|(position, hit)| RerankCandidate {
            question_id: format!("n{position}"),
            position,
            slug: hit.entry.slug.clone(),
            summary: truncate_on_char_boundary(
                &hit.entry.summary,
                RERANK_SUMMARY_MAX_BYTES,
                TRUNCATION_MARKER,
            ),
        })
        .collect()
}

/// The one state every question in the batch reads.
///
/// Names and summaries only. A store path is where a file sits on this machine
/// and answers nothing about relevance, so it never leaves.
#[must_use]
pub fn rerank_state(request: &str, candidates: &[RerankCandidate]) -> Value {
    let request = match request.char_indices().nth(RERANK_REQUEST_CHAR_CAP) {
        Some((cut, _)) => &request[..cut],
        None => request,
    };
    json!({
        "request": request,
        "notes": candidates
            .iter()
            .map(|candidate| json!({"name": candidate.slug, "summary": candidate.summary}))
            .collect::<Vec<_>>(),
    })
}

/// One question per note, by the id its answer comes back under.
///
/// Independent judgments over one state, so they travel in a single request and
/// no note's rating can see another's. A question id is not sent to the model,
/// so each question names the note it rates by its path in the state.
#[must_use]
pub fn rerank_questions(candidates: &[RerankCandidate]) -> BTreeMap<String, SystemOneQuestion> {
    candidates
        .iter()
        .map(|candidate| {
            let instructions = format!(
                "How much does `notes[{}]` help with `request`?",
                candidate.position
            );
            (
                candidate.question_id.clone(),
                SystemOneQuestion::score(&instructions, RERANK_LEVELS),
            )
        })
        .collect()
}

/// One note's rating, checked against the question that asked for it.
#[derive(Debug, Clone, PartialEq)]
pub struct RerankReading {
    pub position: usize,
    pub slug: String,
    /// The position on the level scale, as answered.
    pub score: f64,
    /// The same reading on 0 to 1, so scales of different lengths compare.
    pub normalised: f64,
    pub confidence: f64,
}

/// Why a reply was thrown away. Every one of these discards the whole judgment:
/// a batch half-read would reorder some notes by the model and the rest by
/// recall, which is neither order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerankRejection {
    /// A note was asked about and not answered.
    MissingAnswer(String),
    /// An answer came back under an id nothing was asked under.
    UnknownAnswer(String),
    /// The answer is not shaped as a score answer.
    NotAScore(String),
    /// `probabilities` does not name exactly the levels that were offered.
    LevelKeys(String),
    /// A probability outside `[0, 1]`, or not a number at all.
    ProbabilityRange(String),
    /// The probabilities do not sum to one within [`PROBABILITY_SUM_TOLERANCE`].
    ProbabilitySum(String),
    /// `score` is missing, not finite, or outside the scale.
    ScoreRange(String),
    /// `score` is not the probability-weighted mean of the level numbers.
    ScoreMismatch(String),
    /// `confidence` outside `[0, 1]`.
    ConfidenceRange(String),
}

/// Check a reply against the questions [`rerank_questions`] asked.
///
/// The wire type says an answer is score-shaped; what it cannot say is that the
/// score is the one these levels were asked for, which is the rest of this.
///
/// # Errors
/// The first rule the reply breaks.
pub fn validate_rerank(
    candidates: &[RerankCandidate],
    response: &SystemOneResponse,
) -> Result<Vec<RerankReading>, RerankRejection> {
    let asked: BTreeSet<&str> = candidates
        .iter()
        .map(|candidate| candidate.question_id.as_str())
        .collect();
    // A note nobody asked about is the one failure that says the reply is not
    // about this batch at all, so it is checked before any answer is read.
    if let Some(stray) = response.answers.keys().find(|id| !asked.contains(id.as_str())) {
        return Err(RerankRejection::UnknownAnswer(stray.clone()));
    }
    candidates
        .iter()
        .map(|candidate| read_answer(candidate, response))
        .collect()
}

/// Whether an answer is about the levels that were offered: the same level
/// numbers in its spread and in the legend it echoes back.
fn names_the_levels(answer: &SystemOneScoreAnswer) -> bool {
    answer.probabilities.len() == RERANK_LEVELS.len()
        && answer.legend.len() == RERANK_LEVELS.len()
        && (0..RERANK_LEVELS.len()).all(|level| {
            let level = level.to_string();
            answer.probabilities.contains_key(&level) && answer.legend.contains_key(&level)
        })
}

fn read_answer(
    candidate: &RerankCandidate,
    response: &SystemOneResponse,
) -> Result<RerankReading, RerankRejection> {
    let id = &candidate.question_id;
    let answer = response
        .score_answer(id)
        .ok_or_else(|| RerankRejection::MissingAnswer(id.clone()))?
        .map_err(|_| RerankRejection::NotAScore(id.clone()))?;
    if answer.kind != SystemOneQuestionKind::Score {
        return Err(RerankRejection::NotAScore(id.clone()));
    }
    if !names_the_levels(&answer) {
        return Err(RerankRejection::LevelKeys(id.clone()));
    }
    let mut weighted = 0.0;
    let mut total = 0.0;
    // The level's own number, carried alongside rather than converted from the
    // index: the scale is short and a cast would be the only lossy step here.
    let mut number = 0.0;
    for level in 0..RERANK_LEVELS.len() {
        let probability = answer
            .probabilities
            .get(&level.to_string())
            .copied()
            .ok_or_else(|| RerankRejection::LevelKeys(id.clone()))?;
        if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
            return Err(RerankRejection::ProbabilityRange(id.clone()));
        }
        weighted += probability * number;
        total += probability;
        number += 1.0;
    }
    if (total - 1.0).abs() > PROBABILITY_SUM_TOLERANCE {
        return Err(RerankRejection::ProbabilitySum(id.clone()));
    }
    let top = RERANK_TOP_LEVEL;
    if !answer.score.is_finite() || !(0.0..=top).contains(&answer.score) {
        return Err(RerankRejection::ScoreRange(id.clone()));
    }
    // The contract says the score IS the weighted mean. Checking it is how a
    // reply built for some other scale, or rounded from one, is caught before
    // it orders anything.
    if (answer.score - weighted).abs() > SCORE_MEAN_TOLERANCE {
        return Err(RerankRejection::ScoreMismatch(id.clone()));
    }
    if !answer.confidence.is_finite() || !(0.0..=1.0).contains(&answer.confidence) {
        return Err(RerankRejection::ConfidenceRange(id.clone()));
    }
    Ok(RerankReading {
        position: candidate.position,
        slug: candidate.slug.clone(),
        score: answer.score,
        normalised: answer.score / top,
        confidence: answer.confidence,
    })
}

/// What a judgment would have done to recall's order, and what the graph
/// refused to let it do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerankComparison {
    /// Recall's own order — what a turn reads, in shadow and until a later
    /// phase says otherwise.
    pub recalled: Vec<String>,
    /// The judgment's order after the graph's rules are applied.
    pub proposed: Vec<String>,
    /// Notes whose place differs between the two orders.
    pub moved: usize,
    /// Whether the two disagree about which note comes first.
    pub top_changed: bool,
    /// Notes the graph's rules pinned — where the judgment alone would have put
    /// them somewhere else. This is the number a later phase has to read before
    /// letting a judgment reorder anything for real.
    pub held_by_graph: Vec<String>,
}

/// Fold a checked judgment into recall's order without breaking the graph.
///
/// `None` when the notes cannot be ordered at all — the vault declaring two
/// pages each other's successor is the one way that happens — in which case
/// recall's order is the answer and nothing is reported as moved.
#[must_use]
pub fn compare(hits: &[MemoryHit], readings: &[RerankReading]) -> Option<RerankComparison> {
    let scores = scores_by_position(hits.len(), readings)?;
    let constraints = graph_constraints(hits);
    let proposed = graph_safe_order(hits.len(), &scores, &constraints)?;
    let ungoverned = judgment_only_order(hits.len(), &scores);

    let slug = |position: usize| hits[position].entry.slug.clone();
    let recalled: Vec<String> = (0..hits.len()).map(slug).collect();
    let held_by_graph = proposed
        .iter()
        .zip(&ungoverned)
        .filter(|(governed, alone)| governed != alone)
        .map(|(governed, _)| slug(*governed))
        .collect();
    let moved = proposed
        .iter()
        .enumerate()
        .filter(|(place, position)| place != &**position)
        .count();
    Some(RerankComparison {
        top_changed: proposed.first() != Some(&0),
        recalled,
        proposed: proposed.into_iter().map(slug).collect(),
        moved,
        held_by_graph,
    })
}

/// Every note's normalised reading by its place in recall's answer, or `None`
/// when the readings do not cover exactly those notes.
fn scores_by_position(hits: usize, readings: &[RerankReading]) -> Option<Vec<f64>> {
    let mut scores = vec![f64::NAN; hits];
    for reading in readings {
        let slot = scores.get_mut(reading.position)?;
        if !slot.is_nan() {
            return None;
        }
        *slot = reading.normalised;
    }
    scores.iter().all(|score| score.is_finite()).then_some(scores)
}

/// What the graph decided about these notes, read from the marks recall wrote
/// onto them.
#[derive(Debug, Default)]
struct GraphConstraints {
    /// A note, and the note that replaced it. The successor comes first.
    superseded_by: BTreeMap<usize, usize>,
    /// A note, and the note it disagrees with. The two travel together.
    contradicts: BTreeMap<usize, usize>,
}

fn graph_constraints(hits: &[MemoryHit]) -> GraphConstraints {
    let by_slug: BTreeMap<&str, usize> = hits
        .iter()
        .enumerate()
        .map(|(position, hit)| (hit.entry.slug.as_str(), position))
        .collect();
    let mut constraints = GraphConstraints::default();
    for (position, hit) in hits.iter().enumerate() {
        let summary = hit.entry.summary.as_str();
        if let Some(rest) = summary.strip_prefix(RECALL_SUPERSEDED_PREFIX) {
            // The fold's own opening bracket is already eaten by the prefix.
            if let Some(successor) = rest.find("]]").map(|end| &rest[..end]) {
                if let Some(successor) = by_slug.get(successor) {
                    if *successor != position {
                        constraints.superseded_by.insert(position, *successor);
                    }
                }
            }
            continue;
        }
        // The mark sits after the summary's own text, which may carry links of
        // its own, so the partner is the first link AFTER the mark.
        if let Some(at) = summary.find(RECALL_CONTRADICTS_MARK) {
            let after = &summary[at + RECALL_CONTRADICTS_MARK.len()..];
            if let Some(other) = wikilink_target(after).and_then(|slug| by_slug.get(slug)) {
                if *other != position {
                    constraints.contradicts.insert(position, *other);
                }
            }
        }
    }
    constraints
}

/// One unit of the reordering: a note, or a contradicting pair that moves as
/// one.
struct Group {
    members: Vec<usize>,
    /// The strongest reading in the group. A pair travels on its better half:
    /// it is one open question, not two answers to be split.
    key: f64,
    /// Where the group sits in recall's order, for the tie-break.
    first: usize,
}

/// Recall's order, reordered by the judgment, with the graph's rules kept.
fn graph_safe_order(
    hits: usize,
    scores: &[f64],
    constraints: &GraphConstraints,
) -> Option<Vec<usize>> {
    let mut group_of = vec![usize::MAX; hits];
    let mut groups: Vec<Group> = Vec::new();
    for position in 0..hits {
        if group_of[position] != usize::MAX {
            continue;
        }
        let mut members = vec![position];
        if let Some(partner) = constraints.contradicts.get(&position) {
            if group_of[*partner] == usize::MAX {
                members.push(*partner);
            }
        }
        let key = members
            .iter()
            .map(|member| scores[*member])
            .fold(f64::MIN, f64::max);
        for member in &members {
            group_of[*member] = groups.len();
        }
        groups.push(Group {
            first: position,
            members,
            key,
        });
    }

    // A note may only follow the note that replaced it.
    let mut after: Vec<Vec<usize>> = vec![Vec::new(); groups.len()];
    for (superseded, successor) in &constraints.superseded_by {
        let (late, early) = (group_of[*superseded], group_of[*successor]);
        if late != early {
            after[late].push(early);
        }
    }

    let mut emitted = vec![false; groups.len()];
    let mut order = Vec::with_capacity(hits);
    for _ in 0..groups.len() {
        let mut best: Option<usize> = None;
        for (index, group) in groups.iter().enumerate() {
            if emitted[index] || after[index].iter().any(|earlier| !emitted[*earlier]) {
                continue;
            }
            best = match best {
                Some(held) if !stronger(group, &groups[held]) => Some(held),
                _ => Some(index),
            };
        }
        // Nothing free to place means the vault named two pages each other's
        // successor. That is a question for a person, not an order to invent.
        let chosen = best?;
        emitted[chosen] = true;
        order.extend_from_slice(&groups[chosen].members);
    }
    Some(order)
}

/// Better reading first; recall's own order breaks a tie, because it already
/// spells lexical evidence, then the boost axis, then the slug.
fn stronger(candidate: &Group, against: &Group) -> bool {
    match candidate.key.partial_cmp(&against.key) {
        Some(Ordering::Greater) => true,
        Some(Ordering::Equal) => candidate.first < against.first,
        _ => false,
    }
}

/// Recall's hits in the order a judgment settled on — the one place a judgment
/// is allowed to change what a turn reads.
///
/// `proposed` is [`RerankComparison::proposed`]: the judged head, by name,
/// after the graph has had its say. The head is the notes [`rerank_candidates`]
/// built a question for — that same rule, so the two can never disagree about
/// which notes were asked about — and the tail behind it was never asked, so it
/// keeps recall's order.
///
/// `None` unless `proposed` is exactly that head's names in some order. The
/// contract says a judgment may only permute what recall already admitted; an
/// order that cannot be PROVED a permutation is one this refuses to fold, and
/// the caller then reads recall's order. Two hits recalled under one name
/// cannot be told apart, so they cannot be proved either.
#[must_use]
pub fn apply_order(hits: &[MemoryHit], proposed: &[String]) -> Option<Vec<MemoryHit>> {
    let judged = hits.len().min(MAX_RERANK_CANDIDATES);
    if judged == 0 || proposed.len() != judged {
        return None;
    }
    let head = &hits[..judged];
    let mut by_slug: BTreeMap<&str, usize> = BTreeMap::new();
    for (position, hit) in head.iter().enumerate() {
        if by_slug.insert(hit.entry.slug.as_str(), position).is_some() {
            return None;
        }
    }
    let mut taken = vec![false; head.len()];
    let mut read = Vec::with_capacity(hits.len());
    for name in proposed {
        let position = *by_slug.get(name.as_str())?;
        // A name the order spells twice would read one note twice and drop
        // another, which is the same broken promise as inventing an id.
        if std::mem::replace(&mut taken[position], true) {
            return None;
        }
        read.push(head[position].clone());
    }
    read.extend_from_slice(&hits[judged..]);
    Some(read)
}

/// The order the judgment alone would have produced, with no graph rule
/// applied — the control [`RerankComparison::held_by_graph`] is measured
/// against.
fn judgment_only_order(hits: usize, scores: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..hits).collect();
    order.sort_by(|left, right| {
        scores[*right]
            .partial_cmp(&scores[*left])
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.cmp(right))
    });
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(slug: &str, summary: &str) -> MemoryHit {
        MemoryHit {
            entry: core_types::MemoryEntry {
                slug: slug.to_string(),
                path: format!("/Users/someone/.zo/projects/p/memory/{slug}.md"),
                summary: summary.to_string(),
            },
            score: 0,
        }
    }

    /// Each level's number, spelled for the same reason [`RERANK_TOP_LEVEL`] is.
    const LEVEL_NUMBERS: [f64; RERANK_LEVELS.len()] = [0.0, 1.0, 2.0, 3.0];

    /// One way a reply breaks the contract: what to spoil in a well-formed
    /// answer, and the refusal that has to come back.
    type BrokenCase = (fn(&mut Value), RerankRejection);

    /// A score answer whose probability sits entirely on one level, with the
    /// legend the contract echoes back.
    fn answer(level: usize) -> Value {
        let probabilities: serde_json::Map<String, Value> = (0..RERANK_LEVELS.len())
            .map(|index| {
                (
                    index.to_string(),
                    json!(if index == level { 1.0 } else { 0.0 }),
                )
            })
            .collect();
        let legend: serde_json::Map<String, Value> = (0..RERANK_LEVELS.len())
            .map(|index| (index.to_string(), json!(RERANK_LEVELS[index])))
            .collect();
        json!({
            "type": "score",
            "score": LEVEL_NUMBERS[level],
            "confidence": 1.0,
            "legend": legend,
            "probabilities": probabilities,
        })
    }

    fn reply(levels: &[usize]) -> SystemOneResponse {
        SystemOneResponse {
            model: "jev-test".to_string(),
            answers: levels
                .iter()
                .enumerate()
                .map(|(position, level)| (format!("n{position}"), answer(*level)))
                .collect(),
            usage: api::SystemOneUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        }
    }

    fn readings(hits: &[MemoryHit], levels: &[usize]) -> Vec<RerankReading> {
        let candidates = rerank_candidates(hits);
        validate_rerank(&candidates, &reply(levels)).expect("valid reply")
    }

    #[test]
    fn the_state_carries_names_and_summaries_and_never_a_store_path() {
        let hits = [hit("wiki/a", "a claim"), hit("wiki/b", "another claim")];
        let candidates = rerank_candidates(&hits);
        let state = rerank_state("왜 이렇게 되었나", &candidates);
        let rendered = state.to_string();

        assert!(rendered.contains("wiki/a") && rendered.contains("another claim"));
        assert!(
            !rendered.contains("/Users/"),
            "a store path answers nothing about relevance and must not leave: {rendered}"
        );
    }

    #[test]
    fn each_question_names_the_note_it_rates_because_its_id_is_not_sent() {
        let hits = [hit("wiki/a", "one"), hit("wiki/b", "two")];
        let candidates = rerank_candidates(&hits);
        let questions = rerank_questions(&candidates);

        assert_eq!(questions.len(), 2);
        assert_eq!(questions["n0"].instructions, "How much does `notes[0]` help with `request`?");
        assert_eq!(questions["n1"].instructions, "How much does `notes[1]` help with `request`?");
        for question in questions.values() {
            assert_eq!(question.kind, SystemOneQuestionKind::Score);
            let api::SystemOneCriteria::Ordered(levels) = &question.criteria else {
                panic!("a relevance question rates along ordered levels");
            };
            assert!(
                levels.iter().map(String::as_str).eq(RERANK_LEVELS),
                "every question spends the one levels table, in its order"
            );
        }
    }

    #[test]
    fn a_reply_that_breaks_the_contract_is_discarded_whole() {
        let hits = [hit("wiki/a", "one"), hit("wiki/b", "two")];
        let candidates = rerank_candidates(&hits);
        // Each case takes one well-formed answer and breaks exactly one rule.
        let broken: [BrokenCase; 6] = [
            (
                |answer| answer["type"] = json!("choice"),
                RerankRejection::NotAScore("n1".into()),
            ),
            (
                |answer| answer["probabilities"] = json!({"0": 0.0, "1": 1.0}),
                RerankRejection::LevelKeys("n1".into()),
            ),
            (
                |answer| answer["probabilities"]["1"] = json!(0.5),
                RerankRejection::ProbabilitySum("n1".into()),
            ),
            (
                |answer| answer["score"] = json!(9.0),
                RerankRejection::ScoreRange("n1".into()),
            ),
            (
                |answer| answer["score"] = json!(3.0),
                RerankRejection::ScoreMismatch("n1".into()),
            ),
            (
                |answer| answer["confidence"] = json!(4.0),
                RerankRejection::ConfidenceRange("n1".into()),
            ),
        ];
        for (break_one_rule, expected) in broken {
            let mut batch = reply(&[3, 1]);
            let mut answer = batch.answers["n1"].clone();
            break_one_rule(&mut answer);
            batch.answers.insert("n1".to_string(), answer);
            assert_eq!(validate_rerank(&candidates, &batch), Err(expected));
        }

        let mut missing = reply(&[3, 1]);
        missing.answers.remove("n1");
        assert_eq!(
            validate_rerank(&candidates, &missing),
            Err(RerankRejection::MissingAnswer("n1".into()))
        );
    }

    #[test]
    fn a_note_nobody_asked_about_is_never_ordered() {
        let hits = [hit("wiki/a", "one")];
        let candidates = rerank_candidates(&hits);
        let mut batch = reply(&[3]);
        batch.answers.insert("n7".to_string(), answer(3));

        assert_eq!(
            validate_rerank(&candidates, &batch),
            Err(RerankRejection::UnknownAnswer("n7".into())),
            "an id nothing was asked under says the reply is not about this batch"
        );
    }

    #[test]
    fn the_judgment_reorders_what_recall_admitted_and_nothing_else() {
        let hits = [
            hit("wiki/a", "barely on topic"),
            hit("wiki/b", "the answer"),
            hit("wiki/c", "background"),
        ];
        let comparison = compare(&hits, &readings(&hits, &[0, 3, 1])).expect("an order");

        assert_eq!(comparison.proposed, ["wiki/b", "wiki/c", "wiki/a"]);
        assert_eq!(comparison.recalled, ["wiki/a", "wiki/b", "wiki/c"]);
        assert!(comparison.top_changed);
        assert_eq!(comparison.moved, 3);
        assert!(comparison.held_by_graph.is_empty());
    }

    #[test]
    fn recalls_order_breaks_a_tie_the_judgment_could_not() {
        let hits = [
            hit("wiki/a", "one"),
            hit("wiki/b", "two"),
            hit("wiki/c", "three"),
        ];
        let comparison = compare(&hits, &readings(&hits, &[2, 2, 2])).expect("an order");

        assert_eq!(
            comparison.proposed,
            ["wiki/a", "wiki/b", "wiki/c"],
            "equal readings leave lexical evidence, the boost axis and the slug deciding"
        );
        assert_eq!(comparison.moved, 0);
    }

    #[test]
    fn a_superseded_page_never_rises_above_the_page_that_replaced_it() {
        // Recall folds the replaced page's whole summary to this one line.
        let hits = [
            hit("wiki/new", "the current decision"),
            hit("wiki/old", "superseded by [[wiki/new]]"),
        ];
        // The judgment likes the old page more — the graph still refuses.
        let comparison = compare(&hits, &readings(&hits, &[1, 3])).expect("an order");

        assert_eq!(comparison.proposed, ["wiki/new", "wiki/old"]);
        assert_eq!(
            comparison.held_by_graph,
            ["wiki/new", "wiki/old"],
            "both places differ from what the judgment alone wanted"
        );
        assert!(!comparison.top_changed);
    }

    #[test]
    fn a_contradicting_pair_is_never_split() {
        let hits = [
            hit("wiki/one", "the claim · ⚠ contradicts [[wiki/two]] (older)"),
            hit("wiki/two", "the other claim · ⚠ contradicts [[wiki/one]] (newer)"),
            hit("wiki/apart", "unrelated but well liked"),
        ];
        // The judgment would put the unrelated page between the two halves.
        let comparison = compare(&hits, &readings(&hits, &[3, 1, 2])).expect("an order");

        assert_eq!(
            comparison.proposed,
            ["wiki/one", "wiki/two", "wiki/apart"],
            "two answers to one question read as one open question only while adjacent"
        );
        assert_eq!(
            comparison.held_by_graph,
            ["wiki/two", "wiki/apart"],
            "the judgment alone would have placed the unrelated page second"
        );
    }

    #[test]
    fn a_partner_named_by_a_summary_that_carries_its_own_links_is_still_found() {
        let hits = [
            hit("wiki/a", "reads [[wiki/elsewhere]] · ⚠ contradicts [[wiki/b]] (newer)"),
            hit("wiki/b", "the other half"),
            hit("wiki/c", "liked most"),
        ];
        let comparison = compare(&hits, &readings(&hits, &[1, 1, 3])).expect("an order");

        assert_eq!(
            comparison.proposed,
            ["wiki/c", "wiki/a", "wiki/b"],
            "the partner is the first link AFTER the mark, not the first link in the line"
        );
    }

    #[test]
    fn a_mark_naming_a_page_that_did_not_come_back_orders_nothing() {
        let hits = [
            hit("wiki/a", "a claim · ⚠ contradicts [[wiki/absent]] (newer)"),
            hit("wiki/b", "liked most"),
        ];
        let comparison = compare(&hits, &readings(&hits, &[0, 3])).expect("an order");

        assert_eq!(comparison.proposed, ["wiki/b", "wiki/a"]);
        assert!(comparison.held_by_graph.is_empty());
    }

    #[test]
    fn two_pages_each_naming_the_other_their_successor_leave_recalls_order_alone() {
        let hits = [
            hit("wiki/a", "superseded by [[wiki/b]]"),
            hit("wiki/b", "superseded by [[wiki/a]]"),
        ];

        assert_eq!(
            compare(&hits, &readings(&hits, &[3, 0])),
            None,
            "a vault that contradicts itself is a question for a person, not an order to invent"
        );
    }

    #[test]
    fn a_reading_that_does_not_cover_the_notes_orders_nothing() {
        let hits = [hit("wiki/a", "one"), hit("wiki/b", "two")];
        let mut partial = readings(&hits, &[3, 1]);
        partial.pop();
        assert_eq!(compare(&hits, &partial), None);

        let mut doubled = readings(&hits, &[3, 1]);
        doubled[1].position = 0;
        assert_eq!(compare(&hits, &doubled), None);
    }

    #[test]
    fn the_scale_normalises_so_a_reading_is_comparable_across_rubrics() {
        let hits = [hit("wiki/a", "one")];
        let reading = &readings(&hits, &[3])[0];

        assert!((reading.score - 3.0).abs() < f64::EPSILON);
        assert!((reading.normalised - 1.0).abs() < f64::EPSILON);
        assert!(
            (RERANK_TOP_LEVEL - 3.0).abs() < f64::EPSILON && RERANK_LEVELS.len() == 4,
            "the levels and the scale's upper end are one table"
        );
    }

    #[test]
    fn only_the_notes_one_judgment_may_be_asked_about_are_asked_about() {
        let hits: Vec<MemoryHit> = (0..MAX_RERANK_CANDIDATES + 3)
            .map(|index| hit(&format!("wiki/p{index}"), "a claim"))
            .collect();

        assert_eq!(rerank_candidates(&hits).len(), MAX_RERANK_CANDIDATES);
    }

    fn slugs(hits: &[MemoryHit]) -> Vec<String> {
        hits.iter().map(|hit| hit.entry.slug.clone()).collect()
    }

    fn names(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn the_fold_reads_the_judgments_order_and_carries_every_hit_whole() {
        let hits = [
            hit("wiki/a", "barely on topic"),
            hit("wiki/b", "the answer"),
            hit("wiki/c", "background"),
        ];
        let comparison = compare(&hits, &readings(&hits, &[0, 3, 1])).expect("an order");

        let read = apply_order(&hits, &comparison.proposed).expect("a permutation folds");

        assert_eq!(slugs(&read), comparison.proposed);
        assert_eq!(
            read[0], hits[1],
            "a fold moves a hit, it does not rebuild one"
        );
    }

    /// Only the head is judged when recall admitted more notes than one
    /// judgment may be asked about; the tail keeps recall's order behind it.
    #[test]
    fn notes_past_the_judged_head_keep_recalls_order_behind_it() {
        let hits: Vec<MemoryHit> = (0..MAX_RERANK_CANDIDATES + 2)
            .map(|index| hit(&format!("wiki/p{index}"), "a claim"))
            .collect();
        let head = &hits[..MAX_RERANK_CANDIDATES];
        let mut levels = vec![0usize; MAX_RERANK_CANDIDATES];
        levels[MAX_RERANK_CANDIDATES - 1] = 3;
        let comparison = compare(head, &readings(head, &levels)).expect("an order");

        let read = apply_order(&hits, &comparison.proposed).expect("a head permutation folds");

        assert_eq!(read.len(), hits.len());
        assert_eq!(read[0].entry.slug, format!("wiki/p{}", MAX_RERANK_CANDIDATES - 1));
        assert_eq!(
            slugs(&read[MAX_RERANK_CANDIDATES..]),
            slugs(&hits[MAX_RERANK_CANDIDATES..]),
            "the unjudged tail is not reordered and not dropped"
        );
    }

    /// The contract is that a judgment may only PERMUTE what recall admitted.
    /// A fold that cannot prove that of the order it was handed does not
    /// happen, so recall's order stands.
    #[test]
    fn an_order_that_is_not_a_permutation_folds_nothing() {
        let hits = [hit("wiki/a", "one"), hit("wiki/b", "two"), hit("wiki/c", "three")];

        for (order, why) in [
            (names(&["wiki/b", "wiki/a"]), "a note recall admitted was dropped"),
            (names(&["wiki/b", "wiki/a", "wiki/z"]), "a note recall never admitted"),
            (names(&["wiki/b", "wiki/b", "wiki/a"]), "one note put in twice"),
            (Vec::new(), "no order at all"),
            (
                names(&["wiki/a", "wiki/b", "wiki/c", "wiki/c"]),
                "an order longer than the notes",
            ),
        ] {
            assert_eq!(apply_order(&hits, &order), None, "{why}");
        }
    }

    /// Two notes recall returned under one name cannot be told apart, so the
    /// order cannot be proved a permutation of them either.
    #[test]
    fn a_name_recall_returned_twice_folds_nothing() {
        let hits = [hit("wiki/a", "one"), hit("wiki/a", "again"), hit("wiki/b", "two")];

        assert_eq!(apply_order(&hits, &names(&["wiki/b", "wiki/a", "wiki/a"])), None);
    }

    #[test]
    fn an_order_that_agrees_with_recall_is_recalls_order() {
        let hits = [hit("wiki/a", "one"), hit("wiki/b", "two")];

        let read = apply_order(&hits, &slugs(&hits)).expect("a permutation folds");

        assert_eq!(read, hits);
    }
}
