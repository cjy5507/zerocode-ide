//! Whether a seat left on `auto` has earned the right to act, and whether one
//! that is acting has lost it (docs/design/jev-settings-20260917.md §4).
//!
//! `auto` says a seat records until its own evidence promotes it. Until this
//! module there was nothing that promoted: [`JevUse::promotes`] was a field
//! only tests had ever read, so every `auto` in the product was `shadow` under
//! a name that promised otherwise. A switch that says it will decide and never
//! does is worse than one that says it will not.
//!
//! The judgment is pure. What it needs from outside — the window's tally, the
//! apply stage's own deadline, whether labels exist and what they said — is
//! handed in, because each of those is owned somewhere else and a judge that
//! went and read them would be a second copy of all three.

use serde_json::{Value, json};

use crate::jev::summary::{AT, JUDGED_EVERY_ROWS, TRANSITION, Tally};

/// The word every schema refusal's token is built from: zo writes it alone
/// when a reply failed its checks, and [`crate::jev::choice::ChoiceRefusal`]
/// writes it with the broken rule after it. Spelled once here, and a contract
/// holds the rest to it — a seat is not promoted while any reply is arriving
/// malformed, so the judge has to be able to recognise one.
pub const SCHEMA: &str = "schema";

/// How many fallbacks in a row end an applying seat's turn at acting (§4).
///
/// Three, and not one: a single timeout is the wire having a bad minute, and
/// a seat that fell back to recording on every one of those would spend its
/// life climbing back. Three in a row is the wire, the key or the model, none
/// of which the next request will fix.
pub const FALLBACKS_THAT_END_IT: u32 = 3;

/// Whether an outcome token names a reply that arrived and failed its checks.
#[must_use]
pub fn names_a_schema_failure(token: &str) -> bool {
    token == SCHEMA
        || token
            .strip_prefix(SCHEMA)
            .is_some_and(|rest| rest.starts_with('_'))
}

/// Where a seat stands when it is judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stand {
    /// Recording only — what `auto` starts as.
    Recording,
    /// Acting on answers that pass their checks.
    Applying,
}

/// What the evidence says about labels, when a person has written any.
///
/// Agreement with the probe is deliberately not enough: two readers saying the
/// same thing is not evidence that either is right, and §4 says so. Only a
/// comparison against labels a person wrote can carry a seat up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labels {
    /// Labelled rows compared.
    pub compared: usize,
    /// Rows where the judgment named what the label named.
    pub judgment_right: usize,
    /// Rows where the probe did.
    pub probe_right: usize,
}

impl Labels {
    /// Whether the judgment is at least as right as the probe it would replace.
    #[must_use]
    pub const fn judgment_at_least_as_right(&self) -> bool {
        self.judgment_right >= self.probe_right
    }
}

/// Everything the judgment reads.
#[derive(Debug, Clone, Copy)]
pub struct Evidence<'window> {
    /// The recent window's rows.
    pub window: &'window Tally,
    /// The use's own rise line, per thousand.
    pub floor_permille: u16,
    /// The apply stage's deadline in milliseconds — the same constant the
    /// stage falls back on, read and not respelled.
    pub deadline_ms: u64,
    /// What labels said, when any exist.
    pub labels: Option<Labels>,
    /// Fallbacks in a row while applying.
    pub fallbacks_in_a_row: u32,
}

/// Why a seat may not act, in the order §4 asks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Line {
    /// Fewer rows than a judgment window.
    TooFewRows { rows: usize, wanted: usize },
    /// The answered share's lower bound is under the use's floor.
    Answered {
        bound_permille: u16,
        floor_permille: u16,
    },
    /// The calls' p95 is over the apply stage's deadline.
    Latency { p95_ms: u64, deadline_ms: u64 },
    /// Replies arrived malformed in the window.
    Schema { rows: usize },
    /// Nobody has labelled enough rows to say whether it is right.
    NoLabels { wanted: usize },
    /// Labels say the probe is righter.
    Labels {
        judgment_right: usize,
        probe_right: usize,
    },
    /// It fell back this many times in a row while acting.
    Fallbacks { in_a_row: u32 },
}

