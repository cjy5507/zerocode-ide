//! Why a quiet worker stopped, when the measured marker table cannot say
//! (docs/design/jev-settings-20260917.md §2, t-4538): the question the
//! window's stall sweep asks Jev about one silence, what makes an answer one,
//! and what happened after — the label an answer is later graded against.
//!
//! The marker table (`crates/zerocode-shell/src/quota_wall.rs`) names three
//! causes from an agent's own words, measured one CLI at a time: its quota
//! wall, a transient API error and a safety classifier's decline. Every other
//! silence is `went_quiet` news, and a coordinator reads the pane to learn
//! why. This module puts that same reading to Jev as a closed choice — the
//! three measured causes (in words the table has not measured, or before the
//! table's two witnesses agree), five more ways a worker's silence reads on
//! this machine, and `unknown` — and says what came of the silence
//! afterwards. The answer is a label and never a witness: a decline is told
//! on the table's two witnesses alone (t-6747).
//! Under `on`, or an `auto` its own evidence raised, the seat acts on the
//! answer (`crate::jev::STALL`, docs/design/jev-every-seat-acts-20260920.md).
//!
//! Every example phrase in the criteria below was read off this machine's own
//! records on 2026-09-17, never written from memory of what a CLI prints: the
//! released worker screens the orchestration ledger archives (348), the Claude
//! transcripts under both project roots, and the Codex rollouts the marker
//! table was measured on. Nothing here touches the network, the clock, a pane
//! or a file.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::jev::choice::{self, ChoiceRefusal};
use crate::jev::door::newest_from;
use crate::jev::{STALL_SCREEN_BYTE_CAP, STALL_TRANSCRIPT_BYTE_CAP};
use crate::orchestration::{MessageKind, Run, worker_address};

/// The one question's name. The endpoint does not show a question's name to
/// the model, so this is the caller's key and nothing more.
const QUESTION: &str = "cause";

/// The words of the question. The screen and the record reach the model as
/// state, never as an instruction — each a list of its own parts, so what
/// the judgment reads is the shape the pane had and no entry is a text to
/// split (t-9469).
const INSTRUCTIONS: &str = "A worker agent running in a terminal has printed nothing for `quietSeconds` seconds, and nothing the window has measured for a quota wall, a transient API error or a safety classifier's decline ends its conversation. `agent` names its program. `screen` is the bottom of its terminal as it stands now, one entry a line, top to bottom, and `transcript` the last records of its conversation, oldest first, each its `role` and its `words`. Choose why the worker stopped.";

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 4] = ["agent", "quietSeconds", "screen", "transcript"];

/// The keys of one turn of `transcript`, in the order the fingerprint reads
/// them: who spoke, and what was said.
const TURN_KEYS: [&str; 2] = ["role", "words"];

/// The version of the words in this module. Bump it when any of them changes:
/// a judgment read under one wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to [`crate::jev::rubric_fingerprint`].
///
/// Or when the label they are graded by changes: version 4 asks version 3's
/// words and waits [`STALL_LABEL_WINDOW_MS`]'s four hours for what followed,
/// where version 3's labels were cut at two (t-9087). Or when what the
/// question reads changes shape: version 5 carries the screen as a list of
/// its lines and the record as a list of turns, each its `role` and its
/// `words`, where version 4 carried each as one text (t-9469). The version
/// rides every request row, so a reader can tell the series apart; reading
/// them apart is t-6877's contract.
pub const STALL_CAUSE_RUBRIC_VERSION: u32 = 5;

/// How long after a silence was asked about its label waits for what
/// followed ([`followed`]).
///
/// Four hours, measured on this machine's orchestration ledger over the 104
/// silences the stall seat answered in the week to 2026-09-25 (t-9087): every
/// one was followed by a coordinator's mail, a report, a stop or the pane's
/// death within 3.95 hours — p50 46 min, p75 1.7 h, p90 2.7 h, p95 3.3 h. The
/// two hours drawn from twenty episodes on 2026-09-17 (p90 91 min) closed on
/// 19 of the 104 first, and a window that closes before the answer arrives
/// writes `none` for it: eight `finished_without_report` answers the
/// coordinator's mail or a stop then proved right were marked wrong, and
/// eleven `long_running_tool` ones were marked right only because the cut
/// came before each worker's own report. Re-marked with the same table on the
/// same 104 silences, two hours agreed 81 of 95 (lower bound 767‰) and four
/// hours 85 of 91 (863‰). Past the window the label is `none`: whatever came
/// later answered something other than that silence.
pub const STALL_LABEL_WINDOW_MS: i64 = 4 * 60 * 60 * 1_000;

