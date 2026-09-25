//! Where a worker the window just started should stand.
//!
//! The measured rule (`tilePlacement`, `ui/shell-term.js`) answers what the
//! layout can be asked: given that a pane is being cut, which way, and
//! whether there is room left to cut one at all. It cannot answer the
//! question a person actually has an opinion about — whether THIS worker is
//! one they want beside what they are already reading, or one that should
//! start and stay out of the way. That depends on why it was summoned, and
//! the layout does not know why.
//!
//! So this is a closed choice over the rooms the window can actually put a
//! worker in, and only the ones it can do RIGHT NOW: a question that offers
//! `split` to a window with four panes already open is a question whose
//! answer nobody could carry out. The caller hands in what it can honestly
//! report, and gets back the question it may ask and the set that answer will
//! be judged against — the two travel together so they cannot drift apart at
//! a call site.
//!
//! Nothing acts on the answer. [`crate::jev::PLACEMENT`] offers no mode that
//! applies, and the window keeps putting every worker in the one room it has
//! always used — its own unfocused tab. The reason is not that the other two
//! rooms are unreachable (`tileTermPane` and `detachedAgents` are both
//! standing surfaces) but that no row anywhere says a judgment would place a
//! worker better than the checkout and the seat already do. The question can
//! be asked and recorded (`cmd::worker_room`); until those rows exist,
//! nothing calls it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::jev::choice;
use crate::jev::{PLACEMENT_BRIEF_CHAR_CAP, PLACEMENT_OPTIONS};

/// The one question's name — the caller's key for reading the answer back.
/// The endpoint never shows a question's name to the model.
const QUESTION: &str = "placement";

/// The words of the question. They are ours: a summons's own text reaches the
/// model as state, never as an instruction.
const INSTRUCTIONS: &str = "An agent has just been started to do the work described in `brief`. Choose where its window should stand for the person described in the rest of the state. Judge only what would serve someone working right now: whether they would want to watch this agent beside what they are already looking at, glance at it later, or not have it take the screen at all.";

/// What each room means, as a situation rather than as a degree — every
/// option is judged on its own words.
const TAB_MEANS: &str = "Its own tab, behind the one in front. The person can go to it when they want it, and nothing they are looking at moves.";
const SPLIT_MEANS: &str = "Beside what is in front of them, sharing the screen. The work in front and this agent's work are the same piece of work, and seeing both at once is the point.";
const BACKGROUND_MEANS: &str = "Started with no window on the stage at all. Nobody is waiting on it, and a window would only be something to close.";

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 5] = ["brief", "startedBy", "inFront", "sameWorkspace", "panes"];

/// The version of the words above. Bump it when any of them changes: a
/// judgment read under one wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to [`crate::jev::rubric_fingerprint`],
/// so changing a word without bumping the version is a red test rather than a
/// quiet drift.
///
/// Or when the label they are graded by changes: version 2 asks version 1's
/// words and grades an answer only on a pane that tried its room ([`mark`],
/// t-9427), where version 1's quiet label graded a recorded answer against
/// the tab its pane was put in. The version rides every request row, so the
/// judge reads the two series apart (t-6877).
pub const WORKER_PLACEMENT_RUBRIC_VERSION: u32 = 2;

/// Who started this worker. The one fact that most changes the answer and the
/// one the window always knows for certain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartedBy {
    /// Somebody pressed something. They are at the keyboard now.
    Person,
    /// A schedule, a hook or a coordinator fired it. Nobody is waiting.
    Schedule,
}

impl StartedBy {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Schedule => "schedule",
        }
    }

    /// The reading a word names, if it names one this file knows.
    ///
    /// The window answers in these words rather than in a boolean, because
    /// whether somebody is at the keyboard is the WINDOW's fact — it holds
    /// the focus and the visibility the browser reports — while whether
    /// anybody is waiting on the run is the LEDGER's. The two are folded into
    /// one word where both are true, and this reads it back.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        match word {
            "person" => Some(Self::Person),
            "schedule" => Some(Self::Schedule),
            _ => None,
        }
    }
}

/// What is on the stage when the worker starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InFront {
    /// A terminal tab — the only surface a worker can be tiled into.
    Terminal,
    /// A browser page or a document; a shell cannot tile into either.
    Page,
    /// Nothing is on the stage.
    Nothing,
}