/// What the judgment decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Record-only, and this is the first line it does not clear.
    Hold(Line),
    /// Rise to acting: every line clear.
    Rise,
    /// Keep acting.
    Keep,
    /// Fall back to recording, on this line.
    Fall(Line),
}

/// The share, per thousand, a bound in `[0, 1]` reaches. Floored, so a bound
/// is never read as clearing a line it sits a hair under.
#[must_use]
pub fn permille(share: f64) -> u16 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let floored = (share.clamp(0.0, 1.0) * 1000.0).floor() as u16;
    floored
}

/// Rows in the window whose reply arrived and failed its checks.
#[must_use]
pub fn schema_rows(window: &Tally) -> usize {
    window
        .failures
        .iter()
        .filter(|(token, _)| names_a_schema_failure(token))
        .map(|(_, rows)| *rows)
        .sum()
}

/// The first line the evidence does not clear, in §4's order, or `None` when
/// it clears them all.
#[must_use]
pub fn first_broken_line(evidence: &Evidence) -> Option<Line> {
    let window = evidence.window;
    if window.rows < JUDGED_EVERY_ROWS {
        return Some(Line::TooFewRows {
            rows: window.rows,
            wanted: JUDGED_EVERY_ROWS,
        });
    }
    let bound = permille(window.answered_lower_bound().unwrap_or(0.0));
    if bound < evidence.floor_permille {
        return Some(Line::Answered {
            bound_permille: bound,
            floor_permille: evidence.floor_permille,
        });
    }
    if let Some(p95) = window.p95_ms.filter(|p95| *p95 > evidence.deadline_ms) {
        return Some(Line::Latency {
            p95_ms: p95,
            deadline_ms: evidence.deadline_ms,
        });
    }
    let malformed = schema_rows(window);
    if malformed > 0 {
        return Some(Line::Schema { rows: malformed });
    }
    match evidence.labels {
        None => Some(Line::NoLabels {
            wanted: JUDGED_EVERY_ROWS,
        }),
        Some(labels) if labels.compared < JUDGED_EVERY_ROWS => Some(Line::NoLabels {
            wanted: JUDGED_EVERY_ROWS,
        }),
        Some(labels) if !labels.judgment_at_least_as_right() => Some(Line::Labels {
            judgment_right: labels.judgment_right,
            probe_right: labels.probe_right,
        }),
        Some(_) => None,
    }
}

/// Judge a seat on its window (§4).
///
/// An applying seat is asked the fallback question first: a wire that has
/// failed three times running is not a seat's fault and not something the
/// other lines can see, and the whole point of that rule is that it does not
/// wait for the next twenty rows.
#[must_use]
pub fn judge(stand: Stand, evidence: &Evidence) -> Verdict {
    if stand == Stand::Applying && evidence.fallbacks_in_a_row >= FALLBACKS_THAT_END_IT {
        return Verdict::Fall(Line::Fallbacks {
            in_a_row: evidence.fallbacks_in_a_row,
        });
    }
    match (stand, first_broken_line(evidence)) {
        (Stand::Recording, None) => Verdict::Rise,
        (Stand::Recording, Some(line)) => Verdict::Hold(line),
        (Stand::Applying, None) => Verdict::Keep,
        // A seat already acting is not held to the window's width: it earned
        // its place on a full one, and a fresh window is not evidence against
        // it. Only a line it actually breaks takes it back.
        (Stand::Applying, Some(Line::TooFewRows { .. } | Line::NoLabels { .. })) => Verdict::Keep,
        (Stand::Applying, Some(line)) => Verdict::Fall(line),
    }
}

#[cfg(test)]
mod tests;

/// What a rising row's [`TRANSITION`](crate::jev::summary::TRANSITION) says.
pub const ROSE: &str = "rise";
/// What a falling row's says.
pub const FELL: &str = "fall";
/// The key holding the line a fall was decided on, for a person reading back.
pub const ON_LINE: &str = "line";

impl Line {
    /// The word this line writes in a transition row. Closed, and carrying
    /// none of the request — a ledger says which rule, never what was asked.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::TooFewRows { .. } => "too_few_rows",
            Self::Answered { .. } => "answered",
            Self::Latency { .. } => "latency",
            Self::Schema { .. } => SCHEMA,
            Self::NoLabels { .. } => "no_labels",
            Self::Labels { .. } => "labels",
            Self::Fallbacks { .. } => "fallbacks",
        }
    }
}

