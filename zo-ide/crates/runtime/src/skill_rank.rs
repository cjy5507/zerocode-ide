//! Ranking and suggesting installed skills — the pure half.
//!
//! zo has always shown the model every installed skill on every request: a
//! `# Available skills` section of one line per skill, under a 900-token
//! budget that folds the tail into a line naming a count
//! ([`crate::SKILL_INDEX_BUDGET_TOKENS`]). A machine past that budget pays
//! for it on every turn AND cannot reach the skills it folded away by
//! reading, only by already knowing their names.
//!
//! The explicit search tool retains its original per-skill Score rubric.
//! The turn-start suggestion asks a wide Choice with three gate Nouls, then
//! checks three candidates with a second Choice and three fits Nouls. Every
//! skill's words are state in both requests, as in the search's own; an
//! option says which skill it loads, by its place and its name (t-10010).
//!
//! Nothing here calls anything. It builds the state and the questions, cuts
//! them into requests no larger than one request should carry, checks a reply
//! against the questions it asked, and ranks what came back. The executor
//! that puts them on the wire, the door they pass, the setting that permits
//! removing the index, and the ledger that keeps the row are the tools
//! crate's (`smart_router::skill_search`).
//!
//! # Nothing gets worse for asking
//!
//! Behind the judgment stands a deterministic word match ([`lexical_rank`]),
//! and a search that was refused, failed, or answered nothing above the floor
//! hands that back instead, on a row whose `routeUse` says `fallback`. So the
//! prompt can drop the index without the turn ever being left with no way to
//! find a skill — and the ledger says which reader actually chose.

use std::collections::{BTreeMap, BTreeSet};

use api::{SystemOneQuestion, SystemOneQuestionKind, SystemOneResponse};
use core_types::text::levenshtein_distance;
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use zerocode_core::jev::door::cut;
use zerocode_core::jev::noul;
use zerocode_core::jev::questions::{
    SKILL_ACTS_ON_SYSTEM, SKILL_ENTRY_KEYS, SKILL_EXCERPT_KEY, SKILL_FITS, SKILL_FOLLOWS_PROCEDURE,
    SKILL_NARROW_QUESTION, SKILL_NO, SKILL_NO_MATCH, SKILL_NO_MATCH_CRITERION, SKILL_OPTION,
    SKILL_PROSE_SUFFICES, SKILL_SHORTLIST_KEY, SKILL_STATE_KEYS, SKILL_WIDE_QUESTION, SKILL_YES,
};
use zerocode_core::jev::{
    shard, Cap, SKILL_DESCRIPTION_CHAR_CAP, SKILL_EXCERPT_CHAR_CAP,
    SKILL_FITS_FLOOR_PERMILLE, SKILL_GATE_FLOOR_PERMILLE, SKILL_LEVELS,
    SKILL_RELEVANCE_FLOOR_PERMILLE, SKILL_SHARD_TARGET, SKILL_SUGGESTION_SHORTLIST,
    SKILL_TASK_CHAR_CAP,
};

use crate::jev_score::{read_score, Scale, ScoreRule};
use crate::skills::SKILL_RECOMMENDATION_REMINDER_PREFIX;
use crate::prompt::SkillIndexEntry;

/// Bumped whenever a level description, the instructions or the state's shape
/// changes. A row carries it, so a reading taken under other words is never
/// read as evidence about these ones. The number is the seat's row's own
/// (t-6877), read from the table and not respelled.
pub const SKILL_RUBRIC_VERSION: u32 = zerocode_core::jev::questions::SKILL_SEARCH_RUBRIC_VERSION;

const WHICH: &str = "which";
const ACTS_ON_SYSTEM: &str = "acts_on_system";
const FOLLOWS_PROCEDURE: &str = "follows_procedure";
const PROSE_SUFFICES: &str = "prose_suffices";
const FITS_PREFIX: &str = "fits_";

