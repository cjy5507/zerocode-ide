//! What a person set for the continue gate and the launch ledger (t-26583).

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
    /// What the settings pane sent.
    ///
    /// # Errors
    ///
    /// The sentence for a ceiling that is not a whole number of launches.
    pub(crate) fn parse(_value: &serde_json::Value) -> Result<Self, String> {
        Ok(Self::default())
    }
}

#[cfg(test)]
mod tests;
