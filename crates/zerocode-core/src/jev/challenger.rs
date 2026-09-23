//! Who may try a model nobody has evidence for, how often, what the judge is
//! shown, and how the answer is scored — the pure half of the challenger arm
//! ([`crate::jev::CHALLENGER`]).
//!
//! Every number here is the seat row's or the user's decision relayed with
//! it: one attempt in [`crate::jev::CHALLENGER_ONE_IN`], a day's challenger
//! spend under [`crate::jev::CHALLENGER_DAY_SPEND_PERMILLE`] of the day's
//! own, and four kinds of route that never draw at all.
//!
//! It is IO-free and clock-free on purpose: the draw and the blind have to
//! answer the same way for the same attempt however often they are asked, or
//! a retry would re-roll, a replay would show the judge a different order,
//! and a dashboard would disagree with what actually ran. The caller hands in
//! what it knows — the attempt, the day's spend, the two designs, the
//! judge's answer, the verification loop's receipt — and this module only
//! decides and spells.
//!
//! Nothing here sends. The wire, the ledger writer and the price table live
//! in the programs that own them; what this module fixes is the contract
//! between them — the columns a challenger row carries, the shape of the
//! question, and the one reading of a row that the judge, the counter and a
//! replay all take.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::jev::choice::{self, ChoiceRefusal};
use crate::jev::summary::{
    AGREED, AT, BASELINE_AGREED, LABEL, LedgerKey, WILSON_Z_95, wilson_lower,
};
use crate::jev::{
    A_WINDOW_OF_COMPARISONS, CHALLENGER_DAY_SPEND_PERMILLE, CHALLENGER_DESIGN_CAP,
    CHALLENGER_ONE_IN,
};

/// What an attempt is, as far as the draw is concerned.
///
/// A struct rather than five arguments, because every field is a `bool` or a
/// role and a caller that swapped two of them would compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt<'a> {
    /// A name that is this attempt's and no other's — the dispatch id, or
    /// the run's own attempt key. The draw and the blind are taken over it.
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
    /// The day's challenger spend, with what is reserved and what this
    /// attempt would cost, has reached its share.
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

/// The first eight bytes of SHA-256 over `domain`, a NUL, and `key`, as a
/// number — the one digest every decision taken over an attempt's key is
/// read from.
///
/// SHA-256 over a domain-separated key rather than a random number: the same
/// attempt must decide the same way when a dashboard re-reads it, when a
/// replay harness re-derives it, and when a person asks why this attempt and
/// not that one. `DefaultHasher` is documented as unstable across compiler
/// releases and these decisions are written to a ledger, so a newer build
/// would re-roll every historical row. The domain says what the digest is
/// OF, so two decisions over the same key are two independent draws and
/// neither can collide with some other digest this product takes over the
/// same bytes.
fn digest_head(domain: &str, key: &str) -> u64 {
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update([0u8]);
    digest.update(key.as_bytes());
    let bytes = digest.finalize();
    u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

/// Whether `key` is the one attempt in [`CHALLENGER_ONE_IN`] that draws.
#[must_use]
pub fn draws(key: &str) -> bool {
    digest_head("zerocode.jev.challenger.draw.v1", key).is_multiple_of(CHALLENGER_ONE_IN)
}

/// What a day has spent, in the price table's own units (micro-dollars,
/// whatever the table says a dollar is — every field is the same unit, and
/// the line is a ratio, so the number itself never has to agree with the
/// table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DaySpend {
    /// What the day's challenger rows say their requests cost.
    pub challenger_micros: u64,
    /// What attempts drawn today and still running were reserved for — spend
    /// no row carries yet. Without it five attempts drawn in one minute would
    /// each fit a share only one of them could: the arm's ceiling would be
    /// the ceiling times however many draws land before the first row is
    /// written.
    pub reserved_micros: u64,
    /// The day's whole model spend, every model's, as the usage ledger sums
    /// it.
    pub day_micros: u64,
}

/// Whether a day that has spent `day` may spend `expected_micros` more on
/// the arm.
///
/// Read as a ratio, so a quiet day buys a small experiment and a busy one a
/// larger. What is counted against the share is everything the arm has
/// spent, everything it has reserved, and what this attempt would cost —
/// so a day that has spent nothing has no share yet, and the first challenge
/// of a day waits until the day's own work has bought it. (The first cut of
/// this line let an empty day through unconditionally, which was a ceiling
/// with no number under it: the first draw of every morning was free
/// whatever it cost.)
#[must_use]
pub fn within_day_budget(day: &DaySpend, expected_micros: u64) -> bool {
    let would_be = u128::from(day.challenger_micros)
        + u128::from(day.reserved_micros)
        + u128::from(expected_micros);
    would_be * 1_000 <= u128::from(day.day_micros) * u128::from(CHALLENGER_DAY_SPEND_PERMILLE)
}

