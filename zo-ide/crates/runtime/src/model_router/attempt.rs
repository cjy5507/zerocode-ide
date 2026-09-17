//! The attempt key — the one string four ledgers join on.
//!
//! A task's cost is spread across four files that never shared a key:
//! `route-outcomes.jsonl` (what was decided and how it ended),
//! `requests.jsonl` (what each request billed), `timings.jsonl` (where the
//! wait went) and the agent manifests. Nothing could answer "what did ONE
//! attempt cost, in time and in tokens, and was it verified" from disk,
//! because no two of those files could be lined up
//! (`docs/design/zo-attempt-key-contract-20260915.md` §1).
//!
//! An attempt is one try at one piece of work: a user turn in the main
//! session, or one generation of one spawned agent. This module is the ONLY
//! place either spelling is formed, so every writer names the same attempt the
//! same way and every reader joins on equality alone — no reader parses a key,
//! and the two shapes use different separators (`#` and `@`) so a reader can
//! tell them apart without one.

/// A spawn attempt: the agent id and the generation that attempt ran as.
///
/// `<agentId>#<runGeneration>` — the spelling `RouteOutcomeRecord::run_id` has
/// always used. The pair is the agent store's durable identity: the id is
/// wall-clock nanoseconds claimed with an exclusive create, and the generation
/// lives in the manifest and advances on every resume. A blank id names
/// nothing, so it yields `None` rather than a key like `#0` that would join
/// unrelated records to each other.
#[must_use]
pub fn spawn_attempt_key(agent_id: &str, run_generation: u64) -> Option<String> {
    let agent_id = agent_id.trim();
    (!agent_id.is_empty()).then(|| format!("{agent_id}#{run_generation}"))
}

/// A main-session turn attempt: the session and the 1-based ordinal of the
/// user turn within it.
///
/// `<sessionId>@<turnOrdinal>`. The separator differs from
/// [`spawn_attempt_key`]'s deliberately: the two kinds of attempt live in the
/// same columns, and a reader must be able to tell a turn from a spawn without
/// parsing either.
#[must_use]
pub fn turn_attempt_key(session_id: &str, turn_ordinal: usize) -> String {
    format!("{session_id}@{turn_ordinal}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawn_attempt_is_the_agent_and_its_generation() {
        assert_eq!(
            spawn_attempt_key("agent-1787013448552488000", 2).as_deref(),
            Some("agent-1787013448552488000#2")
        );
    }

    /// A blank id names no attempt at all — a record with `#0` would join to
    /// every other blank-id record, which is worse than staying unattributed.
    #[test]
    fn a_blank_agent_id_names_no_attempt() {
        assert_eq!(spawn_attempt_key("", 3), None);
        assert_eq!(spawn_attempt_key("   ", 3), None);
    }

    /// The two shapes must stay distinguishable without parsing: a turn key
    /// carries `@` and no `#`, a spawn key the reverse.
    #[test]
    fn the_two_shapes_are_told_apart_by_their_separator() {
        let turn = turn_attempt_key("session-2026-09-15", 7);
        let spawn = spawn_attempt_key("agent-9", 1).expect("named agent");
        assert_eq!(turn, "session-2026-09-15@7");
        assert!(turn.contains('@') && !turn.contains('#'));
        assert!(spawn.contains('#') && !spawn.contains('@'));
    }
}
