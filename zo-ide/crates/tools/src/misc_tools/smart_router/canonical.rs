//! Route-outcome model-id canonicalization (P3 outcome-store v2).
//!
//! Historical `route-outcomes.jsonl` records key model buckets by the RAW
//! `selectedModel` string a spawn recorded at the time, so id fragments that
//! all name the same underlying model dilute the decisive sample count
//! across near-duplicate buckets — live data showed `claude-opus-4-8` vs
//! `claude-opus-4.8`, `fable`/`fable5` vs `claude-fable-5`, and `gpt-5.5` vs
//! its dated canonical id all splitting what should be one bucket.
//!
//! The router engine (`runtime::model_router::outcome`) stays free of any
//! alias-resolution logic (it has zero dependency on `api`), so this fn lives
//! in the tools layer and is INJECTED into the engine's canonicalizing
//! summarize/lookup entry points (`summarize_route_outcomes_with_canonicalizer`,
//! `weighted_feedback_hint_for_route_key`) instead of the engine resolving
//! aliases itself.
//!
//! Built entirely on the existing `api::resolve_model_alias` — no bespoke
//! dot/dash or dated-suffix logic needed here. That resolver already:
//! - maps registered aliases to their canonical id (`fable`/`fable5` →
//!   `claude-fable-5`; `gpt-5.5` → its dated canonical id), and
//! - normalizes a `claude-`-prefixed id's `.` version separator to `-`
//!   (`normalize_model_id` in `providers/mod.rs`) — the exact `claude-opus-4-8`
//!   ≡ `claude-opus-4.8` case — as a fallback for ids that resolve to no
//!   known alias/canonical entry.
//!
//! Caveat (inherited from `resolve_model_alias`, not introduced here): a
//! near-miss/alias resolution only fires when the target provider is
//! currently enabled/credentialed (or `ZO_ALLOW_EXPERIMENTAL_PROVIDERS`),
//! since that function is primarily used for live routing decisions. Reading
//! history for a provider that is temporarily uncredentialed merges fewer
//! fragments than a fully-credentialed session would — a documented
//! limitation of reusing this primitive, not a correctness bug.
#[must_use]
pub(crate) fn canonicalize_route_model_id(raw: &str) -> String {
    api::resolve_model_alias(raw.trim())
}

#[cfg(test)]
mod tests {
    use super::canonicalize_route_model_id;

    #[test]
    fn merges_claude_dot_and_dash_version_fragments() {
        assert_eq!(canonicalize_route_model_id("claude-opus-4.8"), "claude-opus-4-8");
        assert_eq!(canonicalize_route_model_id("claude-opus-4-8"), "claude-opus-4-8");
    }

    #[test]
    fn merges_fable_alias_fragments() {
        // The family alias follows its head (Fable 5.1); a release-bearing id
        // stays the release it names.
        assert_eq!(canonicalize_route_model_id("fable"), "claude-fable-5-1");
        assert_eq!(canonicalize_route_model_id("fable5"), "claude-fable-5-1");
        assert_eq!(canonicalize_route_model_id("claude-fable-5"), "claude-fable-5");
        assert_eq!(canonicalize_route_model_id("claude-fable-5-1"), "claude-fable-5-1");
    }

    #[test]
    fn merges_gpt_5_5_dated_alias() {
        // The bare-alias → dated-canonical resolution only fires when
        // `resolve_model_alias` sees OpenAI as "enabled" (its near-miss/alias
        // gate for non-Anthropic providers) — true on a session with OpenAI
        // configured, but NOT guaranteed in a bare test process. Force it
        // deterministically instead of depending on the ambient machine's
        // credential state, under the crate's shared env-mutation lock (other
        // modules assert on the same global env).
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior = std::env::var_os("OPENAI_API_KEY");
        std::env::set_var("OPENAI_API_KEY", "test-key-for-canonicalization");
        // gpt-5.5 별칭은 퇴역(2026-07-11) — passthrough. bare-alias→canonical
        // 병합의 살아있는 예는 gpt-5.6→terra다.
        assert_eq!(canonicalize_route_model_id("gpt-5.5"), "gpt-5.5");
        assert_eq!(canonicalize_route_model_id("gpt-5.6"), "gpt-5.6-terra");
        match prior {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }

        // Already-canonical (dated) form: `is_known_canonical` short-circuits
        // before the provider-enabled gate, so this half is deterministic
        // regardless of ambient environment.
        assert_eq!(
            canonicalize_route_model_id("gpt-5.5-2026-04-23"),
            "gpt-5.5-2026-04-23"
        );
    }

    #[test]
    fn leaves_unrelated_dotted_ids_untouched() {
        // Non-Claude families spell their version with a dot ON PURPOSE
        // (`gpt-5.5-fast`, `gemini-3.5-flash`) — canonicalization must never
        // rewrite those.
        assert_eq!(canonicalize_route_model_id("gpt-5.5-fast"), "gpt-5.5-fast");
    }

    #[test]
    fn is_a_no_op_on_empty_input() {
        assert_eq!(canonicalize_route_model_id(""), "");
        assert_eq!(canonicalize_route_model_id("   "), "");
    }
}
