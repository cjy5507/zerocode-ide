//! The forked phone step (t-6044, [`crate::jev::BRANCHING`]): which of the
//! screens a step's candidates led to is the one to keep.
//!
//! A goal walk on a phone asks the emulator seat which numbered control to
//! press, and presses the one it ranks first. When that answer ranks two or
//! more controls, the window can do what the walk cannot on a desktop: save
//! the device where it stands, press each of the top [`BRANCHING_K`] in turn,
//! read the screen each one leads to, put the device back — and ask which
//! RESULT is the closest to the goal. This module is that question and its
//! rules, and nothing else: which candidates a fork tries ([`top_k`]), what
//! the question carries ([`ask`]), what makes an answer one
//! ([`BranchAsk::read`]), how long a fork may hold the walk
//! ([`fork_budget_ms`]) and how the walk's own next step grades the pick
//! ([`agreed`]). Nothing here touches a device, the network or a clock; the
//! window's `computer_use::errand::branch` does the saving and pressing.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::jev::choice::{self, ChoiceRefusal};
use crate::jev::promote::permille;
use crate::jev::{
    BRANCHING, BRANCHING_APPLY_DEADLINE_MS, BRANCHING_FORK_MARGIN_PERMILLE, BRANCHING_K,
    BRANCHING_K_CAP, SCREEN_CANDIDATE_CAP,
};
use crate::screen_action::{ActionChoice, Chosen, Where, mark_of, option_of};

/// The one question's name. The endpoint does not show a question's name to
/// the model, so this is the caller's key and nothing more.
const QUESTION: &str = "best";

/// The words of the question. A screen's own words reach the model as state
/// and as an option's description, never as an instruction.
const INSTRUCTIONS: &str = "Someone wants to reach the goal in `goal` on the mobile screen described by `where`, and they can only press things. `before` lists the controls that screen showed, each as the number drawn on it, its role, its words and its centre. The walk saved the device, pressed each candidate in turn, read the screen it led to and put the device back. Every option is one of those candidates, named by the number it pressed: its `action` is the control's own legend line, and its `result` — when the walk explored it — says whether the screen `moved`, the `controls` the new screen shows and their `count`. A candidate with no `result` was not explored; judge it from its action alone. Choose the one candidate whose result is closest to the goal: prefer a screen that plainly moved toward the goal over one that did not move, and never a result that moved away from it.";

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 4] = ["goal", "where", "before", "candidates"];

/// The keys of one candidate, in the order the fingerprint reads them.
const CANDIDATE_KEYS: [&str; 3] = ["option", "action", "result"];

/// The keys of one explored result, in the order the fingerprint reads them.
const RESULT_KEYS: [&str; 3] = ["moved", "controls", "count"];

/// How an option's description says its candidate was not explored.
const UNEXPLORED: &str = "not explored";

/// The version of the words in this module. Bump it when any of them
/// changes: a judgment read under one wording is not evidence about another.
/// The test `the_version_is_pinned_to_the_words` holds it to
/// [`crate::jev::rubric_fingerprint`].
pub const BRANCHING_RUBRIC_VERSION: u32 = 1;

/// What one explored candidate led to, as the walk read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Whether the screen after the press differs from the screen before it,
    /// by the question's own view of a screen (its legend lines).
    pub moved: bool,
    /// The legend lines of the controls the new screen shows, top to bottom,
    /// uncut: the question cuts them to [`SCREEN_CANDIDATE_CAP`].
    pub controls: Vec<String>,
    /// How many controls the new screen showed in all.
    pub count: usize,
}

/// One candidate, as the fork tried it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The number the look drew on the control.
    pub mark: usize,
    /// The control's legend line — what was pressed.
    pub action: String,
    /// What pressing it led to; `None` when the fork did not explore it.
    pub result: Option<Outcome>,
}

/// What a fork is asking about.
#[derive(Debug, Clone)]
pub struct BranchLook<'a> {
    pub goal: &'a str,
    pub at: Where<'a>,
    /// The legend lines of the screen before the fork, uncut.
    pub before: &'a [String],
    /// The candidates, the emulator seat's own press first.
    pub candidates: &'a [Candidate],
}

