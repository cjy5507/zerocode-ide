//! Holding a system-prompt section out of an experiment arm.
//!
//! zo already has an ablation axis for harness *features*
//! (`telemetry::harness_attest`, `ZO_ABLATE`): the harness chose to run a
//! policy, so an experiment can measure the arm where it chose not to. Prompt
//! sections are the same kind of thing — policy the harness put in front of the
//! model — but they are **state, not events**, so they cannot be counted as
//! `fired`/`leaked` and do not belong in that enum. They get their own axis
//! with the same discipline.
//!
//! Why this exists: the fixed prefix is 18,186 provider tokens against Claude
//! Code's 16,204, and three measurements have now shown the gap cannot be
//! closed by cleanup — cross-section duplication is ~163 tokens, the big tools
//! are pinned on the wire by a stated contract, and eleven of the twelve static
//! sections turn a test red when removed. What is left is a question no static
//! measurement can answer: **does dropping this policy make the agent worse?**
//! Answering it needs an arm to compare against, and this is that arm.

use std::collections::BTreeSet;

/// Environment variable naming the sections held out of this process, as a
/// comma-separated list of [`section_key`] values (or `all`).
pub const PROMPT_ABLATION_ENV: &str = "ZO_ABLATE_PROMPT";

/// Sections an experiment may NOT hold out.
///
/// Same rule the feature axis uses: a holdout is only meaningful when the arm
/// without it is *different*, not *broken*. Removing the care-with-actions
/// policy produces an arm that may take a destructive action it would otherwise
/// have paused on, and removing the trust label produces one that may obey text
/// it read out of a file. Neither is a measurement; both are a hazard wearing
/// an experiment's clothes.
const NOT_ABLATABLE: &[&str] = &["executing_actions_with_care", "context_trust_label_v1"];

/// A heading (`"# Doing tasks"`) to its stable ablation key (`"doing_tasks"`).
///
/// Derived from the shipped heading rather than written out, so the vocabulary
/// an experiment types cannot drift from the sections that actually exist.
///
/// Idempotent: a key fed back in returns itself, so a spec may be typed either
/// way. Dropping `_` instead of keeping it as a separator was the first bug
/// here, and it did not merely mangle a name — it made the `NOT_ABLATABLE`
/// comparison miss, so a safety section became holdable.
#[must_use]
pub fn section_key(heading: &str) -> String {
    heading
        .trim_start_matches('#')
        .trim()
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if ch.is_whitespace() || ch == '-' || ch == '_' {
                Some('_')
            } else {
                None
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// A malformed holdout spec.
///
/// A hard error rather than a skipped token, for the reason the feature axis
/// gives: an experiment whose treatment silently failed to apply produces a
/// confident measurement of nothing, which is worse than no measurement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAblationError {
    /// The token that could not be honored.
    pub token: String,
    /// True when the token names a real section that may not be held out.
    pub not_ablatable: bool,
}

impl std::fmt::Display for PromptAblationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.not_ablatable {
            write!(
                f,
                "{PROMPT_ABLATION_ENV}: `{}` may not be held out — the arm without it is \
                 unsafe, not merely different",
                self.token
            )
        } else {
            write!(
                f,
                "{PROMPT_ABLATION_ENV}: `{}` is not a prompt section",
                self.token
            )
        }
    }
}

impl std::error::Error for PromptAblationError {}

/// The sections suppressed in this process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptAblation {
    keys: BTreeSet<String>,
    all: bool,
}

impl PromptAblation {
    /// Nothing held out — the ordinary production arm.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Read the spec from the environment. An absent or empty variable is the
    /// production arm; a malformed one is an error the caller must surface.
    pub fn from_env() -> Result<Self, PromptAblationError> {
        match std::env::var(PROMPT_ABLATION_ENV) {
            Ok(spec) if !spec.trim().is_empty() => Self::parse(&spec),
            _ => Ok(Self::none()),
        }
    }

    /// Parse a comma-separated spec. Whitespace is ignored, empty tokens are
    /// skipped (a trailing comma is harmless), `all` holds out every ablatable
    /// section, and anything else must name a real, ablatable one.
    pub fn parse(spec: &str) -> Result<Self, PromptAblationError> {
        let mut keys = BTreeSet::new();
        let mut all = false;
        for token in spec.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            if token.eq_ignore_ascii_case("all") {
                all = true;
                continue;
            }
            let key = section_key(token);
            if NOT_ABLATABLE.contains(&key.as_str()) {
                return Err(PromptAblationError {
                    token: token.to_string(),
                    not_ablatable: true,
                });
            }
            keys.insert(key);
        }
        Ok(Self { keys, all })
    }

    /// Whether the section under `heading` stays out of this arm.
    ///
    /// `all` never reaches [`NOT_ABLATABLE`] — the same reason the feature axis
    /// filters them out of its own `all`: a blanket holdout must not quietly
    /// disarm a safety policy nobody named.
    #[must_use]
    pub fn suppresses(&self, heading: &str) -> bool {
        let key = section_key(heading);
        if NOT_ABLATABLE.contains(&key.as_str()) {
            return false;
        }
        self.all || self.keys.contains(&key)
    }

    /// Whether this arm holds anything out at all.
    #[must_use]
    pub fn is_production_arm(&self) -> bool {
        !self.all && self.keys.is_empty()
    }

    /// The keys named, for stamping the arm into a run's evidence.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        if self.all {
            return vec!["all".to_string()];
        }
        self.keys.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{section_key, PromptAblation, NOT_ABLATABLE};

    #[test]
    fn a_key_comes_from_the_heading_that_ships() {
        assert_eq!(section_key("# Doing tasks"), "doing_tasks");
        assert_eq!(
            section_key("# Delegation and workflow routing"),
            "delegation_and_workflow_routing"
        );
        assert_eq!(section_key("# Context Trust Label v1"), "context_trust_label_v1");
        // Already a key: idempotent, so a spec may be typed either way.
        assert_eq!(section_key("doing_tasks"), "doing_tasks");
    }

    /// A treatment that silently failed to apply is worse than no measurement,
    /// so an unknown or forbidden token stops the run instead of being skipped.
    #[test]
    fn a_spec_that_cannot_be_honored_is_an_error() {
        let unknown = PromptAblation::parse("no_such_section").expect("parses");
        assert!(
            !unknown.suppresses("# Doing tasks"),
            "an unrelated key must not suppress a real section"
        );

        for forbidden in NOT_ABLATABLE {
            let error = PromptAblation::parse(forbidden).expect_err("must refuse");
            assert!(error.not_ablatable, "{forbidden} is refused as unsafe");
            assert!(error.to_string().contains("unsafe, not merely different"));
        }
    }

    /// The safety sections stay in every arm, including the blanket one.
    #[test]
    fn all_never_reaches_the_sections_that_must_not_leave() {
        let every = PromptAblation::parse("all").expect("parses");
        assert!(every.suppresses("# Doing tasks"));
        assert!(every.suppresses("# Delegation and workflow routing"));
        for forbidden in NOT_ABLATABLE {
            assert!(
                !every.suppresses(forbidden),
                "`all` must not disarm {forbidden} — nobody named it"
            );
        }
    }

    #[test]
    fn the_production_arm_holds_nothing_out() {
        let none = PromptAblation::none();
        assert!(none.is_production_arm());
        assert!(!none.suppresses("# Doing tasks"));
        assert!(none.keys().is_empty());

        let one = PromptAblation::parse(" doing_tasks , ").expect("parses");
        assert!(!one.is_production_arm());
        assert_eq!(one.keys(), vec!["doing_tasks".to_string()]);
    }
}