/// The wide request's state: the explicit search's own state over the whole
/// catalog — the task, and every skill's name and description (t-10010).
/// Version 2 sent the task alone and the catalog as the options' words.
#[must_use]
pub fn wide_state(task: &str, candidates: &[SkillCandidate]) -> Value {
    skill_state(task, candidates)
}

/// Where a skill stands in a request's state: its place in `list`.
fn place(list: &str, at: usize) -> String {
    format!("{list}[{at}]")
}

/// What choosing the skill at `place` means ([`SKILL_OPTION`]): loading it,
/// by its name. Its description and instructions are the state's.
fn option_words(place: &str, name: &str) -> String {
    SKILL_OPTION.replace("{at}", place).replace("{name}", name)
}

/// The wide request carries the entire installed catalog in one Choice. The
/// no-match criterion remains available even when all descriptions sound near.
#[must_use]
pub fn wide_questions(candidates: &[SkillCandidate]) -> BTreeMap<String, SystemOneQuestion> {
    let criteria: Vec<(String, Option<String>)> = candidates
        .iter()
        .enumerate()
        .map(|(at, candidate)| (
            candidate.question_id.clone(),
            Some(option_words(&place(SKILL_STATE_KEYS[1], at), &candidate.name)),
        ))
        .collect();
    let borrowed = criteria.iter().map(|(id, description)| (id.as_str(), description.as_deref()))
        .chain(std::iter::once((SKILL_NO_MATCH, Some(SKILL_NO_MATCH_CRITERION))));
    BTreeMap::from([
        (WHICH.into(), SystemOneQuestion::choice(SKILL_WIDE_QUESTION, borrowed)),
        (ACTS_ON_SYSTEM.into(), SystemOneQuestion::noul(SKILL_ACTS_ON_SYSTEM, SKILL_YES, SKILL_NO)),
        (FOLLOWS_PROCEDURE.into(), SystemOneQuestion::noul(SKILL_FOLLOWS_PROCEDURE, SKILL_YES, SKILL_NO)),
        (PROSE_SUFFICES.into(), SystemOneQuestion::noul(SKILL_PROSE_SUFFICES, SKILL_YES, SKILL_NO)),
    ])
}

pub struct WideReading {
    pub gate: f64,
    pub shortlist: Vec<usize>,
}

fn checked_choice(
    response: &SystemOneResponse,
    allowed: &BTreeSet<String>,
) -> Result<api::SystemOneChoiceAnswer, &'static str> {
    let answer = response
        .choice_answer(WHICH)
        .ok_or("missing_choice")?
        .map_err(|_| "invalid_choice")?;
    if answer.kind != SystemOneQuestionKind::Choice
        || !allowed.contains(&answer.choice)
        || !answer.confidence.is_finite()
        || !(0.0..=1.0).contains(&answer.confidence)
        || answer.probabilities.keys().collect::<BTreeSet<_>>()
            != allowed.iter().collect::<BTreeSet<_>>()
        || answer.probabilities.values().any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
        || (answer.probabilities.values().sum::<f64>() - 1.0).abs() > 0.02
    {
        return Err("invalid_choice");
    }
    Ok(answer)
}

/// Validate every answer before letting a low gate prevent the second request.
pub fn read_wide(
    response: &SystemOneResponse,
    candidates: &[SkillCandidate],
) -> Result<WideReading, &'static str> {
    let allowed = candidates
        .iter()
        .map(|candidate| candidate.question_id.clone())
        .chain(std::iter::once(SKILL_NO_MATCH.into()))
        .collect();
    let choice = checked_choice(response, &allowed)?;
    let answers = serde_json::to_value(&response.answers).map_err(|_| "invalid_noul")?;
    let acts = noul::read(&answers, ACTS_ON_SYSTEM).map_err(|_| "invalid_noul")?;
    let procedure = noul::read(&answers, FOLLOWS_PROCEDURE).map_err(|_| "invalid_noul")?;
    let prose = noul::read(&answers, PROSE_SUFFICES).map_err(|_| "invalid_noul")?;
    let gate = (acts + procedure + (1.0 - prose)) / 3.0;
    let mut ranked: Vec<(usize, f64)> = candidates
        .iter()
        .map(|candidate| (candidate.position, choice.probabilities[&candidate.question_id]))
        .collect();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
    let shortlist = if gate < f64::from(SKILL_GATE_FLOOR_PERMILLE) / 1_000.0
        || choice.choice == SKILL_NO_MATCH
    {
        Vec::new()
    } else {
        ranked.into_iter().take(SKILL_SUGGESTION_SHORTLIST).map(|(position, _)| position).collect()
    };
    Ok(WideReading { gate, shortlist })
}

