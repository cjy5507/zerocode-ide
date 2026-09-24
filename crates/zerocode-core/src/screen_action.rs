//! Choosing one of the actions a look already numbered
//! (docs/design/jev-browser-action-20260917.md §2.2): the question a walk asks
//! about the screen in front of it, and what makes an answer valid.
//!
//! Two errands ask it, and the module is named for what they share — a screen
//! whose controls are already numbered — rather than for the first of them:
//!
//! - [`Errand::Clear`]: a recorded walk stopped at a failed step or check, and
//!   one press should let that step run again. This is the errand the module
//!   was written for, and the one that only ever presses twice.
//! - [`Errand::Goal`]: nothing has failed; a goal was named in a person's own
//!   words and every step of reaching it is this question asked again. The
//!   walk ends when the judgment says the goal is reached, when nothing on the
//!   screen would help, when the screen stops moving under the presses, or
//!   when the step budget is spent.
//!
//! Two surfaces answer it ([`Where`]), because the same closed choice fits
//! both: a browser pane numbers a page's controls and the desktop's
//! accessibility tree numbers an app's, through the same marks table, the same
//! legend and the same pin.
//!
//! Nothing here touches the network, the clock or a page. The wire is the
//! window's (`crates/zerocode-shell/src/systemone.rs`) and what it sends is
//! what this module rendered, so the words that define the question live in
//! exactly one place.
//!
//! The rule the whole design rests on: **the answer space is closed.** A look
//! hands in the controls it saw, this module offers exactly those numbers plus
//! [`GIVE_UP`] (and, for a goal, [`DONE`]), and an answer naming anything else
//! is refused whole. A judgment cannot reach an element the look did not see,
//! and it never writes a selector, a URL, a script or a value — it writes a
//! number, which the caller spends on `click --mark <n>`, through the same pin
//! and the same fingerprint every other press goes through.
//!
//! What a look may put in the state is likewise narrow. A step's argv is NOT
//! state: `type <label> <css> <text>` carries the text a person typed, so
//! [`Errand::Clear`]'s `step` takes the verb alone.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::computer_use_protocol::marks::legend_line;
use crate::jev::choice::{self, ChoiceRefusal};
use crate::jev::noul::{self, NoulRefusal};
// The first guard — whether the screen's own text tells an assistant what to
// do (t-6187) — is named and worded in the question catalog, where the tool
// text guard reads the same words of every block a tool hands back (t-6348).
use crate::jev::questions::{
    INSTRUCTED, SCREEN_INSTRUCTED_ASKS as INSTRUCTED_INSTRUCTIONS,
    SCREEN_INSTRUCTED_NO as INSTRUCTED_NO, SCREEN_INSTRUCTED_YES as INSTRUCTED_YES,
};

/// Every way an answer fails to be one: a rule of its closed choice, which
/// every Jev question with a closed answer space keeps (`crate::jev::choice`),
/// or a rule of one of the two guards asked beside it ([`crate::jev::noul`]).
/// Either discards the answer whole — a judgment that got one part of its
/// shape wrong has said nothing about the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRefusal {
    Choice(ChoiceRefusal),
    Guard(NoulRefusal),
}

impl ActionRefusal {
    /// The word a ledger row writes for this refusal — the broken rule's own.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Choice(refusal) => refusal.token(),
            Self::Guard(refusal) => refusal.token(),
        }
    }
}

impl From<ChoiceRefusal> for ActionRefusal {
    fn from(refusal: ChoiceRefusal) -> Self {
        Self::Choice(refusal)
    }
}

impl From<NoulRefusal> for ActionRefusal {
    fn from(refusal: NoulRefusal) -> Self {
        Self::Guard(refusal)
    }
}