/// One question, ready for the wire, holding the closed set it offered.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`.
    pub questions: Value,
    /// The numbers offered, in the order they were offered.
    marks: Vec<usize>,
}

/// A validated answer.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchChoice {
    /// The candidate to make canonical.
    pub mark: usize,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// The candidates a fork tries, from the emulator seat's own answer: the
/// control it chose first, then the others it gave any probability at all,
/// highest first — at most `k` and never more than [`BRANCHING_K_CAP`].
///
/// A control the seat gave no probability is not a candidate: the seat said
/// nothing for it, and a fork that explored it would be inventing an
/// alternative rather than comparing the ones the seat named. Ties keep the
/// lower number first, so two reads of one answer try the same list.
#[must_use]
pub fn top_k(choice: &ActionChoice, k: usize) -> Vec<usize> {
    let mut ranked: Vec<(usize, f64)> = choice
        .probabilities
        .iter()
        .filter_map(|(option, probability)| Some((mark_of(option)?, *probability)))
        .filter(|(_, probability)| *probability > 0.0)
        .collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.0.cmp(&right.0))
    });
    let mut marks: Vec<usize> = Vec::new();
    if let Chosen::Mark(chosen) = choice.chosen {
        marks.push(chosen);
    }
    for (mark, _) in ranked {
        if !marks.contains(&mark) {
            marks.push(mark);
        }
    }
    marks.truncate(k.min(BRANCHING_K_CAP));
    marks
}

/// The candidates a fork tries: the table's own `k` ([`BRANCHING_K`]) when
/// the seat is torn, the first choice alone when it is not.
///
/// Torn is the table's line (t-6155 F3): the first choice leads its
/// runner-up by under [`BRANCHING_FORK_MARGIN_PERMILLE`] of the answer's
/// mass, or sits under the seat's own press floor whatever its lead. A clear
/// lead is a single step — a fork costs the walk a save, `k` presses with a
/// look and a load each, and the comparison's wall, and buys nothing at a
/// step the seat was already sure of. Before the line, every answer that
/// gave a second control any weight at all forked. The lead is read in the
/// judge's own permille ([`permille`], floored), so the line is a number two
/// readers agree on exactly.
#[must_use]
pub fn fork_wanted(choice: &ActionChoice) -> Vec<usize> {
    let marks = top_k(choice, BRANCHING_K);
    let [first, second, ..] = marks.as_slice() else {
        return marks;
    };
    let weight = |mark: usize| {
        permille(
            choice
                .probabilities
                .get(&option_of(mark))
                .copied()
                .unwrap_or(0.0),
        )
    };
    let leader = weight(*first);
    let lead = leader.saturating_sub(weight(*second));
    let unsure = BRANCHING
        .press_floor_permille
        .is_some_and(|floor| leader < floor);
    if lead < BRANCHING_FORK_MARGIN_PERMILLE || unsure {
        marks
    } else {
        vec![*first]
    }
}

/// The words that define the question, as one string. The version is pinned
/// to this, not to a date or to a reviewer's memory.
#[must_use]
pub fn rubric_words() -> String {
    let mut words = String::from(INSTRUCTIONS);
    for line in [UNEXPLORED, QUESTION] {
        words.push('\n');
        words.push_str(line);
    }
    words.push('\n');
    words.push_str(&STATE_KEYS.join(","));
    words.push('\n');
    words.push_str(&CANDIDATE_KEYS.join(","));
    words.push('\n');
    words.push_str(&RESULT_KEYS.join(","));
    words.push('\n');
    words.push_str(&option_of(0));
    words
}

/// The controls a result carries: the first [`SCREEN_CANDIDATE_CAP`], cut
/// here as well as by the door, so the row that records what was asked and
/// the request that carried it hold the same cut.
fn controls_cut(controls: &[String]) -> Vec<&str> {
    controls
        .iter()
        .take(SCREEN_CANDIDATE_CAP)
        .map(String::as_str)
        .collect()
}

