//! Walking a screen by judgment (docs/design/jev-browser-action-20260917.md
//! §2.3): the screen in front of a walk already numbers every control a person
//! could press, so one of those numbers — chosen by a judgment, never invented
//! by one — is pressed, and what follows is looked at again.
//!
//! It was written as `recover`, for one errand only: a recorded flow that
//! stopped because a step or a check failed. It is named `errand` now because
//! a second errand asks the same question of the same screen, and a module
//! named after the first of two would have to lie about the other:
//!
//! - [`Why::Cleared`] — a recorded walk stopped. One press should let the
//!   stopped step run again, and the document's own re-walk says whether it
//!   did. Two presses at most: a third guess on a screen that did not move
//!   twice is a loop, not a recovery.
//! - [`Why::Goal`] — nothing failed. A goal was named in a person's own words
//!   at a point where the procedure BRANCHES — where what to press next cannot
//!   be written down in advance because it depends on what the screen came
//!   back with. The deterministic run before and after it is the caller's
//!   (a batch, a recipe); this is the one step of it that needed reading.
//!
//! The shape is the repeat's ([`super::repeat`]): this module decides WHETHER
//! and WITH WHAT, and the world it acts on is one narrow seam ([`World`]) a
//! test replaces whole. It walks no step itself — pressing goes down the same
//! door, through the same pin and the same fingerprint, as every other press —
//! and it invents no resume point: for a stopped walk, the step to walk again
//! from is the one the report already computed (`RecipeStop::resumes_after`).
//!
//! Four things are load-bearing and each has a test of its own:
//!
//! 1. **A stop is cleared only where clearing means what it says.**
//!    `StepFailed` and `CheckFailed` resume AT the step that stopped. Every
//!    other stop either belongs to a person, was set by one, or resumes after
//!    the stopped step — and walking on from there after an unrelated press
//!    would skip work the recipe asked for. A goal is not a stop and is not
//!    read through that gate at all.
//! 2. **Money is never walked by judgment.** A document that moves money is
//!    barred whatever its policy, a `guarded` Flow is barred whatever its
//!    steps, and a goal walk — which has no document to read a money step out
//!    of — is barred from the money door by the same gate, since it reaches
//!    the door through the same [`barred`].
//! 3. **Pressing is not succeeding.** The row says what was chosen and, apart
//!    from it, whether what followed got past what was in the way.
//! 4. **A screen that does not move is where a walk stops.** Every errand
//!    keeps it: the numbers already spent are the numbers spent ON THIS
//!    SCREEN, they are offered again the moment the screen moves, and two
//!    presses that change nothing end the walk. That, not the step budget, is
//!    what makes an unattended walk terminate.

use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::branching::{BranchAsk, NextStep};
use zerocode_core::computer_flow::{FlowSpec, Policy};
use zerocode_core::computer_recipe::{RecipeLine, RecipeStop, RecipeTool};
use zerocode_core::guarded::{ControlKind, kind_of};
use zerocode_core::jev::promote::SEAT_RECORDING;
use zerocode_core::jev::summary::{AGREED, AT, CACHED, ELAPSED_MS, MODEL};
use zerocode_core::jev::{BROWSER, DESKTOP, EMULATOR, JevMode, JevUse, SCREEN_APPLY_DEADLINE_MS};
use zerocode_core::screen_action::{
    ActionAsk, ActionChoice, ActionLook, ActionRead, Beside, Chosen, Guard, Observe,
    SCREEN_ACTION_RUBRIC_VERSION, Stopped, TYPE_TEXT, Where, ask_with, option_of, snapshot,
    typed_line,
};

pub use branch::{Branching, Compared, Saved};

/// How long one judgment may hold a walk: the screen seats' own wall, read
/// from the use table that judges a rising seat against it
/// ([`SCREEN_APPLY_DEADLINE_MS`]) rather than spelled again here. A judgment
/// that has not answered by then is slower than the fallback it would
/// replace, and the stage that waits and the judge that reads the wait have
/// to be reading one number.
pub const ACTION_DEADLINE: Duration = Duration::from_millis(SCREEN_APPLY_DEADLINE_MS);

/// How many numbers one walk may spend clearing one stop. Two, because a
/// third guess on a screen that did not move twice is a loop, not a recovery.
pub const MAX_RECOVERY_ATTEMPTS: usize = 2;

/// How many presses in a row may leave the screen exactly as it was before a
/// walk gives up on it. Two — [`MAX_RECOVERY_ATTEMPTS`]' own reason, kept for
/// every errand: a second guess is worth making, a third on a screen that has
/// not moved twice is a loop. How many presses a goal walk may spend in all
/// is the verb's, measured and clamped there
/// (`zerocode_core::computer_use::WALK_STEPS_DEFAULT`); this is the rule that
/// ends a walk before that number is anywhere near reached.
pub const SAME_SCREEN_LIMIT: usize = MAX_RECOVERY_ATTEMPTS;

/// The seat a surface's judgments sit in, and whose ledger they are written
/// to — the Jev use table's rows, never a name spelled here.
#[must_use]
pub const fn seat_of(surface: Surface) -> &'static JevUse {
    match surface {
        Surface::Page => &BROWSER,
        Surface::Desk => &DESKTOP,
        Surface::Phone => &EMULATOR,
    }
}

/// Which screen a walk is looking at. The two have separate seats on the
/// settings card because what they send is different in kind, not because the
/// question differs — it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A browser pane.
    Page,
    /// The desktop's accessibility tree.
    Desk,
    /// A mobile accessibility tree, with independent consent.
    Phone,
}

/// What the person set for this surface's seat. The ladder is the routing
/// card's, because it is the same question: does anything go to the vendor,
/// and may it change what the product does. `off` asks nothing and is what an
/// unknown word reads as; `shadow` and `auto` ask and record while the walk
/// goes exactly as it would have — nothing promotes a press — and `on`
/// presses what it chose.
pub type Mode = JevMode;

/// What a judgment answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Judged {
    /// A validated answer to every head that was asked: the action over the
    /// set it offered, the field when the action is to type, and what the
    /// observation heads chose.
    Chose(ActionRead),
    /// Nothing usable, named by the ledger's own failure token
    /// (`no_key`, `timeout`, `schema`, `unauthorized`, `http_503`, …).
    Refused(String),
}

/// What asking a judgment cost at the Jev door — the wire's own account.
pub use crate::systemone::Spent;

/// How a walk is asked to go beyond today's loop — switches the caller sets,
/// every one off by default, so a walk asked plainly is today's walk to the
/// byte ([`run`] is [`run_with`] under these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// Overlap the next judgment with the press before it (t-6132 S2): the
    /// moment a goal walk's press is taken, the next question is asked of
    /// the screen as it was — the last look, with the pressed number spent
    /// and its legend among `pressed` — on a thread of its own, while the
    /// press lands and the next look is taken. When that look asks the same
    /// question to the byte, the answer in flight is used and the walk
    /// waited only for what the judgment had left; when it does not, the
    /// answer is dropped, its request spent, and the screen is asked afresh.
    /// A press on a link, a screen one more stand from stuck, and a walk
    /// that clears a stop never ask ahead: those walks do not come back to
    /// the same screen.
    ///
    /// A phone asks after its press instead (t-6385): the screen it left is
    /// the one a question begun before the press is about — 10 of 10 such
    /// judgments were dropped on a Settings walk (t-6350) — so a world whose
    /// press waits for its screen to stop changing hands back what it stopped
    /// on ([`World::settled`]), the next question is begun there, and the
    /// full look is taken while it is answered.
    ///
    /// A page asks after its press too, and before its settle (t-9712): its
    /// press answers the moment it is made with the page as the press changed
    /// it ([`World::unsettled`]), the next question is begun there, and the
    /// settle — the fifty quiet milliseconds a press by number waits — is
    /// waited for behind it ([`World::settle`]). A page that did not settle
    /// cancels what was begun, and the walk looks again. A page the press
    /// left as it was pressed begins nothing — its change may come after the
    /// press answered (a result a fetch renders), and a question begun on it
    /// is one the settled page no longer asks — so the look after its settle
    /// asks in turn, one request a step as ever. The browser door settles such
    /// a press before it answers, as a press without the flag does, and
    /// answers the settled page with how it settled (t-9876): that page is the
    /// next step's look, and no settle is left to wait for.
    pub overlap: bool,
    /// The second rung (t-6132 S3): when the seat's judgment is under its
    /// press floor, ask the second reader the walk was handed
    /// ([`run_with`]'s `rescue`) the same closed choice, and press its
    /// number under the seat's own press rule; only when that too is under
    /// the floor, refused, a link, `give_up` or `done` does the walk step
    /// back to the person as it does today. Off, the walk never asks: the
    /// second reader costs a frontier turn.
    pub rescue: bool,
    /// The act line the screen seat's graded answers drew, kept beside its
    /// ledger (t-9468, `crate::systemone::act_line`): a plain press asks it
    /// in place of the seat's press floor, and a press that cannot be taken
    /// back still asks nine in ten. `None` — the default, and every seat
    /// whose labels drew no line — presses from the floor as ever.
    pub act_line: Option<u16>,
}