/// Shortlisted instruction text is read locally by the caller and first
/// appears in the wire body after the SKILLS door has checked consent.
pub struct SkillDetail {
    pub position: usize,
    pub excerpt: String,
}

/// The shortlist as the narrow request lays it out, in the wide answer's
/// order: each detail beside the skill it reads, a place in `candidates`
/// each.
fn shortlisted<'a>(
    candidates: &'a [SkillCandidate],
    details: &'a [SkillDetail],
) -> impl Iterator<Item = (&'a SkillCandidate, &'a SkillDetail)> {
    details.iter().filter_map(|detail| candidates.get(detail.position).map(|candidate| (candidate, detail)))
}

/// The narrow request's state: the task, and each shortlisted skill's name,
/// description and the head of its instructions — once each (t-10010).
#[must_use]
pub fn narrow_state(task: &str, candidates: &[SkillCandidate], details: &[SkillDetail]) -> Value {
    json!({
        SKILL_STATE_KEYS[0]: cut(task, Cap::Chars(SKILL_TASK_CHAR_CAP)),
        SKILL_SHORTLIST_KEY: shortlisted(candidates, details).map(|(candidate, detail)| json!({
            SKILL_ENTRY_KEYS[0]: candidate.name,
            SKILL_ENTRY_KEYS[1]: candidate.description,
            SKILL_EXCERPT_KEY: cut(&detail.excerpt, Cap::Chars(SKILL_EXCERPT_CHAR_CAP)),
        })).collect::<Vec<_>>(),
    })
}

/// The narrow request's Choice over the shortlist and one fits Noul a
/// skill, each naming the skill by its place in `candidates` — never
/// repeating what the state says of it.
#[must_use]
pub fn narrow_questions(
    candidates: &[SkillCandidate],
    details: &[SkillDetail],
) -> BTreeMap<String, SystemOneQuestion> {
    let mut questions = BTreeMap::new();
    let mut criteria: Vec<(String, Option<String>)> = Vec::new();
    for (at, (candidate, _)) in shortlisted(candidates, details).enumerate() {
        let place = place(SKILL_SHORTLIST_KEY, at);
        criteria.push((candidate.question_id.clone(), Some(option_words(&place, &candidate.name))));
        questions.insert(
            format!("{FITS_PREFIX}{}", candidate.question_id),
            SystemOneQuestion::noul(&SKILL_FITS.replace("{at}", &place), SKILL_YES, SKILL_NO),
        );
    }
    let borrowed = criteria.iter().map(|(id, description)| (id.as_str(), description.as_deref()))
        .chain(std::iter::once((SKILL_NO_MATCH, Some(SKILL_NO_MATCH_CRITERION))));
    questions.insert(WHICH.into(), SystemOneQuestion::choice(SKILL_NARROW_QUESTION, borrowed));
    questions
}