/// How many of a look's numbers one question may offer. The numbers
/// themselves are already capped by
/// [`crate::computer_use::MARK_CAP`] (99); a choice with ninety-nine options
/// is not a question worth asking, so the judgment keeps a tighter cap of its
/// own. The slice that is cut and the slice that may be chosen from are the
/// same slice — offering a number the reader then cannot pick is how one
/// screen comes to have two truths. The number is the Jev use table's
/// ([`crate::jev::SCREEN_CANDIDATE_CAP`]), where every use's caps are read
/// from, and both surfaces read the same one: the question is the same
/// question.
pub const MAX_ACTION_CANDIDATES: usize = crate::jev::SCREEN_CANDIDATE_CAP;

/// The option that means "none of these would help".
pub const GIVE_UP: &str = "give_up";

/// The option that means "the goal is reached; press nothing". Offered to a
/// goal walk alone: a walk clearing an obstacle is not the judge of whether
/// the document it interrupted is finished — the document's own re-walk is.
pub const DONE: &str = "done";

/// What a mark's option is called. The number after it is the number the look
/// handed out, so the caller spends the answer without a table of its own.
const MARK_OPTION_PREFIX: &str = "mark:";

/// The one question's name. The endpoint does not show a question's name to
/// the model, so this is the caller's key and nothing more.
const QUESTION: &str = "action";

/// The words of the question when a recorded walk stopped. They are ours: a
/// page's own text reaches the model as an option's description and as state,
/// never as an instruction.
const CLEAR_INSTRUCTIONS: &str = "A recorded browser flow stopped at the step named in `stopped`, for the reason in `refusal`. The controls the page is showing right now are the options, each named by the number the screen drew on it. `shows` is the text the page displays right now that is not a control, and `pressed` is what this walk already pressed, oldest first. Choose the one control a person would press so that the stopped step can run again.";

/// The words of the question when a goal was named and nothing has failed.
const GOAL_INSTRUCTIONS: &str = "Someone wants to reach the goal in `goal` on the screen described by `where`, and they can only press things. The controls that screen is showing right now are the options, each named by the number the screen drew on it. `shows` is the text the screen displays right now that is not a control — a value, a heading, a message — and `pressed` is what this walk already pressed, oldest first. Read the two together to tell how far along the goal already is, and choose the one control a person would press NEXT to get closer to it: never a press whose effect `shows` already carries, and never the same control again unless the goal itself repeats it.";

/// What choosing [`GIVE_UP`] means. Written as a situation rather than as a
/// degree, because each option is judged on its own words.
const GIVE_UP_MEANS: &str =
    "No control on this screen would let the stopped step run again — the flow needs a person.";

/// What [`GIVE_UP`] means to a goal walk: the same situation, said about the
/// goal rather than about a stopped step.
const GOAL_GIVE_UP_MEANS: &str =
    "No control on this screen would get any closer to the goal — this needs a person.";

/// What [`DONE`] means.
const DONE_MEANS: &str = "The goal has already been reached on this screen; nothing more to press.";

/// The second guard's name: whether the screen is a wall in front of the
/// page the goal expects — a sign-in, a captcha, an error dialog (t-6187).
/// Pressing on a wall is pressing on a page the goal never named.
const WALLED: &str = "walled";

/// The words the wall guard asks.
const WALLED_INSTRUCTIONS: &str = "Compare this screen with what the goal in `goal` needs. Is this screen a wall standing in front of that page instead of the page itself — a sign-in or log-in form, a captcha or robot check, or an error dialog?";

/// What yes means for the wall guard.
const WALLED_YES: &str = "The screen is a sign-in form, a captcha or robot check, or an error dialog, not the page the goal expects.";

/// What no means for the wall guard.
const WALLED_NO: &str = "The screen is the page the goal works on, or a step on the way to it.";

/// The state's keys a stopped walk fills, in the order the fingerprint reads
/// them.
const CLEAR_STATE_KEYS: [&str; 3] = ["stopped", "step", "refusal"];

/// The state's keys both errands fill.
const STATE_KEYS: [&str; 5] = ["goal", "where", "alreadyTried", "pressed", "shows"];

