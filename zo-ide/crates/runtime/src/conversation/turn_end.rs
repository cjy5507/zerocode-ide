//! Turn-completion support for [`ConversationRuntime`]: exhausted-budget
//! closers, the `TurnEnd`-hook context builder, the changed-files snapshots, and
//! the deterministic spec-literal auto-patch. Split out of `mod.rs` so the turn
//! loops there read as orchestration. Behaviour-preserving: these were
//! `ConversationRuntime` methods and module-level helpers, now `pub(super)`
//! where the loops in `mod.rs` (and `deep_gate`/tests) still reach them.

use serde_json::{json, Value};

use crate::session::{ContentBlock, ConversationMessage};

use super::{
    is_edit_or_write_tool, ApiClient, BudgetExhausted, ConversationRuntime, ToolExecutor,
    TurnSummary,
};

/// Short, factual label for a turn-budget kind, used in both the synthetic
/// closing assistant message and the headless/stream notice.
fn budget_exhausted_label(kind: BudgetExhausted) -> &'static str {
    match kind {
        BudgetExhausted::Iterations => "Iteration budget",
        BudgetExhausted::Deadline => "Time budget",
        BudgetExhausted::ToolCalls => "Tool-call budget",
        BudgetExhausted::OutputTokens => "Output-token budget",
        BudgetExhausted::InputTokens => "Input-token budget",
        BudgetExhausted::VerificationTreadmill => "Verification loop",
        BudgetExhausted::ToolRepetition => "Tool-repetition guard",
    }
}

/// The user-facing one-liner recorded when a turn stops on an exhausted budget:
/// what ran out, that the work so far is preserved, and how to proceed. Kept
/// short and factual — the same text is used for the synthetic closer message,
/// the headless `eprintln`, and the streaming `System { Warn }` notice.
pub(super) fn budget_exhausted_notice(kind: BudgetExhausted, iterations: usize) -> String {
    format!(
        "[budget] {} exhausted after {iterations} iteration(s); the work above is \
         preserved. Continue in a follow-up turn or narrow the task.",
        budget_exhausted_label(kind)
    )
}

/// Reminder installed on the turn AFTER a budget/treadmill closer, naming that
/// closer as a harness cutoff rather than the model's own decision to stop.
///
/// The closer is stored under the `assistant` role because the wire contract
/// requires it (a user turn is never left without an assistant response) — but
/// it is the harness speaking, not the model. Left unexplained, the next turn
/// reads "I've stopped here because I kept planning…" as its OWN last words and
/// is biased toward quitting again, a self-reinforcing stall no amount of
/// prompting undoes. This reminder is where that gets corrected.
///
/// It carries no marker into the closer text itself: that message is also the
/// turn's `assistant_messages` tail, which [`super::final_assistant_text`]
/// surfaces as the headless `result` field — an internal control tag there would
/// leak into user-facing output. Mirrors
/// [`super::EMPTY_STREAM_CONTINUATION_REMINDER`], which solves the same problem
/// for the empty-stream fallback.
pub(super) fn turn_budget_continuation_reminder() -> &'static str {
    "[zo:turn-budget-continuation] <system-reminder>The previous turn ended because it hit a \
     harness execution budget, not because the work was finished and not because you decided to \
     stop — its closing message was written by the harness, not by you. The session state is \
     intact: the earlier request, tool calls, and results remain valid context. Continue from \
     exactly where it stopped and finish the remaining work; do not redo completed steps, do not \
     re-plan from scratch, and do not treat that closer as your own conclusion. If everything is \
     genuinely complete, summarize the outcome instead.</system-reminder>"
}

/// CC-style handback used as the assistant closer when the verification-treadmill
/// breaker force-ends a turn. Unlike the terse budget one-liner, the fix here is
/// human guidance (the loop won't converge on its own), so the closer reads as
/// the agent stopping and handing back rather than "continue in a follow-up".
fn verification_treadmill_handback() -> &'static str {
    "I've stopped here because I kept planning, validating, and re-checking without \
     actually changing any files — a self-verification loop that won't make progress on its \
     own. The work and findings above are preserved. Rather than keep re-verifying the same \
     thing, I'd like to hand this back: tell me how you'd like to proceed — confirm the \
     approach, point me at what's blocking, or narrow the task — and I'll continue from here."
}