/// What a judgment begun ahead of the walk came to ([`Pending::wait`]): the
/// judge's answer, and what the judge would have said of itself had it been
/// asked in the walk's own turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub judged: Judged,
    pub spent: Option<Spent>,
    pub cached: bool,
    /// Rows of the judge's own seats (the judgment cache's, t-6132 S1) the
    /// question wrote on the way, handed back to the judge that will write
    /// them.
    pub rows: Vec<Value>,
}

/// A judgment in flight — begun before the walk needed it, waited for only
/// once the walk asks the very question it was begun on.
#[derive(Debug)]
pub struct Pending {
    ask: ActionAsk,
    done: std::sync::mpsc::Receiver<Done>,
    began: std::time::Instant,
}

impl Pending {
    /// A judgment of `ask` whose answer will arrive on `done`.
    #[must_use]
    pub fn new(ask: ActionAsk, done: std::sync::mpsc::Receiver<Done>) -> Self {
        Self {
            ask,
            done,
            began: std::time::Instant::now(),
        }
    }

    /// Whether this is the very question the walk is asking now — the same
    /// state and the same options, to the byte; anything else is another
    /// screen and the answer in flight says nothing about it.
    #[must_use]
    pub fn matches(&self, asked: &ActionAsk) -> bool {
        self.ask == *asked
    }

    /// Milliseconds the judgment has been running ahead of the walk.
    #[must_use]
    pub fn ran_ms(&self) -> u64 {
        u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// The answer, waited for. A judge whose thread died is a wire that
    /// never answered, in the wire's own word.
    #[must_use]
    pub fn wait(self) -> Done {
        self.done.recv().unwrap_or_else(|_| Done {
            judged: Judged::Refused(crate::systemone::TRANSPORT.to_string()),
            spent: None,
            cached: false,
            rows: Vec::new(),
        })
    }
}

/// Choosing one of the numbers a look handed out — the seam the window's
/// System One wire sits behind and a test replaces.
pub trait ActionJudge {
    fn choose(&mut self, ask: &ActionAsk) -> Judged;

    /// Which of a forked step's results is the closest to the goal (t-6044,
    /// `zerocode_core::jev::BRANCHING`) — asked under that seat's own row,
    /// down the same wire. A judge with no comparison to give — a test's that
    /// was handed none — refuses, and the first candidate stands as today.
    fn compare(&mut self, ask: &BranchAsk) -> Compared {
        let _ = ask;
        Compared::Refused(branch::NO_COMPARISON.to_string())
    }

    /// Begin choosing on a thread of the judge's own, for a walk that will
    /// ask this very question next ([`Options::overlap`]). `None` from a
    /// judge that cannot ask ahead — the default — and then the walk asks in
    /// its own turn as ever.
    fn begin(&mut self, ask: &ActionAsk) -> Option<Pending> {
        let _ = ask;
        None
    }

    /// Take back what a judgment begun ahead came to, so [`Self::spent`] and
    /// [`Self::cached`] say of it what they would have said of a question
    /// asked in turn.
    fn finish(&mut self, done: Done) -> Judged {
        done.judged
    }

    /// [`Self::choose`], told how long the walk can wait — what a second
    /// reader whose answer takes seconds rather than a wire's milliseconds
    /// needs ([`Options::rescue`]). A judge with a wall of its own ignores
    /// it; the default asks as ever.
    fn choose_within(&mut self, ask: &ActionAsk, left: Duration) -> Judged {
        let _ = left;
        self.choose(ask)
    }

    /// What the last [`Self::choose`] or [`Self::compare`] sent through the
    /// Jev door, for the row. A judge that sends nowhere — a test's — has
    /// nothing to say.
    fn spent(&self) -> Option<Spent> {
        None
    }

    /// Whether the last [`Self::choose`] was answered by the judgment memo
    /// rather than the wire ([`zerocode_core::jev::memo`], t-6132) — the
    /// row's [`CACHED`], so a counter reads it as an answer and not a call.
    fn cached(&self) -> bool {
        false
    }
}

/// The address of the screen a walk is looking at, owned. The borrowed twin
/// the question reads is [`Where`]; this is what a world hands back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    Page {
        host: String,
        path: String,
    },
    Desk {
        app: String,
        window: String,
    },
    Phone {
        platform: zerocode_core::computer_use::EmulatorPlatform,
        device: String,
    },
}

impl Seen {
    const fn surface(&self) -> Surface {
        match self {
            Self::Page { .. } => Surface::Page,
            Self::Desk { .. } => Surface::Desk,
            Self::Phone { .. } => Surface::Phone,
        }
    }

    fn asked(&self) -> Where<'_> {
        match self {
            Self::Page { host, path } => Where::Page { host, path },
            Self::Desk { app, window } => Where::Desk { app, window },
            Self::Phone { platform, device } => Where::Phone {
                platform: platform.as_str(),
                device,
            },
        }
    }
}

impl Default for Seen {
    fn default() -> Self {
        Self::Page {
            host: String::new(),
            path: String::new(),
        }
    }
}

/// The screen a walk is looking at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Screen {
    pub at: Seen,
    pub items: Vec<Value>,
    /// The text the surface read off the screen that is not a control — a
    /// display value, a heading, a message — top to bottom, uncut: the
    /// question cuts it ([`zerocode_core::screen_action::shows_cut`]). Empty
    /// for a surface that reads none.
    pub shows: Vec<String>,
    /// What the look read beside its numbered controls, when its surface
    /// reads more than numbers (a page's snapshot, t-6721 U4). Empty for a
    /// look of numbers alone — and such a look walks exactly as before.
    pub snapshot: Snapshot,
}

/// What a page's look read beside its numbered controls, in the same pass
/// ([`zerocode_core::screen_action::snapshot`]): the document it read them
/// in, its form fields, and the containers, images and rows it found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    /// The document's epoch, as the look spelled it; empty when it named
    /// none.
    pub epoch: String,
    pub fields: Vec<Value>,
    pub containers: Vec<Value>,
    pub images: Vec<Value>,
    pub rows: Vec<Value>,
}

impl Snapshot {
    /// What a marks answer carries beside its `items`, read by the keys the
    /// question names. A key the answer does not carry is empty.
    #[must_use]
    pub fn of(said: &Value) -> Self {
        let list = |key: &str| {
            said.get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        // An epoch is whatever the page counts documents with — a word or a
        // number — and nothing at all is no epoch.
        let epoch = match said.get(snapshot::EPOCH_KEY) {
            Some(Value::String(word)) => word.trim().to_string(),
            Some(Value::Number(number)) => number.to_string(),
            _ => String::new(),
        };
        Self {
            epoch,
            fields: list(snapshot::FIELDS_KEY),
            containers: list(Observe::Container.key()),
            images: list(Observe::Image.key()),
            rows: list(Observe::Row.key()),
        }
    }

    /// The candidates the look read for `head`.
    #[must_use]
    pub fn of_head(&self, head: Observe) -> &[Value] {
        match head {
            Observe::Container => &self.containers,
            Observe::Image => &self.images,
            Observe::Row => &self.rows,
        }
    }
}

impl Screen {
    /// What the question may offer beyond a press on this screen: `fields`
    /// when the walk `types` ([`Self::fields_left`]), and the candidates the
    /// look observed.
    #[must_use]
    pub fn beside<'a>(&'a self, types: bool, fields: &'a [Value]) -> Beside<'a> {
        Beside {
            types,
            fields,
            containers: &self.snapshot.containers,
            images: &self.snapshot.images,
            rows: &self.snapshot.rows,
        }
    }

    /// The look's fields, less those this walk already entered a value into
    /// — named by the look's own selector for them, since the entry itself
    /// changes the words a field is read by. A field takes one entry in a
    /// walk: the next question is about what comes after it.
    #[must_use]
    pub fn fields_left(&self, entered: &[String]) -> Vec<Value> {
        self.snapshot
            .fields
            .iter()
            .filter(|field| {
                field
                    .get("mark")
                    .and_then(Value::as_u64)
                    .and_then(|mark| usize::try_from(mark).ok())
                    .and_then(|mark| selector_of(self, mark))
                    .is_none_or(|selector| !entered.contains(&selector))
            })
            .cloned()
            .collect()
    }

    /// Whether two looks show the same screen — the same words, in the same
    /// order, at the same address.
    ///
    /// The comparison is the question's own view of the screen (the legend
    /// line each control is described by, [`same_legend`]), not the raw
    /// items: a caret that blinked or a pixel that moved is not a screen that
    /// moved, and reading the raw items would make every look different and
    /// quietly disable the rule that ends an unattended walk. The browser
    /// door holds a press to the same comparison (t-9876).
    ///
    /// [`same_legend`]: zerocode_core::computer_use_protocol::marks::same_legend
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        self.at == other.at
            && zerocode_core::computer_use_protocol::marks::same_legend(&self.items, &other.items)
    }
}

/// What a press's screen did before the press answered, for a world whose
/// press waits for its screen to stop changing (a phone's, t-6385).
#[derive(Debug, Clone, PartialEq)]
pub struct Settled {
    /// What the walk's row says of it under [`SETTLE`]: how long it took, in
    /// how many reads, and how it ended.
    pub note: Value,
    /// The screen it stopped on, numbered as a look of it would be — when
    /// the world was asked for it: what a walk that asks ahead begins its
    /// next judgment on while the full look is taken ([`Options::overlap`]).
    pub screen: Option<Screen>,
}