/// How much of the screen's own text a question carries: the newest-first
/// cut is the top of the screen, because that is where a display value, a
/// title or a banner stands. Forty lines is more than a calculator's one and
/// a settings pane's dozen; 1,500 characters is about what the legend of
/// [`MAX_ACTION_CANDIDATES`] controls already costs, so a screen's words never
/// more than double the request the door redacts and the deadline bounds.
/// A screen that shows more is cut, and the row's `showsLines` says so.
pub const SHOWS_LINE_CAP: usize = 40;
pub const SHOWS_CHAR_CAP: usize = 1_500;

/// The key the numbered controls sit under.
const CANDIDATES_KEY: &str = "candidates";

/// The version of the words above. Bump it when any of them changes: a
/// judgment read under one wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to
/// [`crate::jev::rubric_fingerprint`], so changing a word without bumping the version is
/// a red test rather than a quiet drift.
///
/// One version covers both errands' words. A row says which errand it was
/// ([`Errand::key`]), so evidence is read per errand; what a single version
/// buys is that neither errand's words can change while the other's evidence
/// silently keeps its number.
pub const SCREEN_ACTION_RUBRIC_VERSION: u32 = 5;

/// What a walk is asking the screen about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errand<'a> {
    /// A recorded walk stopped and one press should clear what stopped it.
    Clear {
        /// Why the walk stopped, in the walk's own word (`step_failed`, …).
        stopped: &'a str,
        /// The stopped step's VERB alone. Never its argv: a `type` line
        /// carries what a person typed.
        step: &'a str,
        /// What the step was refused with.
        refusal: &'a str,
    },
    /// A goal named in a person's words, with nothing failed.
    Goal,
}

impl Errand<'_> {
    /// The word a ledger row and a refusal use for this errand.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Clear { .. } => "clear",
            Self::Goal => "goal",
        }
    }

    /// Whether this errand may answer [`DONE`] — whether "we are there" is a
    /// question this errand's caller asked.
    #[must_use]
    pub const fn ends_itself(self) -> bool {
        matches!(self, Self::Goal)
    }

    const fn instructions(self) -> &'static str {
        match self {
            Self::Clear { .. } => CLEAR_INSTRUCTIONS,
            Self::Goal => GOAL_INSTRUCTIONS,
        }
    }

    const fn give_up_means(self) -> &'static str {
        match self {
            Self::Clear { .. } => GIVE_UP_MEANS,
            Self::Goal => GOAL_GIVE_UP_MEANS,
        }
    }
}

/// The screen the walk is looking at, as the question names it. Never more
/// than the surface's own address: a page's query carries tokens and a
/// window's contents are the candidates, not the address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where<'a> {
    /// A browser pane: the page's host and its path.
    Page { host: &'a str, path: &'a str },
    /// The desktop: the app whose tree was numbered, and the window's title.
    Desk { app: &'a str, window: &'a str },
    /// A mobile device, by the platform and identity the caller selected.
    Phone { platform: &'a str, device: &'a str },
}

impl Where<'_> {
    /// The address as the state carries it — the two keys of whichever
    /// surface this is. Shared with the forked step's question
    /// (`crate::branching`), so the two seats spell one address one way.
    #[must_use]
    pub fn said(self) -> Value {
        match self {
            Self::Page { host, path } => json!({ "host": host, "path": path }),
            Self::Desk { app, window } => json!({ "app": app, "window": window }),
            Self::Phone { platform, device } => json!({ "platform": platform, "device": device }),
        }
    }

    /// The keys this surface's address uses, for the words the version pins.
    const fn keys(self) -> [&'static str; 2] {
        match self {
            Self::Page { .. } => ["host", "path"],
            Self::Desk { .. } => ["app", "window"],
            Self::Phone { .. } => ["platform", "device"],
        }
    }
}