pub fn read_narrow(
    response: &SystemOneResponse,
    candidates: &[SkillCandidate],
    details: &[SkillDetail],
) -> Result<Option<SkillReading>, &'static str> {
    let allowed = shortlisted(candidates, details)
        .map(|(candidate, _)| candidate.question_id.clone())
        .chain(std::iter::once(SKILL_NO_MATCH.into())).collect();
    let choice = checked_choice(response, &allowed)?;
    let answers = serde_json::to_value(&response.answers).map_err(|_| "invalid_noul")?;
    let best_fit = shortlisted(candidates, details)
        .map(|(candidate, _)| noul::read(&answers, &format!("{FITS_PREFIX}{}", candidate.question_id)))
        .collect::<Result<Vec<_>, _>>().map_err(|_| "invalid_noul")?
        .into_iter().fold(0.0, f64::max);
    if best_fit < f64::from(SKILL_FITS_FLOOR_PERMILLE) / 1_000.0
        || choice.choice == SKILL_NO_MATCH {
        return Ok(None);
    }
    let candidate = candidates.iter().find(|candidate| candidate.question_id == choice.choice)
        .ok_or("invalid_choice")?;
    Ok(Some(SkillReading {
        position: candidate.position,
        name: candidate.name.clone(),
        score: choice.probabilities[&choice.choice] * SKILL_SCALE.top(),
        normalised: choice.probabilities[&choice.choice],
        confidence: choice.confidence,
    }))
}

#[must_use]
pub fn suggestion_note(name: Option<&str>) -> String {
    match name {
        Some(name) => {
            let safe = name.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            format!(
                "{SKILL_RECOMMENDATION_REMINDER_PREFIX} <system-reminder>Relevant to this request: {safe}. Load it if it actually fits.</system-reminder>"
            )
        }
        None => format!(
            "{SKILL_RECOMMENDATION_REMINDER_PREFIX} <system-reminder>No installed skill appears relevant to this request.</system-reminder>"
        ),
    }
}

/// The runtime's turn boundary calls this seat on every public turn. The
/// implementation lives with the Jev door in tools; this interface keeps
/// runtime independent of that crate.
pub trait SkillSuggestionSeat: Send + Sync {
    fn suggest(&self, request: String) -> BoxFuture<'_, Option<String>>;
    fn finish(&self, turn: &[crate::session::ConversationMessage]);
}

/// The scale every skill question is asked on — the use table's own levels.
pub const SKILL_SCALE: Scale<'static> = Scale::new(&SKILL_LEVELS);

/// The letter a question id opens with, so what comes back is named rather
/// than numbered. Spelled once: [`skill_questions`] writes ids with it and
/// [`position_of`] reads them back through it.
const QUESTION_ID_PREFIX: &str = "s";

/// The skill a question id names, by its place in the whole catalog, or
/// `None` when the id is not one this module wrote.
fn position_of(question_id: &str) -> Option<usize> {
    question_id.strip_prefix(QUESTION_ID_PREFIX)?.parse().ok()
}

/// The question one skill of a shard is scored under. It names the skill by
/// its place in the shard's state, because a question id is never sent;
/// spelled once, for the questions and for the words the version is pinned
/// to ([`search_rubric_words`]).
fn search_instructions(at: usize) -> String {
    format!("How much does `skills[{at}]` cover `task`?")
}

/// The words the explicit search asks, as one string: the question a skill is
/// scored under, the use table's levels and the keys the state and each skill
/// carry. [`SKILL_RUBRIC_VERSION`] is pinned to it, so a word changed without
/// a version is a red test rather than a quiet drift (t-9469). The turn
/// boundary's suggestion is a seat of its own, with words of its own
/// (`zerocode_core::jev::questions::skill_suggestion_rubric_fingerprint`).
#[must_use]
pub fn search_rubric_words() -> String {
    let mut words = vec![search_instructions(0)];
    words.extend(SKILL_LEVELS.iter().map(|level| (*level).to_string()));
    words.push(SKILL_STATE_KEYS.join(","));
    words.push(SKILL_ENTRY_KEYS.join(","));
    words.join("\n")
}

/// The fewest characters of a task word that are worth matching on. Shorter
/// than this and a word is a preposition, an article, or a variable name.
const LEXICAL_WORD_CHARS: usize = 3;

/// The most name suggestions one unknown name is answered with.
const NAME_SUGGESTIONS: usize = 3;

