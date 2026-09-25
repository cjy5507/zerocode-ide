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
//!
//! One request, every head (t-6720, the ultrafast shape): what to do next and
//! what to do it TO are asked together, and only the head the chosen action
//! needs is spent. The `action` head is the operation with the press targets
//! inline — `mark:<n>` IS "press n" — and, when a goal walk's look read a
//! field it may type into ([`Beside`]), it offers [`TYPE_TEXT`] too, with
//! the field asked beside it in `type_target`. When the look read the
//! containers, images and rows a page holds, three observation heads ask
//! which of them the goal is about ([`Observe`]). Every head is a closed
//! choice over what the look saw, answered in the same round trip; none of
//! them writes a selector or a value, and a head the chosen action does not
//! need is read — a broken one refuses the answer whole — and never acted on.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::computer_use_protocol::marks::legend_line;
use crate::jev::choice::{self, Choice, ChoiceRefusal};
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

/// The operation that enters text into a field — offered in the `action`
/// head beside the presses, only to a goal walk whose world can type and
/// whose look read a field it may type into ([`Beside`]). Which field is the
/// `type_target` head's answer; the text itself is written by the value seat
/// (`crate::type_value`), never by this judgment.
pub const TYPE_TEXT: &str = "type_text";

/// What [`TYPE_TEXT`] means.
const TYPE_TEXT_MEANS: &str = "Enter text into one of the fields this screen shows — the one `type_target` names — because the goal needs text there that the field does not hold yet.";

/// The words of the question when a goal walk may also type: the goal's own
/// question, with entering text among the things a person could do next.
const GOAL_TYPING_INSTRUCTIONS: &str = "Someone wants to reach the goal in `goal` on the screen described by `where`, and they can press things or enter text into a field. The controls that screen is showing right now are the options, each named by the number the screen drew on it; `type_text` means entering text into one of the fields, and `type_target` says which. `shows` is the text the screen displays right now that is not a control — a value, a heading, a message — and `pressed` is what this walk already pressed or typed into, oldest first. Read the two together to tell how far along the goal already is, and choose what a person would do NEXT to get closer to it: never a press or an entry whose effect `shows` or a field's own words already carry, and never the same control again unless the goal itself repeats it.";

/// The head that names the field a [`TYPE_TEXT`] enters text into.
const TYPE_TARGET: &str = "type_target";

/// The words the `type_target` head asks.
const TYPE_TARGET_INSTRUCTIONS: &str = "If the next thing to do on this screen is to enter text, which field should receive it? The options are the fields this screen is showing, each named by the number the screen drew on it.";

/// The option an observation head offers beside the candidates the look
/// read: none of them is what the goal is about.
pub const NONE: &str = "none";

/// What a line of `pressed` says of a field this walk typed into: the
/// operation's own word before the field's legend line, so the next question
/// reads an entry apart from a press — and never the text entered.
#[must_use]
pub fn typed_line(legend: &str) -> String {
    format!("{TYPE_TEXT} {legend}")
}

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