/// Why a quiet worker stopped — the question's closed answer space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    TransientApiError,
    QuotaWall,
    AuthFailure,
    ClassifierDecline,
    WaitingOnOwnCliQuestion,
    FinishedWithoutReport,
    LongRunningTool,
    HumanTookOver,
    Unknown,
}

impl Cause {
    /// Every cause, in the order the question offers them.
    pub const ALL: [Self; 9] = [
        Self::TransientApiError,
        Self::QuotaWall,
        Self::AuthFailure,
        Self::ClassifierDecline,
        Self::WaitingOnOwnCliQuestion,
        Self::FinishedWithoutReport,
        Self::LongRunningTool,
        Self::HumanTookOver,
        Self::Unknown,
    ];

    /// The option's name — the word a ledger row keeps.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::TransientApiError => "transient_api_error",
            Self::QuotaWall => "quota_wall",
            Self::AuthFailure => "auth_failure",
            Self::ClassifierDecline => "classifier_decline",
            Self::WaitingOnOwnCliQuestion => "waiting_on_own_cli_question",
            Self::FinishedWithoutReport => "finished_without_report",
            Self::LongRunningTool => "long_running_tool",
            Self::HumanTookOver => "human_took_over",
            Self::Unknown => "unknown",
        }
    }

    /// What the option means, written as the situation a screen and a record
    /// show, with the words this machine's records show it in.
    ///
    /// - transient: w-4525's Claude transcript (2026-09-17, the marker
    ///   table's `CLAUDE_TRANSIENT_RECORD`) and two Codex rollout edges
    ///   (`server_overloaded`, and `other` opening with the stream sentence).
    /// - wall: two released Claude screens — the second is a limit the marker
    ///   table has no row for — and Codex's own sentence.
    /// - login: an opencode worker's pane (`Token refresh failed: 401`,
    ///   w-5479, 2026-09-20, t-5498) and a Claude turn's error in the same
    ///   verification (`Refresh token not found or invalid`, t-5499) — the
    ///   silence the first rubric read as `unknown` with 0.95.
    /// - decline: the 18 Claude records of a decline no fallback answered
    ///   (2026-09-10..24, t-6747: Fable 5.1's 14 and Opus 5's 4), and the
    ///   pause dialog a coordinator read off w-5770's pane on 2026-09-21 —
    ///   the person's `switchModelsOnFlag: false`, which writes nothing until
    ///   a key answers it. A box, but no question of the work's: named here
    ///   so it is not read as one.
    /// - question: a Claude question box as a coordinator read it off a pane
    ///   (transcript `e4d2cde0-…`), and `AskUserQuestion` calls in 31 Claude
    ///   transcripts.
    /// - finished: released Claude and Codex screens — the turn footers and
    ///   the empty composers — and the report command as a Codex worker ran it.
    /// - tool: a released Codex screen, and a released Claude screen whose
    ///   footer names a shell still running.
    /// - person: 139 and 22 records of the two interruptions in Claude
    ///   transcripts.
    const fn means(self) -> &'static str {
        match self {
            Self::TransientApiError => {
                "The last turn ended on a provider or connection failure that the next request may not meet, in words like `API Error: The response stopped arriving. The response above may be incomplete.`, `stream disconnected before completion` or `Selected model is at capacity. Please try a different model.`"
            }
            Self::QuotaWall => {
                "The provider refuses more work until a limit resets or is raised, in words like `You've hit your session limit · resets 2:10am (Asia/Seoul)`, `You've hit your monthly spend limit. Run /usage-credits to manage your limit` or `You've hit your usage limit.`"
            }
            Self::AuthFailure => {
                "The agent's login to its provider no longer works, so every request is refused until somebody signs it in again, in words like `Token refresh failed: 401`, `Refresh token not found or invalid`, `401 Unauthorized`, `authentication_error`, `Not logged in`, or a prompt to run its own login command. Waiting, retrying and typing a continuation do not help: another agent, or a person, has to take the work."
            }
            Self::ClassifierDecline => {
                "The provider's safety classifier declined the conversation's last request, and the turn ended on it or waits on it: `API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup).` as the last record, or a box over the conversation reading `Session paused`, `Details: [cyber]`, `1. Switch to Opus 4.8` and `2. Edit prompt and retry`, which waits for a key and is not a question the work asked. Sending the same request to the same model usually meets the same decline: another model, or a person, has to take the turn."
            }
            Self::WaitingOnOwnCliQuestion => {
                "The agent's own program has a question or a confirmation on the screen and waits for a key: a box like `Do you want to continue?` over `1. Yes` and `2. No` with `Enter to select · ↑/↓ to navigate · Esc to cancel`, or an `AskUserQuestion` call as the conversation's last record with no answer after it."
            }
            Self::FinishedWithoutReport => {
                "The work reads as finished — a closing summary, a turn footer like `✻ Worked for 27m 0s · done 2:11 AM` or `─ Worked for 8m 43s ─`, and an empty composer (`❯`, `› Ask Codex to do anything`) — but no `zerocode-orc send --type worker_done` ran after the summary."
            }
            Self::LongRunningTool => {
                "A command the agent started is still running and the agent is waiting on it, as in `• Working (19m 43s • esc to interrupt) · 1 background terminal running` or a turn footer ending `· 1 shell still running`."
            }
            Self::HumanTookOver => {
                "A person stopped the turn or took the pane: `[Request interrupted by user]` or `[Request interrupted by user for tool use]` is the conversation's last record, or a person's own words follow the agent's."
            }
            Self::Unknown => "Neither the screen nor the record shows why the worker stopped.",
        }
    }

    /// The cause an option names, if it names one.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.word() == word)
    }
}

