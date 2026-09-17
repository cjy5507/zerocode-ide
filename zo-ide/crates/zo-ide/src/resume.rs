//! Session-list and continuation helpers for the plain `zo` front-end.
//!
//! The existing session registry already owns discovery, ordering, transcript
//! parsing, and the `latest` resolution rules. This module gives that internal
//! result a public, small API for the IDE picker and renders the no-argument
//! `--resume` view. A transcript does not carry its workspace path, so new
//! sessions write a tiny `.cwd` sidecar; old sessions remain listable with an
//! explicit "not recorded" location.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session_registry;

/// A session row suitable for a picker or a non-interactive resume listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeSession {
    pub id: String,
    pub path: PathBuf,
    pub modified_epoch_millis: u128,
    /// 트랜스크립트 머리글의 `created_at_ms` — `/resume` 의 `Sort: [Created]`
    /// 가 읽는 축이고 수정 시각과는 다르다(오래 산 세션은 둘이 몇 시간 벌어진다).
    pub created_epoch_millis: u128,
    pub cwd: Option<PathBuf>,
    pub first_prompt: Option<String>,
}

/// Alias that reads naturally at call sites that refer to the rows as a list.
pub type SessionSummary = ResumeSession;

/// Every limit of the resume list, in one table (t-2947).
pub mod limits {
    /// Rows the no-argument `--resume` view prints and the `/resume` picker
    /// lists — read from disk once, at open.
    pub const LIST_LIMIT: usize = 20;
    /// Characters of the first prompt a row carries. Read from the head of
    /// the transcript, never from further in.
    pub const FIRST_PROMPT_PREVIEW_CHARS: usize = 60;
    /// Characters of the one-line summary the `--resume` view prints per row.
    pub const SUMMARY_LINE_CHARS: usize = 80;
}

/// Maximum number of rows printed by the no-argument resume view.
pub const DEFAULT_LIST_LIMIT: usize = limits::LIST_LIMIT;

/// Return recent sessions for the current workspace, newest first.
pub fn list_recent_sessions() -> Result<Vec<ResumeSession>, Box<dyn std::error::Error>> {
    list_recent_sessions_limited(DEFAULT_LIST_LIMIT)
}

/// Return at most `limit` recent sessions for the current workspace, newest
/// first. The registry applies the limit before parsing transcript contents.
/// 최근 세션 목록 — `<id>.system-prompt.json` 사이드카는 세션이 아니므로 뺀다.
pub fn list_recent_sessions_limited(
    limit: usize,
) -> Result<Vec<ResumeSession>, Box<dyn std::error::Error>> {
    Ok(resumable(list_recent_sessions_limited_unfiltered(limit)?))
}

/// The sessions worth offering back: a transcript is a session, a sidecar is
/// not, and a session nobody ever spoke into has nothing to resume — every
/// `zo` that was opened and closed left one, and they crowded the picker
/// ("resume시 메시지 없는 내역은 안보여도 됨", 2026-09-02). The file stays
/// resumable by id; it is only not listed.
fn resumable(sessions: Vec<ResumeSession>) -> Vec<ResumeSession> {
    sessions
        .into_iter()
        .filter(|session| !session.id.ends_with(".system-prompt"))
        .filter(|session| {
            session
                .first_prompt
                .as_deref()
                .is_some_and(|prompt| !prompt.trim().is_empty())
        })
        .collect()
}

fn list_recent_sessions_limited_unfiltered(
    limit: usize,
) -> Result<Vec<ResumeSession>, Box<dyn std::error::Error>> {
    session_registry::list_managed_sessions_limited(Some(limit)).map(|sessions| {
        sessions
            .into_iter()
            .map(|session| ResumeSession {
                cwd: load_session_cwd(&session.path),
                id: session.id,
                path: session.path,
                modified_epoch_millis: session.modified_epoch_millis,
                created_epoch_millis: session.created_epoch_millis,
                first_prompt: session.first_user_text,
            })
            .collect()
    })
}

/// Return the most recent session in the current workspace, if one exists.
pub fn latest_session() -> Result<Option<ResumeSession>, Box<dyn std::error::Error>> {
    Ok(list_recent_sessions_limited(1)?.into_iter().next())
}

/// The id that `--continue` should use for the current workspace.
pub fn continue_session_id() -> Result<Option<String>, Box<dyn std::error::Error>> {
    Ok(latest_session()?.map(|session| session.id))
}

