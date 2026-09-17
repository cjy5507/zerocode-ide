//! The prompt-cache break ledger.
//!
//! The api layer already judges every response's cache usage
//! (`PromptCacheRecord` → [`crate::conversation::PromptCacheEvent`]): an
//! `unexpected` drop of cache-read tokens names its reason and its size, and a
//! cold streak gets a one-line warning. Until now that judgement lived for one
//! turn — a `[cache]` warn row on screen — and was gone. So the one axis the
//! scoreboard loses hardest on (cache-hit failures, 3.69% of requests on
//! 2026-08-31 against Claude Code's 0.14%; a usage-only proxy read 2.12% on
//! 2026-09-10) had a number and no causes.
//!
//! Every unexpected event now lands here, one JSON line per event under the
//! project's state (`prompt-cache/breaks.jsonl`), with the turn's context: the
//! session, the model on the wire, the iteration, and how many messages the
//! request carried. That is what a "why did the prefix move" table is built
//! from. Same doctrine as the route-outcome store: serialize first, one
//! `write_all` per line (`O_APPEND` is atomic per write, not per record), and a
//! reader that drops a line it cannot parse rather than the whole file.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::conversation::PromptCacheEvent;

const BREAKS_DIR: &str = "prompt-cache";
const BREAKS_FILE: &str = "breaks.jsonl";

/// One unexpected prompt-cache break, with the turn context that lets it be
/// attributed later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCacheBreakRecord {
    /// Unix seconds when the break was observed.
    pub recorded_at: u64,
    pub session_id: String,
    /// The model the request went out on (the wire model), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The turn-loop iteration (1-based) whose response reported the drop.
    pub iteration: usize,
    /// How many transcript messages the request carried.
    pub request_messages: usize,
    /// The api layer's judgement of why the prefix moved.
    pub reason: String,
    pub previous_cache_read: u32,
    pub current_cache_read: u32,
    pub token_drop: u32,
    /// The cold-streak warning, when the event carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// The turn context a batch of events is stamped with.
#[derive(Debug, Clone)]
pub struct BreakContext<'a> {
    pub session_id: &'a str,
    pub model: Option<&'a str>,
    pub iteration: usize,
    pub request_messages: usize,
    pub recorded_at: u64,
}

/// The records a turn's events yield: only the unexpected ones and the cold
/// streaks — an expected cache growth is not a break.
#[must_use]
pub fn break_records(
    context: &BreakContext<'_>,
    events: &[PromptCacheEvent],
) -> Vec<PromptCacheBreakRecord> {
    events
        .iter()
        .filter(|event| event.unexpected || event.warning.is_some())
        .map(|event| PromptCacheBreakRecord {
            recorded_at: context.recorded_at,
            session_id: context.session_id.to_string(),
            model: context.model.map(str::to_string),
            iteration: context.iteration,
            request_messages: context.request_messages,
            reason: event.reason.clone(),
            previous_cache_read: event.previous_cache_read_input_tokens,
            current_cache_read: event.current_cache_read_input_tokens,
            token_drop: event.token_drop,
            warning: event.warning.clone(),
        })
        .collect()
}

/// Where a project's break ledger lives.
#[must_use]
pub fn prompt_cache_breaks_path(cwd: &Path) -> PathBuf {
    crate::zo_project_state_dir(cwd)
        .join(BREAKS_DIR)
        .join(BREAKS_FILE)
}

/// Append records to the project's ledger.
pub fn record_prompt_cache_breaks(
    cwd: &Path,
    records: &[PromptCacheBreakRecord],
) -> io::Result<()> {
    record_prompt_cache_breaks_at_path(&prompt_cache_breaks_path(cwd), records)
}

/// Append records at an explicit path — one serialized line, one write, each.
pub fn record_prompt_cache_breaks_at_path(
    path: &Path,
    records: &[PromptCacheBreakRecord],
) -> io::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    for record in records {
        let mut line = serde_json::to_string(record).map_err(io::Error::other)?;
        line.push('\n');
        file.write_all(line.as_bytes())?;
    }
    Ok(())
}

/// Read a project's ledger, dropping any line that does not parse.
pub fn read_prompt_cache_breaks(cwd: &Path) -> io::Result<Vec<PromptCacheBreakRecord>> {
    read_prompt_cache_breaks_at_path(&prompt_cache_breaks_path(cwd))
}

/// Read a ledger at an explicit path; a missing file is an empty ledger.
pub fn read_prompt_cache_breaks_at_path(path: &Path) -> io::Result<Vec<PromptCacheBreakRecord>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(unexpected: bool, reason: &str, drop: u32, warning: Option<&str>) -> PromptCacheEvent {
        PromptCacheEvent {
            unexpected,
            reason: reason.to_string(),
            previous_cache_read_input_tokens: 40_000,
            current_cache_read_input_tokens: 40_000 - drop,
            token_drop: drop,
            warning: warning.map(str::to_string),
        }
    }

    #[test]
    fn only_unexpected_drops_and_cold_streaks_become_records() {
        let context = BreakContext {
            session_id: "session-1",
            model: Some("claude-opus-5"),
            iteration: 7,
            request_messages: 241,
            recorded_at: 1_789_000_000,
        };
        let records = break_records(
            &context,
            &[
                event(false, "expected growth", 0, None),
                event(true, "history diverges at message #12/241", 18_000, None),
                event(false, "cold streak", 0, Some("prompt cache degraded: 3 consecutive requests re-billed ~180k tokens")),
            ],
        );
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].reason, "history diverges at message #12/241");
        assert_eq!(records[0].token_drop, 18_000);
        assert_eq!(records[0].model.as_deref(), Some("claude-opus-5"));
        assert_eq!((records[0].iteration, records[0].request_messages), (7, 241));
        assert!(records[1].warning.as_deref().unwrap().starts_with("prompt cache degraded"));
    }

    #[test]
    fn the_ledger_round_trips_and_a_torn_line_costs_only_itself() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("state").join("prompt-cache").join("breaks.jsonl");
        let context = BreakContext {
            session_id: "session-2",
            model: None,
            iteration: 1,
            request_messages: 3,
            recorded_at: 1,
        };
        let records = break_records(&context, &[event(true, "a", 5, None), event(true, "b", 6, None)]);
        record_prompt_cache_breaks_at_path(&path, &records).expect("append");
        // Nothing to write is not an error and creates nothing.
        record_prompt_cache_breaks_at_path(&root.path().join("never").join("x.jsonl"), &[]).expect("noop");
        assert!(!root.path().join("never").exists());
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(b"{not json\n"))
            .expect("torn line");
        let back = read_prompt_cache_breaks_at_path(&path).expect("read");
        assert_eq!(back, records);
        assert_eq!(
            read_prompt_cache_breaks_at_path(&root.path().join("missing.jsonl")).expect("missing"),
            Vec::new()
        );
    }
}
