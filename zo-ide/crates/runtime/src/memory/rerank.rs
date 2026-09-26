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
//! * no id is invented, and a note is dropped only on the one ground below —
//!   a judgment may otherwise only permute what recall already admitted;
//! * a note the graph said anything about is never dropped, whatever the
//!   judgment thinks of it, because that judgment is already made;
//! * ties fall back to recall's own order, which already spells lexical
//!   evidence, then the boost axis, then the slug.
//!
//! A reply that breaks any rule of the contract is not partly used: the whole
//! judgment is discarded and recall's order stands. The two orders are reported
//! side by side ([`RerankComparison`]) so a reader can see how often the
//! judgment wanted what the graph refused. Under `smart.rerankShadow: on` the
//! graph-safe order is also what the turn reads — and [`apply_order`] folds it
//! only when it can account for every note recall admitted, so the same discard
//! rule holds at the moment it would change something.
//!
//! # The bottom level is not injected
//!
//! The one ground for removing a note: the judgment put it on the bottom level
//! of [`RERANK_LEVELS`] — *the note is about some other subject; nothing in it
//! bears on the request* — and the graph said nothing about it. That note is
//! not reordered, it is left out of what the turn reads
//! ([`RerankComparison::dropped`]).
//!
//! Removing rather than only reordering, because **a judgment that may only
//! permute has a ceiling.** The notes it is shown are the judged head and the
//! notes a turn reads are the rendered few at the front of it; a rerank can
//! neither bring in a note recall did not admit nor send one away. So the whole
//! of what perfect ordering can buy is the turns that have a useful page inside
//! the head but outside the rendered front — and on a recall whose head holds
//! nothing that bears on the request, it buys exactly nothing: the same useless
//! pages, in a better order.
//!
//! How big that ceiling is, on this machine's vault: of 362 labelled lines of
//! the vault's own log, 284 already have a labelled page inside the rendered
//! five and 302 inside the judged eight (`memory::recall`'s
//! `recall_miss_against_the_vaults_own_log`). Five points is the CEILING on
//! reordering, not a result — and both counts are *at least one labelled page*
//! read off prefixes of a single deep call rather than the retriever re-run at
//! each depth, so they size the ceiling rather than measure it.
//!
//! What the readings say about the other end: of 580 notes this machine's
//! ledger has judged across 74 distinct recalls, 26.0% read as the bottom level
//! and another 54.7% as *shares background or vocabulary but answers no part of
//! the request* — and the recall section is a prompt that tells an agent to open
//! those pages first. On 5 of those 74 recalls the bottom level is every note
//! there was. So the fix is a floor under the results, not a better order and
//! not a wider net.
//!
//! Those readings are what this rule is made of, so they cannot also score it:
//! what a drop is worth is a comparison of turns, which nothing here claims to
//! have made. What the ledger CAN be replayed for — what leaves, and what the
//! graph keeps — is `rerank_shadow`'s `what_the_bottom_level_drop_removes`.
//!
//! Only the bottom level, and only unspoken-for notes. The level above it —
//! shared background — may well be worth a turn's attention, and which of the
//! two a person is better off with is a question for a measured comparison
//! rather than for this rule.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use api::{SystemOneQuestion, SystemOneResponse};
// The grid the wire answers on, from the one crate both programs read: the
// window derives its own screen judgments' tolerances from the same rounding,
// and this fact spelled in two places is two facts that can disagree. Only
// the tests read it directly now — `crate::jev_score` derives every bound
// from it, and they check those bounds against it.
#[cfg(test)]
use zerocode_core::jev::ANSWER_STEP as SYSTEMONE_ANSWER_STEP;
use crate::jev_score::{read_score, ScoreRule};
use core_types::text::truncate_on_char_boundary;
use core_types::MemoryHit;
use serde_json::{json, Value};

use super::recall::{wikilink_target, RECALL_CONTRADICTS_MARK, RECALL_SUPERSEDED_PREFIX};

/// Bumped whenever a level description, the instructions or the state's shape
/// changes. A comparison row carries it, so a reading taken under other words
/// is never read as evidence about these ones. The number is the seat's
/// row's own (t-6877), read from the table and not respelled.
pub const RERANK_RUBRIC_VERSION: u32 = zerocode_core::jev::questions::RECALL_RUBRIC_VERSION;

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