impl Stand {
    /// The word a row and a screen use for where a seat stands.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Applying => "applying",
        }
    }
}

impl Verdict {
    /// The word this verdict is named by. A rise and a fall reuse the words
    /// their transition rows carry, so a screen and a ledger cannot disagree.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Hold(_) => "hold",
            Self::Rise => ROSE,
            Self::Keep => "keep",
            Self::Fall(_) => FELL,
        }
    }

    /// The line it turned on, when it turned on one.
    #[must_use]
    pub const fn line(self) -> Option<Line> {
        match self {
            Self::Hold(line) | Self::Fall(line) => Some(line),
            Self::Rise | Self::Keep => None,
        }
    }
}

/// The settings key naming the file of labels a person wrote, under the Jev
/// block ([`crate::jev::door::JEV_SETTINGS_KEY`]) — `smart.jev.labels`.
///
/// §4 will not raise a seat without labels, so the product has to know where
/// they live; without this key the rule is a door with no handle, and the
/// screen's "twenty labels are needed" names nothing a person can act on.
pub const LABELS_KEY: &str = "labels";

/// The settings key that turns on label drafts (`smart.jev.labelDrafts`).
///
/// §4 will not raise a seat until labels say its judgment is at least as right
/// as the probe's, and the labels join the ledger by a task's fingerprint. But
/// the ledger carries no words — deliberately — and neither, it turns out, do
/// the transcripts: on this machine not one of the ten judged tasks could be
/// matched back to the text it was judged on (measured 2026-09-19). So the
/// rule had no road to it at all: a person could see that a seat wanted twenty
/// labels and had nothing to write them against.
///
/// With this on, each judged task is drafted where it is asked — the one place
/// the words are still in hand — for a person to fill the axes in. Off by
/// default and named for what it does, because it writes their own prompts to
/// a file, which the ledger beside it goes out of its way not to do.
pub const LABEL_DRAFTS_KEY: &str = "labelDrafts";

/// Whether the person asked for label drafts.
#[must_use]
pub fn label_drafts_wanted(root: &Value) -> bool {
    root.get(crate::jev::SMART_SETTINGS_KEY)
        .and_then(|smart| smart.get(crate::jev::door::JEV_SETTINGS_KEY))
        .and_then(|jev| jev.get(LABEL_DRAFTS_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The labels file a person named, if they named one.
#[must_use]
pub fn labels_path_in(root: &Value) -> Option<&str> {
    root.get(crate::jev::SMART_SETTINGS_KEY)?
        .get(crate::jev::door::JEV_SETTINGS_KEY)?
        .get(LABELS_KEY)?
        .as_str()
        .map(str::trim)
        .filter(|path| !path.is_empty())
}

/// Where a seat stands, read back from the transitions its ledger recorded.
///
/// No transition means it has never risen, which is what `auto` starts as.
#[must_use]
pub fn stand_from(rows: &[Value]) -> Stand {
    rows.iter()
        .rev()
        .find_map(|row| match TRANSITION.read(row).and_then(Value::as_str) {
            Some(ROSE) => Some(Stand::Applying),
            Some(FELL) => Some(Stand::Recording),
            _ => None,
        })
        .unwrap_or(Stand::Recording)
}

/// The row a rise or a fall appends. `None` for a verdict that changed
/// nothing — a ledger of "still recording" every twenty rows is a ledger
/// nobody can read.
#[must_use]
pub fn transition_row(now_ms: i64, verdict: Verdict, window: &Tally) -> Option<Value> {
    let (word, line) = match verdict {
        Verdict::Rise => (ROSE, None),
        Verdict::Fall(line) => (FELL, Some(line)),
        Verdict::Hold(_) | Verdict::Keep => return None,
    };
    let mut row = json!({
        AT.canonical: now_ms,
        (TRANSITION.canonical): word,
        "rows": window.rows,
        "answered": window.answered,
        "answeredLowerBoundPermille": window.answered_lower_bound().map(permille),
        "p95Ms": window.p95_ms,
    });
    if let Some(line) = line {
        row[ON_LINE] = Value::from(line.token());
    }
    Some(row)
}
