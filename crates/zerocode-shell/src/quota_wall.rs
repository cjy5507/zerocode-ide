//! The agent's OWN words about why it stopped — the measured stall-cause
//! table. Three causes today: the quota wall, the first of the two witnesses a
//! `quota_walled` notice needs (`docs/design/quota-aware-summoning-and-handover.md`
//! §2.2), the transient API error a declared continuation answers (t-4537),
//! and the expired login, which answers every prompt the way a wall does.
//!
//! The two walls have a second reader (t-6560): the mail pointer, which asks
//! whether the pane it is about to type at ended its last turn at one. Any
//! prompt typed there only meets the same wall again — on 2026-09-20..23 the
//! coordinator's pane was retyped at 1,391 times that way — so the pointer
//! holds its line until the wall stops standing. It reads the conversation's
//! record and never the screen: the record says when it was written, and a
//! hold needs to know until when.
//!
//! A table of measured markers, one row per CLI and cause, the way `TUNABLE`
//! is a table of measured launch dials: nothing here is guessed from
//! documentation. Each row was read off a real stop — the codex rollout and
//! TUI of 2026-09-03 23:29 (`usage_limit_exceeded`), fifty-one Claude
//! transcripts stopping on "You've hit your session limit", zo's own hint
//! after its 2026-09-07 fix, and for the transient rows every Claude
//! transcript and Codex rollout on this machine (2026-09-17). The window asks
//! this table only for a worker that is already QUIET. For a wall the answer
//! is one witness of two: the provider's number is the other, and the ledger
//! takes neither alone. For a transient error the answer is the whole
//! witness, and so the transient rows read only the record that ENDS the
//! conversation — a screen line cannot say which turn it belongs to.

use std::path::Path;

use zerocode_core::orchestration::{
    ClassifierDeclineMarker, DeclineScreen, ModelDeviation, QuotaWallMarker, Text,
    TransientErrorMarker,
};
use zerocode_shell_state::durable_file::{FileIdentity, file_identity};

/// Where the words were read.
const SCREEN: &str = "screen";
const ROLLOUT: &str = "rollout";
const TRANSCRIPT: &str = "transcript";

/// The most of a marker line that travels: a coordinator reads it in a
/// notice, and a screen line can be a whole wrapped paragraph.
const MARKER_LINE_MAX: usize = 200;

/// Why a quiet worker stopped, as its own words say — the table's cause
/// column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StallCause {
    /// The provider's quota wall.
    QuotaWall,
    /// A provider or transport failure that ended the turn and that the next
    /// request may well not meet (t-4537).
    TransientApiError,
    /// The agent's login is gone: every prompt is answered by the CLI itself
    /// until somebody signs it in again (t-6560).
    LoginWall,
    /// The provider's safety classifier declined the conversation's last
    /// request and the agent's CLI stopped there rather than continue on a
    /// fallback model (t-6747).
    ClassifierDecline,
}

impl StallCause {
    /// Whether a prompt after the record takes the marker back.
    ///
    /// A continuation ACTS on the composer, so its witness has to be the last
    /// word of the conversation: a person's line, a mail pointer, a queued
    /// message after the error is a turn somebody already started. A wall is
    /// news, and a person typing into a wall does not lift it. A decline is
    /// the first kind: a prompt after it is a new request, which the
    /// classifier judges again.
    const fn needs_the_last_word(self) -> bool {
        matches!(self, Self::TransientApiError | Self::ClassifierDecline)
    }

    /// Whether the next prompt meets this cause again for certain — the walls
    /// the mail pointer holds its line back from. A transient error is the
    /// opposite: the next prompt is the cure.
    const fn answers_every_prompt(self) -> bool {
        matches!(self, Self::QuotaWall | Self::LoginWall)
    }

    /// The cause as the window's own log names it.
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::QuotaWall => "quota",
            Self::TransientApiError => "transient-error",
            Self::LoginWall => "login",
            Self::ClassifierDecline => "classifier-decline",
        }
    }
}

/// A Codex turn edge's error, as its rollout records it: the
/// `codex_error_info` word, and — where that word is a catch-all — the start
/// of the message that narrows it.
#[derive(Clone, Copy, Debug)]
struct CodexError {
    info: &'static str,
    message_starts: Option<&'static str>,
}

/// How one CLI's transcript says it stopped for this row's cause.
#[derive(Clone, Copy, Debug)]
enum TranscriptRule {
    /// The window reads no transcript of this agent's.
    None,
    /// A Codex rollout: the NEWEST turn edge (`task_started` /
    /// `task_complete` / `turn_aborted`) carries one of these errors.
    CodexRollout(&'static [CodexError]),
    /// A Claude transcript: the NEWEST assistant record is an API error
    /// (`isApiErrorMessage`) whose `error` is one of `errors`, or whose text
    /// carries every phrase of one `says` group.
    ClaudeJsonl {
        errors: &'static [&'static str],
        says: &'static [&'static [&'static str]],
    },
}

/// One measured row: the agent, the cause, the phrases its SCREEN shows for
/// it, and how its transcript says the same.
#[derive(Clone, Copy, Debug)]
struct StallMarkerRule {
    agent: &'static str,
    cause: StallCause,
    /// Every phrase is matched as a substring of one screen line; a line
    /// has to carry EVERY phrase of one group, so `("usage limit", "resets in")`
    /// is a status line and not a stray word.
    screen: &'static [&'static [&'static str]],
    transcript: TranscriptRule,
}

/// The markers, one row per CLI and cause this window can read.
///
/// Quota wall (2026-09-07):
/// - codex: the TUI prints its provider's sentence ("You've hit your usage
///   limit. Visit … or try again at …"); the rollout closes the turn with
///   `task_complete` carrying `error.codex_error_info: "usage_limit_exceeded"`
///   — measured, and NOT the `turn_aborted` + `usage_limit_reached` pair the
///   design guessed (seven rollouts under `~/.codex/sessions`, all `task_complete`).
/// - claude: "You've hit your session limit · resets 4:10am (Asia/Seoul)"
///   on screen and as the `isApiErrorMessage` assistant record with
///   `error: "rate_limit"`; "You've hit your limit" is the older spelling
///   the design quotes and is kept beside it. A model's own cap
///   (2026-09-22, w-5922) is the same `rate_limit` record with a sentence
///   that names the model — "You've reached your Fable limit. Run
///   /usage-credits to continue or switch models with /model." — so its
///   marker is the family, `You've reached your … limit`, never the name:
///   the session gauge read 23% while that pane stood at the wall.
/// - zo: its own hint line, "This account's usage limit … resets in 2h 17m"
///   (`usage_limit_hint`, zo-ide 2026-09-07), and the raw frame code it
///   prints when a stream dies on the wall.
///
/// Transient API error (2026-09-17, both Claude project roots and 975 Codex
/// rollouts). No screen phrases: see the module head.
/// - claude: `error: "server_error"` on the `isApiErrorMessage` record, and
///   nothing else. Every one of its 69 records is transient in its own words
///   — "The response stopped arriving" (w-4525, 2.1.274), "529 Overloaded",
///   "500 Internal server error", "Server error / Connection lost /
///   Connection closed mid-response", "Your computer went to sleep
///   mid-response", "Unable to connect to API (ENOTFOUND)". No record says
///   `overloaded_error`: a 529 is a `server_error` too. The other words on
///   this machine are not transient and are not here — `rate_limit` is the
///   wall, `authentication_failed` waits for a login, `invalid_request` is a
///   safeguard that refuses the same turn again, `model_not_found` a setting.
/// - codex: `server_overloaded` ("Selected model is at capacity", 28 edges),
///   and `other` only when its message starts "stream disconnected before
///   completion" (2 edges) — `other` also closes 400 `invalid_request_error`
///   turns that a continuation would only repeat.
/// - zo: no row. Its session files hold `message`, `vault`, `session_meta`
///   and `compaction` records and nothing a provider error ends (987 files),
///   the window reads no zo transcript, and zo reconnects a dropped stream
///   itself (`waiting` / `reconnecting` on its status line).
///
/// Login wall (2026-09-24, the coordinator transcript `68b2661a-…`, 2.1.263
/// through 2.1.280):
/// - claude: "Login expired · Please run /login", all thirteen records the
///   `isApiErrorMessage` assistant record with `error: "authentication_failed"`
///   and no `requestId` — the CLI answers the prompt itself and asks nobody.
///   Nine of them came two seconds apart on 2026-09-20 12:10, each after a
///   mail pointer. The word, not the sentence, is the marker.
/// - codex, zo: no row — no expired login in their records on this machine.
///
/// Classifier decline (t-6747, both Claude project roots and 95 Codex
/// rollouts, 2026-09-10..24):
/// - claude: the CLI's own sentence, "<Model>'s safeguards flagged this
///   message", on screen and in the `isApiErrorMessage` assistant record a
///   decline with no fallback writes (18 records, all `error:
///   "invalid_request"` — Fable 5.1's 14 and Opus 5's 4; the `system`
///   record beside it, `model_refusal_no_fallback`, names the category).
///   The sentence, not the error word: `invalid_request` is also any other
///   400. A decline the CLI answered on its fallback writes
///   `model_refusal_fallback` and goes on — no stop, so no row here; its
///   switch of model is read by [`fallbacks_in`]. The pause dialog (the
///   person's `switchModelsOnFlag: false`) writes nothing until answered,
///   and shows its choices: [`DECLINE_DIALOG_CHOICE`].
/// - codex: no row — no decline in its records on this machine.
/// - zo: no row — zo walks its own ladder in-process and says so on its own
///   screen; the window reads no zo transcript.
const STALL_MARKERS: &[StallMarkerRule] = &[
    StallMarkerRule {
        agent: "codex",
        cause: StallCause::QuotaWall,
        screen: &[&["You've hit your usage limit"]],
        transcript: TranscriptRule::CodexRollout(&[CodexError {
            info: "usage_limit_exceeded",
            message_starts: None,
        }]),
    },
    StallMarkerRule {
        agent: "claude",
        cause: StallCause::QuotaWall,
        screen: &[
            &["You've hit your limit"],
            &["You've hit your session limit"],
            &["You've hit your weekly limit"],
            &["You've reached your", "limit"],
        ],
        transcript: TranscriptRule::ClaudeJsonl {
            errors: &["rate_limit"],
            says: &[&["hit your", "limit"], &["reached your", "limit"]],
        },
    },
    StallMarkerRule {
        agent: "zo",
        cause: StallCause::QuotaWall,
        screen: &[&["usage limit", "resets in"], &["usage_limit_reached"]],
        transcript: TranscriptRule::None,
    },
    StallMarkerRule {
        agent: "codex",
        cause: StallCause::TransientApiError,
        screen: &[],
        transcript: TranscriptRule::CodexRollout(&[
            CodexError {
                info: "server_overloaded",
                message_starts: None,
            },
            CodexError {
                info: "other",
                message_starts: Some("stream disconnected before completion"),
            },
        ]),
    },
    StallMarkerRule {
        agent: "claude",
        cause: StallCause::TransientApiError,
        screen: &[],
        transcript: TranscriptRule::ClaudeJsonl {
            errors: &["server_error"],
            says: &[],
        },
    },
    StallMarkerRule {
        agent: "claude",
        cause: StallCause::LoginWall,
        screen: &[],
        transcript: TranscriptRule::ClaudeJsonl {
            errors: &["authentication_failed"],
            says: &[],
        },
    },
    StallMarkerRule {
        agent: "claude",
        cause: StallCause::ClassifierDecline,
        screen: &[&[DECLINE_SENTENCE]],
        transcript: TranscriptRule::ClaudeJsonl {
            errors: &[],
            says: &[&[DECLINE_SENTENCE]],
        },
    },
];

/// Claude Code's own words for a decline, in every spelling 2.1.281 has
/// ("<Model>'s safeguards flagged this message", "This model's safeguards
/// flagged this message") and in all 18 records on this machine.
const DECLINE_SENTENCE: &str = "safeguards flagged this message";

/// The choice Claude Code's pause dialog offers beside its fallback —
/// "Edit prompt and retry", alone or "… with <Model>" (2.1.281's labels, and
/// the screen a coordinator read off w-5770 on 2026-09-21). A screen that
/// shows it IN THE DIALOG'S OWN LAYOUT ([`dialog_stands_in`]) is a dialog
/// waiting for a key, not a printed error; the words alone are anything a
/// tool result or a brief quoted.
const DECLINE_DIALOG_CHOICE: &str = "Edit prompt and retry";

/// The pause dialog's header (2.1.281), the first line of its box.
const DECLINE_DIALOG_HEADER: &str = "Session paused";

/// The glyph Claude Code's menus put before the highlighted choice
/// (`❯  1.  Switch to Opus 4.8` on w-5770's screen and on the hermetic
/// probe's pty, t-6747 `d-dialog`).
const DECLINE_DIALOG_CURSOR: char = '❯';

/// How many lines above the retry choice the dialog's header stands, at
/// most: measured 5 on the probe's 118-column pty (the sentence wrapped over
/// three lines, then the detail line, then the first choice) and 3 on
/// w-5770's screen; a narrower pane wraps the sentence further.
const DECLINE_DIALOG_LINES: usize = 8;

/// The system record a decline with no fallback writes beside its error,
/// naming the category (`apiRefusalCategory`), and the one a decline the
/// CLI answered on its fallback writes instead.
const CLAUDE_NO_FALLBACK_RECORD: &str = "model_refusal_no_fallback";
const CLAUDE_FALLBACK_RECORD: &str = "model_refusal_fallback";

fn rule_for(agent: &str, cause: StallCause) -> Option<&'static StallMarkerRule> {
    STALL_MARKERS
        .iter()
        .find(|rule| rule.agent == agent && rule.cause == cause)
}

/// Whether this table has a row for `agent` and `cause` at all — asked
/// BEFORE a pane's grid is copied or its transcript read, so an agent nobody
/// measured costs nothing per beat.
pub(crate) fn has_rule(agent: &str, cause: StallCause) -> bool {
    rule_for(agent, cause).is_some()
}

/// The wall marker for `agent`, read off its screen and the bounded tail of
/// its transcript at `transcript_path` — the production road, one file read.
pub(crate) fn marker_for(
    agent: &str,
    screen: Option<&str>,
    transcript_path: Option<&Path>,
) -> Option<QuotaWallMarker> {
    let rule = rule_for(agent, StallCause::QuotaWall)?;
    let lines = match rule.transcript {
        TranscriptRule::None => None,
        _ => transcript_path.and_then(zerocode_core::transcript::tail_lines),
    };
    marker_in(agent, screen, lines.as_deref())
}

/// The wall marker for `agent` in what the window holds: its screen, and the
/// tail lines of its transcript (oldest first). Pure, so the fixtures below
/// are the whole test.
///
/// The transcript is asked first: it outlives the screen and says which
/// turn it is talking about. The screen is asked second and only for the
/// row's exact phrases — a line that merely says `429` is a line.
pub(crate) fn marker_in(
    agent: &str,
    screen: Option<&str>,
    transcript_lines: Option<&[String]>,
) -> Option<QuotaWallMarker> {
    let rule = rule_for(agent, StallCause::QuotaWall)?;
    if let Some(found) = transcript_lines.and_then(|lines| read_transcript(rule, lines)) {
        return Some(QuotaWallMarker {
            source: found.source.to_string(),
            line: found.line,
        });
    }
    let screen = screen?;
    screen.lines().rev().find_map(|line| {
        let line = line.trim();
        screen_says(rule, line).then(|| QuotaWallMarker {
            source: SCREEN.to_string(),
            line: clipped(line),
        })
    })
}

/// Whether one line carries every phrase of one of the row's screen groups.
fn screen_says(rule: &StallMarkerRule, line: &str) -> bool {
    rule.screen
        .iter()
        .any(|group| group.iter().all(|phrase| line.contains(phrase)))
}

/// How each CLI says, when asked once headless, that its login is what
/// stopped it (t-10372) — measured on this machine, 2026-09-26, with a home
/// that holds no login: Claude Code 2.1.283 answers its one JSON result with
/// "Not logged in · Please run /login" (and an expired login's record reads
/// "Login expired · Please run /login", this table's login row); codex-cli
/// 0.157.1 retries and ends its turn with "unexpected status 401
/// Unauthorized: Missing bearer or basic authentication in header".
const ONE_SHOT_LOGIN: &[(&str, &str)] = &[
    ("claude", "Please run /login"),
    ("codex", "401 Unauthorized"),
];

/// What a one-shot run's own answer says stopped it (t-10372): the quota
/// wall, by the same screen phrases this table reads off a pane — a headless
/// run answers with the sentence a pane shows — or the login wall, by the
/// CLI's own request to sign in ([`ONE_SHOT_LOGIN`]). `None` for anything
/// else: an error nobody measured is said as the CLI's refusal, not guessed
/// into a cause.
pub(crate) fn one_shot_cause(agent: &str, said: &str) -> Option<StallCause> {
    if let Some(rule) = rule_for(agent, StallCause::QuotaWall)
        && said.lines().any(|line| screen_says(rule, line.trim()))
    {
        return Some(StallCause::QuotaWall);
    }
    ONE_SHOT_LOGIN
        .iter()
        .any(|(cli, phrase)| *cli == agent && said.contains(phrase))
        .then_some(StallCause::LoginWall)
}

/// The transient-error marker for `agent`, read off the bounded tail of its
/// transcript at `transcript_path` — the production road, one file read.
pub(crate) fn transient_error_for(
    agent: &str,
    transcript_path: &Path,
) -> Option<TransientErrorMarker> {
    if !has_rule(agent, StallCause::TransientApiError) {
        return None;
    }
    let lines = zerocode_core::transcript::tail_lines(transcript_path)?;
    transient_error_in(agent, &lines)
}

/// The transient-error marker for `agent` in its transcript's tail lines
/// (oldest first), when the error is the conversation's last word. Pure.
///
/// A record with no identity of its own is no marker: the key is what keeps
/// one error from being typed at twice.
pub(crate) fn transient_error_in(agent: &str, lines: &[String]) -> Option<TransientErrorMarker> {
    let rule = rule_for(agent, StallCause::TransientApiError)?;
    let found = read_transcript(rule, lines)?;
    Some(TransientErrorMarker {
        source: found.source.to_string(),
        line: found.line,
        key: found.key?,
    })
}

/// What a quiet pane shows and records of a safety classifier's decline
/// (t-6747): the screen and the transcript's last record — the two
/// witnesses `classifier_decline_witness` needs — and every switch of model
/// its CLI recorded answering a decline on a fallback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeclineReading {
    pub(crate) screen: Option<DeclineScreen>,
    pub(crate) record: Option<ClassifierDeclineMarker>,
    /// The switches, oldest first, with no worker named yet — the sweep
    /// knows whose pane it read.
    pub(crate) fallbacks: Vec<ModelDeviation>,
}