/// What a page's look carries beside its numbered `items` (the snapshot
/// contract of t-6721 U4): the document it read them in, and the fields,
/// containers, images and rows it read in the same pass. The marks
/// answer keeps `items` as it always was; every key here is added beside it,
/// and a look that carries none of them walks exactly as before — presses
/// only. Named here, where the question reads them, so the page that writes
/// them and the walk that reads them spell each key once.
pub mod snapshot {
    /// The document the look read — a value that changes when the page's
    /// document is replaced, never when it merely changes.
    pub const EPOCH_KEY: &str = "documentEpoch";
    /// The form fields among the numbered controls.
    pub const FIELDS_KEY: &str = "fields";
    /// A field's number among the look's `items` — the control it is.
    pub const FIELD_MARK_KEY: &str = "mark";
    /// A field's kind: an `<input>`'s own `type`, lower-cased; `textarea`,
    /// `select` or `contenteditable` for the rest.
    pub const FIELD_KIND_KEY: &str = "kind";
    /// Whether a field holds a secret — a password, or one the page declares
    /// a current password. Only an explicit `false` lets a value in.
    pub const FIELD_SECRET_KEY: &str = "secret";
    /// A field's or a container's label — for a field, the first of the
    /// words around it the value seat reads (`crate::type_value`), beside
    /// its placeholder and the page's own words next to it.
    pub const LABEL_KEY: &str = "label";
    pub const FIELD_PLACEHOLDER_KEY: &str = "placeholder";
    pub const FIELD_NEAR_KEY: &str = "near";
    /// An observed candidate's own words: a container's role and how many
    /// rows it holds, an image's alt text and its size, a row's text.
    pub const ROLE_KEY: &str = "role";
    pub const COUNT_KEY: &str = "count";
    pub const ALT_KEY: &str = "alt";
    pub const WIDTH_KEY: &str = "width";
    pub const HEIGHT_KEY: &str = "height";
    pub const TEXT_KEY: &str = "text";
    /// What the field holds right now. Never sent to a judgment or a model:
    /// it tells a retry of one entry from a new one.
    pub const FIELD_VALUE_KEY: &str = "value";
    /// A numbered control's or an observed candidate's own identity in the
    /// document, as the look wrote it — the only name a walk ever types
    /// into or reports, and never one a walk or a judgment composed.
    pub const SELECTOR_KEY: &str = "selector";
    /// A numbered control's element name — with its selector and its role,
    /// what tells one control in a document from another.
    pub const TAG_KEY: &str = "tag";
}

/// The kinds of field ([`snapshot::FIELD_KIND_KEY`]) a walk may type a
/// written value into: the ones whose whole value is one line of text a
/// keyboard types. A password is not among them and a secret field of any
/// kind is refused beside it ([`snapshot::FIELD_SECRET_KEY`]); a `select`, a
/// checkbox, a date or a file is pressed or left to a person, never typed.
pub const TEXT_FIELD_KINDS: [&str; 8] = [
    "text",
    "search",
    "email",
    "tel",
    "url",
    "number",
    "textarea",
    "contenteditable",
];

/// The observation heads (t-4692): which of the containers, images and rows
/// a page's look read the goal is about — the three choices a collection
/// made by a person's coordinator took four round trips and two wrong
/// answers to settle. Each is a closed choice over what the look read, by
/// the look's own number, plus [`NONE`]; the answer names a candidate the
/// look already holds and never composes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observe {
    /// The container that holds the results the goal is about.
    Container,
    /// The picture of the item the goal is about.
    Image,
    /// The row that matches the goal.
    Row,
}

impl Observe {
    /// Every head, in the order a request asks them.
    pub const ALL: [Self; 3] = [Self::Container, Self::Image, Self::Row];

