//! Verification-treadmill circuit breaker for [`ConversationRuntime`]: detects a
//! turn that keeps re-planning / re-validating / re-spawning (`Workflow`,
//! `WorkflowValidate`, `SpawnMultiAgent`, `Agent`) without ever changing a file
//! and stops it gracefully, instead of letting it self-verify forever. Split out
//! of `mod.rs` next to the tool-call [`super::repetition`] guard, which it
//! complements: repetition catches an *identical* call repeated, this catches a
//! *self-verification loop* whose spec differs every round, so the fingerprint
//! guard never fires.

use super::{
    is_edit_or_write_tool, ApiClient, ContentBlock, ConversationMessage, ConversationRuntime,
    ToolExecutor, TurnContinuity, TurnStopSignal,
};

/// Default soft (advisory) threshold: the number of consecutive verify-class
/// rounds with no file mutation this turn that injects the "stop self-verifying,
/// report to the user" advisory. Overridable via `ZO_VERIFY_TREADMILL_ROUNDS`
/// (`0` disables the whole guard). See [`verify_treadmill_thresholds`].
pub(super) const VERIFY_TREADMILL_ADVISE: usize = 6;
/// Fixed grace between the soft advisory and the hard stop: the hard threshold
/// is always the soft threshold plus this, so an env override of the soft
/// threshold keeps the same four-round grace before the turn is force-ended.
pub(super) const VERIFY_TREADMILL_HARD_STOP_MARGIN: usize = 4;

/// Transient wire-reminder prefix for the soft advisory, so it is refreshed
/// (replace-by-prefix) as the round count climbs and cleared at turn start
/// rather than accumulating — mirrors [`super::RECALL_HINT_REMINDER_PREFIX`].
pub(super) const VERIFY_TREADMILL_REMINDER_PREFIX: &str = "[zo:verify-treadmill]";

/// Tools that plan / validate / delegate rather than directly changing the
/// workspace. An iteration that calls one of these but mutates no file is a
/// "verification treadmill" round. `Agent`/`SpawnMultiAgent` are included so a
/// turn that keeps re-spawning without ever converging is bounded too; see the
/// false-positive note on [`ConversationRuntime::note_verify_treadmill`].
pub(super) fn is_verify_class_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Workflow" | "WorkflowValidate" | "SpawnMultiAgent" | "Agent"
    )
}

/// Whether this tool batch actually CHANGED a file: a *successful*
/// `edit_file`/`write_file` RESULT, not merely a requested one.
///
/// Reading the requested tool names instead let a failed — or permission-denied
/// — edit reset the treadmill counter, so a loop that kept *attempting* the same
/// impossible write evaded the guard forever while making no progress at all.
/// The counter may only be forgiven by work that landed, which is also the
/// definition [`super::count_progress_tool_results`] uses for the nudge ledger —
/// the two must agree or the guard and its reprieve disagree about what progress
/// even is.
pub(super) fn batch_had_successful_mutation(results: &[ConversationMessage]) -> bool {
    results
        .iter()
        .flat_map(|message| &message.blocks)
        .any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult {
                    tool_name,
                    is_error,
                    ..
                } if !is_error && is_edit_or_write_tool(tool_name)
            )
        })
}