/// What a walk does to the world, and nothing else — each a seam.
pub trait World {
    /// The screen and the controls it is showing now, or `None` when the
    /// screen cannot be read (a walk that cannot see does not press).
    fn look(&mut self) -> Option<Screen>;
    /// Press one number down the ordinary door. `true` when the door took it;
    /// a stale pin, a host outside the recording and a shut door are all
    /// `false`, and none of them is retried.
    fn press(&mut self, mark: usize) -> bool;
    /// Milliseconds the call has left.
    fn left_ms(&mut self) -> u64;
    /// Walk the document again from `step`, answering the report. Only a
    /// [`Why::Cleared`] errand asks this, and only a world that HAS a
    /// document can answer it; a goal walk's world has none and keeps the
    /// answer this default gives.
    fn walk_from(&mut self, step: usize) -> Option<Value> {
        let _ = step;
        None
    }
    /// What the last press's screen did while the press waited for it to stop
    /// changing (t-6385) — a phone's, or a page's press that left the legend
    /// it was made on and so settled before it answered (t-9876). `None` for a
    /// world whose press does not wait — the desktop's, an Android device's,
    /// a page's that changed its legend — and then nothing is noted here.
    fn settled(&mut self) -> Option<Settled> {
        None
    }
    /// The screen as the last press left it, the moment it was made — for a
    /// world whose press answers before its screen settles (a page's, pressed
    /// with `--settle-later`, t-9712, when the press changed its legend): what
    /// a walk that asks ahead begins its next judgment on while the settle is
    /// waited for ([`Self::settle`]). `None` for a world whose press waited,
    /// or read nothing.
    fn unsettled(&mut self) -> Option<Screen> {
        None
    }
    /// Wait for the last press's screen to settle, for a world whose press
    /// answered before it did (t-9712): how it ended, in the door's own
    /// words, and the screen it settled on. `None` when no settle waits — a
    /// world whose press waited for its own, or a press that was not taken.
    fn settle(&mut self) -> Option<Settled> {
        None
    }
    /// Whether a judgment begun before a press can stand for the question
    /// the next look asks ([`Options::overlap`]). A window's can when its
    /// press leaves the screen where it was; a phone's press moves its screen
    /// too often for that, and it asks on the screen its press settled on
    /// instead ([`Self::settled`]); a page that answers before it settles
    /// asks on the page its press changed ([`Self::unsettled`]).
    fn asks_ahead_of_the_press(&self) -> bool {
        true
    }
    /// Whether the caller's own success condition is met — the deterministic
    /// one written down before the walk started, asked of the screen and
    /// never of a judgment. A world whose caller named no condition answers
    /// `None`, and then a goal walk has only the judgment's own `done` to end
    /// on, which is the weaker of the two and says so in the row.
    fn reached(&mut self) -> Option<bool> {
        None
    }
    /// Save the device where it stands, so a forked step can try a candidate
    /// and come back (t-6044). `None` when this world cannot — a page, the
    /// desktop, an iOS simulator, an Android device whose save failed — and
    /// then the step is taken once, as today. What comes back is what
    /// [`Self::restore`] and [`Self::forget`] take.
    fn save(&mut self) -> Option<Saved> {
        None
    }
    /// Put the device back where [`Self::save`] left it. `false` when it
    /// could not, and then the device stands where the last press left it.
    fn restore(&mut self, saved: &Saved) -> bool {
        let _ = saved;
        false
    }
    /// Let go of a saved state the fork is done with, so a device does not
    /// fill with the states of every step it ever forked. Best effort.
    fn forget(&mut self, saved: &Saved) {
        let _ = saved;
    }
    /// Whether this world can enter a written value into a field its last
    /// look read (t-6720) — a page's, when the walk was handed a writer.
    /// `false`, the question never offers [`TYPE_TEXT`].
    fn types(&self) -> bool {
        false
    }
    /// Enter into the field `mark` names on the last look the value `goal`
    /// needs there: the value written (or remembered) for exactly this
    /// field, pressed by its pinned number first and typed down the door's
    /// value road. What was typed never comes back — only where it came from.
    fn type_into(&mut self, mark: usize, goal: &str) -> Typed {
        let _ = (mark, goal);
        Typed::Refused(NO_TYPING.to_string())
    }
}

/// Why a world typed nothing: it cannot type at all; its last look read no
/// such field (no control, no selector of its own, no field facts); the
/// field's pinned press was refused; the door refused the typing.
pub const NO_TYPING: &str = "no_typing";
pub const NO_FIELD: &str = "no_field";
pub const PRESS_REFUSED: &str = "press_refused";
pub const TYPE_REFUSED: &str = "type_refused";

/// Where a typed value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueSource {
    /// Written now, by `model`, in `ms`.
    Written { model: String, ms: u64 },
    /// Typed again from what an identical input was written before — no
    /// model asked. Kept apart from a written one in every row, so a reused
    /// value is never counted as a sample of a model's speed or accuracy.
    Reused,
}

impl ValueSource {
    /// The word a row says it by.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Written { .. } => "written",
            Self::Reused => "reused",
        }
    }
}

/// What entering a value into a field came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// The value went in: where it came from, and how many characters it
    /// was — never what it was.
    Typed { source: ValueSource, chars: usize },
    /// Nothing went in, and the word for why: no field there, a pin that
    /// broke, a wire that never answered, a value the seat's rules refused.
    Refused(String),
}

/// Why a walk is asking the screen anything.
#[derive(Debug, Clone, Copy)]
pub enum Why<'a> {
    /// A recorded walk stopped here.
    Cleared {
        stop: RecipeStop,
        /// The stopped step's VERB alone; a `type` line's argv carries a value.
        step: &'a str,
        refusal: &'a str,
        /// The step to walk again from — the report's own `next`.
        next: usize,
    },
    /// A goal named at a branch in an otherwise deterministic procedure.
    Goal {
        /// The most presses this walk may spend, as the caller asked and the
        /// verb's parser already clamped.
        steps: usize,
    },
}

/// What a walk is asking about, as a judgment reads it.
#[derive(Debug, Clone, Copy)]
pub struct Errand<'a> {
    /// What the walk is for — the flow's name and the check that stopped, or
    /// the sentence a person wrote.
    pub goal: &'a str,
    pub why: Why<'a>,
    pub flow: Option<&'a FlowSpec>,
    /// Whether the document has a money step at all.
    pub moves_money: bool,
}

impl Errand<'_> {
    /// The most presses this errand may spend.
    #[must_use]
    pub const fn steps(&self) -> usize {
        match self.why {
            Why::Cleared { .. } => MAX_RECOVERY_ATTEMPTS,
            Why::Goal { steps } => steps,
        }
    }

    /// The word a row uses for this errand.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self.why {
            Why::Cleared { .. } => "clear",
            Why::Goal { .. } => "goal",
        }
    }

    /// The errand as the question reads it.
    fn asked(&self) -> zerocode_core::screen_action::Errand<'_> {
        match self.why {
            Why::Cleared {
                stop,
                step,
                refusal,
                ..
            } => zerocode_core::screen_action::Errand::Clear {
                stopped: stop.as_str(),
                step,
                refusal,
            },
            Why::Goal { .. } => zerocode_core::screen_action::Errand::Goal,
        }
    }
}

/// Why a walk did not proceed, before a judgment or before a press.
/// Each is recorded so a person can see which gate held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Barred {
    /// Nobody asked for it.
    Off,
    /// This stop is a person's, was set by one, or resumes past its step.
    NotRecoverable,
    /// The document moves money.
    MovesMoney,
    /// The Flow presses past its own gate.
    Guarded,
    /// No step to walk again from.
    NoResume,
    /// Not enough of the call's clock left to ask and still act.
    NoBudget,
    /// The caller asked for no presses at all.
    NoSteps,
    /// A valid choice does not meet its seat's screen-press confidence floor.
    LowConfidence,
    /// The screen's own text tells an assistant what to do (t-6187): the
    /// walk steps back to the person rather than press on a screen that is
    /// giving it orders.
    Injected,
    /// The screen is a wall — a sign-in, a robot check, an error dialog — in
    /// front of the page the goal needs (t-6187).
    Walled,
}

impl From<Stopped> for Barred {
    fn from(stopped: Stopped) -> Self {
        match stopped {
            Stopped::Injected => Self::Injected,
            Stopped::Walled => Self::Walled,
        }
    }
}

impl Barred {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => JevMode::Off.key(),
            Self::NotRecoverable => "not_recoverable",
            Self::MovesMoney => "moves_money",
            Self::Guarded => "guarded",
            Self::NoResume => "no_resume",
            Self::NoBudget => "no_budget",
            Self::NoSteps => "no_steps",
            Self::LowConfidence => "low_confidence",
            Self::Injected => Stopped::Injected.word(),
            Self::Walled => Stopped::Walled.word(),
        }
    }
}

