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

use zerocode_core::orchestration::{QuotaWallMarker, Text, TransientErrorMarker};

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
}

impl StallCause {
    /// Whether a prompt after the record takes the marker back.
    ///
    /// A continuation ACTS on the composer, so its witness has to be the last
    /// word of the conversation: a person's line, a mail pointer, a queued
    /// message after the error is a turn somebody already started. A wall is
    /// news, and a person typing into a wall does not lift it.
    const fn needs_the_last_word(self) -> bool {
        matches!(self, Self::TransientApiError)
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
];

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
        rule.screen
            .iter()
            .any(|group| group.iter().all(|phrase| line.contains(phrase)))
            .then(|| QuotaWallMarker {
                source: SCREEN.to_string(),
                line: clipped(line),
            })
    })
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

/// The WIP commit's message: who stopped, at which wall, and when it lifts.
pub(crate) fn wip_message(
    worker: &str,
    provider: &str,
    resets_at_ms: Option<i64>,
    local_offset_secs: i64,
) -> String {
    let resets = resets_at_ms.map_or_else(
        || "unknown".to_string(),
        |at| clock_hhmm(at, local_offset_secs),
    );
    format!("wip(handover): {worker} stopped at {provider} wall, resets {resets}")
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
        let message = wip_message("w-3", "codex", Some(1_788_478_149_000), 9 * 3600);
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
            wip_message("w-3", "codex", Some(1_788_478_149_000), 9 * 3600),
            "wip(handover): w-3 stopped at codex wall, resets 08:29"
        );
        assert_eq!(
            wip_message("w-3", "claude", None, 0),
            "wip(handover): w-3 stopped at claude wall, resets unknown"
        );
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
}
