//! Turn-end gate: the harness-side "check your last paragraph" lint.
//!
//! The system prompt's turn discipline asks the model to do promised work
//! before ending the turn; strong models comply, weaker ones end on
//! "I'll do X next." and hand the promise back to the user. This module makes
//! the contract enforceable at the only place it can be observed — the natural
//! end of a streaming turn (no tool calls in the final assistant message):
//! when the reply's last paragraph is a promise of future work, or (on an
//! autonomous surface) a question nobody is present to answer, the loop
//! re-prompts with a bounded reminder instead of ending the turn.
//!
//! Deterministic and conservative by design: a missed promise costs the user
//! one manual nudge, while a false positive costs a whole extra model
//! iteration — so the marker lists are tight, and every "waiting on the user"
//! phrasing is an explicit skip. Complements (never replaces) the budget
//! breakers: the gate only ever runs at a natural end, so a turn stopped by
//! deadline/token/treadmill closers is never re-prompted.

/// Why the gate wants one more iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnEndingIssue {
    /// The final paragraph promises work the model has not done ("I'll …").
    Promise,
    /// The final paragraph is a question, and nobody can answer mid-run.
    Question,
    /// The reply ends on a tool call written as text — Gemini's
    /// `call:default_api:read_file{…}` / `print(default_api.read_file(…))`,
    /// an open model's `<tool_call>` — so nothing ran and the model believes
    /// it is waiting on a result that will never come. Seen live: a turn that
    /// ended on `…to understand why X wasn't set.call:default_api:read_file{…}`
    /// with the work half done.
    TextualToolCall,
    /// The final paragraph reports the work as done — or passing — while the
    /// verified-state ledger shows edits this turn that no check ran green
    /// after, and the reply does not say so. A completion without evidence is
    /// asked for the check, or for the honest caveat.
    UnverifiedCompletion,
}

/// Fragments by which a final paragraph reports the work as done or passing.
/// Screened only when the ledger already shows unverified edits this turn, so
/// the breadth costs nothing on a turn that changed no code.
const COMPLETION_CLAIM_MARKERS: [&str; 26] = [
    " done", "complete", "finished", "implemented", "landed", "shipped", "fixed", "ready",
    "works now", "passes", "passing", "all pass", "green", "verified", "완료", "끝났", "마쳤",
    "구현했", "고쳤", "수정했", "통과", "초록", "검증했", "확인했", "동작합니다", "됩니다",
];

/// Fragments that admit the check was not run — or that there is none to
/// run — anywhere in the reply, since an honest caveat counts wherever it is
/// written. "There is no test runner here" is as honest as "I did not run
/// it": the gate asks for the truth about verification, not for a test.
const UNVERIFIED_ADMISSION_MARKERS: [&str; 26] = [
    "did not run", "didn't run", "not run", "not verified", "unverified", "have not tested",
    "haven't tested", "not tested", "untested", "without running", "could not run", "no test",
    "no check", "no automated", "nothing to run", "no gate", "돌리지 않", "돌려보지 않", "실행하지 않",
    "실행 안", "검증하지 않", "미검증", "테스트하지 않", "테스트가 없", "검사가 없", "돌릴 것이 없",
];

