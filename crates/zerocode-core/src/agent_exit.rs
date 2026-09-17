//! Noticing that an agent left a pane that its shell outlived.
//!
//! Every agent state this window draws arrives as a hook event, and the event
//! that ENDS a run is `Stop`. Two ordinary situations never produce one:
//!
//! - an agent whose vendor has no hook channel on this machine (the hooks were
//!   never installed, or the vendor's own trust gate refused ours), and
//! - an agent that was *quit* rather than finished — `codex`, then `/exit`, and
//!   the person is back at their prompt.
//!
//! Neither is a closed terminal, so `term:exited` does not fire either: the
//! SHELL is still there, still alive, still drawing a prompt. So the pane's
//! last word stood forever and a sidebar reported "Codex 작업 중" at a checkout
//! where nothing at all was running — the reported bug this module exists for.
//!
//! What replaces the missing event is a fact the operating system will answer
//! at any moment: **which process group is holding the terminal**. A job-control
//! shell hands it to the command it starts and takes it back when that command
//! ends, so "the pty's foreground group is the shell's own again" is the agent
//! having gone, told by the kernel rather than by the agent.
//!
//! Orca answers the same question by CLASSIFYING THE TERMINAL'S TITLE. A title
//! is a string the running program chooses to write: it is absent for programs
//! that write none, stale for programs that die without restoring it, and
//! spelled differently by every vendor and every version — so the classifier is
//! a list of patterns that has to grow forever and is wrong in between. The
//! foreground group is none of those things. It is not a claim, it is the
//! terminal's own state, it exists for every program including the ones that
//! have never heard of us, and it cannot be stale because it is not a message.
//!
//! Everything here is pure. The syscall lives in `zerocode-pty`
//! (`PtyLane::foreground_is_child`) and the cadence lives in the window's pump;
//! what is decided here is only *what a run of observations means*, which is
//! the part with a judgement in it.

use crate::hook::HookState;

/// How many consecutive looks must find the shell back in front before the
/// agent counts as gone.
///
/// Not one. A single look is a single instant, and there are instants when a
/// shell legitimately holds its own terminal with an agent still very much
/// alive: the moment between two commands in a launch line, and the moment a
/// job is stopped and resumed. Three agreeing looks is a claim about a
/// *stretch* of time rather than a sample of it.
///
/// Three and no more, because the cost of waiting is a badge that keeps lying
/// for as long as the wait. At [`LOOK_EVERY_MS`] that is 1.5 seconds — the same
/// grace this product already makes a `Done` stand for before it believes a
/// turn ended ([`crate::notify::DONE_QUIET_MS`]), which is not a coincidence:
/// both are answers to "how long must a stop hold still before it is real".
pub const LOOKS_BEFORE_GONE: u8 = 3;

/// How often the pump asks. Not every round — the pump turns at the display
/// rate and this is two syscalls per agent-bearing terminal, asked to learn a
/// thing that changes once a session.
pub const LOOK_EVERY_MS: u64 = 500;

/// Whether a pane's held state claims there is an agent running in it.
///
/// `None` — a pane this window launched as an agent that has never reported —
/// counts as a claim, because that is exactly what the board does with it: a
/// live shell under an agent launch is drawn `working` rather than `idle`
/// (`pane_agents`), on the argument that a vendor with no hook integration
/// running invisibly is the situation the board exists to reveal. That fallback
/// is the one this check has to be able to correct; leaving it out would fix
/// the bug for agents that report and leave it standing for the agents that
/// never say anything, which are the ones it was reported against.
#[must_use]
pub const fn claims_running(state: Option<HookState>) -> bool {
    match state {
        None | Some(HookState::Working | HookState::NeedsAttention) => true,
        Some(HookState::Done | HookState::Idle) => false,
    }
}