    /// The head's name on the wire, and the stem of its options
    /// (`container:2`).
    #[must_use]
    pub const fn head(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Image => "image",
            Self::Row => "row",
        }
    }

    /// The key the look carries this head's candidates under.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Container => "containers",
            Self::Image => "images",
            Self::Row => "rows",
        }
    }

    /// What the head asks.
    const fn instructions(self) -> &'static str {
        match self {
            Self::Container => CONTAINER_INSTRUCTIONS,
            Self::Image => IMAGE_INSTRUCTIONS,
            Self::Row => ROW_INSTRUCTIONS,
        }
    }

    /// What [`NONE`] means to this head.
    const fn none_means(self) -> &'static str {
        match self {
            Self::Container => CONTAINER_NONE,
            Self::Image => IMAGE_NONE,
            Self::Row => ROW_NONE,
        }
    }

    /// The option naming the look's `n`th candidate of this head.
    #[must_use]
    pub fn option(self, n: usize) -> String {
        format!("{}:{n}", self.head())
    }

    /// The number an option of this head names, if it names one.
    #[must_use]
    pub fn number_of(self, option: &str) -> Option<usize> {
        option
            .strip_prefix(self.head())?
            .strip_prefix(':')?
            .parse()
            .ok()
    }

    /// How the look's `number`th candidate of this head is described to the
    /// judgment — its own words, never its identity — or `None` for one the
    /// look gave nothing to describe it by. Cut to [`OBSERVED_CHAR_CAP`].
    fn line(self, number: usize, candidate: &Value) -> Option<String> {
        let text = |key: &str| {
            candidate
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|words| !words.is_empty())
        };
        let size = |key: &str| {
            candidate
                .get(key)
                .and_then(Value::as_f64)
                .filter(|pixels| pixels.is_finite() && *pixels > 0.0)
        };
        let line = match self {
            Self::Container => {
                let words: Vec<&str> = [text(snapshot::ROLE_KEY), text(snapshot::LABEL_KEY)]
                    .into_iter()
                    .flatten()
                    .collect();
                if words.is_empty() {
                    return None;
                }
                let held = candidate
                    .get(snapshot::COUNT_KEY)
                    .and_then(Value::as_u64)
                    .map(|count| format!(" ({count} {OBSERVED_COUNT_WORD})"))
                    .unwrap_or_default();
                format!("{number} {}{held}", words.join(" "))
            }
            Self::Image => {
                let alt = text(snapshot::ALT_KEY);
                let dimensions = size(snapshot::WIDTH_KEY)
                    .zip(size(snapshot::HEIGHT_KEY))
                    .map(|(width, height)| format!("{width:.0}x{height:.0}"));
                if alt.is_none() && dimensions.is_none() {
                    return None;
                }
                let words: Vec<String> = [
                    Some(OBSERVED_IMAGE_WORD.to_string()),
                    alt.map(str::to_string),
                    dimensions,
                ]
                .into_iter()
                .flatten()
                .collect();
                format!("{number} {}", words.join(" "))
            }
            Self::Row => format!("{number} {}", text(snapshot::TEXT_KEY)?),
        };
        Some(line.chars().take(OBSERVED_CHAR_CAP).collect())
    }
}

/// The words an observed candidate's line is written with beside its own:
/// what an image is called, and what a container's count counts.
const OBSERVED_IMAGE_WORD: &str = "image";
const OBSERVED_COUNT_WORD: &str = "items";

/// What the container head asks.
const CONTAINER_INSTRUCTIONS: &str = "Which of the containers this screen holds has the results the goal in `goal` is about? The options are the containers the screen was read to hold, each named by its number.";
/// What [`NONE`] means to the container head.
const CONTAINER_NONE: &str = "None of these containers holds what the goal is about.";
/// What the image head asks.
const IMAGE_INSTRUCTIONS: &str = "Which of the images this screen shows is the picture of the item the goal in `goal` is about? The options are the images the screen was read to show, each named by its number.";
/// What [`NONE`] means to the image head.
const IMAGE_NONE: &str = "None of these images is the picture the goal is about.";
/// What the row head asks.
const ROW_INSTRUCTIONS: &str = "Which of the rows this screen lists matches the goal in `goal`? The options are the rows the screen was read to list, each named by its number.";
/// What [`NONE`] means to the row head.
const ROW_NONE: &str = "None of these rows matches the goal.";

/// How many characters one observed candidate's description may take: the
/// screen's own text cap shared among the most candidates one head offers,
/// so a head's options never cost more than the screen's words do
/// ([`SHOWS_CHAR_CAP`], [`MAX_ACTION_CANDIDATES`]).
pub const OBSERVED_CHAR_CAP: usize = SHOWS_CHAR_CAP / MAX_ACTION_CANDIDATES;

/// What a look read beside its numbered controls, and whether the walk can
/// type at all — what [`ask_with`] may offer beyond a press. The default is
/// a look of numbers alone, and it asks exactly what [`ask`] asks.
#[derive(Debug, Clone, Copy, Default)]
pub struct Beside<'a> {
    /// Whether the walk's world has a road for a written value (a page's).
    pub types: bool,
    /// The look's fields ([`snapshot::FIELDS_KEY`]).
    pub fields: &'a [Value],
    /// The look's containers, images and rows ([`Observe::key`]).
    pub containers: &'a [Value],
    pub images: &'a [Value],
    pub rows: &'a [Value],
}

