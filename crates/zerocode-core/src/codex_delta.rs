//! How much a Codex turn actually spent, out of a record that reports two
//! numbers and means neither of them the obvious way.
//!
//! Every `token_count` event carries a running session total AND the figure
//! for that turn. Adding the totals up is the trap, and it is not a near
//! miss: on one session in this machine's own corpus the final total is
//! 18,905,900 tokens, the turns sum to 19,476,166 — and adding the running
//! totals gives **1,012,523,733**, fifty-three times the truth. Claude's
//! corpus taught that a naive count is wrong by half; this one is wrong by
//! fifty.
//!
//! Two rules here are worth stating because neither is what you would write
//! from first principles, and both are Orca's
//! (`codex-usage-token-delta.ts:104-150`):
//!
//! - **The increment is `last_token_usage`, not the subtraction.** Totals are
//!   mutable snapshots — compaction and resume rewrite them — so the
//!   difference between two of them is not a spend. The per-turn figure is
//!   the billable one, and the totals are only a baseline for spotting
//!   duplicates and rewinds.
//! - **The identity of an event does not include the session.** Forking or
//!   resuming copies `token_count` records byte-for-byte into a new rollout
//!   file while rewriting the session id, so keying on the session would
//!   count the copy again. The key is the timestamp and the two usage tuples
//!   — the parts a copy preserves exactly.

use serde::Serialize;

/// One reading of Codex's counters.
///
/// `reasoning` is reported apart but is already inside `output`, which is why
/// the synthesised total below adds input and output alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RawUsage {
    pub input: u64,
    pub cached_input: u64,
    pub output: u64,
    pub reasoning_output: u64,
    pub total: u64,
}

impl RawUsage {
    /// Everything that is not the total, added — the figure the monotonic and
    /// regression checks compare on (`rawUsageMagnitude`, `:96-100`).
    const fn magnitude(self) -> u64 {
        self.input + self.cached_input + self.output + self.reasoning_output
    }

    /// Whether two readings hold the same counters, the total aside
    /// (`rawUsageEquals`, `:78-86`).
    const fn same_counters(self, other: Self) -> bool {
        self.input == other.input
            && self.cached_input == other.cached_input
            && self.output == other.output
            && self.reasoning_output == other.reasoning_output
    }

    /// Whether this reading only ever moved forward from `previous`
    /// (`rawUsageIsMonotonic`, `:88-94`).
    const fn moved_forward_from(self, previous: Self) -> bool {
        self.input >= previous.input
            && self.cached_input >= previous.cached_input
            && self.output >= previous.output
            && self.reasoning_output >= previous.reasoning_output
    }

    /// Field by field, floored at zero (`subtractRawUsage`, `:44-58`).
    const fn minus(self, previous: Self) -> Self {
        Self {
            input: self.input.saturating_sub(previous.input),
            cached_input: self.cached_input.saturating_sub(previous.cached_input),
            output: self.output.saturating_sub(previous.output),
            reasoning_output: self
                .reasoning_output
                .saturating_sub(previous.reasoning_output),
            total: self.total.saturating_sub(previous.total),
        }
    }

    /// Field by field (`addRawUsage`, `:60-70`).
    const fn plus(self, other: Self) -> Self {
        Self {
            input: self.input + other.input,
            cached_input: self.cached_input + other.cached_input,
            output: self.output + other.output,
            reasoning_output: self.reasoning_output + other.reasoning_output,
            total: self.total + other.total,
        }
    }
}

