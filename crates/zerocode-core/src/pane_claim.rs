//! Whether a hook report speaks for the pane it arrived wearing.
//!
//! A pane's identity travels to its agent through the environment — the pane
//! key, the launch token, the worktree — and an agent that runs another agent
//! (`codex exec …` under a Bash tool) hands that whole environment to the
//! child. The child's hooks then arrive wearing the parent's identity: same
//! pane, same token, its own conversation. Nothing in the envelope says "I am
//! the nested one", because nothing put it there.
//!
//! Adopting those reports cost three things at once: the child's conversation
//! id was written over the lead's, so reopening the pane's session reopened the
//! CHILD's; the child's prompt and answer were stamped on the lead's card; and
//! the pane's completion rang the moment the child stopped, while the lead
//! worked on.
//!
//! Orca guards the same inheritance at each consumer, by the vendor that sent
//! the event — `main/agent-hooks/server.ts:1540` ("a nested non-codex CLI
//! inherits ORCA_PANE_KEY, so clearing here would silently end a live codex
//! poll") and `:2048` ("nested CLIs may inherit the pane key; only accepted
//! statuses may mutate its background-work gate"). This is that judgement in
//! ONE place, stated over facts the caller already holds, so the pane's state,
//! its session record and its ring cannot answer the question three different
//! ways.

/// The facts that decide whose word a hook report is.
#[derive(Debug, Clone, Copy)]
pub struct PaneClaim<'a> {
    /// The vendor that sent this report. Read off the path the hook posted to
    /// and never out of the payload, so a body cannot claim to be another
    /// agent.
    pub vendor: &'a str,
    /// Whether a nested run of THAT SAME vendor is in flight in this pane.
    ///
    /// Ours rather than the original's, and it earns its place twice: it is the
    /// only thing that can tell an agent's own CLI apart from itself, and it
    /// outlives the parent going quiet, which the liveness test below does not.
    pub nested_run_live: bool,
    /// The pane's own agent, as far as anything knows it.
    pub pane_vendor: Option<&'a str>,
    /// Whether the pane's OWN turn is still going: its last word was not an
    /// ending one, and it is not old enough to have gone stale.
    ///
    /// The original's whole discriminator (`resolveAgentStatusIdentity`,
    /// `shared/agent-status-identity.ts:60-79`), and it says something a
    /// registered run cannot: **while the parent's turn is alive, a nested
    /// hook must not take the pane's identity.** Its other half matters just as
    /// much — once that turn is over or stale, the incoming vendor IS the pane,
    /// which is how somebody who quits one agent in a terminal and starts
    /// another gets their pane back.
    pub pane_turn_active: bool,
    /// Whether the word being judged is a turn-ending one.
    ///
    /// Only for the case nothing else can decide, where the original suppresses
    /// exactly this and nothing else: "a child completion does not prove the
    /// active parent turn completed" (`shouldSuppressInheritedTerminalStatus`,
    /// `:20-26`).
    pub incoming_is_done: bool,
    /// The conversation this report names, when its payload carried one.
    pub said_session: Option<&'a str>,
    /// The conversation this pane is already known to be having.
    pub known_session: Option<&'a str>,
}

/// Whether this report is the pane's own word.
///
/// Three questions, and the first is the original's:
///
/// 1. **A vendor other than the pane's.** It is the child while the pane's own
///    turn is still going — a nested hook must not take a live pane's identity
///    — or while a run of that very vendor is in flight, which stays true after
///    the parent has gone quiet. Otherwise the pane has been HANDED OVER:
///    somebody quit one agent in that terminal and started another, and the
///    incoming vendor is the pane now. Getting that half wrong is not a
///    cosmetic bug, it is a live agent whose every word is thrown away.
/// 2. **The same vendor, with no run of it in flight** — the pane. Nobody else
///    could be speaking, and this is the common path.
/// 3. **The same vendor with a run in flight** (an agent running its own CLI):
///    the conversation id is the only thing left that can differ, and a report
///    naming another one is the child's.
///
///    When neither side names one — and several vendors never do — the word is
///    left to the pane EXCEPT for a turn-ending one, which is the single word
///    that costs something to get wrong: a `done` rings a completion and empties
///    the card, while a `working` only says what is already true. That
///    asymmetry is the original's, exactly
///    (`shouldSuppressInheritedTerminalStatus`).
pub fn speaks_for_the_pane(claim: &PaneClaim<'_>) -> bool {
    if claim.pane_vendor.is_some_and(|own| own != claim.vendor) {
        return !(claim.pane_turn_active || claim.nested_run_live);
    }
    if !claim.nested_run_live {
        return true;
    }
    match (claim.said_session, claim.known_session) {
        (Some(said), Some(known)) => said == known,
        _ => !(claim.incoming_is_done && claim.pane_turn_active),
    }
}