/// The words that define the question, as one string. The version is pinned
/// to this, not to a date or to a reviewer's memory.
#[must_use]
pub fn rubric_words() -> String {
    let mut words = String::from(INSTRUCTIONS);
    for cause in Cause::ALL {
        words.push('\n');
        words.push_str(cause.word());
        words.push('\n');
        words.push_str(cause.means());
    }
    words.push('\n');
    words.push_str(&STATE_KEYS.join(","));
    words.push('\n');
    words.push_str(&TURN_KEYS.join(","));
    words
}

/// The silence a question is about, as the window found it.
#[derive(Debug, Clone, Copy)]
pub struct StallLook<'a> {
    /// The agent's catalog id (`claude`, `codex`, `zo`, …).
    pub agent: &'a str,
    /// How long the pane has printed nothing.
    pub quiet_ms: i64,
    /// The pane's visible screen, top to bottom.
    pub screen: &'a str,
    /// The bounded tail of the agent's transcript, oldest line first.
    pub transcript: &'a [String],
}

/// One question, ready for the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct StallAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`.
    pub questions: Value,
}

/// A validated answer.
#[derive(Debug, Clone, PartialEq)]
pub struct StallChoice {
    pub cause: Cause,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// The question this silence asks, or `None` when there is nothing to read —
/// a screen with no words and a transcript with no turns say nothing a
/// judgment could.
#[must_use]
pub fn ask(look: &StallLook<'_>) -> Option<StallAsk> {
    let screen = screen_tail(look.screen);
    let transcript = transcript_tail(look.transcript);
    if screen.is_empty() && transcript.is_empty() {
        return None;
    }
    let mut criteria = Map::new();
    for cause in Cause::ALL {
        criteria.insert(
            cause.word().to_string(),
            Value::String(cause.means().to_string()),
        );
    }
    Some(StallAsk {
        state: json!({
            STATE_KEYS[0]: look.agent,
            STATE_KEYS[1]: look.quiet_ms.max(0) / 1_000,
            STATE_KEYS[2]: screen,
            STATE_KEYS[3]: transcript,
        }),
        questions: choice::asked(QUESTION, INSTRUCTIONS, criteria),
    })
}

impl StallAsk {
    /// What the endpoint's `answers` map says about this question. One broken
    /// rule discards the answer whole.
    ///
    /// # Errors
    ///
    /// [`ChoiceRefusal`] names which rule the answer broke.
    pub fn read(&self, answers: &Value) -> Result<StallChoice, ChoiceRefusal> {
        let offered: BTreeSet<String> = Cause::ALL
            .iter()
            .map(|cause| cause.word().to_string())
            .collect();
        let choice = choice::read(answers, QUESTION, &offered)?;
        Ok(StallChoice {
            cause: Cause::from_word(&choice.chosen).ok_or(ChoiceRefusal::UnknownOption)?,
            probabilities: choice.probabilities,
            confidence: choice.confidence,
        })
    }
}

/// The bottom of a terminal's screen as the question carries it, one entry a
/// line: trailing blanks off each line and blank lines off the bottom, then
/// the newest lines that fit [`STALL_SCREEN_BYTE_CAP`] once the door has
/// cleared them ([`newest_from`]) — the cap a whole screen's text was held to
/// when it went as one, so the same lines go.
#[must_use]
pub fn screen_tail(screen: &str) -> Vec<String> {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    let end = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(0, |at| at + 1);
    let lines = &lines[..end];
    lines[newest_from(lines, STALL_SCREEN_BYTE_CAP)..]
        .iter()
        .map(|line| (*line).to_string())
        .collect()
}

/// The end of a conversation as the question carries it: its turns in the
/// words the board reads them in (`crate::transcript::turns_in` — a tool call
/// is its name and target, a result its text), one entry each, its role and
/// its words (`TURN_KEYS`) clamped to a card line
/// (`crate::transcript::clamp`), then the newest whose words fit
/// [`STALL_TRANSCRIPT_BYTE_CAP`] once the door has cleared them
/// ([`newest_from`]). Reasoning is left out: why the agent thought is not why
/// its pane went still.
#[must_use]
pub fn transcript_tail(lines: &[String]) -> Vec<Value> {
    let turns = crate::transcript::turns_in(&lines.join("\n"));
    let said: Vec<(&str, String)> = turns
        .iter()
        .filter(|turn| turn.role != "thinking")
        .map(|turn| (turn.role.as_str(), crate::transcript::clamp(&turn.text)))
        .collect();
    let words: Vec<&str> = said.iter().map(|(_, words)| words.as_str()).collect();
    said[newest_from(&words, STALL_TRANSCRIPT_BYTE_CAP)..]
        .iter()
        .map(|(role, words)| json!({ TURN_KEYS[0]: role, TURN_KEYS[1]: words }))
        .collect()
}

/// The one question a stall answer's label asks of both sides: did the
/// silence need its coordinator's hand (t-6342).
///
/// The label used to pair each cause with ONE follow-up, and read anything
/// else as a miss. That is how this machine's ledger came to hold eleven
/// misses out of eleven marks (2026-09-23): every answer read a command still
/// running, every worker then reported on its own, and a long tool's pair was
/// "nothing" — so each worker that came back by itself was written down as
/// the answer being wrong. A cause does not say which message arrives first;
/// it says whether somebody will have to act. So does what followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hand {
    /// Somebody had to act: write to the worker, type a continuation, or end
    /// the attempt.
    Needed,
    /// Nobody had to: the worker came back on its own, or nothing happened
    /// before the label's window closed.
    NotNeeded,
}