/// The screen a walk is looking at, as the question reads it.
#[derive(Debug, Clone, Copy)]
pub struct ActionLook<'a> {
    /// What the walk is for — the flow's name and the check that stopped, or
    /// the sentence a person wrote.
    pub goal: &'a str,
    /// Why it is asking.
    pub errand: Errand<'a>,
    /// The surface and its address.
    pub at: Where<'a>,
    /// Numbers this walk already spent ON THIS SCREEN. They are not offered
    /// again, so a second guess cannot repeat the first; a screen that moved
    /// is a new screen and hands back an empty list.
    pub tried: &'a [usize],
    /// The marks answer's items, in the order the look numbered them.
    pub items: &'a [Value],
    /// What this walk already pressed, oldest first — each the legend line of
    /// the control as it was numbered when pressed. Unlike `tried`, it is not
    /// forgotten when the screen moves: a display that changed because of
    /// the last press is exactly what the next question needs to know.
    pub pressed: &'a [String],
    /// The text the screen shows right now that is not a control — a display
    /// value, a heading, a message — as the surface read it, top to bottom.
    /// Empty when the surface reads no text; cut to [`SHOWS_LINE_CAP`] and
    /// [`SHOWS_CHAR_CAP`] here, so every surface is cut the same way.
    pub shows: &'a [String],
}

/// The screen's text as the question carries it: whole lines from the top,
/// until either cap is reached.
#[must_use]
pub fn shows_cut(shows: &[String]) -> Vec<&str> {
    let mut kept = Vec::new();
    let mut chars = 0usize;
    for line in shows
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
    {
        if kept.len() == SHOWS_LINE_CAP || chars + line.chars().count() > SHOWS_CHAR_CAP {
            break;
        }
        chars += line.chars().count();
        kept.push(line);
    }
    kept
}

/// One question, ready for the wire, holding the closed set it offered.
///
/// [`Self::read`] is a method rather than a free function so that an answer is
/// always judged against the very set that was asked — the two cannot drift
/// apart at a call site.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`.
    pub questions: Value,
    /// The numbers offered, in the order they were offered.
    marks: Vec<usize>,
    /// Whether this question offered [`DONE`].
    ends_itself: bool,
}

/// What an answer chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chosen {
    /// Press this number.
    Mark(usize),
    /// Nothing here helps.
    GiveUp,
    /// The goal is already reached; press nothing.
    Done,
}

/// A validated answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionChoice {
    pub chosen: Chosen,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
    /// What the two guards asked beside the choice said — `None` for an
    /// answer nothing asked them of: a second reader's closed choice
    /// ([`ActionAsk::choice_of`]).
    pub guard: Option<Guard>,
}

/// What the two guards a screen question asks beside its choice said, each a
/// probability of yes (t-6187).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guard {
    /// That the screen's own text tells an assistant what to do.
    pub instructed: f64,
    /// That the screen is a wall in front of the page the goal expects.
    pub walled: f64,
}

/// Which guard stops a press ([`Guard::stops`]) — the word a row's `barred`
/// carries for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// The screen's text tells an assistant what to do.
    Injected,
    /// The screen is a wall, not the page.
    Walled,
}

impl Stopped {
    /// The word a row and a walk's answer name this stop by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Injected => "injected",
            Self::Walled => "walled",
        }
    }
}

impl Guard {
    /// The guard that stops a press, if one does: its probability at or over
    /// [`crate::jev::SCREEN_INSTRUCTED_FLOOR_PERMILLE`], read in the floor's
    /// own units. An instruction is named before a wall — obeying a sentence
    /// on the screen is the worse of the two presses.
    #[must_use]
    pub fn stops(self) -> Option<Stopped> {
        let over = |yes: f64| {
            crate::jev::promote::permille(yes) >= crate::jev::SCREEN_INSTRUCTED_FLOOR_PERMILLE
        };
        if over(self.instructed) {
            Some(Stopped::Injected)
        } else if over(self.walled) {
            Some(Stopped::Walled)
        } else {
            None
        }
    }