#[cfg(test)]
mod tests {
    use super::{PaneClaim, speaks_for_the_pane};

    /// A claude pane, mid-turn, with no nested run and no conversation known.
    fn claim<'a>(vendor: &'a str) -> PaneClaim<'a> {
        PaneClaim {
            vendor,
            nested_run_live: false,
            pane_vendor: Some("claude"),
            pane_turn_active: true,
            incoming_is_done: false,
            said_session: None,
            known_session: None,
        }
    }

    #[test]
    fn a_lead_still_speaks_while_its_nested_run_goes() {
        // The pane is claude and a `codex exec` is in flight under it. The live
        // run is codex's, so claude's own word is not gated at all.
        assert!(speaks_for_the_pane(&claim("claude")));
    }

    #[test]
    fn a_nested_vendors_word_is_not_the_panes_while_the_turn_is_alive() {
        // Both roads to the same answer: the parent is mid-turn, and — even if
        // it were not — a run of this very vendor is in flight.
        assert!(!speaks_for_the_pane(&claim("codex")));
        let mut quiet_parent = claim("codex");
        quiet_parent.pane_turn_active = false;
        quiet_parent.nested_run_live = true;
        assert!(!speaks_for_the_pane(&quiet_parent));
    }

    #[test]
    fn a_pane_somebody_handed_to_another_agent_belongs_to_that_agent() {
        // Quit claude in a terminal, start codex in the same one. The pane's
        // remembered vendor is stale, its turn is long over, and nothing of
        // ours is running — so codex IS the pane. Getting this half wrong is a
        // live agent whose every word is thrown away.
        let mut swapped = claim("codex");
        swapped.pane_turn_active = false;
        swapped.said_session = Some("codex-1");
        swapped.known_session = Some("claude-1");
        assert!(speaks_for_the_pane(&swapped));
        // And a pane nothing ever reported for is nobody's yet.
        let mut unknown = claim("codex");
        unknown.pane_vendor = None;
        unknown.pane_turn_active = false;
        assert!(speaks_for_the_pane(&unknown));
    }

    #[test]
    fn an_agent_running_its_own_cli_is_told_apart_by_the_conversation() {
        let mut child = claim("claude");
        child.nested_run_live = true;
        child.said_session = Some("child-session");
        child.known_session = Some("lead-session");
        assert!(!speaks_for_the_pane(&child));

        let mut lead = child;
        lead.said_session = Some("lead-session");
        assert!(speaks_for_the_pane(&lead));
    }

    #[test]
    fn where_nothing_names_a_conversation_only_the_ending_word_is_suppressed() {
        // Several vendors publish no session id at all, so the id cannot tell
        // an agent's own CLI apart from itself. A `working` is allowed through
        // — it only says what is already true — and a `done` is not: it would
        // ring a completion and empty the card while the lead works on.
        let mut working = claim("claude");
        working.nested_run_live = true;
        working.known_session = Some("lead-session");
        assert!(speaks_for_the_pane(&working));

        let mut ending = working;
        ending.incoming_is_done = true;
        assert!(!speaks_for_the_pane(&ending));
        // Once the pane's own turn is over, an ending word has nothing left to
        // interrupt.
        let mut after = ending;
        after.pane_turn_active = false;
        assert!(speaks_for_the_pane(&after));
    }

    #[test]
    fn a_pane_whose_agent_nobody_named_falls_to_the_conversation() {
        let mut child = claim("claude");
        child.pane_vendor = None;
        child.nested_run_live = true;
        child.said_session = Some("child-session");
        child.known_session = Some("lead-session");
        assert!(!speaks_for_the_pane(&child));

        let mut lead = child;
        lead.said_session = Some("lead-session");
        assert!(speaks_for_the_pane(&lead));
    }
}