impl Cause {
    /// What this cause says about the silence's end. A wall, a transient
    /// error, a dead login, a classifier's decline, a question box and a
    /// worker that finished without saying so all wait for somebody — the
    /// last one until it is asked for its report. A command still running and a person at the keyboard end
    /// on their own. `Unknown` says nothing either way.
    #[must_use]
    pub const fn predicts(self) -> Option<Hand> {
        match self {
            Self::TransientApiError
            | Self::QuotaWall
            | Self::AuthFailure
            | Self::ClassifierDecline
            | Self::WaitingOnOwnCliQuestion
            | Self::FinishedWithoutReport => Some(Hand::Needed),
            Self::LongRunningTool | Self::HumanTookOver => Some(Hand::NotNeeded),
            Self::Unknown => None,
        }
    }
}

/// The stall seat's mark for one silence: whether the answer named a cause
/// that predicts what the ledger then showed — the one table the label reads
/// ([`Cause::predicts`] beside [`Followed::shows`]).
///
/// # Errors
///
/// The word of the side that says nothing — `unknown`, or `worker_died` — for
/// a label row to carry in place of a mark it has no right to
/// ([`crate::jev::summary::NOT_COMPARED`]).
pub fn mark(cause: Cause, followed: Followed) -> Result<bool, &'static str> {
    let predicted = cause.predicts().ok_or(cause.word())?;
    let shown = followed.shows().ok_or(followed.word())?;
    Ok(predicted == shown)
}

/// What the stall seat's baseline ([`crate::jev::STALL`]'s `baseline`: the
/// same answer every time) would have been marked on the same silence —
/// `None` when the baseline is not a constant answer this table knows, or
/// when what followed says nothing (t-6342).
#[must_use]
pub fn baseline_mark(followed: Followed) -> Option<bool> {
    match crate::jev::STALL.baseline {
        crate::jev::Baseline::AlwaysSame(word) => mark(Cause::from_word(word)?, followed).ok(),
        crate::jev::Baseline::TodaysRule | crate::jev::Baseline::None => None,
    }
}

