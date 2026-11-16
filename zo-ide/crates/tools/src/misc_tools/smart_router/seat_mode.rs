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
