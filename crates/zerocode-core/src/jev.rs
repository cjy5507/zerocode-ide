//! Jev — TypeSafe's System One — as this product asks it: the one table of
//! every place that asks (docs/design/jev-settings-20260917.md §2).
//!
//! Two programs ask. zo asks about a task it is routing, about the notes a
//! recall found and about a patch an edit has just written; the window asks
//! which control a walk should press next — on a
//! page or on the desktop — about a worker whose pane went quiet, about where
//! a worker's window belongs, about which agent a summons should start,
//! about which blocks of a page an agent's browser read should fold away
//! about whether a ring is worth interrupting the person for, and about
//! which of the screens a forked phone step led to is the one to keep.
//! Each keeps its own wire — the two Cargo workspaces carry different `reqwest` majors
//! (docs/design/jev-browser-action-20260917.md §1.4) — so what must not fork
//! lives here, in the one crate both already read:
//!
//! - the words a use's setting may hold ([`JevMode`]), and which of them each
//!   use offers — a use with no apply stage, like `stall`, reads its `on` as
//!   `off`;
//! - what each request carries that the product did not write itself, and the
//!   most of it one request may carry ([`Sent`]);
//! - the ledger each use appends its rows to.
//!
//! zo's parser, the window's parser and the settings pane's choices are all
//! read from [`JEV_USES`]; a mode word spelled anywhere else is a copy, and a
//! source contract holds that there are none.

use serde_json::Value;
use sha2::{Digest, Sha256};

pub mod challenger;
pub mod choice;
pub mod count;
pub mod door;
pub mod hedge;
pub mod memo;
pub mod noul;
pub mod promote;
pub mod recent;
pub mod shard;
pub mod summary;

/// The object zo's settings keep every Jev switch under.
pub const SMART_SETTINGS_KEY: &str = "smart";

/// The key under [`SMART_SETTINGS_KEY`] naming the model every Jev request
/// asks for (t-6187): the vendor's alias unless a person pinned a version.
///
/// One switch for every seat of both programs, because what it decides is
/// not a seat's: an alias answers with whatever version the vendor ships
/// under it, and a floor or a promotion window fitted to one version
/// silently measures the next. A person who wants the numbers to stay put
/// pins the version the rows say answered (`jev-1.13.0`). The door writes
/// it into every request it clears ([`door::may_send`]), so no seat can ask
/// under another.
pub const MODEL_SETTING: &str = "jevModel";

/// The model a request names when nobody pinned one: the vendor's alias —
/// the word the SDKs call by default, which named `jev-1.13.0` as the
/// answering version on every one of the 3,179 routing, recall and step
/// rows zo had recorded a version on by 2026-09-23. The window's wire and
/// zo's client spell the alias as this word; a contract holds zo's copy,
/// which cannot read this crate, to it.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// The model a settings document pins (`smart.jevModel`), or
/// [`DEFAULT_MODEL`] when it pins none. A value that is not a pin
/// ([`pinned_model`]) reads as no pin, as an unknown mode word reads as
/// `off`: a slip never sends a model nobody named.
#[must_use]
pub fn model_in(root: &Value) -> &str {
    pin_in(root).unwrap_or(DEFAULT_MODEL)
}

/// The pin a settings document holds, if it holds one ([`model_in`] without
/// the alias behind it) — what a screen reads to say whether the model it
/// shows was a person's choice.
#[must_use]
pub fn pin_in(root: &Value) -> Option<&str> {
    root.get(SMART_SETTINGS_KEY)
        .and_then(|smart| smart.get(MODEL_SETTING))
        .and_then(Value::as_str)
        .and_then(pinned_model)
}

/// `word` as a pin: trimmed, and one word — not empty, no space and no
/// control character in it, which is the shape of every model id the
/// vendor names. The settings writer refuses what this refuses, so a pin
/// the reader would ignore is never written.
#[must_use]
pub fn pinned_model(word: &str) -> Option<&str> {
    let word = word.trim();
    (!word.is_empty()
        && !word
            .chars()
            .any(|glyph| glyph.is_whitespace() || glyph.is_control()))
    .then_some(word)
}

/// Characters of a task a routing judgment reads — the chat probe's prompt and
/// Jev's state alike. The head of a brief is what bands it; the cap bounds
/// what each call costs and what leaves the machine.
pub const ROUTING_TASK_CHAR_CAP: usize = 2_000;

/// Characters of the request a recall's notes are judged against.
pub const RECALL_REQUEST_CHAR_CAP: usize = 1_000;

/// Notes one recall judgment is asked about. Recall renders a few and asks for
/// a few more behind them for its reminder block; past this the tail is not
/// worth a question.
pub const RECALL_NOTE_CAP: usize = 12;

/// Bytes of one note's summary a recall judgment carries.
pub const RECALL_SUMMARY_BYTE_CAP: usize = 320;

/// Controls one screen's question offers. The numbers themselves are capped
/// at 99 by the look; a choice with ninety-nine options is not a question
/// worth asking. One number, not one per surface: a page's controls and an
/// app's are numbered by the same marks table and read by the same question,
/// so two caps would be two answers to one question.
pub const SCREEN_CANDIDATE_CAP: usize = 12;

/// Characters of the goal a walk is given — the sentence that says what to
/// reach, which is the whole of what a goal walk knows about why it is
/// pressing.
///
/// The 436 sentences the recorded walks on this machine were handed
/// (`--text`, `--reason`, `--note`, `--name` in their step logs, 2026-09-18)
/// are at most 48 characters long (p50 9, p90 11). 400 holds every one of
/// them and a goal written in a sentence or two; past that a goal is a plan,
/// and a plan is a recipe with steps of its own, not one question's state.
pub const GOAL_CHAR_CAP: usize = 400;

/// Bytes of a quiet worker's screen one stall question carries: the newest
/// lines, so the composer, the mode line and the last words above them are
/// always among them. 8 KiB holds the whole visible screen of 307 of the 348
/// released worker screens the orchestration ledger archives on this machine
/// (2026-09-17; p50 4,193 B, p90 8,325 B), and the bottom 40 lines of nine in
/// ten (p90 5,045 B).
pub const STALL_SCREEN_BYTE_CAP: usize = 8 * 1024;

/// Bytes of a quiet worker's transcript tail one stall question carries — its
/// newest turns, one clamped card line each. 4 KiB holds the last 16 turns of
/// every one of the 391 worker transcripts on this machine (2026-09-17; max
/// 3,280 B) and the last 24 of nine in ten (p90 3,802 B).
pub const STALL_TRANSCRIPT_BYTE_CAP: usize = 4 * 1024;

/// What a byte cap leaves after the cut, so a clipped text is visibly one.
pub const CUT_MARK: &str = "…";

/// The grid an answer's numbers arrive on: the contract rounds every
/// probability and every score to two decimal places, so each one is a whole
/// number of these steps and carries up to half a step of rounding.
///
/// It is here, in the one crate both programs read, because it is a fact
/// about the WIRE rather than about any question — and because two copies of
/// a fact are two facts. zo's client re-exports it under its own name
/// (`api::SYSTEMONE_ANSWER_STEP`).
///
/// A caller that rebuilds one of an answer's numbers from the others — a
/// score from the spread it is the mean of, a sum from the parts — is
/// comparing two rounded numbers, and derives from this how far apart the
/// rounding alone can put them. Measured 2026-09-18 against `jev-1.13.0`: all
/// 1,200 numbers of 240 score answers were exact multiples of it, none finer.
pub const ANSWER_STEP: f64 = 0.01;

/// The most the wire's rounding can have moved any one number an answer
/// carries: half of [`ANSWER_STEP`].
pub const WIRE_ROUNDING: f64 = ANSWER_STEP / 2.0;

/// Characters of a summons' brief one agent-choice judgment reads.
///
/// The head of a brief is what bands it — routing's cap already says so — and
/// a summons says what it wants before it starts listing the constraints it
/// wants it done under. 1,200 characters holds 69.7% of the 532 task specs
/// this machine's orchestration ledger carries whole (2026-09-18; p50 662,
/// p75 1,415, p90 2,318, max 7,748) and the opening of the rest.
pub const SUMMON_BRIEF_CHAR_CAP: usize = 1_200;

/// Characters of a worker's brief one placement judgment reads.
///
/// The question is which of three rooms a worker belongs in, and what decides
/// it is what the worker was summoned FOR — the opening sentence of a summons
/// already separates "look at what is in front of me" from "sweep the backlog
/// at three in the morning". Past that the brief is the task's detail, which
/// says nothing more about where to put its window.
pub const PLACEMENT_BRIEF_CHAR_CAP: usize = 400;

/// The three rooms the window can actually put a worker in.
///
/// Spelled once, here, because a closed choice is only closed if the options
/// the question offers are the options the caller can carry out: a fourth
/// word would be an answer nothing could act on. `background` is a real
/// answer rather than a refusal — a worker nobody is watching is started and
/// left off the stage, which is what a scheduled run already does.
pub const PLACEMENT_OPTIONS: [&str; 3] = ["tab", "split", "background"];

/// The word a row's `routeUse` carries when the seat's answer is what the
/// product did — the one column a reader sweeps a ledger for to find out
/// whether a judgment ever changed anything.
///
/// Its two companions are read rather than written here: a recording row
/// carries [`JevMode::Shadow`]'s own word, because that is exactly what it
/// says, and a row that fell back to the product's own reader carries
/// [`ROUTE_USE_FALLBACK`]. Three words for one column, spelled once, because
/// the column is swept by scripts and by the promotion judge and a fourth
/// spelling of "fallback" is a row neither of them counts.
pub const ROUTE_USE_APPLIED: &str = "applied";

/// The word a row's `routeUse` carries when the seat was asked and the
/// product used its own reader anyway: no key, a refusal, a failure, or an
/// answer that did not clear the seat's floor.
pub const ROUTE_USE_FALLBACK: &str = "fallback";

/// What a person set a use to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JevMode {
    /// Ask nothing. The default, and what an unknown word reads as.
    #[default]
    Off,
    /// Ask and record; the product does what it would have done anyway.
    Shadow,
    /// Ask, and act on an answer that passed its checks.
    On,
    /// Ask and record until the use's own evidence promotes it (§4). Nothing
    /// promotes yet, so today this is [`Self::Shadow`] under another name.
    Auto,
}

impl JevMode {
    /// Every mode, in the order a setting offers them.
    pub const ALL: [Self; 4] = [Self::Off, Self::Shadow, Self::On, Self::Auto];

    /// The word a settings file holds.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shadow => "shadow",
            Self::On => "on",
            Self::Auto => "auto",
        }
    }

    /// Whether a use in this mode asks at all.
    #[must_use]
    pub const fn asks(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether a use in this mode acts on what it is told, read without a
    /// judgment to hand — `auto` reads as recording, which is where it starts
    /// and where it stays until [`crate::jev::promote`] raises it.
    #[must_use]
    pub const fn applies(self) -> bool {
        self.applies_with(false)
    }

    /// Whether a use acts, given what the judge last decided for it (§4).
    ///
    /// `raised` is the seat's standing, read back from the transitions its own
    /// ledger recorded ([`crate::jev::promote::stand_from`]). A person's `on`
    /// outranks it in both directions: they said act, and no window of rows
    /// takes that back; `off` and `shadow` are theirs the same way.
    #[must_use]
    pub const fn applies_with(self, raised: bool) -> bool {
        match self {
            Self::On => true,
            Self::Auto => raised,
            Self::Off | Self::Shadow => false,
        }
    }

    /// Whether evidence rather than a person decides when this mode acts.
    #[must_use]
    pub const fn automatic(self) -> bool {
        matches!(self, Self::Auto)
    }
}

/// The most of one piece of text a single request may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// Characters, cut on a character boundary.
    Chars(usize),
    /// UTF-8 bytes, cut on a character boundary with [`CUT_MARK`] after the cut.
    Bytes(usize),
    /// Elements of a list; the first ones stay.
    Items(usize),
    /// No cap has been measured for it yet. It is still cleared of anything
    /// that may carry a credential; it is not cut.
    Uncut,
}

/// One piece of what a use sends: where words the product did not write sit
/// in the request body, and how much of them one request carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    /// A JSON pointer into the request body (`/state/request`), where `*`
    /// stands for every element of an array or every value of an object.
    pub at: &'static str,
    pub cap: Cap,
}

/// One place this product asks Jev something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JevUse {
    /// The use's name — the word a ledger row, a refusal and a screen use.
    pub id: &'static str,
    /// The key under [`SMART_SETTINGS_KEY`] holding this use's mode.
    pub setting: &'static str,
    /// The modes this use offers, `off` first.
    pub modes: &'static [JevMode],
    /// Every place a request carries words the product did not write itself.
    pub sends: &'static [Sent],
    /// The ledger file this use appends one row per request to.
    pub ledger: &'static str,
    /// Whether `auto` may ever rise to acting for this use (§4). A use that
    /// never promotes stays record-only under `auto`.
    pub promotes: bool,
    /// The share of rows that must answer, in parts per thousand — read as a
    /// 95% Wilson lower bound over the judgment window
    /// ([`summary::JUDGED_EVERY_ROWS`]) — before `auto` rises to acting (§4).
    ///
    /// Per thousand and not a float: this table is compared and hashed whole,
    /// and a line a seat is promoted on should be a number two readers can
    /// agree on exactly.
    ///
    /// It sits on the row rather than in the judge because the line is the
    /// use's own: a seat whose answer a person waits on cannot afford the
    /// same miss rate as one that quietly reorders a list. A use that does
    /// not promote names no floor, and a contract holds the two together —
    /// a floor on a seat that never rises is a number nobody reads, and a
    /// rising seat with no floor is a promotion with nothing to pass.
    pub answer_floor_permille: Option<u16>,
    /// Minimum confidence for the seat to act on ONE answer alone — a screen
    /// press, or a block the browser read folds away — distinct from the
    /// answer-rate promotion floor. None means this seat acts on no single
    /// answer's confidence: it has no authority to press, or it applies
    /// whatever it answers.
    pub press_floor_permille: Option<u16>,
    /// The share of compared axes on which the judgment must bound above in
    /// naming what the reader it replaces named, per thousand, before `auto`
    /// rises with no labels to hand (§4) — a route-change budget. A use that
    /// does not promote names none; a contract holds it to the rise line.
    pub agreement_floor_permille: Option<u16>,
    /// The wall the apply stage waits for an answer, in milliseconds — the
    /// latency line a rising seat is judged against (§4), spelled once here
    /// so the stage that waits and the judge that reads the wait cannot
    /// disagree. A use that does not promote names none.
    pub apply_deadline_ms: Option<u64>,
    /// How many of the judgment window's rows may have missed and the seat
    /// still clear its answer floor — what widens the window
    /// ([`summary::rows_that_can_clear_forgiving`]).
    ///
    /// It is the use's own because a miss is: at a seat whose answer the
    /// coordinator or the graph was going to stand in for anyway, one timeout
    /// is the wire having a bad minute and costs nothing at all; at a seat
    /// whose miss holds a turn for its whole wall, a seat that misses is one
    /// the person feels. The unforgiving width makes every floor read "the
    /// last n were all answers", so without this column the three seats with
    /// a full window all sat one timeout under their line (2026-09-22).
    /// A use that does not promote forgives nothing, because nothing is
    /// judged.
    pub window_forgives: Option<usize>,
    /// The sample floor at which the agreement line may speak: until this
    /// many marks are in hand the seat is held at `too_few_compared`,
    /// whatever kind of mark it writes ([`AgreementKind`]). A thinner
    /// sample never promotes (t-6155 F1).
    pub agreement_rows_wanted: Option<usize>,
    /// Where the seat's `agreed` marks come from: a second reader, or a
    /// later fact.
    pub agreement_kind: AgreementKind,
}