/// What followed a silence — the first of these the ledger holds after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Followed {
    /// The run's coordinator wrote to the worker.
    Mail,
    /// The ledger typed a continuation for the attempt (t-4537).
    Resumed,
    /// The worker reported.
    WorkerDone,
    /// The attempt ended without a report and without its terminal dying:
    /// `worker-stop` or `worker-abandon` — the dispatch row does not say
    /// which verb.
    WorkerStop,
    /// The terminal holding the worker exited while it carried the attempt.
    WorkerDied,
    /// None of these within [`STALL_LABEL_WINDOW_MS`].
    Nothing,
}

impl Followed {
    /// Everything that can follow a silence, in the order the ledger is read.
    pub const ALL: [Self; 6] = [
        Self::Mail,
        Self::Resumed,
        Self::WorkerDone,
        Self::WorkerStop,
        Self::WorkerDied,
        Self::Nothing,
    ];

    /// The word a label row keeps.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Mail => "mail",
            Self::Resumed => "resumed",
            Self::WorkerDone => "worker_done",
            Self::WorkerStop => "worker_stop",
            Self::WorkerDied => "worker_died",
            Self::Nothing => "none",
        }
    }

    /// What this follow-up says about the same question [`Cause::predicts`]
    /// answers. Mail, a continuation and a stop are somebody acting; the
    /// worker's own report and a window that closed on nothing are nobody
    /// having to. A terminal that exited says nothing either way — a window
    /// restart and a crash both end a silence for reasons no cause names.
    #[must_use]
    pub const fn shows(self) -> Option<Hand> {
        match self {
            Self::Mail | Self::Resumed | Self::WorkerStop => Some(Hand::Needed),
            Self::WorkerDone | Self::Nothing => Some(Hand::NotNeeded),
            Self::WorkerDied => None,
        }
    }
}

/// What followed the silence of `worker`'s attempt `dispatch` asked about at
/// `asked_ms`, and when — or `None` while the window is still open and
/// nothing has.
///
/// Read off the ledger's own stamps, so when the label is written changes
/// nothing in it. Only the run's coordinator's mail is mail: a peer's is not
/// the coordinator's action, and mail to a crowd (`@all`) is filed under the
/// crowd rather than the worker. A dispatch that ended with the worker's
/// report or its terminal's death is the message that says so; one that ended
/// any other way was a verb.
#[must_use]
pub fn followed(
    run: &Run,
    worker: &str,
    dispatch: &str,
    asked_ms: i64,
    now_ms: i64,
) -> Option<(Followed, i64)> {
    let until = asked_ms.saturating_add(STALL_LABEL_WINDOW_MS);
    let within = |at: i64| at > asked_ms && at <= until;
    let reporter = worker_address(worker);
    let coordinator = run.address();
    let of_this_attempt = |held: Option<&str>| held == Some(dispatch);
    let mut first: Option<(Followed, i64)> = None;
    let mut died = false;
    for message in run.messages() {
        if message.kind == MessageKind::WorkerDied && of_this_attempt(message.dispatch.as_deref()) {
            died = true;
        }
        let at = message.created_ms;
        if !within(at) || first.is_some_and(|(_, held)| held <= at) {
            continue;
        }
        let what = match message.kind {
            MessageKind::WorkerDone if message.from == reporter => Followed::WorkerDone,
            MessageKind::Resumed if of_this_attempt(message.dispatch.as_deref()) => {
                Followed::Resumed
            }
            MessageKind::WorkerDied if of_this_attempt(message.dispatch.as_deref()) => {
                Followed::WorkerDied
            }
            kind if message.to == reporter
                && message.from == coordinator
                && !kind.is_the_ledgers_own() =>
            {
                Followed::Mail
            }
            _ => continue,
        };
        first = Some((what, at));
    }
    let ended = run
        .dispatch(dispatch)
        .and_then(|held| Some((held.ended_ms?, held.succeeded)));
    if let Some((at, succeeded)) = ended
        && within(at)
        && succeeded != Some(true)
        && !died
        && first.is_none_or(|(_, held)| at < held)
    {
        first = Some((Followed::WorkerStop, at));
    }
    match first {
        Some(found) => Some(found),
        None if now_ms > until => Some((Followed::Nothing, until)),
        None => None,
    }
}

#[cfg(test)]
mod tests;