impl InFront {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Page => "page",
            Self::Nothing => "nothing",
        }
    }

    /// The surface a word names, if it names one this file knows.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        match word {
            "terminal" => Some(Self::Terminal),
            "page" => Some(Self::Page),
            "nothing" => Some(Self::Nothing),
            _ => None,
        }
    }
}

/// What the window can honestly report about a worker it is about to place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacementLook<'a> {
    /// Why this worker was summoned, in the summoner's own words.
    pub brief: &'a str,
    pub started_by: StartedBy,
    pub in_front: InFront,
    /// Whether the worker's checkout is the one whose tabs the stage is
    /// drawing. The stage draws one checkout, so a worker belonging to
    /// another cannot be a pane of what is in front.
    pub same_workspace: bool,
    /// How many panes the tab in front already holds.
    pub panes: usize,
    /// Whether the measured rule would have room to cut a pane at all. The
    /// caller decides this, because the cap and the sides are the rule's.
    pub may_split: bool,
}

/// A room a worker can be put in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Tab,
    Split,
    Background,
}

impl Placement {
    /// The option word, as the question offers it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Tab => "tab",
            Self::Split => "split",
            Self::Background => "background",
        }
    }

    /// The room every worker stood in before the seat existed, and still
    /// stands in when the seat only records: its own tab — the fallback of
    /// the surface that seats it (`roomForWorker`, `ui/shell.js`).
    pub const TODAYS: Self = Self::Tab;

    /// The room an option names, if it names one this file knows.
    #[must_use]
    pub fn of(option: &str) -> Option<Self> {
        match option {
            "tab" => Some(Self::Tab),
            "split" => Some(Self::Split),
            "background" => Some(Self::Background),
            _ => None,
        }
    }

    fn means(self) -> &'static str {
        match self {
            Self::Tab => TAB_MEANS,
            Self::Split => SPLIT_MEANS,
            Self::Background => BACKGROUND_MEANS,
        }
    }
}

/// One question and the set its answer is judged against.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`.
    pub questions: Value,
    /// The rooms offered, in the order they were offered.
    offered: Vec<Placement>,
}

/// A validated answer.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementChoice {
    pub chosen: Placement,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// The words that define the question, as one string. The version is pinned
/// to this, not to a date or to a reviewer's memory.
#[must_use]
pub fn rubric_words() -> String {
    let mut words = String::new();
    words.push_str(INSTRUCTIONS);
    for room in [Placement::Tab, Placement::Split, Placement::Background] {
        words.push('\n');
        words.push_str(room.key());
        words.push('\n');
        words.push_str(room.means());
    }
    words.push('\n');
    words.push_str(&STATE_KEYS.join(","));
    words
}

/// The question this look asks.
///
/// `split` is offered only where the window could carry it out; the other two
/// rooms are always reachable. The brief is cut to the table's cap here as
/// well as at the door, so the state a caller records beside the answer is the
/// state that was asked about.
#[must_use]
pub fn ask(look: &PlacementLook<'_>) -> PlacementAsk {
    let mut offered = vec![Placement::Tab];
    if look.may_split && look.same_workspace && look.in_front == InFront::Terminal {
        offered.push(Placement::Split);
    }
    offered.push(Placement::Background);

    let mut criteria = Map::new();
    for room in &offered {
        criteria.insert(room.key().to_string(), Value::from(room.means()));
    }
    let state = Value::Object(Map::from_iter([
        (
            "brief".to_string(),
            Value::from(crate::jev::door::cut(
                look.brief,
                crate::jev::Cap::Chars(PLACEMENT_BRIEF_CHAR_CAP),
            )),
        ),
        ("startedBy".to_string(), Value::from(look.started_by.key())),
        ("inFront".to_string(), Value::from(look.in_front.key())),
        (
            "sameWorkspace".to_string(),
            Value::from(look.same_workspace),
        ),
        ("panes".to_string(), Value::from(look.panes)),
    ]));
    let questions = choice::asked(QUESTION, INSTRUCTIONS, criteria);
    PlacementAsk {
        state,
        questions,
        offered,
    }
}

impl PlacementAsk {
    /// The rooms this question offered, in order.
    #[must_use]
    pub fn offered(&self) -> &[Placement] {
        &self.offered
    }

    /// The option words this question offered.
    #[must_use]
    pub fn options(&self) -> Vec<String> {
        self.offered
            .iter()
            .map(|room| room.key().to_string())
            .collect()
    }

