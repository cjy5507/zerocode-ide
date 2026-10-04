//! What a person set for the continue gate and the launch ledger (t-26583).
//!
//! One small record in the settings document, two halves: how the gate may act
//! and what a task and a day may cost ([`zerocode_core::continue_gate::Settings`]),
//! and how many background launches of an agent's CLI may run at once, in an hour
//! and in a day ([`zerocode_core::launch_budget::Limits`]). Both ship with a
//! default a person who never opens the settings gets — the gate tells and ends
//! nobody, the ceilings are far above an honest day — so nothing here has to be
//! set for the window to be safe, and everything can be.

use serde::{Deserialize, Serialize};
use zerocode_core::continue_gate::Settings;
use zerocode_core::launch_budget::Limits;

/// What the ledger may tell on its own (t-34501): the notices about work that landed late, sent to the
/// run's coordinator, and the word to a live worker whose branch ran far behind the main branch or would
/// conflict with it. On by default; the notices only inform — nothing is merged, deleted or pushed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Alerts {
    pub(crate) landing: bool,
}

impl Default for Alerts {
    fn default() -> Self {
        Self { landing: true }
    }
}

/// The gate's half, the ledger's half and its alerts, as stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct HarnessSettings {
    pub(crate) gate: Settings,
    pub(crate) launches: Limits,
    pub(crate) alerts: Alerts,
}

impl HarnessSettings {
    /// What the settings pane sent. The gate's half is read field by field — a
    /// bad budget is none and does not take the mode with it
    /// ([`Settings::parse`]); the ceilings are read whole, and a number that is not
    /// a count of launches refuses the save, so a typo never silently lifts a
    /// ceiling.
    ///
    /// # Errors
    ///
    /// The sentence for a ceiling that is not a whole number of launches.
    pub(crate) fn parse(value: &serde_json::Value) -> Result<Self, String> {
        let launches = match value.get("launches") {
            Some(held) => serde_json::from_value(held.clone()).map_err(|_| {
                "실행 한도는 0 이상의 정수나 빈칸(한도 없음)이어야 합니다".to_string()
            })?,
            None => Limits::default(),
        };
        // A bad word turns nothing off: only a plain `false` does.
        let alerts = Alerts {
            landing: value
                .get("alerts")
                .and_then(|held| held.get("landing"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        };
        Ok(Self {
            gate: Settings::parse(&value.get("gate").cloned().unwrap_or_default()),
            launches,
            alerts,
        })
    }
}

#[cfg(test)]
mod tests;
