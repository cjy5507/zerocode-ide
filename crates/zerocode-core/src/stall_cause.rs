//! Why a quiet worker stopped, when the measured marker table cannot say
//! (docs/design/jev-settings-20260917.md §2, t-4538): the question the
//! window's stall sweep asks Jev about one silence, what makes an answer one,
//! and what happened after — the label an answer is later graded against.
//!
//! The marker table (`crates/zerocode-shell/src/quota_wall.rs`) names two
//! causes from an agent's own words, measured one CLI at a time: its quota
//! wall and a transient API error. Every other silence is `went_quiet` news,
//! and a coordinator reads the pane to learn why. This module puts that same
//! reading to Jev as a closed choice — the two measured causes (in words the
//! table has not measured), five more ways a worker's silence reads on this
//! machine, and `unknown` — and says what came of the silence afterwards.
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
use crate::jev::door::WITHHELD_LINE;
use crate::jev::{STALL_SCREEN_BYTE_CAP, STALL_TRANSCRIPT_BYTE_CAP};
use crate::orchestration::{MessageKind, Run, worker_address};

/// The one question's name. The endpoint does not show a question's name to
/// the model, so this is the caller's key and nothing more.
const QUESTION: &str = "cause";

/// The words of the question. The screen and the record reach the model as
/// state, never as an instruction.
const INSTRUCTIONS: &str = "A worker agent running in a terminal has printed nothing for `quietSeconds` seconds, and nothing the window has measured for a quota wall or a transient API error ends its conversation. `screen` is the bottom of its terminal as it stands now and `transcript` the last records of its conversation, oldest first, one per line. Choose why the worker stopped.";

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 4] = ["agent", "quietSeconds", "screen", "transcript"];

/// The version of the words in this module. Bump it when any of them changes:
/// a judgment read under one wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to [`crate::jev::rubric_fingerprint`].
pub const STALL_CAUSE_RUBRIC_VERSION: u32 = 2;

/// How long after a silence was asked about its label waits for what
/// followed ([`followed`]).
///
/// Two hours, measured on this machine's orchestration ledger (2026-09-17):
/// its twenty stall episodes (155 `went_quiet` stall notices, grouped by
/// episode, the first notice of each) were followed by a coordinator's mail,
/// a report or a stop within one hour for 16, within two for 18 and within
/// three for 19 — p50 12 min, p75 39 min, p90 91 min. Past the window the
/// label is `none`: whatever came later answered something other than that
/// silence.
pub const STALL_LABEL_WINDOW_MS: i64 = 2 * 60 * 60 * 1_000;

/// Why a quiet worker stopped — the question's closed answer space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    TransientApiError,
    QuotaWall,
    AuthFailure,
    WaitingOnOwnCliQuestion,
    FinishedWithoutReport,
    LongRunningTool,
    HumanTookOver,
    Unknown,
}

impl Cause {
    /// Every cause, in the order the question offers them.
    pub const ALL: [Self; 8] = [
        Self::TransientApiError,
        Self::QuotaWall,
        Self::AuthFailure,
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

/// The bottom of a terminal's screen as the question carries it: trailing
/// blanks off each line and blank lines off the bottom, then the newest lines
/// that fit [`STALL_SCREEN_BYTE_CAP`].
#[must_use]
pub fn screen_tail(screen: &str) -> String {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    let end = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(0, |at| at + 1);
    newest_within(&lines[..end], STALL_SCREEN_BYTE_CAP)
}

/// The end of a conversation as the question carries it: its turns in the
/// words the board reads them in (`crate::transcript::turns_in` — a tool call
/// is its name and target, a result its text), one line each as
/// `role: words`, each clamped to a card line (`crate::transcript::clamp`),
/// then the newest that fit [`STALL_TRANSCRIPT_BYTE_CAP`]. Reasoning is left
/// out: why the agent thought is not why its pane went still.
#[must_use]
pub fn transcript_tail(lines: &[String]) -> String {
    let turns = crate::transcript::turns_in(&lines.join("\n"));
    let said: Vec<String> = turns
        .iter()
        .filter(|turn| turn.role != "thinking")
        .map(|turn| format!("{}: {}", turn.role, crate::transcript::clamp(&turn.text)))
        .collect();
    newest_within(&said, STALL_TRANSCRIPT_BYTE_CAP)
}

/// The newest `lines`, oldest first, joined by newlines, that fit `cap` bytes
/// once the Jev door has cleared them. A line is counted at the larger of
/// itself and the mark that may stand in for it, so the door — which cuts a
/// text's END to its cap — never has to cut the newest words this kept.
fn newest_within<S: AsRef<str>>(lines: &[S], cap: usize) -> String {
    let mut used = 0;
    let mut start = lines.len();
    for (at, line) in lines.iter().enumerate().rev() {
        let cleared = line.as_ref().len().max(WITHHELD_LINE.len());
        let joined = cleared + usize::from(start < lines.len());
        if used + joined > cap {
            break;
        }
        used += joined;
        start = at;
    }
    lines[start..]
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a cause leads to, when it leads anywhere in particular — the mark a
/// label row carries for the judge (§4 of the settings design): the answer
/// was right when what followed the silence is what its cause leads to. A
/// transient error and a quota wall lead to a continuation or a handover
/// (`Resumed`); a worker waiting on its own CLI's question needs a word from
/// its coordinator (`Mail`); one that finished without reporting reports when
/// asked (`WorkerDone`); a long tool and a person at the keyboard lead to
/// nothing the ledger does. `Unknown` leads nowhere, so it leaves no mark.
#[must_use]
pub const fn expected_followed(cause: Cause) -> Option<Followed> {
    match cause {
        Cause::TransientApiError | Cause::QuotaWall => Some(Followed::Resumed),
        // A dead login is not waited out: the attempt is stopped or abandoned
        // and the work summoned again on an agent that can sign in.
        Cause::AuthFailure => Some(Followed::WorkerStop),
        Cause::WaitingOnOwnCliQuestion => Some(Followed::Mail),
        Cause::FinishedWithoutReport => Some(Followed::WorkerDone),
        Cause::LongRunningTool | Cause::HumanTookOver => Some(Followed::Nothing),
        Cause::Unknown => None,
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
