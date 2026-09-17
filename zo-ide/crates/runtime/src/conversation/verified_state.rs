//! Verified-state observation: the conversation-layer half of
//! [`crate::verified_state`].
//!
//! Three seams, all on paths the sync and streaming loops already share:
//!
//! * **record** — [`ConversationRuntime::record_verified_state_from_tool`] runs
//!   at `record_tool_finished`, folding each settled tool result into the
//!   session ledger as it happens (which write-throughs to its sidecar, so the
//!   next stage's process sees them). `record_turn_completed` only closes the
//!   turn counter ([`ConversationRuntime::note_verified_state_turn_boundary`]);
//!   it records no facts, so nothing can be counted twice.
//! * **observe** — [`ConversationRuntime::inject_verified_state_reminder`] runs
//!   at turn start next to the other input-triggered reminders, so the
//!   observation lands where the model PLANS the stage.
//! * **cite** — [`ConversationRuntime::verify_leg_session_checks`] hands the
//!   deep gate's VERIFY leg the session checks that are still fresh, joining
//!   them to this attempt's own `exec_green_checks`.
//!
//! That placement is the whole lesson of the reverted attempt (`0c39f26`),
//! which appended a "you already saved this file" note to the tail of tool
//! RESULTS and measured zero adoption in a paid run: by the time a result is
//! read the plan that will re-verify has already been made. Turn start is the
//! only moment the observation can change what the model decides to do.
//!
//! ## Why the classifier is shared, not re-derived
//!
//! `exec_green_checks` already owns "what counts as a check that ran green",
//! including the bash success landmine (`returnCodeInterpretation` absent =
//! success, `interrupted: false`, no `backgroundTaskId`). A second definition
//! here would drift, and the drift would be invisible — both surfaces report
//! the same class of fact. So the vocabulary stays in `deep_gate` and this
//! module only differs where the SEMANTICS differ: `exec_green_checks` DROPS
//! every check that predates the turn's last edit (stale evidence for a
//! verifier judging one diff), whereas the session ledger KEEPS it and reports
//! the edit delta instead — a check invalidated by a later edit is exactly the
//! fact worth telling the planner.
//!
//! ## Joining the session ledger into the VERIFY leg
//!
//! The deep-gate VERIFY prompt reports `exec_green_checks` — THIS attempt's
//! post-edit greens. The session ledger's still-fresh greens now join them, and
//! the seam move above is what makes that honest rather than merely useful.
//! While recording happened at turn end, a mid-turn reader saw a ledger that
//! did not yet know about the edits the turn had just made, so a session check
//! rendered into a VERIFY prompt could assert "nothing edited since" about a
//! tree the EXEC leg had just changed — a harness-owned IO fact that was false.
//! Recording at the tool-result seam does not make that unlikely, it makes it
//! unrepresentable: EXEC's edits are in the ledger before the leg is built, so
//! every check they invalidate is already excluded by
//! [`crate::verified_state::VerifiedStateLedger::fresh_green_checks`].
//!
//! Only fresh checks join. An invalidated one is genuinely useful at turn start
//! — the planner can weigh "this ran green BUT these files changed" against
//! what it knows the check covers — but a verifier is handed facts to CITE, and
//! a fact it must qualify for itself is one it can only be misled by.
//!
//! The join is therefore narrow ON PURPOSE, and its firing rate is the thing
//! worth watching rather than assuming: since this attempt's edits invalidate
//! everything older, the leg gains a session line only where the ledger knows
//! something this turn's summary does not — a retry attempt that edited nothing
//! since the previous attempt's green run, a leg whose turn ran checks without
//! edits, and fresh checks that fell off `exec_green_checks`' newest-3 cap.
//! Hence [`telemetry::HarnessFeature::VerifiedStateVerifyLeg`], which counts
//! those separately from the turn-start observation.
//!
//! The standing turn-start observation is still RETIRED at the turn's first
//! successful mutation
//! ([`ConversationRuntime::retire_verified_state_observation_on_mutation`]):
//! the block's TEXT was rendered at turn start and does not re-render, so it
//! ages even though the ledger behind it no longer does.

use super::deep_gate::{
    bash_result_exited_zero, command_is_check_shaped, tool_result_path, truncate_on_boundary,
    EXEC_CHECK_COMMAND_BYTES,
};
use super::{
    ApiClient, ContentBlock, ConversationMessage, ConversationRuntime, ToolExecutor,
};
use crate::verified_state::{VerifiedStateEvent, VERIFIED_STATE_REMINDER_PREFIX};

