//! Reviewable Jev evidence and bounded acquisition of the next labels.
//!
//! A case is the exact cleared request and typed response. Its content identity
//! includes the seat, rubric and answering model; the state group deliberately
//! excludes the response, so repeated states cannot straddle evaluation splits.
//! Reviews are explicit observations. Neither sampling nor a model's confidence
//! creates a correctness label or changes a production promotion ledger.

use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashSet};
use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{JevUse, choice, door::Cleared, noul};

pub mod store;

/// Bound the evidence copied from a request and its response.
pub const MAX_CASE_BYTES: usize = 128 * 1024;
/// A review batch is for inspection, never a second copy of the whole history.
pub const MAX_BATCH: usize = 100;
/// The maximum note retained beside a reviewed outcome.
pub const MAX_NOTE_CHARS: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub group: String,
    pub at: i64,
    pub seat: String,
    /// Local provenance, never added to the request sent to Jev.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Hash of the actual local session/task identity, never sent to Jev.
    /// Missing provenance is not an independent evaluation group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_group: Option<String>,
    pub rubric_version: u32,
    pub model: String,
    pub request: Value,
    pub answers: Value,
}

/// Why evidence cannot be treated as a reviewable case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    TooLarge,
    Identity,
    Questions,
    Answer,
    Outcome,
    Batch,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::TooLarge => "evidence exceeds the case byte limit",
            Self::Identity => "evidence identity does not match its content",
            Self::Questions => "evidence has no complete bounded question set",
            Self::Answer => "evidence contains a missing or malformed typed answer",
            Self::Outcome => "the review does not identify a valid case and question",
            Self::Batch => "review batch must contain 1 to 100 cases and a valid audit count",
        })
    }
}

impl std::error::Error for Invalid {}

struct HashWriter(Sha256);

impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn digest(value: &impl Serialize) -> Result<String, Invalid> {
    let mut out = HashWriter(Sha256::new());
    serde_json::to_writer(&mut out, value).map_err(|_| Invalid::Identity)?;
    Ok(format!("{:x}", out.0.finalize()))
}

/// Stable local provenance shared by all observations of a session/task.
///
/// # Errors
/// Empty or unbounded identifiers cannot establish an origin.
pub fn origin_group(namespace: &str, origin: &str) -> Result<String, Invalid> {
    if namespace.trim().is_empty()
        || origin.trim().is_empty()
        || namespace.len() > 128
        || origin.len() > 4_096
    {
        return Err(Invalid::Identity);
    }
    digest(&(namespace, origin))
}

/// Return distribution concentration, not the probability of being correct.
fn certainty(questions: &Value, answers: &Value, id: &str) -> Result<f64, Invalid> {
    let question = &questions[id];
    let answer = &answers[id];
    match question["type"].as_str() {
        Some("choice") => {
            let criteria = question["criteria"].as_object().ok_or(Invalid::Questions)?;
            if !(2..=255).contains(&criteria.len()) {
                return Err(Invalid::Questions);
            }
            let offered = criteria.keys().cloned().collect::<BTreeSet<_>>();
            choice::read(answers, id, &offered)
                .map(|read| read.confidence)
                .map_err(|_| Invalid::Answer)
        }
        Some("noul") => noul::read(answers, id)
            .map(|p| (2.0 * p - 1.0).abs())
            .map_err(|_| Invalid::Answer),
        Some("score") => {
            let levels = question["criteria"].as_array().ok_or(Invalid::Questions)?;
            super::score::read_value(answer, levels.len())
                .map(|read| read.confidence)
                .map_err(|_| Invalid::Answer)
        }
        _ => Err(Invalid::Questions),
    }
}

impl Case {
    /// Bind a known local session or task; raw identifiers are not retained.
    ///
    /// # Errors
    /// Invalid provenance or a modified/oversized case.
    pub fn with_origin(mut self, namespace: &str, origin: &str) -> Result<Self, Invalid> {
        self.validate()?;
        self.origin_group = Some(origin_group(namespace, origin)?);
        self.id = self.identity()?;
        self.validate()?;
        Ok(self)
    }

    /// Preserve original source grouping in an explicitly reviewed replay.
    ///
    /// # Errors
    /// Invalid group identity or evidence.
    pub fn with_origin_group(mut self, origin: Option<String>) -> Result<Self, Invalid> {
        self.validate()?;
        self.origin_group = origin;
        self.id = self.identity()?;
        self.validate()?;
        Ok(self)
    }
    /// Bind locally retained evidence to its source workspace, without adding
    /// that path to the request sent to Jev.
    ///
    /// # Errors
    /// The resulting case exceeds its size limit or is otherwise invalid.
    pub fn with_workspace(mut self, workspace: &std::path::Path) -> Result<Self, Invalid> {
        self.validate()?;
        self.workspace = Some(super::door::resolved_path(workspace));
        self.id = self.identity()?;
        self.validate()?;
        Ok(self)
    }

