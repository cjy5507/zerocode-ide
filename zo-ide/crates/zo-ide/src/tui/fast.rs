//! `/fast` presentation and capability helpers.
//!
//! The request-side implementation already lives at the provider boundary:
//! `api::openai_fast_variant_pair` identifies the verified OpenAI family and
//! `chatgpt_backend` turns its `[fast]`/`-fast` spelling into
//! `service_tier: "priority"`. Keeping this module as the only TUI-side
//! decision point means the popup, toggle, and footer cannot drift apart or
//! invent a capability from an arbitrary model suffix.

/// Codex 0.150.0's service-tier popup description for the fast command.
pub const COMMAND_DESCRIPTION: &str = "1.5x speed, increased usage";

/// The footer token used by Codex when priority serving is active.
pub const FOOTER_TOKEN: &str = "fast";

/// Existing provider wire values echoed by the TUI's status notice.
pub const PRIORITY_REQUEST_VALUE: &str = "priority";
pub const DEFAULT_REQUEST_VALUE: &str = "default";

/// The exact Codex notice used when a slash command is not in the current
/// model's dynamic service-tier command set.
pub const UNSUPPORTED_COMMAND_MESSAGE: &str =
    "Unrecognized command '/fast'. Type \"/\" for a list of supported commands.";

/// Return the provider-owned base/fast pair for `model`.
#[must_use]
pub fn variant_pair(model: &str) -> Option<(String, String)> {
    api::openai_fast_variant_pair(model)
}

/// The one TUI-side fast capability decision. All presentation and toggle
/// callers derive their answer from this state rather than checking model
/// names independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub base_model: String,
    pub fast_model: String,
    pub enabled: bool,
}

#[must_use]
fn state(model: &str) -> Option<State> {
    let resolved = crate::cli_args::resolve_model_alias(model);
    let (base_model, fast_model) = variant_pair(&resolved)?;
    Some(State {
        enabled: resolved.eq_ignore_ascii_case(&fast_model),
        base_model,
        fast_model,
    })
}

/// Whether `model` is one of the verified fast-capable OpenAI families.
#[must_use]
pub fn supported(model: &str) -> bool {
    state(model).is_some()
}

/// Whether the current model spelling is the fast member of its pair.
#[must_use]
pub fn enabled(model: &str) -> bool {
    state(model).is_some_and(|state| state.enabled)
}

/// Normalize the model shown in TUI surfaces while keeping the transient fast
/// state as a separate boolean. The model id itself remains the transport
/// selector so the existing provider request path can carry the priority tier.
#[must_use]
pub fn display(model: &str) -> (String, bool) {
    let resolved = crate::cli_args::resolve_model_alias(model);
    match state(&resolved) {
        Some(state) if state.enabled => (state.base_model, true),
        Some(state) => (resolved, state.enabled),
        None => (resolved, false),
    }
}

/// Return the provider model id for the requested fast state.
#[must_use]
pub fn target(model: &str, fast_enabled: bool) -> Option<String> {
    let state = state(model)?;
    Some(if fast_enabled {
        state.fast_model
    } else {
        state.base_model
    })
}

/// Add the service-tier token to the footer/boot-card effort column without
/// changing the reasoning-effort value used by the picker or request builder.
#[must_use]
pub fn display_effort(effort: &str, fast_enabled: bool) -> String {
    if fast_enabled && effort.is_empty() {
        FOOTER_TOKEN.to_string()
    } else if fast_enabled && !effort.split_whitespace().any(|token| token == FOOTER_TOKEN) {
        format!("{effort} {FOOTER_TOKEN}")
    } else {
        effort.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{display, display_effort, enabled, supported, target, COMMAND_DESCRIPTION};

    #[test]
    fn only_catalogued_openai_families_support_fast() {
        assert!(supported("gpt-5.6-terra"));
        assert!(supported("gpt-5.5"));
        assert!(!supported("gpt-5.3-codex-spark"));
        assert!(!supported("claude-opus-5"));
    }

    #[test]
    fn fast_state_uses_the_provider_owned_variant_pair() {
        assert!(enabled("gpt-5.6-terra[fast]"));
        assert!(!enabled("gpt-5.6-terra"));
        assert_eq!(
            target("gpt-5.6-terra", true).as_deref(),
            Some("gpt-5.6-terra[fast]")
        );
        assert_eq!(
            target("gpt-5.6-terra[fast]", false).as_deref(),
            Some("gpt-5.6-terra")
        );
    }

    #[test]
    fn display_keeps_fast_out_of_the_model_label() {
        assert_eq!(
            display("gpt-5.6-terra[fast]"),
            ("gpt-5.6-terra".to_string(), true)
        );
        assert_eq!(
            display("claude-opus-5"),
            ("claude-opus-5".to_string(), false)
        );
    }

    #[test]
    fn popup_copy_matches_the_local_codex_capture() {
        assert_eq!(COMMAND_DESCRIPTION, "1.5x speed, increased usage");
    }

    #[test]
    fn footer_token_does_not_change_reasoning_effort() {
        assert_eq!(display_effort("medium", true), "medium fast");
        assert_eq!(display_effort("medium fast", true), "medium fast");
        assert_eq!(display_effort("medium", false), "medium");
    }
}