/// One pane's running answer to "is the thing this pane claims still there".
///
/// Two fields, and the first one is the important one.
///
/// `armed` is what keeps this from firing on panes where the answer is
/// meaningless. Some terminals ARE the agent — a launch that spawns `claude`
/// straight into the pty, or a `sh -lc "cd … && codex …"` line, where job
/// control is off and the command never gets a group of its own. In those the
/// foreground group is the pty's child from the first instant to the last, so
/// "the child is in front" is true while the agent is working and would read as
/// an exit on the very first look. It is also the case that needs no help: when
/// that agent ends, the pty ends with it and the ordinary `term:exited` road
/// already cleans up.
///
/// So nothing is believed until something OTHER than the pty's own child has
/// been seen holding the terminal. That single bit tells the two shapes apart
/// without anything having to record which shape a pane is — no flag at launch,
/// no argv guess, no list of agent names — and it is right for the case the
/// list would miss: a `claude` somebody typed by hand into a plain shell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForegroundWatch {
    /// Whether anything but the pty's own child has ever held this terminal.
    armed: bool,
    /// Consecutive looks since then that found the child back in front.
    quiet: u8,
}

impl ForegroundWatch {
    /// Fold one look into the run behind it.
    ///
    /// `child_in_front` is `PtyLane::foreground_is_child`'s
    /// answer. A look the operating system would not answer must not arrive
    /// here at all — the caller keeps the watch untouched, because "no opinion"
    /// is not evidence of a departure and counting it as one would clear every
    /// agent on a platform that has no foreground groups.
    #[must_use]
    pub const fn look(self, child_in_front: bool) -> Self {
        if child_in_front {
            Self {
                armed: self.armed,
                // Saturating rather than wrapping: a pane that sat at its
                // prompt for an hour must not roll back to zero and un-declare
                // an exit it already declared.
                quiet: if self.armed {
                    self.quiet.saturating_add(1)
                } else {
                    0
                },
            }
        } else {
            // Something else has the terminal. That both arms the watch and
            // ends any run of quiet: an agent that took the terminal back is an
            // agent that is there, whatever the last two looks thought.
            Self {
                armed: true,
                quiet: 0,
            }
        }
    }

    /// Whether the run is now long enough to say the agent has gone.
    #[must_use]
    pub const fn agent_left(self) -> bool {
        self.armed && self.quiet >= LOOKS_BEFORE_GONE
    }

    /// Whether this watch has ever seen an agent hold the terminal — for the
    /// gate tests, and for a caller that wants to drop the ones that never did.
    #[must_use]
    pub const fn armed(self) -> bool {
        self.armed
    }
}