/// One settled tool result's ledger-worthy facts.
///
/// Usually zero or one: a result message carries a single `ToolResult` block
/// (plus any images), but the blocks are walked rather than indexed so a
/// multi-block result cannot silently drop a fact.
///
/// `tool_input` is the EFFECTIVE input — what the executor actually ran after
/// `PreToolUse` hooks had their say — which is a strictly better source than
/// the `tool_use` block the turn-end fold used to join by id: a hook that
/// rewrites a command changes what ran, and the ledger must record the command
/// whose exit 0 was observed, not the one that was proposed.
fn tool_verified_state_events(
    result_message: &ConversationMessage,
    tool_input: &str,
) -> Vec<VerifiedStateEvent> {
    let mut events: Vec<VerifiedStateEvent> = Vec::new();
    for block in &result_message.blocks {
        let ContentBlock::ToolResult {
            tool_name,
            output,
            is_error,
            ..
        } = block
        else {
            continue;
        };
        if *is_error {
            continue;
        }
        if super::is_edit_or_write_tool(tool_name) {
            if let Some(path) = tool_result_path(output) {
                events.push(VerifiedStateEvent::Edit(path));
            }
            continue;
        }
        if tool_name != "bash" || !bash_result_exited_zero(output) {
            continue;
        }
        if let Some(command) = green_check_command(tool_input) {
            events.push(VerifiedStateEvent::GreenCheck(command));
        }
    }
    events
}

/// The check-shaped command inside a bash tool input, capped for storage.
/// `None` for anything that is not a check (`ls -la` is green every time and
/// proves nothing) or whose input did not parse.
fn green_check_command(input: &str) -> Option<String> {
    let mut command = serde_json::from_str::<serde_json::Value>(input)
        .ok()?
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)?;
    if !command_is_check_shaped(&command) {
        return None;
    }
    truncate_on_boundary(&mut command, EXEC_CHECK_COMMAND_BYTES);
    Some(command)
}