/// The furthest a typo may be from a real skill's name and still be read as
/// that name being meant — the same distance the slash-command suggester
/// allows, because it is the same mistake: a name typed from memory.
const NAME_TYPO_DISTANCE: usize = 2;

/// One skill put to the judgment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillCandidate {
    /// The key this skill's question and answer share. Never sent to the
    /// model — the contract says a question id is for code — so the
    /// instructions name the skill by its path in the state instead.
    pub question_id: String,
    /// Where this skill sits in the whole catalog, in the order the prompt
    /// index would have listed it.
    pub position: usize,
    pub name: String,
    /// The frontmatter description, cut at the table's cap by the door's own
    /// cutter, so the text a question carries and the text the door would
    /// clear are the same text.
    pub description: String,
}

/// Every installed skill, in the catalog's own precedence order.
///
/// A skill with no description is still asked about: its name is what the
/// index would have shown, and a skill nobody wrote a line for is exactly the
/// one a ranking is worth having for.
#[must_use]
pub fn skill_candidates(skills: &[SkillIndexEntry]) -> Vec<SkillCandidate> {
    skills
        .iter()
        .enumerate()
        .map(|(position, skill)| SkillCandidate {
            question_id: format!("{QUESTION_ID_PREFIX}{position}"),
            position,
            name: skill.name.clone(),
            description: cut(
                skill.description.as_deref().unwrap_or_default(),
                Cap::Chars(SKILL_DESCRIPTION_CHAR_CAP),
            ),
        })
        .collect()
}

/// The requests `candidates` are asked in: even shards of the table's target,
/// so the shard that decides the answer is the one the caller waits for and
/// not the longest one ([`shard::even_shards`]).
#[must_use]
pub fn skill_shards(candidates: &[SkillCandidate]) -> Vec<&[SkillCandidate]> {
    shard::even_shards(candidates.len(), SKILL_SHARD_TARGET)
        .into_iter()
        .map(|range| &candidates[range])
        .collect()
}

/// The one state every question in a shard reads.
///
/// Names and descriptions only. A skill's BODY never leaves: it sits on this
/// machine's disk and the search hands it to the model as a tool result, so
/// the judgment prices a line and the turn reads a document.
#[must_use]
pub fn skill_state(task: &str, shard: &[SkillCandidate]) -> Value {
    json!({
        SKILL_STATE_KEYS[0]: cut(task, Cap::Chars(SKILL_TASK_CHAR_CAP)),
        SKILL_STATE_KEYS[1]: shard
            .iter()
            .map(|candidate| json!({
                SKILL_ENTRY_KEYS[0]: candidate.name,
                SKILL_ENTRY_KEYS[1]: candidate.description,
            }))
            .collect::<Vec<_>>(),
    })
}

/// One question per skill in the shard, by the id its answer comes back
/// under.
///
/// The id is the skill's place in the WHOLE catalog and the instructions name
/// its place in THIS shard's state — the offset mapping a split batch needs.
/// Without it the second shard would ask about `skills[0]` and answer under
/// an id the first shard already used, and two shards' answers could not be
/// read into one ranking.
#[must_use]
pub fn skill_questions(shard: &[SkillCandidate]) -> BTreeMap<String, SystemOneQuestion> {
    let offset = shard.first().map_or(0, |first| first.position);
    shard
        .iter()
        .map(|candidate| {
            let at = candidate.position - offset;
            (
                candidate.question_id.clone(),
                SystemOneQuestion::score(&search_instructions(at), SKILL_LEVELS),
            )
        })
        .collect()
}

/// One skill's reading, checked against the question that asked for it.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillReading {
    pub position: usize,
    pub name: String,
    /// The position along the levels, as answered.
    pub score: f64,
    /// The same reading on 0 to 1, so scales of different lengths compare.
    pub normalised: f64,
    pub confidence: f64,
}

impl SkillReading {
    /// Whether this reading reaches the table's relevance floor — the line
    /// under which a skill is not handed back at all.
    #[must_use]
    pub fn covers_the_task(&self) -> bool {
        SKILL_SCALE.reaches(self.score, SKILL_RELEVANCE_FLOOR_PERMILLE)
    }
}