/// What a seat forgives whose miss the product was going to cover anyway: one
/// row of the judgment window ([`JevUse::window_forgives`]).
///
/// One and not three, which is what [`crate::jev::promote::FALLBACKS_THAT_END_IT`]
/// reads as the wire, the key or the model: this is the other reading of the
/// same fact, a single bad minute, and a window that forgave three would let a
/// seat rise on an afternoon the wire spent failing. One is also what the
/// measurement asked for — the three seats with a full window on 2026-09-22
/// were each holding exactly one timeout.
pub const FORGIVES_A_BAD_MINUTE: usize = 1;

/// What a seat forgives whose miss holds a turn: nothing.
///
/// The seats at the 950‰ line. Their window is already 73 rows wide, forgiving
/// one takes it to 110, and the thing being bought is the right to miss — at
/// the one seat where a miss is a turn held for its whole wall before the
/// probe runs anyway. A seat that cannot answer 73 times running is not one to
/// hand a turn's route to.
pub const FORGIVES_NOTHING: usize = 0;

/// What a seat with a reader to be compared against must show before its
/// route-change budget binds: a judgment window's worth
/// ([`JevUse::agreement_rows_wanted`]).
pub const A_WINDOW_OF_COMPARISONS: usize = summary::JUDGED_EVERY_ROWS;

/// Where a seat's `agreed` marks come from — a second reader answering the
/// same question, or a later fact grading the answer. A description of the
/// mark for a reader and a screen; the judge holds both kinds to the same
/// sample floor and the same Wilson line ([`crate::jev::promote`]), so a
/// hindsight seat with no marks yet is held exactly as a comparison seat
/// with none (t-6155 F1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgreementKind {
    /// A replacement reader needs comparison evidence before it may act.
    Comparison,
    /// Later outcome labels grade an otherwise eligible seat once populated.
    Hindsight,
}

/// The routing seat's route-change budget: four compared axes in five must
/// agree with the chat probe, as a 95% lower bound.
///
/// A policy line, not a calibrated accuracy claim, like
/// [`SCREEN_PRESS_FLOOR_PERMILLE`]. Where it sits: on this machine's 25
/// answered rows the judgment agreed with the probe on 53% of all axes and
/// 77% of the axes it was at least half sure of (2026-09-20) — under the
/// line, which is the point: a seat that disagrees with the router it would
/// replace on every other turn is not one that should replace it unasked.
/// Twenty comparisons that all agree bound at 0.839, so the line is one a
/// single judgment window can clear.
pub const ROUTE_AGREEMENT_FLOOR_PERMILLE: u16 = 800;

/// Initial conservative screen-press floor, above the observed wrong choice
/// at confidence 0.29. This is a policy line, not a calibrated accuracy claim.
pub const SCREEN_PRESS_FLOOR_PERMILLE: u16 = 500;

/// What either of the two guards a screen question carries beside its
/// choice must reach, per thousand, before a seat that is pressing presses
/// nothing and steps back to the person (t-6187,
/// `crate::screen_action::Guard`): seven in ten that the screen's own text
/// tells an assistant what to do, or that the screen is a wall — a sign-in,
/// a captcha, an error dialog — in front of the page the goal expects.
///
/// A policy line, not a calibrated accuracy claim, like
/// [`SCREEN_PRESS_FLOOR_PERMILLE`]. A Noul near a half says yes and no are
/// about as likely, so the line sits where yes clearly leads — the lean the
/// skill seat's relevance floor is drawn at
/// ([`SKILL_RELEVANCE_FLOOR_PERMILLE`]), written as this seat's own number
/// because two lines that coincide are still two policies. The two guards
/// share it because they are one judgment's two halves: this is not a
/// screen to press on unasked. The guards cost the request nothing but
/// their own two lines — the state is charged once.
pub const SCREEN_INSTRUCTED_FLOOR_PERMILLE: u16 = 700;

/// What a seat's answer must reach, per thousand, before it presses a
/// control that cannot be taken back — one that pays, moves money, deletes,
/// sends, submits or settles something ([`crate::guarded::kind_of`], t-6187):
/// nine in ten, where a plain control asks the seat's own press floor.
///
/// A policy line, not a calibrated accuracy claim, like
/// [`SCREEN_PRESS_FLOOR_PERMILLE`] — the line the vendor's own guide draws
/// for destructive actions (reads at a half, destructive at nine in ten), and
/// the one this table already asks of a seat's answers before it may act at
/// all ([`SCREEN_ANSWER_FLOOR_PERMILLE`]). A plain press the walk gets wrong
/// costs one more press; a delete or a payment it gets wrong is the person's
/// to undo, if it can be undone. Under this line the walk does what it does
/// with any answer under its floor: the second reader, when it was handed
/// one, or the person.
pub const SCREEN_DESTRUCTIVE_PRESS_FLOOR_PERMILLE: u16 = 900;

/// What a screen seat's answers must bound above before `auto` rises to
/// pressing (docs/design/jev-seats-accuracy-wave-20260921.md §4): nine in ten.
///
/// The orchestration seats' number, arrived at from the walk's own side: a
/// question that does not come back costs the walk nothing it was not already
/// going to pay — the walk ends where a walk without a judgment ends, at the
/// step that stopped — while every answer that does come back moves a real
/// pointer on a real screen. Written here rather than read from
/// [`ORCHESTRATION_ANSWER_FLOOR_PERMILLE`] for the reason that constant is
/// itself not [`ROUTE_AGREEMENT_FLOOR_PERMILLE`]: two lines that happen to
/// coincide are still two policies, and a screen that learns to answer at a
/// different rate than a coordinator's sweep should move one of them alone.
pub const SCREEN_ANSWER_FLOOR_PERMILLE: u16 = 900;

/// The screen seats' route-change budget (§4): four presses in five must be
/// ones the walk went on to confirm.
///
/// What a screen seat has in place of a probe to compare against is hindsight
/// — the walk's own end. A press the walk reached its goal after is a press
/// the judgment got right; one the walk was still stuck after is not
/// ([`summary::AGREED`], stamped a walk at a time). That is stronger evidence
/// than the routing seat's agreement, which says only that two readers said
/// the same thing, and it is held to the same line: a reader that leaves a
/// walk stuck every fifth press is not one to hand the mouse to unasked.
pub const SCREEN_AGREEMENT_FLOOR_PERMILLE: u16 = 800;

/// The wall one screen question may hold a walk, in milliseconds — the number
/// the walk itself waits (`computer_use::errand::ACTION_DEADLINE`, which reads
/// it from here) and the latency line the judge holds a rising seat to.
///
/// It is the question's deadline and not the walk's speed target. A screen
/// question's floor is the judgment server's own answer time — 280 ms at the
/// median once the client stopped opening a new connection for each one
/// (§2.1, 2026-09-21) — so the 131 ms a person's own step costs would be a
/// line no seat could ever clear. That number is the walk's goal and lives in
/// the measurement table; this one is the wall past which an answer is slower
/// than the fallback it would replace.
pub const SCREEN_APPLY_DEADLINE_MS: u64 = 1_500;

/// The share of memo answers that must name what a fresh answer named, per
/// thousand, before the judgment cache's `auto` answers a walk from the memo
/// alone ([`JUDGMENT_CACHE`]): nine in ten.
///
/// The label is a comparison the seat makes itself under `shadow`: the memo
/// held an answer for these exact bytes, the wire was asked anyway, and the
/// two chose the same number or did not. A cache whose answers disagree with
/// a fresh judgment more than one time in ten is a stale cache, and the walk
/// it would be answering is a real pointer on a real screen. Held above the
/// screen seats' own line ([`SCREEN_AGREEMENT_FLOOR_PERMILLE`]) because what
/// is compared here is the same reader against itself — the notify seat's
/// replay put Jev's repeatability on identical inputs at 96.5% (t-6043), so
/// nine in ten is a line a working memo clears with room.
pub const JUDGMENT_CACHE_AGREEMENT_FLOOR_PERMILLE: u16 = 900;

/// The share of memo lookups that must answer, per thousand, before the
/// judgment cache's `auto` rises — the screen seats' own line, because a memo
/// hit stands in for a screen question and is held to what that question is
/// held to. A hit that no longer reads (a remembered body the question's own
/// rules refuse) is the miss this floor counts.
pub const JUDGMENT_CACHE_ANSWER_FLOOR_PERMILLE: u16 = SCREEN_ANSWER_FLOOR_PERMILLE;

/// The wall one memo lookup may hold a walk, in milliseconds — the latency
/// line the judge holds the cache seat to. A memo is one local file of at
/// most [`memo::MEMO_ROWS_CAP`] rows read whole (measured 2026-09-22: 2,000
/// rows of ~400 bytes read and scanned in under 5 ms on this machine); a
/// lookup past this wall is a memo that has stopped being cheaper than the
/// question it answers for.
pub const JUDGMENT_MEMO_DEADLINE_MS: u64 = 50;