    /// Both probabilities as the row keeps them, per thousand: the words the
    /// row names them by, and the floored numbers.
    #[must_use]
    pub fn permille(self) -> [(&'static str, u16); 2] {
        [
            (INSTRUCTED, crate::jev::promote::permille(self.instructed)),
            (WALLED, crate::jev::promote::permille(self.walled)),
        ]
    }
}

/// A mark's option name — `mark:7`. Shared with the forked step's question
/// (`crate::branching`), which offers the same numbers as options so a
/// ledger reads one spelling for "the control numbered 7" across both seats.
#[must_use]
pub fn option_of(mark: usize) -> String {
    format!("{MARK_OPTION_PREFIX}{mark}")
}

/// The number an option names, if it names one.
#[must_use]
pub fn mark_of(option: &str) -> Option<usize> {
    option.strip_prefix(MARK_OPTION_PREFIX)?.parse().ok()
}

/// The words that define the question, as one string — every errand's and
/// every surface's, so a word changed in any of them moves the fingerprint.
/// The version is pinned to this, not to a date or to a reviewer's memory.
#[must_use]
pub fn rubric_words() -> String {
    let mut words = String::new();
    for line in [
        CLEAR_INSTRUCTIONS,
        GOAL_INSTRUCTIONS,
        GIVE_UP,
        GIVE_UP_MEANS,
        GOAL_GIVE_UP_MEANS,
        DONE,
        DONE_MEANS,
        MARK_OPTION_PREFIX,
        INSTRUCTED,
        INSTRUCTED_INSTRUCTIONS,
        INSTRUCTED_YES,
        INSTRUCTED_NO,
        WALLED,
        WALLED_INSTRUCTIONS,
        WALLED_YES,
        WALLED_NO,
    ] {
        words.push_str(line);
        words.push('\n');
    }
    words.push_str(&STATE_KEYS.join(","));
    words.push('\n');
    words.push_str(&CLEAR_STATE_KEYS.join(","));
    words.push('\n');
    words.push_str(&Where::Page { host: "", path: "" }.keys().join(","));
    words.push('\n');
    words.push_str(
        &Where::Desk {
            app: "",
            window: "",
        }
        .keys()
        .join(","),
    );
    words.push('\n');
    words.push_str(CANDIDATES_KEY);
    words.push('\n');
    words.push_str(
        &Where::Phone {
            platform: "",
            device: "",
        }
        .keys()
        .join(","),
    );
    words
}

/// The question this look asks, or `None` when there is nothing left to
/// choose between — no control the look saw that this walk has not already
/// spent on this screen. A caller that gets `None` stops exactly as it would
/// have without a judgment at all.
#[must_use]
pub fn ask(look: &ActionLook<'_>) -> Option<ActionAsk> {
    let tried: BTreeSet<usize> = look.tried.iter().copied().collect();
    let mut marks = Vec::new();
    let mut criteria = Map::new();
    let mut candidates = Vec::new();
    for item in look.items {
        if marks.len() == MAX_ACTION_CANDIDATES {
            break;
        }
        let Some(mark) = item.get("mark").and_then(Value::as_u64) else {
            continue;
        };
        let Ok(mark) = usize::try_from(mark) else {
            continue;
        };
        if tried.contains(&mark) {
            continue;
        }
        let Some(line) = legend_line(item) else {
            continue;
        };
        criteria.insert(option_of(mark), Value::String(line.clone()));
        candidates.push(Value::String(line));
        marks.push(mark);
    }
    if marks.is_empty() {
        return None;
    }
    criteria.insert(
        GIVE_UP.to_string(),
        Value::String(look.errand.give_up_means().to_string()),
    );
    let ends_itself = look.errand.ends_itself();
    if ends_itself {
        criteria.insert(DONE.to_string(), Value::String(DONE_MEANS.to_string()));
    }

    let mut state = Map::new();
    state.insert(STATE_KEYS[0].to_string(), json!(look.goal));
    if let Errand::Clear {
        stopped,
        step,
        refusal,
    } = look.errand
    {
        state.insert(CLEAR_STATE_KEYS[0].to_string(), json!(stopped));
        state.insert(CLEAR_STATE_KEYS[1].to_string(), json!(step));
        state.insert(CLEAR_STATE_KEYS[2].to_string(), json!(refusal));
    }
    state.insert(STATE_KEYS[1].to_string(), look.at.said());
    state.insert(STATE_KEYS[2].to_string(), json!(look.tried));
    state.insert(STATE_KEYS[3].to_string(), json!(look.pressed));
    state.insert(STATE_KEYS[4].to_string(), json!(shows_cut(look.shows)));
    state.insert(CANDIDATES_KEY.to_string(), Value::Array(candidates));
    let mut questions = choice::asked(QUESTION, look.errand.instructions(), criteria);
    // The two guards ride the same request: the state is charged once and an
    // answer's output is free, so asking them costs their own two lines.
    if let Some(asked) = questions.as_object_mut() {
        asked.insert(
            INSTRUCTED.to_string(),
            noul::question(INSTRUCTED_INSTRUCTIONS, INSTRUCTED_YES, INSTRUCTED_NO),
        );
        asked.insert(
            WALLED.to_string(),
            noul::question(WALLED_INSTRUCTIONS, WALLED_YES, WALLED_NO),
        );
    }
    Some(ActionAsk {
        state: Value::Object(state),
        questions,
        marks,
        ends_itself,
    })
}

impl ActionAsk {
    /// The numbers this question offered, in order.
    #[must_use]
    pub fn marks(&self) -> &[usize] {
        &self.marks
    }

