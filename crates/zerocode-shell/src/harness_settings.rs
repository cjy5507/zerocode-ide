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

/// The gate's half and the ledger's half, as stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct HarnessSettings {
    pub(crate) gate: Settings,
    pub(crate) launches: Limits,
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
        Ok(Self {
            gate: Settings::parse(&value.get("gate").cloned().unwrap_or_default()),
            launches,
        })
    }
}

#[cfg(test)]
mod tests;
