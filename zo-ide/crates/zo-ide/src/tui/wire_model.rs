//! The footer's wire-model badge.
//!
//! The runtime announces which model every request goes out on and why
//! (`RenderBlock::WireModel`). While that is not the session's configured
//! model — a Fable safety-classifier refusal retried on the Opus head, a quota
//! fallback on another provider, a deep-gate leg on its own model — the footer
//! shows the model actually on the wire and one warn-tinted word for the
//! reason. A request back on the session model clears it. The transcript's
//! `System` row already explains each swap; this is the standing word.

use runtime::message_stream::{WireModel, WireModelSource};

/// What the footer's model field shows while a swap is on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    /// The model id the request goes out on.
    pub model: String,
    /// The footer's word for why (see [`label`]).
    pub label: &'static str,
}

/// The footer's word for each swap — one table, so a new source cannot ship
/// without a word. `None` for the session model: nothing to flag.
#[must_use]
pub const fn label(source: WireModelSource) -> Option<&'static str> {
    match source {
        WireModelSource::Session => None,
        WireModelSource::RefusalFallback => Some("safety fallback"),
        WireModelSource::RefusalCooldown => Some("safety cooldown"),
        WireModelSource::QuotaFallback => Some("quota fallback"),
        WireModelSource::OverloadDemotion => Some("overload fallback"),
        WireModelSource::Escalation => Some("escalated"),
        WireModelSource::DeepPlan => Some("deep plan"),
        WireModelSource::DeepVerify => Some("deep verify"),
        WireModelSource::ExecImplementer => Some("implementer"),
    }
}

/// The badge a wire-model announcement leaves on the footer: `None` when the
/// request is back on the session model, which clears any standing badge.
#[must_use]
pub fn badge(wire: &WireModel) -> Option<Badge> {
    label(wire.source).map(|label| Badge {
        model: wire.model.clone(),
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::{badge, label, Badge};
    use runtime::message_stream::{WireModel, WireModelSource};

    /// Every swap has a word and the session model has none — the table is
    /// what keeps a new `WireModelSource` from reaching the footer nameless.
    #[test]
    fn every_swap_has_a_word_and_the_session_model_has_none() {
        for source in WireModelSource::ALL {
            let word = label(source);
            if source == WireModelSource::Session {
                assert_eq!(word, None);
            } else {
                let word = word.unwrap_or_else(|| panic!("{source:?} has no footer word"));
                assert!(
                    !word.is_empty() && word.len() <= 20,
                    "{source:?}: a footer word is short: {word:?}"
                );
            }
        }
    }

    #[test]
    fn a_swap_becomes_a_badge_and_the_session_model_clears_it() {
        let swap = WireModel {
            model: "claude-opus-5".to_string(),
            source: WireModelSource::RefusalFallback,
        };
        assert_eq!(
            badge(&swap),
            Some(Badge {
                model: "claude-opus-5".to_string(),
                label: "safety fallback",
            })
        );
        let home = WireModel {
            model: "claude-fable-5-1".to_string(),
            source: WireModelSource::Session,
        };
        assert_eq!(badge(&home), None);
    }
}