/// The decline reading for `agent`, off its screen and the bounded tail of
/// its transcript at `transcript_path` — the production road, one file read,
/// and nothing at all for an agent the table has no row for.
pub(crate) fn decline_reading_for(
    agent: &str,
    screen: Option<&str>,
    transcript_path: Option<&Path>,
) -> Option<DeclineReading> {
    if !has_rule(agent, StallCause::ClassifierDecline) {
        return None;
    }
    let lines = transcript_path.and_then(zerocode_core::transcript::tail_lines);
    Some(decline_reading_in(agent, screen, lines.as_deref()))
}

/// The decline reading in what the window holds. Pure.
pub(crate) fn decline_reading_in(
    agent: &str,
    screen: Option<&str>,
    transcript_lines: Option<&[String]>,
) -> DeclineReading {
    let Some(rule) = rule_for(agent, StallCause::ClassifierDecline) else {
        return DeclineReading::default();
    };
    let claude = matches!(rule.transcript, TranscriptRule::ClaudeJsonl { .. });
    DeclineReading {
        screen: screen.and_then(|screen| decline_screen(rule, screen)),
        record: transcript_lines.and_then(|lines| decline_record(rule, lines)),
        fallbacks: transcript_lines
            .filter(|_| claude)
            .map(fallbacks_in)
            .unwrap_or_default(),
    }
}

/// The decline sentence as the pane's screen shows it, whether it stands in
/// the pause dialog, and the category the dialog's detail line names — the
/// dialog's own, read within its box; a screen that is no dialog names none.
fn decline_screen(rule: &StallMarkerRule, screen: &str) -> Option<DeclineScreen> {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let line = lines.iter().rev().find(|line| {
        rule.screen
            .iter()
            .any(|group| group.iter().all(|phrase| line.contains(phrase)))
    })?;
    let dialog = dialog_stands_in(&lines);
    Some(DeclineScreen {
        line: clipped(line),
        dialog: dialog.is_some(),
        category: dialog.and_then(|(header, choice)| {
            lines[header..choice]
                .iter()
                .find_map(|line| details_category(line))
        }),
    })
}

/// Where Claude Code's pause dialog stands on the screen, when it does
/// (t-7153, P1-3): the line range from its header to its retry choice.
///
/// The dialog is a LAYOUT, not a phrase — its header, then the sentence and
/// the detail line, then a numbered choice under the menu cursor, then the
/// retry choice, numbered, within [`DECLINE_DIALOG_LINES`] of the header
/// (both real dialogs read here: w-5770's screen, the probe's pty). A tool
/// result or a brief that quotes the words shows them indented under the
/// tool's own mark, with no menu cursor on a numbered line, under the turn's
/// own spinner. A quote that reproduced the whole layout, cursor and all,
/// would still read as a dialog: which is why a dialog is diagnostic news
/// and ends no worker (`decline_source_may_stop`).
fn dialog_stands_in(lines: &[&str]) -> Option<(usize, usize)> {
    let numbered = |line: &str| {
        let mut rest = line.trim_start_matches(DECLINE_DIALOG_CURSOR).trim_start();
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest = &rest[digits..];
        rest.starts_with('.')
    };
    let choice = lines
        .iter()
        .rposition(|line| numbered(line) && line.contains(DECLINE_DIALOG_CHOICE))?;
    let box_top = choice.saturating_sub(DECLINE_DIALOG_LINES);
    let header = lines[box_top..choice]
        .iter()
        .rposition(|line| line.contains(DECLINE_DIALOG_HEADER))
        .map(|at| box_top + at)?;
    lines[header..=choice]
        .iter()
        .any(|line| line.starts_with(DECLINE_DIALOG_CURSOR) && numbered(line))
        .then_some((header, choice))
}

/// The category in Claude Code's own detail line — "Details: `[cyber]`"
/// (2.1.281 prints the category word between the brackets). A word that is
/// not one is no category.
fn details_category(line: &str) -> Option<String> {
    let after = line.split("Details:").nth(1)?;
    let start = after.find('[')? + 1;
    let end = after[start..].find(']')? + start;
    let word = after[start..end].trim();
    (!word.is_empty()
        && word
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'))
    .then(|| word.to_string())
}

/// The conversation's last record, when it is a decline its CLI stopped at:
/// the error record the table's row names, as the last word, and the
/// category off the system record the CLI wrote beside it — THIS error's
/// own record, never an earlier request's (t-7153, P1-4).
fn decline_record(rule: &StallMarkerRule, lines: &[String]) -> Option<ClassifierDeclineMarker> {
    let found = read_transcript(rule, lines)?;
    let key = found.key?;
    Some(ClassifierDeclineMarker {
        source: found.source.to_string(),
        line: found.line,
        category: declines_own_category(lines, found.request.as_deref(), found.parent.as_deref()),
        key,
    })
}

/// The category the CLI wrote beside THIS decline: the
/// `model_refusal_no_fallback` system record the error record's two ids
/// both name — the same request (`requestId`) and the record the error
/// names as its parent (`parentUuid`, the system record's `uuid`); 2.1.281
/// writes both, the system record first and the error record after it.
/// The first record either id names decides, category or none: a decline
/// whose own record names no category HAS none, and an earlier request's
/// word is never borrowed for it (before this, a `null` category read the
/// tail on and took the previous decline's `cyber`). A record one id names
/// and the other DISPUTES — the request's record, whose uuid is not the
/// parent the error names, or the parent record, answering another request
/// — is a contradiction the table does not guess its way out of: no
/// category (t-7153, R1; before this either id alone was taken). An id a
/// record does not carry disputes nothing (a CLI older than the field), and
/// an empty id is no key at all. No record of its own, no category.
fn declines_own_category(
    lines: &[String],
    request: Option<&str>,
    parent: Option<&str>,
) -> Option<String> {
    /// An id as a join key: trimmed, and none when empty.
    fn key(id: Option<&str>) -> Option<&str> {
        id.map(str::trim).filter(|id| !id.is_empty())
    }
    /// Whether a record's id agrees with ours: none when either side is
    /// silent, which neither agrees nor disputes.
    fn agrees(ours: Option<&str>, theirs: Option<&str>) -> Option<bool> {
        Some(ours? == key(theirs)?)
    }
    let (request, parent) = (key(request), key(parent));
    if request.is_none() && parent.is_none() {
        return None;
    }
    lines
        .iter()
        .rev()
        .find_map(|line| {
            if !line.contains(CLAUDE_NO_FALLBACK_RECORD) {
                return None;
            }
            let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
            if value["type"] != "system" || value["subtype"] != CLAUDE_NO_FALLBACK_RECORD {
                return None;
            }
            let joins = [
                agrees(request, value["requestId"].as_str()),
                agrees(parent, value["uuid"].as_str()),
            ];
            if !joins.contains(&Some(true)) {
                // Another request's record: read on.
                return None;
            }
            let disputed = joins.contains(&Some(false));
            Some((!disputed).then(|| value["apiRefusalCategory"].as_str().map(str::to_string)))
        })
        .flatten()
        .flatten()
}

/// Whether the table reads switches of model in `agent`'s records — a
/// decline its CLI answered on a fallback. Claude's transcripts only.
pub(crate) fn reads_fallbacks(agent: &str) -> bool {
    rule_for(agent, StallCause::ClassifierDecline)
        .is_some_and(|rule| matches!(rule.transcript, TranscriptRule::ClaudeJsonl { .. }))
}

/// One reading of a worker transcript's switch scan (t-7153, P2): the
/// switches found past the cursor it was given, and where the cursor would
/// stand once the ledger HOLDS them. Nothing here moves a cursor: the caller
/// commits `next` after the rows are durable and not before
/// (`orchestration::note_model_deviations`), so a row the actor refused —
/// recovering, its store full — is read again next beat, and written once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeviationScan {
    pub(crate) switches: Vec<ModelDeviation>,
    pub(crate) next: ScanCursor,
}

/// Where a switch scan stands: `offset` bytes into the file that was at its
/// path when it last read — THAT file, by its identity, not whatever stands
/// at the path now (t-7153, R3) — and a fingerprint of what those bytes
/// were, so a file rewritten in place under the same identity is read
/// again from its start. A cursor that knows no file yet reads from the
/// start of the one it finds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ScanCursor {
    pub(crate) file: Option<TranscriptIdentity>,
    pub(crate) offset: u64,
    /// What the `offset` counted, as the scan read it.
    pub(crate) counted: Counted,
}

/// A fingerprint of the bytes a cursor counted past (t-7153, R3): of each
/// window of them — the scan's own reading window,
/// [`zerocode_core::transcript::MAX_TAIL_BYTES`], counted from the file's
/// start — and of the last few before the cursor. On a file the platform
/// names ([`FileIdentity`]) every reading checks the last few, and one
/// window more, in rounds ([`Round`]): a file truncated and rewritten under
/// its own inode, its first line kept, no longer holds them — found at once
/// when the rewrite reached the bytes just before the cursor, and within
/// `2N − 1` readings for the `N` windows the cursor counted when it changed
/// only a record further back, however fast the file grows (46 windows for
/// the 11.8 MB transcript a worker wrote here: 91 readings at most), where
/// before the last few alone were read and such a rewrite stood as the
/// file it replaced for good. On a file the platform cannot name nothing is
/// taken on faith, and every window is read again and compared before the
/// cursor is trusted. A bare fingerprint counted nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Counted {
    /// Each whole window the cursor counted, oldest first.
    windows: Vec<u64>,
    /// The bytes it counted past the last whole window.
    rest: u64,
    /// The last [`TRANSCRIPT_FINGERPRINT_BYTES`] before the cursor.
    anchor: u64,
    /// Which window the next reading checks again.
    round: Round,
}

impl Default for Counted {
    fn default() -> Self {
        Self {
            windows: Vec::new(),
            rest: FNV_EMPTY,
            anchor: FNV_EMPTY,
            round: Round::default(),
        }
    }
}

/// A round of the checks a cursor's readings make of what it counted
/// (t-7153 r4, R3a): its end fixed as it begins — the windows the cursor
/// had counted then — and walked a window a reading; a window counted since
/// waits for the next round. It is the cursor's own, and begins again with
/// the cursor's count when the cursor does. So a window `i` of the `N` a
/// cursor has counted is checked within `2N − 1` readings however fast the
/// file grows: what is left of the round under way, its end no more than
/// `N`, then `i + 1` readings of the next. Before this the window a reading
/// checked was its caller's count of readings over the windows counted NOW,
/// and a file growing a window a reading moved both by one: the same window
/// was named on every reading for as long as the file grew, and the others
/// never again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Round {
    /// The window the next reading checks.
    next: u64,
    /// How many windows the cursor had counted when the round began.
    end: u64,
}

impl Round {
    /// The window a reading checks, of the `windows` the cursor counts now,
    /// and the round after it: a round walked to its end — or one that
    /// names a window the cursor does not count — begins again, its end
    /// fixed at `windows`. Nothing to check before a window is counted.
    const fn step(self, windows: u64) -> (Option<u64>, Self) {
        let round = if self.next < self.end && self.end <= windows {
            self
        } else {
            Self {
                next: 0,
                end: windows,
            }
        };
        if round.end == 0 {
            return (None, round);
        }
        (
            Some(round.next),
            Self {
                next: round.next + 1,
                end: round.end,
            },
        )
    }
}

impl Counted {
    /// This fingerprint carried over `bytes` — the file's bytes just before
    /// `to`, which the cursor just counted — with the anchor read afresh at
    /// `to`, the cursor's new stand.
    fn past(&self, bytes: &[u8], file: &mut std::fs::File, to: u64) -> Option<Self> {
        let window = zerocode_core::transcript::MAX_TAIL_BYTES;
        let mut counted = self.clone();
        let mut at = to.checked_sub(bytes.len() as u64)?;
        let mut left = bytes;
        while !left.is_empty() {
            let room = usize::try_from(window - at % window)
                .map_or(left.len(), |room| room.min(left.len()));
            let (piece, after) = left.split_at(room);
            counted.rest = fnv1a(counted.rest, piece);
            at += piece.len() as u64;
            if at.is_multiple_of(window) {
                counted
                    .windows
                    .push(std::mem::replace(&mut counted.rest, FNV_EMPTY));
            }
            left = after;
        }
        counted.anchor = fnv1a(FNV_EMPTY, &bytes_before(file, to)?);
        Some(counted)
    }

    /// Whether what this fingerprint counted still stands before `offset`
    /// in `file`: the last few bytes and the window its round names
    /// ([`Round`]), the round walked on by one, when the file is `named` by
    /// its platform; every window when it is not.
    fn still_stands(&mut self, file: &mut std::fs::File, offset: u64, named: bool) -> bool {
        let window = zerocode_core::transcript::MAX_TAIL_BYTES;
        let windows = offset.div_ceil(window);
        let holds = |index: u64| {
            let start = index * window;
            let counted = usize::try_from(index)
                .ok()
                .and_then(|index| self.windows.get(index))
                .copied()
                .unwrap_or(self.rest);
            bytes_at(file, start, window.min(offset - start))
                .is_some_and(|bytes| fnv1a(FNV_EMPTY, &bytes) == counted)
        };
        if !named {
            return (0..windows).all(holds);
        }
        let (check, round) = self.round.step(windows);
        self.round = round;
        check.is_none_or(holds)
            && bytes_before(file, offset).is_some_and(|tail| fnv1a(FNV_EMPTY, &tail) == self.anchor)
    }
}

/// `len` bytes of `file` from `start`, or nothing when they cannot be read.
/// The file's position is not kept.
fn bytes_at(file: &mut std::fs::File, start: u64, len: u64) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = vec![0u8; usize::try_from(len).ok()?];
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

/// The last [`TRANSCRIPT_FINGERPRINT_BYTES`] before `offset` in `file` —
/// fewer when the file is shorter than that — or nothing when they cannot
/// be read.
fn bytes_before(file: &mut std::fs::File, offset: u64) -> Option<Vec<u8>> {
    let len = offset.min(TRANSCRIPT_FINGERPRINT_BYTES as u64);
    bytes_at(file, offset - len, len)
}

/// FNV-1a, 64-bit, carried on from `seed`: a fingerprint that resumes where
/// it stopped, so what a cursor counted over many beats is one number a
/// window. The empty fingerprint is [`FNV_EMPTY`].
fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// FNV-1a's offset basis: the fingerprint of nothing.
const FNV_EMPTY: u64 = 0xcbf2_9ce4_8422_2325;

impl ScanCursor {
    /// The cursor standing at `offset` in `file`, having counted `counted`.
    const fn at(file: TranscriptIdentity, offset: u64, counted: Counted) -> Self {
        Self {
            file: Some(file),
            offset,
            counted,
        }
    }
}

/// Which file a cursor's offset counts into (t-7153, R3): the file's own
/// name on its platform ([`FileIdentity`]) where the platform gives one,
/// when it was born where the file system says, and its first line's
/// opening bytes — a Claude transcript opens on a record with the session's
/// own uuid and timestamp. A file atomically replaced under the same path —
/// a session id reused, a conversation rewritten, its first line kept — is
/// another file by its name alone, read from its start whether it is
/// shorter, as long as, or longer than the one before: before this the
/// identity was the first line and a birth time, and a replacement that kept
/// the first line, on a file system that gives no birth, read as unchanged.
/// A file appended to keeps every part of its identity, so a cursor into it
/// stands — once what the cursor counted is found still there
/// ([`Counted`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptIdentity {
    id: Option<FileIdentity>,
    born: Option<std::time::SystemTime>,
    opening: Vec<u8>,
}

/// What the platform says of an open file: its name, and when it was born
/// — either nothing where the platform gives none. The scanner takes this
/// as a parameter so a test can stand where a platform gives neither.
pub(crate) type FileEvidence =
    fn(&std::fs::File, &std::fs::Metadata) -> (Option<FileIdentity>, Option<std::time::SystemTime>);

/// This platform's evidence: the file's name and its birth.
fn platform_evidence(
    file: &std::fs::File,
    meta: &std::fs::Metadata,
) -> (Option<FileIdentity>, Option<std::time::SystemTime>) {
    (file_identity(file, meta).ok(), meta.created().ok())
}

/// How much of the first line names a transcript, and how many bytes before
/// the cursor its fingerprint anchors on: a Claude record's `uuid` and
/// `timestamp` ride in its first few hundred bytes, and a
/// `file-history-snapshot` opens on its `messageId`.
const TRANSCRIPT_FINGERPRINT_BYTES: usize = 4 * 1024;