/// Whether this errand may be walked at all, before anything is looked at or
/// asked. Pure: the whole gate reads its answer from here, and both errands
/// read it from the same place — which is how a goal walk cannot quietly
/// acquire a narrower set of gates than a recovery has.
#[must_use]
pub fn barred(mode: Mode, at: &Errand<'_>, left_ms: u64) -> Option<Barred> {
    if !mode.asks() {
        return Some(Barred::Off);
    }
    if let Why::Cleared { stop, next, .. } = at.why {
        if !matches!(stop, RecipeStop::StepFailed | RecipeStop::CheckFailed) {
            return Some(Barred::NotRecoverable);
        }
        if next == 0 {
            return Some(Barred::NoResume);
        }
    }
    if at.moves_money || at.flow.is_some_and(|spec| spec.money.is_some()) {
        return Some(Barred::MovesMoney);
    }
    if at.flow.is_some_and(|spec| spec.policy == Policy::Guarded) {
        return Some(Barred::Guarded);
    }
    if at.steps() == 0 {
        return Some(Barred::NoSteps);
    }
    let asking = u64::try_from(ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX);
    if left_ms <= asking {
        return Some(Barred::NoBudget);
    }
    None
}

/// What the person set for `seat`, read from the settings file the TypeSafe
/// card writes — by the row's own parser. Unreadable, missing or misspelt all
/// read as [`JevMode::Off`]: a walk never starts sending a screen anywhere
/// because a file could not be parsed.
#[must_use]
pub fn mode_now(seat: &JevUse) -> Mode {
    let root = crate::api_routers::zo_settings_path()
        .and_then(|path| crate::api_routers::read_zo_settings_root(&path).ok())
        .map_or(Value::Null, Value::Object);
    seat.mode_in(&root)
}

/// What a walk's report and its lines say about where it stopped. Owned,
/// because a report is a value and a borrow of it would outlive the read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoppedRead {
    pub stop: RecipeStop,
    /// The step that stopped, counted from 1.
    pub at: usize,
    /// The step to walk again from — the report's own `next`.
    pub next: usize,
    /// The stopped step's verb alone.
    pub step: String,
    /// What it was refused with.
    pub refusal: String,
    /// The browser pane the stopped step aimed at.
    pub pane: String,
}

/// Read a stopped walk, or `None` when there is nothing here to clear.
///
/// It answers `None` for a walk that finished, for a report naming no stop or
/// no resume point, and — the rule that is not obvious — for a stopped step
/// that did not go through the BROWSER door. Numbering a page's controls is
/// that door's; a desktop step's screen is numbered by another door and
/// reaches this loop as a goal walk of its own, not as a recovery.
///
/// Whether the stop MAY be cleared is [`barred`]'s answer, not this one:
/// reading and allowing are kept apart so a test can hold each on its own.
#[must_use]
pub fn read_report(report: &Value, lines: &[RecipeLine]) -> Option<StoppedRead> {
    if report["done"] == json!(true) {
        return None;
    }
    let stop = RecipeStop::from_word(report.pointer("/stop/kind")?.as_str()?)?;
    let at = usize::try_from(report["stoppedAt"].as_u64()?).ok()?;
    let next = usize::try_from(report["next"].as_u64()?).ok()?;
    let line = lines.iter().find(|line| line.step == at)?;
    if line.tool != RecipeTool::Browser {
        return None;
    }
    // `<verb> <pane> …`: the verb alone, never the rest — a `type` line's
    // argv carries the text a person typed.
    let step = line.argv.first()?.clone();
    let pane = line.argv.get(1)?.clone();
    Some(StoppedRead {
        stop,
        at,
        next,
        step,
        refusal: report
            .pointer("/stop/message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        pane,
    })
}

/// How a row says the judgment was used: recorded beside the walk, as the
/// record-only modes do.
///
/// The three words are the use table's, not this file's: every Jev ledger
/// carries this column and a reader sweeping them all — the promotion judge,
/// a script counting how often a seat changed anything — reads one spelling
/// or none (`zerocode_core::jev`).
const USE_SHADOW: &str = JevMode::Shadow.key();
const USE_APPLIED: &str = zerocode_core::jev::ROUTE_USE_APPLIED;
const USE_FALLBACK: &str = zerocode_core::jev::ROUTE_USE_FALLBACK;

/// The key a row carries its reason under, beside `routeUse`: why the hand
/// never went out although the judgment named a number. Written here and read
/// by [`no_press_reason`] alone, so the walk's rows and the words it answers
/// in cannot come to spell it differently.
pub(crate) const REASON: &str = "reason";

/// The key a row names the kind of control its judgment named under
/// ([`ControlKind::word`], t-6187): what the press rule read, and what a
/// later reader counts destructive presses by — the core counter's own key
/// (`zerocode_core::jev::summary::CONTROL_KIND`), so the writer and the
/// counter cannot come to spell it two ways.
pub(crate) const CONTROL_KIND: &str = zerocode_core::jev::summary::CONTROL_KIND.canonical;

/// The key a row names why no hand went out under — a guard's stop, a floor,
/// a gate — the core counter's own key (`zerocode_core::jev::summary::BARRED`),
/// which counts the guards' stops off it (t-6277 D6).
pub(crate) const BARRED: &str = zerocode_core::jev::summary::BARRED.canonical;

/// The key a row says under which way a judgment begun ahead of the walk
/// went ([`Options::overlap`]), and its two words: the walk asked the very
/// question and used the answer, or the next look asked another and the
/// answer was dropped with its request spent.
pub const OVERLAP: &str = "overlap";
pub const OVERLAP_USED: &str = "used";
pub const OVERLAP_DISCARDED: &str = "discarded";
/// A judgment begun on the page a press changed that the page's settle then
/// cancelled — it did not end `ready` (t-9712): its request spent, the page
/// looked at again and asked afresh.
pub const OVERLAP_CANCELLED: &str = "cancelled";

/// The key a row keeps what the second reader said under
/// ([`Options::rescue`]): its outcome, what it chose, its confidence and
/// how long it took — beside the seat's own judgment, which stays the row's
/// `confidence` and `chosen`. `rescuedBy` names who pressed when the second
/// reader did.
pub const RESCUE: &str = "rescue";
pub const RESCUED_BY: &str = "rescuedBy";
pub const RESCUED_BY_TEAM: &str = "team";

/// The keys a row says what the heads beside the action said under
/// (t-6720): the operation when it was not a press ([`TYPE_TEXT`]), what an
/// entry came to, what the field's head answered, and what the observation
/// heads chose.
pub const OPERATION: &str = "operation";
pub const TYPED: &str = "typed";
pub const TYPE_TARGET_KEY: &str = "typeTarget";
pub const OBSERVED: &str = "observed";

/// The key a look names a control's or a candidate's own identity under —
/// the only name a walk ever acts on, and never one it composed.
const SELECTOR_KEY: &str = snapshot::SELECTOR_KEY;

/// The key a row says how the pressed screen settled under, when the world's
/// press waits for it to stop changing (a phone's, t-6385; a page's that left
/// its legend, t-9876) or leaves it for the next look (a page's, t-9712) — the
/// doors' own word, so a click's or a look's answer and the walk's row cannot
/// come to spell it two ways.
pub const SETTLE: &str = zerocode_core::agent_emulator::EMULATOR_SETTLE_KEY;

/// Why this walk pressed nothing, when one of its rows says why.
///
/// The caller asks here rather than reading `pressed: 0` and guessing. A walk
/// under a seat that only records is indistinguishable, from the outside,
/// from one that found nothing to press — the shape that cost the v1.1.3
/// measurement thirteen walks before anyone looked at the seat (t-5455).
#[must_use]
pub fn no_press_reason(rows: &[Value]) -> Option<&str> {
    rows.iter()
        .find_map(|row| row.get(REASON).and_then(Value::as_str))
}

/// What a walk came to: the rows it wrote, the report of the document's
/// re-walk when there was one, and — for a goal — whether it got there.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Walked {
    pub rows: Vec<Value>,
    pub report: Option<Value>,
    /// How many presses the walk actually made.
    pub pressed: usize,
    /// Whether the goal was reached, and by what: `Some(true)` only when the
    /// caller's own condition said so or, failing that, when the judgment
    /// said `done`. `None` for an errand that has no goal to reach.
    pub reached: Option<bool>,
    /// What the walk itself went on to show about the numbers it pressed —
    /// the hindsight the seat's promotion is judged on
    /// (docs/design/jev-seats-accuracy-wave-20260921.md §4, decision 2),
    /// stamped onto every press of this walk as
    /// [`zerocode_core::jev::summary::AGREED`].
    ///
    /// A walk and not a press is the unit: a goal reached in three presses
    /// says all three were right, and counting the first two as misses
    /// because the screen was not yet there would judge the seat on a
    /// question nobody asked it. `None` when nothing confirmed the walk
    /// either way — a goal walk given no `until`, or one the judgment ended
    /// with its own `done` — and those walks are left out of the agreement
    /// rather than guessed at.
    pub agreed: Option<bool>,
    /// The branching seat's rows (t-6044): one per forked step, written to
    /// that seat's own ledger by the caller, never to the screen seat's.
    pub forks: Vec<Value>,
    /// Judgments begun ahead of the walk that the walk went on to use
    /// ([`Options::overlap`]).
    pub overlapped: usize,
    /// Judgments begun ahead that the next look made moot — their request
    /// spent, the screen asked afresh.
    pub discarded: usize,
    /// Judgments begun on the page a press changed that its settle cancelled
    /// before the next look (t-9712) — their request spent, the page looked
    /// at again.
    pub cancelled: usize,
    /// Steps the seat's judgment left under its press floor that the second
    /// reader pressed for ([`Options::rescue`]).
    pub rescued: usize,
    /// Steps the second reader was asked about and could not press for —
    /// the walk stepped back to the person as it does today.
    pub rescue_failed: usize,
    /// Presses that entered a value into a field (t-6720) — counted among
    /// [`Self::pressed`] too, since a hand went out for each.
    pub typed: usize,
}