/// The whole decision, in the order §2 asks it: what the attempt is, then
/// the draw, then what the day has left for what this attempt would cost.
///
/// The cheap refusals come first so a held-back role never costs a digest,
/// and the budget last so a day's spend is only read for an attempt that
/// would otherwise challenge.
///
/// # Errors
/// The first line the attempt does not clear.
pub fn challenges(attempt: &Attempt<'_>, day: &DaySpend, expected_micros: u64) -> Result<(), Held> {
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
    if !within_day_budget(day, expected_micros) {
        return Err(Held::DayBudget);
    }
    Ok(())
}

/// Whose a design is — the one fact the judge is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// The model the attempt acts on.
    Incumbent,
    /// The model nobody has evidence for.
    Challenger,
}

impl Side {
    /// The word a row writes for this side.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Incumbent => "incumbent",
            Self::Challenger => "challenger",
        }
    }
}

/// The order the two designs are put to the judge in.
///
/// Drawn from the attempt's key, so a replay shows the judge the same order
/// the attempt did — and from a domain of its own, so which design comes
/// first is independent of whether the attempt drew at all. Over many
/// attempts each side is first half the time, which is what keeps a judge
/// with a taste for first answers from scoring one side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blind {
    challenger_first: bool,
}

impl Blind {
    /// The order for the attempt named `key`.
    #[must_use]
    pub fn over(key: &str) -> Self {
        Self {
            challenger_first: digest_head("zerocode.jev.challenger.blind.v1", key)
                .is_multiple_of(2),
        }
    }

    /// Whose design the judge sees first.
    #[must_use]
    pub const fn first(self) -> Side {
        if self.challenger_first {
            Side::Challenger
        } else {
            Side::Incumbent
        }
    }

    /// Whose design the judge sees second.
    #[must_use]
    pub const fn second(self) -> Side {
        if self.challenger_first {
            Side::Incumbent
        } else {
            Side::Challenger
        }
    }

    /// The word a row writes for this order, so a reader can unblind the
    /// row without the key.
    #[must_use]
    pub const fn token(self) -> &'static str {
        if self.challenger_first {
            "challenger_first"
        } else {
            "incumbent_first"
        }
    }

    /// The side a position's word names, `None` for the word that names
    /// neither.
    fn side_of(self, option: &str) -> Option<Side> {
        if option == OPTIONS[0] {
            Some(self.first())
        } else if option == OPTIONS[1] {
            Some(self.second())
        } else {
            None
        }
    }
}

/// The two designs, by whose they are. The judge is shown them in the
/// [`Blind`]'s order, under no name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Designs<'a> {
    pub incumbent: &'a str,
    pub challenger: &'a str,
}

/// The words the comparison offers: the design shown first, the one shown
/// second, or neither. Positions and not names, because names are what the
/// blind withholds.
pub const OPTIONS: [&str; 3] = ["first", "second", "neither"];

/// The question's name in the request and the answer.
const QUESTION: &str = "preferred";

/// What the judge is asked. A plan, not a patch, and the plainer path to
/// verifying it — the same yardstick the verification loop applies
/// afterwards, so the label the receipt writes is a label of the same thing.
const INSTRUCTIONS: &str = "Two designs answer the same request. Name the one a careful \
engineer would act on: the one that answers what was asked, makes the fewer unforced \
assumptions, and leaves the plainer path to verifying it. Name neither when nothing \
separates them or both miss the request.";

/// The keys of the request's `state`, in the order the seat's `sends`
/// name them (`/state/task`, `/state/designs`).
pub const STATE_KEYS: [&str; 2] = ["task", "designs"];

/// The key each design's text sits under (`/state/designs/*/body`).
pub const DESIGN_BODY_KEY: &str = "body";

/// One comparison, ready for the wire: the state the door clears, the
/// question, and the order it was put in.
#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonAsk {
    /// What the seat's `sends` describe, under the pointers they name.
    pub state: Value,
    /// The closed choice, as [`choice::asked`] spells it.
    pub questions: Value,
    blind: Blind,
}