/// Why a reply was thrown away. Every one of these discards that SHARD's
/// judgment: a shard half-read would rank some skills by the model and the
/// rest by nothing at all.
///
/// A shard and not the whole search, deliberately: the shards are independent
/// requests over disjoint skills, so one refused reply leaves the others'
/// rankings standing and only the skills it asked about unranked. The row
/// says how many shards answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillRejection {
    /// A skill was asked about and not answered.
    MissingAnswer(String),
    /// An answer came back under an id nothing was asked under.
    UnknownAnswer(String),
    /// One of the answer rules refused it ([`ScoreRule`]).
    Answer(String, ScoreRule),
}

impl SkillRejection {
    /// The rule's own word, as a ledger row spells it.
    #[must_use]
    pub const fn rule(&self) -> &'static str {
        match self {
            Self::MissingAnswer(_) => "missing_answer",
            Self::UnknownAnswer(_) => "unknown_answer",
            Self::Answer(_, rule) => rule.word(),
        }
    }

    /// Which skill the rule broke on, by its place in the catalog.
    ///
    /// A number, never the id itself: [`Self::UnknownAnswer`] carries what the
    /// reply called its answer, which is the model's word and names no skill
    /// of ours, so it reads as no place at all.
    #[must_use]
    pub fn position(&self) -> Option<usize> {
        match self {
            Self::UnknownAnswer(_) => None,
            Self::MissingAnswer(id) | Self::Answer(id, _) => position_of(id),
        }
    }
}

/// Check one shard's reply against the questions [`skill_questions`] asked.
///
/// # Errors
/// The first rule the reply breaks.
pub fn validate_skills(
    shard: &[SkillCandidate],
    response: &SystemOneResponse,
) -> Result<Vec<SkillReading>, SkillRejection> {
    let asked: BTreeSet<&str> = shard
        .iter()
        .map(|candidate| candidate.question_id.as_str())
        .collect();
    // A skill nobody asked about is the one failure that says the reply is not
    // about this shard at all, so it is checked before any answer is read.
    if let Some(stray) = response
        .answers
        .keys()
        .find(|id| !asked.contains(id.as_str()))
    {
        return Err(SkillRejection::UnknownAnswer(stray.clone()));
    }
    shard
        .iter()
        .map(|candidate| {
            let id = &candidate.question_id;
            let answer = response
                .score_answer(id)
                .ok_or_else(|| SkillRejection::MissingAnswer(id.clone()))?
                .map_err(|_| SkillRejection::Answer(id.clone(), ScoreRule::NotAScore))?;
            let reading = read_score(&answer, &SKILL_SCALE)
                .map_err(|rule| SkillRejection::Answer(id.clone(), rule))?;
            Ok(SkillReading {
                position: candidate.position,
                name: candidate.name.clone(),
                score: reading.score,
                normalised: reading.normalised,
                confidence: reading.confidence,
            })
        })
        .collect()
}

/// The skills a search hands back, best first: the ones whose reading reaches
/// the table's floor, ordered by score, then by how sure the judgment was,
/// then by name.
///
/// The name breaks the last tie so that one catalog and one task rank the
/// same way twice — a ranking whose ties fall out of a hash is a ranking
/// nobody can compare two runs of.
#[must_use]
pub fn rank(mut readings: Vec<SkillReading>) -> Vec<SkillReading> {
    readings.retain(SkillReading::covers_the_task);
    readings.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                right
                    .confidence
                    .partial_cmp(&left.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then_with(|| left.name.cmp(&right.name))
    });
    readings
}