/// Render a deterministic, append-only recent-session list for `--resume`
/// without an id. It intentionally does not open a runtime or contact a
/// provider, so asking for the list is safe in a pipe and in an unauthenticated
/// environment.
pub fn render_recent_sessions(limit: usize) -> Result<String, Box<dyn std::error::Error>> {
    let sessions = list_recent_sessions_limited(limit)?;
    let mut lines = vec![
        "Recent sessions".to_string(),
        "  Use `zo --resume <id>` to restore one; `zo --continue` chooses the newest.".to_string(),
    ];
    if sessions.is_empty() {
        lines.push("  (none found in the current workspace)".to_string());
        return Ok(lines.join("\n"));
    }

    for session in sessions {
        let cwd = session
            .cwd
            .as_deref()
            .map_or_else(|| "(not recorded)".to_string(), |path| path.display().to_string());
        let prompt = session
            .first_prompt
            .as_deref()
            .map_or_else(|| "(no prompt)".to_string(), one_line_summary);
        lines.push(format!(
            "  {id:<24} {time}  cwd={cwd}  prompt={prompt}",
            id = session.id,
            time = format_timestamp(session.modified_epoch_millis),
        ));
    }
    Ok(lines.join("\n"))
}

/// Path of the workspace sidecar associated with a transcript.
///
/// The extension is deliberately neither `jsonl` nor `json`, because the
/// canonical registry treats those extensions as resumable transcript files.
#[must_use]
pub fn session_cwd_path(session_path: &Path) -> PathBuf {
    session_path.with_extension("cwd")
}

/// Record a session's workspace once, without overwriting an existing record.
/// This is crate-visible because `PlainSession` owns session creation; the
/// path helper remains public for picker integrations and tests.
pub(crate) fn write_session_cwd_if_missing(session_path: &Path, cwd: &Path) -> io::Result<()> {
    let path = session_cwd_path(session_path);
    if path.exists() {
        return Ok(());
    }
    let metadata = SessionCwd {
        cwd: cwd.to_string_lossy().into_owned(),
    };
    let payload = serde_json::to_vec(&metadata).map_err(io::Error::other)?;
    crate::write_atomic(&path, &payload)
}

pub(crate) fn load_session_cwd(session_path: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(session_cwd_path(session_path)).ok()?;
    let metadata = serde_json::from_str::<SessionCwd>(&raw).ok()?;
    (!metadata.cwd.trim().is_empty()).then(|| PathBuf::from(metadata.cwd))
}

fn one_line_summary(prompt: &str) -> String {
    const MAX_CHARS: usize = limits::SUMMARY_LINE_CHARS;

    let mut summary = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.chars().count() > MAX_CHARS {
        summary = summary.chars().take(MAX_CHARS - 1).collect();
        summary.push('…');
    }
    summary
}

fn format_timestamp(modified_epoch_millis: u128) -> String {
    let seconds = u64::try_from(modified_epoch_millis / 1_000).unwrap_or(u64::MAX);
    let seconds_today = seconds % 86_400;
    let date = core_types::date::utc_date_from_unix_secs(seconds);
    format!(
        "{date} {:02}:{:02}:{:02}Z",
        seconds_today / 3_600,
        (seconds_today % 3_600) / 60,
        seconds_today % 60,
    )
}

#[derive(Debug, Deserialize, Serialize)]
struct SessionCwd {
    cwd: String,
}

/// Path of the agent-registry locator sidecar beside a transcript
/// (`<session>.registry`): the `registries/<sid>.json` record the session's
/// helper store is described by, so `/resume` from another directory comes
/// back to the same root (t-2511). The extension is neither `jsonl` nor
/// `json` for the same reason as [`session_cwd_path`].
#[must_use]
pub fn session_registry_locator_path(session_path: &Path) -> PathBuf {
    session_path.with_extension("registry")
}

/// Record where the session's agent registry lives. Overwrites only when the
/// locator actually moved, so a resumed session does no write at all.
pub(crate) fn write_session_registry_locator(session_path: &Path, locator: &Path) -> io::Result<()> {
    if load_session_registry_locator(session_path).as_deref() == Some(locator) {
        return Ok(());
    }
    let metadata = SessionRegistryLocator {
        locator: locator.to_string_lossy().into_owned(),
    };
    let payload = serde_json::to_vec(&metadata).map_err(io::Error::other)?;
    crate::write_atomic(&session_registry_locator_path(session_path), &payload)
}

/// The locator a session remembered, if it was born with a registry.
pub(crate) fn load_session_registry_locator(session_path: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(session_registry_locator_path(session_path)).ok()?;
    let metadata = serde_json::from_str::<SessionRegistryLocator>(&raw).ok()?;
    (!metadata.locator.trim().is_empty()).then(|| PathBuf::from(metadata.locator))
}

#[derive(Debug, Deserialize, Serialize)]
struct SessionRegistryLocator {
    locator: String,
}

#[cfg(test)]
mod tests {
    use super::{format_timestamp, one_line_summary};

    #[test]
    fn timestamp_is_stable_and_utc() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(
            format_timestamp(1_762_118_400_000),
            "2025-11-02 21:20:00Z"
        );
    }

    #[test]
    fn prompt_summary_is_single_line_and_bounded() {
        assert_eq!(one_line_summary(" first\n second  third "), "first second third");
        let long = "a".repeat(100);
        let summary = one_line_summary(&long);
        assert_eq!(summary.chars().count(), 80);
        assert!(summary.ends_with('…'));
    }
}