/// The comparison for the attempt named `key`: the task in the person's
/// words and the two designs in the blind's order, under no name.
///
/// Uncut: the door cuts each piece at the cap the seat's row names
/// ([`crate::jev::CHALLENGER`]'s `sends`) on its way out, as it does for
/// every seat, and a second cut here would be a second number.
#[must_use]
pub fn ask(key: &str, task: &str, designs: &Designs<'_>) -> ComparisonAsk {
    let blind = Blind::over(key);
    let body_of = |side: Side| match side {
        Side::Incumbent => designs.incumbent,
        Side::Challenger => designs.challenger,
    };
    let shown: [Value; CHALLENGER_DESIGN_CAP] =
        [blind.first(), blind.second()].map(|side| json!({ DESIGN_BODY_KEY: body_of(side) }));
    let state = json!({
        STATE_KEYS[0]: task,
        STATE_KEYS[1]: shown,
    });
    let criteria = Map::from_iter([
        (
            OPTIONS[0].to_string(),
            Value::from("the design shown first is the one to act on"),
        ),
        (
            OPTIONS[1].to_string(),
            Value::from("the design shown second is the one to act on"),
        ),
        (
            OPTIONS[2].to_string(),
            Value::from("nothing separates them, or both miss the request"),
        ),
    ]);
    ComparisonAsk {
        state,
        questions: choice::asked(QUESTION, INSTRUCTIONS, criteria),
        blind,
    }
}

/// What the judge preferred, unblinded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preferred {
    Incumbent,
    Challenger,
    /// Nothing separated them, or both missed.
    Neither,
}

impl Preferred {
    /// The word a row writes.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Incumbent => Side::Incumbent.token(),
            Self::Challenger => Side::Challenger.token(),
            Self::Neither => "neither",
        }
    }

    /// The preference a row's word names, if it names one.
    #[must_use]
    pub fn from_token(word: &str) -> Option<Self> {
        [Self::Incumbent, Self::Challenger, Self::Neither]
            .into_iter()
            .find(|preferred| preferred.token() == word)
    }

    const fn of(side: Option<Side>) -> Self {
        match side {
            Some(Side::Incumbent) => Self::Incumbent,
            Some(Side::Challenger) => Self::Challenger,
            None => Self::Neither,
        }
    }
}

/// A read answer: what the judge preferred, and how sure it was — the shape
/// of its distribution, not a rate of being right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Answer {
    pub preferred: Preferred,
    pub confidence: f64,
}

impl ComparisonAsk {
    /// The order this comparison was put in — what the row records so the
    /// answer can be read back without the key.
    #[must_use]
    pub const fn blind(&self) -> Blind {
        self.blind
    }

    /// What the endpoint's `answers` map says, unblinded. One broken rule
    /// discards the answer whole.
    ///
    /// # Errors
    ///
    /// [`ChoiceRefusal`] names which rule the answer broke.
    pub fn read(&self, answers: &Value) -> Result<Answer, ChoiceRefusal> {
        let offered: BTreeSet<String> = OPTIONS.iter().map(ToString::to_string).collect();
        let choice = choice::read(answers, QUESTION, &offered)?;
        Ok(Answer {
            preferred: Preferred::of(self.blind.side_of(&choice.chosen)),
            confidence: choice.confidence,
        })
    }
}

/// What the verification loop said of the design that ran — the
/// incumbent's, always. A finished attempt is not a receipt: the caller
/// that has only a completion has `None`, never `Passed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receipt {
    Passed,
    Failed,
}

impl Receipt {
    /// The word a label row writes.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }
}

/// The quality label of one comparison, by the user's rule (2026-09-22):
/// the verification loop's receipt where the attempt has one, the
/// anonymized comparison where it does not, and completion alone never a
/// win.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    /// Whether the challenger won this comparison — the mark its standing
    /// for the role is counted on ([`standing`]).
    pub won: bool,
    /// Whether the judge named what the receipt vindicated — the seat's own
    /// mark ([`crate::jev::AgreementKind::Comparison`]), which says whether
    /// the seat is reading quality. `None` where the receipt cannot say: it
    /// grades the one design that ran, so a passing incumbent says nothing
    /// against a challenger the judge preferred, and a judge that named
    /// neither is not contradicted by anything.
    pub agreed: Option<bool>,
}