/// Session checks reported to ONE VERIFY leg (newest kept). Mirrors
/// `EXEC_CHECK_MAX_COMMANDS`, the cap this attempt's own observations use: the
/// leg's objective section stays a handful of quotable lines whichever source
/// they came from.
const SESSION_CHECK_MAX_COMMANDS: usize = 3;

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Fold one settled tool result into the session ledger, as it settles.
    ///
    /// Called from `record_tool_finished`, which BOTH turn loops reach for every
    /// tool result on every path — including a deep-lane sub-turn's, so an EXEC
    /// leg's edits are on record before the VERIFY leg that follows it is even
    /// built. That immediacy is the whole point: see the module docs.
    ///
    /// This is the ONLY place facts enter the ledger. The turn-end fold that
    /// used to do it is gone rather than kept as a backstop — a second writer
    /// replaying the same turn would record every check twice, and while the
    /// ledger's newest-wins dedupe would hide the duplicate rows, it would not
    /// hide the sequence numbers they burned: a re-recorded check would jump
    /// ahead of the edits that had invalidated it and read as fresh again.
    pub(super) fn record_verified_state_from_tool(
        &mut self,
        result_message: &ConversationMessage,
        tool_input: &str,
    ) {
        for event in tool_verified_state_events(result_message, tool_input) {
            self.verified_state.record_event(&event);
        }
    }

    /// Close the turn in the ledger. Advances its turn counter and nothing else
    /// — every fact this turn produced was recorded when it happened.
    pub(super) fn note_verified_state_turn_boundary(&mut self) {
        self.verified_state.note_turn_boundary();
    }

    /// The session-ledger green checks a VERIFY leg may cite, given what this
    /// attempt already observed for itself.
    ///
    /// Fresh only, and never a command `exec_checks` already reports: the two
    /// surfaces truncate identically (`EXEC_CHECK_COMMAND_BYTES`), so a repeat
    /// is exact string equality, and printing the same command under two
    /// headings would read as two independent runs.
    ///
    /// Returns empty — leaving the prompt byte-identical — whenever there is
    /// nothing safe to add, and attests WHICH kind of nothing it was, because
    /// "no fresh check exists" and "every fresh check is already in this
    /// attempt's own list" are the same silence with very different meanings.
    pub(super) fn verify_leg_session_checks(&self, exec_checks: &[String]) -> Vec<String> {
        let fresh = self
            .verified_state
            .fresh_green_checks(SESSION_CHECK_MAX_COMMANDS + exec_checks.len());
        if fresh.is_empty() {
            telemetry::attest_declined(
                telemetry::HarnessFeature::VerifiedStateVerifyLeg,
                "no_fresh_session_checks",
            );
            return Vec::new();
        }
        let mut joined: Vec<String> = fresh
            .into_iter()
            .filter(|command| !exec_checks.iter().any(|exec| exec == command))
            .map(str::to_string)
            .collect();
        if joined.len() > SESSION_CHECK_MAX_COMMANDS {
            joined.drain(..joined.len() - SESSION_CHECK_MAX_COMMANDS);
        }
        if joined.is_empty() {
            telemetry::attest_declined(
                telemetry::HarnessFeature::VerifiedStateVerifyLeg,
                "already_reported_by_this_attempt",
            );
            return Vec::new();
        }
        // Checked after the content gate, like the turn-start injection: an
        // ablation decline must count the legs that would really have carried a
        // session line, not every leg that ran.
        if telemetry::attest_ablated(telemetry::HarnessFeature::VerifiedStateVerifyLeg) {
            return Vec::new();
        }
        telemetry::attest_fired(telemetry::HarnessFeature::VerifiedStateVerifyLeg);
        joined
    }

    /// Install this turn's verified-state observation, or nothing at all.
    ///
    /// Turn-scoped like the other input-triggered reminders: cleared at turn
    /// start by `clear_turn_start_transient_reminders`, re-armed here. It rides
    /// the transient-reminder path, so it is absorbed into the transcript as a
    /// trailing `System` message at request-build time
    /// (`absorb_wire_reminders_into_session`) — an APPEND behind the new user
    /// message, never a rewrite of the cached prefix.
    ///
    /// A ledger with no green check renders `None` and installs nothing, which
    /// is the byte-neutrality guarantee: the feature costs zero until the
    /// session has actually verified something.
    pub(super) fn inject_verified_state_reminder(&mut self) {
        let root = self.trace_cwd();
        let Some(body) = self.verified_state.render_observation(root.as_deref()) else {
            // A distinct decline from the ablation one below: "this session has
            // verified nothing yet" is normal operation, while a held-out arm
            // means a block WAS available and an experiment suppressed it.
            // Conflating them is how a never-firing feature reads as idle.
            telemetry::attest_declined(
                telemetry::HarnessFeature::VerifiedStateObservation,
                "no_green_checks",
            );
            return;
        };
        // Checked after the content gate so an ablation decline counts turns
        // that would really have carried an observation.
        if telemetry::attest_ablated(telemetry::HarnessFeature::VerifiedStateObservation) {
            return;
        }
        telemetry::attest_fired(telemetry::HarnessFeature::VerifiedStateObservation);
        self.replace_transient_system_reminder_by_prefix(
            VERIFIED_STATE_REMINDER_PREFIX,
            Some(&body),
        );
    }

    /// Stop re-asserting the observation once this turn has actually mutated
    /// the workspace.
    ///
    /// The ledger settles at turn end, so mid-turn the standing block cannot
    /// know about the edit that just happened — and the reminder absorber
    /// re-appends a standing set near the tail when it drifts past its dedupe
    /// window, which would place a stale "nothing edited since" BELOW the edits
    /// that falsified it. Two places would have read that lie: the model's next
    /// iteration, and a deep-lane VERIFY leg (an internal subturn, which by
    /// design does not re-run the turn-start injection).
    ///
    /// Retiring rather than re-rendering keeps the feature's cost honest: a
    /// mid-turn refresh would append a fresh block after every mutating batch,
    /// which is exactly the automatic token burn this feature is not allowed to
    /// become. The copy persisted at turn-start position is left alone — it was
    /// true where it sits, and the model reads the turn's edits after it.
    pub(super) fn retire_verified_state_observation_on_mutation(
        &mut self,
        message: &ConversationMessage,
    ) {
        let mutated = message.blocks.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { tool_name, is_error, .. }
                    if !is_error && super::is_edit_or_write_tool(tool_name)
            )
        });
        if mutated {
            self.replace_transient_system_reminder_by_prefix(VERIFIED_STATE_REMINDER_PREFIX, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::VERIFIED_STATE_REMINDER_PREFIX;
    use crate::conversation::{
        ApiClient, ApiRequest, AssistantEvent, ConversationRuntime, RuntimeError,
        StaticToolExecutor,
    };
    use crate::permissions::{PermissionMode, PermissionPolicy};
    use crate::session::{ConversationMessage, Session};

    /// A client that never talks to a provider: every test here drives the
    /// record/observe seams directly, so the turn loop is deliberately absent.
    struct SilentApiClient;

    impl ApiClient for SilentApiClient {
        fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
            Ok(vec![AssistantEvent::MessageStop])
        }
    }

    type TestRuntime = ConversationRuntime<SilentApiClient, StaticToolExecutor>;

    fn static_runtime() -> TestRuntime {
        ConversationRuntime::new(
            Session::new(),
            SilentApiClient,
            StaticToolExecutor::new(),
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
    }

    /// Settle one bash result through the seam BOTH turn loops use, exactly as
    /// they do: the effective input carries the command, the result carries the
    /// exit shape.
    fn settle_bash(runtime: &mut TestRuntime, id: &str, command: &str, output: &str) {
        runtime.record_tool_finished(
            1,
            &ConversationMessage::tool_result(id, "bash", output, false),
            &serde_json::json!({ "command": command }).to_string(),
        );
    }

    /// A green foreground exit: `stdout` present, no `returnCodeInterpretation`,
    /// not interrupted, not backgrounded.
    fn settle_green_bash(runtime: &mut TestRuntime, id: &str, command: &str) {
        settle_bash(runtime, id, command, r#"{"stdout":"ok"}"#);
    }

    fn settle_edit(runtime: &mut TestRuntime, id: &str, path: &str, is_error: bool) {
        runtime.record_tool_finished(
            1,
            &ConversationMessage::tool_result(
                id,
                "edit_file",
                serde_json::json!({ "filePath": path }).to_string(),
                is_error,
            ),
            &serde_json::json!({ "path": path }).to_string(),
        );
    }

    /// The classifier keeps the same bash-success landmine as
    /// `exec_green_checks` (non-zero / interrupted / backgrounded are not
    /// green, non-check shapes are not evidence) but — unlike it — does NOT
    /// drop a check that a later edit invalidated: it records both, in arrival
    /// order, and lets the ledger report the delta.
    #[test]
    fn settled_results_record_green_checks_and_edits_in_arrival_order() {
        let mut runtime = static_runtime();
        settle_green_bash(&mut runtime, "pre", "cargo build --tests");
        settle_edit(&mut runtime, "edit-1", "/repo/src/main.rs", false);
        settle_green_bash(&mut runtime, "post", "cargo test -p runtime");
        settle_bash(
            &mut runtime,
            "red",
            "cargo test broken",
            r#"{"stdout":"","returnCodeInterpretation":"exit_code:101"}"#,
        );
        settle_bash(
            &mut runtime,
            "bg",
            "cargo test --workspace",
            r#"{"stdout":"started","backgroundTaskId":"t1"}"#,
        );
        settle_bash(&mut runtime, "noise", "ls -la", r#"{"stdout":"x"}"#);
        // A failed edit changed nothing and must not invalidate anything.
        settle_edit(&mut runtime, "edit-2", "/repo/src/other.rs", true);

        assert_eq!(
            runtime.verified_state.green_check_commands(),
            vec!["cargo build --tests", "cargo test -p runtime"],
            "only foreground exit-0 check shapes are evidence"
        );
        let block = runtime
            .verified_state
            .render_observation(None)
            .expect("observation");
        assert!(
            block.contains(
                "`cargo build --tests` — ran green BUT these files changed since: \
                 /repo/src/main.rs"
            ),
            "{block}"
        );
        assert!(
            block.contains("`cargo test -p runtime` — ran green, nothing edited since"),
            "the post-edit check is still fresh: {block}"
        );
        assert!(
            !block.contains("/repo/src/other.rs"),
            "a failed edit must not appear at all: {block}"
        );
    }

    /// (a) Record in one turn, observe at the next turn's start — the FRESH
    /// form, wired end to end through the runtime rather than the ledger alone.
    #[test]
    fn a_recorded_green_check_is_injected_at_the_next_turn_start() {
        let mut runtime = static_runtime();
        settle_green_bash(&mut runtime, "c1", "cargo test -p runtime");
        runtime.note_verified_state_turn_boundary();

        runtime.clear_turn_start_transient_reminders();
        runtime.inject_verified_state_reminder();
        let reminders = runtime.transient_reminders.join("\n");
        assert!(reminders.contains(VERIFIED_STATE_REMINDER_PREFIX), "{reminders}");
        assert!(
            reminders.contains("`cargo test -p runtime` — ran green, nothing edited since"),
            "{reminders}"
        );

        // Turn-scoped: the next turn's clear drops it, and it never accumulates.
        runtime.clear_turn_start_transient_reminders();
        assert!(!runtime
            .transient_reminders
            .join("\n")
            .contains(VERIFIED_STATE_REMINDER_PREFIX));
        runtime.inject_verified_state_reminder();
        runtime.inject_verified_state_reminder();
        assert_eq!(
            runtime
                .transient_reminders
                .iter()
                .filter(|reminder| reminder.starts_with(VERIFIED_STATE_REMINDER_PREFIX))
                .count(),
            1
        );
    }

    /// (b) An edit recorded after the check surfaces as the invalidated form,
    /// naming the changed file.
    #[test]
    fn an_edit_after_the_check_surfaces_as_invalidated_with_the_path() {
        let mut runtime = static_runtime();
        settle_green_bash(&mut runtime, "c1", "cargo test -p runtime");
        runtime.note_verified_state_turn_boundary();
        settle_edit(&mut runtime, "e1", "/repo/crates/runtime/src/a.rs", false);
        runtime.note_verified_state_turn_boundary();

        runtime.inject_verified_state_reminder();
        let reminders = runtime.transient_reminders.join("\n");
        assert!(
            reminders.contains(
                "ran green BUT these files changed since: /repo/crates/runtime/src/a.rs"
            ),
            "{reminders}"
        );
    }

    /// (c) Byte neutrality: with no green check on record the turn start
    /// installs nothing, so the request is byte-identical to one built without
    /// this feature — including on a turn that only edited files.
    #[test]
    fn without_a_green_check_the_turn_start_is_byte_neutral() {
        let mut runtime = static_runtime();
        let before = runtime.transient_reminders.clone();
        runtime.inject_verified_state_reminder();
        assert_eq!(runtime.transient_reminders, before);

        settle_edit(&mut runtime, "e1", "/repo/a.rs", false);
        runtime.note_verified_state_turn_boundary();
        runtime.inject_verified_state_reminder();
        assert_eq!(
            runtime.transient_reminders, before,
            "an edit-only session must stay byte-neutral"
        );
    }

    /// A standing observation is retired the moment the turn mutates anything,
    /// so the absorber's tail re-anchor can never re-assert "nothing edited
    /// since" below the edits that falsified it. A FAILED edit changed nothing
    /// and must leave the observation standing.
    #[test]
    fn a_successful_mutation_retires_the_standing_observation() {
        let mut runtime = static_runtime();
        settle_green_bash(&mut runtime, "c1", "cargo test -p runtime");
        runtime.note_verified_state_turn_boundary();
        runtime.inject_verified_state_reminder();
        assert!(runtime
            .transient_reminders
            .join("\n")
            .contains(VERIFIED_STATE_REMINDER_PREFIX));

        // A read is not a mutation; a failed edit is not one either.
        runtime.record_tool_finished(
            1,
            &ConversationMessage::tool_result("r1", "read_file", r#"{"path":"/repo/a.rs"}"#, false),
            r#"{"path":"/repo/a.rs"}"#,
        );
        settle_edit(&mut runtime, "e0", "/repo/a.rs", true);
        assert!(
            runtime
                .transient_reminders
                .join("\n")
                .contains(VERIFIED_STATE_REMINDER_PREFIX),
            "only a SUCCESSFUL mutation may retire the observation"
        );

        settle_edit(&mut runtime, "e1", "/repo/a.rs", false);
        assert!(
            !runtime
                .transient_reminders
                .join("\n")
                .contains(VERIFIED_STATE_REMINDER_PREFIX),
            "a successful edit must stop the observation being re-asserted"
        );
    }

    /// (d) Sidecar round trip THROUGH the runtime seam: a stage records, a
    /// freshly built runtime rebinds the same sidecar, and the observation
    /// survives the process boundary. This is the multi-stage bench case.
    #[test]
    fn the_observation_survives_a_runtime_rebuild_via_the_sidecar() {
        let dir = std::env::temp_dir().join(format!(
            "zo-verified-state-runtime-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let sidecar = dir.join("session.verified-state.json");

        let mut stage_one = static_runtime();
        stage_one.reset_verified_state_session(Some(sidecar.clone()));
        settle_green_bash(&mut stage_one, "c1", "cargo test -p runtime");
        // Deliberately NO turn boundary: recording write-throughs as it happens,
        // so a stage killed mid-turn still hands its greens to the next process.
        assert!(sidecar.exists(), "the seam must write through immediately");

        // A brand-new runtime — the stage-2 process — sees it only because the
        // ledger was rebound to the sidecar.
        let mut stage_two = static_runtime();
        assert!(
            stage_two.verified_state.render_observation(None).is_none(),
            "an unbound runtime starts empty"
        );
        stage_two.reset_verified_state_session(Some(sidecar));
        stage_two.inject_verified_state_reminder();
        assert!(
            stage_two
                .transient_reminders
                .join("\n")
                .contains("cargo test -p runtime"),
            "the sidecar must carry the ledger across the stage boundary"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// **The reason the join was deferred, now a test.**
    ///
    /// A previous turn ran a check green. THIS turn's EXEC edits a file and the
    /// VERIFY leg opens immediately — mid-turn, before any turn boundary. The
    /// leg must be handed NOTHING, because "nothing edited since" stopped being
    /// true the moment that edit settled.
    ///
    /// Under the turn-end fold this test could not pass: the ledger would not
    /// learn about the edit until `record_turn_completed`, which is after the
    /// leg is built, so the stale check would have been quoted to the verifier
    /// as a harness-owned IO fact. Recording at the seam is what makes it fail
    /// to be quotable rather than merely unlikely to be.
    #[test]
    fn an_edit_this_turn_withholds_the_stale_check_from_a_mid_turn_verify_leg() {
        let mut runtime = static_runtime();
        settle_green_bash(&mut runtime, "c1", "cargo test -p runtime");
        runtime.note_verified_state_turn_boundary();
        assert_eq!(
            runtime.verify_leg_session_checks(&[]),
            vec!["cargo test -p runtime".to_string()],
            "before this turn edits anything the check is quotable"
        );

        // EXEC edits. No turn boundary — the turn is still in flight, which is
        // exactly when a deep-lane VERIFY leg opens.
        settle_edit(&mut runtime, "e1", "/repo/crates/runtime/src/a.rs", false);
        assert!(
            runtime.verify_leg_session_checks(&[]).is_empty(),
            "a check the turn's own edits invalidated must never reach the leg"
        );

        // The turn-start observation still reports it — with the delta — because
        // that surface exists to let the PLANNER judge coverage. The leg gets
        // facts to cite; the planner gets facts to weigh.
        let block = runtime
            .verified_state
            .render_observation(None)
            .expect("observation");
        assert!(
            block.contains(
                "ran green BUT these files changed since: /repo/crates/runtime/src/a.rs"
            ),
            "{block}"
        );
    }

    /// The join never repeats a command this attempt already reports for itself:
    /// the same run printed under two headings would read as two runs.
    #[test]
    fn the_leg_join_drops_commands_this_attempt_already_reported() {
        let mut runtime = static_runtime();
        settle_green_bash(&mut runtime, "c1", "cargo test -p runtime");
        settle_green_bash(&mut runtime, "c2", "cargo clippy -p runtime");
        runtime.note_verified_state_turn_boundary();

        assert_eq!(
            runtime.verify_leg_session_checks(&["cargo test -p runtime".to_string()]),
            vec!["cargo clippy -p runtime".to_string()],
            "only the commands the attempt did NOT report join"
        );
        assert!(
            runtime
                .verify_leg_session_checks(&[
                    "cargo test -p runtime".to_string(),
                    "cargo clippy -p runtime".to_string(),
                ])
                .is_empty(),
            "a fully-covered ledger adds nothing, leaving the prompt byte-identical"
        );
    }

    /// The leg cap is small and newest-wins, whatever the ledger holds.
    #[test]
    fn the_leg_join_is_capped_and_keeps_the_newest() {
        let mut runtime = static_runtime();
        for index in 0..8 {
            settle_green_bash(&mut runtime, &format!("c{index}"), &format!("cargo test case{index}"));
        }
        runtime.note_verified_state_turn_boundary();
        assert_eq!(
            runtime.verify_leg_session_checks(&[]),
            vec![
                "cargo test case5".to_string(),
                "cargo test case6".to_string(),
                "cargo test case7".to_string(),
            ]
        );
    }
}
