//! Jev — TypeSafe's System One — as this product asks it: the one table of
//! every place that asks (docs/design/jev-settings-20260917.md §2).
//!
//! Two programs ask. zo asks about a task it is routing and about the notes a
//! recall found; the window asks which control a walk should press next — on a
//! page or on the desktop — about a worker whose pane went quiet, about where
//! a worker's window belongs and about which agent a summons should start.
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

pub mod choice;
pub mod count;
pub mod door;
pub mod hedge;
pub mod promote;
pub mod recent;
pub mod shard;
pub mod summary;

/// The object zo's settings keep every Jev switch under.
pub const SMART_SETTINGS_KEY: &str = "smart";

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
    /// Minimum confidence for a screen press, distinct from the answer-rate
    /// promotion floor. None means this seat has no authority to press.
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
};

/// The window's worker placement: which of [`PLACEMENT_OPTIONS`] a worker it
/// just started belongs in.
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
    answer_floor_permille: Some(ORCHESTRATION_ANSWER_FLOOR_PERMILLE),
    press_floor_permille: None,
    agreement_floor_permille: Some(ORCHESTRATION_AGREEMENT_FLOOR_PERMILLE),
    apply_deadline_ms: Some(PLACEMENT_APPLY_DEADLINE_MS),
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
};

/// Every place this product asks Jev something.
pub static JEV_USES: [JevUse; 11] = [
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
];

impl JevUse {
    /// Whether a validated screen choice meets this seat's press policy.
    #[must_use]
    pub fn permits_press(&self, confidence: f64) -> bool {
        self.press_floor_permille.is_some_and(|floor| {
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

/// The first sixteen hex digits of the SHA-256 of a question's defining
/// words — what a question's rubric version is pinned to, so a word changed
/// without a version bump is a red test rather than a quiet drift.
#[must_use]
pub fn rubric_fingerprint(words: impl FnOnce() -> String) -> String {
    Sha256::digest(words().as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