/// One row, with the words every ledger of this family uses.
fn row(mode: Mode, at: &Errand<'_>, attempt: usize, more: Value) -> Value {
    let mut row = json!({
        "flow": at.goal,
        "errand": at.key(),
        "mode": mode.key(),
        "rubricVersion": SCREEN_ACTION_RUBRIC_VERSION,
        "attempt": attempt,
    });
    if let Why::Cleared { stop, step, .. } = at.why
        && let Some(row) = row.as_object_mut()
    {
        row.insert("stop".to_string(), json!(stop.as_str()));
        row.insert("step".to_string(), json!(step));
    }
    if let (Some(row), Some(more)) = (row.as_object_mut(), more.as_object()) {
        for (key, value) in more {
            row.insert(key.clone(), value.clone());
        }
    }
    row
}

/// Put one more word on a row already being written.
fn note(said: &mut Value, key: &str, value: Value) {
    if let Some(said) = said.as_object_mut() {
        said.insert(key.to_string(), value);
    }
}

/// Walk the screen by judgment until the errand is served or a gate ends it.
///
/// Off, barred, unreadable, unanswered, refused at the door, stuck on a screen
/// that will not move, or out of steps, the answer carries no report and the
/// caller ends up exactly where it would have without a judgment at all. That
/// equivalence is the feature's first promise and
/// `off_and_shadow_change_nothing_about_the_walk` holds it.
///
/// `acting` is whether this seat presses at all, and it is the caller's to
/// answer rather than the mode's: under `auto` a seat presses once its own
/// ledger has promoted it (`crate::systemone::applies`), and the ledger is a
/// file this module has no business reading in the middle of a walk. `mode`
/// still says whether anything is ASKED, and still names itself on every row.
///
/// The window's two callers ask through [`run_with`], with the switches the
/// verb was given; this plain form is the tests' — every switch off, no
/// second reader — and the promise [`run_with`] keeps is that it is this.
#[cfg(test)]
pub fn run(
    mode: Mode,
    acting: bool,
    at: &Errand<'_>,
    judge: &mut dyn ActionJudge,
    world: &mut dyn World,
) -> Walked {
    run_with(
        mode,
        acting,
        Branching::OFF,
        at,
        judge,
        world,
        Options::default(),
        None,
    )
}

/// [`run`], with the branching seat's standing beside the screen seat's
/// (t-6044) and the switches a caller may set (t-6132): `branching` says
/// whether a phone step whose judgment ranked two or more controls is asked
/// about at all, and whether the comparison's pick is the one pressed;
/// [`Options`] are the walk's own switches, and `rescue` the second reader
/// they may ask ([`Options::rescue`]). [`Branching::OFF`] with every switch
/// off is [`run`] byte for byte, whoever was handed in.
#[allow(clippy::too_many_arguments)]
pub fn run_with(
    mode: Mode,
    acting: bool,
    branching: Branching,
    at: &Errand<'_>,
    judge: &mut dyn ActionJudge,
    world: &mut dyn World,
    options: Options,
    rescue: Option<&mut dyn ActionJudge>,
) -> Walked {
    let mut walked = walk(mode, acting, branching, at, judge, world, options, rescue);
    agree(&mut walked);
    walked
}

/// Whether pressing `item` carries the screen elsewhere — a link, on a page
/// or in an app's tree — so the screen after it is not one the last look
/// can stand in for ([`Options::overlap`]).
#[must_use]
pub fn moves_the_page(item: &Value) -> bool {
    item.get("role")
        .and_then(Value::as_str)
        .is_some_and(|role| role.eq_ignore_ascii_case("link") || role.ends_with("Link"))
}

/// A row's words with a judgment-in-flight note on them, when there is one.
fn overlapped(mut said: Value, overlap: Option<&Value>) -> Value {
    if let (Some(said), Some(Value::Object(overlap))) = (said.as_object_mut(), overlap) {
        for (key, value) in overlap {
            said.insert(key.clone(), value.clone());
        }
    }
    said
}

/// What the walk itself said about the numbers it pressed, written onto the
/// presses it said it of (§4, decision 2).
///
/// A walk that pressed nothing agreed with nothing: the mark is about
/// judgments that were acted on, and a shadow row or a barred one is not one
/// of those.
fn agree(walked: &mut Walked) {
    if walked.pressed == 0 {
        walked.agreed = None;
        return;
    }
    let Some(agreed) = walked.agreed else {
        return;
    };
    for row in &mut walked.rows {
        if row.get("pressed").and_then(Value::as_bool) == Some(true) {
            note(row, AGREED.canonical, json!(agreed));
        }
    }
}

