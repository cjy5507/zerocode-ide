//! Whether a Jev seat asks at all, read the same way by every seat that
//! attests what it does.
//!
//! A seat's question is held out by an ablation, left unasked when its
//! setting cannot be read, and left unasked under a mode that asks nothing —
//! and the last two are attested under the seat's own feature, so a run that
//! asked nothing says why. Five seats each carried this reading word for
//! word. It is written once here; a seat names its feature and the setting
//! it reads.

use std::path::Path;

use zerocode_core::jev::JevMode;

/// What a seat attests, and writes in its row, when its setting could not be
/// read.
pub(super) const FAIL_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

/// The mode the seat behind `feature` is to be judged under in the project at
/// `cwd`, or `None` when it is not to be judged at all: an ablation holding
/// it out, an unreadable setting, or a mode that asks nothing.
pub(super) fn asking_mode(
    feature: telemetry::HarnessFeature,
    cwd: &Path,
    mode_from: impl FnOnce(&runtime::ConfigLoader) -> Option<JevMode>,
) -> Option<JevMode> {
    if telemetry::attest_ablated(feature) {
        return None;
    }
    let Some(mode) = mode_from(&runtime::ConfigLoader::default_for(cwd)) else {
        telemetry::attest_failed(feature, FAIL_SETTINGS_UNAVAILABLE);
        return None;
    };
    if !mode.asks() {
        telemetry::attest_declined(feature, mode.key());
        return None;
    }
    Some(mode)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use zerocode_core::jev::JevMode;

    use super::{asking_mode, FAIL_SETTINGS_UNAVAILABLE};

    /// A feature no seat of this crate attests under, so its counts move only
    /// for the checks below: the process's ledger is shared by every test.
    const FEATURE: telemetry::HarnessFeature = telemetry::HarnessFeature::ReasoningReplayCall;

    fn failed(reason: &str) -> u64 {
        telemetry::harness_attest_snapshot().attestation(FEATURE).failed.get(reason).copied().unwrap_or(0)
    }

    fn declined(reason: &str) -> u64 {
        telemetry::harness_attest_snapshot().attestation(FEATURE).declined.get(reason).copied().unwrap_or(0)
    }

    /// Five seats read whether they ask through this one function, and none
    /// of them had a test for a setting that cannot be read: the seat asks
    /// nothing, and says so as a failure under the feature it was given.
    #[test]
    fn a_setting_that_cannot_be_read_asks_nothing_and_is_attested_as_a_failure() {
        let before = failed(FAIL_SETTINGS_UNAVAILABLE);
        assert_eq!(asking_mode(FEATURE, Path::new("."), |_| None), None);
        assert_eq!(failed(FAIL_SETTINGS_UNAVAILABLE), before + 1);
    }

    /// A mode that asks nothing is a decision, not a failure: it is attested
    /// as declined, under the word the setting holds.
    #[test]
    fn a_mode_that_asks_nothing_is_attested_as_declined_under_its_own_word() {
        let before = declined(JevMode::Off.key());
        assert_eq!(asking_mode(FEATURE, Path::new("."), |_| Some(JevMode::Off)), None);
        assert_eq!(declined(JevMode::Off.key()), before + 1);
    }

    /// Every mode that asks is handed back as it was read, and nothing is
    /// attested for it: the seat goes on to ask.
    #[test]
    fn a_mode_that_asks_is_handed_back_as_it_was_read() {
        for mode in [JevMode::Shadow, JevMode::On, JevMode::Auto] {
            assert_eq!(asking_mode(FEATURE, Path::new("."), |_| Some(mode)), Some(mode));
            assert_eq!(declined(mode.key()), 0, "{mode:?} was attested as declined");
        }
    }
}