/// zo's routing judgment: a task's complexity, risk and intent beside the
/// chat probe's (docs/design/jev-decision-shadow-20260917.md).
///
/// The `agreed` rule (t-5806): a routing judgment agreed when the turn it
/// routed STOOD — no quota wall, refusal fallback or overload demotion moved
/// the wire to another model, and the person did not name one themselves —
/// and disagreed when any of those unseated the route before the turn ended.
/// The label is one row per turn, keyed by the turn's attempt, written by
/// the host when the turn ends (`decision_shadow::note_route_followed`); a
/// turn the person cancelled is not judged, and a turn the seat was never
/// asked about leaves no label. The judge counts it beside the probe's axis
/// agreement, one comparison per label row.
pub const ROUTING: JevUse = JevUse {
    id: "routing",
    setting: "decisionShadow",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[Sent {
        at: "/state",
        cap: Cap::Chars(ROUTING_TASK_CHAR_CAP),
    }],
    ledger: "decision-shadow.jsonl",
    promotes: true,
    answer_floor_permille: Some(950),
    press_floor_permille: None,
    agreement_floor_permille: Some(ROUTE_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(ROUTING_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_NOTHING),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// The wall zo's routing waits for a judgment when the seat acts: the batch
/// pays it at most once, every unique task running concurrently, and a task
/// past it falls back to the chat probe (`decision_shadow`).
pub const ROUTING_APPLY_DEADLINE_MS: u64 = 1_500;

/// What an orchestration seat's answers must bound above to rise (§4):
/// nine in ten. Lower than routing's line because a seat that does not answer
/// costs nothing — the coordinator's own choice stands, as it did before the
/// seat existed — where a routing miss holds a turn for its whole wall.
pub const ORCHESTRATION_ANSWER_FLOOR_PERMILLE: u16 = 900;

/// The orchestration seats' agreement line (§4): four in five of the
/// judgments must have named what the coordinator, or the window's own rule,
/// did — the summon's pinned agent, the room the layout rule chose, what
/// followed a silence. The same budget as routing's, for the same reason: a
/// seat that would have overruled the person every other time is not one to
/// hand the decision to unasked.
pub const ORCHESTRATION_AGREEMENT_FLOOR_PERMILLE: u16 = 800;

/// The walls the window waits for the orchestration seats' answers — read
/// by the seats that wait (`stall_cause`, `worker_room`, `summon_choice`)
/// and by the judge, from here.
pub const STALL_APPLY_DEADLINE_MS: u64 = 30_000;
pub const PLACEMENT_APPLY_DEADLINE_MS: u64 = 2_000;
pub const SUMMON_APPLY_DEADLINE_MS: u64 = 10_000;

/// How long after a placement answer a person's move of the worker's pane
/// still counts against that answer, in milliseconds (t-5806).
///
/// The placement seat's label is what the person did with the window in the
/// minutes after it appeared: a pane they moved to another room said the
/// seat chose wrong, and a pane they left where it was said it chose right
/// — or that nobody was looking, which the label cannot tell apart and does
/// not pretend to. Five minutes is a policy line, not a measured one: long
/// enough for somebody who was typing elsewhere to turn to the new pane,
/// short enough that a rearrangement an hour later — a different piece of
/// work — is not charged to a decision it had nothing to do with. The beat
/// reads it to write the `agreed` row once the window has passed, and the
/// window's move recorder reads it to refuse a move that came too late.
pub const PLACEMENT_LABEL_WINDOW_MS: i64 = 5 * 60 * 1_000;

/// What the recall seat's answers must bound above before `auto` rises to
/// ordering what a turn reads (§4): nine in ten — the skill seat's line, read
/// from there rather than respelled (t-5806, "모든 승격"), because the two
/// seats are the same kind: a ranking a reader falls back from at no cost
/// (recall's own order stands when the judgment does not come back, as the
/// word match stands behind a skill search). One name of its own, so a
/// re-measurement moves this seat's line alone.
pub const RECALL_ANSWER_FLOOR_PERMILLE: u16 = SKILL_ANSWER_FLOOR_PERMILLE;

/// The recall seat's route-change budget (§4): four readings in five must
/// have put the note the turn then read or cited first — the seat's own
/// hindsight mark (`rerank_shadow::note_recall_read`), which is what it has
/// in place of a probe, as the skill seat has. The skill seat's budget, read
/// from there for the reason its answer floor is.
pub const RECALL_AGREEMENT_FLOOR_PERMILLE: u16 = SKILL_AGREEMENT_FLOOR_PERMILLE;

/// zo's recall rerank: how much each note a recall found helps with the
/// request (docs/design/typesafe-judgment-expansion-20260917.md).
///
/// `on` is the apply stage: the judgment's order, after the vault's graph has
/// had its say, is the order the turn reads. The 887 answered readings this
/// machine had recorded by 2026-09-17 say the judgment moves something on
/// nearly every recall (a median of 7 notes reordered, the first note changed
/// in 73% of them), which is why the seat stayed a person's choice until it
/// had a mark to rise on. It has one now (t-5806): `auto` rises when the
/// window's readings answered inside the routing seat's wall and the turns
/// then read what the judgment put first four times in five, on the lines
/// [`RECALL_ANSWER_FLOOR_PERMILLE`] and [`RECALL_AGREEMENT_FLOOR_PERMILLE`];
/// the judge writes the rise in the seat's own ledger, and the seat reads it
/// back per recall (`rerank_shadow::settle`).
/// Hindsight labels enter that decision only at [`A_WINDOW_OF_COMPARISONS`];
/// below the sample floor, the answer, latency and schema lines decide alone.
///
/// The apply also LEAVES OUT the notes the judgment put on its bottom level —
/// *nothing in it bears on the request* — which the same ledger says is 24.5%
/// of what a turn was handed, and on 5 of those 74 recalls was all of it. The
/// rule that survives both is the graph's, not the judgment's: a page the vault
/// marked superseded or contradicted is never dropped, and a judgment that
/// cannot account for every page recall admitted is one recall's own order
/// outlives (`runtime::memory::rerank`).
///
/// The `agreed` rule (t-5806): a rerank agreed when the note it put FIRST
/// was actually read or cited before the turn ended — a `Read` of the note's
/// own path, or a `[[slug]]` citation in what the assistant wrote, both
/// inside the same turn and after the order was handed over; nothing a
/// later turn reads counts. The label row also carries `rank`, the place in
/// the judgment's order of the first note the turn touched (absent when it
/// touched none), and `applied`, so an applied order and a recorded one can
/// be compared on the same mark. Written by the host at the turn's end
/// (`rerank_shadow::note_recall_read`), and read by the judge as this seat's
/// agreement — one comparison per label row.
pub const RECALL: JevUse = JevUse {
    id: "recall",
    setting: "rerankShadow",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/request",
            cap: Cap::Chars(RECALL_REQUEST_CHAR_CAP),
        },
        Sent {
            at: "/state/notes",
            cap: Cap::Items(RECALL_NOTE_CAP),
        },
        Sent {
            at: "/state/notes/*/name",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/notes/*/summary",
            cap: Cap::Bytes(RECALL_SUMMARY_BYTE_CAP),
        },
    ],
    ledger: "rerank-shadow.jsonl",
    promotes: true,
    answer_floor_permille: Some(RECALL_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(RECALL_AGREEMENT_FLOOR_PERMILLE),
    // The routing seat's active wall: the apply road already waits inside
    // it (`rerank_shadow::RERANK_APPLY_DEADLINE`), so the judge times the
    // seat against the wall the stage actually holds.
    apply_deadline_ms: Some(ROUTING_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Hindsight,
};

/// What every screen question carries, whichever surface answered it
/// (`crate::screen_action`): the goal, the numbered controls, and the legend
/// lines those controls are described by. Spelled once, because the two rows
/// below ask the SAME question — what differs between them is the address
/// (§`BROWSER`/`DESKTOP`) and nothing else, and a second copy of this list is
/// how the two would drift.
const SCREEN_SENDS: [Sent; 4] = [
    Sent {
        at: "/state/goal",
        cap: Cap::Chars(GOAL_CHAR_CAP),
    },
    Sent {
        at: "/state/candidates",
        cap: Cap::Items(SCREEN_CANDIDATE_CAP),
    },
    Sent {
        at: "/state/candidates/*",
        cap: Cap::Uncut,
    },
    Sent {
        at: "/questions/*/criteria/*",
        cap: Cap::Uncut,
    },
];

/// What a browser walk sends: its own address, then [`SCREEN_SENDS`] — named
/// by index rather than written out again, so the shared list stays one list.
/// A stopped walk fills the refusal; a goal walk leaves it out, and a key a
/// request does not carry sends nothing.
const BROWSER_SENDS: [Sent; 7] = [
    Sent {
        at: "/state/refusal",
        cap: Cap::Uncut,
    },
    Sent {
        at: "/state/where/host",
        cap: Cap::Uncut,
    },
    Sent {
        at: "/state/where/path",
        cap: Cap::Uncut,
    },
    SCREEN_SENDS[0],
    SCREEN_SENDS[1],
    SCREEN_SENDS[2],
    SCREEN_SENDS[3],
];

/// What a desktop walk sends: the app it is in and that app's window title,
/// then [`SCREEN_SENDS`]. No path, because a desktop has none, and no
/// refusal, because nothing failed.
const DESKTOP_SENDS: [Sent; 6] = [
    Sent {
        at: "/state/where/app",
        cap: Cap::Uncut,
    },
    Sent {
        at: "/state/where/window",
        cap: Cap::Uncut,
    },
    SCREEN_SENDS[0],
    SCREEN_SENDS[1],
    SCREEN_SENDS[2],
    SCREEN_SENDS[3],
];

/// The window's browser walk: which numbered control on a page to press
/// (docs/design/jev-browser-action-20260917.md). It answers two errands — a
/// recorded walk that stopped, and a goal named in a person's words — and
/// both of them press.
///
/// `auto` rises here (docs/design/jev-seats-accuracy-wave-20260921.md §4).
/// It did not until then, and the sentence this doc used to carry — that a
/// press is always a person's to allow — read as a safety rule when it was
/// really an absence: nothing had been built that could tell a seat pressing
/// well from one pressing badly, so every screen seat sat at `shadow` under a
/// name that promised otherwise. What tells them apart now is the walk's own
/// end: a press the walk reached its goal after was the right number, and one
/// it was still stuck after was not ([`SCREEN_AGREEMENT_FLOOR_PERMILLE`]).
/// A person's `off`, `shadow` and `on` still outrank the judge in both
/// directions; `auto` is the mode that says "decide on the evidence", and it
/// now does.
pub const BROWSER: JevUse = JevUse {
    id: "browser",
    setting: "browserAction",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &BROWSER_SENDS,
    ledger: "browser-action.jsonl",
    promotes: true,
    answer_floor_permille: Some(SCREEN_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: Some(SCREEN_PRESS_FLOOR_PERMILLE),
    agreement_floor_permille: Some(SCREEN_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(SCREEN_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// The window's desktop walk: which numbered control of an app's
/// accessibility tree to press (`computer_use::errand`, t-4774).
///
/// A row of its own rather than a wider browser row, for three reasons, each
/// of which would be a lie told to a person if the two shared one switch:
///
/// 1. **What is sent is not the same thing.** A page's host and path name a
///    place the person deliberately opened in a pane kept for automation. An
///    app's name and window title name what they have open on their own
///    desktop right now. Consenting to one is not consenting to the other.
/// 2. **The switch would widen itself.** Somebody who turned browser recovery
///    on months ago would, on the day this landed, have started sending their
///    desktop — a safety line widened by an upgrade rather than by a person.
/// 3. **The evidence would be one pile.** The two surfaces' accuracy is the
///    open question this seat exists to answer, and a ledger that mixes them
///    cannot answer it for either.
///
/// `auto` rises here on this seat's own evidence, as [`BROWSER`]'s does, and
/// on nothing the other surface earned — which is reason 3 above holding at
/// the moment it matters.
pub const DESKTOP: JevUse = JevUse {
    id: "desktop",
    setting: "desktopAction",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &DESKTOP_SENDS,
    ledger: "desktop-action.jsonl",
    promotes: true,
    answer_floor_permille: Some(SCREEN_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: Some(SCREEN_PRESS_FLOOR_PERMILLE),
    agreement_floor_permille: Some(SCREEN_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(SCREEN_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// A mobile screen is a separate consent and evidence surface. Existing
/// browser/desktop settings never enable it, and its `auto` rises — as
/// [`BROWSER`]'s and [`DESKTOP`]'s do — only on the presses this surface's own
/// walks confirmed.
pub const EMULATOR: JevUse = JevUse {
    id: "emulator",
    setting: "emulatorAction",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/where/platform",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/where/device",
            cap: Cap::Uncut,
        },
        SCREEN_SENDS[0],
        SCREEN_SENDS[1],
        SCREEN_SENDS[2],
        SCREEN_SENDS[3],
    ],
    ledger: "emulator-action.jsonl",
    promotes: true,
    answer_floor_permille: Some(SCREEN_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: Some(SCREEN_PRESS_FLOOR_PERMILLE),
    agreement_floor_permille: Some(SCREEN_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(SCREEN_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// The window's stall sweep: why a quiet worker stopped when the measured
/// marker table cannot say (`crate::stall_cause`, t-4538). Nothing acts on
/// the answer yet — it is a row beside what the coordinator then did, and
/// those rows are the labels a later stage would promote on — so the use
/// offers no mode that applies, and `auto` records.
pub const STALL: JevUse = JevUse {
    id: "stall",
    setting: "stallCause",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/screen",
            cap: Cap::Bytes(STALL_SCREEN_BYTE_CAP),
        },
        Sent {
            at: "/state/transcript",
            cap: Cap::Bytes(STALL_TRANSCRIPT_BYTE_CAP),
        },
    ],
    ledger: "stall-cause.jsonl",
    promotes: true,
    answer_floor_permille: Some(ORCHESTRATION_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(ORCHESTRATION_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(STALL_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// What the placement seat's answers must bound above before `auto` rises to
/// choosing the room (§4): four in five, and not the nine in ten the other
/// orchestration seats are held to.
///
/// The line belongs where the negatives are, not where the ceiling is (the
/// vault's "a judgment seat pays where its negatives are few, not where its
/// ceiling is high"): this seat has the cheapest negative in the table on
/// both sides. A miss costs nothing — the window's own layout rule places the
/// pane, exactly as it did before the seat existed, inside a 2 s wall. A
/// WRONG answer costs one drag, and that drag is the seat's own mark
/// ([`PLACEMENT_LABEL_WINDOW_MS`]), so the seat is contradicted by the very
/// move that undoes it. Nothing is lost, nothing is spent, and no other agent
/// runs. [`ORCHESTRATION_ANSWER_FLOOR_PERMILLE`]'s own reason for sitting
/// under routing's — "a seat that does not answer costs nothing" — reads one
/// step further here, and this is where it lands.
///
/// Four in five is the table's existing word for a budget whose negative is
/// cheap ([`ROUTE_AGREEMENT_FLOOR_PERMILLE`] and the two beside it), and it
/// takes the window this seat is judged on from the 53 rows a forgiving 900‰
/// line asks for down to 25 — which its ledger, 38 rows on 2026-09-22,
/// already holds.
pub const PLACEMENT_ANSWER_FLOOR_PERMILLE: u16 = 800;

/// The window's worker placement: which of [`PLACEMENT_OPTIONS`] a worker it
/// just started belongs in.
/// Its hindsight marks bind only at [`A_WINDOW_OF_COMPARISONS`]. Before then,
/// even a mixed thin sample leaves the other eligibility lines to decide.
///
/// The measured rule (`tilePlacement`, `ui/shell-term.js`) answers a
/// different question well — given that a pane IS being cut, which way and
/// whether there is room. What it cannot read is whether this worker is one
/// somebody wants beside what they are already looking at, or one that should
/// not take the stage at all; that depends on why it was summoned, and the
/// layout does not know why.
///
/// Recording only, and deliberately so: the seat offers no mode that applies
/// (t-4781). Two things are missing before it could, and the second is the
/// one that matters.
///
/// The first is rooms. The window puts EVERY ledger worker in the same one —
/// its own unfocused tab, `seatLedgerManagedTerm` — and a worker is the one
/// pane road that never consults the layout's rule at all: the `split-window`
/// a summons plans is the team table's bookkeeping, and `worker-start
/// --horizontal` names a direction the birth event
/// (`term:worker`) does not carry. So `split` and `background` are words
/// nothing would carry out today; the surfaces exist (`tileTermPane`,
/// `detachedAgents`) and nothing has been pointed at them.
///
/// The second is evidence, and there is none. What the window does now is
/// decided by the checkout and the seat — facts it is certain of — and no row
/// on this machine says a judgment would put a worker anywhere better. The
/// question this use asks can be recorded and read
/// (`crate::worker_placement`, `cmd::worker_room`); nothing calls it, and
/// nothing should until those rows exist.
///
/// The `agreed` rule (t-5806): the answer agreed when the person left the
/// worker's pane in the room the seat named for [`PLACEMENT_LABEL_WINDOW_MS`]
/// after the answer, and disagreed when they moved it to another room inside
/// that window — closed a tiled pane to the background, dragged its tab out
/// beside something else, brought a parked worker back to a tab. A move
/// that keeps the room (a tab dragged to another group) is still written
/// down, as a move the answer survived. The window's surface reports the
/// move through one door (`note_worker_room_change`) and the beat writes the
/// quiet case once the window has passed (`worker_room::label_rooms`); a
/// pane placed before this window process started is not labeled, because
/// nothing saw what became of it. The row it labels is named by its
/// `label` key, the worker id the answered row carries as `placement`.
pub const PLACEMENT: JevUse = JevUse {
    id: "placement",
    setting: "workerPlacement",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[Sent {
        at: "/state/brief",
        cap: Cap::Chars(PLACEMENT_BRIEF_CHAR_CAP),
    }],
    ledger: "worker-placement.jsonl",
    promotes: true,
    answer_floor_permille: Some(PLACEMENT_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(ORCHESTRATION_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(PLACEMENT_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Hindsight,
};

/// The summons' agent choice: which of the agents this window could start
/// right now should carry the work a `worker-start` describes
/// (`crate::summon_choice`, t-4711).
///
/// Every other launch fact a summons carries is checked against a measured
/// table before anything is minted — the agent exists, its CLI takes the
/// dial, its provider has room. The one fact nothing checks is the one a
/// person actually asked for: WHICH agent, on which model, at which effort.
/// The coordinator writes those three words by hand, and on run-4275 all
/// twenty-two workers were decided that way.
///
/// The set to choose from is not computed here and is not computed twice: it
/// is the set the quota gate already builds to name the agents still holding
/// room when it refuses one (`orchestration::summonable`), so an agent at its
/// wall cannot be offered as an answer nobody could carry out.
///
/// Recording only, and the coordinator's own words keep summoning every
/// worker. `auto` records too: what a later stage would promote on is the row
/// beside what the summons actually did and what became of that worker, and
/// no such judge exists yet.
///
/// The `agreed` rule (t-5873). The answer agreed when it named the agent this
/// summons actually landed on — `--agent` as the quota gate left it, which is
/// what the pane really ran. The label is written once, at the summons, and
/// never revised: what a coordinator does NEXT is another summons with its
/// own row.
///
/// Three consequences worth saying out loud, because each looked like a
/// mislabel until it was read:
///
/// - A coordinator who tries a second agent on the same work files a
///   `--retry-of` summons, and that row is labeled against ITS own agent, not
///   against the one it replaces. So a judgment that named `codex` for a task
///   a person then re-ran on `claude` disagrees with the retry and agreed
///   with nothing — correctly: the coordinator's choice for THAT summons was
///   `claude`. The state says a retry is what it is (`replaces`), so the
///   judgment is asked under the same fact the coordinator had.
/// - `--agent auto` has no coordinator's word to agree with — the seat's
///   answer WAS the word — so the row carries no mark at all
///   (`SummonShadow::auto`). Eighteen of this machine's fifty-six rows are
///   such summonses and none of them is in the agreement.
/// - A summons whose own agent was never offered is not a comparison either;
///   the row says [`crate::summon_choice::NOT_OFFERED`] under
///   [`crate::summon_choice::NOT_COMPARED_KEY`] instead of a mark.
///
/// What the rule does NOT count as disagreement: a person taking the pane
/// over (`taken_over`), the worker being stopped, or the work failing. Those
/// are facts about what happened after, and this seat is judged on whether
/// it picks what the coordinator picks — not on whether the coordinator was
/// right.
///
/// Why this seat has no confidence axis (measured 2026-09-22, t-5875 with
/// t-5873's finding). Its marks split hard by how sure the judgment was: over
/// this machine's 54 comparisons, the rows at confidence 0.5 and over agreed
/// 14 of 20 (70.0%) and the rows under it 4 of 34 (11.8%). That is real
/// signal, and it is still not a promotion. The line is read as a bound, not
/// a share, and cutting to the confident rows shrinks the sample faster than
/// it lifts the share: the 95% lower bound goes from 222‰ over 54 rows to
/// 481‰ over 20, against a floor of [`ORCHESTRATION_AGREEMENT_FLOOR_PERMILLE`]
/// — 319 short. At 0.6 and over it is 12 of 15, bounding at 548‰. And no
/// width fixes it: a bound converges on the true share from below, so a
/// subset that agrees 70% of the time can never bound above 80% however many
/// rows it gathers. This seat has to be RIGHTER, not filtered.
///
/// A confidence axis would also have to be a matched pair to mean anything:
/// this seat applies whatever it answers — it names no
/// [`JevUse::press_floor_permille`] and `summon_choice` reads no confidence
/// before acting — so an agreement read over the confident rows alone would
/// be a bound on a population the seat does not act on. The axis belongs here
/// the day the apply stage gates on the same number, and not before.
pub const SUMMON: JevUse = JevUse {
    id: "summon",
    setting: "summonChoice",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[Sent {
        at: "/state/brief",
        cap: Cap::Chars(SUMMON_BRIEF_CHAR_CAP),
    }],
    ledger: "summon-choice.jsonl",
    promotes: true,
    answer_floor_permille: Some(ORCHESTRATION_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(ORCHESTRATION_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(SUMMON_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// Characters of the repeated tool call one step-effort question carries —
/// the call's name and target as the board draws them, one card line
/// (`crate::transcript::clamp`), which is what the question is about: the
/// thing the worker keeps doing.
pub const STEP_EFFORT_REPEATED_CHAR_CAP: usize = 240;

/// The wall the beat waits for a step-effort answer, in milliseconds.
///
/// A move lands between two turns, and the next turn can start the moment
/// the coordinator's pointer is typed — so the seat is held to the summons'
/// wall (ten seconds keeps routing's slowest measured answer, 6,798 ms) and
/// not the stall sweep's thirty: past it the rule's own move stands in for
/// the answer, exactly as routing falls back to its probe.
pub const STEP_EFFORT_APPLY_DEADLINE_MS: u64 = SUMMON_APPLY_DEADLINE_MS;

/// The window's between-turn effort move for a worker whose CLI takes one
/// (t-5637, `crate::step_effort`): which way a worker's effort should go
/// before its next turn — up a rung when its last turn repeated one tool
/// call or failed tools in a row, down when the turn was routine and the
/// effort stands above the summons' word, or hold.
///
/// The idea is vechen's Codex+Jev experiment (the vault's
/// `a-reasoning-effort-governor-changes-effort-per-step…`): judge the effort
/// per step, not once per turn. zo does it per request from inside; the
/// window can only reach a Claude Code or Codex worker at its composer, so
/// its unit is the turn, and its door is the row's `moves.effort` — Claude
/// Code's `/effort` picker, driven one rung a time for this session only.
/// Codex takes nothing typed (its `/model low` is a message to the model),
/// so a Codex row records the move it would have made under `relaunch`.
///
/// What is sent is the repeated call's card line and numbers the product
/// wrote itself. Under `on`, or an `auto` its own evidence raised, the seat's
/// answer is what moves — and the rule's own move when the answer does not
/// arrive within [`STEP_EFFORT_APPLY_DEADLINE_MS`]; a person's `shadow`
/// records both beside each other and types nothing. The label is what the
/// next turn did: progressed, or the same stuck shape again.
pub const STEP_EFFORT: JevUse = JevUse {
    id: "effort",
    setting: "stepEffort",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[Sent {
        at: "/state/repeated",
        cap: Cap::Chars(STEP_EFFORT_REPEATED_CHAR_CAP),
    }],
    ledger: "step-effort.jsonl",
    promotes: true,
    answer_floor_permille: Some(ORCHESTRATION_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(ORCHESTRATION_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(STEP_EFFORT_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// Characters of the task a skill ranking reads — what the turn is about, in
/// the words the model hands the search.
///
/// The routing seat's cap and not a second number: the two questions read the
/// same kind of text for the same reason — the head of a task is what bands
/// it — and a second cap on the same words would be a second answer to how
/// much of a person's work leaves the machine.
pub const SKILL_TASK_CHAR_CAP: usize = ROUTING_TASK_CHAR_CAP;

/// Characters of one skill's description a ranking question carries.
///
/// A skill's frontmatter description is written to be read by a model
/// deciding whether to open the skill, which is exactly this question, so the
/// cut is generous: 1,200 characters holds every one of the 50 skills
/// installed on this machine whole (2026-09-21, across `~/.zo/skills`,
/// `~/.claude/skills` and `~/.codex/skills`; p50 232, p75 405, p90 548, max
/// 916). Past it a description is a document, and a document is what
/// `skill_load` hands back.
pub const SKILL_DESCRIPTION_CHAR_CAP: usize = 1_200;

/// Skills one request asks about ([`shard::even_shards`]'s target).
///
/// The reference build this seat is borrowed from sends 137 skills as three
/// even shards at this target, and reports one search at 38.6k input tokens
/// (`pi-jev-skill-picker`, 2026-09-21). Fifty questions over one state is a
/// request whose state is written once and whose questions are a line each,
/// so the shard that decides the answer is the one that is asked, not the one
/// that is longest.
pub const SKILL_SHARD_TARGET: usize = 50;

/// The most skills one search hands back whole. Past this a tool result is a
/// document dump: the measured mean skill body on this machine is 9.4 KiB
/// (2026-09-21, 18 `~/.zo/skills`), so five is already the size of the index
/// this seat exists to remove.
pub const SKILL_TOP_CAP: usize = 5;

/// How many a search hands back when the caller does not say.
///
/// Three, which is what the reference build returns, and 28 KiB of skill
/// bodies at this machine's mean — a tool result a turn reads once, against
/// an index that rode every request. A caller that wants more asks for it, up
/// to [`SKILL_TOP_CAP`].
pub const SKILL_TOP_DEFAULT: usize = 3;
const _: () = assert!(
    SKILL_TOP_DEFAULT <= SKILL_TOP_CAP,
    "the default is one of the sizes a caller may ask for"
);

/// The ordered levels of the one question asked about each skill, lowest
/// first — the words themselves, so the seat's rubric is in the table its
/// caps are in and a level cannot be reworded without the row noticing.
///
/// Three levels and not four, because a skill is not a note: a note can share
/// a subject without answering anything, which is the distinction recall's
/// four levels are for, while a skill either covers the procedure at hand,
/// sits next to it, or does not.
pub const SKILL_LEVELS: [&str; 3] = [
    "The skill is about some other kind of work; nothing in it applies to this task.",
    "The skill is adjacent — it shares tools, files or vocabulary with the task but does not cover it.",
    "The skill covers this task directly: its procedure is one this task should follow.",
];

/// What a skill's reading must reach, in parts per thousand of the top level,
/// before the search hands the skill back at all.
///
/// The line the reference build names is "the expected level must lean toward
/// *covers this directly*" — 1.4 of a top level of 2 — and it is written here
/// in the units this table's other lines are written in, so a reader
/// comparing a seat's floors is comparing numbers of the same kind. 1.4 / 2.0
/// is 700 per thousand.
///
/// It is not [`SKILLS`]'s `answer_floor_permille`, and the two are never read
/// for each other: this one is a line under one SKILL's relevance, and that
/// one is a line under how often the SEAT answers at all. A seat can answer
/// every time and still rate nothing above this floor, which is the honest
/// outcome for a task no installed skill covers.
pub const SKILL_RELEVANCE_FLOOR_PERMILLE: u16 = 700;

/// What the skill seat's answers must bound above before `auto` rises to
/// replacing the prompt's index (§4): nine in ten.
///
/// The orchestration seats' number, reached from this seat's own side: a
/// search that does not come back costs the turn nothing it was not already
/// going to pay, because the deterministic word match behind it still hands
/// the turn a ranked list ([`ROUTE_USE_FALLBACK`]). Written here rather than
/// read from [`ORCHESTRATION_ANSWER_FLOOR_PERMILLE`] for the reason that
/// constant is not [`SCREEN_ANSWER_FLOOR_PERMILLE`]: two lines that happen to
/// coincide are still two policies, and a search that learns to answer at a
/// different rate than a coordinator's sweep should move one of them alone.
pub const SKILL_ANSWER_FLOOR_PERMILLE: u16 = 900;

/// The skill seat's route-change budget (§4): four searches in five must have
/// put a skill the turn went on to load among the ones they handed back.
///
/// What this seat has in place of a probe is hindsight, as the screen seats
/// do — the turn's own next move. A search whose answer the turn then loaded
/// named the right skill; one the turn ignored, and read the index or another
/// skill instead, did not. Held to the same budget as every other seat that
/// takes a decision off a reader: one in five wrong is not a rate at which to
/// remove the index unasked.
pub const SKILL_AGREEMENT_FLOOR_PERMILLE: u16 = 800;

/// The wall one skill search may hold the tool call that asked for it.
///
/// A tool result is the thing the model is waiting for, so the wall is the
/// one a person notices rather than the one a router can hide: ten seconds,
/// the summons seat's number, because both are a wait somebody is sitting
/// through rather than a step inside a turn. Past it the search hands back
/// what the word match ranked and says so on the row.
pub const SKILL_SEARCH_APPLY_DEADLINE_MS: u64 = 10_000;

/// zo's skill search: how much each installed skill covers the task a turn
/// describes, one score question per skill (t-5629).
///
/// The seat exists to take the skill index out of the system prompt. Today
/// every installed skill's name and compacted description is rendered on
/// every request under a 900-token budget, and a machine past that budget
/// pays it in full and still has its tail folded into a line that names only
/// a count. What the seat puts in its place is a tool: the search ranks every
/// skill — none folded away — and hands the top ones back whole, in a tool
/// result, where the cached prefix never sees them.
///
/// `on` and a risen `auto` are the apply stage, and what they apply is the
/// prompt itself: the index section is replaced by the two tools' names.
/// `shadow` leaves the index exactly where it is and records what the search
/// would have handed back, which is what makes the two readable side by side.
///
/// What is sent is the task, and the name and description of each installed
/// skill. A skill's BODY is never sent: it is read from the disk this machine
/// already holds it on and handed to the model as a tool result, so the
/// judgment prices a line and the turn reads a document.
pub const SKILLS: JevUse = JevUse {
    id: "skills",
    setting: "skillSearch",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/task",
            cap: Cap::Chars(SKILL_TASK_CHAR_CAP),
        },
        Sent {
            at: "/state/skills",
            cap: Cap::Items(SKILL_SHARD_TARGET),
        },
        Sent {
            at: "/state/skills/*/name",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/skills/*/description",
            cap: Cap::Chars(SKILL_DESCRIPTION_CHAR_CAP),
        },
    ],
    ledger: "skill-search.jsonl",
    promotes: true,
    answer_floor_permille: Some(SKILL_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(SKILL_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(SKILL_SEARCH_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// The wall zo's step effort governor holds a step judgment to, in
/// milliseconds — the latency line the judge reads a rising seat against.
///
/// The judgment is detached: it never holds a request, and its answer is
/// read at the NEXT step of the loop. What the wall means here is "in time
/// for the next request": a step's own round trip — the model's reply plus
/// the tool batch after it — runs longer than this on every measured turn,
/// so an answer inside it is on the desk before the request it could move
/// leaves. Written as its own number and not read from
/// [`ROUTING_APPLY_DEADLINE_MS`]: routing's wall is what a turn WAITS, this
/// one is what a step can still USE, and two lines that coincide today are
/// still two policies.
pub const ZO_STEP_EFFORT_APPLY_DEADLINE_MS: u64 = 1_500;

/// zo's step effort governor: the band of a step inside the turn — read as
/// the routing rubric's complexity of the turn's words with the step's
/// signals after them — so a request may spend a rung less on a routine
/// step and a rung more on a stuck one
/// (docs/design/zo-step-effort-governor-20260921.md, t-5633).
///
/// The seat is asked only where the governor's own table is unsure — a
/// read-only batch that is also slipping — or once every few steps; its
/// answer moves the NEXT request's effort, never the current one. What is
/// sent is the same head of the turn the routing seat sends, under the same
/// cap, with one line of counts (batch kind, repeats, errors, a red check)
/// that name no file and quote no output.
///
/// `auto` rises on the seat's own evidence: the `agreed` mark its writer
/// leaves one step after the judgment was consulted — whether that step
/// made progress (no repeated call, no error, no red check) — is what the
/// judge counts, held to the routing seat's own lines because the answer
/// moves a request field the same way a route does.
pub const ZO_STEP_EFFORT: JevUse = JevUse {
    id: "step_effort",
    setting: "zoStepEffort",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[Sent {
        at: "/state",
        cap: Cap::Chars(ROUTING_TASK_CHAR_CAP),
    }],
    ledger: "step-effort-zo.jsonl",
    promotes: true,
    answer_floor_permille: Some(950),
    press_floor_permille: None,
    agreement_floor_permille: Some(ROUTE_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(ZO_STEP_EFFORT_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_NOTHING),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// Characters of the person's last request one compaction judgment reads
/// as the goal the remaining work serves — the routing seat's cap, for the
/// routing seat's reason: the head of a request is what says what it is
/// about, and a second number on the same words would be a second answer
/// to how much of a person's work leaves the machine.
pub const COMPACTION_GOAL_CHAR_CAP: usize = ROUTING_TASK_CHAR_CAP;

/// Characters of the assistant's newest words one compaction judgment reads
/// as where the work stands — the recall seat's request cap, because the two
/// are the same kind of text read for the same purpose: the sentence or two
/// that says what is being done right now.
pub const COMPACTION_RECENT_CHAR_CAP: usize = RECALL_REQUEST_CHAR_CAP;

/// Characters of one tool call's input a compaction judgment carries beside
/// the result it produced — the step-effort seat's card-line cap, which is
/// what a call's name and target take on the board. Measured on this
/// machine's 1,091 transcripts (2026-09-22): tool inputs run to 122
/// characters at the median and 324 at p75; 68% fit whole, and the rest are
/// a body (a file's new content, a long command) whose head is the target.
pub const COMPACTION_INPUT_CHAR_CAP: usize = STEP_EFFORT_REPEATED_CHAR_CAP;

/// Bytes of one tool result's head a compaction judgment carries — the recall
/// seat's summary cap, for the same reason: a head is what a reader skims to
/// know what a result was about, never the result. Measured on this machine's
/// 14,509 clearable tool results (2026-09-22, bodies of 240 B and over):
/// 320 bytes holds their first five lines at the median and two at p10,
/// against bodies of 1,995 B (p50) and 8,561 B (p90).
pub const COMPACTION_BLOCK_HEAD_BYTE_CAP: usize = RECALL_SUMMARY_BYTE_CAP;

/// Tool results one compaction request asks about ([`shard::even_shards`]'s
/// target) — the skill seat's number, because the two requests have the same
/// shape: one state written once and a line-long question per item, so the
/// shard that decides the answer is the one that is asked, not the longest.
pub const COMPACTION_SHARD_TARGET: usize = SKILL_SHARD_TARGET;

/// The closed answer every compaction question offers, spelled once: keep the
/// block for the summary to read, or drop it. A third word would be an answer
/// nothing could act on.
pub const COMPACTION_OPTIONS: [&str; 2] = [COMPACTION_KEEP, COMPACTION_DROP];

/// The word that keeps a block in the summary's input.
pub const COMPACTION_KEEP: &str = "keep";

/// The word that takes a block out of the summary's input.
pub const COMPACTION_DROP: &str = "drop";

/// What a `drop` answer's own probability must reach, per thousand, before
/// the block is dropped at all.
///
/// The two sides of this question are not symmetric. A block kept costs the
/// summary a few hundred tokens of input once; a block dropped is one the
/// summary never sees — recoverable from the vault by `session_recall`, but
/// not from the summary — so the judgment has to be more than half sure
/// before it takes one out. Seven in ten is the skill seat's line for the
/// same lean ([`SKILL_RELEVANCE_FLOOR_PERMILLE`], "the expected level must
/// lean toward covers this directly"), written here in the units this table
/// writes its other lines in.
pub const COMPACTION_DROP_FLOOR_PERMILLE: u16 = 700;

/// What the compaction seat's answers must bound above before `auto` rises
/// to dropping (§4): nine in ten.
///
/// The skill seat's reasoning, reached from this seat's own side: a question
/// that does not come back costs the compaction nothing it was not already
/// going to pay — every block is kept and the summary reads what it read
/// before the seat existed ([`ROUTE_USE_FALLBACK`]). Written as its own
/// number rather than read from [`SKILL_ANSWER_FLOOR_PERMILLE`]: two lines
/// that coincide are still two policies.
pub const COMPACTION_ANSWER_FLOOR_PERMILLE: u16 = 900;

/// The compaction seat's route-change budget (§4): four dropped blocks in
/// five must be ones the turns after never went back for.
///
/// What this seat has in place of a probe is hindsight: a block dropped and
/// then read again within [`COMPACTION_REGRET_TURNS`] turns — the same path
/// opened, or the same call made — was a block the work still needed, and
/// the label says so (`regret`). Held to the same budget as every other seat
/// that takes a decision off a reader: one in five wrong is not a rate at
/// which to take blocks away from the summary unasked.
pub const COMPACTION_AGREEMENT_FLOOR_PERMILLE: u16 = 800;

/// The wall one compaction waits for its judgments, in milliseconds — one
/// number for the whole batch, every shard leaving together and the slowest
/// of them being what the boundary waits.
///
/// Three seconds: twice the routing seat's wall, because a compaction request
/// carries up to [`COMPACTION_SHARD_TARGET`] questions over a state of some
/// 16 KiB where routing carries five over 2,000 characters, and a compaction
/// is not a turn's wait but a boundary that already sits through a summary
/// round-trip measured in tens of seconds. Past it every unanswered block is
/// kept, which is exactly what a compaction did before the seat existed.
pub const COMPACTION_APPLY_DEADLINE_MS: u64 = 3_000;

/// Turns after a compaction inside which reading a dropped block again counts
/// against the judgment that dropped it (the seat's hindsight label).
///
/// Five is a policy line, not a measured one: long enough that the turn
/// resuming from the summary and the few after it — where a missing fact
/// shows as a re-read — are inside it, short enough that a file opened an
/// hour later for another reason is not charged to a decision it had nothing
/// to do with (the placement seat's window, [`PLACEMENT_LABEL_WINDOW_MS`],
/// is drawn the same way). A label is written once per dropped block: at the
/// turn that read it again, or at the fifth turn that did not.
pub const COMPACTION_REGRET_TURNS: u32 = 5;

/// zo's relevance compaction: for each tool result about to be summarized
/// out of a session, whether the summary still needs to read it (t-6039).
///
/// Compaction used to trim by age alone — microcompact clears the OLDEST
/// results — and the summary read everything the tail did not keep. The
/// idea is jevable's "instant compaction": ask, per tool result, whether the
/// remaining work needs it, and summarize only what is left. A drop reuses
/// the microcompact machinery whole: the original is sealed to the vault
/// first, the summary's copy carries the placeholder, and the eviction seal
/// heals the placeholder back before the raw record is written — so the
/// vault stays lossless and `session_recall` can still read a dropped block.
///
/// What is sent is the head of the person's last request, the head of the
/// assistant's newest words, and for each block its tool's name, the head of
/// its input and the head of its output — never a body, never the tail.
/// Every request passes the door; a refusal, a timeout or a reply that
/// breaks the contract keeps every block it covered.
///
/// `on` and a risen `auto` are the apply stage: the dropped blocks leave the
/// summary's input. `shadow` asks and records what it would have dropped and
/// the summary reads what it read before. The `agreed` rule is hindsight
/// ([`COMPACTION_REGRET_TURNS`]): one label per dropped block, `false` when
/// a turn inside the window read the same path or made the same call again
/// (regret), `true` at the window's end otherwise.
pub const COMPACTION: JevUse = JevUse {
    id: "compaction",
    setting: "jevCompaction",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/goal",
            cap: Cap::Chars(COMPACTION_GOAL_CHAR_CAP),
        },
        Sent {
            at: "/state/recent",
            cap: Cap::Chars(COMPACTION_RECENT_CHAR_CAP),
        },
        Sent {
            at: "/state/blocks",
            cap: Cap::Items(COMPACTION_SHARD_TARGET),
        },
        Sent {
            at: "/state/blocks/*/tool",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/blocks/*/input",
            cap: Cap::Chars(COMPACTION_INPUT_CHAR_CAP),
        },
        Sent {
            at: "/state/blocks/*/head",
            cap: Cap::Bytes(COMPACTION_BLOCK_HEAD_BYTE_CAP),
        },
    ],
    ledger: "compaction-relevance.jsonl",
    promotes: true,
    answer_floor_permille: Some(COMPACTION_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(COMPACTION_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(COMPACTION_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Hindsight,
};

/// Characters of one text an agent's own question carries — the question,
/// its context, an option, a level, an item (t-6040).
///
/// The routing seat's cap and not a second number, for the reason
/// [`SKILL_TASK_CHAR_CAP`] is: the head of a text is what a question reads,
/// and a second cap on a caller's words would be a second answer to how much
/// of them leaves the machine.
pub const AGENT_TOOL_TEXT_CHAR_CAP: usize = ROUTING_TASK_CHAR_CAP;

/// The most options one `choose` may offer: the wire's own ceiling
/// (docs.typesafe.ai/primitives/choice — "up to 255 options"), spelled once
/// here so the tool and the CLI refuse the same list.
pub const AGENT_TOOL_OPTION_CAP: usize = 255;

/// How many levels one `score` may name: the wire's own bounds
/// (docs.typesafe.ai/primitives/score — "at least two levels; the API accepts
/// up to 10").
pub const AGENT_TOOL_LEVELS: std::ops::RangeInclusive<usize> = 2..=10;

/// The most items one `score` call takes, asked as even shards of
/// [`SKILL_SHARD_TARGET`] at once ([`shard::even_shards`], the skill seat's
/// own arithmetic). Five shards, because that is what the largest batch
/// measured here wanted — the 240-page classification golden of t-6040 — and
/// a caller with more pipes twice; each shard is one request against the
/// day's budget.
pub const AGENT_TOOL_ITEM_CAP: usize = 5 * SKILL_SHARD_TARGET;

/// The wall one agent question may hold the tool call or the shell that asked
/// it: the skill seat's, for the skill seat's reason — a tool result is what
/// a model is waiting for, so the wall is the one a person notices.
pub const AGENT_TOOL_DEADLINE_MS: u64 = SKILL_SEARCH_APPLY_DEADLINE_MS;

/// The two options an `ask` is a choice over, the affirmative first. Spelled
/// once: the tool's answer and the CLI's exit code both read it.
pub const AGENT_TOOL_ASK_OPTIONS: [&str; 2] = ["yes", "no"];

/// The seat an agent asks on purpose: zo's `Jev` tool and `zo jev
/// ask|choose|score`, which a worker or an agent in a pane calls instead of
/// spending its own model's tokens on a classification, a pick or a grading
/// (t-6040; the vault's "a Jev showcase maps to our eleven seats and leaves
/// compaction and an agent door").
///
/// It is not a stage of the product's own: nothing falls back to it and
/// nothing it answers is compared with a reader it would replace, so it has
/// no label, never promotes, and offers no `auto` — an `auto` that can never
/// rise is `shadow` under a name that promises otherwise. `shadow` asks and
/// writes the row and hands the caller nothing, so a person can read what an
/// agent's questions cost before letting agents act on the answers; `on`
/// hands the answer over. `off`, the default, is the tool refusing and the
/// CLI exiting 2 with nothing sent and nothing written.
///
/// What is sent is the caller's own words — the question, its context, the
/// options or levels, the items — each cut to [`AGENT_TOOL_TEXT_CHAR_CAP`]
/// and cleared of credential lines where the door reads it. The option and
/// level words ride the questions' criteria (a choice's descriptions, a
/// score's levels), so the door reads that path too, as the screen seats do.
/// The rows only count: caller, shape, size, tokens and latency
/// (docs: the seat is "집계만").
pub const AGENT_TOOL: JevUse = JevUse {
    id: "agent_tool",
    setting: "agentTool",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On],
    sends: &[
        Sent {
            at: "/state/question",
            cap: Cap::Chars(AGENT_TOOL_TEXT_CHAR_CAP),
        },
        Sent {
            at: "/state/context",
            cap: Cap::Chars(AGENT_TOOL_TEXT_CHAR_CAP),
        },
        Sent {
            at: "/state/items",
            cap: Cap::Items(SKILL_SHARD_TARGET),
        },
        Sent {
            at: "/state/items/*",
            cap: Cap::Chars(AGENT_TOOL_TEXT_CHAR_CAP),
        },
        Sent {
            at: "/questions/*/criteria/*",
            cap: Cap::Chars(AGENT_TOOL_TEXT_CHAR_CAP),
        },
    ],
    ledger: "agent-tool.jsonl",
    promotes: false,
    answer_floor_permille: None,
    press_floor_permille: None,
    agreement_floor_permille: None,
    apply_deadline_ms: None,
    window_forgives: None,
    agreement_rows_wanted: None,
    agreement_kind: AgreementKind::Comparison,
};

/// Characters of a page's title one browser-read question carries — the head
/// of what the tab says the page is, which is what bands a block's words:
/// "Sign in" under a title that is a product page is a chrome block, and the
/// same two words under "Sign in" are the page.
pub const BROWSER_READ_TITLE_CHAR_CAP: usize = 200;

/// Blocks one browser read asks about ([`Sent`] `Items` cap on
/// `/state/blocks`). A page past it keeps its remaining blocks unjudged and
/// unfolded: a block nobody asked about is content, because the fallback of
/// this seat is the whole page ([`BROWSER_READ`]).
///
/// Forty-eight is four requests of [`BROWSER_READ_SHARD_TARGET`], which is
/// the most the read's one wall ([`BROWSER_READ_APPLY_DEADLINE_MS`]) is asked
/// to carry side by side; a page that cuts into more blocks than that is a
/// page whose tail is already past the read's own text cap.
pub const BROWSER_READ_BLOCK_CAP: usize = 48;

/// Characters of one block's text head a browser-read question carries. The
/// head is what names a block — "Accept all cookies", "Related articles",
/// "Skip to content" — and the body under it is never sent: it is read from
/// the page this machine already holds and handed to the agent or folded.
pub const BROWSER_READ_HEAD_CHAR_CAP: usize = 240;

/// Characters of one block's tag path a browser-read question carries —
/// `body>main>article>aside[role=complementary]` — the structural address
/// that says what the page's author called the block.
pub const BROWSER_READ_PATH_CHAR_CAP: usize = 160;

/// Blocks one browser-read request asks about ([`shard::even_shards`]'s
/// target): twelve choice questions over one state, the same width a screen
/// question offers controls ([`SCREEN_CANDIDATE_CAP`]) and for the same
/// reason — past it a request is longer than the answer it decides. Written
/// as its own number because the two are two policies.
pub const BROWSER_READ_SHARD_TARGET: usize = 12;

/// The structural selectors a page is cut into blocks at, in the browser
/// door's read: HTML's own landmarks and sectioning elements, and the ARIA
/// roles that name the same things on a page built of `div`s. One list, here
/// and nowhere else: the page script that cuts blocks reads it from the
/// request, and the click and type scripts that name the block a press
/// landed in read the same list, so a fold and a label cannot disagree about
/// where a block begins.
pub const BROWSER_READ_BLOCK_ROOTS: [&str; 20] = [
    "main",
    "article",
    "section",
    "nav",
    "header",
    "footer",
    "aside",
    "form",
    "dialog",
    "[role=main]",
    "[role=navigation]",
    "[role=banner]",
    "[role=contentinfo]",
    "[role=complementary]",
    "[role=dialog]",
    "[role=alertdialog]",
    "[role=search]",
    "[role=region]",
    "[role=form]",
    "[role=article]",
];

/// The two answers a block can be: the page's own substance, or the
/// furniture around it. Spelled once, because the question offers exactly
/// these and the fold acts on exactly one of them.
pub const BROWSER_READ_OPTIONS: [&str; 2] = ["content", "chrome"];

/// [`BROWSER_READ_OPTIONS`]'s word for a block the fold may drop.
pub const BROWSER_READ_CHROME: &str = BROWSER_READ_OPTIONS[1];

/// [`BROWSER_READ_OPTIONS`]'s word for a block the read keeps.
pub const BROWSER_READ_CONTENT: &str = BROWSER_READ_OPTIONS[0];

/// The wall one browser read waits for its blocks' judgments, in
/// milliseconds — every shard side by side, one clock. A read is a tool
/// result the agent is waiting on, so the wall is a screen question's
/// ([`SCREEN_APPLY_DEADLINE_MS`], 280 ms at the median on this wire) and
/// not a summons' ten seconds; past it the read hands back the whole page,
/// which is what it handed back before the seat existed. Its own number, for
/// the reason every other coinciding wall in this table is.
pub const BROWSER_READ_APPLY_DEADLINE_MS: u64 = 1_500;

/// What a browser-read seat's answers must bound above before `auto` rises
/// to folding (§4): nine in ten — the screen seats' line, arrived at from
/// the read's own side: a judgment that does not come back costs the agent
/// nothing but the wall, because the whole page is what it reads then.
pub const BROWSER_READ_ANSWER_FLOOR_PERMILLE: u16 = 900;

/// The browser-read seat's route-change budget (§4): nine folds in ten must
/// be ones the agent never reached back into.
///
/// Stricter than the screen seats' four in five, because this seat's wrong
/// answer is not one press but a page the agent has to read twice — the
/// fold, then `read --full` — and the label that says so is the agent's own
/// next move ([`AgreementKind::Comparison`]: the block a `click` or `type`
/// then landed in was one the judgment called chrome, or it was not).
pub const BROWSER_READ_AGREEMENT_FLOOR_PERMILLE: u16 = 900;

/// The least confidence a block's `chrome` answer must carry before the
/// fold drops it, per thousand — read through [`JevUse::permits_press`], the
/// same gate a screen press passes, because it is the same question: is
/// this one answer sure enough to act on alone.
///
/// A policy line, not a calibrated accuracy claim, like
/// [`SCREEN_PRESS_FLOOR_PERMILLE`], and above it: a press the walk can undo
/// costs one more press, a block the fold dropped costs the agent the page.
/// The measurement this seat is held to says the line has to sit where no
/// content block of the golden pages falls under it (t-6041).
pub const BROWSER_READ_FOLD_FLOOR_PERMILLE: u16 = 700;

/// The window's browser read: which blocks of a page's text an agent's
/// `zerocode-browser read` should hand over, and which it should fold away
/// (t-6041).
///
/// The read gives the agent the page's visible text whole, and most of a
/// page is not what the agent came for — the site's navigation, its header
/// and footer, a cookie banner, the sponsored rail, the related-links
/// column. The page script cuts the body into blocks at
/// [`BROWSER_READ_BLOCK_ROOTS`], and one question per block asks whether the
/// block is the page ([`BROWSER_READ_CONTENT`]) or its furniture
/// ([`BROWSER_READ_CHROME`]). What is sent is each block's tag path and the
/// head of its text, and the page's title; a block's body never leaves.
///
/// `on`, and an `auto` its own evidence raised, fold the chrome blocks into
/// one line that says how many were left out and what kinds, and how to
/// read them anyway (`read --full`). `shadow` records the same judgment and
/// hands over the whole page. A judgment that does not come back inside
/// [`BROWSER_READ_APPLY_DEADLINE_MS`], or that breaks the closed choice's
/// rules on any block, hands over the whole page too: the seat's fallback is
/// exactly the read that existed before it.
///
/// The `agreed` rule: the judgment agreed when the agent's next `click` or
/// `type` on that pane, on that same page, landed outside every block the
/// judgment called chrome, and disagreed when it landed inside one — under
/// `shadow` as much as under `on`, since what is compared is the judgment
/// and not the fold. One label per read, written by the window at the press;
/// a read the agent never pressed after leaves no label, and a read on a page
/// the pane had left by the press leaves none either.
pub const BROWSER_READ: JevUse = JevUse {
    id: "browser_read",
    setting: "jevBrowserRead",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/title",
            cap: Cap::Chars(BROWSER_READ_TITLE_CHAR_CAP),
        },
        Sent {
            at: "/state/blocks",
            cap: Cap::Items(BROWSER_READ_BLOCK_CAP),
        },
        Sent {
            at: "/state/blocks/*/path",
            cap: Cap::Chars(BROWSER_READ_PATH_CHAR_CAP),
        },
        Sent {
            at: "/state/blocks/*/head",
            cap: Cap::Chars(BROWSER_READ_HEAD_CHAR_CAP),
        },
    ],
    ledger: "browser-read.jsonl",
    promotes: true,
    answer_floor_permille: Some(BROWSER_READ_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: Some(BROWSER_READ_FOLD_FLOOR_PERMILLE),
    agreement_floor_permille: Some(BROWSER_READ_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(BROWSER_READ_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// The closed answer every notify question offers, spelled once: ring now,
/// hold it for the next moment the person turns to the window, or say
/// nothing. A fourth word would be an answer nothing could act on.
pub const NOTIFY_OPTIONS: [&str; 3] = [NOTIFY_INTERRUPT, NOTIFY_BATCH, NOTIFY_IGNORE];

/// The word that rings the person now — what today's rule does with every
/// ring it does not suppress.
pub const NOTIFY_INTERRUPT: &str = "interrupt";

/// The word that holds a ring for the next moment the person is at the
/// window, where every held ring is folded into one notice.
pub const NOTIFY_BATCH: &str = "batch";

/// The word that drops a ring.
pub const NOTIFY_IGNORE: &str = "ignore";

/// How long after a ring the person's hand on that pane counts as the
/// ring's label, in milliseconds (t-6043).
///
/// One minute is a policy line, not a measured one — the brief's own
/// number: long enough to walk back to the desk after a lock-screen
/// notification and click the pane it named, short enough that a pane
/// opened later for another reason is not charged to a ring it had nothing
/// to do with (the placement seat's window,
/// [`PLACEMENT_LABEL_WINDOW_MS`], is drawn the same way at five). The seat
/// writes the label at the first hand inside it, or once it has passed.
pub const NOTIFY_LABEL_WINDOW_MS: i64 = 60 * 1_000;

/// How long since the person's last hand on the window they still count as
/// present at it, in milliseconds — the one attendance fact the question
/// carries beside the window's focus.
///
/// The placement seat's window, read from there rather than respelled: the
/// same question, asked the other way round. That seat asks how long after a
/// pane appeared a person's move still counts against it — "long enough for
/// somebody who was typing elsewhere to turn to the new pane" — and this one
/// asks how long since they last typed anywhere they should still be counted
/// as somebody who could turn. A focused window nobody has touched for longer
/// than this is a desk somebody left, and a ring there is a lock-screen
/// ring.
pub const NOTIFY_ATTENDANCE_WINDOW_MS: i64 = PLACEMENT_LABEL_WINDOW_MS;

/// Characters of the ring's words one notify question carries — the
/// question the agent stopped on, or its last words — which is the
/// notification's own body: one card line
/// (`crate::transcript::SUMMARY_CHARS`), read through the step-effort seat's
/// cap because the two are the same cut of the same kind of text, and a
/// second number on it would be a second answer to how much of an agent's
/// words leave the machine per ring.
pub const NOTIFY_WORDS_CHAR_CAP: usize = STEP_EFFORT_REPEATED_CHAR_CAP;

/// Rings of the same pane one notify question carries as its context —
/// each as its event word, how long ago, and whether the person turned to
/// the pane inside [`NOTIFY_LABEL_WINDOW_MS`] of it.
///
/// Six is a policy line: the per-worktree cooldown
/// (`crate::notify::COOLDOWN_MS`, five seconds) bounds a pane to twelve
/// rings a minute, so six is the busiest half-minute a pane can have had —
/// enough to show a pane that has finished four times without a response,
/// and not a history. Older rings are not what makes this one worth an
/// interruption.
pub const NOTIFY_RECENT_CAP: usize = 6;

/// The wall the bell holds an attention ring for the seat's answer, in
/// milliseconds, when the seat acts.
///
/// The completion's own quiet (`crate::notify::DONE_QUIET_MS`), which every
/// completion ring already waits before it may ring at all: an answer inside
/// it delays an attention ring by no more than every "finished" is already
/// delayed, and a completion's question is asked beside its quiet rather
/// than after it. Past the wall today's rule rings, exactly as it did before
/// the seat existed. Written as the completion's number and not read from
/// [`ROUTING_APPLY_DEADLINE_MS`], which happens to coincide: that is a
/// turn's wait, this is a bell's, and two policies that coincide are still
/// two policies.
pub const NOTIFY_APPLY_DEADLINE_MS: u64 = crate::notify::DONE_QUIET_MS.unsigned_abs();

/// What the notify seat's answers must bound above before `auto` rises to
/// holding and dropping rings (§4): nine in ten.
///
/// The orchestration seats' reasoning, reached from this seat's own side: a
/// question that does not come back costs the person nothing they were not
/// already going to get — today's rule rings, as it did before the seat
/// existed — and every answer that does come back can take a ring away.
/// Written as its own number rather than read from
/// [`ORCHESTRATION_ANSWER_FLOOR_PERMILLE`]: two lines that coincide are
/// still two policies.
pub const NOTIFY_ANSWER_FLOOR_PERMILLE: u16 = 900;

/// The notify seat's route-change budget (§4): four calls in five must have
/// been what the person's own hand then said — a ring they turned to within
/// the minute was worth an interruption, one they did not turn to while at
/// the window was not. Held to the same budget as every other seat that
/// takes a decision off a reader: one ring in five wrongly dropped is an
/// agent waiting on a person who was never told.
pub const NOTIFY_AGREEMENT_FLOOR_PERMILLE: u16 = 800;

/// The window's ring judgment: whether one notification is worth the
/// interruption — ring now, hold it for the next moment the person turns to
/// the window, or say nothing (t-6043, `crate::notify_call`).
///
/// Today one rule table decides: a stopped or finished pane rings unless it
/// is the screen being watched, and a per-worktree cooldown thins the rest
/// (`crate::notify`). What the table cannot read is whether THIS ring is one
/// the person needs this second — the fourth "finished" in ten minutes from
/// a worker nobody has turned to, a "needs input" while three other panes
/// already wait, a completion at two in the morning. The seat is asked at
/// the one point the table decides "ring or not", about the ring's kind
/// ([`crate::notify::verb`]'s word), the pane's name, whether the person is
/// present at the window or away, one card line of the ring's words, the
/// pane's last rings and whether each was turned to, and how many panes
/// wait. A watched screen is today's own ignore and is not a question.
///
/// Under `off`, `shadow`, a timeout or a refusal the bell rings exactly as
/// today; only a person's `on`, or an `auto` its own evidence raised, lets
/// `batch` and `ignore` change what the OS shows and what the tab strip
/// marks. A held ring is
/// folded with every other held ring into one notice at the person's next
/// hand on the window (`crate::notify::batched`).
///
/// The `agreed` rule: a Comparison label written by the person's own hand.
/// A key or a paste into the ring's pane within [`NOTIFY_LABEL_WINDOW_MS`]
/// says the ring was worth the interruption — `agreed` iff the call was
/// [`NOTIFY_INTERRUPT`]. No hand inside the window while the person was
/// present at it says it was not — `agreed` iff the call was not. No hand
/// while they were away says nothing either way, and leaves no mark: a
/// person who was not there could not have turned to it. One label row per
/// answered row, keyed by the row's own `notify` key.
pub const NOTIFY: JevUse = JevUse {
    id: "notify",
    setting: "jevNotify",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/pane",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/words",
            cap: Cap::Chars(NOTIFY_WORDS_CHAR_CAP),
        },
        Sent {
            at: "/state/recent",
            cap: Cap::Items(NOTIFY_RECENT_CAP),
        },
    ],
    ledger: "notify-call.jsonl",
    promotes: true,
    answer_floor_permille: Some(NOTIFY_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(NOTIFY_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(NOTIFY_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// Characters of the sentence a person is writing that one mention
/// judgment reads as their intent — the composer's text around the `@`
/// token, or the words typed into the `/resume` search. The recall seat's
/// request cap, because it is the same kind of text read for the same
/// purpose: the sentence or two that says what the person is after.
pub const MENTION_INTENT_CHAR_CAP: usize = RECALL_REQUEST_CHAR_CAP;

/// Candidate rows one mention judgment is asked about: one page of the
/// popup — the eight rows a person sees without scrolling (codex
/// `MAX_POPUP_ROWS`, which zo's view pins to this number). A ninth row is one
/// the person has not seen, and a judgment that puts it first would be
/// reordering a list the person was never shown.
pub const MENTION_CANDIDATE_CAP: usize = 8;

/// Bytes of one candidate's head a mention judgment carries — a skill's
/// description, a session's first prompt as its row shows it. The recall
/// seat's summary cap: a head is what a reader skims to tell rows apart,
/// never a body. A file or a page carries no head at all, only its name.
pub const MENTION_HEAD_BYTE_CAP: usize = RECALL_SUMMARY_BYTE_CAP;

/// What the mention seat's answers must bound above before `auto` rises to
/// reordering the page (§4): nine in ten — the recall seat's line, read from
/// there because the two are the same kind: a ranking a reader falls back
/// from at no cost (the fuzzy page stands when the judgment does not come
/// back, as recall's own order stands). One name, so a re-measurement moves
/// this seat's line alone.
pub const MENTION_ANSWER_FLOOR_PERMILLE: u16 = RECALL_ANSWER_FLOOR_PERMILLE;

/// The mention seat's route-change budget (§4): four picks in five must be
/// the row the judgment put first — the comparison the person makes with
/// every completion, which is what this seat has in place of a probe. The
/// recall seat's budget, for the reason its answer floor is.
pub const MENTION_AGREEMENT_FLOOR_PERMILLE: u16 = RECALL_AGREEMENT_FLOOR_PERMILLE;

/// The wall past which a mention answer is dropped, in milliseconds — the
/// latency line the judge reads a rising seat against.
///
/// Nothing waits on this seat: the fuzzy page is drawn the moment the
/// person types, and the judgment lands on it afterwards, only while the
/// selection still sits on the first row. The wall is therefore not a wait
/// but a staleness line — an answer that arrives after it is for a page the
/// person has been looking at too long to move under them. The routing
/// seat's wall, because it is the same wire and the same question of how
/// long a Jev answer may hold the thing it is deciding, and the wire's
/// measured p95 sits well inside it (337 ms over 240 single-choice answers
/// on 2026-09-22, t-6040; this seat's own replay is in
/// `tools/mention-rerank-replay`). Written as its own number rather than read from
/// [`ROUTING_APPLY_DEADLINE_MS`]: two lines that coincide are still two
/// policies, and a page that learns to tolerate a later reorder should move
/// this one alone.
pub const MENTION_APPLY_DEADLINE_MS: u64 = 1_500;

/// zo's mention rerank: which row of the `@` popup, or of the `/resume`
/// list, the person means — read off the sentence they are writing
/// (t-6042).
///
/// The `@` popup (t-5871) and the `/resume` picker rank by fuzzy score and
/// recency; neither knows what the person is writing. The idea is jevable's
/// "Jev Search"/"Upweight": the fuzzy page is drawn first, exactly as today,
/// and one closed choice over its rows — asked off the key path — may put
/// the row the sentence is about first. The answer lands only while the
/// selection still sits on the first row; a person who has moved it has
/// already chosen a page, and a page is not moved under them. A newer
/// keystroke discards the older question: one question in flight, ever.
///
/// What is sent is the head of the sentence, the token typed, and one page
/// of candidate names with the head each row already shows on screen.
/// Nothing is read from disk for it: no file body, no page body, no
/// transcript. Every request passes the door; a refusal, a timeout or a
/// reply that breaks the contract leaves the fuzzy page as it was.
///
/// `on` and a risen `auto` are the apply stage: the page's rows take the
/// judgment's order. `shadow` asks and records what it would have put first
/// and the page stands. The `agreed` rule is a comparison, made by the
/// person: one label per completion, `true` when the row they took was the
/// row the judgment put first, `false` otherwise, with `rank` the place the
/// judgment gave the row they took; a pick outside the page the judgment
/// saw is written down as not compared, since the seat never offered it.
pub const MENTION_RERANK: JevUse = JevUse {
    id: "mention_rerank",
    setting: "jevMentionRerank",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/intent",
            cap: Cap::Chars(MENTION_INTENT_CHAR_CAP),
        },
        // The token is a piece of the same sentence, under the same cap —
        // a second cap on the same words would be a second answer to how
        // much of a person's writing leaves the machine.
        Sent {
            at: "/state/query",
            cap: Cap::Chars(MENTION_INTENT_CHAR_CAP),
        },
        Sent {
            at: "/state/candidates",
            cap: Cap::Items(MENTION_CANDIDATE_CAP),
        },
        Sent {
            at: "/state/candidates/*/name",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/candidates/*/head",
            cap: Cap::Bytes(MENTION_HEAD_BYTE_CAP),
        },
    ],
    ledger: "mention-rerank.jsonl",
    promotes: true,
    answer_floor_permille: Some(MENTION_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(MENTION_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(MENTION_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// How many of the emulator seat's candidates a forked phone step tries
/// before one is made canonical (t-6044, [`BRANCHING`]).
///
/// Two is the brief's number and the smallest fork there is: the press the
/// emulator seat would have made anyway, and the one it ranked next. Every
/// candidate past the first costs the walk a press, a look and a snapshot
/// load — on the AVD saves this window has measured, 1.4–2.0 s each for the
/// load alone (`emulator::android::SNAPSHOT_SAVE_LIMIT`'s note) — so the
/// count is a policy line on the walk's clock, not an accuracy claim. It is
/// read through [`crate::branching::top_k`], never respelled.
pub const BRANCHING_K: usize = 2;

/// The most candidates a forked step may ever try, whatever a later reading
/// of [`BRANCHING_K`] says: the question offers at most this many results
/// ([`Sent`] `Items` cap on `/state/candidates`), so a `k` above it would be
/// a candidate explored and never judged.
pub const BRANCHING_K_CAP: usize = 3;
const _: () = assert!(
    BRANCHING_K >= 2 && BRANCHING_K <= BRANCHING_K_CAP,
    "a fork tries at least two candidates and never more than the question can carry"
);

/// How far the emulator seat's first choice must lead its runner-up, in
/// parts per thousand of the answer's mass, for the step to be a single
/// press: a lead under this forks (`crate::branching::fork_wanted`), as does
/// a leader under the seat's own press floor ([`SCREEN_PRESS_FLOOR_PERMILLE`])
/// whatever its lead.
///
/// One in five, a policy line and not a calibrated claim, like every other
/// line in this table: without it a fork was taken at every step whose
/// answer gave a second control any weight at all (t-6155 F3), which on the
/// fake desk is every step and on a real device is ×4.82 the clock of the
/// step it replaces. One fifth is the same one-in-five the route-change
/// budgets are cut at (800‰): a leader the seat itself is not four-in-five
/// sure of over its runner-up is the answer a fork is for. Two of this
/// machine's phone presses to date carried a weighted runner-up, so the
/// line waits for `branching.jsonl` to move it.
pub const BRANCHING_FORK_MARGIN_PERMILLE: u16 = 200;
const _: () = assert!(
    BRANCHING_FORK_MARGIN_PERMILLE > 0 && BRANCHING_FORK_MARGIN_PERMILLE < 1_000,
    "a margin of nothing never forks and a margin of everything always does"
);

/// The wall the forked step holds a walk for the comparison's answer, in
/// milliseconds — the screen seats' wall, written as this seat's own number
/// for the reason every coinciding wall in this table is: the screen
/// question is asked BEFORE anything is pressed, this one after `k` presses
/// have already been tried and undone, and two policies that coincide today
/// are still two policies. Past it the first candidate — the emulator seat's
/// own press — is made canonical, which is exactly the step a walk without
/// this seat would have taken.
pub const BRANCHING_APPLY_DEADLINE_MS: u64 = 1_500;

/// What the branching seat's answers must bound above before `auto` rises to
/// choosing the canonical candidate (§4): nine in ten — the screen seats'
/// line, reached from this seat's own side: a comparison that does not come
/// back costs the walk the fork's clock and nothing else, because the first
/// candidate is then pressed as it would have been without the seat.
pub const BRANCHING_ANSWER_FLOOR_PERMILLE: u16 = 900;

/// The branching seat's route-change budget (§4): four canonical picks in
/// five must have been ones the walk then went on from. Held to the screen
/// seats' budget because the negative is the same kind: a pick the walk had
/// to undo or retry is one more press on a screen, and one in five wrong is
/// not a rate at which to hand the canonical step to a comparison unasked.
pub const BRANCHING_AGREEMENT_FLOOR_PERMILLE: u16 = 800;

/// The window's forked phone step (t-6044, `crate::branching`): when the
/// emulator seat's answer is torn between two or more controls — its first
/// choice leading the runner-up by under [`BRANCHING_FORK_MARGIN_PERMILLE`],
/// or sitting under the press floor itself — the walk saves the
/// Android device where it stands, presses each of the top [`BRANCHING_K`]
/// in turn, reads the screen each one leads to, puts the device back, and
/// asks Jev which RESULT is the closest to the goal — the candidate it names
/// is the one pressed for real. jevable's "Mario Never Dies" idea, on the
/// state rather than on the request: the hedge (`crate::jev::hedge`) copies a
/// question, this copies a world.
///
/// It has a seat of its own and not a flag on the emulator row because it
/// sends what that row never does — the screens a press LED TO, which a
/// person consenting to "judge which control to press" did not consent to
/// having explored on their device — and because the two are judged apart:
/// the emulator seat's mark is whether the press it chose reached the goal,
/// this seat's is whether a comparison of results picked better than that
/// press would have.
///
/// What is sent: the goal, the device's platform and name, the controls of
/// the screen before the fork, and per candidate the legend line that was
/// pressed and — when the fork explored — the legend lines of the screen it
/// led to and whether the screen moved. Never a screenshot, never a control's
/// body beyond its legend line.
///
/// `off` is today's walk byte for byte. `shadow` explores nothing: it asks
/// the same question over the candidates' actions alone, records what it
/// would have made canonical beside what the emulator seat pressed, and
/// presses the emulator seat's choice. Only a person's `on`, or an `auto`
/// this seat's own ledger raised, saves, explores and presses the
/// comparison's pick; a timeout, a
/// refusal, an answer under [`SCREEN_PRESS_FLOOR_PERMILLE`], a device that
/// cannot be saved (iOS has no snapshot road) or a fork past its clock all
/// press the first candidate — the emulator seat's own — as today. A step at
/// which the person's turn stands is never forked: the errand's own gates
/// bar it before any candidate exists.
///
/// The `agreed` rule (a Comparison label, one per forked step, written by the
/// walk's own next step): the canonical pick agreed when the walk went on
/// from it — the caller's condition held, or the next look showed a screen
/// that moved and the next judgment chose a control on it — and disagreed
/// when the next step was a retry (the same screen again) or the judgment
/// gave up on what the pick led to. Under `shadow` the comparison is graded
/// against the emulator seat's press: the same pick shares its fate, a
/// different pick is wrong when the press went on fine, and says nothing
/// when the press failed — an alternative nobody tried is not evidence
/// (`crate::branching::agreed`).
pub const BRANCHING: JevUse = JevUse {
    id: "branching",
    setting: "jevBranching",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/goal",
            cap: Cap::Chars(GOAL_CHAR_CAP),
        },
        Sent {
            at: "/state/where/platform",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/where/device",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/before",
            cap: Cap::Items(SCREEN_CANDIDATE_CAP),
        },
        Sent {
            at: "/state/before/*",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/candidates",
            cap: Cap::Items(BRANCHING_K_CAP),
        },
        Sent {
            at: "/state/candidates/*/action",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/candidates/*/result/controls",
            cap: Cap::Items(SCREEN_CANDIDATE_CAP),
        },
        Sent {
            at: "/state/candidates/*/result/controls/*",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/questions/*/criteria/*",
            cap: Cap::Uncut,
        },
    ],
    ledger: "branching.jsonl",
    promotes: true,
    answer_floor_permille: Some(BRANCHING_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: Some(SCREEN_PRESS_FLOOR_PERMILLE),
    agreement_floor_permille: Some(BRANCHING_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(BRANCHING_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// The judgment cache: a memo in front of the wire that answers a screen
/// question the door already cleared once with the very same bytes
/// (docs/design/stagehand-v4-jev-review-20260922.md, t-6132).
///
/// It is a seat with no question of its own. What it sends is nothing — a
/// hit leaves the machine no bytes and takes no place in the day's count —
/// and what it decides is whether the walk may take the memo's answer
/// instead of the wire's. Its rows are lookups: one per hit under `shadow`
/// and `auto`, carrying the memo's own `agreed` — the remembered choice
/// against the fresh one the wire gave for the same bytes — and one row per
/// hit answered under a risen `auto`, carrying `routeUse: applied`. A miss
/// is written beside them without an `outcome`, so a reader can count the
/// hit rate and the judge counts only what the memo answered.
///
/// `off` is today's walk to the byte: no lookup, no memo file, no row.
/// `shadow` asks the wire as today and records whether the memo would have
/// agreed. `on` is the person's own word: a hit answers with no evidence
/// asked for. `auto` answers from the memo only once its own comparisons
/// bound above [`JUDGMENT_CACHE_AGREEMENT_FLOOR_PERMILLE`] over a window
/// ([`promote`]), and either falls back to the wire the moment a remembered
/// body stops reading (the seat contract's third rule, second correction:
/// a labeled seat offers all four words).
///
/// The memo's key is the seat the question belongs to and the bytes the door
/// let through — after every withheld line and every cap — so two questions
/// share an answer exactly when the wire would have seen the same request
/// ([`memo::key_of`]). The rubric's words are inside those bytes, so a word
/// changed without a version bump is a different key rather than a stale hit.
pub const JUDGMENT_CACHE: JevUse = JevUse {
    id: "judgment_cache",
    setting: "jevJudgmentCache",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[],
    ledger: "judgment-cache.jsonl",
    promotes: true,
    answer_floor_permille: Some(JUDGMENT_CACHE_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(JUDGMENT_CACHE_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(JUDGMENT_MEMO_DEADLINE_MS),
    window_forgives: Some(FORGIVES_NOTHING),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// How many of a role's eligible attempts try the model nobody has evidence
/// for: one in five (user decision 2026-09-22, relayed m-6151).
///
/// A model discovery has never routed to earns no outcome rows, and a model
/// with no rows can never clear [`promote`]'s window — so the lineup this
/// machine believes in is the one it believed in the day the catalog was
/// written. One in five is the smallest share that fills a comparison window
/// inside a working day at this machine's measured rate (4,093 route
/// outcomes to 2026-09-22), and small enough that four of five attempts are
/// still the incumbent's.
pub const CHALLENGER_ONE_IN: u64 = 5;

/// The most of a day's model spend the challenger arm may be: one tenth
/// (same decision).
///
/// Counted by the price table, over the day's own rows, so a quiet day buys
/// a small experiment and a busy one a larger — a fixed dollar ceiling would
/// mean either no evidence on a quiet day or a runaway on a busy one.
pub const CHALLENGER_DAY_SPEND_PERMILLE: u16 = 100;

/// What a design question may carry of the work it is about.
///
/// The request in the person's own words, capped as the mention seat caps a
/// sentence: a judge comparing two answers needs the question, never the
/// repository.
pub const CHALLENGER_TASK_CHAR_CAP: usize = MENTION_INTENT_CHAR_CAP;

/// How many designs a comparison carries: two, the incumbent's and the
/// challenger's. A third would not be a comparison.
pub const CHALLENGER_DESIGN_CAP: usize = 2;

/// How much of each design the judge reads — a plan, not a patch. The stall
/// seat's transcript cap, which is the largest piece of a person's own text
/// this product already sends through the door.
pub const CHALLENGER_DESIGN_BYTE_CAP: usize = STALL_TRANSCRIPT_BYTE_CAP;

/// What the challenger seat's answers must bound above before `auto` scores
/// a comparison that moves a role's model: nine in ten, the recall seat's
/// line, because a judgment that does not come back costs nothing here —
/// the incumbent acted either way and the row is simply unlabeled.
pub const CHALLENGER_ANSWER_FLOOR_PERMILLE: u16 = RECALL_ANSWER_FLOOR_PERMILLE;

/// The challenger seat's agreement line (§4): four in five of its scored
/// comparisons must name the design the verification loop later vindicated.
/// The recall seat's budget, for the same reason — a seat that names the
/// winner four times in five is reading quality; one that names it half the
/// time is a coin this product would be spending money to flip.
pub const CHALLENGER_AGREEMENT_FLOOR_PERMILLE: u16 = RECALL_AGREEMENT_FLOOR_PERMILLE;

/// The wall past which a comparison is dropped, in milliseconds.
///
/// Nothing waits on it: the incumbent's design is what the attempt acts on,
/// and the score lands afterwards on a row already written. Two seconds is
/// therefore a staleness line rather than a wait — past it the attempt has
/// usually moved on and the row is better left unlabeled than labeled late.
pub const CHALLENGER_APPLY_DEADLINE_MS: u64 = 2_000;

/// Whether a model nobody has evidence for is worth what it costs to find
/// out — the challenger arm (user decision 2026-09-22, relayed m-6151).
///
/// Discovery moves a family alias to the newest release on its own
/// (`model_discovery`), and the router learns from outcomes on its own
/// (`model_router::learned`), but nothing in between ever tries a model the
/// catalog ranks below the incumbent. So a new release is either adopted
/// wholesale by an alias move or never measured at all, and "Fable outranks
/// Opus" stays a sentence in a shipped table rather than a thing this
/// machine has observed.
///
/// This seat closes that gap the way the pool seats close theirs. One in
/// [`CHALLENGER_ONE_IN`] eligible attempts asks the challenger for the same
/// design the incumbent is about to act on; the attempt acts on the
/// INCUMBENT's design either way, and the two — anonymized, so the judge
/// scores the plan rather than the name — are put to Jev as a closed
/// comparison. The label is quality: the verification loop's own verdict
/// where the attempt has one, and the anonymized comparison where it does
/// not. A finished attempt is never a win by itself
/// ([[agent-turn-done-is-not-task-verified]]).
///
/// What it sends is the request and two designs. What it decides is nothing
/// a person waits on: `shadow` records the score, `on` is the person's own
/// word, and `auto` lets a role's model move only once the seat's own
/// comparisons clear its line over a window and the challenger's Wilson
/// lower bound passes the incumbent's rate for THAT role ([`promote`]) —
/// which is also what takes it back down.
///
/// Held back on purpose: verification, review, the release lane and guarded
/// flows (payment, operator) never draw a challenger, and neither does a
/// retry or a handover — a second opinion is not what a retry needs.
pub const CHALLENGER: JevUse = JevUse {
    id: "challenger",
    setting: "jevChallenger",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/task",
            cap: Cap::Chars(CHALLENGER_TASK_CHAR_CAP),
        },
        Sent {
            at: "/state/designs",
            cap: Cap::Items(CHALLENGER_DESIGN_CAP),
        },
        Sent {
            at: "/state/designs/*/body",
            cap: Cap::Bytes(CHALLENGER_DESIGN_BYTE_CAP),
        },
    ],
    ledger: "challenger.jsonl",
    promotes: true,
    answer_floor_permille: Some(CHALLENGER_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(CHALLENGER_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(CHALLENGER_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Comparison,
};

/// Characters of the person's words one patch review reads as the task the
/// patch is for — the routing seat's cap, for the routing seat's reason: the
/// head of a request is what says what it asks, and a second number on the
/// same words would be a second answer to how much of a person's work leaves
/// the machine. Measured on the 1,673 patches this machine's zo sessions wrote
/// (147 transcripts, their vaults read too, 2026-09-23): the person's newest
/// words ran to 50 characters at the median — a pasted brief to 6,152 at p90
/// — and 2,000 held 87.3% of them whole.
pub const PATCH_REVIEW_TASK_CHAR_CAP: usize = ROUTING_TASK_CHAR_CAP;

/// Bytes of one patch's unified diff a review carries — its hunks with their
/// context lines, and nothing of the file around them. Measured on the same
/// 1,673 patches: the rendered hunks ran to 1,037 B at the median, 5,424 B at
/// p90 and 7,743 B at p95. 8 KiB holds 95.3% of them whole; past it a patch is
/// a rewrite, and its head is what a review of it can still read.
pub const PATCH_REVIEW_PATCH_BYTE_CAP: usize = 8 * 1024;

/// Bytes of evidence one review carries: the newest lines of the tool result
/// the edit followed — a test's output, a read, a search — kept from the end,
/// because a check prints its verdict last. Measured on the same 1,673
/// patches: that result ran to 1,399 B at the median and 4,627 B at p90; 4 KiB
/// holds 88.3% of them whole and the newest lines of the rest.
pub const PATCH_REVIEW_EVIDENCE_BYTE_CAP: usize = 4 * 1024;

/// How far each of a review's four answers must lean toward the side that lets
/// a patch stand, per thousand, before the code calls it a `permit` — the
/// probability that it addresses the task and that the evidence supports it
/// at least this, and that it carries unrelated changes or needed a question
/// first at most one minus this (t-6203).
///
/// A policy line and not a calibrated accuracy claim: it is the line the
/// reference harness this seat borrows its four questions from draws
/// (TypeSafeAI/jev-harness, "0.8, uncalibrated default"), written in this
/// table's units. It is not [`PATCH_REVIEW_ANSWER_FLOOR_PERMILLE`], which is a
/// line under how often the SEAT answers at all.
pub const PATCH_REVIEW_PERMIT_FLOOR_PERMILLE: u16 = 800;

/// What the patch review seat's answers must bound above before `auto` rises
/// to noting (§4): nine in ten. The orchestration seats' reasoning, reached
/// from this seat's own side: a review that does not come back costs the edit
/// nothing — it was written before the question left, and its result reads
/// as it did before the seat existed. Its own number, because two lines that
/// coincide are still two policies.
pub const PATCH_REVIEW_ANSWER_FLOOR_PERMILLE: u16 = 900;

/// The patch review seat's route-change budget (§4): four verdicts in five
/// must be the ones hindsight then gave ([`PATCH_REVIEW_REGRET_TURNS`]) — a
/// `permit` on a patch that stood, a `proposal_only` on one that was undone
/// or fixed again. Held to the same budget as every other seat that takes a
/// decision off a reader: a note that is wrong one time in five is a note a
/// model learns to read past.
pub const PATCH_REVIEW_AGREEMENT_FLOOR_PERMILLE: u16 = 800;

/// The wall a review may hold an edit's result, in milliseconds, when the seat
/// acts — the latency line the judge holds a rising seat to.
///
/// A tool result is what the model is waiting on, so the wall is a screen
/// question's: one request of four questions over one state, answered at the
/// wire's measured median of a few hundred milliseconds (280 ms for a screen
/// question, §2.1). Past it the result goes back as it would have without
/// the seat. A recording seat never holds the result at all: it asks beside
/// the turn and writes its row when the answer comes. Its own number, for the
/// reason every coinciding wall in this table is.
pub const PATCH_REVIEW_APPLY_DEADLINE_MS: u64 = 1_500;

/// Turns after a reviewed patch inside which editing the same lines of the
/// same file again — a fix of the fix, or an undo — counts as the patch's
/// regret (the seat's hindsight label, t-6203).
///
/// Five is a policy line, not a measured one, drawn the way
/// [`COMPACTION_REGRET_TURNS`] is: long enough that the turns which test a
/// change and repair it are inside it, short enough that the same lines
/// reworked an hour later for another reason are not charged to a review
/// that had nothing to do with it. A check that ran green after the turn's
/// last edit settles the label first — the harness's own receipt (r43).
pub const PATCH_REVIEW_REGRET_TURNS: u32 = 5;

/// zo's patch review: every patch an edit tool has just written — `edit_file`,
/// `write_file`, `MultiEdit`, anything whose result carries a structured
/// patch — put to four Noul questions before the model reads the result
/// (t-6203; the questions are TypeSafeAI/jev-harness's): does it address the
/// task, does the evidence support it, does it carry changes the task did not
/// ask for, should the agent have asked first. One code judgment reads the
/// four against [`PATCH_REVIEW_PERMIT_FLOOR_PERMILLE`]: `permit`,
/// `proposal_only`, or `unavailable` when nothing answered.
///
/// What is sent is the head of the person's newest words, the patch's hunks
/// with their context lines, the newest lines of the tool result the edit
/// followed, and the path's fingerprint ([`fingerprint_of`]) — never the
/// path, never the file around the hunks.
///
/// Nothing blocks. The edit is written before the question leaves: this
/// product is not a sandbox, and a patch it held back would be one the model
/// believes it made. Under a person's `on`, or an `auto` its own evidence
/// raised, a `proposal_only` adds one line to the result the model reads —
/// which question leaned the wrong way, how far, and what to do about it —
/// and the row carries the verdict. `shadow` asks beside the turn, records,
/// and changes neither the result nor its timing. `off` is today's result to
/// the byte.
///
/// The `agreed` rule is hindsight, one label per answered review: the patch
/// stood when a check ran green after the turn's last edit (the harness's
/// receipt, which settles it first) or when [`PATCH_REVIEW_REGRET_TURNS`]
/// turns passed without its lines being edited again; it was regretted when
/// an edit inside the window touched the same lines of the same file — a fix
/// of the fix, or an undo. A `permit` agreed when the patch stood, a
/// `proposal_only` when it was regretted.
pub const PATCH_REVIEW: JevUse = JevUse {
    id: "patch_review",
    setting: "jevPatchReview",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/task",
            cap: Cap::Chars(PATCH_REVIEW_TASK_CHAR_CAP),
        },
        Sent {
            at: "/state/patch",
            cap: Cap::Bytes(PATCH_REVIEW_PATCH_BYTE_CAP),
        },
        Sent {
            at: "/state/evidence",
            cap: Cap::Bytes(PATCH_REVIEW_EVIDENCE_BYTE_CAP),
        },
        // A fingerprint the product wrote, cleared like everything else the
        // door reads: declared so a reader of this table sees every key the
        // state carries.
        Sent {
            at: "/state/path",
            cap: Cap::Uncut,
        },
    ],
    ledger: "patch-review.jsonl",
    promotes: true,
    answer_floor_permille: Some(PATCH_REVIEW_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(PATCH_REVIEW_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(PATCH_REVIEW_APPLY_DEADLINE_MS),
    window_forgives: Some(FORGIVES_A_BAD_MINUTE),
    agreement_rows_wanted: Some(A_WINDOW_OF_COMPARISONS),
    agreement_kind: AgreementKind::Hindsight,
};

/// Every place this product asks Jev something.
pub static JEV_USES: [JevUse; 20] = [
    ROUTING,
    RECALL,
    SKILLS,
    BROWSER,
    DESKTOP,
    EMULATOR,
    STALL,
    PLACEMENT,
    SUMMON,
    STEP_EFFORT,
    ZO_STEP_EFFORT,
    COMPACTION,
    AGENT_TOOL,
    BROWSER_READ,
    NOTIFY,
    MENTION_RERANK,
    BRANCHING,
    JUDGMENT_CACHE,
    CHALLENGER,
    PATCH_REVIEW,
];

impl JevUse {
    /// Whether a validated screen choice meets this seat's press policy for
    /// a control of `kind`: the seat's own press floor for a plain one, and
    /// never less than [`SCREEN_DESTRUCTIVE_PRESS_FLOOR_PERMILLE`] for one a
    /// press cannot take back (t-6187). A seat with no press floor presses
    /// nothing, of either kind.
    #[must_use]
    pub fn permits_press(&self, confidence: f64, kind: crate::guarded::ControlKind) -> bool {
        self.press_floor_permille
            .map(|floor| match kind {
                crate::guarded::ControlKind::Plain => floor,
                crate::guarded::ControlKind::Destructive => {
                    floor.max(SCREEN_DESTRUCTIVE_PRESS_FLOOR_PERMILLE)
                }
            })
            .is_some_and(|floor| {
                (0.0..=1.0).contains(&confidence) && confidence >= f64::from(floor) / 1_000.0
            })
    }

    /// The mode `value` names for this use: one of this use's own words,
    /// trimmed, in any case. Anything else — a typo, a boolean, a mode this
    /// use does not offer — is `off`, so a slip never starts sending anything.
    #[must_use]
    pub fn mode_of(&self, value: Option<&Value>) -> JevMode {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .and_then(|word| {
                self.modes
                    .iter()
                    .copied()
                    .find(|mode| mode.key().eq_ignore_ascii_case(word))
            })
            .unwrap_or_default()
    }

    /// This use's mode in a settings document (`smart.<setting>`).
    #[must_use]
    pub fn mode_in(&self, root: &Value) -> JevMode {
        self.mode_of(
            root.get(SMART_SETTINGS_KEY)
                .and_then(|smart| smart.get(self.setting)),
        )
    }

    /// The mode a writer was handed, spelled exactly as this use offers it —
    /// a writer refuses what a reader would only have read as `off`.
    #[must_use]
    pub fn offered(&self, word: &str) -> Option<JevMode> {
        self.modes.iter().copied().find(|mode| mode.key() == word)
    }
}

/// The use named `id`.
#[must_use]
pub fn jev_use(id: &str) -> Option<&'static JevUse> {
    JEV_USES.iter().find(|row| row.id == id)
}

/// ---- the gate in front of the routing seat --------------------------------
///
/// The key under [`SMART_SETTINGS_KEY`] that decides how a spawn's difficulty
/// is classified — and, as a consequence nobody reading the routing row would
/// guess, whether the routing seat is asked anything at all.
///
/// The chain, read in zo's own source: the decision shadow is fired only by
/// `probe_and_shadow` (`smart_router/probe_exec.rs`), which is reached only
/// through `route_probe_assessment(s)`, which `smart_router/apply.rs` calls
/// only when this setting reads as [`ClassifierMode::Probed`]. So under the
/// other three words `smart.decisionShadow` may say `on` and there is nothing
/// the seat can ask — the switch a person CAN see promises a judgment the one
/// they cannot see has already refused.
///
/// It lives here rather than beside zo's own `RouteAutoClassifierMode` for the
/// reason this module exists: two programs now read it — zo to route, and the
/// window to put it on the card beside the seat it gates — and a word spelled
/// twice is a word that forks.
pub const CLASSIFIER_SETTING: &str = "autoClassifier";

/// How a spawn's difficulty is classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifierMode {
    /// Not classified at all; smart routing does not run.
    Off,
    /// The keyword tables alone — no provider is asked anything.
    Deterministic,
    /// The keyword tables, plus the lane and shape markers a person wrote into
    /// the task text. Still provider-free: it is a statement about whose words
    /// to trust, not about asking anybody.
    Assisted,
    /// The keyword tables, plus one bounded Fast-tier probe (~200 output
    /// tokens) whose verdict is fused on top of them — refining, never
    /// replacing. The only word under which the routing seat is asked.
    Probed,
}

impl ClassifierMode {
    /// Every mode, in the order the setting offers them.
    pub const ALL: [Self; 4] = [Self::Off, Self::Deterministic, Self::Assisted, Self::Probed];

    /// The word a settings file holds.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Deterministic => "deterministic",
            Self::Assisted => "assisted",
            Self::Probed => "probed",
        }
    }

    /// Whether the classifier runs at all.
    #[must_use]
    pub const fn runs(self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Whether markers a person wrote into the task text are read as evidence
    /// (`smart_router/evidence.rs`, deliberately this mode only).
    #[must_use]
    pub const fn markers(self) -> bool {
        matches!(self, Self::Assisted)
    }

    /// Whether a probe is called — which is also whether the routing seat is
    /// ever asked.
    #[must_use]
    pub const fn probes(self) -> bool {
        matches!(self, Self::Probed)
    }

    /// The mode `value` names: one of the four words, trimmed, in any case.
    ///
    /// An ABSENT value is [`Self::Probed`] and an unreadable one is
    /// [`Self::Deterministic`] — zo's own split (`smart_router/settings.rs`:
    /// "only its absence means probed", over
    /// `RouteAutoClassifierMode::from_settings_value`, whose unknown-word
    /// answer is deterministic). The two differ on purpose: nobody has chosen
    /// yet, versus somebody wrote something this reader could not honour.
    #[must_use]
    pub fn of(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::Probed;
        };
        value
            .as_str()
            .map(str::trim)
            .and_then(|word| {
                Self::ALL
                    .into_iter()
                    .find(|mode| mode.key().eq_ignore_ascii_case(word))
            })
            .unwrap_or(Self::Deterministic)
    }

    /// This setting's mode in a settings document (`smart.autoClassifier`).
    #[must_use]
    pub fn in_settings(root: &Value) -> Self {
        Self::of(
            root.get(SMART_SETTINGS_KEY)
                .and_then(|smart| smart.get(CLASSIFIER_SETTING)),
        )
    }

    /// The mode a writer was handed, spelled exactly as the setting offers it.
    #[must_use]
    pub fn offered(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.key() == word)
    }
}

/// A brief's shape: its head, cut to `cap`, and the length of the whole of
/// it.
///
/// One producer for every use that carries the head of somebody else's words,
/// so the cut a row records and the cut a question carries are the same cut,
/// and the count a reader compares them against is of the same thing.
#[must_use]
pub fn brief_shape(brief: &str, cap: Cap) -> (String, usize) {
    (door::cut(brief, cap), brief.chars().count())
}

/// The first sixteen hex digits of the SHA-256 of `words` — what a ledger
/// row carries in place of a string it must not carry (a page's path, a
/// walk's goal, t-6155 F8/F12), and what a question's rubric version is
/// pinned to. One producer, so two rows that name the same thing agree
/// exactly and a reader can group them without ever seeing the words.
#[must_use]
pub fn fingerprint_of(words: &str) -> String {
    hex_of(&Sha256::digest(words.as_bytes())[..FINGERPRINT_BYTES])
}

/// The bytes of a SHA-256 a fingerprint keeps: sixteen hex digits, enough to
/// tell apart everything one ledger names and short enough to read.
const FINGERPRINT_BYTES: usize = 8;

/// The whole SHA-256, in hex, of what one request was — the seat that asked,
/// the version of its rubric, the model it asked for, and the body's bytes as
/// the door let them through (after every withheld line and every cut): a
/// row's receipt (`requestDigest`, t-6203). Two rows carry the same digest
/// exactly when the wire was handed the same question, so a replay can check
/// a recorded answer against the request it answered without the row ever
/// holding the words.
///
/// The same hasher as [`fingerprint_of`], whole rather than cut: a
/// fingerprint names a thing to group rows by, a digest vouches for bytes.
/// Each part goes in behind its length (eight bytes, little-endian) — the
/// version as its decimal digits — so no two different tuples hash as one.
#[must_use]
pub fn digest_of(seat: &str, rubric_version: u32, model: &str, request: &[u8]) -> String {
    let version = rubric_version.to_string();
    let mut hasher = Sha256::new();
    for part in [
        seat.as_bytes(),
        version.as_bytes(),
        model.as_bytes(),
        request,
    ] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hex_of(&hasher.finalize())
}

/// Bytes as lowercase hex — the one spelling a fingerprint and a digest share.
fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// [`fingerprint_of`] a question's defining words — what a rubric version
/// is pinned to, so a word changed without a version bump is a red test
/// rather than a quiet drift.
#[must_use]
pub fn rubric_fingerprint(words: impl FnOnce() -> String) -> String {
    fingerprint_of(&words())
}

#[cfg(test)]
mod tests;
