//! What one helper's run cost — and the one place every surface spells it.
//!
//! A sub-agent's price is three numbers: how many tools it ran, how many
//! output tokens it spent, and how long it took. Claude Code writes them as
//! `12 tool uses · 45.3k tokens · 1m 20s` under a running task and again in
//! its `Done (…)` line. zo shows the same three on four surfaces — the status
//! line under `Working`, the live row inside the spawn cell, the finished
//! helper's card, and the notification the model reads — so the spelling lives
//! here rather than being re-invented at each of them.
//!
//! Nothing here reads a clock or a manifest: callers hand over what they
//! measured, and these functions only render it.

use std::time::Duration;

/// What separates two facts on one line, everywhere zo writes a status row.
pub const FACT_SEPARATOR: &str = " · ";

/// One finished-or-running helper's measured cost.
///
/// Every field is "what is known so far": a helper that has not called a tool
/// yet, a backend that reports no usage, and a completion built before any
/// clock started are all representable, and each missing piece simply drops
/// out of [`HelperRun::summary`] instead of being printed as a zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HelperRun {
    /// Tools the helper has started (the manifest's `toolCalls`).
    pub tool_calls: u64,
    /// Output tokens the helper reported spending across its turns.
    pub output_tokens: u64,
    /// Wall-clock time from spawn to the terminal state, when it was measured.
    pub elapsed: Option<Duration>,
}

impl HelperRun {
    /// A run nothing is known about — printing it would say only `()`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.tool_calls == 0 && self.output_tokens == 0 && self.elapsed.is_none()
    }

    /// `12 tool uses · 45.3k tokens · 1m 20s`, keeping only the known parts.
    ///
    /// `None` when nothing was measured, so a caller can leave its line exactly
    /// as it reads today instead of appending an empty parenthesis.
    #[must_use]
    pub fn summary(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts: Vec<String> = Vec::with_capacity(3);
        if self.tool_calls > 0 {
            parts.push(tool_uses(self.tool_calls));
        }
        if self.output_tokens > 0 {
            parts.push(format!("{} tokens", tokens_compact(self.output_tokens)));
        }
        if let Some(elapsed) = self.elapsed {
            parts.push(elapsed_compact(elapsed.as_secs()));
        }
        Some(parts.join(FACT_SEPARATOR))
    }
}

/// `1 tool use` / `12 tool uses` — Claude Code's count on a running task.
#[must_use]
pub fn tool_uses(count: u64) -> String {
    if count == 1 {
        "1 tool use".to_string()
    } else {
        format!("{count} tool uses")
    }
}

/// Compact a token count for human-facing output (`45.3k`, `1.23M`).
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn tokens_compact(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else if tokens.is_multiple_of(1_000_000) {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else {
        format!("{:.2}M", tokens as f64 / 1_000_000.0)
    }
}

/// Elapsed seconds in codex's `fmt_elapsed_compact` grammar (`30s`, `1m 20s`).
#[must_use]
pub fn elapsed_compact(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3600 {
        return format!("{}m {:02}s", seconds / 60, seconds % 60);
    }
    format!(
        "{}h {:02}m {:02}s",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{elapsed_compact, tokens_compact, tool_uses, HelperRun};

    #[test]
    fn the_count_agrees_with_itself_at_zero_one_and_many() {
        assert_eq!(tool_uses(0), "0 tool uses");
        assert_eq!(tool_uses(1), "1 tool use");
        assert_eq!(tool_uses(12), "12 tool uses");
    }

    #[test]
    fn a_full_run_reads_the_way_claude_code_writes_it() {
        let run = HelperRun {
            tool_calls: 12,
            output_tokens: 45_300,
            elapsed: Some(Duration::from_secs(80)),
        };
        assert_eq!(
            run.summary().as_deref(),
            Some("12 tool uses · 45.3k tokens · 1m 20s")
        );
    }

    /// A backend that reports no usage must not print `0 tokens` — the line
    /// would read as "this helper spent nothing", which is a claim we cannot
    /// make. The same holds for a helper that called no tool.
    #[test]
    fn unmeasured_parts_drop_out_instead_of_printing_zero() {
        let no_tokens = HelperRun {
            tool_calls: 3,
            output_tokens: 0,
            elapsed: Some(Duration::from_secs(5)),
        };
        assert_eq!(no_tokens.summary().as_deref(), Some("3 tool uses · 5s"));

        let no_clock = HelperRun {
            tool_calls: 1,
            output_tokens: 900,
            elapsed: None,
        };
        assert_eq!(no_clock.summary().as_deref(), Some("1 tool use · 900 tokens"));

        let tokens_only = HelperRun {
            tool_calls: 0,
            output_tokens: 1_200,
            elapsed: None,
        };
        assert_eq!(tokens_only.summary().as_deref(), Some("1.2k tokens"));
    }

    #[test]
    fn a_run_nothing_was_measured_for_earns_no_parenthesis() {
        assert!(HelperRun::default().is_empty());
        assert_eq!(HelperRun::default().summary(), None);
    }

    #[test]
    fn compact_spellings_match_the_surfaces_that_already_used_them() {
        assert_eq!(tokens_compact(999), "999");
        assert_eq!(tokens_compact(45_300), "45.3k");
        assert_eq!(tokens_compact(2_000_000), "2.0M");
        assert_eq!(tokens_compact(1_234_567), "1.23M");

        assert_eq!(elapsed_compact(0), "0s");
        assert_eq!(elapsed_compact(59), "59s");
        assert_eq!(elapsed_compact(80), "1m 20s");
        assert_eq!(elapsed_compact(3_725), "1h 02m 05s");
    }
}