/// The label for `preferred` given `receipt`.
///
/// A receipt outranks the comparison. It grades the incumbent's design, the
/// one that ran, so the cells are: an incumbent that failed vindicates a
/// judge who preferred the challenger and contradicts one who preferred the
/// incumbent; an incumbent that passed vindicates a judge who preferred it
/// and says nothing about one who preferred the other. The challenger wins
/// only where the receipt is against the incumbent and the judge was for
/// the challenger — or, with no receipt at all, where the judge was.
#[must_use]
pub const fn quality(receipt: Option<Receipt>, preferred: Preferred) -> Quality {
    match (receipt, preferred) {
        (None, Preferred::Challenger) => Quality {
            won: true,
            agreed: None,
        },
        (None, Preferred::Incumbent | Preferred::Neither) => Quality {
            won: false,
            agreed: None,
        },
        (Some(Receipt::Failed), Preferred::Challenger) => Quality {
            won: true,
            agreed: Some(true),
        },
        (Some(Receipt::Failed), Preferred::Incumbent) => Quality {
            won: false,
            agreed: Some(false),
        },
        (Some(Receipt::Passed), Preferred::Incumbent) => Quality {
            won: false,
            agreed: Some(true),
        },
        (Some(Receipt::Passed), Preferred::Challenger)
        | (Some(Receipt::Passed | Receipt::Failed), Preferred::Neither) => Quality {
            won: false,
            agreed: None,
        },
    }
}

/// The route role the attempt ran under.
pub const ROLE: LedgerKey = LedgerKey {
    canonical: "role",
    also: &[],
};
/// The attempt's own name — what a label row names it by
/// ([`crate::jev::summary::LABEL`]).
pub const ATTEMPT: LedgerKey = LedgerKey {
    canonical: "attempt",
    also: &[],
};
/// The model the attempt acted on. Spelled beside the summons' `workerModel`
/// and not as `model`, which every Jev row keeps for the version that
/// answered the question ([`crate::jev::summary::MODEL`]).
pub const INCUMBENT_MODEL: LedgerKey = LedgerKey {
    canonical: "incumbentModel",
    also: &[],
};
/// The model that was asked for the other design.
pub const CHALLENGER_MODEL: LedgerKey = LedgerKey {
    canonical: "challengerModel",
    also: &[],
};
/// What the challenger's design was expected to cost when it was drawn —
/// what the day's share was charged before the row could carry a price.
pub const EXPECTED_MICROS: LedgerKey = LedgerKey {
    canonical: "expectedMicros",
    also: &[],
};
/// The order the judge saw the designs in ([`Blind::token`]).
pub const BLIND: LedgerKey = LedgerKey {
    canonical: "blind",
    also: &[],
};
/// What the judge preferred, unblinded ([`Preferred::token`]).
pub const PREFERRED: LedgerKey = LedgerKey {
    canonical: "preferred",
    also: &[],
};
/// What the verification loop said of the incumbent's design
/// ([`Receipt::token`]), on the label row that carried it.
pub const VERIFIED: LedgerKey = LedgerKey {
    canonical: "verified",
    also: &[],
};
/// Whether the challenger won ([`Quality::won`]). On the request row by the
/// comparison alone; a label row for the same attempt writes it again by
/// the receipt, and the later word is the one that counts.
pub const WON: LedgerKey = LedgerKey {
    canonical: "won",
    also: &[],
};
/// Why an attempt was held ([`Held::token`]), on a row that asked nothing.
pub const HELD: LedgerKey = LedgerKey {
    canonical: "held",
    also: &[],
};

/// Every key this module writes beyond the wire's own
/// ([`crate::jev::summary::LEDGER_KEYS`]), so a contract can walk them.
pub const CHALLENGER_KEYS: &[LedgerKey] = &[
    ROLE,
    ATTEMPT,
    INCUMBENT_MODEL,
    CHALLENGER_MODEL,
    EXPECTED_MICROS,
    BLIND,
    PREFERRED,
    VERIFIED,
    WON,
    HELD,
];

/// One comparison as its request row records it, beyond what the wire
/// writes on every row (`at`, `outcome`, `elapsedMs`, `model`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comparison<'a> {
    pub attempt: &'a str,
    pub role: &'a str,
    pub incumbent_model: &'a str,
    pub challenger_model: &'a str,
    pub expected_micros: u64,
    pub blind: Blind,
    /// What the judge said, `None` on a row whose answer did not arrive or
    /// did not pass its checks — the wire's `outcome` says which.
    pub preferred: Option<Preferred>,
}

impl Comparison<'_> {
    /// The row's columns, for the writer to lay beside the wire's own. A row
    /// with an answer carries the comparison's own word on the challenger
    /// ([`WON`]), which a receipt's label row later outranks.
    #[must_use]
    pub fn columns(&self) -> Map<String, Value> {
        let mut columns = Map::from_iter([
            (ATTEMPT.canonical.to_string(), Value::from(self.attempt)),
            (ROLE.canonical.to_string(), Value::from(self.role)),
            (
                INCUMBENT_MODEL.canonical.to_string(),
                Value::from(self.incumbent_model),
            ),
            (
                CHALLENGER_MODEL.canonical.to_string(),
                Value::from(self.challenger_model),
            ),
            (
                EXPECTED_MICROS.canonical.to_string(),
                Value::from(self.expected_micros),
            ),
            (BLIND.canonical.to_string(), Value::from(self.blind.token())),
        ]);
        if let Some(preferred) = self.preferred {
            columns.insert(
                PREFERRED.canonical.to_string(),
                Value::from(preferred.token()),
            );
            columns.insert(
                WON.canonical.to_string(),
                Value::from(quality(None, preferred).won),
            );
        }
        columns
    }
}