    /// Capture only bytes the shared consent and redaction door already cleared.
    ///
    /// # Errors
    /// Refuses oversized, incomplete or malformed evidence.
    pub fn from_cleared(
        seat: &JevUse,
        cleared: &Cleared,
        response: &Value,
        at: i64,
    ) -> Result<Self, Invalid> {
        if cleared.bytes().len() > MAX_CASE_BYTES {
            return Err(Invalid::TooLarge);
        }
        let mut size = SizeBound {
            remaining: MAX_CASE_BYTES - cleared.bytes().len(),
        };
        serde_json::to_writer(&mut size, response).map_err(|_| Invalid::TooLarge)?;
        Self::from_request(
            seat.id,
            seat.rubric_version,
            serde_json::from_slice(cleared.bytes()).map_err(|_| Invalid::Questions)?,
            response,
            at,
            None,
        )
    }

    fn from_request(
        seat: &str,
        rubric_version: u32,
        mut request: Value,
        response: &Value,
        at: i64,
        workspace: Option<String>,
    ) -> Result<Self, Invalid> {
        // Both workspaces may enable different serde_json map features. Keep
        // stored object order canonical so they read the same case identity.
        request.sort_all_objects();
        let mut answers = response["answers"].clone();
        answers.sort_all_objects();
        let mut case = Self {
            id: String::new(),
            group: String::new(),
            at,
            seat: seat.to_string(),
            workspace,
            origin_group: None,
            rubric_version,
            model: response["model"]
                .as_str()
                .ok_or(Invalid::Answer)?
                .to_string(),
            request,
            answers,
        };
        case.group = digest(&case.request["state"])?;
        case.id = case.identity()?;
        case.validate()?;
        Ok(case)
    }

    fn identity(&self) -> Result<String, Invalid> {
        let original = digest(&(
            &self.seat,
            &self.workspace,
            self.rubric_version,
            &self.request,
            &self.model,
            &self.answers,
        ))?;
        match &self.origin_group {
            Some(origin) => digest(&("origin-v1", original, origin)),
            None => Ok(original),
        }
    }

    /// Validate an imported or persisted case before it is reviewed or replayed.
    ///
    /// # Errors
    /// Returns the first invalid identity, size, question or answer.
    pub fn validate(&self) -> Result<(), Invalid> {
        let mut bounded = SizeBound {
            remaining: MAX_CASE_BYTES,
        };
        serde_json::to_writer(&mut bounded, self).map_err(|_| Invalid::TooLarge)?;
        if self.seat.is_empty() || self.rubric_version == 0 || self.model.trim().is_empty() {
            return Err(Invalid::Identity);
        }
        if self.origin_group.as_deref().is_some_and(|group| {
            group.len() != 64
                || !group
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }) {
            return Err(Invalid::Identity);
        }
        if self.group != digest(&self.request["state"])? || self.id != self.identity()? {
            return Err(Invalid::Identity);
        }
        let questions = self.request["questions"]
            .as_object()
            .ok_or(Invalid::Questions)?;
        let answers = self.answers.as_object().ok_or(Invalid::Answer)?;
        if questions.is_empty()
            || questions.len() > MAX_BATCH
            || questions.len() != answers.len()
            || !questions
                .keys()
                .all(|id| !id.is_empty() && answers.contains_key(id))
        {
            return Err(Invalid::Questions);
        }
        for id in questions.keys() {
            certainty(&self.request["questions"], &self.answers, id)?;
        }
        Ok(())
    }

    /// The least certain answer that still needs review.
    fn certainty_except(&self, reviewed: &HashSet<(&str, &str)>) -> Option<f64> {
        self.request["questions"]
            .as_object()?
            .keys()
            .filter(|id| !reviewed.contains(&(self.id.as_str(), id.as_str())))
            .filter_map(|id| certainty(&self.request["questions"], &self.answers, id).ok())
            .min_by(f64::total_cmp)
    }
}

struct SizeBound {
    remaining: usize,
}