impl<'a> Beside<'a> {
    /// The candidates the look read for `head`.
    #[must_use]
    pub const fn of(&self, head: Observe) -> &'a [Value] {
        match head {
            Observe::Container => self.containers,
            Observe::Image => self.images,
            Observe::Row => self.rows,
        }
    }
}

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
pub const SCREEN_ACTION_RUBRIC_VERSION: u32 = 6;

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
    /// The fields `type_target` offered, in order — empty when the question
    /// offered no [`TYPE_TEXT`].
    typing: Vec<usize>,
    /// Each observation head asked, with the look's numbers it offered.
    observing: Vec<(Observe, Vec<usize>)>,
}

/// What an answer chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chosen {
    /// Press this number.
    Mark(usize),
    /// Enter a written value into the field this number names — the
    /// `type_target` head's answer to a [`TYPE_TEXT`].
    Type(usize),
    /// Nothing here helps.
    GiveUp,
    /// The goal is already reached; press nothing.
    Done,
}

/// What one observation head chose: the look's own number of the candidate,
/// or `None` for [`NONE`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observed {
    pub head: Observe,
    pub chosen: Option<usize>,
    pub confidence: f64,
}

/// A validated answer to every head a question asked: the action (with the
/// field folded into [`Chosen::Type`] when it is to type), what the
/// `type_target` head said when it was asked — its spread, for the row — and
/// what each observation head chose.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionRead {
    pub choice: ActionChoice,
    pub typed: Option<Choice>,
    pub observed: Vec<Observed>,
}

impl From<ActionChoice> for ActionRead {
    /// An answer to the action head alone — a second reader's, or a question
    /// that asked nothing beside it.
    fn from(choice: ActionChoice) -> Self {
        Self {
            choice,
            typed: None,
            observed: Vec::new(),
        }
    }
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
    // The typing and observation heads (v6, t-6720): the operation, its
    // field's head, the kinds of field it may enter text into, and each
    // observation head's words.
    for line in [
        TYPE_TEXT,
        TYPE_TEXT_MEANS,
        GOAL_TYPING_INSTRUCTIONS,
        TYPE_TARGET,
        TYPE_TARGET_INSTRUCTIONS,
        NONE,
    ] {
        words.push('\n');
        words.push_str(line);
    }
    words.push('\n');
    words.push_str(&TEXT_FIELD_KINDS.join(","));
    for line in [OBSERVED_IMAGE_WORD, OBSERVED_COUNT_WORD] {
        words.push('\n');
        words.push_str(line);
    }
    for head in Observe::ALL {
        for line in [
            head.head(),
            head.key(),
            head.instructions(),
            head.none_means(),
        ] {
            words.push('\n');
            words.push_str(line);
        }
    }
    words
}

/// The question this look asks, or `None` when there is nothing left to
/// choose between — no control the look saw that this walk has not already
/// spent on this screen. A caller that gets `None` stops exactly as it would
/// have without a judgment at all.
#[must_use]
pub fn ask(look: &ActionLook<'_>) -> Option<ActionAsk> {
    ask_with(look, &Beside::default())
}