/// How many levels there are, and the level numbers added up. Spelled beside
/// [`RERANK_TOP_LEVEL`] for its reason, and held to the table by its assert.
///
/// Only the tests read them now: what the bounds are derived from is the
/// scale itself (`crate::jev_score::Scale`), and these are the numbers that
/// derivation is checked against.
#[cfg(test)]
const RERANK_LEVEL_COUNT: f64 = 4.0;
#[cfg(test)]
const RERANK_LEVEL_NUMBER_SUM: f64 = 6.0;
const _: () = assert!(
    RERANK_LEVELS.len() == 4,
    "RERANK_TOP_LEVEL is the last level's number, RERANK_LEVEL_COUNT is how many \
     there are, and RERANK_LEVEL_NUMBER_SUM is 0 + 1 + 2 + 3"
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

/// The most the wire's rounding can have moved any one number an answer
/// carries: half of `zerocode_core::jev::ANSWER_STEP`. The shared reader
/// derives every bound from it (`crate::jev_score`); here it is what a test
/// checks those bounds against.
#[cfg(test)]
const WIRE_ROUNDING: f64 = SYSTEMONE_ANSWER_STEP / 2.0;

/// The midpoint between the bottom level's number and the next one. Spelled
/// beside [`RERANK_TOP_LEVEL`] for its reason: the levels are numbered 0, 1, 2,
/// 3, so half of the first step is half of one.
const HALF_A_LEVEL: f64 = 0.5;

/// A reading below this reads as the bottom level — the one reading a judgment
/// may act on by removing a note rather than moving it.
///
/// The shared cut (`crate::jev_score::BOTTOM_LEVEL_CUT`): half a level is
/// where conventional rounding puts the boundary, and the cut sits half a
/// wire step below it so no number the wire can spell lands ON it. At 0.495
/// every grid value up to 0.49 is the bottom level and 0.50 is not, and a
/// reading that made the round trip through [`RerankReading::normalised`] and
/// back cannot have moved across it either.
pub const RERANK_BOTTOM_LEVEL_CUT: f64 = crate::jev_score::BOTTOM_LEVEL_CUT;
const _: () = assert!(
    RERANK_BOTTOM_LEVEL_CUT < HALF_A_LEVEL,
    "the cut sits below the midpoint between the bottom level and the next"
);

/// Whether a reading on the level scale is the bottom level's: *the note is
/// about some other subject; nothing in it bears on the request*.
///
/// Takes the reading on the scale the levels are numbered on, so a reading kept
/// normalised — as a ledger row keeps it — is multiplied by
/// [`RERANK_TOP_LEVEL`] back onto that scale first.
#[must_use]
pub fn reads_as_bottom_level(score: f64) -> bool {
    score < RERANK_BOTTOM_LEVEL_CUT
}

/// How far the level probabilities may sum from one before the answer is
/// refused, and how far a score may sit from the probability-weighted mean of
/// its levels.
///
/// Both are the scale's own numbers now, derived once for every seat that
/// asks a score question (`crate::jev_score::Scale`): four rounded
/// probabilities can put their sum 4 × 0.005 from one, and the level numbers
/// 0 + 1 + 2 + 3 multiply the same rounding into the mean, each with one more
/// half-step so the bound never lands on a distance the grid spells exactly.
/// The numbers this scale derives are the ones measured here against
/// `jev-1.13.0` on 2026-09-18 — 0.025 and 0.035 — and a test in that module
/// holds them there.
///
/// It was guessed at 0.02 before, which is a distance the grid lands on
/// exactly. Of 240 answers `jev-1.13.0` gave that day, 38 sat exactly there,
/// and the comparison admitted 17 of them and refused 21; two more sat at
/// 0.03. That refused 23 answers in 240, and because one batch asks about
/// eight notes and one refusal discards the batch whole, 17 batches in 30.
pub const RERANK_SCALE: crate::jev_score::Scale<'static> = crate::jev_score::Scale::new(&RERANK_LEVELS);

/// Marker the state's truncations leave, so a clipped summary is visibly one —
/// the table's own, so the door's byte cap re-cutting a summary this already
/// cut leaves it byte for byte as it was.
const TRUNCATION_MARKER: &str = zerocode_core::jev::CUT_MARK;

/// The letter a question id opens with, so what comes back is named rather
/// than numbered. Spelled once: [`rerank_candidates`] writes ids with it and
/// [`RerankRejection::position`] reads them back through it.
const QUESTION_ID_PREFIX: &str = "n";

/// The note a question id names, by its place in the state, or `None` when the
/// id is not one this module wrote.
fn position_of(question_id: &str) -> Option<usize> {
    question_id.strip_prefix(QUESTION_ID_PREFIX)?.parse().ok()
}

/// The state's keys, in the order the fingerprint reads them: the person's
/// request, and the notes put to the judgment.
const RERANK_STATE_KEYS: [&str; 2] = ["request", "notes"];

/// The keys of one note in `notes`, in the order the fingerprint reads them.
const RERANK_NOTE_KEYS: [&str; 2] = ["name", "summary"];

/// The question one note is rated under. It names the note by its path in
/// the state, because a question id is never sent; spelled once, for the
/// questions and for the words the version is pinned to ([`rubric_words`]).
fn rerank_instructions(position: usize) -> String {
    format!("How much does `notes[{position}]` help with `request`?")
}

/// The words the recall seat asks, as one string: the question one note is
/// rated under, the four levels, and the keys the state and each note carry.
/// [`RERANK_RUBRIC_VERSION`] is pinned to it, so a word changed without a
/// version is a red test rather than a quiet drift (t-9469).
#[must_use]
pub fn rubric_words() -> String {
    let mut words = vec![rerank_instructions(0)];
    words.extend(RERANK_LEVELS.iter().map(|level| (*level).to_string()));
    words.push(RERANK_STATE_KEYS.join(","));
    words.push(RERANK_NOTE_KEYS.join(","));
    words.join("\n")
}

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
            question_id: format!("{QUESTION_ID_PREFIX}{position}"),
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
        RERANK_STATE_KEYS[0]: request,
        RERANK_STATE_KEYS[1]: candidates
            .iter()
            .map(|candidate| json!({RERANK_NOTE_KEYS[0]: candidate.slug, RERANK_NOTE_KEYS[1]: candidate.summary}))
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
            (
                candidate.question_id.clone(),
                SystemOneQuestion::score(&rerank_instructions(candidate.position), RERANK_LEVELS),
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

impl RerankReading {
    /// Whether the judgment put this note on the bottom level. The one reading
    /// [`compare`] may answer by leaving the note out of what the turn reads.
    #[must_use]
    pub fn reads_as_bottom_level(&self) -> bool {
        reads_as_bottom_level(self.score)
    }
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
    /// The probabilities do not sum to one within the scale's tolerance.
    ProbabilitySum(String),
    /// `score` is missing, not finite, or outside the scale.
    ScoreRange(String),
    /// `score` is not the probability-weighted mean of the level numbers.
    ScoreMismatch(String),
    /// `confidence` outside `[0, 1]`.
    ConfidenceRange(String),
}

impl RerankRejection {
    /// The rule's own word.
    ///
    /// A closed set of nine, so a ledger row can say which check refused a
    /// reply — `schema` is one word for all nine, and a reader of it cannot
    /// tell a malformed answer from an arithmetic one. None of these words
    /// comes from the reply, the notes or the request.
    #[must_use]
    pub const fn rule(&self) -> &'static str {
        match self {
            Self::MissingAnswer(_) => "missing_answer",
            Self::UnknownAnswer(_) => "unknown_answer",
            Self::NotAScore(_) => "not_a_score",
            Self::LevelKeys(_) => "level_keys",
            Self::ProbabilityRange(_) => "probability_range",
            Self::ProbabilitySum(_) => "probability_sum",
            Self::ScoreRange(_) => "score_range",
            Self::ScoreMismatch(_) => "score_mismatch",
            Self::ConfidenceRange(_) => "confidence_range",
        }
    }

    /// Which note the rule broke on, by its place in recall's order.
    ///
    /// A number, never the id itself: [`Self::UnknownAnswer`] carries what the
    /// reply called its answer, which is the model's word and names no note of
    /// ours, so it reads as no place at all.
    #[must_use]
    pub fn position(&self) -> Option<usize> {
        match self {
            Self::UnknownAnswer(_) => None,
            Self::MissingAnswer(id)
            | Self::NotAScore(id)
            | Self::LevelKeys(id)
            | Self::ProbabilityRange(id)
            | Self::ProbabilitySum(id)
            | Self::ScoreRange(id)
            | Self::ScoreMismatch(id)
            | Self::ConfidenceRange(id) => position_of(id),
        }
    }
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

/// One note's answer, read on [`RERANK_SCALE`].
///
/// The arithmetic — the level keys, the probabilities, the sum, the score's
/// range and its being the weighted mean, the confidence — is the shared
/// reader's (`crate::jev_score::read_score`), because it is arithmetic about
/// the wire and the length of a scale rather than anything about a note. What
/// stays here is which note broke the rule, because that is what this
/// judgment's ledger row records.
fn read_answer(
    candidate: &RerankCandidate,
    response: &SystemOneResponse,
) -> Result<RerankReading, RerankRejection> {
    let id = &candidate.question_id;
    let answer = response
        .score_answer(id)
        .ok_or_else(|| RerankRejection::MissingAnswer(id.clone()))?
        .map_err(|_| RerankRejection::NotAScore(id.clone()))?;
    let reading = read_score(&answer, &RERANK_SCALE).map_err(|rule| match rule {
        ScoreRule::NotAScore => RerankRejection::NotAScore(id.clone()),
        ScoreRule::LevelKeys => RerankRejection::LevelKeys(id.clone()),
        ScoreRule::ProbabilityRange => RerankRejection::ProbabilityRange(id.clone()),
        ScoreRule::ProbabilitySum => RerankRejection::ProbabilitySum(id.clone()),
        ScoreRule::ScoreRange => RerankRejection::ScoreRange(id.clone()),
        ScoreRule::ScoreMismatch => RerankRejection::ScoreMismatch(id.clone()),
        ScoreRule::ConfidenceRange => RerankRejection::ConfidenceRange(id.clone()),
    })?;
    Ok(RerankReading {
        position: candidate.position,
        slug: candidate.slug.clone(),
        score: reading.score,
        normalised: reading.normalised,
        confidence: reading.confidence,
    })
}

/// What a judgment would have done to recall's answer — reordered it, and left
/// part of it out — and what the graph refused to let it do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerankComparison {
    /// Recall's own order — what a turn reads, in shadow and until a later
    /// phase says otherwise.
    pub recalled: Vec<String>,
    /// The judgment's order after the graph's rules are applied, with the
    /// bottom-level notes of [`Self::dropped`] left out. Under the apply mode
    /// this is what the turn reads, so it is shorter than [`Self::recalled`]
    /// whenever the judgment dropped anything.
    pub proposed: Vec<String>,
    /// Notes whose place differs between the two orders, before anything is
    /// dropped — how much the judgment REORDERED, kept apart from how much it
    /// removed, so the two axes read separately down a ledger. What a turn
    /// actually ended up with is [`Self::recalled`] against [`Self::proposed`].
    pub moved: usize,
    /// Whether the two disagree about which note comes first, on the same
    /// before-the-drop reading as [`Self::moved`].
    pub top_changed: bool,
    /// Notes the graph's rules pinned — where the judgment alone would have put
    /// them somewhere else. This is the number a later phase has to read before
    /// letting a judgment reorder anything for real.
    pub held_by_graph: Vec<String>,
    /// Notes the judgment put on the bottom level of [`RERANK_LEVELS`], that
    /// the graph said nothing about: not reordered, left out. In the
    /// graph-safe order, so a row reads down the page.
    pub dropped: Vec<String>,
    /// Notes the bottom level would have dropped, that the graph's claim kept
    /// — the second number that rule has to be read with, because a page a
    /// person marked superseded or contradicted is one the graph already
    /// decided about.
    pub kept_by_graph: Vec<String>,
}

/// Fold a checked judgment into recall's answer without breaking the graph:
/// the order it settles, and the bottom-level notes it leaves out.
///
/// `None` when the notes cannot be ordered at all — the vault declaring two
/// pages each other's successor is the one way that happens — in which case
/// recall's order is the answer, and nothing is reported as moved or dropped.
#[must_use]
pub fn compare(hits: &[MemoryHit], readings: &[RerankReading]) -> Option<RerankComparison> {
    let scores = scores_by_position(hits.len(), readings)?;
    let constraints = graph_constraints(hits);
    let ordered = graph_safe_order(hits.len(), &scores, &constraints)?;
    let ungoverned = judgment_only_order(hits.len(), &scores);

    let slug = |position: usize| hits[position].entry.slug.clone();
    let recalled: Vec<String> = (0..hits.len()).map(slug).collect();
    let held_by_graph = ordered
        .iter()
        .zip(&ungoverned)
        .filter(|(governed, alone)| governed != alone)
        .map(|(governed, _)| slug(*governed))
        .collect();
    let moved = ordered
        .iter()
        .enumerate()
        .filter(|(place, position)| place != &**position)
        .count();

    // The drop is the last word, after the ordering rules have had theirs:
    // `moved` and `held_by_graph` stay readings of the ordering alone, and
    // what leaves is a filter over the order they settled.
    let bottom: BTreeSet<usize> = readings
        .iter()
        .filter(|reading| reading.reads_as_bottom_level())
        .map(|reading| reading.position)
        .collect();
    let spoken_for = constraints.spoken_for();
    let leaves = |position: &usize| bottom.contains(position) && !spoken_for.contains(position);
    let dropped = ordered.iter().filter(|at| leaves(at)).map(|at| slug(*at)).collect();
    let kept_by_graph = ordered
        .iter()
        .filter(|at| bottom.contains(at) && !leaves(at))
        .map(|at| slug(*at))
        .collect();

    Some(RerankComparison {
        top_changed: ordered.first() != Some(&0),
        recalled,
        proposed: ordered.iter().filter(|at| !leaves(at)).map(|at| slug(*at)).collect(),
        moved,
        held_by_graph,
        dropped,
        kept_by_graph,
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

impl GraphConstraints {
    /// Every note the graph said something about, by its place in recall's
    /// order. Both ends of every claim, because a claim is about a pair: a
    /// fold reads as a dangling pointer once the page it names is gone, and
    /// half a contradicting pair reads as the settled answer.
    ///
    /// This is what the drop rule asks before leaving a note out — not
    /// [`RerankComparison::held_by_graph`], which is about places rather than
    /// presence, and which a note can fall into by being shifted past one the
    /// graph pinned.
    fn spoken_for(&self) -> BTreeSet<usize> {
        self.superseded_by
            .iter()
            .chain(&self.contradicts)
            .flat_map(|(one, other)| [*one, *other])
            .collect()
    }
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

/// Recall's hits as a judgment settled them — the one place a judgment is
/// allowed to change what a turn reads.
///
/// `proposed` and `dropped` are [`RerankComparison`]'s two lists: the judged
/// head by name, after the graph has had its say, split into what the turn
/// reads and what the bottom-level rule left out. The head is the notes
/// [`rerank_candidates`] built a question for — that same rule, so the two can
/// never disagree about which notes were asked about — and the tail behind it
/// was never asked, so it keeps recall's order and is never dropped.
///
/// `None` unless the two lists together are exactly that head's names, each
/// once. The contract lets a judgment permute what recall admitted and remove
/// a note on one named ground; what this proves is the accounting — every note
/// recall admitted is either ordered or SAID to have been dropped, so nothing
/// goes missing unnamed. A judgment whose lists cannot be checked that far is
/// one this refuses to fold, and the caller then reads recall's order. Two hits
/// recalled under one name cannot be told apart, so they cannot be proved
/// either.
#[must_use]
pub fn apply_order(
    hits: &[MemoryHit],
    proposed: &[String],
    dropped: &[String],
) -> Option<Vec<MemoryHit>> {
    let judged = hits.len().min(MAX_RERANK_CANDIDATES);
    if judged == 0 || proposed.len().saturating_add(dropped.len()) != judged {
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
    // A name spelled twice across the two lists would read one note twice and
    // account for another not at all, which is the same broken promise as
    // inventing an id.
    let mut claim = |name: &String| -> Option<usize> {
        let position = *by_slug.get(name.as_str())?;
        (!std::mem::replace(&mut taken[position], true)).then_some(position)
    };
    let mut read = Vec::with_capacity(hits.len());
    for name in proposed {
        read.push(head[claim(name)?].clone());
    }
    // The dropped names are claimed and then left behind: claiming them is the
    // whole of what makes their absence accounted for rather than silent.
    for name in dropped {
        claim(name)?;
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
    use api::SystemOneQuestionKind;

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
    /// answer, the refusal that has to come back, and the word a ledger row
    /// keeps it under.
    type BrokenCase = (fn(&mut Value), RerankRejection, &'static str);

    /// One score answer as the wire spells one: a probability for every level,
    /// the score, the confidence, and the legend the contract echoes back.
    fn spread(probabilities: [f64; RERANK_LEVELS.len()], score: f64, confidence: f64) -> Value {
        let legend: serde_json::Map<String, Value> = (0..RERANK_LEVELS.len())
            .map(|index| (index.to_string(), json!(RERANK_LEVELS[index])))
            .collect();
        let probabilities: serde_json::Map<String, Value> = probabilities
            .iter()
            .enumerate()
            .map(|(index, probability)| (index.to_string(), json!(probability)))
            .collect();
        json!({
            "type": "score",
            "score": score,
            "confidence": confidence,
            "legend": legend,
            "probabilities": probabilities,
        })
    }

    /// A score answer whose probability sits entirely on one level.
    fn answer(level: usize) -> Value {
        let mut probabilities = [0.0; RERANK_LEVELS.len()];
        probabilities[level] = 1.0;
        spread(probabilities, LEVEL_NUMBERS[level], 1.0)
    }

    /// One answer the wire actually carried: its spread, its score, and what
    /// the rounding left between the two.
    type RoundedCase = ([f64; RERANK_LEVELS.len()], f64, &'static str);

    /// A reply whose answers are spelled out, for readings that sit between
    /// two levels rather than on one.
    fn reply_of(answers: &[Value]) -> SystemOneResponse {
        SystemOneResponse {
            model: "jev-test".to_string(),
            answers: answers
                .iter()
                .enumerate()
                .map(|(position, answer)| (format!("n{position}"), answer.clone()))
                .collect(),
            usage: api::SystemOneUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        }
    }

    fn reply(levels: &[usize]) -> SystemOneResponse {
        let answers: Vec<Value> = levels.iter().map(|level| answer(*level)).collect();
        reply_of(&answers)
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

    /// The recall seat's words are pinned to its version (t-9469): the
    /// question a note is rated under, the four levels and the keys the state
    /// carries. A word changed without a version is red here, and the
    /// question names every key the state carries.
    #[test]
    fn the_version_is_pinned_to_the_words() {
        assert_eq!(RERANK_RUBRIC_VERSION, 1);
        assert_eq!(zerocode_core::jev::rubric_fingerprint(rubric_words), "dc314a35f7b09af6");
        let asked = rerank_instructions(0);
        for key in RERANK_STATE_KEYS {
            assert!(asked.contains(&format!("`{key}")), "{key}: {asked}");
        }
        let state = rerank_state("request", &rerank_candidates(&[hit("wiki/a", "a claim")]));
        let mut keys: Vec<&str> = state.as_object().expect("an object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["notes", "request"]);
        let mut note: Vec<&str> = state["notes"][0].as_object().expect("a note").keys().map(String::as_str).collect();
        note.sort_unstable();
        assert_eq!(note, RERANK_NOTE_KEYS);
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
        let broken: [BrokenCase; 7] = [
            (
                |answer| answer["type"] = json!("choice"),
                RerankRejection::NotAScore("n1".into()),
                "not_a_score",
            ),
            (
                |answer| answer["probabilities"] = json!({"0": 0.0, "1": 1.0}),
                RerankRejection::LevelKeys("n1".into()),
                "level_keys",
            ),
            (
                |answer| answer["probabilities"]["1"] = json!(-0.5),
                RerankRejection::ProbabilityRange("n1".into()),
                "probability_range",
            ),
            (
                |answer| answer["probabilities"]["1"] = json!(0.5),
                RerankRejection::ProbabilitySum("n1".into()),
                "probability_sum",
            ),
            (
                |answer| answer["score"] = json!(9.0),
                RerankRejection::ScoreRange("n1".into()),
                "score_range",
            ),
            (
                |answer| answer["score"] = json!(3.0),
                RerankRejection::ScoreMismatch("n1".into()),
                "score_mismatch",
            ),
            (
                |answer| answer["confidence"] = json!(4.0),
                RerankRejection::ConfidenceRange("n1".into()),
                "confidence_range",
            ),
        ];
        for (break_one_rule, expected, rule) in broken {
            let mut batch = reply(&[3, 1]);
            let mut answer = batch.answers["n1"].clone();
            break_one_rule(&mut answer);
            batch.answers.insert("n1".to_string(), answer);
            let refused = validate_rerank(&candidates, &batch).expect_err("one rule was broken");
            assert_eq!(refused, expected);
            assert_eq!(
                (refused.rule(), refused.position()),
                (rule, Some(1)),
                "a row says which rule refused the reply and which note it broke on"
            );
        }

        let mut missing = reply(&[3, 1]);
        missing.answers.remove("n1");
        let refused = validate_rerank(&candidates, &missing).expect_err("a note went unanswered");
        assert_eq!(refused, RerankRejection::MissingAnswer("n1".into()));
        assert_eq!((refused.rule(), refused.position()), ("missing_answer", Some(1)));
    }

    /// The rule words are what a ledger row and a counter read, so they are a
    /// closed set of distinct words — and none of them is a word of the reply.
    #[test]
    fn every_rule_has_its_own_word_and_an_id_of_the_replys_own_is_no_place_of_ours() {
        let id = "n2".to_string();
        let every: [RerankRejection; 9] = [
            RerankRejection::MissingAnswer(id.clone()),
            RerankRejection::UnknownAnswer(id.clone()),
            RerankRejection::NotAScore(id.clone()),
            RerankRejection::LevelKeys(id.clone()),
            RerankRejection::ProbabilityRange(id.clone()),
            RerankRejection::ProbabilitySum(id.clone()),
            RerankRejection::ScoreRange(id.clone()),
            RerankRejection::ScoreMismatch(id.clone()),
            RerankRejection::ConfidenceRange(id),
        ];
        let words: BTreeSet<&str> = every.iter().map(RerankRejection::rule).collect();
        assert_eq!(words.len(), every.len(), "two rules would be one row: {words:?}");

        let stray = RerankRejection::UnknownAnswer("whatever the reply called it".into());
        assert_eq!(stray.position(), None, "an id nobody asked under names no note");
        assert_eq!(
            RerankRejection::ScoreRange("not-an-id".into()).position(),
            None,
            "a place is read back from the one spelling this module writes"
        );
    }

    /// Jev answers on a grid: every probability and every score it gives is a
    /// whole number of hundredths, rounded from a full-precision one. These
    /// four are answers it actually gave (2026-09-18, jev-1.13.0), copied
    /// digit for digit. In each, that rounding is the only thing standing
    /// between the score and the mean of the spread beside it, and the checks
    /// have to admit them: at eight notes a batch, refusing one answer in ten
    /// throws away half the judgments asked for.
    #[test]
    fn an_answer_only_the_wires_rounding_moved_is_not_a_broken_contract() {
        let hits = [hit("wiki/a", "one")];
        let candidates = rerank_candidates(&hits);
        let rounded: [RoundedCase; 4] = [
            ([0.88, 0.11, 0.01, 0.0], 0.16, "the spread's mean reads 0.13 — three hundredths"),
            ([0.96, 0.03, 0.01, 0.0], 0.08, "the spread's mean reads 0.05 — three hundredths"),
            ([0.67, 0.23, 0.07, 0.03], 0.48, "the mean reads 0.46 — two hundredths"),
            ([0.09, 0.54, 0.28, 0.08], 1.35, "the spread sums to 0.99"),
        ];
        for (probabilities, score, rounding) in rounded {
            let mut batch = reply(&[0]);
            batch.answers.insert("n0".to_string(), spread(probabilities, score, 0.5));
            let readings = validate_rerank(&candidates, &batch).unwrap_or_else(|refused| {
                panic!("{} refused an answer where {rounding}", refused.rule())
            });
            assert_eq!(readings.len(), 1);
            assert!((0.0..=1.0).contains(&readings[0].normalised));
        }
    }

    /// Each bound admits every distance the wire's rounding can open and
    /// nothing the grid can put past it. Between those two is the only place a
    /// bound can sit: on the grid, a double's last bit decides it instead of
    /// the contract.
    #[test]
    fn each_bound_sits_between_the_roundings_reach_and_the_next_step_of_the_grid() {
        for (bound, widest, what) in [
            (
                RERANK_SCALE.score_mean_tolerance(),
                RERANK_LEVEL_NUMBER_SUM * WIRE_ROUNDING,
                "a score against the mean of its spread",
            ),
            (
                RERANK_SCALE.spread_sum_tolerance(),
                RERANK_LEVEL_COUNT * WIRE_ROUNDING,
                "a spread against one",
            ),
        ] {
            assert!(bound > widest, "{what}: {bound} refuses a gap the rounding opens");
            assert!(
                bound < widest + SYSTEMONE_ANSWER_STEP,
                "{what}: {bound} admits a gap the rounding cannot reach"
            );
        }
    }

    /// What the rounding cannot reach is still refused. The check is what
    /// keeps a reply built for some other scale, or a spread that is not a
    /// distribution, from ordering a single note.
    #[test]
    fn a_reply_no_rounding_explains_is_still_refused() {
        let hits = [hit("wiki/a", "one")];
        let candidates = rerank_candidates(&hits);
        let outside: [RoundedCase; 3] = [
            // The first spread above, one hundredth further out than four
            // roundings and the score's own can open between them.
            ([0.88, 0.11, 0.01, 0.0], 0.17, "score_mismatch"),
            // A score read off some other scale entirely.
            ([0.88, 0.11, 0.01, 0.0], 2.0, "score_mismatch"),
            // Not a distribution: a twentieth of the mass never arrived.
            ([0.80, 0.10, 0.04, 0.01], 0.25, "probability_sum"),
        ];
        for (probabilities, score, rule) in outside {
            let mut batch = reply(&[0]);
            batch.answers.insert("n0".to_string(), spread(probabilities, score, 0.5));
            let refused = validate_rerank(&candidates, &batch)
                .expect_err("an answer outside what the rounding explains");
            assert_eq!(refused.rule(), rule, "{probabilities:?} answered {score}");
        }
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
        // Levels 1 and up, so this stays a reading of the ORDER: the bottom
        // level is the one reading that removes a note instead of moving it.
        let comparison = compare(&hits, &readings(&hits, &[1, 3, 2])).expect("an order");

        assert_eq!(comparison.proposed, ["wiki/b", "wiki/c", "wiki/a"]);
        assert_eq!(comparison.recalled, ["wiki/a", "wiki/b", "wiki/c"]);
        assert!(comparison.top_changed);
        assert_eq!(comparison.moved, 3);
        assert!(comparison.held_by_graph.is_empty());
        assert!(comparison.dropped.is_empty() && comparison.kept_by_graph.is_empty());
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
        let comparison = compare(&hits, &readings(&hits, &[1, 3])).expect("an order");

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
        let comparison = compare(&hits, &readings(&hits, &[1, 3, 2])).expect("an order");

        let read = apply_order(&hits, &comparison.proposed, &comparison.dropped)
            .expect("a permutation folds");

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
        let mut levels = vec![1usize; MAX_RERANK_CANDIDATES];
        levels[MAX_RERANK_CANDIDATES - 1] = 3;
        let comparison = compare(head, &readings(head, &levels)).expect("an order");

        let read = apply_order(&hits, &comparison.proposed, &comparison.dropped)
            .expect("a head permutation folds");

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
            assert_eq!(apply_order(&hits, &order, &[]), None, "{why}");
        }
    }

    /// Two notes recall returned under one name cannot be told apart, so the
    /// order cannot be proved a permutation of them either.
    #[test]
    fn a_name_recall_returned_twice_folds_nothing() {
        let hits = [hit("wiki/a", "one"), hit("wiki/a", "again"), hit("wiki/b", "two")];

        assert_eq!(apply_order(&hits, &names(&["wiki/b", "wiki/a", "wiki/a"]), &[]), None);
    }

    #[test]
    fn an_order_that_agrees_with_recall_is_recalls_order() {
        let hits = [hit("wiki/a", "one"), hit("wiki/b", "two")];

        let read = apply_order(&hits, &slugs(&hits), &[]).expect("a permutation folds");

        assert_eq!(read, hits);
    }

    #[test]
    fn the_bottom_level_is_left_out_of_what_the_turn_reads() {
        let hits = [
            hit("wiki/a", "about some other subject"),
            hit("wiki/b", "the answer"),
            hit("wiki/c", "background"),
        ];
        let comparison = compare(&hits, &readings(&hits, &[0, 3, 1])).expect("an order");

        assert_eq!(comparison.proposed, ["wiki/b", "wiki/c"]);
        assert_eq!(comparison.dropped, ["wiki/a"]);
        assert!(comparison.kept_by_graph.is_empty());
        assert_eq!(
            comparison.recalled,
            ["wiki/a", "wiki/b", "wiki/c"],
            "the row still says what recall handed over, or the drop is unreadable"
        );
        assert_eq!(
            comparison.moved, 3,
            "how much the judgment reordered is read before anything is dropped"
        );

        let read = apply_order(&hits, &comparison.proposed, &comparison.dropped)
            .expect("a judgment that accounts for every note folds");

        assert_eq!(slugs(&read), ["wiki/b", "wiki/c"]);
    }

    #[test]
    fn the_level_above_the_bottom_is_not_this_rules_business() {
        let hits = [
            hit("wiki/a", "shares the vocabulary"),
            hit("wiki/b", "the answer"),
        ];
        let comparison = compare(&hits, &readings(&hits, &[1, 3])).expect("an order");

        assert!(
            comparison.dropped.is_empty(),
            "shared background may still be worth a turn's attention, and which of \
             the two a person is better off with is a measured comparison"
        );
        assert_eq!(comparison.proposed, ["wiki/b", "wiki/a"]);
    }

    #[test]
    fn a_page_the_graph_spoke_about_is_never_dropped() {
        let hits = [
            hit("wiki/new", "the current decision"),
            hit("wiki/old", "superseded by [[wiki/new]]"),
            hit("wiki/one", "the claim · ⚠ contradicts [[wiki/two]] (older)"),
            hit("wiki/two", "the other claim · ⚠ contradicts [[wiki/one]] (newer)"),
            hit("wiki/loose", "about some other subject"),
        ];
        // The judgment reads every one of them as the bottom level.
        let comparison = compare(&hits, &readings(&hits, &[0, 0, 0, 0, 0])).expect("an order");

        assert_eq!(comparison.dropped, ["wiki/loose"]);
        assert_eq!(
            comparison.kept_by_graph,
            ["wiki/new", "wiki/old", "wiki/one", "wiki/two"],
            "both ends of a fold and both halves of a pair are the graph's say, and \
             it is already made"
        );
        assert_eq!(
            comparison.proposed,
            ["wiki/new", "wiki/old", "wiki/one", "wiki/two"]
        );
    }

    #[test]
    fn a_recall_the_judgment_read_as_nothing_but_noise_leaves_the_turn_nothing() {
        let hits = [
            hit("wiki/a", "about some other subject"),
            hit("wiki/b", "and so is this one"),
        ];
        let comparison = compare(&hits, &readings(&hits, &[0, 0])).expect("an order");

        assert!(comparison.proposed.is_empty());
        assert_eq!(comparison.dropped, ["wiki/a", "wiki/b"]);

        let read = apply_order(&hits, &comparison.proposed, &comparison.dropped)
            .expect("a judgment that accounts for every note folds");

        assert!(
            read.is_empty(),
            "a recall that answered no part of the request is a section the turn does not get"
        );
    }

    #[test]
    fn the_unjudged_tail_survives_a_head_that_drops_whole() {
        let hits: Vec<MemoryHit> = (0..MAX_RERANK_CANDIDATES + 2)
            .map(|index| hit(&format!("wiki/p{index}"), "a claim"))
            .collect();
        let head = &hits[..MAX_RERANK_CANDIDATES];
        let levels = vec![0usize; MAX_RERANK_CANDIDATES];
        let comparison = compare(head, &readings(head, &levels)).expect("an order");

        let read = apply_order(&hits, &comparison.proposed, &comparison.dropped)
            .expect("every judged note is accounted for");

        assert_eq!(
            slugs(&read),
            slugs(&hits[MAX_RERANK_CANDIDATES..]),
            "the tail was never asked about, so it is never dropped"
        );
    }

    /// The fold's proof is an ACCOUNTING: every note recall admitted is either
    /// ordered or named as dropped, exactly once. A judgment that cannot be
    /// checked that far does not fold, and recall's order stands.
    #[test]
    fn a_judgment_that_does_not_account_for_every_note_folds_nothing() {
        let hits = [
            hit("wiki/a", "one"),
            hit("wiki/b", "two"),
            hit("wiki/c", "three"),
        ];

        for (proposed, dropped, why) in [
            (
                names(&["wiki/b"]),
                names(&["wiki/c"]),
                "a note neither ordered nor dropped",
            ),
            (
                names(&["wiki/b", "wiki/a"]),
                names(&["wiki/a"]),
                "one note both ordered and dropped",
            ),
            (
                names(&["wiki/b", "wiki/a"]),
                names(&["wiki/z"]),
                "a note recall never admitted, dropped",
            ),
            (
                Vec::new(),
                names(&["wiki/a", "wiki/b", "wiki/b"]),
                "a dropped name spelled twice, leaving one unaccounted for",
            ),
        ] {
            assert_eq!(apply_order(&hits, &proposed, &dropped), None, "{why}");
        }
    }

    /// The cut between the bottom level and the next sits where no number the
    /// wire can spell lands on it — the assert beside
    /// [`RERANK_BOTTOM_LEVEL_CUT`] holds it there — so this is the two grid
    /// values either side of it reading as the levels a person would read them
    /// as, and the round trip through the normalised scale a ledger row keeps
    /// not moving either of them across.
    #[test]
    fn the_cut_sits_where_no_answer_the_wire_can_spell_lands_on_it() {
        // The two grid values either side of the midpoint, each spelled as the
        // probability-weighted mean the contract says a score is.
        let hits = [hit("wiki/a", "one"), hit("wiki/b", "two")];
        let candidates = rerank_candidates(&hits);
        let batch = reply_of(&[
            spread([0.51, 0.49, 0.0, 0.0], 0.49, 0.6),
            spread([0.5, 0.5, 0.0, 0.0], 0.5, 0.6),
        ]);
        let readings = validate_rerank(&candidates, &batch).expect("two well-formed answers");

        assert!(readings[0].reads_as_bottom_level());
        assert!(!readings[1].reads_as_bottom_level());
        for reading in &readings {
            assert_eq!(
                reads_as_bottom_level(reading.normalised * RERANK_TOP_LEVEL),
                reading.reads_as_bottom_level(),
                "a ledger row keeps the normalised reading; reading it back must \
                 not move a note across the cut"
            );
        }
    }
}