impl Write for SizeBound {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len())
            .ok_or_else(|| io::Error::other("case exceeds byte limit"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Who supplied a correctness observation. Agent assistance is never called human review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reviewer {
    Human,
    Agent,
    Execution,
}

impl Reviewer {
    pub const ALL: [Self; 3] = [Self::Human, Self::Agent, Self::Execution];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Outcome {
    pub case_id: String,
    pub question: String,
    pub at: i64,
    pub correct: bool,
    pub reviewer: Reviewer,
    pub note: String,
}

impl Outcome {
    /// A review must name an answer actually present in this exact case.
    ///
    /// # Errors
    /// Refuses a mismatched case, unknown question or empty/oversized evidence note.
    pub fn validate(&self, case: &Case) -> Result<(), Invalid> {
        case.validate()?;
        self.for_valid_case(case)
    }

    fn for_valid_case(&self, case: &Case) -> Result<(), Invalid> {
        if self.case_id != case.id
            || !case
                .answers
                .as_object()
                .is_some_and(|answers| answers.contains_key(&self.question))
            || self.note.trim().is_empty()
            || self.note.chars().count() > MAX_NOTE_CHARS
            || self.at < case.at
        {
            return Err(Invalid::Outcome);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Uncertain,
    Audit,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample<'case> {
    pub case: &'case Case,
    pub reason: Reason,
    pub remaining: Vec<&'case str>,
}

fn random_rank(id: &str, seed: u64) -> u64 {
    let mut hash = Sha256::new();
    hash.update(seed.to_le_bytes());
    hash.update(id.as_bytes());
    let bytes = hash.finalize();
    let mut first = [0; 8];
    first.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(first)
}

/// Keep only the best `limit` indices; no full sorted copy of the evidence pool.
fn best(items: impl Iterator<Item = (u64, u64, usize)>, limit: usize) -> Vec<usize> {
    if limit == 0 {
        return Vec::new();
    }
    let mut kept = BinaryHeap::with_capacity(limit);
    for item in items {
        if kept.len() < limit {
            kept.push(item);
        } else if kept.peek().is_some_and(|worst| item < *worst) {
            kept.pop();
            kept.push(item);
        }
    }
    kept.into_sorted_vec()
        .into_iter()
        .map(|(_, _, index)| index)
        .collect()
}

fn selected_indices(
    candidates: &[(f64, u64, usize)],
    limit: usize,
    audit: usize,
) -> Vec<(usize, Reason)> {
    let uncertain = best(
        candidates.iter().map(|&(certainty, rank, index)| {
            let certainty = if certainty == 0.0 { 0.0 } else { certainty };
            (certainty.to_bits(), rank, index)
        }),
        limit - audit,
    );
    let chosen: HashSet<usize> = uncertain.iter().copied().collect();
    let audited = best(
        candidates
            .iter()
            .filter(|(_, _, index)| !chosen.contains(index))
            .map(|&(_, rank, index)| (rank, rank, index)),
        limit - uncertain.len(),
    );
    uncertain
        .into_iter()
        .map(|index| (index, Reason::Uncertain))
        .chain(audited.into_iter().map(|index| (index, Reason::Audit)))
        .collect()
}

/// Select uncertain cases plus a seeded audit sample of the remainder.
///
/// Invalid cases and reviews never enter the pool. Repeated identical evidence
/// is one case, using its earliest observation time. Partially reviewed batches
/// remain eligible for their unanswered questions. Selection itself writes nothing.
///
/// # Errors
/// Refuses an invalid batch size or audit allocation.
pub fn select<'case>(
    cases: &'case [Case],
    outcomes: &[Outcome],
    limit: usize,
    audit: usize,
    seed: u64,
) -> Result<Vec<Sample<'case>>, Invalid> {
    if !(1..=MAX_BATCH).contains(&limit) || audit > limit {
        return Err(Invalid::Batch);
    }
    let mut unique = BTreeMap::<&str, usize>::new();
    for (index, case) in cases
        .iter()
        .enumerate()
        .filter(|(_, case)| case.validate().is_ok())
    {
        let held = unique.entry(&case.id).or_insert(index);
        if case.at < cases[*held].at {
            *held = index;
        }
    }
    let reviewed: HashSet<(&str, &str)> = outcomes
        .iter()
        .filter_map(|outcome| {
            let case = &cases[*unique.get(outcome.case_id.as_str())?];
            outcome.for_valid_case(case).ok()?;
            Some((case.id.as_str(), outcome.question.as_str()))
        })
        .collect();
    let candidates = {
        unique
            .values()
            .copied()
            .filter_map(|index| {
                let case = &cases[index];
                Some((
                    case.certainty_except(&reviewed)?,
                    random_rank(&case.id, seed),
                    index,
                ))
            })
            .collect::<Vec<_>>()
    };
    Ok(selected_indices(&candidates, limit, audit)
        .into_iter()
        .map(|(index, reason)| {
            let case = &cases[index];
            let remaining = case
                .answers
                .as_object()
                .into_iter()
                .flat_map(|answers| answers.keys())
                .filter(|id| !reviewed.contains(&(case.id.as_str(), id.as_str())))
                .map(String::as_str)
                .collect();
            Sample {
                case,
                reason,
                remaining,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests;
