//! Who may try a model nobody has evidence for, and how often — the pure
//! half of the challenger arm ([`crate::jev::CHALLENGER`]).
//!
//! Every number here is the seat row's or the user's decision relayed with
//! it: one attempt in [`crate::jev::CHALLENGER_ONE_IN`], a day's challenger
//! spend under [`crate::jev::CHALLENGER_DAY_SPEND_PERMILLE`] of the day's
//! own, and four kinds of route that never draw at all.
//!
//! It is IO-free and clock-free on purpose: the draw has to answer the same
//! way for the same attempt however often it is asked, or a retry would
//! re-roll and a dashboard would disagree with what actually ran. The caller
//! hands in what it knows; this module only decides.

use sha2::{Digest, Sha256};

use crate::jev::{CHALLENGER_DAY_SPEND_PERMILLE, CHALLENGER_ONE_IN};

/// What an attempt is, as far as the draw is concerned.
///
/// A struct rather than five arguments, because every field is a `bool` or a
/// role and a caller that swapped two of them would compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt<'a> {
    /// A name that is this attempt's and no other's — the dispatch id, or
    /// the run's own attempt key. The draw is taken over it.
    pub key: &'a str,
    /// The route role, by the key the outcome ledger writes
    /// (`coding`, `fast`, `verifier`, …).
    pub role: &'a str,
    /// Whether this attempt is a retry of one that already ran, or a task
    /// handed over from another worker.
    pub retry_or_handover: bool,
    /// Whether the work sits behind a guard — payment, the operator, the
    /// release lane. A guarded flow is not a place to ask a new question.
    pub guarded: bool,
}

/// Why an attempt did not draw a challenger. Closed, because a ledger row
/// carries the word and a reader greps for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// The role is one the arm does not touch.
    Role,
    /// A retry or a handover.
    Retry,
    /// A guarded flow.
    Guarded,
    /// Eligible, but this attempt is one of the four in five that do not
    /// draw.
    NotDrawn,
    /// The day's challenger spend has reached its share.
    DayBudget,
}

impl Held {
    /// The word this holding writes in a ledger row.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Role => "role",
            Self::Retry => "retry",
            Self::Guarded => "guarded",
            Self::NotDrawn => "not_drawn",
            Self::DayBudget => "day_budget",
        }
    }
}

/// The roles the arm may challenge, by the outcome ledger's own keys.
///
/// The user's first pass (2026-09-22): the implementation ladder, the fast
/// lane, and the read-only work of exploring and summarising. Everything
/// else is either a judgment this product relies on being right
/// (`verifier`, `reviewer`, `judge`, `synthesizer`) or work where a second
/// opinion costs more than it can win.
pub const CHALLENGED_ROLES: [&str; 5] = ["coding", "debugging", "fast", "research", "analysis"];

/// Whether `role` is one the arm may challenge.
#[must_use]
pub fn role_may_be_challenged(role: &str) -> bool {
    CHALLENGED_ROLES.contains(&role)
}

/// Whether `key` is the one attempt in [`CHALLENGER_ONE_IN`] that draws.
///
/// SHA-256 over a domain-separated key rather than a random number: the same
/// attempt must draw the same way when a dashboard re-reads it, when a
/// replay harness re-derives it, and when a person asks why this attempt and
/// not that one. `DefaultHasher` is documented as unstable across compiler
/// releases and this decision is written to a ledger, so a newer build would
/// re-roll every historical row.
#[must_use]
pub fn draws(key: &str) -> bool {
    /// What this digest is OF, so a draw can never collide with some other
    /// digest this product takes over the same bytes.
    const DOMAIN: &str = "zerocode.jev.challenger.draw.v1";

    let mut digest = Sha256::new();
    digest.update(DOMAIN.as_bytes());
    digest.update([0u8]);
    digest.update(key.as_bytes());
    let bytes = digest.finalize();
    let head = u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    head % CHALLENGER_ONE_IN == 0
}

/// Whether a day that has spent `challenger_micros` of `day_micros` may
/// spend more on the arm.
///
/// Both in the price table's own units, whatever they are — this reads a
/// ratio, so it cannot disagree with the table about what a dollar is. A day
/// that has spent nothing has room (a first challenge is what makes the
/// ratio meaningful at all).
#[must_use]
pub fn within_day_budget(challenger_micros: u128, day_micros: u128) -> bool {
    if day_micros == 0 {
        return true;
    }
    challenger_micros.saturating_mul(1_000)
        <= day_micros * u128::from(CHALLENGER_DAY_SPEND_PERMILLE)
}

/// The whole decision, in the order §2 asks it: what the attempt is, then
/// the draw, then what the day has left.
///
/// The cheap refusals come first so a held-back role never costs a digest,
/// and the budget last so a day's spend is only read for an attempt that
/// would otherwise challenge.
///
/// # Errors
/// The first line the attempt does not clear.
pub fn challenges(
    attempt: &Attempt<'_>,
    challenger_micros: u128,
    day_micros: u128,
) -> Result<(), Held> {
    if attempt.retry_or_handover {
        return Err(Held::Retry);
    }
    if attempt.guarded {
        return Err(Held::Guarded);
    }
    if !role_may_be_challenged(attempt.role) {
        return Err(Held::Role);
    }
    if !draws(attempt.key) {
        return Err(Held::NotDrawn);
    }
    if !within_day_budget(challenger_micros, day_micros) {
        return Err(Held::DayBudget);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