/// The synthetic assistant message appended when a turn stops on an exhausted
/// budget (rather than a natural end), so the turn is well-formed — a user turn
/// is never left with no assistant response — and the cutoff is visible in the
/// transcript and on the headless path. Mirrors [`refusal_surfaced_message`].
/// The verification-treadmill stop gets a CC-style handback instead of the terse
/// budget line, since its fix is user direction, not "continue".
fn budget_exhausted_message(kind: BudgetExhausted, iterations: usize) -> ConversationMessage {
    // Text kept verbatim: this message is the turn's `assistant_messages` tail,
    // so it is what `final_assistant_text` reports as the headless `result`.
    // The "a harness stopped you, not your own judgement" framing belongs in the
    // next turn's reminder ([`turn_budget_continuation_reminder`]), never in
    // output the user reads.
    let text = match kind {
        BudgetExhausted::VerificationTreadmill => verification_treadmill_handback().to_string(),
        _ => budget_exhausted_notice(kind, iterations),
    };
    ConversationMessage::assistant(vec![ContentBlock::Text { text }])
}
pub(super) fn build_turn_end_hook_context(
    summary: &TurnSummary,
    loop_count: usize,
    files_changed: &[String],
    session_goal: Option<&str>,
) -> Value {
    let edit_write_count = summary
        .tool_results
        .iter()
        .filter_map(|message| message.blocks.first())
        .filter(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { tool_name, .. }
                    if is_edit_or_write_tool(tool_name)
            )
        })
        .count();
    let mut context = json!({
        "iterations": summary.iterations,
        "loop_count": loop_count,
        "tool_results": summary.tool_results.len(),
        "edit_write_count": edit_write_count,
        "files_changed_count": files_changed.len(),
        "files_changed": files_changed,
    });
    // Stop-gate fuel: a `TurnEnd` hook judging "is the work done?" needs the
    // standing objective, not just turn mechanics.
    if let Some(goal) = session_goal {
        context["sessionGoal"] = Value::String(goal.to_string());
    }
    context
}
#[cfg(test)]
thread_local! {
    /// Test-only seam: counts [`gate_changed_files`] entries on the *current
    /// thread* (i.e. how many times this turn spawned the `git diff` /
    /// `git ls-files` subprocesses). The spec-literal gate skips the whole probe
    /// when the original request has no candidate backticked literal, so a
    /// non-literal turn must leave this at zero. Thread-local (not a global
    /// atomic) because `run_turn` is synchronous and runs on the calling test's
    /// own thread — so parallel tests never perturb each other's count.
    pub(crate) static GATE_CHANGED_FILES_CALLS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// Async wrapper that runs [`changed_files_snapshot`] on a blocking thread.
///
/// The sync version spawns `git diff` and blocks until it returns. Inside the
/// deep-gate loops it is called on the same `select!` task that drives the TUI
/// render tick, so on a large or index-locked working tree (e.g. a repo with a
/// multi-MB tracked blob) the synchronous `git` spawn froze the spinner/stream
/// mid-turn — the reported "도구 사용 중 멈춤". Offload it for the same reason
/// [`command_is_green`](super::deep_gate) and `bounded_git_diff_for_paths` are
/// offloaded: the turn task yields while git runs, keeping the event loop live.
pub(crate) async fn changed_files_snapshot_async() -> Vec<String> {
    tokio::task::spawn_blocking(changed_files_snapshot)
        .await
        .unwrap_or_default()
}

pub(super) fn changed_files_snapshot() -> Vec<String> {
    let output = std::process::Command::new("git")
        .args([
            "--no-optional-locks",
            "diff",
            "--name-only",
            "HEAD",
            "--",
            ":(exclude)target",
            ":(exclude)node_modules",
            ":(exclude).build",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Files touched this turn for the spec-literal gate: tracked changes
/// (`changed_files_snapshot`) plus newly created untracked files, which
/// `git diff HEAD` omits but a feature task routinely adds (a new module, a
/// scratch repro). `.gitignore` still filters build output.
fn gate_changed_files() -> Vec<String> {
    #[cfg(test)]
    GATE_CHANGED_FILES_CALLS.with(|c| c.set(c.get() + 1));
    let mut files = changed_files_snapshot();
    if let Ok(output) = std::process::Command::new("git")
        .args([
            "--no-optional-locks",
            "ls-files",
            "--others",
            "--exclude-standard",
        ])
        .output()
    {
        if output.status.success() {
            files.extend(
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(ToOwned::to_owned),
            );
        }
    }
    files
}

/// Whether `original` contains at least one *candidate* spec literal — a
/// single-backtick span the spec-literal autopatch could ever act on. This
/// uses the exact candidate filter from `decision-core`, including its rejection
/// of Markdown inline-code references such as `Cart.subtotal()`. When it returns
/// `false`, the autopatch is provably a no-op regardless of what changed on disk,
/// so the git probe ([`gate_changed_files`]) can be skipped entirely.
pub(super) fn original_has_candidate_spec_literals(original: &str) -> bool {
    decision_core::has_candidate_spec_literals(original)
}

/// The deterministic postprocessor must never rewrite tests, fixtures, or
/// golden snapshots. Those files are evidence, not implementation output; in a
/// new/uncommitted repository they all appear in `git ls-files --others` and the
/// old gate could silently alter assertions the user explicitly protected.
fn spec_literal_autopatch_path_allowed(path: &str) -> bool {
    let path = std::path::Path::new(path);
    if path.components().any(|component| {
        matches!(
            component
                .as_os_str()
                .to_string_lossy()
                .to_ascii_lowercase()
                .as_str(),
            "test"
                | "tests"
                | "__tests__"
                | "fixtures"
                | "snapshots"
                | "golden"
                | "testdata"
        )
    }) {
        return false;
    }

    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let file_name = file_name.to_ascii_lowercase();
    !(file_name.starts_with("test_")
        || file_name.contains(".test.")
        || file_name.contains(".spec.")
        || file_name
            .split_once('.')
            .is_some_and(|(stem, _)| stem.ends_with("_test") || stem.ends_with("_spec")))
}

/// Spec-literal self-verify (deterministic auto-patch): for each file changed
/// this turn, if it reproduced a literal the original request spelled out in
/// backticks (e.g. a help marker `` `(DEPRECATED)` ``) with the wrong case
/// (`(Deprecated)`), rewrite it to the exact case on disk and return `true`.
/// Does NOT route through the model: a repair prompt only fixes the casing
/// ~50 % of the time (measured), whereas the marker is a pure substitution.
/// `false` when nothing changed, nothing mismatched, or the workspace is not
/// git-inspectable.
///
/// A free function (not an associated fn): the streaming dispatcher offloads
/// it through `spawn_blocking`, whose `'static` closure must not drag the
/// runtime's `C`/`T` type parameters along.
///
/// [perf] This terminal arm runs on *every* completed turn — both the sync
/// `run_turn` loop and the streaming `run_turn_streaming_maybe_deep`
/// dispatcher. The git probe ([`gate_changed_files`] = `git diff HEAD` +
/// `git ls-files --others`) is the dominant per-turn stall on a dirty repo,
/// and the old `changed.is_empty()` short-circuit ran only *after* both
/// subprocesses had already spawned — so a chatty, non-coding turn paid the
/// full cost every time. The autopatch can only ever repair a backticked spec
/// literal in the request, so when `original` carries no candidate literal we
/// return early *before* touching git
/// ([`original_has_candidate_spec_literals`]): no candidate ⇒ no possible
/// patch ⇒ no reason to inspect the worktree.
pub(super) fn spec_literal_autopatch(original: &str) -> bool {
    if !original_has_candidate_spec_literals(original) {
        return false;
    }
    let changed = gate_changed_files();
    if changed.is_empty() {
        return false;
    }
    let mut patched = false;
    for path in &changed {
        if !spec_literal_autopatch_path_allowed(path) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let mismatches = decision_core::detect_case_mismatched_literals(original, &content);
        if mismatches.is_empty() {
            continue;
        }
        let fixed = decision_core::apply_case_fixes(&content, &mismatches);
        if fixed != content && std::fs::write(path, fixed).is_ok() {
            patched = true;
        }
    }
    patched
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Assemble the [`TurnSummary`] for a completed turn. Shared by the sync
    /// ([`Self::run_turn_once`]) and streaming
    /// ([`Self::run_turn_streaming_with_images`]) loops, which built an identical
    /// record inline — `turn_output_tokens` is the in-turn output-token delta
    /// (cumulative-now minus the `turn_start_output_tokens` baseline captured at
    /// turn start). `deep_verification`/`verification_issues` start empty; the
    /// deep gate fills them later when it runs.
    /// Append the synthetic budget-exhausted closer to the session, record it as
    /// this turn's terminal assistant iteration, and mirror it into
    /// `assistant_messages`. Shared by the sync and streaming budget seams; the
    /// caller maps the session-push error into its own turn-error type. When a
    /// budget check trips, the prior iteration has already closed the session
    /// well-formed (it ends on a `user` tool-result / input message), so
    /// appending an assistant closer keeps user/assistant alternation valid
    /// without any rollback — the whole point of preserving the work.
    pub(super) fn push_budget_exhausted_closer(
        &mut self,
        kind: BudgetExhausted,
        iterations: usize,
        assistant_messages: &mut Vec<ConversationMessage>,
    ) -> Result<(), String> {
        let message = budget_exhausted_message(kind, iterations);
        // Arm the next turn's continuation reminder: this closer is now in the
        // transcript under the assistant role, and the following turn must read
        // it as a harness cutoff rather than as its own give-up.
        self.budget_closer_pending = true;
        self.record_assistant_iteration(iterations, &message, 0);
        self.session
            .push_message(message)
            .map_err(|error| error.to_string())?;
        if let Some(msg) = self.session.messages.last().cloned() {
            assistant_messages.push(msg);
        }
        Ok(())
    }
}

#[cfg(test)]
mod spec_literal_path_tests {
    use super::spec_literal_autopatch_path_allowed;

    #[test]
    fn autopatch_excludes_test_evidence_paths() {
        for path in [
            "tests/test_checkout.py",
            "src/widget_test.rs",
            "web/cart.spec.ts",
            "fixtures/expected.py",
            "snapshots/output.snap",
        ] {
            assert!(
                !spec_literal_autopatch_path_allowed(path),
                "test evidence must stay immutable: {path}"
            );
        }
        assert!(spec_literal_autopatch_path_allowed("checkout/service.py"));
        assert!(spec_literal_autopatch_path_allowed("src/spec_parser.rs"));
    }
}
