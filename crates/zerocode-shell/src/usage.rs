//! Plan usage, read the way Orca reads it: by asking the CLI itself.
//!
//! There is no API for "how much of my Claude plan is left" — the one place
//! the number exists is the `/usage` panel the CLI draws. Orca therefore
//! starts a hidden terminal, types `/usage` at it, waits for the panel and
//! parses the text (`fetchViaPty`, out/main/index.js:199740-199950). Every
//! constant below is that function's, measured:
//!
//!   - 2s after spawn, `/usage⏎`; a nudge ⏎ every 800ms while waiting
//!   - trust prompts answered `y⏎` so a fresh machine still scans
//!   - stop when a section heading appears, settle 2s (8s when the CLI shows
//!     its 2.1 usage tabs, which render slowly), give up at 25s
//!   - percentages read as `N% used|consumed` or inverted from
//!     `N% left|remaining|available`; windows are 300 and 10080 minutes
//!   - a scan is never repeated within 300s unless somebody asks by hand;
//!     the ambient poll is 900s and a snapshot is stale at 1800s
//!
//! One deliberate improvement over the measurement: Orca strips control
//! sequences from the RAW stream, so a TUI that repaints leaves it several
//! interleaved frames to scan. This window already owns a terminal grid that
//! resolves those repaints into a screen — the parser here reads the screen
//! the person would see, not the bytes that drew it.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const STARTUP_DELAY: Duration = Duration::from_secs(2);
pub const NUDGE_EVERY: Duration = Duration::from_millis(800);
pub const SCAN_TIMEOUT: Duration = Duration::from_secs(25);
pub const SETTLE_AFTER_STOP: Duration = Duration::from_secs(2);
pub const SETTLE_AFTER_TABS: Duration = Duration::from_secs(8);
/// The pump cadence of the hidden terminal — ours, not Orca's: node-pty
/// pushes, our reader is drained.
pub const SCAN_PUMP: Duration = Duration::from_millis(50);
pub const MIN_REFETCH: Duration = Duration::from_secs(300);
/// How often the window asks unprompted, and when a snapshot stops being
/// worth showing without a warning triangle. The window's own timers carry
/// these numbers (`USAGE_AMBIENT_MS`/`USAGE_STALE_MS` in shell.js); the
/// constants exist for the gate that keeps the two sides agreeing. The
/// ambient cadence also ships: it is the floor an INACTIVE Claude account's
/// own read keeps (`refresh_inactive_claude_accounts`, t-7538) — the same
/// beat the bar asks the selected account on, and not a second number.
pub const AMBIENT_POLL_MINUTES: u32 = 15;
#[cfg(test)]
pub const STALE_AFTER_MINUTES: u32 = 30;

pub const SESSION_WINDOW_MINUTES: u32 = 300;
pub const WEEKLY_WINDOW_MINUTES: u32 = 10080;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub used_percent: u8,
    pub window_minutes: u32,
    /// Epoch milliseconds, when the reset words could be turned into a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// The reset words as printed, kept even when parsed — the fallback face
    /// and the re-check evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_description: Option<String>,
}

/// One earned Codex reset credit, as the backend lists it.
///
/// A credit is a thing a person can SPEND to clear a throttled window early,
/// so the list is not decoration: `status` says whether this one is still
/// available, and `expires_at` says how long that stays true
/// (`codex-fetcher.ts:285-289`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetCredit {
    /// Lowercased, because the backend has shipped both cases and a match on
    /// `"available"` must not depend on which (`normalizeCreditStatus`,
    /// `codex-fetcher.ts:228-230`).
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<i64>,
}

/// What Codex says the account may still reset.
///
/// `available_count` is the number the window shows; `next_expires_at` is the
/// soonest one of them stops counting, which is the difference between "you
/// have three" and "you have three, one until Friday".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetCredits {
    pub available_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_earned_count: Option<u32>,
    /// Soonest expiry among the AVAILABLE credits only — an expired credit's
    /// stamp is history, not a deadline (`getNextAvailableCreditExpiry`,
    /// `codex-fetcher.ts:243-253`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credits: Vec<ResetCredit>,
}