/// Whether `paragraph` reports the work as done or passing.
#[must_use]
pub(super) fn claims_completion(paragraph: &str) -> bool {
    let lower = paragraph.to_lowercase();
    COMPLETION_CLAIM_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// Whether the reply admits, anywhere, that the check was not run.
#[must_use]
pub(super) fn admits_unverified(text: &str) -> bool {
    let lower = text.to_lowercase();
    UNVERIFIED_ADMISSION_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// The reply reports completion while `unverified_edits` (this turn's edits
/// no green check followed) is non-empty, without admitting it.
#[must_use]
pub(super) fn reports_unverified_completion(final_text: &str, unverified_edits: &[&str]) -> bool {
    !unverified_edits.is_empty()
        && claims_completion(last_paragraph(final_text))
        && !admits_unverified(final_text)
}

/// Files listed on the reminder before it summarizes.
const REMINDER_MAX_EDITS: usize = 6;

/// The re-prompt for [`TurnEndingIssue::UnverifiedCompletion`], naming the
/// edits and the project's check so the model can act on it in one call.
#[must_use]
pub(super) fn unverified_completion_reminder(edits: &[&str], check: Option<&str>) -> String {
    let listed = edits
        .iter()
        .copied()
        .take(REMINDER_MAX_EDITS)
        .collect::<Vec<&str>>()
        .join(", ");
    let files = match edits.len().saturating_sub(REMINDER_MAX_EDITS) {
        0 => listed,
        more => format!("{listed} and {more} more"),
    };
    let check = check.map_or_else(
        || "the project's own check (its test runner, build, or gate recipe)".to_string(),
        |command| format!("`{command}` or the project's own gate"),
    );
    format!(
        "[zo:turn-end-gate] <system-reminder>Your reply reports the work as done, but no check ran green after your edits this turn ({files}). A completion without evidence is not a completion. Either run {check} now and report its actual result, or say plainly in the final answer that it was not run and why.</system-reminder>"
    )
}

/// One transcript line receipting a turn that edited and then checked: the
/// harness's own record that `commands` exited 0 after the last edit.
#[must_use]
pub(super) fn completion_receipt(commands: &[&str]) -> String {
    let listed: Vec<String> = commands
        .iter()
        .map(|command| format!("{} ✓", crate::verified_state::first_line(command)))
        .collect();
    format!("[receipt] green after the last edit: {}", listed.join(" · "))
}

/// The shapes a tool call takes when written as prose rather than issued:
/// each form's opening marker paired with the closer its call ends on.
/// Tight on purpose: `default_api` followed by `.` or `:` is the leaked
/// function-calling namespace, never a word a model uses in an answer.
/// `call:default_api:name{…}` closes on `}`, `print(default_api.name(…))`
/// on `)`, the fenced forms on their fence, the tag forms on their tag.
const TEXTUAL_TOOL_CALL_FORMS: [(&str, &str); 6] = [
    ("default_api.", ")"),
    ("default_api:", "}"),
    ("<tool_call>", "</tool_call>"),
    ("<function_call>", "</function_call>"),
    ("```tool_code", "```"),
    ("```tool_call", "```"),
];

/// How far before the end a marker still counts as the ending without any
/// further reading — the call the model "made" is the last thing it wrote,
/// and a marker quoted deep in a long answer is not that.
const TEXTUAL_TOOL_CALL_TAIL_BYTES: usize = 800;

/// Does the reply end on a tool call written as text? The last marker of any
/// form is the candidate. Inside the tail window it is the ending outright.
/// Beyond it — a long argument (a multi-line script) pushes the marker far
/// back while the call is still the last thing written — it is the ending
/// when the reply closes on that form's own closer; a marker quoted early and
/// followed by prose does not.
pub(super) fn ends_on_textual_tool_call(final_text: &str) -> bool {
    let trimmed = final_text.trim_end();
    let Some((marker_at, closer)) = last_textual_tool_call_marker(trimmed) else {
        return false;
    };
    trimmed.len() - marker_at <= TEXTUAL_TOOL_CALL_TAIL_BYTES || trimmed.ends_with(closer)
}

/// Byte offset of the last textual-call marker in `text` and the closer of
/// the form it opens, or `None` when no form appears.
fn last_textual_tool_call_marker(text: &str) -> Option<(usize, &'static str)> {
    TEXTUAL_TOOL_CALL_FORMS
        .iter()
        .filter_map(|(marker, closer)| text.rfind(marker).map(|at| (at, *closer)))
        .max_by_key(|(at, _)| *at)
}

/// Hard cap on gate re-prompts per turn, so a model that keeps promising
/// (or genuinely needs the user) cannot be looped indefinitely.
pub(super) const TURN_END_GATE_MAX_REPROMPTS: usize = 2;

/// Gate level from `ZO_TURN_END_GATE`: `0` disables, `1` (default) lints
/// promise endings everywhere and question endings on autonomous surfaces,
/// `2` also lints question endings in interactive sessions. Read per turn
/// (not memoized) so an operator can retune without a rebuild — the
/// escape-hatch idiom of `ZO_MAX_ITERATIONS`.
pub(super) fn env_turn_end_gate_level() -> u8 {
    std::env::var("ZO_TURN_END_GATE")
        .ok()
        .and_then(|raw| raw.trim().parse::<u8>().ok())
        .unwrap_or(1)
}

/// Promise markers: first-person future commitments. Matched against the last
/// paragraph only — a promise earlier in the reply followed by a completed
/// summary is fine.
const PROMISE_MARKERS: [&str; 12] = [
    "I'll ",
    "I will ",
    "I'm going to ",
    "Let me now ",
    "Next, I'll",
    "Next I'll",
    "하겠습니다",
    "해보겠습니다",
    "할게요",
    "진행하겠",
    "시작하겠",
    "할 예정입니다",
];

/// Phrasings that mean the model is legitimately stopping or handing the
/// decision to the user. Any of these in the last paragraph suppresses the
/// promise lint — ending on "I'll wait for your decision" is the *correct*
/// blocked-on-user ending, not a broken promise.
const WAITING_ON_USER_MARKERS: [&str; 16] = [
    "I'll wait",
    "I'll stop",
    "I'll hold",
    "I'll leave",
    "let me know",
    "your call",
    "if you want",
    "if you'd like",
    "once you confirm",
    "알려주세요",
    "알려 주세요",
    "말씀해 주세요",
    "말씀해주세요",
    "확인해 주세요",
    "기다리겠",
    "중단하겠",
];

/// The last paragraph of a reply: the text after the final blank line,
/// trimmed. Falls back to the whole (trimmed) text when there is no blank
/// line, so single-paragraph replies are still screened.
fn last_paragraph(text: &str) -> &str {
    let trimmed = text.trim_end();
    trimmed
        .rsplit("\n\n")
        .next()
        .unwrap_or(trimmed)
        .trim()
}

/// Screen a naturally-ending reply's final paragraph. `lint_questions` is true
/// when nobody can answer a mid-run question (autonomous surface, or gate
/// level 2). Returns `None` when the ending is fine.
pub(super) fn screen_turn_ending(
    final_text: &str,
    lint_questions: bool,
) -> Option<TurnEndingIssue> {
    let paragraph = last_paragraph(final_text);
    if paragraph.is_empty() {
        // Empty replies belong to the empty-stream retry machinery.
        return None;
    }
    // Before every other reading of the ending: a reply that ends on a tool
    // call written as text is not a reply at all, whatever it says before.
    if ends_on_textual_tool_call(final_text) {
        return Some(TurnEndingIssue::TextualToolCall);
    }
    if lint_questions
        && paragraph
            .trim_end_matches(['*', '`', ')', ']', '"', '\''])
            .ends_with(['?', '？'])
    {
        return Some(TurnEndingIssue::Question);
    }
    if WAITING_ON_USER_MARKERS
        .iter()
        .any(|marker| paragraph.contains(marker))
    {
        return None;
    }
    if PROMISE_MARKERS
        .iter()
        .any(|marker| paragraph.contains(marker))
    {
        return Some(TurnEndingIssue::Promise);
    }
    None
}

/// The reminder folded in as a fresh user message when the gate fires. A user
/// message (not a wire reminder) so the re-prompt survives replay on every
/// provider and reads as an explicit course correction.
pub(super) fn turn_end_gate_reminder(issue: TurnEndingIssue) -> &'static str {
    match issue {
        TurnEndingIssue::Promise => {
            "[zo:turn-end-gate] <system-reminder>Your reply ended by promising work you have \
             not done yet. Do not end the turn on a promise: do that work now with tool calls — \
             including retrying after errors and gathering missing information yourself. If you \
             are genuinely blocked on input only the user can provide, say exactly what you need \
             and why instead. Then finish with a complete final answer.</system-reminder>"
        }
        TurnEndingIssue::TextualToolCall => {
            "[zo:turn-end-gate] <system-reminder>Your reply ended with a tool call written as \
             text (for example `call:default_api:read_file{…}` or `print(default_api.…)`). Text \
             is not a tool call: nothing ran, and there is no result to wait for. Issue the same \
             call now through the tool interface as a real function call with the same \
             arguments, then continue the work.</system-reminder>"
        }
        TurnEndingIssue::Question => {
            "[zo:turn-end-gate] <system-reminder>Your reply ended with a question, but nobody \
             is present to answer it mid-run. Make the most reasonable assumption, state it \
             explicitly, and continue the work with tool calls. Only end on a question if the \
             task truly cannot proceed without the user's decision — and then say exactly what \
             input is needed and what you completed.</system-reminder>"
        }
        // Composed with the ledger's facts by the runtime
        // (`turn_end_gate_reminder_text`); this static form is the fallback.
        TurnEndingIssue::UnverifiedCompletion => {
            "[zo:turn-end-gate] <system-reminder>Your reply reports the work as done, but no \
             check ran green after your edits this turn. Run the project's check now and report \
             its actual result, or say plainly that it was not run.</system-reminder>"
        }
    }
}

/// Short transcript banner shown when the gate re-prompts, so the extra
/// iteration is never a silent mystery.
pub(super) fn turn_end_gate_banner(issue: TurnEndingIssue) -> &'static str {
    match issue {
        TurnEndingIssue::Promise => {
            "[turn-gate] reply ended on a promise — asking the model to do the work now"
        }
        TurnEndingIssue::Question => {
            "[turn-gate] reply ended on a question nobody can answer — asking the model to proceed"
        }
        TurnEndingIssue::TextualToolCall => {
            "[turn-gate] reply wrote a tool call as text — nothing ran; asking the model for a real call"
        }
        TurnEndingIssue::UnverifiedCompletion => {
            "[turn-gate] reply reports completion but no check ran green after this turn's edits — asking for the check or an honest caveat"
        }
    }
}

