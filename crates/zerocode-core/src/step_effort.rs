//! Which way a worker's effort should move between two of its turns
//! (t-5637): the rule that reads a turn, the question the window puts to Jev
//! beside it, what makes an answer one, and the label the next turn writes.
//!
//! The idea is vechen's Codex+Jev experiment (the vault's
//! `a-reasoning-effort-governor-changes-effort-per-step…`, 2026-09-21):
//! reasoning effort judged per step rather than once per turn — more when
//! stuck, less on routine steps. zo does that per request from inside. The
//! window reaches a Claude Code or Codex worker only at its composer, so its
//! unit is the turn: it reads the turn that just ended off the worker's own
//! transcript and moves the effort before the next one, through the door the
//! agent's row names (`crate::capabilities::TurnMoves`).
//!
//! What the rule reads is what the transcript already says (`crate::transcript
//! ::turns_in`): the same tool call made [`REPEATS_THAT_RAISE`] times running,
//! [`FAILURES_THAT_RAISE`] tool results marked as errors, a turn that touched
//! nothing. What it does not read is prose — a worker saying it is stuck is a
//! worker's claim; a worker doing the same thing four times is a fact.
//!
//! Nothing here touches a pane, a file, the clock or the network.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::capabilities::TurnMoves;
use crate::jev::STEP_EFFORT_REPEATED_CHAR_CAP;
use crate::jev::choice::{self, ChoiceRefusal};
use crate::stall_cause::Cause;
use crate::transcript::{TranscriptTurn, clamp};

/// The one question's name — the caller's key, never shown to the model.
const QUESTION: &str = "move";

/// The words of the question. The numbers and the repeated call reach the
/// model as state, never as an instruction.
const INSTRUCTIONS: &str = "A worker agent in a terminal has just ended a turn, and its next turn has not started. `effort` is the reasoning effort its last turn ran at and `floor` the effort it was summoned with (`null` when its CLI's default). `repeats` is how many times running its last turn made the very same tool call (`repeated` is that call), `toolFailures` how many of the turn's tool results were errors, `readOnly` whether the turn called no tool that can change a file or run a command, `stallCause` what the window's stall sweep last read into a silence of this worker (`null` when it saw none), and `raised` whether an earlier move already raised this worker's effort and it has not been brought back. Choose how the effort should move before the next turn.";

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 9] = [
    "agent",
    "effort",
    "floor",
    "raised",
    "repeats",
    "toolFailures",
    "readOnly",
    "stallCause",
    "repeated",
];

/// The version of the words in this module. Bump it when any of them changes:
/// a judgment read under one wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to [`crate::jev::rubric_fingerprint`].
pub const STEP_EFFORT_RUBRIC_VERSION: u32 = 1;

/// How many times running one turn must make the same tool call before the
/// rule calls it stuck — the vault page's shape (raise from the second
/// repeat) read against the window's coarser unit: a turn that made one call
/// twice is a retry, and three is a loop.
pub const REPEATS_THAT_RAISE: u32 = 3;

/// How many of a turn's tool results must be errors before the rule calls
/// it stuck.
pub const FAILURES_THAT_RAISE: u32 = 2;

/// How long after a move its label waits for the next turn to end; past the
/// window the label is `none`.
///
/// Two hours: the stall label's window as it was measured on 2026-09-17 —
/// twenty stall episodes on this machine's orchestration ledger were
/// followed within two hours for eighteen, p90 91 min — which this label
/// read until the stall label moved to four hours (2026-09-25, t-9087). It
/// keeps its own two because nothing measured moved it: the four hours were
/// read off what followed 104 silences, not off how long a worker's next
/// turn takes after an effort move, and this machine holds no step effort
/// row to read that from. The wait is part of what a label means — a move
/// whose next turn ends three hours on is `none` under two hours and graded
/// under four — so it moves only with [`STEP_EFFORT_RUBRIC_VERSION`] (astra
/// R-EFFORT-1), and every version 1 row was graded under two.
pub const STEP_EFFORT_LABEL_WINDOW_MS: i64 = 2 * 60 * 60 * 1_000;

/// The tool names, as each vendor's transcript spells them, that can change
/// a file or run a command — a turn that called none of them was a reading
/// turn. Claude Code's five (`Bash`, `Edit`, `Write`, `MultiEdit`,
/// `NotebookEdit`) and Codex's shell and patch calls (`shell`, `exec_command`,
/// `write_stdin`, `apply_patch`), read off this machine's transcripts and
/// rollouts (2026-09-21). A tool the list does not name reads as a reading
/// tool, which errs toward `Lower` only when the effort already stands above
/// the summons' word — never below it.
pub const WRITE_TOOLS: &[&str] = &[
    "Bash",
    "Edit",
    "Write",
    "MultiEdit",
    "NotebookEdit",
    "shell",
    "exec_command",
    "write_stdin",
    "apply_patch",
];

/// Which way the effort moves — the question's closed answer space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// One rung up.
    Raise,
    /// One rung down.
    Lower,
    /// Where it stands.
    Hold,
}