/// The row a receipt writes for the attempt it grades, once the
/// verification loop has spoken: named after the request row's attempt
/// ([`crate::jev::summary::LABEL`]), carrying the receipt, the challenger's
/// word by it, and — where the receipt can say — whether the judge named
/// what it vindicated ([`crate::jev::summary::AGREED`]).
#[must_use]
pub fn label_row(attempt: &str, receipt: Receipt, preferred: Preferred, at_ms: i64) -> Value {
    let graded = quality(Some(receipt), preferred);
    let mut row = Map::from_iter([
        (AT.canonical.to_string(), Value::from(at_ms)),
        (LABEL.canonical.to_string(), Value::from(attempt)),
        (VERIFIED.canonical.to_string(), Value::from(receipt.token())),
        (WON.canonical.to_string(), Value::from(graded.won)),
    ]);
    if let Some(agreed) = graded.agreed {
        row.insert(AGREED.canonical.to_string(), Value::from(agreed));
        // The seat's baseline, today's rule: the incumbent's design, which
        // the attempt acts on anyway — marked on the same receipt (t-6342).
        if let Some(incumbent) = quality(Some(receipt), Preferred::Incumbent).agreed {
            row.insert(
                BASELINE_AGREED.canonical.to_string(),
                Value::from(incumbent),
            );
        }
    }
    Value::Object(row)
}

/// The row an attempt the arm held writes: no `outcome`, so it is no
/// request and sits outside every window and share
/// ([`crate::jev::summary::asked_something`]); just the word, for a reader
/// asking why. Not for [`Held::NotDrawn`], which is four rows in five and
/// re-derived from the key by anyone who wants it.
#[must_use]
pub fn held_row(attempt: &Attempt<'_>, held: Held, at_ms: i64) -> Value {
    json!({
        AT.canonical: at_ms,
        ATTEMPT.canonical: attempt.key,
        ROLE.canonical: attempt.role,
        HELD.canonical: held.token(),
    })
}

/// The challenger's record for one role: how many comparisons said
/// anything, and how many it won.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Standing {
    pub compared: usize,
    pub won: usize,
}

impl Standing {
    /// The 95% Wilson lower bound on the won share, `None` when nothing was
    /// compared.
    #[must_use]
    pub fn lower_bound(&self) -> Option<f64> {
        (self.compared > 0).then(|| wilson_lower(self.won, self.compared, WILSON_Z_95))
    }

    /// Whether this record passes the incumbent's own rate for the role —
    /// the line a role's model moves on, and the line it moves back on
    /// ([`crate::jev::CHALLENGER`]). A record thinner than a window of
    /// comparisons passes nothing, however good.
    #[must_use]
    pub fn passes(&self, incumbent_rate: f64) -> bool {
        self.compared >= A_WINDOW_OF_COMPARISONS
            && self
                .lower_bound()
                .is_some_and(|bound| bound > incumbent_rate)
    }
}

/// The challenger `challenger_model`'s standing for `role`, read from a
/// ledger: one word per attempt, the latest — a request row's comparison
/// until a label row's receipt outranks it — over the pair's own rows and
/// the labels that name them.
#[must_use]
pub fn standing(rows: &[Value], role: &str, challenger_model: &str) -> Standing {
    let of_pair = |row: &Value| {
        ROLE.read(row).and_then(Value::as_str) == Some(role)
            && CHALLENGER_MODEL.read(row).and_then(Value::as_str) == Some(challenger_model)
    };
    let mut word_of: BTreeMap<&str, bool> = BTreeMap::new();
    for row in rows {
        let Some(won) = WON.read(row).and_then(Value::as_bool) else {
            continue;
        };
        if of_pair(row) {
            if let Some(attempt) = ATTEMPT.read(row).and_then(Value::as_str) {
                word_of.insert(attempt, won);
            }
        } else if let Some(attempt) = LABEL.read(row).and_then(Value::as_str)
            && word_of.contains_key(attempt)
        {
            word_of.insert(attempt, won);
        }
    }
    Standing {
        compared: word_of.len(),
        won: word_of.values().filter(|won| **won).count(),
    }
}

#[cfg(test)]
mod tests;