impl TranscriptIdentity {
    /// The identity of the open `file`, whose metadata is `meta`, as
    /// `evidence` names it: its platform name and birth, and its first
    /// line's opening bytes (fewer than a line, when the line is still
    /// being written — a file that has no whole first line yet is an empty
    /// one, read from its start on every beat until it has).
    fn of(
        file: &mut std::fs::File,
        meta: &std::fs::Metadata,
        evidence: FileEvidence,
    ) -> Option<Self> {
        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(0)).ok()?;
        let mut opening = Vec::with_capacity(TRANSCRIPT_FINGERPRINT_BYTES.min(meta.len() as usize));
        file.take(TRANSCRIPT_FINGERPRINT_BYTES as u64)
            .read_to_end(&mut opening)
            .ok()?;
        if let Some(end) = opening.iter().position(|byte| *byte == b'\n') {
            opening.truncate(end);
        }
        let (id, born) = evidence(file, meta);
        Some(Self { id, born, opening })
    }
}

/// The switches of model recorded in the transcript at `path` past
/// `cursor` — whole lines only, and at most
/// [`zerocode_core::transcript::MAX_TAIL_BYTES`] of them a reading, so an
/// 11.8 MB transcript is read in beats rather than on one (before this the
/// first reading took the whole file into memory at once). A switch early
/// in a long turn is still read however far the file has grown since, where
/// a tail would have scrolled past it (a worker transcript here ran p50
/// 3.8 MB and up to 11.8 MB over fourteen days, against a 256 KiB tail). A
/// line still being written waits for the next reading; a line longer than
/// the window is a tool result — a switch record is about 700 bytes
/// ([`tests::CLAUDE_FALLBACK`]) — and is stepped over; a file that is not
/// the cursor's ([`TranscriptIdentity`]), that shrank, or that no longer
/// holds what the cursor counted ([`Counted`]) is read again from its
/// start — each reading checks one window of what the cursor counted
/// again, in the rounds the cursor carries ([`Round`]). An unchanged file
/// costs one `stat`, one read of its opening bytes, one of the bytes before
/// the cursor and one of a window it counted.
pub(crate) fn scan_fallbacks(path: &Path, cursor: &ScanCursor) -> DeviationScan {
    scan_fallbacks_as(path, cursor, platform_evidence)
}

/// [`scan_fallbacks`] with the file's platform evidence read by `evidence`
/// — the production road passes this platform's.
pub(crate) fn scan_fallbacks_as(
    path: &Path,
    cursor: &ScanCursor,
    evidence: FileEvidence,
) -> DeviationScan {
    use std::io::{Read, Seek, SeekFrom};
    let standing = |next: ScanCursor| DeviationScan {
        switches: Vec::new(),
        next,
    };
    let Ok(mut file) = std::fs::File::open(path) else {
        return standing(cursor.clone());
    };
    let Ok(meta) = file.metadata() else {
        return standing(cursor.clone());
    };
    let size = meta.len();
    let Some(identity) = TranscriptIdentity::of(&mut file, &meta, evidence) else {
        return standing(cursor.clone());
    };
    let mut counted = cursor.counted.clone();
    let stands = cursor.file.as_ref() == Some(&identity)
        && size >= cursor.offset
        && counted.still_stands(&mut file, cursor.offset, identity.id.is_some());
    let (offset, counted) = if stands {
        (cursor.offset, counted)
    } else {
        (0, Counted::default())
    };
    let at = |offset: u64, counted: Counted| ScanCursor::at(identity.clone(), offset, counted);
    if size == offset || file.seek(SeekFrom::Start(offset)).is_err() {
        return standing(at(offset, counted));
    }
    let window = zerocode_core::transcript::MAX_TAIL_BYTES.min(size - offset);
    let mut fresh = Vec::new();
    if file.by_ref().take(window).read_to_end(&mut fresh).is_err() {
        return standing(at(offset, counted));
    }
    let Some(end) = fresh.iter().rposition(|byte| *byte == b'\n') else {
        // No whole line in the window: a full window is a line longer than
        // it, stepped over; anything shorter is a line still being written.
        let longer_than_the_window =
            fresh.len() as u64 == window && window == zerocode_core::transcript::MAX_TAIL_BYTES;
        if !longer_than_the_window {
            return standing(at(offset, counted));
        }
        let past = offset + window;
        return standing(match counted.past(&fresh, &mut file, past) {
            Some(counted) => at(past, counted),
            None => at(offset, counted),
        });
    };
    let past = offset + end as u64 + 1;
    let Some(counted) = counted.past(&fresh[..=end], &mut file, past) else {
        return standing(at(offset, counted));
    };
    let lines: Vec<String> = String::from_utf8_lossy(&fresh[..end])
        .lines()
        .filter(|line| line.contains(CLAUDE_FALLBACK_RECORD))
        .map(str::to_string)
        .collect();
    DeviationScan {
        switches: fallbacks_in(&lines),
        next: at(past, counted),
    }
}

/// Every switch of model a Claude transcript tail records — a decline its
/// CLI answered on the category's route (`model_refusal_fallback`: 15 on
/// this machine, 2026-09-10..24, every one to `claude-opus-4-8`) — oldest
/// first, decided on parsed fields, never on the bytes a tool result quoted.
pub(crate) fn fallbacks_in(lines: &[String]) -> Vec<ModelDeviation> {
    lines
        .iter()
        .filter(|line| line.contains(CLAUDE_FALLBACK_RECORD))
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
            if value["type"] != "system" || value["subtype"] != CLAUDE_FALLBACK_RECORD {
                return None;
            }
            Some(ModelDeviation {
                worker: String::new(),
                dispatch: String::new(),
                source: String::new(),
                key: value["uuid"].as_str()?.to_string(),
                from: value["originalModel"].as_str()?.to_string(),
                to: value["fallbackModel"].as_str()?.to_string(),
                category: value["apiRefusalCategory"].as_str().map(str::to_string),
                scope: value["scope"].as_str().map(str::to_string),
                at_ms: written_at(&value),
            })
        })
        .collect()
}

/// A wall the pane's own conversation last ended at, as the mail pointer
/// reads it (t-6560): which wall, the agent's words, and until when it
/// stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneWall {
    pub(crate) cause: StallCause,
    pub(crate) line: Text,
    /// The reset the record names and the grace after it, or the longest a
    /// wall with no reset stands — `QUOTA_WAIT_POLICY`, the one table.
    pub(crate) stands_until_ms: i64,
}

impl PaneWall {
    pub(crate) const fn stands(&self, now_ms: i64) -> bool {
        now_ms < self.stands_until_ms
    }
}

/// Whether any wall row of `agent`'s reads a record — asked before a
/// transcript is opened, so an agent whose walls are screen words only (zo)
/// or that nobody measured costs no file read.
fn reads_pane_walls(agent: &str) -> bool {
    STALL_MARKERS.iter().any(|rule| {
        rule.agent == agent
            && rule.cause.answers_every_prompt()
            && !matches!(rule.transcript, TranscriptRule::None)
    })
}

/// The wall `agent`'s conversation last ended at, read off the bounded tail
/// of its transcript at `transcript_path` — the production road, one file
/// read.
pub(crate) fn pane_wall_for(agent: &str, transcript_path: &Path) -> Option<PaneWall> {
    if !reads_pane_walls(agent) {
        return None;
    }
    let lines = zerocode_core::transcript::tail_lines(transcript_path)?;
    pane_wall_in(agent, &lines)
}

/// The wall `agent`'s conversation last ended at, in its transcript's tail
/// lines (oldest first). Pure.
///
/// The newest answer decides, as it does for the wall's first witness: an
/// ordinary answer after the wall is a conversation that went on, and a prompt
/// after it — the pointer's own, a person's — is not an answer. A record with
/// no time of its own is no wall here: a hold that cannot say until when would
/// stand for as long as the record does.
pub(crate) fn pane_wall_in(agent: &str, lines: &[String]) -> Option<PaneWall> {
    STALL_MARKERS
        .iter()
        .filter(|rule| rule.agent == agent && rule.cause.answers_every_prompt())
        .find_map(|rule| {
            let found = read_transcript(rule, lines)?;
            let at_ms = found.at_ms?;
            Some(PaneWall {
                cause: rule.cause,
                line: found.line,
                stands_until_ms: zerocode_core::orchestration::wall_stands_until(
                    at_ms,
                    found.resets_at_ms,
                ),
            })
        })
}

/// What one transcript reader found: where, the words, the record's own
/// identity, when it was written and the reset it names, when it carries them.
struct Found {
    source: &'static str,
    line: Text,
    key: Option<String>,
    at_ms: Option<i64>,
    resets_at_ms: Option<i64>,
    /// The request the record answers (`requestId`) and the record it
    /// follows (`parentUuid`), in a Claude transcript — what binds a
    /// decline's category record to its error record (t-7153). A rollout
    /// carries neither.
    request: Option<String>,
    parent: Option<String>,
}

/// When a record was written, off its own `timestamp` field — the same field
/// in a Claude transcript and a Codex rollout.
fn written_at(record: &serde_json::Value) -> Option<i64> {
    record
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(zerocode_core::civil::epoch_ms_of_iso)
}

fn read_transcript(rule: &StallMarkerRule, lines: &[String]) -> Option<Found> {
    match rule.transcript {
        TranscriptRule::None => None,
        TranscriptRule::CodexRollout(errors) => codex_rollout_marker(lines, errors),
        TranscriptRule::ClaudeJsonl { errors, says } => {
            claude_transcript_marker(lines, errors, says, rule.cause.needs_the_last_word())
        }
    }
}

fn clipped(line: &str) -> Text {
    Text::from(line.chars().take(MARKER_LINE_MAX).collect::<String>())
}

/// Codex's own turn edges, as `zerocode_core::transcript` reads them.
const CODEX_TURN_EDGES: [&str; 3] = ["task_started", "task_complete", "turn_aborted"];