/// The `(advise, hard)` round thresholds, or `None` when the guard is disabled.
/// `ZO_VERIFY_TREADMILL_ROUNDS` overrides the soft threshold (default
/// [`VERIFY_TREADMILL_ADVISE`]); `0` disables the guard entirely; an empty or
/// non-numeric value falls back to the default rather than silently disabling
/// the safety net. The hard threshold is always soft +
/// [`VERIFY_TREADMILL_HARD_STOP_MARGIN`]. Read per verify-round (not memoized)
/// so an operator can retune or disable the backstop without a rebuild — the
/// escape-hatch idiom of `ZO_MAX_ITERATIONS`.
pub(super) fn verify_treadmill_thresholds() -> Option<(usize, usize)> {
    let advise = std::env::var("ZO_VERIFY_TREADMILL_ROUNDS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(VERIFY_TREADMILL_ADVISE);
    (advise > 0).then_some((advise, advise + VERIFY_TREADMILL_HARD_STOP_MARGIN))
}

/// The advisory injected when the HARD threshold is reached but the turn is
/// spared because it has produced fresh objective progress (see
/// [`ConversationRuntime::resolve_heuristic_stop`]). It must not say the turn is
/// ending — the turn deliberately continues — but it must be unmistakably
/// stronger than the soft advisory, because this is the last warning before the
/// stop actually lands.
fn verify_treadmill_nudge_advisory(run: usize) -> String {
    format!(
        "{VERIFY_TREADMILL_REMINDER_PREFIX} <system-reminder>You have now run {run} rounds of \
         planning, validation, or agent-spawning this turn. This turn HAS changed files, so it \
         is not being stopped — but this is the last warning: another run of verification \
         rounds without changing a file will end the turn. Go make the next concrete change now \
         (edit the file, run the command), or report what you have and hand back. Do not open \
         another planning, validation, or spawn pass to re-check the same thing.</system-reminder>"
    )
}

/// The soft-advisory body (a transient system-reminder) injected once the turn
/// has run `run` verify-class rounds with no file change. Carries the prefix so
/// replace-by-prefix refreshes the climbing count in place.
fn verify_treadmill_advisory(run: usize) -> String {
    format!(
        "{VERIFY_TREADMILL_REMINDER_PREFIX} <system-reminder>You have now run {run} rounds of \
         planning, validation, or agent-spawning this turn without changing any files or \
         producing a new result — a self-verification treadmill that is not making progress. \
         Stop re-verifying. Report to the user what you have done and what is blocking you, and \
         ask for direction. Do not run another validation, planning, or spawn pass just to \
         re-check the same thing.</system-reminder>"
    )
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Fold one completed tool batch into the verification-treadmill counter and
    /// act on it. Returns [`TurnContinuity::Stop`] when the turn must hard-stop —
    /// the caller then pushes the graceful
    /// [`super::BudgetExhausted::VerificationTreadmill`] closer and breaks. The
    /// soft advisory is injected as a side effect and never ends the turn.
    ///
    /// Progress model (see the module doc):
    /// - `had_mutation` — the batch edited/wrote a file: real progress, reset to 0.
    /// - `had_verify && !had_mutation` — a planning/validation/spawn round with no
    ///   file change: `+1`. At the soft threshold inject the advisory; at the hard
    ///   threshold signal a stop.
    /// - neither — a pure research batch (read/grep/glob/bash): neutral, the
    ///   counter is unchanged. A turn that never calls a verify-class tool can
    ///   therefore never trip this guard (the core invariant).
    ///
    /// False-positive note: a pure *orchestrator* turn that delegates every edit
    /// to sub-agents (`SpawnMultiAgent`/`Agent`) and never mutates a file itself
    /// looks like a treadmill here. That is why the hard threshold does not end
    /// the turn on its own: it asks
    /// [`ConversationRuntime::resolve_heuristic_stop`], which spares a turn that
    /// has produced fresh objective progress exactly once. A turn with no
    /// progress at all — the genuine no-edit spawn loop this guard exists to
    /// catch — still stops immediately. `ZO_VERIFY_TREADMILL_ROUNDS=0` disables
    /// the guard entirely for a workload built on delegation.
    pub(super) fn note_verify_treadmill(
        &mut self,
        had_verify: bool,
        had_mutation: bool,
    ) -> TurnContinuity {
        if had_mutation {
            self.verify_treadmill_run = 0;
            return TurnContinuity::Continue;
        }
        if !had_verify {
            return TurnContinuity::Continue;
        }
        // Only a verify-without-mutation round reads the env knob, so a turn that
        // never treadmills never touches it — and the guard's env override cannot
        // perturb an unrelated turn.
        let Some((advise, hard)) = verify_treadmill_thresholds() else {
            return TurnContinuity::Continue; // disabled via ZO_VERIFY_TREADMILL_ROUNDS=0
        };
        self.verify_treadmill_run = self.verify_treadmill_run.saturating_add(1);
        let run = self.verify_treadmill_run;
        if run >= hard {
            let continuity = self.resolve_heuristic_stop(TurnStopSignal::VerifyTreadmill);
            if continuity == TurnContinuity::Nudge {
                // Spared: rewind to the soft threshold so the guard re-arms and
                // the SAME climb can stop the turn a second time. The nudge
                // budget is already spent, so that second stop is unconditional.
                self.verify_treadmill_run = advise;
                let advisory = verify_treadmill_nudge_advisory(run);
                self.replace_transient_system_reminder_by_prefix(
                    VERIFY_TREADMILL_REMINDER_PREFIX,
                    Some(&advisory),
                );
            }
            return continuity;
        }
        if run >= advise {
            let advisory = verify_treadmill_advisory(run);
            self.replace_transient_system_reminder_by_prefix(
                VERIFY_TREADMILL_REMINDER_PREFIX,
                Some(&advisory),
            );
        }
        TurnContinuity::Continue
    }
}