/// Whether a terminal's own child IS an agent, told by the name it is
/// called by (`child_programs`, best first, as `zerocode-pty`'s naming roads
/// give them).
///
/// That is the shape [`ForegroundWatch`] tells apart by never seeing
/// anything but the child in front: a launch that spawns `claude` or `zo`
/// straight into the pty. On unix the kernel keeps that shape by itself —
/// an agent does no job control, so its tools share its process group and
/// the foreground group is the child's from the first instant to the last.
/// A ConPTY has no foreground group, and its process tree cannot tell a
/// shell running a command from an agent running a tool: both are a child
/// with a child. Read as a shell, a window-launched `zo.exe` was declared
/// gone 1.5 s after its first tool finished, and its Zo channel was
/// detached on every tool call (WIN16). So where the terminal cannot say
/// it, the rule is stated: a child whose own name is an agent's holds its
/// terminal, whatever runs under it.
///
/// The first name only — the one the process is called by — never a
/// directory of its path: a shell under a profile folder that shares an
/// agent's name (`C:\Users\pi\…`) is still a shell.
#[must_use]
pub fn child_is_the_agent(child_programs: &[String]) -> bool {
    child_programs
        .first()
        .is_some_and(|called| crate::agent::agent_spec_by_process(called).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pty's own child names an agent by the name it is called by —
    /// `zo`, `claude` — and only by that name: a shell is not an agent,
    /// nor is a shell whose path merely passes through a folder that
    /// shares an agent's name, nor a child nobody could read.
    #[test]
    fn a_child_called_by_an_agents_name_is_the_agent() {
        let named = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>()
        };
        assert!(child_is_the_agent(&named(&["zo", "bin", ".cargo"])));
        assert!(child_is_the_agent(&named(&["claude", "bin", ".local"])));
        assert!(!child_is_the_agent(&named(&["pwsh", "7", "PowerShell"])));
        assert!(!child_is_the_agent(&named(&[
            "pwsh", "current", "pi", "Users"
        ])));
        assert!(!child_is_the_agent(&[]));
    }

    /// The states that mean "something is running in there". `Idle` is not one
    /// of them, which is what stops a cleared pane from being cleared again
    /// every half second for the rest of the window's life.
    #[test]
    fn only_a_state_that_claims_an_agent_is_worth_asking_about() {
        assert!(claims_running(Some(HookState::Working)));
        assert!(claims_running(Some(HookState::NeedsAttention)));
        assert!(!claims_running(Some(HookState::Done)));
        assert!(!claims_running(Some(HookState::Idle)));
        // The board draws this one `working` off a live shell alone, so this
        // check has to be willing to correct it.
        assert!(claims_running(None));
    }

    /// A pane whose child has held the terminal from the first look is a pane
    /// where the child IS the agent. It never fires, however long it sits.
    #[test]
    fn a_pane_that_never_gave_the_terminal_away_is_never_declared_empty() {
        let mut watch = ForegroundWatch::default();
        for _ in 0..1000 {
            watch = watch.look(true);
            assert!(!watch.agent_left(), "{watch:?}");
        }
        assert!(!watch.armed());
    }

    /// The whole sequence, as it actually happens: shell at its prompt, agent
    /// takes the terminal, agent exits, shell has it back.
    #[test]
    fn the_shell_taking_the_terminal_back_declares_the_agent_gone() {
        let mut watch = ForegroundWatch::default();
        // Before anything ran, the shell holds its own terminal — and this must
        // say nothing, or a pane would be cleared before its agent started.
        watch = watch.look(true);
        assert!(!watch.agent_left());

        watch = watch.look(false);
        assert!(watch.armed());
        assert!(!watch.agent_left());

        for round in 1..LOOKS_BEFORE_GONE {
            watch = watch.look(true);
            assert!(!watch.agent_left(), "declared gone after {round} look(s)");
        }
        watch = watch.look(true);
        assert!(watch.agent_left());
    }

    /// The debounce earning its keep: a single look that catches the shell
    /// between two things is not an exit.
    #[test]
    fn one_look_between_two_commands_does_not_end_a_run() {
        let mut watch = ForegroundWatch::default().look(false);
        for _ in 0..40 {
            watch = watch.look(true);
            assert!(!watch.agent_left());
            watch = watch.look(false);
            assert!(!watch.agent_left());
        }
    }

    /// A job stopped and resumed — the shell holds the terminal for a stretch
    /// and then the agent takes it back. The run resets rather than resuming
    /// where it left off, so two separate near-misses never add up to one
    /// verdict.
    #[test]
    fn an_agent_that_comes_back_resets_the_run_it_interrupted() {
        let mut watch = ForegroundWatch::default().look(false);
        for _ in 0..LOOKS_BEFORE_GONE - 1 {
            watch = watch.look(true);
        }
        watch = watch.look(false);
        assert!(!watch.agent_left());
        for _ in 0..LOOKS_BEFORE_GONE - 1 {
            watch = watch.look(true);
            assert!(!watch.agent_left(), "the interrupted run was carried over");
        }
    }

    /// Once declared, it stays declared — the counter does not wrap around into
    /// a fresh, quiet watch after a couple of minutes at a prompt.
    #[test]
    fn a_long_wait_at_the_prompt_does_not_wrap_back_into_silence() {
        let mut watch = ForegroundWatch::default().look(false);
        for _ in 0..10_000 {
            watch = watch.look(true);
        }
        assert!(watch.agent_left());
        assert_eq!(watch.quiet, u8::MAX);
    }
}