/// The NEWEST turn edge in a Codex rollout tail, when it closed on one of
/// `errors`.
///
/// Newest and nothing older: a stop an hour ago followed by a clean turn is
/// a session that recovered — and a prompt after the stop opens a
/// `task_started` edge, so the newest edge is also the last word. A
/// `function_call_output` quoting the words is a tool result, not an edge —
/// decided on parsed `type` fields, never on the bytes.
fn codex_rollout_marker(lines: &[String], errors: &[CodexError]) -> Option<Found> {
    for line in lines.iter().rev() {
        let line = line.trim();
        if !line.contains("event_msg") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("event_msg") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        let Some(kind) = payload.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if !CODEX_TURN_EDGES.contains(&kind) {
            continue;
        }
        let error = payload.get("error")?;
        let info = error.get("codex_error_info").and_then(|v| v.as_str())?;
        let said = error.get("message").and_then(|v| v.as_str());
        let measured = errors.iter().any(|known| {
            known.info == info
                && known
                    .message_starts
                    .is_none_or(|start| said.is_some_and(|said| said.starts_with(start)))
        });
        if !measured {
            return None;
        }
        return Some(Found {
            source: ROLLOUT,
            line: clipped(said.unwrap_or(info)),
            key: payload
                .get("turn_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            at_ms: written_at(&value),
            // The reset is in the sentence ("try again at Sep 7th, 2026
            // 11:27 AM"), in words nobody here has measured a parser for.
            resets_at_ms: None,
            request: None,
            parent: None,
        });
    }
    None
}

/// Whether a Claude transcript line is an INPUT: a `user` record (a prompt,
/// typed or pasted by any hand) or a `queue-operation` (one waiting to go in).
fn claude_input_record(line: &str) -> bool {
    if !line.contains("\"user\"") && !line.contains("\"queue-operation\"") {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(line).is_ok_and(|value| {
        matches!(
            value.get("type").and_then(|v| v.as_str()),
            Some("user" | "queue-operation")
        )
    })
}

/// The NEWEST assistant record in a Claude transcript tail, when it is an
/// API error this row names.
///
/// Newest, for the rollout's reason: a later ordinary assistant record means
/// the conversation went on. For a wall a `queue-operation` or a `user`
/// record after the error does not — the person may have typed while the
/// agent sat at the wall — so only an assistant record ends the search. For a
/// cause that `needs_the_last_word`, an input after the error takes the
/// marker back.
fn claude_transcript_marker(
    lines: &[String],
    errors: &[&str],
    says: &[&[&str]],
    needs_the_last_word: bool,
) -> Option<Found> {
    for line in lines.iter().rev() {
        let line = line.trim();
        if needs_the_last_word && claude_input_record(line) {
            return None;
        }
        if !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let api_error = value
            .get("isApiErrorMessage")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !api_error {
            return None;
        }
        let text = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .and_then(|parts| {
                parts
                    .iter()
                    .find_map(|part| part.get("text").and_then(|t| t.as_str()))
            })
            .unwrap_or("");
        let named = value
            .get("error")
            .and_then(|v| v.as_str())
            .is_some_and(|error| errors.contains(&error));
        let said = says
            .iter()
            .any(|group| group.iter().all(|phrase| text.contains(phrase)));
        return (named || said).then(|| Found {
            source: TRANSCRIPT,
            line: clipped(text),
            key: value
                .get("uuid")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            at_ms: written_at(&value),
            // The provider's own answer to the refused request, in epoch
            // seconds: `quotaLimits.resetsAt` rides every session and weekly
            // wall on this machine (2.1.280). A model's own cap and a monthly
            // spend cap carry no `quotaLimits`, and a login has no reset.
            resets_at_ms: value
                .get("quotaLimits")
                .and_then(|limits| limits.get("resetsAt"))
                .and_then(serde_json::Value::as_i64)
                .map(|seconds| seconds.saturating_mul(1000)),
            request: value
                .get("requestId")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            parent: value
                .get("parentUuid")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    None
}

/* ---- step ① of a handover: the WIP commit ------------------------------ */

/// Commit whatever the walled worker left uncommitted in `checkout`, on its
/// behalf — step ① of a handover (§2.3), and only when the run's policy said
/// `--wip-commit`.
///
/// `Ok(None)` is a clean tree (nothing to commit, the step is skipped);
/// `Ok(Some(sha))` is the commit made; `Err` is git's own word, which aborts
/// the handover with news — a tree the window cannot commit is a tree the
/// window should not end a worker in. `--no-verify`, deliberately: a
/// pre-commit hook that runs a gate is somebody's policy for THEIR commits,
/// and this one is a checkpoint made for them by a machine, named as such.
pub(crate) fn wip_commit(checkout: &Path, message: &str) -> Result<Option<String>, String> {
    let git = |args: &[&str]| -> Result<String, String> {
        let output = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(checkout)
            .args(args)
            .output()
            .map_err(|why| format!("git could not be run: {why}"))?;
        if !output.status.success() {
            let said = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "git {} failed: {}",
                args.first().copied().unwrap_or_default(),
                said.lines().next().unwrap_or_default().trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    if !checkout.is_dir() {
        return Err(format!("the checkout {} is gone", checkout.display()));
    }
    if git(&["status", "--porcelain"])?.trim().is_empty() {
        return Ok(None);
    }
    git(&["add", "-A"])?;
    git(&["commit", "-q", "--no-verify", "-m", message])?;
    Ok(Some(
        git(&["rev-parse", "--short", "HEAD"])?.trim().to_string(),
    ))
}

/// `hh:mm` of an epoch-millisecond instant on the local clock, for the WIP
/// commit's message — "resets 14:27" is what a person reads off it.
pub(crate) fn clock_hhmm(at_ms: i64, local_offset_secs: i64) -> String {
    let local = at_ms.div_euclid(1000) + local_offset_secs;
    let minutes = local.div_euclid(60).rem_euclid(24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// The WIP commit's message: who stopped, and why — at which wall and when
/// it lifts, or at which category of decline (t-6747).
pub(crate) fn wip_message(
    worker: &str,
    cause: &zerocode_core::orchestration::HandoverCause,
    local_offset_secs: i64,
) -> String {
    match cause {
        zerocode_core::orchestration::HandoverCause::QuotaWall {
            provider,
            resets_at_ms,
            ..
        } => {
            let resets = resets_at_ms.map_or_else(
                || "unknown".to_string(),
                |at| clock_hhmm(at, local_offset_secs),
            );
            format!("wip(handover): {worker} stopped at {provider} wall, resets {resets}")
        }
        zerocode_core::orchestration::HandoverCause::ClassifierDecline { category, .. } => {
            format!("wip(handover): {worker} stopped at a {category} classifier decline")
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Step ① against a real repository: a dirty tree is committed on the
    /// worker's behalf under the message the walk names, a clean tree is
    /// skipped, and a directory that is not a repository is git's refusal.
    #[test]
    fn the_wip_commit_commits_a_dirty_tree_skips_a_clean_one_and_refuses_a_non_repo() {
        let dir = tempfile::tempdir().expect("a checkout");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "ZeroCode Test"]);
        git(&repo, &["config", "user.email", "test@zerocode"]);
        git(&repo, &["commit", "--allow-empty", "-q", "-m", "first"]);
        assert_eq!(
            wip_commit(&repo, "wip(handover): w-1").expect("clean"),
            None
        );
        std::fs::write(
            repo.join("half.txt"),
            "half done
",
        )
        .expect("write");
        let message = wip_message("w-3", &wall("codex", Some(1_788_478_149_000)), 9 * 3600);
        let sha = wip_commit(&repo, &message)
            .expect("committed")
            .expect("a sha");
        assert!(sha.len() >= 7, "{sha}");
        assert_eq!(git(&repo, &["log", "-1", "--format=%s"]).trim(), message);
        assert!(git(&repo, &["status", "--porcelain"]).trim().is_empty());
        assert_eq!(wip_commit(&repo, "again").expect("clean again"), None);
        let bare = dir.path().join("not-a-repo");
        std::fs::create_dir_all(&bare).expect("dir");
        assert!(wip_commit(&bare, "x").is_err());
        assert!(wip_commit(&dir.path().join("gone"), "x").is_err());
    }

    /// The commit message reads the reset on the local clock.
    #[test]
    fn the_wip_message_names_the_worker_the_wall_and_the_reset_on_the_local_clock() {
        // 2026-09-03T23:29:09Z is 08:29 in Seoul.
        assert_eq!(clock_hhmm(1_788_478_149_000, 9 * 3600), "08:29");
        assert_eq!(clock_hhmm(1_788_478_149_000, 0), "23:29");
        assert_eq!(
            wip_message("w-3", &wall("codex", Some(1_788_478_149_000)), 9 * 3600),
            "wip(handover): w-3 stopped at codex wall, resets 08:29"
        );
        assert_eq!(
            wip_message("w-3", &wall("claude", None), 0),
            "wip(handover): w-3 stopped at claude wall, resets unknown"
        );
        // A decline names its category (t-6747).
        assert_eq!(
            wip_message(
                "w-5",
                &zerocode_core::orchestration::HandoverCause::ClassifierDecline {
                    category: "cyber".to_string(),
                    record_key: "declined".to_string(),
                },
                0,
            ),
            "wip(handover): w-5 stopped at a cyber classifier decline"
        );
    }

    fn wall(
        provider: &str,
        resets_at_ms: Option<i64>,
    ) -> zerocode_core::orchestration::HandoverCause {
        zerocode_core::orchestration::HandoverCause::QuotaWall {
            provider: provider.to_string(),
            used_percent: 98,
            resets_at_ms,
        }
    }

    fn lines(held: &[&str]) -> Vec<String> {
        held.iter().map(|line| line.to_string()).collect()
    }

    /// The codex rollout as the real file carries it (2026-09-03 23:29,
    /// `rollout-…01a0699a…jsonl`, ordinal 12), and the TUI's own sentence.
    const CODEX_WALL_EDGE: &str = r#"{"timestamp":"2026-09-03T23:29:11.434Z","ordinal":12,"type":"event_msg","payload":{"type":"task_complete","turn_id":"01a0699a-d4db-77e1-84cb-1eccfba6c4c5","last_agent_message":null,"error":{"message":"You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Sep 7th, 2026 11:27 AM.","codex_error_info":"usage_limit_exceeded"},"started_at":1788478149,"completed_at":1788478151,"duration_ms":1573}}"#;
    const CODEX_CLEAN_EDGE: &str = r#"{"timestamp":"2026-09-03T23:40:00.000Z","ordinal":20,"type":"event_msg","payload":{"type":"task_complete","turn_id":"01a0699a-ffff-77e1-84cb-1eccfba6c4c5","last_agent_message":"done","error":null}}"#;
    const CODEX_TOOL_OUTPUT_QUOTING_THE_WALL: &str = r#"{"timestamp":"2026-09-03T23:41:00.000Z","ordinal":21,"type":"response_item","payload":{"type":"function_call_output","call_id":"c","output":[{"type":"input_text","text":"grep found: \"codex_error_info\":\"usage_limit_exceeded\" in a log"}]}}"#;
    const CODEX_TUI_LINE: &str = "  You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Sep 7th, 2026 11:27 AM.";

    /// The claude transcript as the real file carries it (2026-08-07, ids
    /// scrubbed), and the screen line.
    pub(crate) const CLAUDE_WALL_RECORD: &str = r#"{"parentUuid":"p","isSidechain":false,"type":"assistant","uuid":"u","timestamp":"2026-08-07T10:20:04.042Z","message":{"id":"m","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","type":"message","content":[{"type":"text","text":"You've hit your session limit · resets 7:30pm (Asia/Seoul)"}]},"requestId":"req_x","error":"rate_limit","isApiErrorMessage":true}"#;
    pub(crate) const CLAUDE_PLAIN_RECORD: &str = r#"{"parentUuid":"u","isSidechain":false,"type":"assistant","uuid":"v","timestamp":"2026-08-07T10:25:04.042Z","message":{"id":"n","model":"claude-fable-5-1","role":"assistant","stop_reason":"end_turn","type":"message","content":[{"type":"text","text":"Continuing with the migration."}]}}"#;
    const CLAUDE_USER_QUOTING_THE_WALL: &str = r#"{"parentUuid":"v","isSidechain":false,"type":"user","uuid":"w","message":{"role":"user","content":[{"type":"tool_result","content":"log says: You've hit your session limit · resets 7:30pm"}]}}"#;
    const CLAUDE_SCREEN_LINE: &str = "You've hit your session limit · resets 4:10am (Asia/Seoul)";
    /// A model's own cap as the real file carried it (2026-09-22, w-5922,
    /// ids scrubbed): the same `rate_limit` record, a sentence naming the model.
    const CLAUDE_MODEL_CAP_RECORD: &str = r#"{"parentUuid":"p","isSidechain":false,"type":"assistant","uuid":"u","timestamp":"2026-09-21T21:08:12.346Z","message":{"id":"m","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","type":"message","content":[{"type":"text","text":"You've reached your Fable limit. Run /usage-credits to continue or switch models with /model."}]},"requestId":"req_y","error":"rate_limit","isApiErrorMessage":true}"#;
    const CLAUDE_MODEL_CAP_SCREEN_LINE: &str = "⎿  You've reached your Fable limit. Run /usage-credits to continue or switch models with /model.";

    /// zo's hint (`usage_limit_hint`, zo-ide 2026-09-07) as the pane shows it.
    const ZO_STATUS_LINE: &str = "This account's usage limit … resets in 2h 17m. With a quota fallback configured (/smart) the turn continues on another model; otherwise wait or /model.";

    /// Every measured row finds its own CLI's words, in the transcript and
    /// on the screen, and names where it read them.
    #[test]
    fn each_agents_measured_wall_is_a_marker_from_its_transcript_or_its_screen() {
        let codex = marker_in(
            "codex",
            None,
            Some(&lines(&[CODEX_CLEAN_EDGE, CODEX_WALL_EDGE])),
        )
        .expect("codex's rollout edge");
        assert_eq!(codex.source, "rollout");
        assert!(
            codex
                .line
                .as_str()
                .starts_with("You've hit your usage limit."),
            "{}",
            codex.line.as_str()
        );
        let codex_screen = marker_in("codex", Some(CODEX_TUI_LINE), None).expect("codex's TUI");
        assert_eq!(codex_screen.source, "screen");
        assert_eq!(codex_screen.line.as_str(), CODEX_TUI_LINE.trim());

        let claude = marker_in(
            "claude",
            None,
            Some(&lines(&[CLAUDE_PLAIN_RECORD, CLAUDE_WALL_RECORD])),
        )
        .expect("claude's api-error record");
        assert_eq!(claude.source, "transcript");
        assert_eq!(
            claude.line.as_str(),
            "You've hit your session limit · resets 7:30pm (Asia/Seoul)"
        );
        let claude_screen = marker_in(
            "claude",
            Some(&format!("some output\n{CLAUDE_SCREEN_LINE}\n> ")),
            None,
        )
        .expect("claude's screen");
        assert_eq!(claude_screen.source, "screen");
        assert_eq!(claude_screen.line.as_str(), CLAUDE_SCREEN_LINE);
        assert!(
            marker_in("claude", Some("You've hit your limit · resets 5pm"), None).is_some(),
            "the design's older spelling was dropped"
        );
        // A model's own cap: the record and the screen line both name the
        // model, and the marker reads the family around it.
        let model_cap = marker_in(
            "claude",
            None,
            Some(&lines(&[CLAUDE_PLAIN_RECORD, CLAUDE_MODEL_CAP_RECORD])),
        )
        .expect("claude's model-cap record");
        assert_eq!(model_cap.source, "transcript");
        assert!(
            model_cap.line.as_str().starts_with("You've reached your"),
            "{}",
            model_cap.line.as_str()
        );
        let model_cap_screen = marker_in(
            "claude",
            Some(&format!("some output\n{CLAUDE_MODEL_CAP_SCREEN_LINE}\n> ")),
            None,
        )
        .expect("claude's model-cap screen line");
        assert_eq!(model_cap_screen.source, "screen");
        assert!(
            marker_in("claude", Some("You've reached your goal, nice"), None).is_none(),
            "a sentence without `limit` is not a wall"
        );

        let zo = marker_in("zo", Some(&format!("…\n{ZO_STATUS_LINE}")), None).expect("zo's hint");
        assert_eq!(zo.source, "screen");
        assert!(zo.line.as_str().starts_with("This account's usage limit"));
        assert!(
            has_rule("codex", StallCause::QuotaWall)
                && has_rule("claude", StallCause::QuotaWall)
                && has_rule("zo", StallCause::QuotaWall)
        );
    }

    /// A one-shot run's answer is read with this table's own phrases: the
    /// walls a pane shows, and each CLI's measured request to sign in — and
    /// another CLI's words, or an error nobody measured, are no cause.
    #[test]
    fn a_one_shot_says_its_wall_and_its_login_in_the_words_this_table_reads() {
        for (agent, said, cause) in [
            (
                "claude",
                "You've hit your session limit · resets 4:10am (Asia/Seoul)",
                Some(StallCause::QuotaWall),
            ),
            (
                "claude",
                "You've reached your Fable limit. Run /usage-credits to continue.",
                Some(StallCause::QuotaWall),
            ),
            (
                "claude",
                "Not logged in · Please run /login",
                Some(StallCause::LoginWall),
            ),
            (
                "codex",
                "You've hit your usage limit. Visit https://example.invalid or try again at 4:10 PM.",
                Some(StallCause::QuotaWall),
            ),
            (
                "codex",
                "unexpected status 401 Unauthorized: Missing bearer or basic authentication in header",
                Some(StallCause::LoginWall),
            ),
            ("claude", "model not found", None),
            ("codex", "Please run /login", None),
        ] {
            assert_eq!(one_shot_cause(agent, said), cause, "{agent}: {said}");
        }
    }

    /// The "429 grep" trap, row by row: a bare 429, a tool result quoting
    /// the words, a person's prompt quoting them, a wall the session has
    /// since recovered from, an agent nobody measured — none is a marker.
    #[test]
    fn a_quoted_wall_or_a_recovered_one_is_not_a_marker() {
        for agent in ["codex", "claude", "zo"] {
            assert!(
                marker_in(agent, Some("HTTP 429 Too Many Requests\nretrying…"), None).is_none(),
                "{agent}: a bare 429 was a marker"
            );
        }
        // A rollout whose newest edge is clean, with the wall an hour back.
        assert!(
            marker_in(
                "codex",
                None,
                Some(&lines(&[CODEX_WALL_EDGE, CODEX_CLEAN_EDGE]))
            )
            .is_none(),
            "a recovered codex session was still at the wall"
        );
        // A tool output quoting the words, after a clean edge.
        assert!(
            marker_in(
                "codex",
                None,
                Some(&lines(&[
                    CODEX_CLEAN_EDGE,
                    CODEX_TOOL_OUTPUT_QUOTING_THE_WALL
                ]))
            )
            .is_none(),
            "a tool result quoting the wall was a marker"
        );
        // A claude conversation that went on, and a person's row quoting it.
        assert!(
            marker_in(
                "claude",
                None,
                Some(&lines(&[CLAUDE_WALL_RECORD, CLAUDE_PLAIN_RECORD]))
            )
            .is_none(),
            "a recovered claude session was still at the wall"
        );
        assert!(
            marker_in(
                "claude",
                None,
                Some(&lines(&[CLAUDE_PLAIN_RECORD, CLAUDE_USER_QUOTING_THE_WALL]))
            )
            .is_none(),
            "a user row quoting the wall was a marker"
        );
        // zo's words need both halves of the status line.
        assert!(marker_in("zo", Some("usage limit reached last week, fine now"), None).is_none());
        // An agent with no row: nothing, whatever its screen says.
        assert!(!has_rule("cursor", StallCause::QuotaWall));
        assert!(marker_in("cursor", Some(CLAUDE_SCREEN_LINE), None).is_none());
    }

    /// The production road reads the transcript's bounded tail through
    /// `zerocode_core::transcript` and asks nothing of a missing file.
    #[test]
    fn the_production_road_reads_the_tail_of_a_real_file() {
        let dir = tempfile::tempdir().expect("a transcript dir");
        let rollout = dir.path().join("rollout-a.jsonl");
        std::fs::write(&rollout, format!("{CODEX_CLEAN_EDGE}\n{CODEX_WALL_EDGE}\n"))
            .expect("write");
        let found = marker_for("codex", None, Some(&rollout)).expect("the rollout's edge");
        assert_eq!(found.source, "rollout");
        assert!(
            marker_for("codex", None, Some(&dir.path().join("gone.jsonl"))).is_none(),
            "a missing file was a marker"
        );
        // The screen still speaks when the file is silent.
        assert!(
            marker_for(
                "codex",
                Some(CODEX_TUI_LINE),
                Some(&dir.path().join("gone.jsonl"))
            )
            .is_some()
        );
    }

    /// The transient error as the real file carries it: w-4525's Claude
    /// transcript, 2026-09-17T06:27:49.769Z (`d4e81823-….jsonl`, lines
    /// 33–36; usage block, cwd and session fields dropped), and the prompt
    /// the mail pointer typed two and a half minutes later.
    pub(crate) const CLAUDE_TRANSIENT_RECORD: &str = r#"{"parentUuid":"675b81e4-a575-499c-aeae-a8c5b9c42723","isSidechain":false,"type":"assistant","uuid":"8ff6adcf-19c1-45a5-b351-c3aa70bd37b7","timestamp":"2026-09-17T06:27:49.769Z","message":{"id":"db6f0a4d-9ce0-4e18-acf4-f6aa9f749d26","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","stop_sequence":"","type":"message","content":[{"type":"text","text":"API Error: The response stopped arriving. The response above may be incomplete."}]},"error":"server_error","truncatedAfterOutput":true,"isApiErrorMessage":true,"version":"2.1.274"}"#;
    pub(crate) const CLAUDE_TURN_DURATION: &str = r#"{"parentUuid":"8ff6adcf-19c1-45a5-b351-c3aa70bd37b7","isSidechain":false,"type":"system","subtype":"turn_duration","durationMs":182093,"messageCount":20,"timestamp":"2026-09-17T06:27:49.796Z","uuid":"aa074450-0dc1-468d-ad90-9b8a8fe740be","isMeta":false,"version":"2.1.274"}"#;
    const CLAUDE_SNAPSHOT: &str = r#"{"type":"file-history-snapshot","messageId":"74750cf8-84de-4bb9-811b-75a7907807db","snapshot":{"messageId":"74750cf8-84de-4bb9-811b-75a7907807db","trackedFileBackups":{},"timestamp":"2026-09-17T06:30:17.270Z"},"isSnapshotUpdate":false}"#;
    pub(crate) const CLAUDE_POINTER_PROMPT: &str = r#"{"parentUuid":"aa074450-0dc1-468d-ad90-9b8a8fe740be","isSidechain":false,"promptId":"65c54239-ddbf-4f4b-a7c5-45b5f15bc50f","type":"user","message":{"role":"user","content":"\nYou have 1 orchestration message. Run `zerocode-orc check`."},"uuid":"74750cf8-84de-4bb9-811b-75a7907807db","timestamp":"2026-09-17T06:30:17.215Z","origin":{"kind":"human"},"promptSource":"typed","version":"2.1.274"}"#;
    /// The words on this machine that are NOT transient, in the same record
    /// shape: a login that expired (2.1.263) and a safeguard (2.1.270).
    const CLAUDE_LOGIN_RECORD: &str = r#"{"type":"assistant","uuid":"l","message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"Login expired · Please run /login"}]},"error":"authentication_failed","isApiErrorMessage":true}"#;
    const CLAUDE_SAFEGUARD_RECORD: &str = r#"{"type":"assistant","uuid":"g","message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"API Error: Fable 5.1's safeguards flagged this message"}]},"error":"invalid_request","isApiErrorMessage":true}"#;

    /// Codex's two measured transient edges (`rollout-2026-08-11T15-49-22-…`
    /// ordinal 6249 and `rollout-2026-08-14T02-39-37-…` ordinal 19927), a
    /// 400 closed as `other` (`rollout-2026-08-12T16-14-17-…`), and the edge
    /// a prompt after the stop opens.
    const CODEX_OVERLOADED_EDGE: &str = r#"{"timestamp":"2026-08-11T21:00:59.672Z","ordinal":6249,"type":"event_msg","payload":{"type":"task_complete","turn_id":"019ff278-9ca6-7f60-ad61-3e837c5b85fd","last_agent_message":null,"error":{"message":"Selected model is at capacity. Please try a different model.","codex_error_info":"server_overloaded"}}}"#;
    const CODEX_DISCONNECTED_EDGE: &str = r#"{"timestamp":"2026-08-14T10:20:30.711Z","ordinal":19927,"type":"event_msg","payload":{"type":"task_complete","turn_id":"fbc88853-6bf4-4dd9-9f6d-3fee51319b2c","last_agent_message":null,"error":{"message":"stream disconnected before completion: error sending request for url (https://chatgpt.com/backend-api/codex/responses)","codex_error_info":"other"}}}"#;
    const CODEX_BAD_REQUEST_EDGE: &str = r#"{"timestamp":"2026-08-12T07:14:21.041Z","ordinal":9,"type":"event_msg","payload":{"type":"task_complete","turn_id":"019ff4d2-0000-7a21-bd10-baf19e65bf52","last_agent_message":null,"error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'gpt-5.1' model is not supported\"}}","codex_error_info":"other"}}}"#;
    const CODEX_NEXT_TURN_STARTED: &str = r#"{"timestamp":"2026-08-11T21:01:19.000Z","ordinal":6260,"type":"event_msg","payload":{"type":"task_started","turn_id":"019ff27a-0000-7f60-ad61-3e837c5b85fd"}}"#;

    /// Each measured transient error is a marker with the record's own
    /// identity, read off the record that ends the conversation; the
    /// bookkeeping Claude writes after it does not take it back.
    #[test]
    fn a_measured_transient_error_is_a_marker_keyed_by_its_record() {
        let claude = transient_error_in(
            "claude",
            &lines(&[
                CLAUDE_PLAIN_RECORD,
                CLAUDE_TRANSIENT_RECORD,
                CLAUDE_TURN_DURATION,
                CLAUDE_SNAPSHOT,
            ]),
        )
        .expect("w-4525's stop");
        assert_eq!(claude.source, "transcript");
        assert_eq!(claude.key, "8ff6adcf-19c1-45a5-b351-c3aa70bd37b7");
        assert!(
            claude
                .line
                .as_str()
                .starts_with("API Error: The response stopped arriving."),
            "{}",
            claude.line.as_str()
        );

        let overloaded =
            transient_error_in("codex", &lines(&[CODEX_CLEAN_EDGE, CODEX_OVERLOADED_EDGE]))
                .expect("codex at capacity");
        assert_eq!(overloaded.source, "rollout");
        assert_eq!(overloaded.key, "019ff278-9ca6-7f60-ad61-3e837c5b85fd");
        let dropped = transient_error_in("codex", &lines(&[CODEX_DISCONNECTED_EDGE]))
            .expect("codex's dropped stream");
        assert!(
            dropped
                .line
                .as_str()
                .starts_with("stream disconnected before completion")
        );
        assert!(
            has_rule("claude", StallCause::TransientApiError)
                && has_rule("codex", StallCause::TransientApiError)
        );
    }

    /// The error has to be the LAST word and a measured one: a prompt after
    /// it (the pointer w-4525 was woken by), a new Codex turn, a recovered
    /// session, a login, a safeguard, a 400 closed as `other`, the wall
    /// itself, an agent with no row — none is a transient marker. And the
    /// wall's road is unchanged by the stricter reading: a prompt after a
    /// wall still leaves the wall standing.
    #[test]
    fn a_transient_error_somebody_answered_or_nobody_measured_is_not_a_marker() {
        for (why, held) in [
            (
                "a prompt after the error",
                vec![
                    CLAUDE_TRANSIENT_RECORD,
                    CLAUDE_TURN_DURATION,
                    CLAUDE_SNAPSHOT,
                    CLAUDE_POINTER_PROMPT,
                ],
            ),
            (
                "a recovered conversation",
                vec![
                    CLAUDE_TRANSIENT_RECORD,
                    CLAUDE_POINTER_PROMPT,
                    CLAUDE_PLAIN_RECORD,
                ],
            ),
            ("a login", vec![CLAUDE_LOGIN_RECORD]),
            ("a safeguard", vec![CLAUDE_SAFEGUARD_RECORD]),
            ("the wall", vec![CLAUDE_WALL_RECORD]),
        ] {
            assert!(
                transient_error_in("claude", &lines(&held)).is_none(),
                "claude: {why} was a transient marker"
            );
        }
        for (why, held) in [
            (
                "a new turn",
                vec![CODEX_OVERLOADED_EDGE, CODEX_NEXT_TURN_STARTED],
            ),
            ("a 400 closed as other", vec![CODEX_BAD_REQUEST_EDGE]),
            ("the wall", vec![CODEX_WALL_EDGE]),
        ] {
            assert!(
                transient_error_in("codex", &lines(&held)).is_none(),
                "codex: {why} was a transient marker"
            );
        }
        assert!(!has_rule("zo", StallCause::TransientApiError));
        assert!(transient_error_in("zo", &lines(&[CLAUDE_TRANSIENT_RECORD])).is_none());
        assert!(
            marker_in(
                "claude",
                None,
                Some(&lines(&[CLAUDE_WALL_RECORD, CLAUDE_POINTER_PROMPT]))
            )
            .is_some(),
            "a prompt after a wall took the wall back"
        );
        // And a transient error is never a wall.
        assert!(marker_in("claude", None, Some(&lines(&[CLAUDE_TRANSIENT_RECORD]))).is_none());
    }

    /// The production road: one bounded tail read of a real file.
    #[test]
    fn the_transient_road_reads_the_tail_of_a_real_file() {
        let dir = tempfile::tempdir().expect("a transcript dir");
        let transcript = dir.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            format!("{CLAUDE_PLAIN_RECORD}\n{CLAUDE_TRANSIENT_RECORD}\n{CLAUDE_TURN_DURATION}\n"),
        )
        .expect("write");
        let found = transient_error_for("claude", &transcript).expect("the stop");
        assert_eq!(found.key, "8ff6adcf-19c1-45a5-b351-c3aa70bd37b7");
        assert!(transient_error_for("claude", &dir.path().join("gone.jsonl")).is_none());
        assert!(transient_error_for("zo", &transcript).is_none());
    }

    /* ---- the pane's own wall, as the mail pointer reads it (t-6560) ---- */

    /// The walls the coordinator's pane met, as its transcript carries them
    /// (`68b2661a-…`, ids scrubbed): a session wall and a weekly one with the
    /// provider's `quotaLimits` beside them, a model's own cap with none, and
    /// an expired login the CLI answered itself.
    const CLAUDE_SESSION_WALL_RECORD: &str = r#"{"type":"assistant","uuid":"s","timestamp":"2026-09-23T08:55:23.536Z","message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"You've hit your session limit · resets 6:30pm (Asia/Seoul)"}]},"requestId":"req_s","error":"rate_limit","isApiErrorMessage":true,"apiErrorStatus":429,"quotaLimits":{"status":"rejected","resetsAt":1790155800,"rateLimitType":"five_hour"}}"#;
    const CLAUDE_WEEKLY_WALL_RECORD: &str = r#"{"type":"assistant","uuid":"w","timestamp":"2026-09-20T02:19:15.811Z","message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"You've hit your weekly limit · resets 11am (Asia/Seoul)"}]},"requestId":"req_w","error":"rate_limit","isApiErrorMessage":true,"apiErrorStatus":429,"quotaLimits":{"status":"rejected","resetsAt":1789956000,"rateLimitType":"seven_day"}}"#;
    const CLAUDE_LOGIN_EXPIRED_RECORD: &str = r#"{"type":"assistant","uuid":"l","timestamp":"2026-09-20T03:10:33.349Z","message":{"model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"Login expired · Please run /login"}]},"error":"authentication_failed","isApiErrorMessage":true}"#;

    fn at(iso: &str) -> i64 {
        zerocode_core::civil::epoch_ms_of_iso(iso).expect("a stamp")
    }

    /// Each wall is read off the pane's last answer with the window the wall
    /// table gives it: the reset the provider named and the grace after it,
    /// or — no reset, or one further than the longest wait — the longest wait
    /// from the moment the answer was written.
    #[test]
    fn a_pane_wall_stands_for_the_window_its_own_record_gives_it() {
        let policy = zerocode_core::orchestration::QUOTA_WAIT_POLICY;
        let read = |held: &[&str]| pane_wall_in("claude", &lines(held));

        let session =
            read(&[CLAUDE_PLAIN_RECORD, CLAUDE_SESSION_WALL_RECORD]).expect("a session wall");
        assert_eq!(session.cause, StallCause::QuotaWall);
        assert_eq!(
            session.stands_until_ms,
            1_790_155_800_000 + policy.slack_ms,
            "the session wall stands until its reset and the grace after it"
        );
        assert!(session.stands(at("2026-09-23T09:30:00.000Z")));
        assert!(!session.stands(at("2026-09-23T09:33:00.000Z")));

        // Reset 23.7 hours away: past the longest wait, which then decides.
        let weekly = read(&[CLAUDE_WEEKLY_WALL_RECORD]).expect("a weekly wall");
        assert_eq!(
            weekly.stands_until_ms,
            at("2026-09-20T02:19:15.811Z") + policy.max_wait_ms
        );

        let cap = read(&[CLAUDE_MODEL_CAP_RECORD]).expect("a model's own cap");
        assert_eq!(cap.cause, StallCause::QuotaWall);
        assert_eq!(
            cap.stands_until_ms,
            at("2026-09-21T21:08:12.346Z") + policy.max_wait_ms,
            "a cap that names no reset stands for the longest wait"
        );

        let login =
            read(&[CLAUDE_PLAIN_RECORD, CLAUDE_LOGIN_EXPIRED_RECORD]).expect("a login wall");
        assert_eq!(login.cause, StallCause::LoginWall);
        assert_eq!(login.line.as_str(), "Login expired · Please run /login");
        assert_eq!(
            login.stands_until_ms,
            at("2026-09-20T03:10:33.349Z") + policy.max_wait_ms
        );

        // A prompt after the wall — the pointer's own line — is no answer.
        assert!(read(&[CLAUDE_SESSION_WALL_RECORD, CLAUDE_POINTER_PROMPT]).is_some());

        let codex = pane_wall_in("codex", &lines(&[CODEX_CLEAN_EDGE, CODEX_WALL_EDGE]))
            .expect("codex's rollout edge");
        assert_eq!(codex.cause, StallCause::QuotaWall);
        assert_eq!(
            codex.stands_until_ms,
            at("2026-09-03T23:29:11.434Z") + policy.max_wait_ms
        );
        assert!(reads_pane_walls("claude") && reads_pane_walls("codex"));
    }

    /// No wall where the conversation went on, where the stop is one the
    /// next prompt cures, where the record cannot say when it was written,
    /// or where the only words are on a screen.
    #[test]
    fn a_pane_that_answered_since_or_stopped_on_something_else_is_not_walled() {
        let read = |held: &[&str]| pane_wall_in("claude", &lines(held));
        for (why, held) in [
            (
                "an answer after the wall",
                vec![
                    CLAUDE_SESSION_WALL_RECORD,
                    CLAUDE_POINTER_PROMPT,
                    CLAUDE_PLAIN_RECORD,
                ],
            ),
            (
                "an answer after the login",
                vec![CLAUDE_LOGIN_EXPIRED_RECORD, CLAUDE_PLAIN_RECORD],
            ),
            ("a transient error", vec![CLAUDE_TRANSIENT_RECORD]),
            ("a safeguard", vec![CLAUDE_SAFEGUARD_RECORD]),
            ("a login record with no time", vec![CLAUDE_LOGIN_RECORD]),
            (
                "a tool result quoting a wall",
                vec![CLAUDE_PLAIN_RECORD, CLAUDE_USER_QUOTING_THE_WALL],
            ),
        ] {
            assert!(read(&held).is_none(), "claude: {why} was a wall");
        }
        assert!(
            pane_wall_in("codex", &lines(&[CODEX_WALL_EDGE, CODEX_CLEAN_EDGE])).is_none(),
            "a recovered codex session was still walled"
        );
        // zo's wall is screen words only: the pointer never guesses from a
        // screen, which cannot say when its line was written.
        assert!(!reads_pane_walls("zo") && !reads_pane_walls("cursor"));
        assert!(pane_wall_in("zo", &lines(&[CLAUDE_SESSION_WALL_RECORD])).is_none());
    }

    /// The production road: the table first, then one bounded tail read.
    #[test]
    fn the_pane_wall_road_reads_the_tail_of_a_real_file() {
        let dir = tempfile::tempdir().expect("a transcript dir");
        let transcript = dir.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            format!(
                "{CLAUDE_PLAIN_RECORD}\n{CLAUDE_POINTER_PROMPT}\n{CLAUDE_SESSION_WALL_RECORD}\n"
            ),
        )
        .expect("write");
        let wall = pane_wall_for("claude", &transcript).expect("the wall");
        assert_eq!(wall.cause, StallCause::QuotaWall);
        assert!(pane_wall_for("claude", &dir.path().join("gone.jsonl")).is_none());
        assert!(pane_wall_for("zo", &transcript).is_none());
    }

    /// The coordinator's typed pointers replayed through the reader the hold
    /// uses (t-6560): how many the window typed, how many met a wall and a
    /// refused request, and how many the hold would have let through.
    ///
    /// The replay walks a Claude transcript in order and keeps the last
    /// answer the pane gave; a typed pointer the hold keeps back takes the
    /// answer it caused out of the replay with it, so the next pointer is
    /// judged against the pane as the hold would have left it. It adds
    /// nothing the record does not hold: an auto-continuation a typed line
    /// cancelled stays cancelled here, and a hold still standing when a burst
    /// ends is counted as the one line owed at its lift.
    ///
    /// ```sh
    /// ZEROCODE_POINTER_WALL_REPLAY=<transcript.jsonl> cargo test -p zerocode-shell \
    ///   --bin zerocode-shell quota_wall::tests::the_typed_pointers_a_wall_hold_would_have_kept_back \
    ///   -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "a measurement over a transcript on this machine, not a rule"]
    fn the_typed_pointers_a_wall_hold_would_have_kept_back() {
        use std::io::BufRead;
        let Some(path) = std::env::var_os("ZEROCODE_POINTER_WALL_REPLAY") else {
            println!("MEASURE skipped: set ZEROCODE_POINTER_WALL_REPLAY to a transcript");
            return;
        };
        /// Pointers closer than this are one burst.
        const BURST_GAP_MS: i64 = 10_000;
        #[derive(Default)]
        struct Burst {
            first_ms: i64,
            last_ms: i64,
            typed: usize,
            refused: usize,
            kept_typed: usize,
            kept_refused: usize,
            held_at_end: bool,
        }
        let pointer = |text: &str| -> bool {
            let bare = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("<pasted_content"))
                .filter(|line| !line.trim_start().starts_with("</pasted_content"))
                .collect::<Vec<_>>()
                .join("\n");
            (1..=64).any(|count| {
                bare.trim() == zerocode_core::orchestration::pointer_text(count).trim()
            })
        };
        let file = std::fs::File::open(&path).expect("the transcript");
        let mut bursts: Vec<Burst> = Vec::new();
        let mut last_answer: Option<String> = None;
        let mut swallow_next_answer = false;
        let mut last_kept_pointer = false;
        let mut last_pointer_ms = i64::MIN;
        let mut held_now = false;
        for line in std::io::BufReader::new(file).lines() {
            let Ok(line) = line else { continue };
            let is_user = line.contains("\"type\":\"user\"");
            let is_answer = line.contains("\"type\":\"assistant\"");
            if !is_user && !is_answer {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let written = written_at(&value).unwrap_or_default();
            if is_user {
                let typed = value.get("promptSource").and_then(|v| v.as_str()) == Some("typed");
                let text = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                if !typed || !pointer(text) {
                    continue;
                }
                if written.saturating_sub(last_pointer_ms) > BURST_GAP_MS {
                    if let Some(open) = bursts.last_mut() {
                        open.held_at_end = held_now;
                    }
                    bursts.push(Burst {
                        first_ms: written,
                        ..Burst::default()
                    });
                }
                last_pointer_ms = written;
                let burst = bursts.last_mut().expect("a burst");
                burst.last_ms = written;
                burst.typed += 1;
                let wall = last_answer
                    .as_ref()
                    .and_then(|answer| pane_wall_in("claude", std::slice::from_ref(answer)));
                held_now = wall.is_some_and(|wall| wall.stands(written));
                last_kept_pointer = !held_now;
                if last_kept_pointer {
                    burst.kept_typed += 1;
                }
                swallow_next_answer = held_now;
                continue;
            }
            // An answer: the one a pointer caused, or anybody else's turn.
            let refused = value.get("isApiErrorMessage").and_then(|v| v.as_bool()) == Some(true)
                && value.get("requestId").is_some();
            if let Some(burst) = bursts.last_mut()
                && written.saturating_sub(burst.last_ms) <= BURST_GAP_MS
                && refused
            {
                burst.refused += 1;
                if last_kept_pointer && !swallow_next_answer {
                    burst.kept_refused += 1;
                }
            }
            if std::mem::take(&mut swallow_next_answer) {
                continue;
            }
            last_kept_pointer = false;
            last_answer = Some(line);
        }
        if let Some(open) = bursts.last_mut() {
            open.held_at_end = held_now;
        }
        let mut totals = (0, 0, 0, 0, 0);
        println!("MEASURE burst start(UTC) end typed refused | kept refused owed-at-lift");
        for burst in bursts.iter().filter(|burst| burst.typed >= 3) {
            let owed = usize::from(burst.held_at_end);
            println!(
                "MEASURE {} {} {} {} | {} {} {}",
                zerocode_core::civil::iso_utc_of(burst.first_ms),
                zerocode_core::civil::iso_utc_of(burst.last_ms),
                burst.typed,
                burst.refused,
                burst.kept_typed,
                burst.kept_refused,
                owed
            );
            totals.0 += burst.typed;
            totals.1 += burst.refused;
            totals.2 += burst.kept_typed;
            totals.3 += burst.kept_refused;
            totals.4 += owed;
        }
        println!(
            "MEASURE bursts>=3 typed={} refused={} | kept={} kept_refused={} owed_at_lift={}",
            totals.0, totals.1, totals.2, totals.3, totals.4
        );
        let all: usize = bursts.iter().map(|burst| burst.typed).sum();
        let kept: usize = bursts.iter().map(|burst| burst.kept_typed).sum();
        println!("MEASURE every typed pointer={all} kept by the hold's rule={kept}");
    }

    /* ---- the classifier decline (t-6747) ------------------------------ */

    /// A decline with no fallback as the real file carries it (the
    /// coordinator transcript, 2026-09-24 04:29:48, 2.1.281, ids scrubbed):
    /// the partial the declined request streamed, the CLI's category record,
    /// its error record, and the turn's end — in that order in the file.
    const CLAUDE_DECLINED_PARTIAL: &str = r#"{"parentUuid":"a","isSidechain":false,"type":"assistant","uuid":"part","timestamp":"2026-09-24T04:29:48.706Z","message":{"id":"m","model":"claude-fable-5-1","role":"assistant","type":"message","content":[{"type":"thinking","thinking":""}]},"requestId":"req_d"}"#;
    const CLAUDE_NO_FALLBACK: &str = r#"{"parentUuid":"part","isSidechain":false,"type":"system","subtype":"model_refusal_no_fallback","content":"","level":"warning","originalModel":"claude-fable-5-1","requestId":"req_d","apiRefusalCategory":"cyber","apiRefusalExplanation":"This request triggered restrictions on violative cyber content and was blocked under Anthropic's Usage Policy.","refusedUserMessageUuid":"q","isMeta":false,"uuid":"sys","timestamp":"2026-09-24T04:29:48.712Z","version":"2.1.281"}"#;
    const CLAUDE_DECLINE_ERROR: &str = r#"{"parentUuid":"sys","isSidechain":false,"type":"assistant","uuid":"declined","timestamp":"2026-09-24T04:29:48.711Z","message":{"id":"e","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","type":"message","content":[{"type":"text","text":"API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup). Our intentionally broad safeguards allow us to deliver more capabilities faster, but can sometimes flag legitimate coding and cybersecurity tasks."}]},"requestId":"req_d","error":"invalid_request","isApiErrorMessage":true}"#;
    const CLAUDE_TURN_END: &str = r#"{"parentUuid":"declined","isSidechain":false,"type":"system","subtype":"turn_duration","durationMs":2400,"uuid":"end","timestamp":"2026-09-24T04:29:48.735Z"}"#;
    const CLAUDE_PERSON_TYPED: &str = r#"{"parentUuid":"end","isSidechain":false,"type":"user","uuid":"typed","timestamp":"2026-09-24T04:30:08.774Z","message":{"role":"user","content":"continue"}}"#;
    const CLAUDE_TOOL_QUOTING_THE_DECLINE: &str = r#"{"parentUuid":"x","isSidechain":false,"type":"user","uuid":"quote","timestamp":"2026-09-24T05:00:00.000Z","message":{"role":"user","content":[{"type":"tool_result","content":"grep: API Error: Fable 5.1's safeguards flagged this message"}]}}"#;
    /// A decline the CLI answered on its fallback, as w-5797's transcript
    /// carries it (2026-09-21 14:06:16, 2.1.278, ids scrubbed).
    pub(crate) const CLAUDE_FALLBACK: &str = r#"{"parentUuid":"b","isSidechain":false,"type":"system","subtype":"model_refusal_fallback","content":"Fable 5.1's safeguards flagged this message. Switched to Opus 4.8.\n\nDetails: `[cyber]`","level":"warning","trigger":"refusal","direction":"retry","scope":"session","originalModel":"claude-fable-5-1","fallbackModel":"claude-opus-4-8","requestId":"req_f","apiRefusalCategory":"cyber","retractedMessageUuids":["r"],"refusedUserMessageUuid":"q","isMeta":false,"uuid":"switch-1","timestamp":"2026-09-21T14:06:16.566Z","version":"2.1.278"}"#;
    /// Claude Code's pause dialog as a coordinator read it off w-5770 on
    /// 2026-09-21, in 2.1.281's own labels.
    const CLAUDE_DIALOG_SCREEN: &str = "\
 Session paused
 Fable 5.1's safeguards flagged this message. Our intentionally broad safeguards allow us to deliver more capabilities faster.
   Details: `[cyber]`
 ❯ 1. Switch to Opus 4.8
   2. Edit prompt and retry
";
    const CLAUDE_PRINTED_DECLINE_SCREEN: &str = "\
⎿  API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup).
   Double press esc to edit your last message, or try a different model with /model.
❯ ";

    /// The decline its CLI stopped at is the conversation's last record,
    /// keyed by that record, with the category off the record beside it;
    /// its screen shows the sentence, and a dialog shows its choices.
    #[test]
    fn a_decline_the_cli_stopped_at_is_read_with_its_category_and_its_screen() {
        let transcript = lines(&[
            CLAUDE_PLAIN_RECORD,
            CLAUDE_DECLINED_PARTIAL,
            CLAUDE_NO_FALLBACK,
            CLAUDE_DECLINE_ERROR,
            CLAUDE_TURN_END,
        ]);
        let reading = decline_reading_in(
            "claude",
            Some(CLAUDE_PRINTED_DECLINE_SCREEN),
            Some(&transcript),
        );
        let record = reading.record.expect("the decline record");
        assert_eq!(record.source, TRANSCRIPT);
        assert_eq!(record.key, "declined");
        assert_eq!(record.category.as_deref(), Some("cyber"));
        assert!(
            record
                .line
                .as_str()
                .contains("safeguards flagged this message")
        );
        let screen = reading.screen.expect("the screen's sentence");
        assert!(!screen.dialog, "a printed error is no dialog");
        assert!(
            screen
                .line
                .as_str()
                .contains("safeguards flagged this message")
        );
        let dialog = decline_reading_in("claude", Some(CLAUDE_DIALOG_SCREEN), None)
            .screen
            .expect("the dialog's sentence");
        assert!(dialog.dialog);
        assert_eq!(dialog.category.as_deref(), Some("cyber"));
        // Nobody measured another agent's decline: it reads nothing.
        assert_eq!(
            decline_reading_in("codex", Some(CLAUDE_DIALOG_SCREEN), Some(&transcript)),
            DeclineReading::default()
        );
    }

    /// A decline somebody answered, one a tool result quotes, and one the
    /// CLI answered on its fallback are not a decline it stopped at.
    #[test]
    fn a_decline_answered_quoted_or_fallen_back_from_is_no_record() {
        let answered = lines(&[
            CLAUDE_NO_FALLBACK,
            CLAUDE_DECLINE_ERROR,
            CLAUDE_TURN_END,
            CLAUDE_PERSON_TYPED,
        ]);
        assert_eq!(
            decline_reading_in("claude", None, Some(&answered)).record,
            None
        );
        let quoted = lines(&[CLAUDE_TOOL_QUOTING_THE_DECLINE, CLAUDE_PLAIN_RECORD]);
        assert_eq!(
            decline_reading_in("claude", None, Some(&quoted)).record,
            None
        );
        let fell_back = lines(&[CLAUDE_FALLBACK, CLAUDE_PLAIN_RECORD]);
        let reading = decline_reading_in("claude", None, Some(&fell_back));
        assert_eq!(reading.record, None);
        // …but its switch of model is read, for the binding it left.
        assert_eq!(
            reading.fallbacks,
            vec![ModelDeviation {
                worker: String::new(),
                dispatch: String::new(),
                source: String::new(),
                key: "switch-1".to_string(),
                from: "claude-fable-5-1".to_string(),
                to: "claude-opus-4-8".to_string(),
                category: Some("cyber".to_string()),
                scope: Some("session".to_string()),
                at_ms: zerocode_core::civil::epoch_ms_of_iso("2026-09-21T14:06:16.566Z"),
            }]
        );
    }

    /// What 2.1.281 writes after a turn's last record (a print-mode run of
    /// the hermetic `--settings` probe, t-6747, ids scrubbed): bookkeeping,
    /// not conversation.
    const CLAUDE_BOOKKEEPING: [&str; 3] = [
        r#"{"type":"last-prompt","lastPrompt":"probe","leafUuid":"declined","sessionId":"s"}"#,
        r#"{"type":"atis-latch","atis":"","sessionId":"s"}"#,
        r#"{"type":"cost-state","sessionId":"s","totalCostUSD":0.00015,"totalAPIDuration":13,"modelUsage":{"claude-fable-5-1":{"inputTokens":10,"outputTokens":1}}}"#,
    ];

    /// The bookkeeping the CLI writes after a turn is no input: the decline
    /// before it is still the conversation's last word.
    #[test]
    fn a_decline_is_read_past_the_bookkeeping_its_cli_writes_after_it() {
        let mut held = vec![CLAUDE_NO_FALLBACK, CLAUDE_DECLINE_ERROR];
        held.extend(CLAUDE_BOOKKEEPING);
        let record = decline_reading_in("claude", None, Some(&lines(&held)))
            .record
            .expect("the decline is still the last word");
        assert_eq!(record.key, "declined");
        assert_eq!(record.category.as_deref(), Some("cyber"));
    }

    /// The switch scan reads on from the cursor it is given and moves
    /// nothing itself (t-7153, P2): whole lines only, an unchanged file
    /// nothing, a rewritten file from its start — and the same cursor read
    /// twice answers the same switches twice, which is what lets a row the
    /// ledger refused be read again.
    #[test]
    fn the_switch_scan_reads_on_from_the_cursor_it_is_given_and_moves_nothing() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("a dir");
        let path = dir.path().join("session.jsonl");
        let mut file = std::fs::File::create(&path).expect("a transcript");
        writeln!(file, "{CLAUDE_PLAIN_RECORD}").expect("write");
        writeln!(file, "{CLAUDE_FALLBACK}").expect("write");
        let first = scan_fallbacks(&path, &ScanCursor::default());
        assert_eq!(first.switches.len(), 1);
        assert_eq!(first.switches[0].key, "switch-1");
        assert_eq!(
            first.next.offset,
            std::fs::metadata(&path).expect("size").len(),
            "the cursor stands after the last whole line"
        );
        assert!(first.next.file.is_some(), "the cursor knows its file");
        assert_eq!(
            scan_fallbacks(&path, &ScanCursor::default()),
            first,
            "the same cursor reads the same"
        );
        let again = scan_fallbacks(&path, &first.next);
        assert!(again.switches.is_empty(), "read past the cursor twice");
        assert_eq!(placed(&again.next), placed(&first.next));
        // A line still being written waits for its newline, cursor unmoved.
        let second = CLAUDE_FALLBACK.replace("switch-1", "switch-2");
        let (head, rest) = second.split_at(40);
        write!(file, "{head}").expect("write");
        file.flush().expect("flush");
        let waiting = scan_fallbacks(&path, &first.next);
        assert!(waiting.switches.is_empty());
        assert_eq!(placed(&waiting.next), placed(&first.next));
        writeln!(file, "{rest}").expect("write");
        file.flush().expect("flush");
        let later = scan_fallbacks(&path, &first.next);
        assert_eq!(later.switches.len(), 1);
        assert_eq!(later.switches[0].key, "switch-2");
        // Rewritten shorter: read again from the start.
        std::fs::write(&path, format!("{CLAUDE_FALLBACK}\n")).expect("rewrite");
        let rewritten = scan_fallbacks(&path, &later.next);
        assert_eq!(rewritten.switches.len(), 1);
        assert_eq!(
            rewritten.next.offset,
            u64::try_from(CLAUDE_FALLBACK.len() + 1).expect("small")
        );
    }

    /// A record exactly `len` bytes long that is no switch — padding for a
    /// fixture that must keep every byte after it where it was (t-7153, R3).
    pub(crate) fn a_record_as_long_as(len: usize) -> String {
        let pad = |body_len: usize| {
            format!(
                r#"{{"type":"user","uuid":"pad","message":{{"role":"user","content":"{}"}}}}"#,
                "x".repeat(body_len)
            )
        };
        let record = pad(len - pad(0).len());
        assert_eq!(record.len(), len, "the record's length");
        record
    }

    /// A transcript exactly `len` bytes long that opens on `first` — the
    /// rest one padding record — for a replacement as long as the file it
    /// replaces (t-7153, R3).
    pub(crate) fn a_transcript_as_long_as(first: &str, len: u64) -> String {
        let content = format!(
            "{first}\n{}\n",
            a_record_as_long_as(usize::try_from(len).expect("small") - first.len() - 2)
        );
        assert_eq!(content.len() as u64, len, "the fixture's length");
        content
    }

    /// A cursor counts into the FILE it read, not the path (t-7153, R3): a
    /// transcript replaced under its path by another — as long as the old
    /// one, or longer, with a switch in its first bytes — is read from its
    /// start, where before the old offset stood and the switch was never
    /// read; a partial first line names no file yet, and the whole line
    /// then does. The file appended to keeps its identity.
    #[test]
    fn the_switch_scan_starts_over_when_the_file_at_its_path_is_another() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("a dir");
        let path = dir.path().join("session.jsonl");
        let replace = |content: &str| {
            let fresh = dir.path().join("session.jsonl.next");
            std::fs::write(&fresh, content).expect("the replacement");
            std::fs::rename(&fresh, &path).expect("atomic replace");
        };
        replace(&format!("{CLAUDE_PLAIN_RECORD}\n{CLAUDE_FALLBACK}\n"));
        let first = scan_fallbacks(&path, &ScanCursor::default());
        assert_eq!(first.switches[0].key, "switch-1");
        let old_len = std::fs::metadata(&path).expect("size").len();
        assert_eq!(first.next.offset, old_len);

        // As long as the old file, a new switch first: read from the start.
        let second = CLAUDE_FALLBACK.replace("switch-1", "switch-2");
        replace(&a_transcript_as_long_as(&second, old_len));
        let rotated = scan_fallbacks(&path, &first.next);
        assert_eq!(
            rotated
                .switches
                .iter()
                .map(|one| one.key.as_str())
                .collect::<Vec<_>>(),
            ["switch-2"],
            "a replacement as long as the old file was read as unchanged"
        );
        assert_eq!(rotated.next.offset, old_len);
        assert_ne!(rotated.next.file, first.next.file, "another file");

        // Longer than the old file, a new switch first: from the start too.
        let third = CLAUDE_FALLBACK.replace("switch-1", "switch-3");
        replace(&format!(
            "{third}\n{CLAUDE_PLAIN_RECORD}\n{CLAUDE_PLAIN_RECORD}\n"
        ));
        let longer = scan_fallbacks(&path, &rotated.next);
        assert_eq!(
            longer
                .switches
                .iter()
                .map(|one| one.key.as_str())
                .collect::<Vec<_>>(),
            ["switch-3"],
            "a longer replacement was read as appended to"
        );
        // Appended to, the file is the same file: the cursor stands.
        let fourth = CLAUDE_FALLBACK.replace("switch-1", "switch-4");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append");
        writeln!(file, "{fourth}").expect("write");
        let appended = scan_fallbacks(&path, &longer.next);
        assert_eq!(appended.next.file, longer.next.file, "the same file");
        assert_eq!(
            appended
                .switches
                .iter()
                .map(|one| one.key.as_str())
                .collect::<Vec<_>>(),
            ["switch-4"],
            "an appended file was read from its start again"
        );

        // A first line still being written names no whole file yet; once
        // whole, the file is read from its start.
        let fifth = CLAUDE_FALLBACK.replace("switch-1", "switch-5");
        let (head, rest) = fifth.split_at(60);
        replace(head);
        let partial = scan_fallbacks(&path, &appended.next);
        assert!(partial.switches.is_empty());
        assert_eq!(partial.next.offset, 0);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append");
        writeln!(file, "{rest}").expect("write");
        let whole = scan_fallbacks(&path, &partial.next);
        assert_eq!(whole.switches.len(), 1);
        assert_eq!(whole.switches[0].key, "switch-5");
    }

    /// A file truncated and rewritten under its own name — the same inode,
    /// the same birth, its first line kept — no longer holds what the cursor
    /// counted, and is read again from its start (t-7153, R3): longer than
    /// before, or exactly as long; before this the identity was the first
    /// line and the birth, both kept, and the rewrite read as unchanged or
    /// as appended to. Appended to after, it is the same file and the
    /// cursor stands.
    #[test]
    fn the_switch_scan_starts_over_when_the_file_is_rewritten_under_its_own_name() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("a dir");
        let path = dir.path().join("session.jsonl");
        let switch = |key: &str| CLAUDE_FALLBACK.replace("switch-1", key);
        let rewrite = |content: &str| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .create(true)
                .open(&path)
                .expect("the file, truncated");
            file.write_all(content.as_bytes()).expect("rewrite");
        };
        let keys = |scan: &DeviationScan| -> Vec<String> {
            scan.switches.iter().map(|one| one.key.clone()).collect()
        };
        rewrite(&format!(
            "{CLAUDE_PLAIN_RECORD}\n{}\n{CLAUDE_PLAIN_RECORD}\n{CLAUDE_PLAIN_RECORD}\n",
            switch("switch-1")
        ));
        let first = scan_fallbacks(&path, &ScanCursor::default());
        assert_eq!(keys(&first), ["switch-1"]);
        let old_len = std::fs::metadata(&path).expect("size").len();
        assert_eq!(first.next.offset, old_len);

        // Rewritten longer, the first line kept, a new switch second.
        rewrite(&format!(
            "{CLAUDE_PLAIN_RECORD}\n{}\n{CLAUDE_PLAIN_RECORD}\n{CLAUDE_PLAIN_RECORD}\n{CLAUDE_PLAIN_RECORD}\n",
            switch("switch-2")
        ));
        let longer = scan_fallbacks(&path, &first.next);
        assert_eq!(
            keys(&longer),
            ["switch-2"],
            "a file rewritten under its own name was read as appended to"
        );
        assert_eq!(
            longer.next.file, first.next.file,
            "the same file by every name the platform gives"
        );

        // Rewritten exactly as long, the first line kept, a new switch second.
        rewrite(&a_transcript_as_long_as(
            &format!("{CLAUDE_PLAIN_RECORD}\n{}", switch("switch-3")),
            longer.next.offset,
        ));
        let as_long = scan_fallbacks(&path, &longer.next);
        assert_eq!(
            keys(&as_long),
            ["switch-3"],
            "a file rewritten as long as before under its own name was read as unchanged"
        );
        assert_eq!(as_long.next.offset, longer.next.offset);

        // Appended to: the same file, and the cursor stands.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append");
        writeln!(file, "{}", switch("switch-4")).expect("write");
        let appended = scan_fallbacks(&path, &as_long.next);
        assert_eq!(keys(&appended), ["switch-4"]);
        let again = scan_fallbacks(&path, &appended.next);
        assert_eq!(keys(&again), Vec::<String>::new());
        assert_eq!(
            placed(&again.next),
            placed(&appended.next),
            "an unchanged file moved the cursor"
        );
    }

    /// A transcript three of the scan's windows long whose second record is
    /// `second` — a switch, or a record exactly as long as one
    /// ([`a_record_as_long_as`]) — and every other byte of which is the
    /// same whatever `second` is (t-7153, R3): a rewrite that puts a switch
    /// there keeps the first line, the length, the bytes before any cursor
    /// at its end, and every window but the first.
    fn three_windows_with(second: &str) -> String {
        let window = usize::try_from(zerocode_core::transcript::MAX_TAIL_BYTES).expect("small");
        let plain = format!("{CLAUDE_PLAIN_RECORD}\n");
        let head = format!("{CLAUDE_PLAIN_RECORD}\n{second}\n");
        let fill = plain.repeat((2 * window + window / 2 - head.len()) / plain.len() + 1);
        let content = format!("{head}{fill}");
        assert_eq!(content.len().div_ceil(window), 3, "three windows");
        content
    }

    /// The same transcript before and after its second record became a
    /// switch (t-7153, R3): as long, its first line, its tail and its last
    /// two windows kept.
    fn a_middle_record_becomes_a_switch() -> (String, String) {
        let switch = CLAUDE_FALLBACK.replace("switch-1", "switch-mid");
        let before = three_windows_with(&a_record_as_long_as(switch.len()));
        let after = three_windows_with(&switch);
        assert_eq!(before.len(), after.len());
        (before, after)
    }

    /// The cursor at the end of the file at `path`, read from `cursor` one
    /// reading at a time as the platform's `evidence` names the file, and
    /// the keys of the switches found on the way.
    fn read_to_the_end(
        path: &Path,
        mut cursor: ScanCursor,
        evidence: FileEvidence,
    ) -> (ScanCursor, Vec<String>) {
        let size = std::fs::metadata(path).expect("size").len();
        let mut keys = Vec::new();
        for _ in 0..16 {
            if cursor.offset == size {
                return (cursor, keys);
            }
            let scan = scan_fallbacks_as(path, &cursor, evidence);
            keys.extend(scan.switches.into_iter().map(|one| one.key));
            cursor = scan.next;
        }
        panic!(
            "sixteen readings did not reach the end of {}",
            path.display()
        );
    }

    /// The number of the scan's windows a cursor counted.
    fn windows_of(cursor: &ScanCursor) -> u64 {
        cursor
            .offset
            .div_ceil(zerocode_core::transcript::MAX_TAIL_BYTES)
    }

    /// The window of what `cursor` counted that its next reading checks,
    /// on a file its platform names (t-7153 r4).
    fn checks_next(cursor: &ScanCursor) -> Option<u64> {
        cursor.counted.round.step(windows_of(cursor)).0
    }

    /// Where a cursor stands — its file, its offset, what it counted —
    /// without the round its readings walk: a reading of an unchanged file
    /// walks the round on by a window and moves the cursor nowhere
    /// (t-7153 r4).
    pub(crate) fn placed(cursor: &ScanCursor) -> ScanCursor {
        ScanCursor {
            counted: Counted {
                round: Round::default(),
                ..cursor.counted.clone()
            },
            ..cursor.clone()
        }
    }

    /// A file atomically replaced under its path by one that keeps its first
    /// line, its length and every byte but one record's near its start — a
    /// conversation rewritten, one record in its middle changed — is another
    /// file by the name the platform gives it, and is read from its start at
    /// once (t-7153, R3): on the very next reading, whose round checks a
    /// window the replacement did not change. Before this the identity was
    /// the first line and a birth time, and where the file system gives no
    /// birth the replacement read as unchanged.
    #[test]
    fn the_switch_scan_starts_over_when_a_replacement_keeps_the_first_line_and_the_tail() {
        let dir = tempfile::tempdir().expect("a dir");
        let path = dir.path().join("session.jsonl");
        let replace = |content: &str| {
            let fresh = dir.path().join("session.jsonl.next");
            std::fs::write(&fresh, content).expect("the replacement");
            std::fs::rename(&fresh, &path).expect("atomic replace");
        };
        let (before, after) = a_middle_record_becomes_a_switch();
        replace(&before);
        let (cursor, keys) = read_to_the_end(&path, ScanCursor::default(), platform_evidence);
        assert_eq!(keys, Vec::<String>::new());
        assert_eq!(windows_of(&cursor), 3);
        assert_ne!(checks_next(&cursor), Some(0), "the window replaced");
        replace(&after);
        let rotated = scan_fallbacks(&path, &cursor);
        assert_eq!(
            rotated
                .switches
                .iter()
                .map(|one| one.key.as_str())
                .collect::<Vec<_>>(),
            ["switch-mid"],
            "a replacement that kept the first line and the tail was read as unchanged"
        );
        assert_ne!(rotated.next.file, cursor.file, "another file");

        // Longer than before, the first line and every byte before the
        // cursor kept: another file too, read from its start at once.
        let (cursor, _) = read_to_the_end(&path, rotated.next, platform_evidence);
        replace(&format!(
            "{}{CLAUDE_PLAIN_RECORD}\n",
            three_windows_with(&CLAUDE_FALLBACK.replace("switch-1", "switch-7"))
        ));
        let longer = scan_fallbacks(&path, &cursor);
        assert_eq!(
            longer
                .switches
                .iter()
                .map(|one| one.key.as_str())
                .collect::<Vec<_>>(),
            ["switch-7"],
            "a longer replacement that kept the first line and the tail was read as appended to"
        );
    }

    /// A file rewritten IN PLACE under its own name — the same inode, the
    /// same birth — that keeps its first line, its length and every byte
    /// but one record's near its start is the same file by every name the
    /// platform gives, and no longer holds what the cursor counted
    /// (t-7153, R3): each reading checks one more window of it, in rounds
    /// ([`Round`]), so within `2N − 1` readings for the `N` windows the
    /// cursor counted the scan starts over and reads the switch the rewrite
    /// put there, once. Before this only the bytes just before the cursor
    /// were checked, and such a rewrite stood as the file it replaced for
    /// good. A file only appended to holds every window of a round, and its
    /// cursor stands and moves on.
    #[test]
    fn the_switch_scan_finds_a_rewrite_in_place_that_kept_the_first_line_and_the_tail() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("a dir");
        let path = dir.path().join("session.jsonl");
        let rewrite = |content: &str| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .create(true)
                .open(&path)
                .expect("the file, truncated");
            file.write_all(content.as_bytes()).expect("rewrite");
        };
        let (before, after) = a_middle_record_becomes_a_switch();
        rewrite(&before);
        let (cursor, keys) = read_to_the_end(&path, ScanCursor::default(), platform_evidence);
        assert_eq!(keys, Vec::<String>::new());
        let windows = windows_of(&cursor);
        assert_eq!(windows, 3);
        rewrite(&after);

        // From a window the rewrite did not change, within the bound.
        assert_ne!(checks_next(&cursor), Some(0), "the window rewritten");
        let bound = 2 * windows - 1;
        let mut at = cursor.clone();
        let mut found = Vec::new();
        let mut readings = 0;
        for _ in 0..bound {
            let scan = scan_fallbacks(&path, &at);
            readings += 1;
            assert_eq!(
                scan.next.file, cursor.file,
                "the same file by every name the platform gives"
            );
            found.extend(scan.switches.into_iter().map(|one| one.key));
            at = scan.next;
            if !found.is_empty() {
                break;
            }
        }
        assert_eq!(
            found,
            ["switch-mid"],
            "a rewrite in place that kept the first line and the tail stood past its bound"
        );
        assert!(readings <= bound, "{readings} readings");

        // Caught up again, then appended to: every window of two rounds
        // holds, and the appended switch is read once.
        let (caught_up, again) = read_to_the_end(&path, at, platform_evidence);
        assert_eq!(again, Vec::<String>::new(), "read twice");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append");
        writeln!(file, "{}", CLAUDE_FALLBACK.replace("switch-1", "switch-4")).expect("write");
        drop(file);
        let mut at = caught_up;
        let mut appended = Vec::new();
        for _ in 0..2 * windows {
            let scan = scan_fallbacks(&path, &at);
            assert!(
                scan.next.offset >= at.offset,
                "a file only appended to was read again from its start"
            );
            appended.extend(scan.switches.into_iter().map(|one| one.key));
            at = scan.next;
        }
        assert_eq!(appended, ["switch-4"]);

        // Rewritten in place once more, only its last record changed — the
        // bytes just before the cursor: the next reading starts over, when
        // its round names the first window, which the rewrite did not
        // change.
        let tail_changed = format!(
            "{after}{}\n",
            CLAUDE_FALLBACK.replace("switch-1", "switch-6")
        );
        assert_eq!(tail_changed.len() as u64, at.offset, "as long as before");
        for _ in 0..windows {
            if checks_next(&at) == Some(0) {
                break;
            }
            at = scan_fallbacks(&path, &at).next;
        }
        assert_eq!(checks_next(&at), Some(0));
        rewrite(&tail_changed);
        let next = scan_fallbacks(&path, &at);
        assert!(
            next.next.offset < at.offset,
            "a rewrite of the bytes just before the cursor was not found at once"
        );
        let (_, found) = read_to_the_end(&path, next.next.clone(), platform_evidence);
        let found: Vec<String> = next
            .switches
            .into_iter()
            .map(|one| one.key)
            .chain(found)
            .collect();
        assert_eq!(found, ["switch-mid", "switch-6"]);
    }

    /// `len` bytes of whole records that open on a plain record and then a
    /// SLOT — a record `slot` bytes long that is no switch, [`SLOT_AT`]
    /// bytes in — so a rewrite in place can put a switch as long there and
    /// move no byte after it (t-7153 r4).
    pub(crate) fn records_with_a_slot(len: usize, slot: usize) -> String {
        let plain = format!("{CLAUDE_PLAIN_RECORD}\n");
        let head = format!("{plain}{}\n", a_record_as_long_as(slot));
        let fill = plain.repeat((len - head.len()) / plain.len() - 1);
        let content = format!(
            "{head}{fill}{}\n",
            a_record_as_long_as(len - head.len() - fill.len() - 1)
        );
        assert_eq!(content.len(), len, "the run's length");
        content
    }

    /// How far into a run of [`records_with_a_slot`] its slot stands: past
    /// the plain record it opens on.
    pub(crate) const SLOT_AT: usize = CLAUDE_PLAIN_RECORD.len() + 1;

    /// `bytes` written over the file at `path` from `at` — in place: the
    /// same inode, the same birth, its length kept (t-7153 r4).
    pub(crate) fn overwrite_at(path: &Path, at: u64, bytes: &str) {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("the file");
        file.seek(SeekFrom::Start(at)).expect("seek");
        file.write_all(bytes.as_bytes()).expect("rewrite in place");
    }

    /// Every window a cursor counted is checked within a bound however the
    /// file grows (t-7153 r4, R3a, after astra t-6963 on 2e4ea291). A round
    /// of checks is fixed as it begins — the windows the cursor had counted
    /// then — each reading checks the next of them, and a window counted
    /// since waits for the next round; so a rewrite in place of window `i`
    /// of the `N` a cursor counted when the file was rewritten — its first
    /// line, its length and the bytes before the cursor kept — is found
    /// within `2N − 1` readings however fast the file grows: what is left
    /// of the round under way (its end fixed at no more than `N`), then
    /// `i + 1` readings of the next. The first, the middle and the last of
    /// the windows, rewritten at every phase of the rounds — 0 to `2N`
    /// readings after the scan caught up with a three-window file — the
    /// file not growing, or growing half a window, a window or two windows
    /// a reading all the while: each found within the bound, read from its
    /// start, and its switch read once on the readings that follow; a file
    /// only appended to is never read again from its start. Before this
    /// the window a reading checked was the reading's count over the
    /// windows counted NOW, and a file growing a window a reading, both
    /// moving one a reading, named the same window on every reading for as
    /// long as it grew: the others were never checked again.
    #[test]
    fn every_window_a_cursor_counted_is_checked_within_its_bound_however_the_file_grows() {
        use zerocode_core::transcript::MAX_TAIL_BYTES;
        const WINDOWS: u64 = 3;
        let window = usize::try_from(MAX_TAIL_BYTES).expect("small");
        let switch = CLAUDE_FALLBACK.replace("switch-1", "switch-rewritten");
        let dir = tempfile::tempdir().expect("a dir");
        let (mut cases, mut misses) = (0, Vec::new());
        for growth in [&[][..], &[window / 2], &[window], &[window, window]] {
            for phase in 0..=2 * WINDOWS {
                for which in ["first", "middle", "last"] {
                    cases += 1;
                    let case = format!(
                        "{} bytes a reading, {phase} readings in, the {which}",
                        growth.iter().sum::<usize>()
                    );
                    let path = dir.path().join("session.jsonl");
                    let mut slots = Vec::new();
                    let append = |slots: &mut Vec<u64>, runs: &[usize]| {
                        use std::io::Write;
                        let mut file = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&path)
                            .expect("append");
                        for len in runs {
                            let at = file.metadata().expect("size").len();
                            slots.push(at + SLOT_AT as u64);
                            file.write_all(records_with_a_slot(*len, switch.len()).as_bytes())
                                .expect("write");
                        }
                    };
                    let grow = |slots: &mut Vec<u64>| append(slots, growth);
                    append(&mut slots, &[window; WINDOWS as usize]);
                    let read = |cursor: &ScanCursor| scan_fallbacks(&path, cursor);

                    // Caught up, a window a reading.
                    let size = std::fs::metadata(&path).expect("size").len();
                    let mut cursor = ScanCursor::default();
                    while cursor.offset < size {
                        let scan = read(&cursor);
                        assert!(scan.switches.is_empty() && scan.next.offset > cursor.offset);
                        cursor = scan.next;
                    }
                    // Grown, never rewritten: the cursor never starts over.
                    for _ in 0..phase {
                        grow(&mut slots);
                        let scan = read(&cursor);
                        assert!(
                            scan.next.offset >= cursor.offset,
                            "{case}: a file only appended to was read again from its start"
                        );
                        cursor = scan.next;
                    }

                    // Rewritten in place: a slot of the window named, clear
                    // of the bytes just before the cursor.
                    let counted = cursor.offset.div_ceil(MAX_TAIL_BYTES);
                    let target = match which {
                        "first" => 0,
                        "middle" => counted / 2,
                        _ => counted - 1,
                    };
                    let len = switch.len() as u64;
                    let slot = slots
                        .iter()
                        .copied()
                        .find(|at| {
                            at / MAX_TAIL_BYTES == target
                                && (at + len - 1) / MAX_TAIL_BYTES == target
                                && at + len <= cursor.offset - TRANSCRIPT_FINGERPRINT_BYTES as u64
                        })
                        .unwrap_or_else(|| panic!("{case}: no slot in window {target}"));
                    overwrite_at(&path, slot, &switch);
                    let bound = 2 * counted - 1;
                    let mut found = Vec::new();
                    let mut readings = 0;
                    let started_over = loop {
                        if readings == 2 * bound {
                            break None;
                        }
                        readings += 1;
                        grow(&mut slots);
                        let scan = read(&cursor);
                        let over = scan.next.offset < cursor.offset;
                        found.extend(scan.switches.into_iter().map(|one| one.key));
                        cursor = scan.next;
                        if over {
                            break Some(readings);
                        }
                    };
                    // Read from its start: window `target` on the reading
                    // `target` after the one that started over.
                    if started_over.is_some() {
                        for _ in 0..target {
                            grow(&mut slots);
                            let scan = read(&cursor);
                            found.extend(scan.switches.into_iter().map(|one| one.key));
                            cursor = scan.next;
                        }
                    }
                    match started_over {
                        Some(readings) if readings <= bound && found == ["switch-rewritten"] => {}
                        _ => misses.push(format!(
                            "{case} (window {target} of {counted}): started over after {started_over:?} readings, bound {bound}; found {found:?}"
                        )),
                    }
                    std::fs::remove_file(&path).expect("the case's file");
                }
            }
        }
        assert_eq!(cases, 4 * 7 * 3);
        assert!(
            misses.is_empty(),
            "{} of {cases} rewrites stood past their bound: {misses:#?}",
            misses.len()
        );
    }

    /// Where the platform names no file, nothing is taken on faith
    /// (t-7153, R3): every window the cursor counted is read again and
    /// compared before the cursor is trusted — a file replaced under its
    /// path by one exactly as long, its first line, its tail and all but
    /// its first window kept, whose identity without a platform name is the
    /// same, is read from its start on the next reading, whatever window
    /// its round names; a longer replacement too; a file appended to still
    /// stands, and its cursor still moves.
    #[test]
    fn the_switch_scan_takes_nothing_on_faith_where_the_platform_names_no_file() {
        use std::io::Write;
        let unnamed: FileEvidence = |_, _| (None, None);
        let dir = tempfile::tempdir().expect("a dir");
        let path = dir.path().join("session.jsonl");
        let replace = |content: &str| {
            let fresh = dir.path().join("session.jsonl.next");
            std::fs::write(&fresh, content).expect("the replacement");
            std::fs::rename(&fresh, &path).expect("atomic replace");
        };
        let keys = |scan: &DeviationScan| -> Vec<String> {
            scan.switches.iter().map(|one| one.key.clone()).collect()
        };
        let (before, after) = a_middle_record_becomes_a_switch();
        replace(&before);
        let (first, found) = read_to_the_end(&path, ScanCursor::default(), unnamed);
        assert_eq!(found, Vec::<String>::new());
        let old_len = std::fs::metadata(&path).expect("size").len();
        assert_eq!(first.offset, old_len);

        // As long as before, the first line, the tail and the last windows
        // kept: the same file by every name this platform gives, and read
        // from its start all the same, on a turn that names a window the
        // replacement did not change.
        replace(&after);
        assert_eq!(std::fs::metadata(&path).expect("size").len(), old_len);
        let as_long = scan_fallbacks_as(&path, &first, unnamed);
        assert_eq!(
            as_long.next.file, first.file,
            "the platform told them apart"
        );
        assert_eq!(
            keys(&as_long),
            ["switch-mid"],
            "a replacement as long as the old file, its first line kept, was taken on faith"
        );
        let (caught_up, found) = read_to_the_end(&path, as_long.next, unnamed);
        assert_eq!(found, Vec::<String>::new(), "read twice");

        // Appended to: what the cursor counted is still there, the cursor
        // stands and moves on.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append");
        writeln!(file, "{}", CLAUDE_FALLBACK.replace("switch-1", "switch-3")).expect("write");
        drop(file);
        let appended = scan_fallbacks_as(&path, &caught_up, unnamed);
        assert_eq!(keys(&appended), ["switch-3"]);
        assert_eq!(
            appended.next.offset,
            std::fs::metadata(&path).expect("size").len()
        );
        let again = scan_fallbacks_as(&path, &appended.next, unnamed);
        assert_eq!(keys(&again), Vec::<String>::new());
        assert_eq!(
            again.next, appended.next,
            "an unchanged file moved the cursor"
        );

        // Longer than before, the first line kept: from the start too.
        replace(&format!(
            "{}{CLAUDE_PLAIN_RECORD}\n",
            three_windows_with(&CLAUDE_FALLBACK.replace("switch-1", "switch-5"))
        ));
        let longer = scan_fallbacks_as(&path, &again.next, unnamed);
        assert_eq!(
            keys(&longer),
            ["switch-5"],
            "a longer replacement, its first line kept, was read as appended to"
        );
    }

    /// What a reading of an unchanged long transcript costs (t-7153, R3):
    /// a file as long as the longest a worker wrote here in fourteen days
    /// (11.8 MB), read to its end, then read again unchanged, each reading
    /// from where the last one left the cursor — the stat, the opening, the
    /// anchor and the one window the cursor's round names. Prints the
    /// median of the readings; run with `--ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn measure_a_reading_of_an_unchanged_long_transcript() {
        const LONGEST_BYTES: usize = 11_800_000;
        const READINGS: u64 = 2_000;
        let dir = tempfile::tempdir().expect("a dir");
        let path = dir.path().join("session.jsonl");
        let plain = format!("{CLAUDE_PLAIN_RECORD}\n");
        std::fs::write(&path, plain.repeat(LONGEST_BYTES / plain.len())).expect("a transcript");
        let size = std::fs::metadata(&path).expect("size").len();
        let mut cursor = ScanCursor::default();
        while cursor.offset < size {
            cursor = scan_fallbacks(&path, &cursor).next;
        }
        let mut took: Vec<std::time::Duration> = (0..READINGS)
            .map(|_| {
                let started = std::time::Instant::now();
                let scan = scan_fallbacks(&path, &cursor);
                let took = started.elapsed();
                assert_eq!(scan.next.offset, size, "an unchanged file moved");
                cursor = scan.next;
                took
            })
            .collect();
        took.sort();
        println!(
            "an unchanged {} byte transcript, {} windows: median {:?}, p90 {:?} a reading over {READINGS}",
            cursor.offset,
            cursor
                .offset
                .div_ceil(zerocode_core::transcript::MAX_TAIL_BYTES),
            took[took.len() / 2],
            took[took.len() * 9 / 10],
        );
    }

    /// A reading is bounded (t-7153, P2): at most the tail window's bytes a
    /// beat, so a long transcript is read in beats; a line longer than the
    /// window — a tool result, never a switch record — is stepped over, and
    /// the record after it is read on the next beat.
    #[test]
    fn the_switch_scan_reads_at_most_one_window_a_beat_and_steps_over_a_longer_line() {
        use std::io::Write;
        use zerocode_core::transcript::MAX_TAIL_BYTES;
        let dir = tempfile::tempdir().expect("a dir");
        let path = dir.path().join("session.jsonl");
        let mut file = std::fs::File::create(&path).expect("a transcript");
        let long = usize::try_from(MAX_TAIL_BYTES).expect("small") + 4_096;
        writeln!(
            file,
            r#"{{"type":"user","uuid":"big","message":{{"role":"user","content":"{}"}}}}"#,
            "x".repeat(long)
        )
        .expect("write");
        writeln!(file, "{CLAUDE_FALLBACK}").expect("write");
        let size = std::fs::metadata(&path).expect("size").len();

        let first = scan_fallbacks(&path, &ScanCursor::default());
        assert!(first.switches.is_empty());
        assert_eq!(
            first.next.offset, MAX_TAIL_BYTES,
            "one window, stepped over"
        );
        let second = scan_fallbacks(&path, &first.next);
        assert_eq!(second.switches.len(), 1, "the record after the long line");
        assert_eq!(second.switches[0].key, "switch-1");
        assert_eq!(second.next.offset, size);

        // Many short lines: one window a beat, whole lines only, until caught up.
        let mut file = std::fs::File::create(&path).expect("a transcript");
        let rows = usize::try_from(MAX_TAIL_BYTES / 64).expect("small") + 8;
        for row in 0..rows {
            writeln!(
                file,
                "{}",
                CLAUDE_PLAIN_RECORD.replace("\"uuid\":\"v\"", &format!("\"uuid\":\"v{row}\""))
            )
            .expect("write");
        }
        writeln!(file, "{CLAUDE_FALLBACK}").expect("write");
        let size = std::fs::metadata(&path).expect("size").len();
        let mut cursor = ScanCursor::default();
        let mut beats = 0;
        let mut switches = Vec::new();
        while cursor.offset < size {
            let scan = scan_fallbacks(&path, &cursor);
            assert!(
                scan.next.offset > cursor.offset,
                "a beat that read nothing whole"
            );
            assert!(
                scan.next.offset - cursor.offset <= MAX_TAIL_BYTES,
                "a beat read past its window"
            );
            switches.extend(scan.switches);
            cursor = scan.next;
            beats += 1;
        }
        assert!(
            beats > 1,
            "a transcript longer than the window was read in one beat"
        );
        assert_eq!(switches.len(), 1);
    }

    /* ---- t-7153: the decline's identity and its screen ----------------- */

    /// An earlier decline of the same conversation, answered by a person
    /// (ids scrubbed): its own category record, its own error, both on
    /// request `req_1`.
    const CLAUDE_OLD_NO_FALLBACK: &str = r#"{"parentUuid":"p0","isSidechain":false,"type":"system","subtype":"model_refusal_no_fallback","content":"","level":"warning","originalModel":"claude-fable-5-1","requestId":"req_1","apiRefusalCategory":"cyber","refusedUserMessageUuid":"q0","isMeta":false,"uuid":"sys-1","timestamp":"2026-09-24T04:00:00.000Z","version":"2.1.281"}"#;
    const CLAUDE_OLD_DECLINE_ERROR: &str = r#"{"parentUuid":"sys-1","isSidechain":false,"type":"assistant","uuid":"declined-1","timestamp":"2026-09-24T04:00:00.010Z","message":{"id":"e1","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","type":"message","content":[{"type":"text","text":"API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup)."}]},"requestId":"req_1","error":"invalid_request","isApiErrorMessage":true}"#;
    /// The newest decline, on request `req_2`: its own category record names
    /// NO category, and its error record names that record as its parent.
    const CLAUDE_NEW_NO_FALLBACK_UNNAMED: &str = r#"{"parentUuid":"part-2","isSidechain":false,"type":"system","subtype":"model_refusal_no_fallback","content":"","level":"warning","originalModel":"claude-fable-5-1","requestId":"req_2","apiRefusalCategory":null,"refusedUserMessageUuid":"q2","isMeta":false,"uuid":"sys-2","timestamp":"2026-09-24T04:29:48.712Z","version":"2.1.281"}"#;
    const CLAUDE_NEW_DECLINE_ERROR: &str = r#"{"parentUuid":"sys-2","isSidechain":false,"type":"assistant","uuid":"declined-2","timestamp":"2026-09-24T04:29:48.711Z","message":{"id":"e2","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","type":"message","content":[{"type":"text","text":"API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup)."}]},"requestId":"req_2","error":"invalid_request","isApiErrorMessage":true}"#;
    /// An error record whose request and parent match no category record in
    /// the tail (the CLI wrote the category record of another request).
    const CLAUDE_STRAY_DECLINE_ERROR: &str = r#"{"parentUuid":"elsewhere","isSidechain":false,"type":"assistant","uuid":"declined-3","timestamp":"2026-09-24T05:29:48.711Z","message":{"id":"e3","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","type":"message","content":[{"type":"text","text":"API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup)."}]},"requestId":"req_3","error":"invalid_request","isApiErrorMessage":true}"#;

    /// The category rides with its own request (t-7153, P1-4): a decline
    /// whose category record names none is a decline with NO category, and
    /// an earlier request's `cyber` is never borrowed for it; a record that
    /// matches neither the error's request nor its parent names nothing.
    #[test]
    fn a_declines_category_is_read_off_its_own_request_and_never_an_earlier_one() {
        let borrowed = lines(&[
            CLAUDE_OLD_NO_FALLBACK,
            CLAUDE_OLD_DECLINE_ERROR,
            CLAUDE_PERSON_TYPED,
            CLAUDE_NEW_NO_FALLBACK_UNNAMED,
            CLAUDE_NEW_DECLINE_ERROR,
            CLAUDE_TURN_END,
        ]);
        let record = decline_reading_in("claude", None, Some(&borrowed))
            .record
            .expect("the newest decline is the last word");
        assert_eq!(record.key, "declined-2");
        assert_eq!(
            record.category, None,
            "an earlier request's category was borrowed for the newest decline"
        );

        let stray = lines(&[
            CLAUDE_OLD_NO_FALLBACK,
            CLAUDE_STRAY_DECLINE_ERROR,
            CLAUDE_TURN_END,
        ]);
        let record = decline_reading_in("claude", None, Some(&stray))
            .record
            .expect("the stray decline is the last word");
        assert_eq!(record.key, "declined-3");
        assert_eq!(
            record.category, None,
            "a category record of another request was taken for this decline"
        );

        // The real order, both ids agreeing: the category is this decline's.
        let own = lines(&[
            CLAUDE_OLD_NO_FALLBACK,
            CLAUDE_OLD_DECLINE_ERROR,
            CLAUDE_PERSON_TYPED,
            CLAUDE_DECLINED_PARTIAL,
            CLAUDE_NO_FALLBACK,
            CLAUDE_DECLINE_ERROR,
            CLAUDE_TURN_END,
        ]);
        let record = decline_reading_in("claude", None, Some(&own))
            .record
            .expect("the decline");
        assert_eq!(record.key, "declined");
        assert_eq!(record.category.as_deref(), Some("cyber"));
    }

    /// The newest decline's own category record, named: `sys-2` answers
    /// `req_2` under `cyber`.
    const CLAUDE_NEW_NO_FALLBACK_NAMED: &str = r#"{"parentUuid":"part-2","isSidechain":false,"type":"system","subtype":"model_refusal_no_fallback","content":"","level":"warning","originalModel":"claude-fable-5-1","requestId":"req_2","apiRefusalCategory":"cyber","refusedUserMessageUuid":"q2","isMeta":false,"uuid":"sys-2","timestamp":"2026-09-24T04:29:48.712Z","version":"2.1.281"}"#;

    /// An error record on request `request` naming `parent` as its parent
    /// — the two ids a category record is joined on. An empty id is written
    /// as an empty string; `None` leaves the field out, as an older CLI
    /// would.
    fn decline_error(request: Option<&str>, parent: Option<&str>) -> String {
        let field = |name: &str, value: Option<&str>| {
            value.map_or(String::new(), |value| format!(r#""{name}":"{value}","#))
        };
        format!(
            r#"{{{}"isSidechain":false,"type":"assistant","uuid":"declined-x","timestamp":"2026-09-24T04:29:48.711Z","message":{{"id":"ex","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","type":"message","content":[{{"type":"text","text":"API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup)."}}]}},{}"error":"invalid_request","isApiErrorMessage":true}}"#,
            field("parentUuid", parent),
            field("requestId", request),
        )
    }

    /// The category is joined on BOTH of the error record's ids (t-7153,
    /// R1): a category record the request names but whose uuid is not the
    /// parent the error names, or the parent record answering another
    /// request, is a contradiction — no category, and no guess between
    /// the two — where before either id alone borrowed the category. An
    /// empty id is no key. An id the category record does not carry, as an
    /// older CLI writes it, disputes nothing, so the one it carries decides;
    /// and both agreeing is this decline's category.
    #[test]
    fn a_category_record_one_id_names_and_the_other_disputes_names_nothing() {
        let category_of = |held: &[String]| {
            let record = decline_reading_in("claude", None, Some(held))
                .record
                .expect("the decline is the last word");
            assert_eq!(record.key, "declined-x");
            record.category
        };
        let old_and_new = |error: String| {
            lines(&[
                CLAUDE_OLD_NO_FALLBACK,
                CLAUDE_OLD_DECLINE_ERROR,
                CLAUDE_PERSON_TYPED,
                CLAUDE_NEW_NO_FALLBACK_NAMED,
                &error,
            ])
        };
        // The request's record, whose uuid is not the parent named: `sys-2`
        // answers `req_2`, but the error names `sys-1` as its parent.
        assert_eq!(
            category_of(&old_and_new(decline_error(Some("req_2"), Some("sys-1")))),
            None,
            "the request's category was taken over a parent that disputes it"
        );
        // The parent record, answering another request: `sys-2` is the
        // parent named, but it answered `req_2` and the error is `req_3`'s.
        assert_eq!(
            category_of(&old_and_new(decline_error(Some("req_3"), Some("sys-2")))),
            None,
            "the parent's category was taken over a request that disputes it"
        );
        // Both agreeing: this decline's category.
        assert_eq!(
            category_of(&old_and_new(decline_error(Some("req_2"), Some("sys-2")))).as_deref(),
            Some("cyber")
        );
        // Empty ids are no keys, even against a record whose id is as empty.
        let blank_record =
            CLAUDE_NEW_NO_FALLBACK_NAMED.replace(r#""requestId":"req_2""#, r#""requestId":"""#);
        assert_eq!(
            category_of(&lines(&[&blank_record, &decline_error(Some(""), Some(""))])),
            None,
            "an empty id joined a category record"
        );
        assert_eq!(
            category_of(&lines(&[
                CLAUDE_NEW_NO_FALLBACK_NAMED,
                &decline_error(None, None)
            ])),
            None,
            "a record with no ids joined a category record"
        );
        // An older CLI's error carries one id: the one it carries decides.
        assert_eq!(
            category_of(&old_and_new(decline_error(Some("req_2"), None))).as_deref(),
            Some("cyber"),
            "a request id alone, agreeing, named nothing"
        );
        assert_eq!(
            category_of(&old_and_new(decline_error(None, Some("sys-2")))).as_deref(),
            Some("cyber"),
            "a parent id alone, agreeing, named nothing"
        );
        // A category record that carries no request id (older still) is
        // silent on the request, and the parent it carries decides.
        let unrequested = CLAUDE_NEW_NO_FALLBACK_NAMED.replace(r#""requestId":"req_2","#, "");
        assert_eq!(
            category_of(&lines(&[
                &unrequested,
                &decline_error(Some("req_2"), Some("sys-2"))
            ]))
            .as_deref(),
            Some("cyber"),
            "a record silent on the request was read as disputing it"
        );
    }

    /// A tool result that quotes the dialog's words, under a turn that is
    /// still working — the screen a worker reading about declines shows.
    const CLAUDE_QUOTED_DIALOG_SCREEN: &str = "\
⏺ Read(notes/declines.md)
  ⎿  Read 5 lines
     Session paused
     Fable 5.1's safeguards flagged this message. Our intentionally broad safeguards…
     Details: `[cyber]`
     1. Switch to Opus 4.8
     2. Edit prompt and retry

✻ Thinking… (esc to interrupt)
";
    /// The dialog as the hermetic probe's pty carried it on 2.1.281
    /// (t-6747 `d-dialog`, 2026-09-24): the header, the sentence wrapped over
    /// three lines, the detail line, the cursor on the first choice.
    const CLAUDE_PROBE_DIALOG_SCREEN: &str = "\
    Session paused
    Fable 5.1's safeguards flagged this message. Our intentionally broad safeguards allow us to deliver more
  capabilities faster, but can sometimes flag legitimate coding and cybersecurity tasks. You can learn more:
  https://support.claude.com/en/articles/15363606
  Details: `[cyber]`
   ❯  1.  Switch to Opus 4.8
   2.  Edit prompt and retry with Fable 5.1

  ✻ Waiting for API response · will retry in 2m 40s · check your network
";

    /// Quoted words are not a dialog (t-7153, P1-3): the pause dialog is the
    /// CLI's own layout — its header, then the sentence, then the cursor on
    /// a numbered choice beside the retry choice — and a tool result that
    /// quotes every one of its lines stands under the turn's own spinner,
    /// with no cursor on a choice. Both real dialogs read here are dialogs.
    #[test]
    fn a_screen_that_quotes_the_dialogs_words_is_no_dialog() {
        let quoted = decline_reading_in("claude", Some(CLAUDE_QUOTED_DIALOG_SCREEN), None)
            .screen
            .expect("the quoted sentence is still on the screen");
        assert!(!quoted.dialog, "a quoted dialog was read as the CLI's own");
        assert_eq!(
            quoted.category, None,
            "a quoted category was read as the dialog's"
        );

        for real in [CLAUDE_DIALOG_SCREEN, CLAUDE_PROBE_DIALOG_SCREEN] {
            let dialog = decline_reading_in("claude", Some(real), None)
                .screen
                .expect("the dialog's sentence");
            assert!(dialog.dialog, "a real dialog was not read as one:\n{real}");
            assert_eq!(dialog.category.as_deref(), Some("cyber"));
        }
        // The printed error is no dialog either, as before.
        let printed = decline_reading_in("claude", Some(CLAUDE_PRINTED_DECLINE_SCREEN), None)
            .screen
            .expect("the printed sentence");
        assert!(!printed.dialog);
    }
}