/// The walk itself, up to whichever gate ends it — [`run`] is this and the
/// hindsight mark, kept apart so that every one of the exits below lands in
/// one place that can stamp them.
#[allow(clippy::too_many_arguments)]
fn walk(
    mode: Mode,
    acting: bool,
    branching: Branching,
    at: &Errand<'_>,
    judge: &mut dyn ActionJudge,
    world: &mut dyn World,
    options: Options,
    mut rescue: Option<&mut dyn ActionJudge>,
) -> Walked {
    let mut walked = Walked::default();
    // The forked step waiting for the walk's next step to grade it (t-6044):
    // the row's place among `walked.forks`, and whether the comparison named
    // the candidate that was actually pressed.
    let mut fork: Option<branch::Pending> = None;
    if matches!(at.why, Why::Goal { .. }) {
        walked.reached = Some(false);
    }
    let left = world.left_ms();
    if let Some(barred) = barred(mode, at, left) {
        // Off is the default and says nothing; the other gates are worth a row.
        if barred != Barred::Off {
            walked.rows.push(row(
                mode,
                at,
                0,
                json!({ "outcome": "barred", BARRED: barred.as_str() }),
            ));
        }
        return walked;
    }

    // The numbers already spent ON THE SCREEN IN FRONT OF THE WALK, and the
    // screen they were spent on. A screen that moved is a new screen: its
    // numbers mean something else, so the list starts again.
    let mut tried: Vec<usize> = Vec::new();
    // What this walk pressed, oldest first, as the legend named each control
    // when it was pressed. Unlike `tried` it survives a screen that moved:
    // the display a press just changed is what the next question reads
    // (t-5497 — a calculator walk pressed `7` three times because every
    // new display looked like a fresh screen with `7` on offer).
    let mut pressed_so_far: Vec<String> = Vec::new();
    // The fields this walk entered a value into, by the look's own selector
    // for each: none is offered for a second entry (t-6720).
    let mut entered: Vec<String> = Vec::new();
    let mut before: Option<Screen> = None;
    let mut still = 0usize;
    // The judgment begun on the last look, if the walk asked ahead
    // ([`Options::overlap`]): used when the next look asks the same question,
    // dropped when it does not.
    let mut ahead: Option<Pending> = None;
    // Whether the last press's settle cancelled the judgment begun on the page
    // it left (t-9712) — the next row says so.
    let mut cancelled = false;
    for attempt in 1..=at.steps() {
        if world.left_ms() <= u64::try_from(ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX) {
            // Out of time with the errand unserved: whatever was pressed on
            // the way here did not get the walk there (§4, decision 2).
            walked.agreed = Some(false);
            walked.rows.push(row(
                mode,
                at,
                attempt,
                json!({ "outcome": "barred", BARRED: Barred::NoBudget.as_str() }),
            ));
            return walked;
        }
        let looking = std::time::Instant::now();
        let Some(screen) = world.look() else {
            walked
                .rows
                .push(row(mode, at, attempt, json!({ "outcome": "no_look" })));
            return walked;
        };
        let look_ms = u64::try_from(looking.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(reason) = barred(mode, at, world.left_ms()) {
            walked.rows.push(row(
                mode,
                at,
                attempt,
                json!({
                    "outcome": "barred", BARRED: reason.as_str(), "look_ms": look_ms,
                }),
            ));
            return walked;
        }
        match before.as_ref() {
            Some(was) if was.same_as(&screen) => {
                // A forked step whose pick left the screen where it was is a
                // step the walk now retries (t-6044).
                branch::settle(&mut walked, fork.take(), NextStep::SameScreen);
                still += 1;
                if still >= SAME_SCREEN_LIMIT {
                    walked.agreed = Some(false);
                    walked.rows.push(row(
                        mode,
                        at,
                        attempt,
                        json!({ "outcome": "stuck", "pressed": walked.pressed }),
                    ));
                    return walked;
                }
            }
            Some(_) => {
                still = 0;
                tried.clear();
            }
            None => {}
        }
        // One request asks every head the look can answer: the action, the
        // field when the world can type into one it read, and the containers,
        // images and rows it read (t-6720).
        let Some(asked) = ask_with(
            &ActionLook {
                goal: at.goal,
                errand: at.asked(),
                at: screen.at.asked(),
                tried: &tried,
                items: &screen.items,
                pressed: &pressed_so_far,
                shows: &screen.shows,
            },
            &screen.beside(world.types(), &screen.fields_left(&entered)),
        ) else {
            walked.agreed = Some(false);
            walked
                .rows
                .push(row(mode, at, attempt, json!({ "outcome": "no_candidate" })));
            return walked;
        };
        let press_policy = seat_of(screen.at.surface());
        let shows_lines = screen.shows.len();
        before = Some(screen);

        let candidates = asked.marks().len();
        let judging = std::time::Instant::now();
        // A judgment begun on the last look answers this question only when
        // it IS this question; the row says which way it went, and how much
        // of the judgment the walk never waited for.
        let (judged, overlap) = match ahead.take() {
            Some(begun) if begun.matches(&asked) => {
                let hidden_ms = begun.ran_ms();
                walked.overlapped += 1;
                (
                    judge.finish(begun.wait()),
                    Some(json!({ OVERLAP: OVERLAP_USED, "hiddenMs": hidden_ms })),
                )
            }
            Some(begun) => {
                walked.discarded += 1;
                drop(begun);
                (
                    judge.choose(&asked),
                    Some(json!({ OVERLAP: OVERLAP_DISCARDED })),
                )
            }
            None if std::mem::take(&mut cancelled) => (
                judge.choose(&asked),
                Some(json!({ OVERLAP: OVERLAP_CANCELLED })),
            ),
            None => (judge.choose(&asked), None),
        };
        let judgment_ms = u64::try_from(judging.elapsed().as_millis()).unwrap_or(u64::MAX);
        let spent = judge.spent();
        let ActionRead {
            choice,
            typed,
            observed,
        } = match judged {
            Judged::Chose(read) => read,
            Judged::Refused(token) => {
                // A next step nobody judged shows nothing about the fork.
                branch::settle(&mut walked, fork.take(), NextStep::Unknown);
                walked.rows.push(row(
                    mode,
                    at,
                    attempt,
                    overlapped(
                        stamped(
                            json!({
                                "outcome": token,
                                "candidates": candidates,
                                "routeUse": USE_FALLBACK,
                            }),
                            spent.as_ref(),
                            judgment_ms,
                            crate::project_runtime::now_epoch_ms(),
                        ),
                        overlap.as_ref(),
                    ),
                ));
                return walked;
            }
        };
        let mut said = overlapped(
            stamped(
                json!({
                    "outcome": "answered",
                    "candidates": candidates,
                    "showsLines": shows_lines,
                    "pressedBefore": pressed_so_far.len(),
                    "confidence": choice.confidence,
                    "probabilities": choice.probabilities,
                }),
                spent.as_ref(),
                judgment_ms,
                crate::project_runtime::now_epoch_ms(),
            ),
            overlap.as_ref(),
        );
        // What the heads asked beside the action said (t-6720): the field's
        // head when it was asked, and every observation head — the look's
        // own number of the candidate chosen and that candidate's own
        // identity, never one composed here.
        if let Some(typed) = &typed {
            note(
                &mut said,
                TYPE_TARGET_KEY,
                json!({
                    "chosen": typed.chosen,
                    "confidence": typed.confidence,
                    "probabilities": typed.probabilities,
                }),
            );
        }
        if !observed.is_empty() {
            let seen = before
                .as_ref()
                .expect("the screen this walk just looked at");
            note(&mut said, OBSERVED, observed_note(seen, &observed));
        }
        // The screen moved past the last forked step: what the judgment says
        // of the new screen is that step's mark (t-6044) — a control to press
        // or a field to type into is the walk going on, `done` is the goal,
        // `give_up` is a dead end.
        if fork.is_some() {
            let next = match choice.chosen {
                Chosen::Mark(_) | Chosen::Type(_) => NextStep::MovedOn,
                Chosen::Done => NextStep::Reached,
                Chosen::GiveUp => NextStep::GaveUp,
            };
            branch::settle(&mut walked, fork.take(), next);
        }
        // A memo hit is an answer that sent nothing: the row says so in the
        // one word every Jev ledger's counter reads it by.
        if judge.cached() {
            note(&mut said, CACHED.canonical, json!(true));
        }
        // What the two guards asked beside the choice said, per thousand,
        // on every answered row — recording or acting (t-6187).
        if let Some(guard) = choice.guard {
            for (key, permille) in guard.permille() {
                note(&mut said, key, json!(permille));
            }
        }
        // The hand goes out to one number either way: a control to press, or
        // a field to type into (t-6720). Everything up to the hand — the
        // row's words, the stand, the guards and the floor — is one road.
        let (chosen, typing) = match choice.chosen {
            Chosen::Mark(mark) => (mark, false),
            Chosen::Type(field) => (field, true),
            Chosen::GiveUp | Chosen::Done => {
                let ended = match choice.chosen {
                    Chosen::Done => zerocode_core::screen_action::DONE,
                    _ => zerocode_core::screen_action::GIVE_UP,
                };
                note(&mut said, "chosen", json!(ended));
                note(&mut said, "routeUse", json!(USE_FALLBACK));
                // Giving up is the judgment saying the presses so far led
                // nowhere; `done` is it saying the opposite about a screen
                // nothing checked, which §4 leaves out of the agreement
                // rather than counting on the judgment's own word.
                if choice.chosen == Chosen::GiveUp {
                    walked.agreed = Some(false);
                }
                if choice.chosen == Chosen::Done {
                    // The weaker of the two ends: nothing was checked, the
                    // judgment simply says it is there. The row says which
                    // end it was, so evidence never reads one as the other.
                    note(&mut said, "reachedBy", json!("judgment"));
                    walked.reached = Some(true);
                }
                walked.rows.push(row(mode, at, attempt, said));
                return walked;
            }
        };
        note(&mut said, "chosen", json!(option_of(chosen)));
        if typing {
            note(&mut said, OPERATION, json!(TYPE_TEXT));
        }
        // The control the judgment named, by kind, on every row that names
        // one — recording or acting — and whether the one press rule lets it
        // go (t-6187).
        let (permitted, kind) = press_rule(
            press_policy,
            options.act_line,
            before
                .as_ref()
                .expect("the screen this walk just looked at"),
            chosen,
            choice.confidence,
        );
        note(&mut said, CONTROL_KIND, json!(kind.word()));

        // A seat that is not acting records what it would have pressed and
        // presses nothing — and says so, in the word the stand itself is
        // named by. Without it the row is a judgment with no consequence and
        // no account of why, which reads from the outside exactly like a
        // screen that had nothing worth pressing (t-5455).
        if !acting {
            note(&mut said, "routeUse", json!(USE_SHADOW));
            note(&mut said, "pressed", json!(false));
            note(&mut said, REASON, json!(SEAT_RECORDING));
            walked.rows.push(row(mode, at, attempt, said));
            return walked;
        }

        // A screen whose text gives the walk orders, or a wall in front of
        // the page the goal needs, is not pressed on — by this judgment or by
        // a second reader's (t-6187). The walk steps back to the person, as a
        // judgment under the press floor does, and the row names the stop.
        if let Some(stopped) = choice.guard.and_then(Guard::stops) {
            let word = Barred::from(stopped).as_str();
            note(&mut said, BARRED, json!(word));
            // The walk's own answer says why no hand went out
            // ([`no_press_reason`]), so the one who asked can tell the person.
            note(&mut said, REASON, json!(word));
            note(&mut said, "pressed", json!(false));
            note(&mut said, "routeUse", json!(USE_FALLBACK));
            walked.rows.push(row(mode, at, attempt, said));
            return walked;
        }

        // The second rung ([`Options::rescue`]): a judgment under the seat's
        // press floor is put to the second reader as the same closed choice,
        // and its number is pressed under the same rule — or the walk steps
        // back to the person as it does today. An entry under the floor
        // steps back at once: the second reader answers one option and
        // names no field, so it rescues presses alone.
        let mut chosen = chosen;
        // The second reader's own answer, when it pressed: the ranking a
        // forked step reads its candidates off, in place of the seat's.
        let mut rescued_by: Option<ActionChoice> = None;
        if !permitted {
            let seen = before
                .as_ref()
                .expect("the screen this walk just looked at");
            let rescued = match rescue.as_deref_mut().filter(|_| options.rescue && !typing) {
                Some(team) => {
                    let asking = std::time::Instant::now();
                    let answered =
                        team.choose_within(&asked, Duration::from_millis(world.left_ms()));
                    let team_ms = u64::try_from(asking.elapsed().as_millis()).unwrap_or(u64::MAX);
                    let (word, mark) = second_rung(press_policy, options.act_line, seen, &answered);
                    if let (Some(_), Judged::Chose(second)) = (mark, &answered) {
                        rescued_by = Some(second.choice.clone());
                    }
                    note(
                        &mut said,
                        RESCUE,
                        json!({
                            "outcome": word,
                            "chosen": match &answered {
                                Judged::Chose(second) => chosen_word(second.choice.chosen),
                                Judged::Refused(token) => token.clone(),
                            },
                            "confidence": match &answered {
                                Judged::Chose(second) => json!(second.choice.confidence),
                                Judged::Refused(_) => Value::Null,
                            },
                            ELAPSED_MS.canonical: team_ms,
                        }),
                    );
                    mark
                }
                None => None,
            };
            match rescued {
                Some(mark) => {
                    walked.rescued += 1;
                    chosen = mark;
                    note(&mut said, RESCUED_BY, json!(RESCUED_BY_TEAM));
                    note(&mut said, "chosen", json!(option_of(mark)));
                    note(
                        &mut said,
                        CONTROL_KIND,
                        json!(control_kind(seen, mark).word()),
                    );
                }
                None => {
                    if options.rescue && rescue.is_some() && !typing {
                        walked.rescue_failed += 1;
                    }
                    note(&mut said, BARRED, json!(Barred::LowConfidence.as_str()));
                    note(&mut said, "pressed", json!(false));
                    note(&mut said, "routeUse", json!(USE_FALLBACK));
                    walked.rows.push(row(mode, at, attempt, said));
                    return walked;
                }
            }
        }
        let chosen = chosen;

        if typing {
            // The entry (t-6720): the world writes — or remembers — the value
            // this field needs, presses the field by its pinned number and
            // types down the door's value road. No fork and nothing asked
            // ahead: an entry changes the very field it named, so the next
            // look is always another question.
            let seen = before
                .as_ref()
                .expect("the screen this walk just looked at");
            let legend = legend_of(seen, chosen);
            let went_in = crate::run_evidence::observing(
                json!({
                    "look_ms": look_ms,
                    "judgment": { "asked": true, "ms": judgment_ms, "confidence": choice.confidence },
                }),
                || world.type_into(chosen, at.goal),
            );
            match went_in {
                Typed::Typed { source, chars } => {
                    note(&mut said, TYPED, typed_note(&source, chars));
                    tried.push(chosen);
                    pressed_so_far.push(typed_line(&legend));
                    if let Some(selector) = selector_of(seen, chosen) {
                        entered.push(selector);
                    }
                    walked.pressed += 1;
                    walked.typed += 1;
                    note(&mut said, "pressed", json!(true));
                    note(&mut said, "routeUse", json!(USE_APPLIED));
                }
                Typed::Refused(token) => {
                    // The walk's own answer says why no value went in
                    // ([`no_press_reason`]): no login, a wire that never
                    // answered, a value the seat refused, a pin that broke.
                    note(&mut said, REASON, json!(token));
                    note(&mut said, TYPED, json!({ "outcome": token }));
                    note(&mut said, "routeUse", json!(USE_FALLBACK));
                    note(&mut said, "pressed", json!(false));
                    walked.rows.push(row(mode, at, attempt, said));
                    return walked;
                }
            }
        } else {
            // The forked step (t-6044): on a phone whose judgment ranked two or
            // more controls, the branching seat may try the top candidates on a
            // saved device and make the comparison's pick canonical. Off, it is
            // the press below exactly; every other way out presses the number
            // chosen above — the first candidate — as today. A step the second
            // reader pressed for is forked on the second reader's own ranking:
            // its answer is the choice the walk goes out with (t-6132 S3).
            let forked = branch::step(
                &branch::Step {
                    branching,
                    at,
                    attempt,
                    screen: before.as_ref(),
                    choice: rescued_by.as_ref().unwrap_or(&choice),
                    chosen,
                    look_ms,
                },
                judge,
                world,
            );
            if forked.mark != chosen {
                // The screen seat's row keeps its own answer; the number the hand
                // went out with is the fork's, and the row says so.
                note(&mut said, "forked", json!(format!("mark:{}", forked.mark)));
            }
            let chosen = forked.mark;
            if let Some(row) = forked.row {
                fork = Some(branch::Pending {
                    row: walked.forks.len(),
                    same_pick: forked.same_pick,
                });
                walked.forks.push(row);
            }

            let seen = before
                .as_ref()
                .expect("the screen this walk just looked at");
            let legend = legend_of(seen, chosen);
            // Ask ahead ([`Options::overlap`]), before the press: the next
            // question as the last look would put it — this screen, the chosen
            // number spent, its legend among `pressed` — begun now, so the
            // judgment runs while the press lands and the next look is taken.
            // Only a goal walk comes back to a look; not after a link, whose
            // screen is another page; not when one more stand on this screen
            // would end the walk before it asked; not on the last step, which
            // asks nothing more.
            // A forked step asks ahead only once its canonical number stands: the
            // fork's own presses and looks are the fork's and not the walk's, so
            // nothing was begun over them, and the question begun here names the
            // number the hand goes out with.
            if options.overlap
                && world.asks_ahead_of_the_press()
                && matches!(at.why, Why::Goal { .. })
                && attempt < at.steps()
                && still + 1 < SAME_SCREEN_LIMIT
                && !presses_a_link(seen, chosen)
            {
                let mut tried_ahead = tried.clone();
                tried_ahead.push(chosen);
                let mut pressed_ahead = pressed_so_far.clone();
                pressed_ahead.push(legend.clone());
                // Every head the look's own question will ask, so the answer in
                // flight can be the one request the next step makes.
                ahead = ask_with(
                    &ActionLook {
                        goal: at.goal,
                        errand: at.asked(),
                        at: seen.at.asked(),
                        tried: &tried_ahead,
                        items: &seen.items,
                        pressed: &pressed_ahead,
                        shows: &seen.shows,
                    },
                    &seen.beside(world.types(), &seen.fields_left(&entered)),
                )
                .and_then(|question| judge.begin(&question));
            }
            let pressed = match forked.pressed {
                Some(pressed) => pressed,
                None => crate::run_evidence::observing(
                    json!({
                        "look_ms": look_ms,
                        "judgment": { "asked": true, "ms": judgment_ms, "confidence": choice.confidence },
                    }),
                    || world.press(chosen),
                ),
            };
            if !pressed {
                note(&mut said, "routeUse", json!(USE_FALLBACK));
                note(&mut said, "pressed", json!(false));
                walked.rows.push(row(mode, at, attempt, said));
                return walked;
            }
            tried.push(chosen);
            pressed_so_far.push(legend);
            walked.pressed += 1;
            note(&mut said, "pressed", json!(true));
            note(&mut said, "routeUse", json!(USE_APPLIED));
            let settled = world.settled();
            if let Some(settled) = &settled {
                note(&mut said, SETTLE, settled.note.clone());
            }
            // The screen the next question is asked on: the one a phone's press
            // settled on (t-6385), or — for a page whose press answered before
            // it settled — the page as the press changed it (t-9712); never
            // after a link, whose page is another's, and never a page the press
            // left as it was pressed: its change may still be on the way (a
            // result a fetch renders), so the look after its settle asks in
            // turn. The browser door settles such a press before it answers
            // (t-9876); this holds the walk to the same rule on any other.
            let left = match settled.and_then(|settled| settled.screen) {
                Some(screen) => Some(screen),
                None => world
                    .unsettled()
                    .filter(|page| !presses_a_link(seen, chosen) && !seen.same_as(page)),
            };
            // Ask ahead on it: the next question as the loop's own head will
            // put it — a screen that moved starts its numbers afresh, one that
            // did not keeps what it spent — begun now, so the judgment is
            // answered while the full look (a phone's) or the settle and the
            // look (a page's) are taken. The look's question decides: the same
            // bytes use the answer, any other drops it (an element only the
            // full look finds, a screen still moving).
            if options.overlap
                && ahead.is_none()
                && matches!(at.why, Why::Goal { .. })
                && attempt < at.steps()
                && let Some(screen) = left
            {
                let moved = !seen.same_as(&screen);
                if moved || still + 1 < SAME_SCREEN_LIMIT {
                    let tried_next = if moved { Vec::new() } else { tried.clone() };
                    ahead = ask_with(
                        &ActionLook {
                            goal: at.goal,
                            errand: at.asked(),
                            at: screen.at.asked(),
                            tried: &tried_next,
                            items: &screen.items,
                            pressed: &pressed_so_far,
                            shows: &screen.shows,
                        },
                        &screen.beside(world.types(), &screen.fields_left(&entered)),
                    )
                    .and_then(|question| judge.begin(&question));
                }
            }
            // A page whose press answered before it settled is settled now,
            // behind the judgment just begun (t-9712). The settle's own words go
            // on the row; a page that did not end `ready` — still moving at the
            // wall, another document, a pane gone, a settle nobody heard end —
            // cancels that judgment, and the next look reads the page again.
            if let Some(late) = world.settle() {
                let ready = zerocode_core::agent_browser::settle_said_ready(&late.note);
                note(&mut said, SETTLE, late.note);
                if !ready && let Some(begun) = ahead.take() {
                    drop(begun);
                    walked.cancelled += 1;
                    cancelled = true;
                }
            }
        }

        match at.why {
            Why::Cleared { next, .. } => {
                note(&mut said, "resumedFrom", json!(next));
                let after = world.walk_from(next);
                // Pressing is not succeeding: the row's `recheck` is the walk
                // that followed, read from its own report, never the door's
                // exit code.
                let cleared = after
                    .as_ref()
                    .is_some_and(|report| cleared_past(report, next));
                note(&mut said, "recheck", json!(cleared));
                walked.agreed = Some(cleared);
                walked.rows.push(row(mode, at, attempt, said));
                walked.report = after;
                // The re-walk is the forked step's next step (t-6044): past
                // the stop is the walk going on, the same stop again a retry.
                branch::settle(
                    &mut walked,
                    fork.take(),
                    if cleared {
                        NextStep::Reached
                    } else {
                        NextStep::SameScreen
                    },
                );
                if cleared {
                    return walked;
                }
            }
            Why::Goal { .. } => {
                // The caller's own condition, asked of the screen. It is the
                // only verified end a goal walk has; `done` is a judgment's
                // word about itself.
                let reached = world.reached();
                if let Some(reached) = reached {
                    note(&mut said, "recheck", json!(reached));
                    walked.agreed = Some(reached);
                }
                walked.rows.push(row(mode, at, attempt, said));
                if reached == Some(true) {
                    branch::settle(&mut walked, fork.take(), NextStep::Reached);
                    walked.reached = Some(true);
                    return walked;
                }
            }
        }
    }
    walked
}

/// What the second reader's answer comes to under the seat's own press rule
/// ([`Options::rescue`]): the number to press, and the word the row says it
/// by. `pressed` when it named a number the seat would press; `low_confidence`
/// when its confidence is under the floor; `link` when the number carries the
/// screen elsewhere — a rescue that navigates away is the person's call;
/// `give_up`/`done` when it declined to press; the wire's own token when it
/// answered nothing.
fn second_rung(
    policy: &JevUse,
    line: Option<u16>,
    seen: &Screen,
    answered: &Judged,
) -> (String, Option<usize>) {
    match answered {
        Judged::Refused(token) => (token.clone(), None),
        Judged::Chose(second) => match second.choice.chosen {
            Chosen::GiveUp => (zerocode_core::screen_action::GIVE_UP.to_string(), None),
            Chosen::Done => (zerocode_core::screen_action::DONE.to_string(), None),
            // A second reader names one option and never a field
            // ([`ActionAsk::choice_of`]); an entry is not a press it rescues.
            Chosen::Type(_) => (TYPE_TEXT.to_string(), None),
            Chosen::Mark(mark)
                if !press_rule(policy, line, seen, mark, second.choice.confidence).0 =>
            {
                (Barred::LowConfidence.as_str().to_string(), None)
            }
            Chosen::Mark(mark) if presses_a_link(seen, mark) => ("link".to_string(), None),
            Chosen::Mark(mark) => ("pressed".to_string(), Some(mark)),
        },
    }
}

/// The word a row names what an answer chose by: the number's option, or
/// the end it chose.
fn chosen_word(chosen: Chosen) -> String {
    match chosen {
        Chosen::Mark(mark) | Chosen::Type(mark) => option_of(mark),
        Chosen::GiveUp => zerocode_core::screen_action::GIVE_UP.to_string(),
        Chosen::Done => zerocode_core::screen_action::DONE.to_string(),
    }
}

/// What a row says of an entry that went in (t-6720): where its value came
/// from and how many characters it was — never the value. A written value
/// names its model and how long it took; a reused one names neither, so no
/// counter reads it as a sample of the model.
fn typed_note(source: &ValueSource, chars: usize) -> Value {
    let mut note = json!({ "source": source.word(), "chars": chars });
    if let (ValueSource::Written { model, ms }, Some(fields)) = (source, note.as_object_mut()) {
        fields.insert(MODEL.canonical.to_string(), json!(model));
        fields.insert("valueMs".to_string(), json!(ms));
    }
    note
}

/// What a row says the observation heads chose (t-6720): per head, the
/// look's own number of the candidate (`null` for none), the head's
/// confidence, and the candidate's own identity as the look carried it.
fn observed_note(seen: &Screen, observed: &[zerocode_core::screen_action::Observed]) -> Value {
    let mut note = serde_json::Map::new();
    for one in observed {
        let candidate = one
            .chosen
            .and_then(|number| seen.snapshot.of_head(one.head).get(number.checked_sub(1)?));
        let identity = candidate
            .and_then(|candidate| candidate.get(SELECTOR_KEY))
            .cloned()
            .unwrap_or(Value::Null);
        note.insert(
            one.head.head().to_string(),
            json!({
                "chosen": one.chosen,
                "confidence": one.confidence,
                (SELECTOR_KEY): identity,
            }),
        );
    }
    Value::Object(note)
}

/// The one press rule every press of a walk passes — the seat's own answer,
/// a second reader's, a fork's pick (t-6187): the seat's floor for a plain
/// control — or the act line its labels drew (`line`, t-9468) — and nine in
/// ten for one a press cannot take back ([`JevUse::permits_press_at`]). The
/// control's kind comes back for the row.
fn press_rule(
    policy: &JevUse,
    line: Option<u16>,
    seen: &Screen,
    mark: usize,
    confidence: f64,
) -> (bool, ControlKind) {
    let kind = control_kind(seen, mark);
    (policy.permits_press_at(confidence, kind, line), kind)
}

/// The kind of the control `mark` names on `seen`, read off the legend line
/// the question offered it under ([`kind_of`]).
fn control_kind(seen: &Screen, mark: usize) -> ControlKind {
    kind_of(&legend_of(seen, mark))
}

/// The control `mark` names on `seen`, as the legend named it — what a walk
/// writes among `pressed` once it has pressed it.
fn legend_of(seen: &Screen, mark: usize) -> String {
    seen.items
        .iter()
        .find(|item| item.get("mark").and_then(Value::as_u64) == u64::try_from(mark).ok())
        .and_then(zerocode_core::computer_use_protocol::marks::legend_line)
        .unwrap_or_else(|| format!("mark:{mark}"))
}

/// The look's own selector for the control `mark` names on `seen`.
fn selector_of(seen: &Screen, mark: usize) -> Option<String> {
    seen.items
        .iter()
        .find(|item| item.get("mark").and_then(Value::as_u64) == u64::try_from(mark).ok())
        .and_then(|item| item.get(SELECTOR_KEY))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Whether the control `mark` names on `seen` carries the screen elsewhere.
fn presses_a_link(seen: &Screen, mark: usize) -> bool {
    seen.items
        .iter()
        .find(|item| item.get("mark").and_then(Value::as_u64) == u64::try_from(mark).ok())
        .is_some_and(moves_the_page)
}

/// Write the walk's rows down, in both of the places a screen seat's row
/// belongs (§4, decision 3), through the one appender every Jev ledger is
/// written by:
///
/// - the walk's evidence folder, where the rows are part of what the run
///   renders — `dir` is that folder, and a walk recording no evidence has
///   none;
/// - `<config home>/jev/<seat>.jsonl`, the root every seat's ledger is judged
///   and counted under. That write goes through
///   [`crate::systemone::record_rows`], so a screen seat's rows are judged by
///   the writer that appended them — the road the orchestration seats already
///   take — and `zo jev summary` reads all eight seats under one root instead
///   of three of them scattered a session folder at a time.
///
/// A folder or a file that will not take them is said once on stderr and
/// never raised: a walk's record is not worth failing a walk that already
/// happened.
pub fn write_rows(
    seat: &JevUse,
    wire: &crate::systemone::Wire,
    dir: Option<&std::path::Path>,
    rows: &[Value],
    now_ms: i64,
) {
    if rows.is_empty() {
        return;
    }
    if let Some(dir) = dir {
        crate::systemone::append_rows(&dir.join(seat.ledger), rows);
    }
    if let Some(ledger) = crate::systemone::ledger_of(wire, seat) {
        crate::systemone::record_rows(seat, &ledger, rows, now_ms);
    }
}

/// A row's words with when it was asked, how long the judgment took, and what
/// it cost at the Jev door when the judge says — under the keys every Jev
/// ledger spells them with, so the one counter that reads every seat
/// (`zo jev summary`) counts this seat too.
///
/// The clock and the wall time were missing until 2026-09-20: the walk timed
/// its judgment and threw the number away, and a row with no `at` fell
/// outside every "today" and "7d" window, so the three screen seats read as
/// "never asked" on the settings card with 41 rows on disk.
fn stamped(mut said: Value, spent: Option<&Spent>, elapsed_ms: u64, now_ms: i64) -> Value {
    if let Some(fields) = said.as_object_mut() {
        fields.insert(AT.canonical.to_string(), json!(now_ms));
        fields.insert(ELAPSED_MS.canonical.to_string(), json!(elapsed_ms));
    }
    if let Some(spent) = spent {
        spent.stamp(&mut said);
    }
    said
}

/// Whether a report shows the walk getting past the step it had stopped at:
/// it finished, or it stopped somewhere later. Stopping at the same step
/// again is the obstacle still standing.
fn cleared_past(report: &Value, was: usize) -> bool {
    if report["done"] == json!(true) {
        return true;
    }
    report["stoppedAt"]
        .as_u64()
        .and_then(|at| usize::try_from(at).ok())
        .is_none_or(|at| at > was)
}

pub mod branch;
pub mod desk;
#[cfg(test)]
pub(crate) mod guard_fixtures;
pub mod live;
#[cfg(test)]
mod probe;
pub mod team;
pub mod value;
pub mod walk;

#[cfg(test)]
mod tests;