    /// What `answers` says about the question that was asked.
    ///
    /// # Errors
    ///
    /// [`crate::jev::choice::ChoiceRefusal`] names the first rule the answer
    /// broke. An option word the table knows but this question did not offer
    /// is already refused by the reader, which is judged against the offered
    /// set and nothing wider.
    pub fn read(
        &self,
        answers: &Value,
    ) -> Result<PlacementChoice, crate::jev::choice::ChoiceRefusal> {
        let offered: BTreeSet<String> = self.options().into_iter().collect();
        let choice = crate::jev::choice::read(answers, QUESTION, &offered)?;
        let chosen = Placement::of(&choice.chosen)
            .ok_or(crate::jev::choice::ChoiceRefusal::UnknownOption)?;
        Ok(PlacementChoice {
            chosen,
            probabilities: choice.probabilities,
            confidence: choice.confidence,
        })
    }
}

/// The room a placed worker's pane stood in while nobody moved it (t-6342):
/// the room the answer named when the seat seated it, and today's room
/// ([`Placement::TODAYS`]) when the seat only recorded.
///
/// The quiet label used to write the answer's room either way, so a recorded
/// `split` whose pane sat in its own tab for five minutes went down as a split
/// the person had left alone — eleven of the thirty marks this machine's
/// ledger held on 2026-09-23 were such panes.
#[must_use]
pub const fn stood_in(chosen: Placement, applied: bool) -> Placement {
    if applied { chosen } else { Placement::TODAYS }
}

/// The word a placement label carries under the summary's `notCompared` when
/// nobody was in front of the pane while its label's window was open.
pub const UNSEEN: &str = "unseen";

/// The word a placement label carries under the summary's `notCompared` when
/// the room the answer named was never tried (t-9427): the seat only
/// recorded, the pane stood in today's room, and nobody moved it. The effort
/// seats' word for an answer the product never carried out, read from there
/// so one fact has one spelling.
pub const NOT_CARRIED: &str = crate::step_effort::NOT_CARRIED;

/// The placement seat's mark (t-6342, t-9427): whether the room the pane
/// ended the label's window in is the room the answer named — counted only
/// for a pane somebody could have moved, and only where the pane tried the
/// answer's room.
///
/// A pane the person `moved` was seen, and their move chose a room: it
/// grades every answer, right or wrong. One nobody moved says something only
/// if it stood on the stage, with the window in front, for
/// [`crate::jev::PLACEMENT_SEEN_DWELL_MS`] — every one of the thirty marks
/// this machine's ledger held on 2026-09-23 said the pane was left where it
/// was, and so would any answer's have: a label that cannot tell "nobody
/// looked" from "looked and kept it" cannot say no. And then it says only
/// that the room it stood in would do: it grades the answer that named that
/// room, and nothing of one that named another. A recording seat's pane
/// stands in today's tab whatever the answer said ([`stood_in`]), so a
/// recorded `split` nobody moved was a split nobody tried — version 1
/// graded all 22 of this machine's seen ones wrong against the tab
/// (2026-09-26), while the one split the seat seated and a person left in
/// place was graded right, and no person moved any of the 169 panes.
///
/// # Errors
///
/// [`UNSEEN`] for a pane nobody was in front of, and [`NOT_CARRIED`] for one
/// nobody moved that stood in a room the answer did not name.
pub fn mark(
    chosen: Placement,
    ended_in: Placement,
    moved: bool,
    seen: bool,
) -> Result<bool, &'static str> {
    if !seen {
        Err(UNSEEN)
    } else if moved || chosen == ended_in {
        Ok(chosen == ended_in)
    } else {
        Err(NOT_CARRIED)
    }
}

/// What the placement seat's baseline — today's room, the tab — would have
/// been marked on the same pane (t-6342): on exactly the panes [`mark`]
/// grades the answer on, so the two readers are held to the same marks, and
/// against the room the pane ended in.
#[must_use]
pub fn baseline_mark(
    chosen: Placement,
    ended_in: Placement,
    moved: bool,
    seen: bool,
) -> Option<bool> {
    mark(chosen, ended_in, moved, seen)
        .ok()
        .map(|_| ended_in == Placement::TODAYS)
}

/// Every room, in the order the table spells them — what a reader of
/// [`PLACEMENT_OPTIONS`] gets as typed values.
#[must_use]
pub fn every_room() -> Vec<Placement> {
    PLACEMENT_OPTIONS
        .iter()
        .filter_map(|word| Placement::of(word))
        .collect()
}

#[cfg(test)]
mod tests;
