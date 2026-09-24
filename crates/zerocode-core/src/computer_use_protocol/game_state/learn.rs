//! A demonstration proposes; the person's goal adopts. A candidate learned
//! from labelled boxes (the helper's `PerceptionLearner`) names colours, the
//! inputs the person made and the scenes it saw. It is a document: it never
//! runs as a spec and grants no input. [`adopt`] turns its colours into
//! palette classes only when the person's approval covers every input and
//! label it carries and a held-out score — from scenes the candidate never
//! saw, judged by an oracle that is not the candidate — shows no wrong action.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::super::reflex::LeaseInput;
use super::{ColorClass, PerceptionLimits, SpecError, check_palette};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Candidate,
}

/// A label's colour as the demonstration showed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearnedClass {
    pub label: String,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub tolerance: u8,
}

/// What a demonstration proposes: one class per label, the inputs the person
/// made on labelled boxes, and the scenes it learned from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub status: CandidateStatus,
    pub classes: Vec<LearnedClass>,
    pub inputs: BTreeSet<LeaseInput>,
    pub scenes: Vec<String>,
}

/// Which part of the labelled scenes a scene belongs to. Held-out scenes are
/// scored once a candidate is fixed; they never teach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    Train,
    Tune,
    Heldout,
}

/// One labelled scene. Scenes in one `group` are too alike to judge each other
/// — one board drawn twice, one level at two sizes — so a group is never both
/// taught from and held out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub id: String,
    pub group: String,
    pub split: Split,
}

/// Labelled scenes split once, checked: no scene named twice and no group on
/// both sides of the held-out line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenes(BTreeMap<String, Scene>);

impl Scenes {
    pub fn new(scenes: Vec<Scene>) -> Result<Self, LearnError> {
        let mut groups: BTreeMap<&str, bool> = BTreeMap::new();
        for scene in &scenes {
            let heldout = scene.split == Split::Heldout;
            if *groups.entry(scene.group.as_str()).or_insert(heldout) != heldout {
                return Err(LearnError::Crossed);
            }
        }
        let count = scenes.len();
        let by_id: BTreeMap<String, Scene> = scenes
            .into_iter()
            .map(|scene| (scene.id.clone(), scene))
            .collect();
        if by_id.len() != count {
            return Err(LearnError::Duplicate);
        }
        Ok(Self(by_id))
    }

    #[must_use]
    pub fn split(&self, id: &str) -> Option<Split> {
        self.0.get(id).map(|scene| scene.split)
    }

    pub fn heldout(&self) -> impl Iterator<Item = &Scene> {
        self.0
            .values()
            .filter(|scene| scene.split == Split::Heldout)
    }
}

/// What the person's goal allows, as the planner wrote it from the goal.
/// Nothing on the screen fills it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Approval {
    pub inputs: BTreeSet<LeaseInput>,
    pub labels: BTreeSet<String>,
}

/// A candidate's held-out score from the independent oracle: the scenes
/// judged and the actions it would have taken wrongly.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeldoutScore {
    pub scenes: BTreeSet<String>,
    pub wrong_actions: u64,
}

/// The one table of what adoption requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adoption {
    /// Held-out scenes a score must cover (the realtime design's held-out
    /// gate for untuned scenes).
    pub min_heldout_scenes: u64,
}

pub const ADOPTION: Adoption = Adoption {
    min_heldout_scenes: 20,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnError {
    /// Two scenes share an id.
    Duplicate,
    /// A group is both taught from and held out.
    Crossed,
    /// The candidate learned from a held-out scene, or one the split does not
    /// know.
    Heldout,
    /// The score covers too few held-out scenes, a scene that is not held out
    /// or one the candidate learned from, or a wrong action.
    Unproven,
    /// The person's approval does not cover an input or a label.
    Unapproved,
    Spec(SpecError),
}

/// The candidate's colours as palette classes, in its order — never a rule,
/// an action or a lease: the plan that uses them is written and validated as
/// any other.
pub fn adopt(
    candidate: &Candidate,
    scenes: &Scenes,
    score: &HeldoutScore,
    approval: &Approval,
    limits: &PerceptionLimits,
) -> Result<Vec<ColorClass>, LearnError> {
    if candidate
        .scenes
        .iter()
        .any(|scene| !matches!(scenes.split(scene), Some(Split::Train | Split::Tune)))
    {
        return Err(LearnError::Heldout);
    }
    if (score.scenes.len() as u64) < ADOPTION.min_heldout_scenes
        || score.wrong_actions > 0
        || score.scenes.iter().any(|scene| {
            scenes.split(scene) != Some(Split::Heldout) || candidate.scenes.contains(scene)
        })
    {
        return Err(LearnError::Unproven);
    }
    if !candidate.inputs.is_subset(&approval.inputs)
        || candidate
            .classes
            .iter()
            .any(|class| !approval.labels.contains(&class.label))
    {
        return Err(LearnError::Unapproved);
    }
    let classes: Vec<ColorClass> = candidate
        .classes
        .iter()
        .map(|class| ColorClass {
            r: class.r,
            g: class.g,
            b: class.b,
            tolerance: class.tolerance,
        })
        .collect();
    check_palette(&classes, None, limits).map_err(LearnError::Spec)?;
    Ok(classes)
}