/// How an option's description says what its candidate led to.
fn means(candidate: &Candidate) -> String {
    match &candidate.result {
        None => format!("Pressed {} ({UNEXPLORED}).", candidate.action),
        Some(result) if !result.moved => {
            format!("Pressed {}; the screen did not move.", candidate.action)
        }
        Some(result) => format!(
            "Pressed {}; the screen moved to one showing {} control(s): {}.",
            candidate.action,
            result.count,
            controls_cut(&result.controls).join(" | ")
        ),
    }
}

/// The question this fork asks, or `None` when there is nothing to choose
/// between — fewer than two candidates. A caller that gets `None` presses
/// the first candidate exactly as it would have without a judgment at all.
#[must_use]
pub fn ask(look: &BranchLook<'_>) -> Option<BranchAsk> {
    if look.candidates.len() < 2 {
        return None;
    }
    let mut criteria = Map::new();
    let mut candidates = Vec::new();
    let mut marks = Vec::new();
    for candidate in look.candidates.iter().take(BRANCHING_K_CAP) {
        let option = option_of(candidate.mark);
        criteria.insert(option.clone(), Value::String(means(candidate)));
        let mut carried = json!({
            CANDIDATE_KEYS[0]: option,
            CANDIDATE_KEYS[1]: candidate.action,
        });
        if let Some(result) = &candidate.result {
            carried[CANDIDATE_KEYS[2]] = json!({
                RESULT_KEYS[0]: result.moved,
                RESULT_KEYS[1]: controls_cut(&result.controls),
                RESULT_KEYS[2]: result.count,
            });
        }
        candidates.push(carried);
        marks.push(candidate.mark);
    }
    let state = json!({
        STATE_KEYS[0]: look.goal,
        STATE_KEYS[1]: look.at.said(),
        STATE_KEYS[2]: controls_cut(look.before),
        STATE_KEYS[3]: candidates,
    });
    Some(BranchAsk {
        state,
        questions: choice::asked(QUESTION, INSTRUCTIONS, criteria),
        marks,
    })
}

impl BranchAsk {
    /// The numbers this question offered, in order.
    #[must_use]
    pub fn marks(&self) -> &[usize] {
        &self.marks
    }

    /// What the endpoint's `answers` map says about this question, judged
    /// against the set this question offered. One broken rule discards the
    /// answer whole.
    ///
    /// # Errors
    ///
    /// [`ChoiceRefusal`] names which rule the answer broke.
    pub fn read(&self, answers: &Value) -> Result<BranchChoice, ChoiceRefusal> {
        let offered: BTreeSet<String> = self.marks.iter().map(|mark| option_of(*mark)).collect();
        let choice = choice::read(answers, QUESTION, &offered)?;
        Ok(BranchChoice {
            mark: mark_of(&choice.chosen).ok_or(ChoiceRefusal::UnknownOption)?,
            probabilities: choice.probabilities,
            confidence: choice.confidence,
        })
    }
}

/// How long a fork of `k` candidates may hold the walk, in milliseconds,
/// from what the walk has already measured: one save, then per candidate a
/// step (a press and the look after it) and a load, then the canonical step
/// itself, then the comparison's own wall. `step_ms` is the walk's last look
/// on this surface and `snapshot_ms` the save it just made — the two numbers
/// the walk holds without guessing — so the budget is the brief's "step time
/// × k + the snapshot round trips" and not a constant nobody measured.
#[must_use]
pub const fn fork_budget_ms(k: usize, step_ms: u64, snapshot_ms: u64) -> u64 {
    let rounds = k as u64 + 1;
    rounds
        .saturating_mul(step_ms.saturating_add(snapshot_ms))
        .saturating_add(BRANCHING_APPLY_DEADLINE_MS)
}

/// What the walk's own next step showed about the canonical pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextStep {
    /// The caller's condition held, or the stopped step was cleared.
    Reached,
    /// The next look showed a screen that moved and the next judgment chose
    /// a control on it.
    MovedOn,
    /// The next look showed the same screen: the step did nothing, and what
    /// follows is a retry.
    SameScreen,
    /// The next judgment gave up on what the pick led to.
    GaveUp,
    /// The walk ended before a next look — its clock, a screen it could not
    /// read, a refusal. Nothing was shown either way.
    Unknown,
}

