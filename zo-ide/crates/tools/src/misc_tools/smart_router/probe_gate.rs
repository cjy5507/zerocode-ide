//! Why the routing judgment did not run, written down where it can be read
//! tomorrow.
//!
//! Three gates in [`super::turn`] can send a turn back with the deterministic
//! verdict before anything reaches the wire, and each already attests its
//! reason to `telemetry::attest_declined`. That attestation is what
//! `assess_turn_probed`'s own comment says makes the question answerable —
//! *"a table showing `not_worth_it` on every turn says the cost gate is the
//! reason a probe-owned axis never armed, which is not a conclusion the
//! probe's own success/failure counters can reach."*
//!
//! But the attestation ledger is per process and in memory
//! (`telemetry::harness_attest_snapshot`), so it dies with the process that
//! held it. The conclusion it was built to enable cannot be reached after the
//! fact: on this machine the routing ledger held 25 rows and not one of them
//! said why the other turns asked nothing, and answering it took an afternoon
//! of reading source. One durable line answers it in a second.
//!
//! # What this writes, and what it does not
//!
//! A row is written only when the verdict **changes**. A log with one row per
//! turn would be 161 rows of "declined" for the 161 turns this workspace's
//! plan ledger holds, which buries the one fact worth having: the moment it
//! started, and why. So each row reads as *from here on, the gate answers
//! this* — and `admitted` is a verdict like the others, or a file of declines
//! could never show a recovery.
//!
//! # What it costs the turn
//!
//! Nothing, in the steady state. The last verdict is held in this process, so
//! an unchanged verdict touches no file at all; only a change writes, and the
//! write is one short line. This matters because the gates exist to keep a
//! round trip off the turn's first request — a diagnostic that put I/O back
//! on that path would be charging the turn for the privilege of being told it
//! was spared.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::shadow_ledger::{SHADOW_LEDGER_MAX_BYTES, append_shadow_row, shadow_ledger_path};

/// The ledger this writes, beside the judgments' own.
///
/// Its own file rather than a second shape inside `decision-shadow.jsonl`:
/// one file, one row shape, so a reader of either never has to skip lines it
/// cannot parse.
pub const PROBE_GATE_LEDGER: &str = "probe-gate.jsonl";

/// Why a turn asked nothing — the same words the gates already attest, named
/// once here so the ledger and the telemetry cannot drift into two
/// vocabularies for one decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeGate {
    /// The judgment ran. Recorded like any other verdict so a run of declines
    /// can be seen to end.
    Admitted,
    /// The complexity gate: this band is not one a verdict could move.
    NotWorthIt,
    /// The person's settings turn the classifier off.
    ClassifierOff,
    /// The settings could not be read — the one verdict nobody chose.
    SettingsUnavailable,
    /// Nothing this turn would read the verdict: no verify leg, and the
    /// settings arm no exec implementer for this main model.
    VerdictUnread,
}

impl ProbeGate {
    /// The word the telemetry ledger and this one both use.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::NotWorthIt => "not_worth_it",
            Self::ClassifierOff => "classifier_off",
            Self::SettingsUnavailable => "settings_unavailable",
            Self::VerdictUnread => "verdict_unread",
        }
    }

    /// What a person reading the row should do about it, in one sentence.
    /// A reason without a next step is a reason nobody acts on.
    #[must_use]
    pub const fn means(self) -> &'static str {
        match self {
            Self::Admitted => "the routing judgment is running",
            Self::NotWorthIt => {
                "this turn's band is already the one a verdict would choose, so nothing was asked"
            }
            Self::ClassifierOff => "smart.autoClassifier is off",
            Self::SettingsUnavailable => "the settings file could not be read",
            Self::VerdictUnread => {
                "nothing this turn would read the verdict: no verify leg is armed, and the \
                 settings arm no exec implementer for this main model"
            }
        }
    }
}

/// One change of the gate's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeGateRow {
    /// Unix milliseconds the answer changed.
    pub at: u64,
    /// The answer from here on.
    pub gate: ProbeGate,
    /// What it means, written into the row rather than left to a reader with
    /// the source open — the whole point is that this is readable without one.
    /// Owned so a row can be read back as easily as it is written; a row is
    /// only ever built when the answer changes, so the copy is not a cost
    /// anything pays per turn.
    pub means: String,
    /// The answer it replaced, absent on the first row a workspace writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<ProbeGate>,
}

/// The last verdict written for each workspace, so an unchanged one costs no
/// file.
///
/// Keyed by workspace rather than held as one value: a process that serves
/// two working directories would otherwise read one's answer as the other's
/// and call a real change "unchanged", which is the one way this ledger can
/// lose the fact it exists to keep. (The parallel tests found it first, for
/// the same reason.)
static LAST: Mutex<Option<HashMap<PathBuf, ProbeGate>>> = Mutex::new(None);

/// Record that the gate now answers `gate`, if that is news.
///
/// Silent when the verdict has not changed since this process last wrote one.
/// A poisoned memo writes rather than skips: a duplicate row is a reader's
/// mild annoyance, a missing transition is the whole question unanswered.
pub(super) fn note(cwd: &Path, gate: ProbeGate) {
    let mut held = match LAST.lock() {
        Ok(held) => held,
        Err(poisoned) => poisoned.into_inner(),
    };
    let seen = held.get_or_insert_with(HashMap::new);
    if seen.get(cwd) == Some(&gate) {
        return;
    }
    let was = seen.insert(cwd.to_path_buf(), gate);
    drop(held);
    let row = ProbeGateRow {
        at: super::decision_shadow::unix_millis(),
        gate,
        means: gate.means().to_string(),
        was,
    };
    let _ = append_shadow_row(
        &shadow_ledger_path(cwd, PROBE_GATE_LEDGER),
        &row,
        SHADOW_LEDGER_MAX_BYTES,
    );
}

/// Forget this process's memo — the seam a test holds so one test's verdict
/// is not another's "unchanged".
#[cfg(test)]
pub(super) fn forget(cwd: &Path) {
    let mut held = match LAST.lock() {
        Ok(held) => held,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(seen) = held.as_mut() {
        seen.remove(cwd);
    }
}

#[cfg(test)]
mod tests;