/// One window wearing its own name — Code Assist quota answers per MODEL
/// (`buckets[]`), and the panel lists each by name while the segment shows
/// only the most constrained. Claude and Codex never fill this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedWindow {
    pub name: String,
    #[serde(flatten)]
    pub window: UsageWindow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider: String,
    pub session: Option<UsageWindow>,
    pub weekly: Option<UsageWindow>,
    pub fable_weekly: Option<UsageWindow>,
    /// A 30-day window, which two providers report and the rest do not —
    /// OpenCode Go always, Grok when the account is on unified billing
    /// (`rate-limit-types.ts:64`). Its own field rather than a borrowed one:
    /// `fable_weekly` is a MODEL's ledger and the window labels it "Fable", so
    /// a month put there reads as a model name to the person looking at it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly: Option<UsageWindow>,
    /// Per-model windows, when the provider's API speaks that way (Code Assist —
    /// and Antigravity, which mirrors it). Absent for everyone else, and
    /// absent from their stored snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buckets: Option<Vec<NamedWindow>>,
    /// Epoch milliseconds of the scan that produced this.
    pub updated_at: i64,
    pub error: Option<String>,
    pub status: String,
    /// What the read failed OF, when it failed.
    ///
    /// Beside `error` and never in place of it — a person reads the sentence,
    /// the scheduler reads the kind (`rate-limit-types.ts:86`). Absent from a
    /// successful read and from every snapshot written before this existed,
    /// which is why it skips serialisation when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<zerocode_core::usage_limit::FailureKind>,
    /// Unix ms before which a refetch should not be attempted, when the
    /// server named one (`rate-limit-types.ts:45`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at_ms: Option<i64>,
    /// The subscription tier the provider named for this login — Codex's
    /// `plan_type`, which the status bar spells beside the name ("Codex ·
    /// Plus", `codex-fetcher.ts:571-572`).
    ///
    /// Read and then thrown away until now: the road already REQUIRED it to be
    /// a string before it would trust the payload, so the window was one field
    /// away from being able to say which plan the numbers belong to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    /// Credits this account can spend to clear a throttled window early.
    ///
    /// Absent for every provider but Codex, and absent from Codex too when the
    /// backend does not report them (`rate-limit-types.ts:69-79`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<ResetCredits>,
    /// Which account this was read as, when the window is holding accounts.
    ///
    /// A percentage is a fact about ONE login, so a snapshot with no owner on
    /// it cannot be shown after the person switches — it would be the previous
    /// account's number sitting under the new account's name, which is the
    /// most convincing way to say "switching did nothing". `None` means the
    /// scan ran against whatever login the machine already had, which is what
    /// a window with no accounts added does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// What a reset phrase meant, without deciding whose clock resolves it.
///
/// The caller owns the local offset; this module owns only the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetMark {
    /// "4h 28m", "2d 1h", "45m" — a duration from now.
    After(Duration),
    /// "3am", "11:30pm", "14:00" — the next time the clock says this.
    AtClock { hour: u8, minute: u8 },
    /// Words this parser does not resolve ("Oct 7 at 3am") — kept as text.
    Unresolved,
}

fn collapsed(line: &str) -> String {
    line.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `/current\s*session/i`
fn is_session_label(line: &str) -> bool {
    line.to_lowercase()
        .split_whitespace()
        .collect::<String>()
        .contains("currentsession")
}

/// `/(?:current\s*week|weekly\s*(?:limits?|usage|rate\s*limits?)|7\s*-?\s*day)/i`,
/// minus anything naming Fable — that is the other ledger.
fn is_weekly_label(line: &str) -> bool {
    let flat = line
        .to_lowercase()
        .split_whitespace()
        .collect::<String>()
        .replace('-', "");
    if flat.contains("fable") {
        return false;
    }
    flat.contains("currentweek")
        || flat.contains("weeklylimit")
        || flat.contains("weeklyusage")
        || flat.contains("weeklyratelimit")
        || flat.contains("7day")
}

/// A bare `fable` line, or a weekly heading scoped to Fable.
fn is_fable_label(line: &str) -> bool {
    let low = collapsed(line);
    if low == "fable" {
        return true;
    }
    let flat = low.split_whitespace().collect::<String>().replace('-', "");
    flat.contains("fable")
        && (flat.contains("currentweek") || flat.contains("weekly") || flat.contains("7day"))
}

fn is_section_label(line: &str) -> bool {
    is_session_label(line) || is_weekly_label(line) || is_fable_label(line)
}

/// `N% used|consumed` as spent, `N% left|remaining|available` inverted.
fn percent_on_line(line: &str) -> Option<f32> {
    let low = line.to_lowercase();
    let bytes = low.as_bytes();
    for (at, _) in low.match_indices('%') {
        // Walk back over the number, fraction included.
        let mut start = at;
        while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
            start -= 1;
        }
        if start == at {
            continue;
        }
        let Ok(value) = low[start..at].parse::<f32>() else {
            continue;
        };
        // Walk forward to the word that says which direction this counts.
        let rest = low[at + 1..].trim_start();
        let word: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        match word.as_str() {
            "used" | "consumed" => return Some(value),
            "left" | "remaining" | "available" => return Some(100.0 - value),
            _ => continue,
        }
    }
    None
}