impl Move {
    /// Every move, in the order the question offers them.
    pub const ALL: [Self; 3] = [Self::Raise, Self::Lower, Self::Hold];

    /// The word a ledger row and the question use.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Raise => "raise",
            Self::Lower => "lower",
            Self::Hold => "hold",
        }
    }

    /// The criterion the question gives each move.
    const fn means(self) -> &'static str {
        match self {
            Self::Raise => {
                "The last turn was stuck — the same tool call over and over, or tool after tool failing — and more reasoning on the next turn is what would get it past that."
            }
            Self::Lower => {
                "The last turn was routine — reading, or plain steps that went through — or an earlier raise has done its work, and the next turn does not need the effort it stands at."
            }
            Self::Hold => {
                "Neither: the turn made progress at the effort it had, or what stopped it is not something more reasoning changes — a question waiting on a person, a quota wall, a login, a long tool still running."
            }
        }
    }

    /// The move `word` names, or `None` for a word the question never offered.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|held| held.word() == word)
    }
}

/// The words of the question, joined for [`crate::jev::rubric_fingerprint`].
#[must_use]
pub fn rubric_words() -> String {
    let mut words = String::from(INSTRUCTIONS);
    for mv in Move::ALL {
        words.push('\n');
        words.push_str(mv.word());
        words.push('\n');
        words.push_str(mv.means());
    }
    for key in STATE_KEYS {
        words.push('\n');
        words.push_str(key);
    }
    words
}

/// What one turn did, read off its tool calls and results.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Signals {
    /// The longest run of one tool call made the same way, back to back.
    pub repeats: u32,
    /// How many of the turn's tool results were marked errors.
    pub tool_failures: u32,
    /// Whether the turn called no tool in [`WRITE_TOOLS`].
    pub read_only: bool,
    /// The call the longest run was of, as the board draws it, when there
    /// was a run at all.
    pub repeated: Option<String>,
}

impl Signals {
    /// Whether these signals read as stuck: the loop or the failures.
    #[must_use]
    pub const fn stuck(&self) -> bool {
        self.repeats >= REPEATS_THAT_RAISE || self.tool_failures >= FAILURES_THAT_RAISE
    }
}

/// The signals of the LAST turn in `turns` — everything after the newest
/// user turn, or everything when nobody spoke. A tool call is "the same" as
/// the one before it when its name and its input are; a result is a failure
/// when the vendor marked it one.
#[must_use]
pub fn signals_of(turns: &[TranscriptTurn]) -> Signals {
    let start = turns
        .iter()
        .rposition(|turn| turn.role == "user")
        .map_or(0, |at| at + 1);
    let last = &turns[start..];
    let mut signals = Signals {
        read_only: true,
        ..Signals::default()
    };
    let mut run: u32 = 0;
    let mut previous: Option<(&str, &str)> = None;
    for turn in last {
        let Some(tool) = &turn.tool else {
            continue;
        };
        if turn.role == "tool_result" {
            if tool.is_error {
                signals.tool_failures += 1;
            }
            continue;
        }
        if WRITE_TOOLS.contains(&tool.name.as_str()) {
            signals.read_only = false;
        }
        let call = (tool.name.as_str(), tool.input.as_str());
        run = if previous == Some(call) { run + 1 } else { 1 };
        previous = Some(call);
        if run > signals.repeats {
            signals.repeats = run;
            signals.repeated = Some(clamp(&turn.text));
        }
    }
    if signals.repeats < 2 {
        signals.repeated = None;
    }
    signals
}

/// Where the worker's effort stands as the beat sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Standing<'a> {
    /// The effort its last turn ran at, off its transcript; `None` before
    /// the CLI has said.
    pub current: Option<&'a str>,
    /// The word it was summoned with; `None` for the CLI's default.
    pub floor: Option<&'a str>,
    /// An earlier raise stands and has not been brought back.
    pub raised: bool,
    /// What the stall sweep last read into this worker's silence, when it
    /// read one.
    pub stall_cause: Option<Cause>,
}

/// The rule's own move — the product's answer, which stands in for a
/// judgment that does not arrive, and which a `shadow` row is written
/// beside.
///
/// Up a rung when the turn was stuck and nothing already raised it; back down
/// when a raise stands and the turn went through; down a rung when a reading
/// turn ran above the summons' word; hold otherwise — and hold whatever the
/// turn did when the stall sweep named a cause more reasoning does not
/// change. One rung a time, never twice up, never below the floor: the rule
/// bounds what the beat may do, so a judgment that said `raise` four turns
/// running would still have moved the worker one rung.
#[must_use]
pub fn ruled(signals: &Signals, standing: &Standing<'_>, moves: &TurnMoves) -> Move {
    if standing
        .stall_cause
        .is_some_and(|cause| !more_reasoning_helps(cause))
    {
        return Move::Hold;
    }
    if standing.raised {
        return if signals.stuck() {
            Move::Hold
        } else {
            Move::Lower
        };
    }
    if signals.stuck() {
        return Move::Raise;
    }
    let above_floor = match (standing.current, standing.floor) {
        (Some(current), Some(floor)) => moves.stands_above(current, floor),
        _ => false,
    };
    if signals.read_only && above_floor {
        Move::Lower
    } else {
        Move::Hold
    }
}