/// The ranking behind the judgment: how much of the task's vocabulary each
/// skill's name and description already carries.
///
/// Deterministic and free, and the answer whenever there is no key, the door
/// refuses, or nothing came back in time. It is NOT held to the table's
/// relevance floor — that line is a claim about an expected level, and a word
/// count has no expected level to make it of. What it does instead is refuse
/// to invent a match: a skill that shares no word with the task is not
/// ranked, so a task nothing covers comes back empty rather than
/// alphabetical.
#[must_use]
pub fn lexical_rank(task: &str, candidates: &[SkillCandidate]) -> Vec<SkillReading> {
    let asked = words_of(task);
    if asked.is_empty() {
        return Vec::new();
    }
    let mut ranked: Vec<SkillReading> = candidates
        .iter()
        .filter_map(|candidate| {
            let mut held = words_of(&candidate.name);
            held.extend(words_of(&candidate.description));
            let mut shared = 0.0;
            let mut total = 0.0;
            for word in &asked {
                total += 1.0;
                if held.contains(word) {
                    shared += 1.0;
                }
            }
            (shared > 0.0).then(|| {
                let share = shared / total;
                SkillReading {
                    position: candidate.position,
                    name: candidate.name.clone(),
                    score: share * SKILL_SCALE.top(),
                    normalised: share,
                    // A word count makes no claim about how sure it is, and a
                    // number here would be one read off the same overlap the
                    // score already is.
                    confidence: 0.0,
                }
            })
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.name.cmp(&right.name))
    });
    ranked
}

/// The words of a text worth matching on: lowercased, split on anything that
/// is not a letter or a digit, and short ones dropped.
fn words_of(text: &str) -> BTreeSet<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() >= LEXICAL_WORD_CHARS)
        .map(str::to_lowercase)
        .collect()
}

/// A skill's name with everything a person might spell differently taken out:
/// case, and the separators a catalog uses between a plugin and its skill or
/// between a name's words.
///
/// `Anthropic Skills: DOCX`, `anthropic-skills:docx` and
/// `anthropic_skills_docx` are one name here, which is what lets a name typed
/// from memory resolve without a suggestion.
#[must_use]
pub fn normalise_skill_name(raw: &str) -> String {
    raw.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// What a name asked for resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameMatch {
    /// The catalog's own spelling of the skill that name means.
    Found(String),
    /// No skill answers to it. The suggestions are the closest names, or
    /// empty when nothing is close enough to be worth guessing at.
    Unknown {
        asked: String,
        suggestions: Vec<String>,
    },
}

/// Resolve every name a caller asked for against the catalog's own names,
/// ignoring case and separators, and suggest the closest names for one that
/// answers to nothing.
#[must_use]
pub fn resolve_skill_names(asked: &[String], names: &[String]) -> Vec<NameMatch> {
    asked
        .iter()
        .map(|raw| {
            let wanted = normalise_skill_name(raw);
            names
                .iter()
                .find(|name| normalise_skill_name(name) == wanted)
                .map_or_else(
                    || NameMatch::Unknown {
                        asked: raw.clone(),
                        suggestions: suggest_skill_names(&wanted, names),
                    },
                    |name| NameMatch::Found(name.clone()),
                )
        })
        .collect()
}

/// The catalog names closest to a name nothing answered to: the ones that
/// contain it or are contained by it first, then the ones a typo away, each
/// group by distance and then by name.
fn suggest_skill_names(wanted: &str, names: &[String]) -> Vec<String> {
    let mut close: Vec<(usize, usize, &String)> = names
        .iter()
        .filter_map(|name| {
            let candidate = normalise_skill_name(name);
            // 0 for a name that contains the one asked for or is contained by
            // it, 1 for one only a typo away — the order suggestions come in.
            let overlaps = candidate.contains(wanted) || wanted.contains(&candidate);
            let nearness = usize::from(!overlaps);
            let distance = levenshtein_distance(&candidate, wanted);
            (overlaps || distance <= NAME_TYPO_DISTANCE)
                .then_some((nearness, distance, name))
        })
        .collect();
    close.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(right.2))
    });
    close
        .into_iter()
        .map(|(_, _, name)| name.clone())
        .take(NAME_SUGGESTIONS)
        .collect()
}

#[cfg(test)]
mod tests;