impl NextStep {
    /// Every outcome, in the order a ledger lists them.
    pub const ALL: [Self; 5] = [
        Self::Reached,
        Self::MovedOn,
        Self::SameScreen,
        Self::GaveUp,
        Self::Unknown,
    ];

    /// The word a ledger row keeps.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Reached => "reached",
            Self::MovedOn => "moved_on",
            Self::SameScreen => "same_screen",
            Self::GaveUp => "gave_up",
            Self::Unknown => "unknown",
        }
    }

    /// Whether the walk went on from the pick, when the step says.
    #[must_use]
    pub const fn went_on(self) -> Option<bool> {
        match self {
            Self::Reached | Self::MovedOn => Some(true),
            Self::SameScreen | Self::GaveUp => Some(false),
            Self::Unknown => None,
        }
    }
}

/// The mark the judge counts (§4 of the settings design): whether the
/// comparison's pick was the right one, read off the walk's own next step.
///
/// `same_pick` is whether the comparison named the candidate that was
/// actually pressed. Under an acting seat that is always so, and the mark is
/// simply whether the walk went on. Under a recording seat the pressed
/// candidate is the emulator seat's: a comparison that named the same one
/// shares its fate; one that named another was wrong when the press went on
/// fine, and says nothing when the press failed — the alternative nobody
/// tried is not evidence for it.
#[must_use]
pub const fn agreed(same_pick: bool, next: NextStep) -> Option<bool> {
    match (same_pick, next.went_on()) {
        (_, None) => None,
        (true, Some(went_on)) => Some(went_on),
        (false, Some(true)) => Some(false),
        (false, Some(false)) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::rubric_fingerprint;

    /// The version is pinned to the words: a wording change without a bump
    /// is a red test, not a quiet drift.
    #[test]
    fn the_version_is_pinned_to_the_words() {
        assert_eq!(BRANCHING_RUBRIC_VERSION, 1);
        assert_eq!(rubric_fingerprint(rubric_words), "8345cffb78ba4bf9");
    }

    fn choice(chosen: usize, spread: &[(usize, f64)]) -> ActionChoice {
        let mut probabilities: BTreeMap<String, f64> = spread
            .iter()
            .map(|(mark, probability)| (option_of(*mark), *probability))
            .collect();
        probabilities.insert("give_up".to_string(), 0.0);
        ActionChoice {
            chosen: Chosen::Mark(chosen),
            probabilities,
            confidence: 0.6,
        }
    }

    /// The candidates are the seat's own press first, then the rest by
    /// probability; a control the seat gave nothing is not one; the count is
    /// the table's `k` and never past its cap.
    #[test]
    fn the_candidates_are_the_seats_press_then_the_rest_it_gave_any_weight() {
        let ranked = choice(4, &[(1, 0.1), (4, 0.5), (7, 0.4), (9, 0.0)]);
        assert_eq!(top_k(&ranked, 3), [4, 7, 1]);
        assert_eq!(fork_wanted(&ranked), [4, 7], "the table's k");
        assert_eq!(top_k(&ranked, 10), [4, 7, 1], "never past the cap");
        // Ties keep the lower number first, so two reads try the same list.
        let tied = choice(2, &[(2, 0.4), (5, 0.3), (3, 0.3)]);
        assert_eq!(top_k(&tied, 3), [2, 3, 5]);
        // A press nobody else was given weight for is a single step.
        let alone = choice(10, &[(10, 1.0), (1, 0.0), (2, 0.0)]);
        assert_eq!(fork_wanted(&alone), [10]);
        assert!(ask(&look(&[candidate(10, None)])).is_none());
        // A judgment that gave up or said done ranks nothing first.
        let mut gave_up = choice(1, &[(1, 0.2), (2, 0.1)]);
        gave_up.chosen = Chosen::GiveUp;
        assert_eq!(top_k(&gave_up, 2), [1, 2]);
    }

    fn candidate(mark: usize, result: Option<(bool, &[&str])>) -> Candidate {
        Candidate {
            mark,
            action: format!("{mark} button 설정 @{},40", 100 + mark),
            result: result.map(|(moved, controls)| Outcome {
                moved,
                controls: controls.iter().map(|line| (*line).to_string()).collect(),
                count: controls.len() + 20,
            }),
        }
    }

    fn look(candidates: &[Candidate]) -> BranchLook<'_> {
        BranchLook {
            goal: "Wi-Fi 설정을 열어라",
            at: Where::Phone {
                platform: "android",
                device: "Pixel_6",
            },
            before: &[],
            candidates,
        }
    }

    /// The state carries the goal, the phone's address, the screen before,
    /// and each candidate's action and result — controls cut to the table's
    /// cap — and the criteria say what each candidate led to.
    #[test]
    fn the_question_carries_each_candidates_action_and_the_screen_it_led_to() {
        let many: Vec<String> = (0..SCREEN_CANDIDATE_CAP + 5)
            .map(|n| format!("{n} button 항목 @10,{n}"))
            .collect();
        let many_refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let candidates = [
            candidate(4, Some((true, &many_refs))),
            candidate(7, Some((false, &[]))),
            candidate(1, None),
        ];
        let before = vec!["4 button 네트워크 @104,40".to_string()];
        let asked = ask(&BranchLook {
            before: &before,
            ..look(&candidates)
        })
        .expect("two candidates are a question");
        assert_eq!(asked.marks(), [4, 7, 1]);
        assert_eq!(asked.state["goal"], "Wi-Fi 설정을 열어라");
        assert_eq!(
            asked.state["where"],
            json!({ "platform": "android", "device": "Pixel_6" })
        );
        assert_eq!(asked.state["before"], json!(["4 button 네트워크 @104,40"]));
        let carried = asked.state["candidates"].as_array().expect("a list");
        assert_eq!(carried.len(), 3);
        assert_eq!(carried[0]["option"], "mark:4");
        assert_eq!(carried[0]["result"]["moved"], true);
        assert_eq!(
            carried[0]["result"]["controls"].as_array().map(Vec::len),
            Some(SCREEN_CANDIDATE_CAP),
            "the result's controls are cut to the table's cap"
        );
        assert_eq!(carried[0]["result"]["count"], SCREEN_CANDIDATE_CAP + 25);
        assert_eq!(carried[1]["result"]["moved"], false);
        assert!(
            carried[2].get("result").is_none(),
            "an unexplored candidate carries no result"
        );
        let criteria = asked.questions[QUESTION]["criteria"]
            .as_object()
            .expect("criteria");
        assert_eq!(criteria.len(), 3);
        assert!(
            criteria["mark:7"]
                .as_str()
                .is_some_and(|said| said.contains("did not move"))
        );
        assert!(
            criteria["mark:1"]
                .as_str()
                .is_some_and(|said| said.contains(UNEXPLORED))
        );
        assert_eq!(asked.questions[QUESTION]["type"], "choice");
        // Past the cap, a fourth candidate is not offered at all.
        let four = [
            candidate(1, None),
            candidate(2, None),
            candidate(3, None),
            candidate(4, None),
        ];
        assert_eq!(
            ask(&look(&four)).expect("asked").marks().len(),
            BRANCHING_K_CAP
        );
    }

    fn answer(chosen: &str, spread: &[(&str, f64)]) -> Value {
        json!({
            QUESTION: {
                "type": "choice",
                "choice": chosen,
                "probabilities": spread.iter().cloned().collect::<BTreeMap<&str, f64>>(),
                "confidence": 0.8,
            }
        })
    }

    /// An answer names one of the offered numbers or is refused whole.
    #[test]
    fn an_answer_names_one_offered_candidate_or_is_refused_whole() {
        let candidates = [candidate(4, Some((true, &[]))), candidate(7, None)];
        let asked = ask(&look(&candidates)).expect("asked");
        let read = asked
            .read(&answer("mark:7", &[("mark:4", 0.3), ("mark:7", 0.7)]))
            .expect("in shape");
        assert_eq!(read.mark, 7);
        assert_eq!(read.confidence, 0.8);
        assert_eq!(
            asked.read(&answer("mark:9", &[("mark:4", 0.3), ("mark:7", 0.7)])),
            Err(ChoiceRefusal::UnknownOption)
        );
        assert_eq!(
            asked.read(&answer("mark:4", &[("mark:4", 0.5), ("mark:7", 0.7)])),
            Err(ChoiceRefusal::NotOne)
        );
        assert_eq!(asked.read(&json!({})), Err(ChoiceRefusal::NoAnswer));
    }

    /// The budget is the brief's arithmetic over measured numbers: a save,
    /// then a step and a load per candidate, the canonical step, the wall.
    #[test]
    fn the_budget_is_k_steps_and_snapshots_and_the_walls_own_number() {
        assert_eq!(
            fork_budget_ms(2, 1_000, 500),
            3 * 1_500 + BRANCHING_APPLY_DEADLINE_MS
        );
        assert_eq!(
            fork_budget_ms(3, 0, 0),
            BRANCHING_APPLY_DEADLINE_MS,
            "a free desk still pays the wall"
        );
        assert_eq!(fork_budget_ms(2, u64::MAX, 1), u64::MAX, "saturates");
    }

    /// A fork is worth its clock only when the seat is torn (t-6155 F3): the
    /// runner-up within the table's margin of the leader, or a leader under
    /// the press floor. A clear lead is a single step, however many controls
    /// were given some weight.
    #[test]
    fn a_fork_is_wanted_only_when_the_seat_is_torn() {
        use crate::jev::{BRANCHING, BRANCHING_FORK_MARGIN_PERMILLE, SCREEN_PRESS_FLOOR_PERMILLE};
        assert_eq!(
            BRANCHING.press_floor_permille,
            Some(SCREEN_PRESS_FLOOR_PERMILLE)
        );
        // A clear lead over a weighted runner-up: one step, and no question.
        let clear = choice(4, &[(4, 0.7), (7, 0.2), (1, 0.1)]);
        assert_eq!(fork_wanted(&clear), [4]);
        // The runner-up inside the margin: a fork of the table's k.
        let torn = choice(4, &[(4, 0.5), (7, 0.4), (1, 0.1)]);
        assert_eq!(fork_wanted(&torn), [4, 7]);
        // A leader under the press floor forks whatever its lead.
        let unsure = choice(4, &[(4, 0.45), (7, 0.15), (1, 0.1)]);
        assert_eq!(fork_wanted(&unsure), [4, 7]);
        // Exactly the margin is a clear lead: the line is "under".
        let at_line = choice(4, &[(4, 0.6), (7, 0.4)]);
        assert_eq!(
            crate::jev::promote::permille(0.6) - crate::jev::promote::permille(0.4),
            BRANCHING_FORK_MARGIN_PERMILLE
        );
        assert_eq!(fork_wanted(&at_line), [4]);
        // A chosen number the seat gave less weight than another is torn by
        // definition.
        let odd = choice(4, &[(4, 0.3), (7, 0.7)]);
        assert_eq!(fork_wanted(&odd), [4, 7]);
        // One candidate is never a fork, sure or not.
        let alone = choice(10, &[(10, 0.3)]);
        assert_eq!(fork_wanted(&alone), [10]);
        // The full list is still there for a reader that wants it.
        assert_eq!(top_k(&clear, 3), [4, 7, 1]);
    }

    /// The mark: the walk going on says the pick was right, a retry or a
    /// give-up says it was not, and an untried alternative says nothing.
    #[test]
    fn the_walks_next_step_writes_the_mark_and_an_untried_alternative_writes_none() {
        for next in NextStep::ALL {
            assert_eq!(agreed(true, next), next.went_on(), "{next:?}");
        }
        assert_eq!(agreed(false, NextStep::Reached), Some(false));
        assert_eq!(agreed(false, NextStep::MovedOn), Some(false));
        assert_eq!(agreed(false, NextStep::SameScreen), None);
        assert_eq!(agreed(false, NextStep::GaveUp), None);
        assert_eq!(agreed(false, NextStep::Unknown), None);
        let words: BTreeSet<&str> = NextStep::ALL.iter().map(|next| next.word()).collect();
        assert_eq!(words.len(), NextStep::ALL.len(), "one word each");
    }
}
