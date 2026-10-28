//! A pane's effort as the composer's chip shows it, and the keys a person's
//! press on that chip sends (t-81532).
//!
//! The word shown is the one the session itself last wrote down
//! ([`crate::transcript::effort_in`]: Claude Code's `perTurnEffort`, Codex's
//! `turn_context.effort`), then the effort the ledger's summons asked for,
//! then nothing — the CLI's own default, which the window does not guess.
//!
//! A press moves the effort down the same road the step-effort seat takes
//! (`crate::capabilities::TurnMoves::effort`): Claude Code's picker, one
//! arrow a rung and `s` for this session only — never `/effort <level>`,
//! which saves the word as the account's default for every later launch. A
//! road that types nothing at a running session (Codex, Gemini CLI), one whose
//! keys nobody measured (zo's `/model` picker) and no road at all are shown
//! and never pressed, each with its reason.
//!
//! Nothing here touches a pane, a file, the clock or the network.

use serde::Serialize;

use crate::capabilities::{MoveRoad, PickerKeys, TurnMoves};

/// Where the word on the chip came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffortWitness {
    /// The session's own record of the turn it last ran.
    Turn,
    /// The effort the ledger's summons asked for — no turn has said yet.
    Summons,
    /// Neither: the CLI runs at its own default.
    Default,
}

/// Why a press cannot move the effort now. The window says each in the
/// person's language; the codes are what it keys them by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stuck {
    /// Nobody measured how this CLI's effort moves between turns.
    NoRoad,
    /// Only a fresh launch sets it: nothing typed at a running session does.
    Relaunch,
    /// Its picker lives on its own screen with keys nobody measured.
    OnScreen,
    /// The session has not said which rung it stands on, so the number of
    /// rungs to the word the person picked is unknown.
    Unknown,
    /// The session stands on a word the ladder does not know.
    OffLadder,
    /// A turn is under way.
    TurnRunning,
    /// The pane never said where its turn stands.
    NeverHeard,
    /// The person stopped the last turn by hand: the pane is theirs.
    HeldByPerson,
    /// A shell is in front of the agent.
    ShellInFront,
    /// Words the person typed wait on the pane's own line.
    Draft,
    /// The step-effort seat is moving this pane's effort itself.
    SeatMoving,
}

/// What the chip shows and whether a press can move it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffortChip {
    /// The word the session stands on, when anyone has said.
    pub current: Option<String>,
    pub witness: EffortWitness,
    /// The rungs a press can pick, lowest first.
    pub ladder: &'static [&'static str],
    /// The row's word for its road (`picker`, `relaunch`, …), when it has one.
    pub door: Option<&'static str>,
    /// The command a CLI whose picker lives on its own screen opens it with.
    pub command: Option<&'static str>,
    /// `None` when a press moves the effort now.
    pub stuck: Option<Stuck>,
    /// Whether a move rewrites the conversation's prompt cache — the line
    /// the person reads before the press goes through.
    pub rewrites_cache: bool,
    /// The word a person's press moved the session to, until a turn has run
    /// at it and the session's own record says so.
    pub pending: Option<String>,
}

/// The word to show and where it came from: the session's own record first,
/// the summons' request second, the CLI's default (no word) last.
#[must_use]
pub fn shown<'a>(
    turn: Option<&'a str>,
    summons: Option<&'a str>,
) -> (Option<&'a str>, EffortWitness) {
    let said = |word: Option<&'a str>| word.map(str::trim).filter(|word| !word.is_empty());
    match (said(turn), said(summons)) {
        (Some(word), _) => (Some(word), EffortWitness::Turn),
        (None, Some(word)) => (Some(word), EffortWitness::Summons),
        (None, None) => (None, EffortWitness::Default),
    }
}

/// Why this row's road cannot carry a press at all, whatever the pane is
/// doing — `None` for a road it can.
#[must_use]
pub const fn road_stuck(moves: Option<&TurnMoves>) -> Option<Stuck> {
    match moves {
        None => Some(Stuck::NoRoad),
        Some(moves) => match moves.effort {
            None => Some(Stuck::NoRoad),
            Some(MoveRoad::Relaunch) => Some(Stuck::Relaunch),
            Some(MoveRoad::Shown(_)) => Some(Stuck::OnScreen),
            Some(MoveRoad::Picker(_) | MoveRoad::Line(_)) => None,
        },
    }
}

/// The chip for one pane: the shown word, then the first reason a press
/// cannot move it — the road's, which no wait changes, before the session's
/// word, before the pane's moment (`at_rest`, the window's own reading).
/// `pending` is dropped once the session's record has caught up with it.
#[must_use]
pub fn chip(
    moves: Option<&TurnMoves>,
    turn: Option<&str>,
    summons: Option<&str>,
    pending: Option<&str>,
    at_rest: Result<(), Stuck>,
) -> EffortChip {
    let (current, witness) = shown(turn, summons);
    let pending = pending.filter(|word| Some(*word) != turn.map(str::trim));
    let ladder = moves.map_or(&[][..], |moves| moves.ladder);
    let stands = pending.or(current);
    let stuck = road_stuck(moves)
        .or_else(|| match stands {
            None => Some(Stuck::Unknown),
            Some(word) if !ladder.contains(&word) => Some(Stuck::OffLadder),
            Some(_) => None,
        })
        .or(at_rest.err());
    EffortChip {
        current: current.map(str::to_string),
        witness,
        ladder,
        door: moves.and_then(|moves| moves.effort).map(MoveRoad::door),
        command: match moves.and_then(|moves| moves.effort) {
            Some(MoveRoad::Shown(command)) => Some(command),
            _ => None,
        },
        stuck,
        rewrites_cache: moves.is_some_and(|moves| moves.rewrites_cache),
        pending: pending.map(str::to_string),
    }
}

/// What a press from `from` to `to` types: the line, or the picker's command
/// and — once its legend is on the screen — the keys after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PressKeys {
    /// One line at the resting composer.
    Line(String),
    /// The picker's command, then one arrow a rung and its this-session key.
    Picker {
        keys: PickerKeys,
        after_legend: Vec<&'static str>,
    },
}

/// The keys that move a session standing on `from` to `to`, or why none do.
/// `Ok(None)` is a press onto the rung the session already stands on.
///
/// # Errors
///
/// The road cannot carry a press ([`road_stuck`]), or either word is off the
/// ladder.
pub fn press(moves: &TurnMoves, from: &str, to: &str) -> Result<Option<PressKeys>, Stuck> {
    if let Some(stuck) = road_stuck(Some(moves)) {
        return Err(stuck);
    }
    let rung = |word: &str| moves.ladder.iter().position(|held| *held == word);
    let (Some(at), Some(goal)) = (rung(from), rung(to)) else {
        return Err(Stuck::OffLadder);
    };
    if at == goal {
        return Ok(None);
    }
    match moves.effort {
        Some(MoveRoad::Picker(keys)) => {
            let arrow = if goal > at { keys.raise } else { keys.lower };
            let mut after_legend = vec![arrow; at.abs_diff(goal)];
            after_legend.push(keys.session_only);
            Ok(Some(PressKeys::Picker { keys, after_legend }))
        }
        Some(road @ MoveRoad::Line(_)) => road
            .line(to)
            .map(PressKeys::Line)
            .map(Some)
            .ok_or(Stuck::NoRoad),
        Some(MoveRoad::Shown(_) | MoveRoad::Relaunch) | None => Err(Stuck::NoRoad),
    }
}

#[cfg(test)]
mod tests;