impl<C, T> super::ConversationRuntime<C, T>
where
    C: super::ApiClient,
    T: super::ToolExecutor,
{
    /// Evaluate the gate against a naturally-ending assistant message. Returns
    /// the issue to re-prompt on, bumping `reprompts`, or `None` when the turn
    /// may end. Reads the env level per call so the knob works without a
    /// rebuild; bounded by [`TURN_END_GATE_MAX_REPROMPTS`] per turn.
    pub(super) fn take_turn_end_gate_issue(
        &self,
        final_text: &str,
        reprompts: &mut usize,
    ) -> Option<TurnEndingIssue> {
        let level = env_turn_end_gate_level();
        if level == 0 || *reprompts >= TURN_END_GATE_MAX_REPROMPTS {
            return None;
        }
        let lint_questions = self.autonomous_surface || level >= 2;
        // The completion screen stands down inside a deep-lane leg: an EXEC
        // leg edits and reports, and the VERIFY leg that follows is its check.
        // The completion screen asks once a turn, and only as the turn's first
        // re-prompt: a second ask over the same edits is a nag (seen live — a
        // reply that had read the file back to "verify" it was asked again),
        // and a reply already re-prompted for a promise has been told enough.
        let issue = screen_turn_ending(final_text, lint_questions).or_else(|| {
            (*reprompts == 0
                && self.deep_subturn_depth == 0
                && reports_unverified_completion(
                    final_text,
                    &self.verified_state.unverified_edits_this_turn(),
                ))
            .then_some(TurnEndingIssue::UnverifiedCompletion)
        })?;
        *reprompts += 1;
        Some(issue)
    }

    /// The reminder for `issue`, with the ledger's facts folded in where the
    /// issue has them: the unverified edits and the project's check command.
    pub(super) fn turn_end_gate_reminder_text(&self, issue: TurnEndingIssue) -> String {
        match issue {
            TurnEndingIssue::UnverifiedCompletion => unverified_completion_reminder(
                &self.verified_state.unverified_edits_this_turn(),
                super::deep_gate::detect_check_command().as_deref(),
            ),
            other => turn_end_gate_reminder(other).to_string(),
        }
    }

    /// The receipt line for a turn that edited and then ran a check green, or
    /// `None` when there is nothing to receipt (no edit, or no check after it).
    pub(super) fn completion_receipt_line(&self) -> Option<String> {
        let checks = self.verified_state.receipt_checks_this_turn();
        (!checks.is_empty()).then(|| completion_receipt(&checks))
    }

}