/// [`ask`], with what the look read beside its numbered controls: a goal
/// walk that may type is offered [`TYPE_TEXT`] with the fields it may type
/// into (`type_target`), and the containers, images and rows the look read
/// are asked about in the observation heads — all in the one request. A look
/// that read nothing beside its numbers, a walk that cannot type and a
/// stopped walk ask exactly what [`ask`] always asked, to the byte.
#[must_use]
pub fn ask_with(look: &ActionLook<'_>, beside: &Beside<'_>) -> Option<ActionAsk> {
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
    // What a goal walk may type into: the offered numbers the look itself
    // read as plain text fields — never a secret, never a field it did not
    // read, never one this walk already spent here.
    let goal = matches!(look.errand, Errand::Goal);
    let typing: Vec<usize> = if goal && beside.types {
        marks
            .iter()
            .copied()
            .filter(|mark| typeable(beside.fields, *mark))
            .collect()
    } else {
        Vec::new()
    };
    if !typing.is_empty() {
        criteria.insert(
            TYPE_TEXT.to_string(),
            Value::String(TYPE_TEXT_MEANS.to_string()),
        );
    }
    criteria.insert(
        GIVE_UP.to_string(),
        Value::String(look.errand.give_up_means().to_string()),
    );
    let ends_itself = look.errand.ends_itself();
    if ends_itself {
        criteria.insert(DONE.to_string(), Value::String(DONE_MEANS.to_string()));
    }
    // The field's head offers the very lines the action head describes those
    // fields by, so the two heads cannot come to name one field two ways.
    let fields: Map<String, Value> = typing
        .iter()
        .filter_map(|mark| {
            let option = option_of(*mark);
            let line = criteria.get(&option)?.clone();
            Some((option, line))
        })
        .collect();

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
    let instructions = if typing.is_empty() {
        look.errand.instructions()
    } else {
        GOAL_TYPING_INSTRUCTIONS
    };
    let mut questions = choice::asked(QUESTION, instructions, criteria);
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
    // So do the heads beside the action (t-6720): what an entry would type
    // into, and what the page's containers, images and rows are to the goal.
    if !fields.is_empty() {
        also_ask(
            &mut questions,
            TYPE_TARGET,
            TYPE_TARGET_INSTRUCTIONS,
            fields,
        );
    }
    // An observation head's candidates stand in the state as well as among
    // its options, as the numbered controls do: a judgment reads the screen
    // from the state, and asked with the lines among the options alone it
    // answered `none` for 47 of 48 heads on a page whose right answers stood
    // plain (2026-09-26, the probe page; with the lines in the state, 18 of
    // 18). Each line is cleared of anything that may carry a credential by
    // the door's own rule before it goes anywhere.
    let mut observing = Vec::new();
    if goal {
        for head in Observe::ALL {
            let mut offered = Vec::new();
            let mut described = Map::new();
            let mut lines = Vec::new();
            for (index, candidate) in beside.of(head).iter().enumerate() {
                if offered.len() == MAX_ACTION_CANDIDATES {
                    break;
                }
                let number = index + 1;
                let Some(line) = head.line(number, candidate) else {
                    continue;
                };
                let (line, _) =
                    crate::jev::door::clear_text(&line, crate::jev::Cap::Chars(OBSERVED_CHAR_CAP));
                described.insert(head.option(number), Value::String(line.clone()));
                lines.push(Value::String(line));
                offered.push(number);
            }
            if offered.is_empty() {
                continue;
            }
            described.insert(
                NONE.to_string(),
                Value::String(head.none_means().to_string()),
            );
            state.insert(head.key().to_string(), Value::Array(lines));
            also_ask(&mut questions, head.head(), head.instructions(), described);
            observing.push((head, offered));
        }
    }
    Some(ActionAsk {
        state: Value::Object(state),
        questions,
        marks,
        ends_itself,
        typing,
        observing,
    })
}

/// One more closed choice asked in the same request, beside the heads
/// `questions` already holds.
fn also_ask(questions: &mut Value, name: &str, instructions: &str, criteria: Map<String, Value>) {
    if let (Some(asked), Value::Object(head)) = (
        questions.as_object_mut(),
        choice::asked(name, instructions, criteria),
    ) {
        asked.extend(head);
    }
}

/// Whether the look read `mark` as a field a written value may go into:
/// every entry naming it says a plain text kind ([`TEXT_FIELD_KINDS`]) and
/// an explicit `false` for a secret, and at least one does. A field the look
/// did not read is not one — a textbox's role alone vouches for nothing.
fn typeable(fields: &[Value], mark: usize) -> bool {
    let mut named = fields.iter().filter(|field| {
        field.get(snapshot::FIELD_MARK_KEY).and_then(Value::as_u64) == u64::try_from(mark).ok()
    });
    let plain = |field: &Value| {
        field
            .get(snapshot::FIELD_SECRET_KEY)
            .and_then(Value::as_bool)
            == Some(false)
            && field
                .get(snapshot::FIELD_KIND_KEY)
                .and_then(Value::as_str)
                .is_some_and(|kind| {
                    TEXT_FIELD_KINDS
                        .iter()
                        .any(|allowed| allowed.eq_ignore_ascii_case(kind.trim()))
                })
    };
    named.next().is_some_and(&plain) && named.all(plain)
}