/// The percent within 12 lines of a heading, stopping at the next heading —
/// Orca's `extractPercentAfterLabel` shape exactly.
fn percent_after_label(lines: &[&str], matches: impl Fn(&str) -> bool) -> Option<f32> {
    for (i, line) in lines.iter().enumerate() {
        if !matches(line) {
            continue;
        }
        for (j, candidate) in lines.iter().enumerate().skip(i).take(12) {
            if j > i && is_section_label(candidate) {
                break;
            }
            if let Some(pct) = percent_on_line(candidate) {
                return Some(pct);
            }
        }
    }
    None
}

/// `/resets?\s+(?:at\s+|in\s+)?(.+)/i`, normalized the way Orca normalizes:
/// box-drawing and closing junk trimmed, whitespace collapsed.
fn reset_words_on_line(line: &str) -> Option<String> {
    // Boundary-safe: an index found in a lowercased COPY does not line up with
    // the original, because `to_lowercase` is not length-preserving. This one
    // only ever slices forward so it misaligns quietly rather than panicking —
    // which is worse to find, not better.
    let at = find_ascii_ignoring_case(line, "reset", 0)?;
    let mut rest = &line[at + "reset".len()..];
    if let Some(stripped) = rest.strip_prefix('s') {
        rest = stripped;
    }
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix("at ")
        .or_else(|| rest.strip_prefix("in "))
        .unwrap_or(rest);
    // Orca also trims a trailing `)`; kept here when it closes a `(` so a
    // timezone parenthetical stays balanced on screen.
    let trimmed = rest.trim_end_matches(|c: char| c == '│' || c.is_whitespace());
    let trimmed = if trimmed.ends_with(')') && !trimmed.contains('(') {
        trimmed.trim_end_matches(')')
    } else {
        trimmed
    };
    let cleaned = collapsed(trimmed);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// The reset words within 14 lines of a heading — Orca's
/// `extractClaudePtyResetMetadata` shape.
fn reset_after_label(lines: &[&str], matches: impl Fn(&str) -> bool) -> Option<String> {
    for (i, line) in lines.iter().enumerate() {
        if !matches(line) {
            continue;
        }
        for (j, candidate) in lines.iter().enumerate().skip(i).take(14) {
            if j > i && is_section_label(candidate) {
                break;
            }
            if let Some(words) = reset_words_on_line(candidate) {
                return Some(words);
            }
        }
    }
    None
}

/// What the reset words mean. Duration forms and clock forms resolve; a
/// dated form ("oct 7 at 3am") stays words — a wrong guess about a month is
/// worse than a label.
pub fn read_reset_mark(words: &str) -> ResetMark {
    // Strip a trailing timezone parenthetical: "3am (asia/seoul)".
    let bare = match words.find('(') {
        Some(at) => words[..at].trim(),
        None => words,
    };
    // Relative: entirely "<n><unit>" tokens.
    let mut total = Duration::ZERO;
    let mut all_relative = !bare.is_empty();
    for token in bare.split_whitespace() {
        let digits: String = token.chars().take_while(|c| c.is_ascii_digit()).collect();
        let unit = &token[digits.len()..];
        let Ok(count) = digits.parse::<u64>() else {
            all_relative = false;
            break;
        };
        let seconds = match unit {
            "d" | "day" | "days" => 86400,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            _ => {
                all_relative = false;
                break;
            }
        };
        total += Duration::from_secs(count * seconds);
    }
    if all_relative && total > Duration::ZERO {
        return ResetMark::After(total);
    }
    // Clock: "3am", "3:30pm", "14:00" — one token, nothing dated around it.
    if bare.split_whitespace().count() == 1 {
        let token = bare.trim();
        let (time, meridian) = if let Some(t) = token.strip_suffix("am") {
            (t, Some(false))
        } else if let Some(t) = token.strip_suffix("pm") {
            (t, Some(true))
        } else {
            (token, None)
        };
        let (hour_text, minute_text) = match time.split_once(':') {
            Some((h, m)) => (h, m),
            None => (time, "0"),
        };
        if let (Ok(mut hour), Ok(minute)) = (hour_text.parse::<u8>(), minute_text.parse::<u8>()) {
            let valid = match meridian {
                Some(_) => (1..=12).contains(&hour) && minute < 60,
                None => hour < 24 && minute < 60 && time.contains(':'),
            };
            if valid {
                if let Some(pm) = meridian {
                    hour = match (pm, hour) {
                        (false, 12) => 0,
                        (true, h) if h < 12 => h + 12,
                        (_, h) => h,
                    };
                }
                return ResetMark::AtClock { hour, minute };
            }
        }
    }
    ResetMark::Unresolved
}

pub struct ParsedUsage {
    pub session: Option<(f32, Option<String>)>,
    pub weekly: Option<(f32, Option<String>)>,
    pub fable_weekly: Option<(f32, Option<String>)>,
}

/// Read the `/usage` screen. Percentages clamp to 0..=100 as Orca clamps.
pub fn parse_usage_screen(text: &str) -> ParsedUsage {
    let lines: Vec<&str> = text.lines().collect();
    let read = |matches: &dyn Fn(&str) -> bool| -> Option<(f32, Option<String>)> {
        let pct = percent_after_label(&lines, matches)?;
        Some((pct.clamp(0.0, 100.0), reset_after_label(&lines, matches)))
    };
    ParsedUsage {
        session: read(&|line| is_session_label(line)),
        weekly: read(&|line| is_weekly_label(line)),
        fable_weekly: read(&|line| is_fable_label(line)),
    }
}

/* ---- Codex's own screen ----------------------------------------------------
 *
 * A different CLI, a different screen, a different parse. Orca reads Codex's
 * `/status` (out/main/index.js:210446) rather than Claude's `/usage`, and where
 * the Claude screen is sections with headings, Codex prints two labelled lines:
 *
 *     5h limit: 12% used (resets 3h 20m)
 *     Weekly limit: 40% used
 *
 * Two details are the measurement rather than the obvious reading:
 *
 * 1. **`used` or `left`.** The same line can print either, and Orca ORIENTS on
 *    the word (`ptyUsedPercent`, :210375) — a screen saying "88% left" means 12
 *    used, and reading it as 88 would show somebody nearly out when they have
 *    almost everything.
 * 2. **The label must start its phrase.** Orca's regexes carry a negative
 *    lookbehind, `(?<![\w-][^\S\r\n]{0,4})` (:210365) — a `5h limit` with a word
 *    or a hyphen within four spaces in front of it is part of a longer phrase
 *    ("per-5h limit", "next 5h limit"), not the label. Rust's regex crate has no
 *    lookbehind, so this is the same rule written out.
 */

/// Find an ASCII `needle` case-insensitively, at or after `from`, as a byte
/// index that is valid for slicing `haystack` ITSELF.
///
/// The obvious version — `haystack.to_lowercase().find(needle)` — hands back an
/// index into a DIFFERENT string, and `to_lowercase` is not length-preserving
/// (`ẞ` is three bytes and lowercases to two; `İ` is two and lowercases to
/// three). Slicing the original with that index panics on a char boundary the
/// moment a line carries such a letter beside multibyte text — which on a
/// Korean-localised machine is an ordinary Tuesday, not an adversarial input.
///
/// ASCII-only comparison on purpose: the labels being matched are ASCII, and a
/// UTF-8 continuation byte is never an ASCII byte, so a match found this way is
/// always on char boundaries and is always exactly `needle.len()` bytes long.
fn find_ascii_ignoring_case(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let hay = haystack.as_bytes();
    let want = needle.as_bytes();
    if want.is_empty() || hay.len() < want.len() || from > hay.len() - want.len() {
        return None;
    }
    (from..=hay.len() - want.len()).find(|&at| hay[at..at + want.len()].eq_ignore_ascii_case(want))
}

/// Is the label at `at` a real label, or part of a longer phrase?
///
/// ASCII word characters, matching the `\w` in the regex this reimplements —
/// Rust's `is_alphanumeric` is Unicode-aware and would call a CJK character in
/// front of the label a word, where the original would not.
fn label_stands_alone(line: &str, at: usize) -> bool {
    let before = &line[..at];
    // Up to four spaces or tabs, then whatever is in front of them.
    let trimmed = before.trim_end_matches([' ', '\t']);
    if before.len() - trimmed.len() > 4 {
        return true;
    }
    match trimmed.chars().next_back() {
        Some(one) => !(one.is_ascii_alphanumeric() || one == '_' || one == '-'),
        None => true,
    }
}

/// The percentage on a Codex `/status` line carrying `label`, already oriented.
fn codex_percent(line: &str, label: &str) -> Option<f32> {
    let mut from = 0usize;
    let at = loop {
        let found = find_ascii_ignoring_case(line, label, from)?;
        if label_stands_alone(line, found) {
            break found;
        }
        // Past this occurrence, and on with the search — a line may carry the
        // phrase twice with only the second one being the label.
        from = found + label.len();
    };
    let rest = &line[at + label.len()..];
    // `[^\d%\r\n]*` — the digits are the first ones after the label, and a `%`
    // or a line end before them means this line does not carry a figure.
    let digits_at = rest.find(|one: char| one.is_ascii_digit() || one == '%')?;
    if rest.as_bytes()[digits_at] == b'%' {
        return None;
    }
    // Digits and at most one decimal point. A figure of `12.5%` read as "no
    // figure at all" is worse than reading it as 12 — the window vanishes from
    // the scan, and a scan with both windows fractional reports itself broken.
    let run: String = rest[digits_at..]
        .chars()
        .take_while(|one| one.is_ascii_digit() || *one == '.')
        .collect();
    // The `%` is measured from the end of the whole RUN, and the value is
    // parsed from the run with a trailing dot dropped — `12.%` is twelve, and
    // looking for the `%` after the trimmed value would find the dot instead.
    let figure = run.trim_end_matches('.');
    if figure.is_empty() || !rest[digits_at + run.len()..].starts_with('%') {
        return None;
    }
    let pct: f32 = figure.parse().ok()?;
    // `used` or `left`, when the line says which. Orca reads the word right
    // after the `%` and flips on `left`.
    let after = rest[digits_at + run.len() + 1..].trim_start();
    let oriented = if after.len() >= 4 && after[..4].eq_ignore_ascii_case("left") {
        100.0 - pct
    } else {
        pct
    };
    Some(oriented.clamp(0.0, 100.0))
}

/// Read Codex's `/status` screen: the five-hour window and the weekly one.
///
/// The reset words come from the same line rather than from a following one —
/// Codex prints them inline, which is why this does not reuse the Claude
/// screen's look-ahead.
pub fn parse_codex_status(text: &str) -> ParsedUsage {
    let read = |label: &str| -> Option<(f32, Option<String>)> {
        text.lines().find_map(|line| {
            let pct = codex_percent(line, label)?;
            Some((pct, reset_words_in(line)))
        })
    };
    ParsedUsage {
        session: read("5h limit"),
        weekly: read("weekly limit"),
        // Codex has no third window. Absent rather than zero: a figure this
        // window invented would be drawn beside two it measured.
        fable_weekly: None,
    }
}

/// The reset phrase on a Codex status line, when it carries one.
///
/// `(resets 3h 20m)` and `resets 3h 20m` both appear; what is taken is
/// everything after the word, which [`read_reset_mark`] then reads.
fn reset_words_in(line: &str) -> Option<String> {
    // Through the boundary-safe finder for the same reason `codex_percent` is:
    // an index taken from a lowercased copy does not line up with the original.
    let at = find_ascii_ignoring_case(line, "reset", 0)?;
    let said = line[at..]
        .trim_start_matches(|one: char| one.is_alphabetic())
        .trim_matches(|one: char| one == ')' || one == '(' || one.is_whitespace());
    (!said.is_empty()).then(|| said.to_string())
}

/// Is this CLI waiting on an answer this window did not ask for?
///
/// **The reason this exists is a real incident**, on the machine this was
/// built on: `codex` opens with an "Update available" prompt whose default
/// choice is *Update now*, and a hidden terminal that nudged with a bare
/// newline ran `npm install -g @openai/codex` on somebody's machine without
/// being asked. Nothing about reading a usage figure is worth pressing an
/// unknown default.
///
/// So a scan that sees a question STOPS and says so. The one exception stays
/// the measured trust prompt ([`wants_trust_answer`]) — that one is asked
/// because we spawned into a directory, and answering it is answering our own
/// question, not somebody else's.
pub fn shows_a_question(text: &str) -> bool {
    let low = collapsed(text);
    // A menu with a numbered first choice, an explicit "press enter", or an
    // update offer. Each is a screen where Enter DOES something.
    low.contains("press enter to continue")
        || low.contains("update available")
        || low.contains("update now")
        || low.contains("1. ")
        || low.contains("❯ 1")
        || low.contains("(y/n)")
        || low.contains("[y/n]")
}

/// Codex is signed in when a `/status` screen could exist at all.
///
/// Orca probes for the auth file before spawning anything
/// (`probeCodexAuthPresence`, :210596) and reports "Codex not signed in" as its
/// own status rather than as a failed scan — the difference between "this
/// cannot be read" and "there is nothing to read" is the difference between a
/// bug and a fact.
pub fn shows_codex_signed_out(text: &str) -> bool {
    let low = collapsed(text);
    shows_signed_out(text) || low.contains("run codex login") || low.contains("codex login")
}

/// The status a scan carries when the CLI said there is no login here.
///
/// Its own word rather than `unavailable`: the account list reads it to mark
/// the row, and "could not read" must not mark anybody (1-g15).
pub const SIGNED_OUT_STATUS: &str = "signed_out";

/// The words any of these CLIs use to say "there is no login here".
///
/// Phrases, not words. Bare `sign in` and `login` appear on screens that have
/// nothing to do with auth — a banner about a GitHub login would report a
/// perfectly signed-in account as signed out.
///
/// Shared because the mistake is shared: a Claude account whose token has
/// gone answers `/usage` with `Not logged in · Please run /login`, and for a
/// while this window called that "/usage 화면이 렌더되지 않았습니다" — a bug
/// report about our renderer for a fact about their credentials (live report
/// 2026-08-14: "로그인이 풀리는 버그").
///
/// Shared, but only what is actually shared. Codex's own two phrases used to
/// sit in this list as well, and this list is what judges the **Claude**
/// screen (`scan_claude_usage_now`) — so a Claude session that merely had the
/// words `codex login` anywhere on it condemned the Claude account, and the
/// account list wears that verdict as 「로그인이 만료되었습니다 — 다시
/// 로그인하세요」 under a login that works (live report 2026-08-25: "지금 로그인
/// 계정인데 잘사용하고있는데 로그인 하라고 표시되는 버그"). One provider's
/// vocabulary is not evidence about another's credential;
/// `shows_codex_signed_out` names its own beside these.
pub fn shows_signed_out(text: &str) -> bool {
    let low = collapsed(text);
    low.contains("not signed in") || low.contains("not logged in") || low.contains("please sign in")
}

/// The screens that mean "answer y" — `/do you trust|trust the files|safety
/// check/i`.
pub fn wants_trust_answer(text: &str) -> bool {
    let low = collapsed(text);
    low.contains("do you trust") || low.contains("trust the files") || low.contains("safety check")
}

/// The 2.1 CLI's usage screen draws tabs and takes its time —
/// `/settings?\s+status?\s+config\s+usage\s+stats/i` plus the session-stats
/// face of the same screen.
pub fn shows_slow_usage_tabs(text: &str) -> bool {
    let low = collapsed(text);
    (low.contains("usage stats") && low.contains("config") && low.contains("setting"))
        || low.contains("total cost")
        || low.contains("total duration")
}

/// A section heading (or the failure text) has rendered — stop nudging and
/// let the screen settle. Orca's `STOP_SUBSTRINGS`, verbatim.
pub fn shows_a_usage_section(text: &str) -> bool {
    const STOPS: [&str; 11] = [
        "Current week (all models)",
        "Current week (Opus)",
        "Current week (Sonnet only)",
        "Current week (Sonnet)",
        "Weekly limits",
        "Weekly limit",
        "Weekly usage",
        "7-day",
        "Current session",
        "Failed to load usage data",
        "failed to load usage data",
    ];
    STOPS.iter().any(|stop| text.contains(stop))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One provider's vocabulary is not evidence about another's credential.
    ///
    /// The Claude screen is the only thing `shows_signed_out` judges, and it
    /// used to carry Codex's two phrases — so a Claude session that merely had
    /// the words on it condemned the Claude account and the settings row wore
    /// 「로그인이 만료되었습니다」 under a login that worked.
    #[test]
    fn a_claude_screen_is_not_condemned_by_another_providers_words() {
        for said in [
            "claude 2.1.241 · run codex login to use codex here",
            "$ codex login",
            "the mcp server 'codex login' is configured",
        ] {
            assert!(!shows_signed_out(said), "claude condemned by: {said}");
            // Codex still owns them, on its own screen.
            assert!(shows_codex_signed_out(said), "codex stopped seeing: {said}");
        }
        // And the phrases that ARE evidence still are, for both.
        for said in ["Not logged in · Please run /login", "You are not signed in"] {
            assert!(shows_signed_out(said), "missed: {said}");
            assert!(shows_codex_signed_out(said), "codex missed: {said}");
        }
    }

    const SCREEN: &str = "\
 Settings   Status   Config   Usage
 ╭──────────────────────────────────╮
 │ Current session                  │
 │ ████░░░░░░░░░░░░  26% used       │
 │ Resets 3am (Asia/Seoul)          │
 │                                  │
 │ Current week (all models)        │
 │ ██░░░░░░░░░░░░░░  88% left       │
 │ Resets in 4d 3h                  │
 │                                  │
 │ Current week (Fable)             │
 │ █░░░░░░░░░░░░░░░  7% used        │
 │ Resets Oct 7 at 3am              │
 ╰──────────────────────────────────╯";

    #[test]
    fn the_three_ledgers_are_read_apart() {
        let parsed = parse_usage_screen(SCREEN);
        let (session, session_reset) = parsed.session.expect("session");
        eq_pct(session, 26.0);
        assert_eq!(session_reset.as_deref(), Some("3am (asia/seoul)"));
        // `left` counts down, so it inverts.
        let (weekly, weekly_reset) = parsed.weekly.expect("weekly");
        eq_pct(weekly, 12.0);
        assert_eq!(weekly_reset.as_deref(), Some("4d 3h"));
        let (fable, fable_reset) = parsed.fable_weekly.expect("fable");
        eq_pct(fable, 7.0);
        assert_eq!(fable_reset.as_deref(), Some("oct 7 at 3am"));
    }

    fn eq_pct(actual: f32, wanted: f32) {
        assert!((actual - wanted).abs() < 0.01, "{actual} != {wanted}");
    }

    #[test]
    fn a_screen_with_no_usage_reads_as_nothing_not_zero() {
        let parsed = parse_usage_screen("welcome to claude\ntype /help to begin");
        assert!(parsed.session.is_none());
        assert!(parsed.weekly.is_none());
        assert!(parsed.fable_weekly.is_none());
    }

    #[test]
    fn the_reset_words_resolve_or_stay_words() {
        assert_eq!(
            read_reset_mark("4h 28m"),
            ResetMark::After(Duration::from_secs(4 * 3600 + 28 * 60))
        );
        assert_eq!(
            read_reset_mark("2d 1h"),
            ResetMark::After(Duration::from_secs(2 * 86400 + 3600))
        );
        assert_eq!(
            read_reset_mark("3am (asia/seoul)"),
            ResetMark::AtClock { hour: 3, minute: 0 }
        );
        assert_eq!(
            read_reset_mark("11:30pm"),
            ResetMark::AtClock {
                hour: 23,
                minute: 30
            }
        );
        assert_eq!(
            read_reset_mark("12am"),
            ResetMark::AtClock { hour: 0, minute: 0 }
        );
        assert_eq!(
            read_reset_mark("14:05"),
            ResetMark::AtClock {
                hour: 14,
                minute: 5
            }
        );
        // A dated phrase is not guessed at.
        assert_eq!(read_reset_mark("oct 7 at 3am"), ResetMark::Unresolved);
    }

    #[test]
    fn the_scan_reads_the_screens_it_steers_by() {
        assert!(wants_trust_answer("Do you trust the files in this folder?"));
        assert!(shows_slow_usage_tabs(
            " Settings  Status  Config  Usage  Stats "
        ));
        assert!(shows_a_usage_section("│ Current session │"));
        assert!(!shows_a_usage_section("$ claude"));
    }

    #[test]
    fn percentages_clamp_and_fractions_read() {
        let parsed = parse_usage_screen("Current session\n120% used");
        assert_eq!(parsed.session.unwrap().0, 100.0);
        let parsed = parse_usage_screen("Current session\n26.5% used");
        assert!((parsed.session.unwrap().0 - 26.5).abs() < 0.01);
    }

    #[test]
    fn codex_prints_two_labelled_lines_and_they_read() {
        let parsed = parse_codex_status(
            "  Account: joe@example.com\n  5h limit: 12% used (resets 3h 20m)\n  Weekly limit: 40% used\n",
        );
        let (session, session_reset) = parsed.session.expect("no five-hour window");
        assert_eq!(session, 12.0);
        assert_eq!(session_reset.as_deref(), Some("3h 20m"));
        assert_eq!(parsed.weekly.expect("no weekly window").0, 40.0);
        // Codex has no third window, and a figure invented here would be drawn
        // beside two that were measured.
        assert!(parsed.fable_weekly.is_none());
    }

    /// The trap that makes a nearly-empty account look nearly-full. The same
    /// line prints either word, and Orca orients on it (`ptyUsedPercent`).
    #[test]
    fn a_screen_that_says_left_is_not_read_as_used() {
        let parsed = parse_codex_status("5h limit: 88% left\nWeekly limit: 3% left");
        assert_eq!(parsed.session.expect("no session").0, 12.0);
        assert_eq!(parsed.weekly.expect("no weekly").0, 97.0);
        // And clamping still holds on the flipped side.
        let over = parse_codex_status("5h limit: 130% left");
        assert_eq!(over.session.expect("no session").0, 0.0);
    }

    /// Orca's negative lookbehind, written out. A label with a word or a hyphen
    /// within four spaces in front of it is part of a longer phrase, not the
    /// label — and a percentage read out of prose is a figure nobody can trace.
    #[test]
    fn a_label_inside_a_longer_phrase_is_not_the_label() {
        assert!(
            parse_codex_status("per-5h limit: 12% used")
                .session
                .is_none()
        );
        assert!(
            parse_codex_status("your 5h limit: 12% used")
                .session
                .is_none()
        );
        // Far enough away is a new phrase, which is what the four-space bound
        // is for — a screen aligns its labels in columns.
        assert_eq!(
            parse_codex_status("Account        5h limit: 12% used")
                .session
                .expect("a column-aligned label was refused")
                .0,
            12.0
        );
        // And the first occurrence being prose does not hide a real one after it.
        assert_eq!(
            parse_codex_status("about your 5h limit — see below\n5h limit: 7% used")
                .session
                .expect("the real label was skipped")
                .0,
            7.0
        );
    }

    /// The incident this rule came from: a hidden terminal nudged with a bare
    /// newline at an "Update available — 1. Update now" screen and ran a global
    /// npm install on somebody's machine. Nothing about a usage figure is worth
    /// pressing an unknown default.
    #[test]
    fn a_question_this_window_did_not_ask_stops_the_scan() {
        for asking in [
            "✨ Update available! 0.146.0 -> 0.147.0\n 1. Update now (runs `npm install -g …`)",
            "Press enter to continue",
            "Overwrite the file? [y/N]",
            "❯ 1. Yes",
        ] {
            assert!(
                shows_a_question(asking),
                "a scan would have answered this screen: {asking}"
            );
        }
        // And an ordinary status screen is not a question — a scan that gave up
        // on every screen would never read anything.
        assert!(!shows_a_question(
            "5h limit: 12% used (resets 3h 20m)\nWeekly limit: 40% used"
        ));
        assert!(!shows_a_question(""));
    }

    /// Found by review, reproduced by fuzzing: an index taken from a lowercased
    /// COPY was used to slice the ORIGINAL, and `to_lowercase` is not
    /// length-preserving — `ẞ` is three bytes and lowercases to two, `İ` is two
    /// and lowercases to three. On a Korean-localised machine a workspace name
    /// beside the label is an ordinary line, not an adversarial one, and this
    /// panicked mid-character. A panic here also wedged the provider's
    /// in-flight flag on forever, so the segment never recovered.
    #[test]
    fn a_label_beside_text_that_changes_length_when_lowercased_does_not_panic() {
        for line in [
            "ẞ가가5h limit: 12% used",
            "İ가 5h limit: 12% used",
            "가나다 ẞ İ Weekly limit: 40% used",
            "ẞ가가reset 3h 20m",
            "İ작업 5h limit: 12% used (resets 3h 20m)",
        ] {
            // The assertion is that these return at all.
            let _ = parse_codex_status(line);
            let _ = reset_words_in(line);
            let _ = reset_words_on_line(line);
        }
        // And the label is still FOUND when such a letter sits in front of it —
        // the misalignment silently missed real labels as often as it panicked.
        let read = parse_codex_status("가 ẞ  5h limit: 12% used");
        assert_eq!(
            read.session.expect("a real label was missed").0,
            12.0,
            "the label was not found beside a letter that changes length"
        );
    }

    /// Also found by review. `12.5%` came back as no figure at all rather than
    /// as 12.5 — and a scan whose only two windows are fractional then reports
    /// itself broken, which sends somebody looking for a bug in the wrong place.
    /// The Claude parser beside this one has read fractions since it was
    /// written.
    #[test]
    fn a_fractional_percentage_is_a_figure_and_not_a_miss() {
        let parsed = parse_codex_status("5h limit: 12.5% used\nWeekly limit: 7.25% left");
        assert!((parsed.session.expect("no session").0 - 12.5).abs() < 0.01);
        assert!((parsed.weekly.expect("no weekly").0 - 92.75).abs() < 0.01);
        // A trailing dot is not part of the figure — `12.% used` is 12.
        assert_eq!(
            parse_codex_status("5h limit: 12.% used")
                .session
                .expect("no session")
                .0,
            12.0
        );
    }

    #[test]
    fn a_line_with_no_figure_on_it_is_not_a_window() {
        for hopeless in [
            "5h limit: unknown",
            "5h limit",
            "5h limit: % used",
            "Weekly limit: soon",
            "",
        ] {
            let parsed = parse_codex_status(hopeless);
            assert!(
                parsed.session.is_none() && parsed.weekly.is_none(),
                "`{hopeless}` produced a figure"
            );
        }
    }

    /// A snapshot written before these fields existed must still read, and a
    /// provider that has nothing to say about them must not write them.
    #[test]
    fn the_new_codex_facts_are_optional_in_both_directions() {
        let old_snapshot = r#"{"provider":"codex","session":null,"weekly":null,
            "fable_weekly":null,"updated_at":17,"error":null,"status":"ok"}"#;
        let read: ProviderUsage = serde_json::from_str(old_snapshot).expect("old snapshot");
        assert!(read.plan_type.is_none() && read.reset_credits.is_none());

        // Nothing to say → nothing written. A snapshot full of nulls is a
        // snapshot that grows a key every time a provider learns a word.
        let written = serde_json::to_string(&read).expect("write");
        assert!(
            !written.contains("plan_type") && !written.contains("reset_credits"),
            "an empty fact was written anyway: {written}"
        );

        let full = ProviderUsage {
            plan_type: Some("plus".to_string()),
            reset_credits: Some(ResetCredits {
                available_count: 2,
                total_earned_count: Some(5),
                next_expires_at: Some(1_800_000_000_000),
                credits: vec![ResetCredit {
                    status: "available".to_string(),
                    expires_at: Some(1_800_000_000_000),
                    granted_at: None,
                }],
            }),
            ..read
        };
        let round = serde_json::to_string(&full).expect("write");
        assert_eq!(
            serde_json::from_str::<ProviderUsage>(&round).expect("read"),
            full
        );
    }
}