/// One `usage` object out of a record, or `None` when there is not one.
///
/// `cached_input_tokens` is the current spelling and `cache_read_input_tokens`
/// the older one; a total of zero is synthesised from input and output because
/// legacy logs omit it — and reasoning is deliberately left out of that sum,
/// since it is billed inside output already (`normalizeRawUsage`, `:16-42`).
#[must_use]
pub fn normalize(value: Option<&serde_json::Value>) -> Option<RawUsage> {
    let value = value?;
    if !value.is_object() {
        return None;
    }
    let count = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    let input = count("input_tokens");
    let output = count("output_tokens");
    let total = count("total_tokens");
    Some(RawUsage {
        input,
        cached_input: value
            .get("cached_input_tokens")
            .or_else(|| value.get("cache_read_input_tokens"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        output,
        reasoning_output: count("reasoning_output_tokens"),
        total: if total > 0 { total } else { input + output },
    })
}

/// What one event contributes, and what the running baseline becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// A real increment, and the totals to carry forward.
    Spend {
        delta: RawUsage,
        next_totals: Option<RawUsage>,
    },
    /// No spend — this event only re-baselines the running totals, which
    /// happens when they rewind for a reason that is not a duplicate.
    Baseline { next_totals: RawUsage },
}

/// Whether a rewind looks like a stale re-report rather than a fresh session.
///
/// Orca's `looksLikeStaleRegression` (`:102-116`), including its two odd
/// thresholds: a current magnitude within 2% of the previous one, or one that
/// gets there once you add twice the turn's own figure. Ported as written —
/// these are shapes measured off real logs, not a rule that can be re-derived.
const fn looks_like_stale_regression(
    current: RawUsage,
    previous: RawUsage,
    last: RawUsage,
) -> bool {
    let (current, previous, last) = (current.magnitude(), previous.magnitude(), last.magnitude());
    if previous == 0 || current == 0 || last == 0 {
        return false;
    }
    current * 100 >= previous * 98 || current + last * 2 >= previous
}

/// What this event spent, given what the last one left behind.
///
/// `None` means the event contributes nothing — it is a duplicate, or a stale
/// re-report of numbers already counted (`resolveCodexUsageDelta`, `:118-160`).
#[must_use]
pub fn resolve(
    total_usage: Option<RawUsage>,
    last_usage: Option<RawUsage>,
    previous_totals: Option<RawUsage>,
) -> Option<Resolution> {
    match (total_usage, last_usage, previous_totals) {
        (Some(total), Some(last), Some(previous)) => {
            if total.same_counters(previous) {
                return None;
            }
            if !total.moved_forward_from(previous)
                && looks_like_stale_regression(total, previous, last)
            {
                return None;
            }
            // The turn's own figure, NOT `total - previous`: the totals are
            // snapshots that compaction rewrites, so their difference is not
            // a spend.
            Some(Resolution::Spend {
                delta: last,
                next_totals: Some(total),
            })
        }
        (Some(total), Some(last), None) => Some(Resolution::Spend {
            delta: last,
            next_totals: Some(total),
        }),
        (Some(total), None, Some(previous)) => {
            if total.same_counters(previous) {
                return None;
            }
            if total.moved_forward_from(previous) {
                Some(Resolution::Spend {
                    delta: total.minus(previous),
                    next_totals: Some(total),
                })
            } else {
                // Rewound with no turn figure to judge it by: take it as a new
                // baseline rather than guessing a spend.
                Some(Resolution::Baseline { next_totals: total })
            }
        }
        (Some(total), None, None) => Some(Resolution::Spend {
            delta: total,
            next_totals: Some(total),
        }),
        (None, Some(last), Some(previous)) => Some(Resolution::Spend {
            delta: last,
            next_totals: Some(previous.plus(last)),
        }),
        (None, Some(last), None) => Some(Resolution::Spend {
            delta: last,
            next_totals: None,
        }),
        (None, None, _) => None,
    }
}

/// The name that says two events are the same event.
///
/// Deliberately without the session: a fork copies `token_count` records
/// byte-for-byte into a new rollout file and rewrites `session_meta.id`, so a
/// session-keyed identity would count the copy twice. Only the fields a copy
/// preserves exactly are in here (`buildCodexUsageEventKey`, `:162-178`).
#[must_use]
pub fn event_key(at: &str, total: Option<RawUsage>, last: Option<RawUsage>) -> String {
    let tuple = |usage: Option<RawUsage>| {
        usage.map_or_else(String::new, |held| {
            format!(
                "{},{},{},{},{}",
                held.input, held.cached_input, held.output, held.reasoning_output, held.total
            )
        })
    };
    format!("{at}|{}|{}", tuple(total), tuple(last))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, cached: u64, output: u64, reasoning: u64, total: u64) -> RawUsage {
        RawUsage {
            input,
            cached_input: cached,
            output,
            reasoning_output: reasoning,
            total,
        }
    }

    /// A missing total is input plus output — and NOT plus reasoning, which is
    /// already inside output.
    #[test]
    fn a_reading_without_a_total_synthesises_one_without_double_counting() {
        let held = normalize(Some(&serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 30,
            "reasoning_output_tokens": 20,
        })))
        .expect("a usage object was refused");
        assert_eq!(held.total, 130, "reasoning was billed twice");
        assert_eq!(held.reasoning_output, 20, "reasoning was lost");

        // A stated total wins over the synthesised one.
        let stated = normalize(Some(&serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 30,
            "total_tokens": 999,
        })))
        .expect("refused");
        assert_eq!(stated.total, 999);

        // Both spellings of the cached field are read.
        for name in ["cached_input_tokens", "cache_read_input_tokens"] {
            let held = normalize(Some(&serde_json::json!({ name: 7 }))).expect("refused");
            assert_eq!(held.cached_input, 7, "{name} was not read");
        }
        assert_eq!(normalize(None), None);
        assert_eq!(normalize(Some(&serde_json::json!("not an object"))), None);
    }

    /// The increment is the turn's own figure, never the difference between
    /// two running totals.
    #[test]
    fn the_spend_is_the_turns_figure_and_not_the_gap_between_totals() {
        let previous = usage(1000, 500, 100, 50, 1100);
        let total = usage(3000, 1500, 300, 150, 3300);
        let last = usage(120, 60, 12, 6, 132);
        let held = resolve(Some(total), Some(last), Some(previous)).expect("no resolution");
        assert_eq!(
            held,
            Resolution::Spend {
                delta: last,
                next_totals: Some(total),
            },
            "the gap between totals was billed instead of the turn"
        );
    }

    /// The same totals twice is one event, not two.
    #[test]
    fn a_repeated_reading_spends_nothing() {
        let standing = usage(1000, 500, 100, 50, 1100);
        // Same counters, and a different `total` field does not make it new.
        let echo = RawUsage {
            total: 9999,
            ..standing
        };
        assert_eq!(
            resolve(Some(echo), Some(usage(1, 1, 1, 1, 1)), Some(standing)),
            None
        );
        assert_eq!(resolve(Some(standing), None, Some(standing)), None);
    }

    /// A rewind that looks like a stale re-report is dropped; one that looks
    /// like a fresh start is taken.
    #[test]
    fn a_rewind_is_read_as_stale_or_as_a_fresh_baseline() {
        let previous = usage(10_000, 0, 1_000, 0, 11_000);
        // Nearly the same size as before: a re-report, worth nothing.
        let nearly = usage(9_900, 0, 990, 0, 10_890);
        assert_eq!(
            resolve(Some(nearly), Some(usage(10, 0, 1, 0, 11)), Some(previous)),
            None
        );

        // Far smaller, and the turn's figure cannot bridge the gap: a real new
        // session, so it spends its turn and re-baselines.
        let fresh = usage(50, 0, 5, 0, 55);
        let turn = usage(50, 0, 5, 0, 55);
        assert_eq!(
            resolve(Some(fresh), Some(turn), Some(previous)),
            Some(Resolution::Spend {
                delta: turn,
                next_totals: Some(fresh),
            })
        );

        // With no turn figure to judge by, a rewind only re-baselines.
        assert_eq!(
            resolve(Some(fresh), None, Some(previous)),
            Some(Resolution::Baseline { next_totals: fresh })
        );
    }

    /// With only totals to go on, the difference IS the spend — that is the
    /// one branch where subtraction is right.
    #[test]
    fn totals_alone_are_differenced_forward() {
        let previous = usage(1000, 500, 100, 50, 1100);
        let total = usage(1500, 700, 160, 80, 1660);
        assert_eq!(
            resolve(Some(total), None, Some(previous)),
            Some(Resolution::Spend {
                delta: usage(500, 200, 60, 30, 560),
                next_totals: Some(total),
            })
        );
        // And the very first event has nothing to subtract from.
        assert_eq!(
            resolve(Some(total), None, None),
            Some(Resolution::Spend {
                delta: total,
                next_totals: Some(total),
            })
        );
        assert_eq!(resolve(None, None, Some(previous)), None);
    }

    /// A forked session copies the record and rewrites the session id, so the
    /// key must not contain one.
    #[test]
    fn the_same_event_in_two_files_carries_the_same_name() {
        let total = usage(100, 50, 10, 5, 110);
        let last = usage(10, 5, 1, 0, 11);
        let one = event_key("2026-08-13T17:40:08.720Z", Some(total), Some(last));
        let copy = event_key("2026-08-13T17:40:08.720Z", Some(total), Some(last));
        assert_eq!(one, copy, "a byte-identical copy got a different name");

        // A different moment, or different numbers, is a different event.
        assert_ne!(
            one,
            event_key("2026-08-13T17:40:09.720Z", Some(total), Some(last))
        );
        assert_ne!(
            one,
            event_key("2026-08-13T17:40:08.720Z", Some(total), None)
        );
        assert_ne!(
            one,
            event_key(
                "2026-08-13T17:40:08.720Z",
                Some(RawUsage {
                    output: 11,
                    ..total
                }),
                Some(last),
            )
        );
    }
}