impl ActionAsk {
    /// The numbers this question offered, in order.
    #[must_use]
    pub fn marks(&self) -> &[usize] {
        &self.marks
    }

    /// The fields `type_target` offered, in order — empty when nothing may
    /// be typed.
    #[must_use]
    pub fn typing(&self) -> &[usize] {
        &self.typing
    }

    /// The press options a second reader chooses among: every offered
    /// number, [`GIVE_UP`] and — for a goal — [`DONE`]. Never [`TYPE_TEXT`]:
    /// a reader answering one option cannot name the field it would type
    /// into, and a rescue presses or steps back.
    #[must_use]
    pub fn press_options(&self) -> Vec<String> {
        self.marks
            .iter()
            .map(|mark| option_of(*mark))
            .chain(std::iter::once(GIVE_UP.to_string()))
            .chain(self.ends_itself.then(|| DONE.to_string()))
            .collect()
    }

    /// Every head's answer, each judged against the set its head offered —
    /// the action, the field when it is to type, the observation heads and
    /// the two guards. One broken rule in any head discards the answer
    /// whole, whether or not the action needed that head.
    ///
    /// # Errors
    ///
    /// [`ActionRefusal`] names which rule the answer broke.
    pub fn read_all(&self, answers: &Value) -> Result<ActionRead, ActionRefusal> {
        let offered: BTreeSet<String> = self.options().into_iter().collect();
        let action = choice::read(answers, QUESTION, &offered)?;
        let typed = if self.typing.is_empty() {
            None
        } else {
            let fields: BTreeSet<String> =
                self.typing.iter().map(|mark| option_of(*mark)).collect();
            Some(choice::read(answers, TYPE_TARGET, &fields)?)
        };
        let mut observed = Vec::with_capacity(self.observing.len());
        for (head, numbers) in &self.observing {
            let offered: BTreeSet<String> = numbers
                .iter()
                .map(|number| head.option(*number))
                .chain(std::iter::once(NONE.to_string()))
                .collect();
            let read = choice::read(answers, head.head(), &offered)?;
            observed.push(Observed {
                head: *head,
                chosen: head.number_of(&read.chosen),
                confidence: read.confidence,
            });
        }
        let guard = Guard {
            instructed: noul::read(answers, INSTRUCTED)?,
            walled: noul::read(answers, WALLED)?,
        };
        // An entry is as sure as the less sure of its two heads: what to do,
        // and which field to do it to.
        let (chosen, confidence) = match action.chosen.as_str() {
            GIVE_UP => (Chosen::GiveUp, action.confidence),
            DONE => (Chosen::Done, action.confidence),
            TYPE_TEXT => {
                let field = typed.as_ref().ok_or(ChoiceRefusal::NoAnswer)?;
                (
                    Chosen::Type(mark_of(&field.chosen).ok_or(ChoiceRefusal::UnknownOption)?),
                    action.confidence.min(field.confidence),
                )
            }
            named => (
                Chosen::Mark(mark_of(named).ok_or(ChoiceRefusal::UnknownOption)?),
                action.confidence,
            ),
        };
        Ok(ActionRead {
            choice: ActionChoice {
                chosen,
                probabilities: action.probabilities,
                confidence,
                guard: Some(guard),
            },
            typed,
            observed,
        })
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
        if !self.press_options().iter().any(|offered| offered == option) {
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

    /// Every option name its action head offered, in the order a reader
    /// would see them: the numbers, [`TYPE_TEXT`] when a field may be typed
    /// into, [`GIVE_UP`] and — for a goal — [`DONE`].
    #[must_use]
    pub fn options(&self) -> Vec<String> {
        self.marks
            .iter()
            .map(|mark| option_of(*mark))
            .chain((!self.typing.is_empty()).then(|| TYPE_TEXT.to_string()))
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
        self.read_all(answers).map(|read| read.choice)
    }
}

#[cfg(test)]
mod tests;
