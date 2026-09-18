//! Jev — TypeSafe's System One — as this product asks it: the one table of
//! every place that asks (docs/design/jev-settings-20260917.md §2).
//!
//! Two programs ask. zo asks about a task it is routing and about the notes a
//! recall found; the window asks about a browser walk that stopped and about a
//! worker whose pane went quiet. Each keeps
//! its own wire — the two Cargo workspaces carry different `reqwest` majors
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

/// Controls one stopped walk's question offers. The numbers themselves are
/// capped at 99 by the look; a choice with ninety-nine options is not a
/// question worth asking.
pub const BROWSER_CANDIDATE_CAP: usize = 12;

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

    /// Whether a use in this mode acts on what it is told. `auto` does not:
    /// the judge that would promote it is a later stage (§4, J4).
    #[must_use]
    pub const fn applies(self) -> bool {
        matches!(self, Self::On)
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
}

/// zo's routing judgment: a task's complexity, risk and intent beside the
/// chat probe's (docs/design/jev-decision-shadow-20260917.md).
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
};

/// zo's recall rerank: how much each note a recall found helps with the
/// request (docs/design/typesafe-judgment-expansion-20260917.md).
///
/// `on` is the apply stage: the judgment's order, after the vault's graph has
/// had its say, is the order the turn reads. It stays a person's choice —
/// `promotes` is false — because the 887 answered readings this machine has
/// recorded say the judgment moves something on nearly every recall (a median
/// of 7 notes reordered, the first note changed in 73% of them), and a use
/// that changes that much of what a turn reads is not one evidence should
/// switch on by itself.
///
/// The apply also LEAVES OUT the notes the judgment put on its bottom level —
/// *nothing in it bears on the request* — which the same ledger says is 24.5%
/// of what a turn was handed, and on 5 of those 74 recalls was all of it. The
/// rule that survives both is the graph's, not the judgment's: a page the vault
/// marked superseded or contradicted is never dropped, and a judgment that
/// cannot account for every page recall admitted is one recall's own order
/// outlives (`runtime::memory::rerank`).
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
    promotes: false,
};

/// The window's browser recovery: which numbered control on a stopped walk's
/// screen to press (docs/design/jev-browser-action-20260917.md). Pressing is
/// always a person's choice, so `auto` never rises to it.
pub const BROWSER: JevUse = JevUse {
    id: "browser",
    setting: "browserAction",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::On, JevMode::Auto],
    sends: &[
        Sent {
            at: "/state/goal",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/refusal",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/page/host",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/page/path",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/state/candidates",
            cap: Cap::Items(BROWSER_CANDIDATE_CAP),
        },
        Sent {
            at: "/state/candidates/*",
            cap: Cap::Uncut,
        },
        Sent {
            at: "/questions/*/criteria/*",
            cap: Cap::Uncut,
        },
    ],
    ledger: "browser-action.jsonl",
    promotes: false,
};

/// The window's stall sweep: why a quiet worker stopped when the measured
/// marker table cannot say (`crate::stall_cause`, t-4538). Nothing acts on
/// the answer yet — it is a row beside what the coordinator then did, and
/// those rows are the labels a later stage would promote on — so the use
/// offers no mode that applies, and `auto` records.
pub const STALL: JevUse = JevUse {
    id: "stall",
    setting: "stallCause",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::Auto],
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
    promotes: false,
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
/// Recording only, and the rule keeps placing every worker. The rows are what
/// a later stage would promote on, and unlike routing the labels cost nobody
/// an afternoon: what the person did with the worker's window in the seconds
/// after it appeared — closed it, moved it, never looked — is the answer to
/// the question that was asked.
pub const PLACEMENT: JevUse = JevUse {
    id: "placement",
    setting: "workerPlacement",
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::Auto],
    sends: &[Sent {
        at: "/state/brief",
        cap: Cap::Chars(PLACEMENT_BRIEF_CHAR_CAP),
    }],
    ledger: "worker-placement.jsonl",
    promotes: false,
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
    modes: &[JevMode::Off, JevMode::Shadow, JevMode::Auto],
    sends: &[Sent {
        at: "/state/brief",
        cap: Cap::Chars(SUMMON_BRIEF_CHAR_CAP),
    }],
    ledger: "summon-choice.jsonl",
    promotes: false,
};

/// Every place this product asks Jev something.
pub static JEV_USES: [JevUse; 6] = [ROUTING, RECALL, BROWSER, STALL, PLACEMENT, SUMMON];

impl JevUse {
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

/// The first sixteen hex digits of the SHA-256 of a question's defining
/// words — what a question's rubric version is pinned to, so a word changed
/// without a version bump is a red test rather than a quiet drift.
#[must_use]
pub fn words_fingerprint(words: &str) -> String {
    Sha256::digest(words.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