/// The malformed-reply half of the gate alone, for a surface that takes none
/// of the discipline lints — a sub-agent on the sync loop, which carries its
/// own completion contract and must not be re-prompted for a promise or a
/// question. A tool call written as text is not a reply on any surface:
/// nothing ran, and a helper that ends on one hands its parent nothing (seen
/// live: five Gemini helpers a day ended on `call:default_api:read_file{…}`).
/// Same level knob, same cap. Needs no runtime, so it is a free function.
pub(super) fn take_textual_tool_call_issue(
    final_text: &str,
    reprompts: &mut usize,
) -> Option<TurnEndingIssue> {
    let level = env_turn_end_gate_level();
    if level == 0 || *reprompts >= TURN_END_GATE_MAX_REPROMPTS {
        return None;
    }
    if !ends_on_textual_tool_call(final_text) {
        return None;
    }
    *reprompts += 1;
    Some(TurnEndingIssue::TextualToolCall)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promise_ending_is_flagged() {
        let text = "Found the bug in the parser.\n\nNext, I'll fix the tokenizer and rerun the tests.";
        assert_eq!(
            screen_turn_ending(text, false),
            Some(TurnEndingIssue::Promise)
        );
    }

    #[test]
    fn korean_promise_ending_is_flagged() {
        let text = "원인을 찾았습니다.\n\n이제 수정을 진행하겠습니다.";
        assert_eq!(
            screen_turn_ending(text, false),
            Some(TurnEndingIssue::Promise)
        );
    }

    /// The live case: Gemini wrote its next read as prose, glued to the last
    /// sentence, and the turn ended with nothing run.
    #[test]
    fn a_tool_call_written_as_text_is_flagged_before_anything_else() {
        let text = "The investigation now turns to paintKnowledgeStage within ui/shell-doc.js \
                    (lines 6620-6670) to understand why knowledgeIngest wasn't set properly in \
                    the empty vault scenario.call:default_api:read_file{limit:50,offset:6620,path:ui/shell-doc.js}";
        assert_eq!(
            screen_turn_ending(text, false),
            Some(TurnEndingIssue::TextualToolCall)
        );
        // Python-flavoured and fenced forms, and a waiting-on-user phrase does
        // not excuse them: the call still did not run.
        for tail in [
            "print(default_api.read_file(path=\"x.rs\"))",
            "```tool_code\nread_file(path=\"x.rs\")\n```",
            "<tool_call>{\"name\":\"read_file\"}</tool_call> — let me know if you want more",
        ] {
            let text = format!("Looking at the file next.\n\n{tail}");
            assert_eq!(
                screen_turn_ending(&text, false),
                Some(TurnEndingIssue::TextualToolCall),
                "{tail}"
            );
        }
    }

    /// A reply that talks about the namespace is an answer, not a leaked call;
    /// and a marker quoted far above the ending is not what the model ended on.
    #[test]
    fn a_mention_of_the_namespace_is_not_a_textual_tool_call() {
        assert_eq!(
            screen_turn_ending("Gemini exposes tools under a default_api namespace.", false),
            None
        );
        let quoted_early = format!(
            "Earlier the model wrote `call:default_api:read_file{{}}`.\n\n{}\n\nAll 42 tests pass.",
            "x".repeat(1_000)
        );
        assert_eq!(screen_turn_ending(&quoted_early, false), None);
    }

    /// The live case that escaped (2026-09-10, gemini-3.8-flash, turn 244): the
    /// leaked call carried a multi-line script as its argument, so the marker
    /// sat ~840 bytes before the end — beyond the tail window — while the call
    /// was still the last thing written. Two of the six leaks seen in three
    /// days escaped exactly this way; both closed with `}`.
    #[test]
    fn a_long_argument_does_not_push_the_call_out_of_the_gate() {
        let script = (0..24)
            .map(|line| format!("print(f\"row {line}: {{arr[{line}].sum()}}\")"))
            .collect::<Vec<_>>()
            .join("\n");
        let text = format!(
            "Need to determine the column index of the vertical line.\
             call:default_api:bash{{command:python3 -c '\n{script}\n'}}"
        );
        let marker_at = text.find("default_api").unwrap();
        assert!(text.len() - marker_at > TEXTUAL_TOOL_CALL_TAIL_BYTES);
        assert_eq!(
            screen_turn_ending(&text, false),
            Some(TurnEndingIssue::TextualToolCall)
        );
        // The same far marker followed by prose, or by a code block that is not
        // the call form's own closer, is an answer that quoted the namespace.
        let filler = "x".repeat(1_000);
        for ending in ["All 42 tests pass.", "```rust\nfn main() {}\n```"] {
            let quoted = format!(
                "Earlier it wrote `call:default_api:read_file{{}}`.\n\n{filler}\n\n{ending}"
            );
            assert_eq!(screen_turn_ending(&quoted, false), None, "{ending}");
        }
    }

    #[test]
    fn completed_report_is_not_flagged() {
        let text = "Fixed the tokenizer and reran the tests — all 42 pass.\n\nThe root cause was an off-by-one in the span math.";
        assert_eq!(screen_turn_ending(text, false), None);
    }

    #[test]
    fn earlier_promise_with_completed_ending_is_not_flagged() {
        let text = "I'll start with the parser.\n\nDone: parser fixed, tests green.";
        assert_eq!(screen_turn_ending(text, false), None);
    }

    #[test]
    fn waiting_on_user_is_not_flagged() {
        let text = "Two viable designs exist.\n\nI'll wait for your decision — let me know which one.";
        assert_eq!(screen_turn_ending(text, false), None);
        let korean = "설계가 두 가지입니다.\n\n어느 쪽으로 갈지 알려주세요.";
        assert_eq!(screen_turn_ending(korean, false), None);
    }

    #[test]
    fn question_ending_only_flagged_when_linting_questions() {
        let text = "The migration is ready.\n\nShould I also update the staging config?";
        assert_eq!(screen_turn_ending(text, false), None);
        assert_eq!(
            screen_turn_ending(text, true),
            Some(TurnEndingIssue::Question)
        );
    }

    #[test]
    fn question_lint_sees_through_trailing_markup() {
        let text = "다음 단계로 갈까요?**";
        assert_eq!(
            screen_turn_ending(text, true),
            Some(TurnEndingIssue::Question)
        );
    }

    /// The completion screen: a done-report counts, a caveat anywhere clears
    /// it, and with no unverified edit nothing is screened at all.
    #[test]
    fn a_done_report_over_unverified_edits_is_flagged_unless_it_admits_it() {
        let edits = ["/ws/src/a.rs"];
        assert!(reports_unverified_completion("Fixed the parser.\n\nAll done — the change is in.", &edits));
        assert!(reports_unverified_completion("파서를 고쳤고 구현이 완료됐습니다.", &edits));
        assert!(!reports_unverified_completion(
            "All done — the change is in. I did not run the tests.",
            &edits
        ));
        assert!(!reports_unverified_completion("구현 완료. 테스트는 돌리지 않았습니다.", &edits));
        assert!(
            !reports_unverified_completion(
                "The file is in place. No test runner or gate recipe exists in this project.",
                &edits
            ),
            "there being nothing to run is an honest caveat too"
        );
        assert!(!reports_unverified_completion("Here is what I found in the code.", &edits));
        assert!(!reports_unverified_completion("All done.", &[]));
        let reminder = unverified_completion_reminder(&edits, Some("cargo build --tests"));
        assert!(reminder.contains("(/ws/src/a.rs)") && reminder.contains("`cargo build --tests`"));
        assert_eq!(
            completion_receipt(&["cargo test -p runtime\n# second line", "just clippy"]),
            "[receipt] green after the last edit: cargo test -p runtime … ✓ · just clippy ✓"
        );
    }

    #[test]
    fn empty_text_is_ignored() {
        assert_eq!(screen_turn_ending("", true), None);
        assert_eq!(screen_turn_ending("\n\n", true), None);
    }
}