    /// A second reader's answer to this question — one option and one
    /// confidence, as a frontier model asked the same closed choice answers
    /// it (t-6132 S3) — judged against the set this question offered. The
    /// rules are the closed choice's own: an option nobody offered, and a
    /// confidence that is not a share, are refused whole. What such an
    /// answer lacks is a spread over the options, so its probabilities carry
    /// the one number it gave.
    ///
    /// # Errors
    ///
    /// [`ChoiceRefusal::UnknownOption`] for an option this question did not
    /// offer; [`ChoiceRefusal::NotOne`] for a confidence outside `[0, 1]`.
    pub fn choice_of(&self, option: &str, confidence: f64) -> Result<ActionChoice, ActionRefusal> {
        let option = option.trim();
        if !self.options().iter().any(|offered| offered == option) {
            return Err(ChoiceRefusal::UnknownOption.into());
        }
        if !(0.0..=1.0).contains(&confidence) {
            return Err(ChoiceRefusal::NotOne.into());
        }
        let chosen = match option {
            GIVE_UP => Chosen::GiveUp,
            DONE => Chosen::Done,
            named => Chosen::Mark(mark_of(named).ok_or(ChoiceRefusal::UnknownOption)?),
        };
        Ok(ActionChoice {
            chosen,
            probabilities: BTreeMap::from([(option.to_string(), confidence)]),
            confidence,
            guard: None,
        })
    }

    /// Every option name it offered, in the order a reader would see them.
    #[must_use]
    pub fn options(&self) -> Vec<String> {
        self.marks
            .iter()
            .map(|mark| option_of(*mark))
            .chain(std::iter::once(GIVE_UP.to_string()))
            .chain(self.ends_itself.then(|| DONE.to_string()))
            .collect()
    }

    /// What the endpoint's `answers` map says about this question — its
    /// choice, judged against the set this question offered, and its two
    /// guards. One broken rule discards the answer whole: a judgment that got
    /// the shape wrong has said nothing about the screen.
    ///
    /// # Errors
    ///
    /// [`ActionRefusal`] names which rule the answer broke.
    pub fn read(&self, answers: &Value) -> Result<ActionChoice, ActionRefusal> {
        let offered: BTreeSet<String> = self.options().into_iter().collect();
        let choice = crate::jev::choice::read(answers, QUESTION, &offered)?;
        let chosen = match choice.chosen.as_str() {
            GIVE_UP => Chosen::GiveUp,
            DONE => Chosen::Done,
            named => Chosen::Mark(mark_of(named).ok_or(ChoiceRefusal::UnknownOption)?),
        };
        let guard = Guard {
            instructed: noul::read(answers, INSTRUCTED)?,
            walled: noul::read(answers, WALLED)?,
        };
        Ok(ActionChoice {
            chosen,
            probabilities: choice.probabilities,
            confidence: choice.confidence,
            guard: Some(guard),
        })
    }
}

#[cfg(test)]
mod tests;