/// Whether a silence read as `cause` is one more reasoning on the next turn
/// changes. Only a silence nobody could name is: every named cause is a
/// wall, a question, a person, a tool or a finished job.
#[must_use]
pub const fn more_reasoning_helps(cause: Cause) -> bool {
    matches!(cause, Cause::Unknown)
}

/// The move a row is about, as the beat found it.
#[derive(Debug, Clone, Copy)]
pub struct StepLook<'a> {
    /// The agent's catalog id.
    pub agent: &'a str,
    pub signals: &'a Signals,
    pub standing: &'a Standing<'a>,
}

/// One question, ready for the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct StepAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`.
    pub questions: Value,
}

/// A validated answer.
#[derive(Debug, Clone, PartialEq)]
pub struct StepChoice {
    pub chosen: Move,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// The question this turn asks.
#[must_use]
pub fn ask(look: &StepLook<'_>) -> StepAsk {
    let mut criteria = Map::new();
    for mv in Move::ALL {
        criteria.insert(mv.word().to_string(), Value::String(mv.means().to_string()));
    }
    let repeated = look.signals.repeated.as_deref().map(|call| {
        call.chars()
            .take(STEP_EFFORT_REPEATED_CHAR_CAP)
            .collect::<String>()
    });
    StepAsk {
        state: json!({
            STATE_KEYS[0]: look.agent,
            STATE_KEYS[1]: look.standing.current,
            STATE_KEYS[2]: look.standing.floor,
            STATE_KEYS[3]: look.standing.raised,
            STATE_KEYS[4]: look.signals.repeats,
            STATE_KEYS[5]: look.signals.tool_failures,
            STATE_KEYS[6]: look.signals.read_only,
            STATE_KEYS[7]: look.standing.stall_cause.map(Cause::word),
            STATE_KEYS[8]: repeated,
        }),
        questions: choice::asked(QUESTION, INSTRUCTIONS, criteria),
    }
}

impl StepAsk {
    /// What the endpoint's `answers` map says about this question. One broken
    /// rule discards the answer whole.
    ///
    /// # Errors
    ///
    /// [`ChoiceRefusal`] names which rule the answer broke.
    pub fn read(&self, answers: &Value) -> Result<StepChoice, ChoiceRefusal> {
        let offered: BTreeSet<String> = Move::ALL.iter().map(|mv| mv.word().to_string()).collect();
        let choice = choice::read(answers, QUESTION, &offered)?;
        Ok(StepChoice {
            chosen: Move::from_word(&choice.chosen).ok_or(ChoiceRefusal::UnknownOption)?,
            probabilities: choice.probabilities,
            confidence: choice.confidence,
        })
    }
}

/// What the turn after a move did — the label a move is graded against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Followed {
    /// The next turn ended without the stuck shape.
    Progressed,
    /// The next turn ended stuck again.
    Stuck,
    /// No turn ended within [`STEP_EFFORT_LABEL_WINDOW_MS`].
    Nothing,
}

impl Followed {
    /// The word a label row keeps.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Progressed => "progressed",
            Self::Stuck => "stuck",
            Self::Nothing => "none",
        }
    }

    /// What the next turn's signals say it did.
    #[must_use]
    pub const fn of(next: &Signals) -> Self {
        if next.stuck() {
            Self::Stuck
        } else {
            Self::Progressed
        }
    }
}

/// The word an effort move's label carries under the summary's `notCompared`
/// when the move never reached the request or the composer: the seat was
/// recording, the host held it back, or the door withheld it.
pub const NOT_CARRIED: &str = "not_carried";

/// The word an effort move's label carries under the summary's `notCompared`
/// when the seat's answer moved nothing the rule would not have moved.
pub const SAME_AS_RULE: &str = "same_as_rule";

/// The mark an effort move earns from what came after it — the one rule both
/// effort seats label by (t-6342): the window's, between two turns of a
/// worker, and zo's step governor, between two requests of a turn.
///
/// "What came after went through" is not a label by itself. On zo's ledger
/// 522 of 547 steps after a judgment progressed whatever the judgment said
/// (2026-09-23), and every one of the 440 judgments that differed from the
/// governor's table was held back on an Anthropic wire, so none of them moved
/// anything the next step could answer for. A move is graded only where the
/// answer had an effect to grade: it moved the effort away from the rule's
/// own move, and the move was carried. Where it had none the row names why
/// and carries no mark, which is how a seat with no such moves reads as a
/// seat with no label rather than one that is always right.
///
/// # Errors
///
/// [`NOT_CARRIED`] for a move that never reached the request or the
/// composer, and [`SAME_AS_RULE`] for one the rule would have made anyway.
pub fn move_mark(
    seat_moved_it: bool,
    carried: bool,
    progressed: bool,
) -> Result<bool, &'static str> {
    if !carried {
        return Err(NOT_CARRIED);
    }
    if !seat_moved_it {
        return Err(SAME_AS_RULE);
    }
    Ok(progressed)
}

#[cfg(test)]
mod tests;
