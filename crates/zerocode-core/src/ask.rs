//! An agent's question, and the keys that answer it.
//!
//! Orca answers `AskUserQuestion` from outside the terminal: the question is
//! parsed off the hook payload, a person picks options in a card, and the
//! card walks the agent's own TUI by keystroke — digits pick rows, an arrow
//! moves between questions, Enter submits. All of it is measured protocol,
//! from one bundle (out/renderer/assets/index-ftls8Hg_.js, 1.4.169):
//!
//! - `parseQuestionsShape`/`parseOptions` (:69040/69067) — what counts as a
//!   question: a non-empty `questions` array whose entries carry a string
//!   `question` or at least one option; options are strings or
//!   `{label, description}` objects and anything else is dropped;
//! - `buildAskAnswerKeys` (:69118) — claude's TUI: `String(i+1)` picks row
//!   i, the row after the last option is "Type something", `ESC[C` advances
//!   to the next question's tab, and a multi-question or multi-select answer
//!   ends on the submit tab's Enter;
//! - `buildCodexAskAnswerKeys` (:69211) — codex's TUI: arrow keys walk rows
//!   (whichever direction is nearer), Tab opens the notes field, and every
//!   question is answered or explicitly skipped;
//! - `formatAskAnswer` (:69110) — the plain-text fallback for a TUI nobody
//!   measured: the picked labels, comma-joined per question;
//! - the pacing (:65033-65035) — one key group per second
//!   (`NATIVE_CHAT_QUESTION_STEP_MS` = 500 submit delay + 500 advance
//!   buffer), because the TUI redraws between groups and a burst outruns it.
//!
//! The builders are pure — groups in, keys out — so the rules live here and
//! the shell only owns the clock and the pty.

/// One key group per this many milliseconds
/// (`NATIVE_CHAT_QUESTION_STEP_MS`, index-ftls8Hg_.js:65035).
pub const QUESTION_STEP_MS: u64 = 1_000;

/// The gap between a pasted answer and its Enter
/// (`NATIVE_CHAT_SUBMIT_DELAY_MS`, :65033).
pub const SUBMIT_DELAY_MS: u64 = 500;

const ENTER: &str = "\r";
const NEXT_TAB: &str = "\x1b[C";
const PREVIOUS_ROW: &str = "\x1b[A";
const NEXT_ROW: &str = "\x1b[B";
const NOTES: &str = "\t";
/// DEL — the key codex's ask dialog binds to "skip this question"
/// (`native-chat-ask.ts:290`, the inline `'\x7f'`). Skipping is what arms the
/// dialog's closing confirmation; a question left merely untouched does not.
const SKIP_QUESTION: &str = "\x7f";

/// One row a question offers (`parseOptions`, :69067).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One question out of the tool call's `questions` array.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AskQuestion {
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default)]
    pub multi_select: bool,
    pub options: Vec<AskOption>,
}

/// The whole ask, as the card will draw it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AskPrompt {
    pub questions: Vec<AskQuestion>,
}

/// What a person picked for one question: option ordinals, and the free-text
/// answer when they typed one.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AskSelection {
    #[serde(default)]
    pub indices: Vec<usize>,
    #[serde(default)]
    pub other: String,
}

/// One thing to send: bytes that ARE keys, or text that must travel as a
/// paste (`{raw}` / `{text}` groups, :69118).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyGroup {
    Raw(String),
    Text(String),
}

/// The questions inside a hook payload's `tool_input`, when they have the
/// shape.
///
/// Orca keys its parser table on the tool's name but falls through to the
/// shape check for every input (`parseToolInput`, :69090 — `?? parseQuestionsShape`),
/// so the shape is the real test and the name is a shortcut. The payload
/// here is the whole hook event; the tool input is its `tool_input` field.
pub fn prompt_in_payload(payload: &str) -> Option<AskPrompt> {
    prompt_in_parsed(&crate::payload::HookPayload::of(payload))
}

/// [`prompt_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn prompt_in_parsed(payload: &crate::payload::HookPayload<'_>) -> Option<AskPrompt> {
    questions_shape(payload.tree()?.get("tool_input")?)
}

/// What answers a permission request — one key each way, sent at once
/// (`parseApprovalFromStatus`, :69216: Allow sends `"1"`, Deny sends the
/// escape). No pacing, no walk: the TUI is sitting on a numbered prompt and
/// escape is its own refusal everywhere.
pub const APPROVAL_ALLOW: &str = "1";
pub const APPROVAL_DENY: &str = "\x1b";

/// How much of the summary survives onto a card
/// (`summarizeApprovalInput`, out/main/index.js:9011 — 200, then `…`).
const APPROVAL_SUMMARY_CHARS: usize = 200;

/// A tool waiting on permission: who it is, and the one line that says what
/// it wants.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApprovalPrompt {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The edit the tool asks to make, as the rows a card draws under the
    /// question — the extension's "Make this edit to utils.py?" shows the
    /// diff before the person answers. Empty for a tool that edits nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edits: Vec<crate::transcript::TranscriptEdit>,
    /// The plan this permission is asking approval FOR, when the tool asking
    /// is the CLI's plan tool — the extension's "Claude's Plan" card previews
    /// it and offers a reason field on the refusal (2.1.268-275).
    ///
    /// Filled by the caller, never here: which tool carries a plan is an
    /// agent fact, and this reader is given a payload and no agent
    /// ([`plan_in`] is the rule, `AgentVoice::plan_tool` the fact).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// The plan inside a permission request's input, when the tool asking IS the
/// agent's plan tool.
///
/// Three things have to line up: the agent names a plan tool at all, the
/// request is that tool, and the input carries a non-empty `plan` string.
/// Anything else is an ordinary approval — a card that guessed a plan out of
/// a tool it does not know would put a "Claude's Plan" heading over a `Bash`
/// command.
#[must_use]
pub fn plan_in(
    input: Option<&serde_json::Value>,
    tool: &str,
    plan_tool: Option<&str>,
) -> Option<String> {
    if plan_tool? != tool {
        return None;
    }
    input
        .and_then(|value| value.get("plan"))
        .and_then(|value| value.as_str())
        .filter(|plan| !plan.is_empty())
        .map(str::to_string)
}

/// The approval inside a permission event's payload.
///
/// Orca builds this only off the `PermissionRequest` hook event
/// (`deriveInteractivePrompt`, out/main/index.js:9027) — the event check is
/// the caller's, like the ask's. The shape here: a non-empty `tool_name`,
/// and NOT a question — `parseInteractivePrompt` (:69232) asks for the
/// question first and only then for the approval, so a payload that parses
/// as questions is a question wearing a tool's name.
pub fn approval_in_payload(payload: &str) -> Option<ApprovalPrompt> {
    approval_in_parsed(&crate::payload::HookPayload::of(payload))
}

/// [`approval_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn approval_in_parsed(payload: &crate::payload::HookPayload<'_>) -> Option<ApprovalPrompt> {
    let value = payload.tree()?;
    let tool = value
        .get("tool_name")
        .and_then(|v| v.as_str())
        .filter(|tool| !tool.is_empty())?;
    let input = value.get("tool_input");
    if input.and_then(questions_shape).is_some() {
        return None;
    }
    Some(ApprovalPrompt {
        tool: tool.to_string(),
        summary: approval_summary(input),
        edits: crate::transcript::edits_in(tool, input),
        plan: None,
    })
}

/// `summarizeApprovalInput` (:9011): the one input string that identifies
/// what the tool wants — `command`, `file_path`, `path`, `url` or `pattern`
/// — and failing those, the whole input as JSON. Either way clipped at 200
/// with an ellipsis; nothing worth saying comes back as nothing.
fn approval_summary(input: Option<&serde_json::Value>) -> Option<String> {
    let direct = ["command", "file_path", "path", "url", "pattern"]
        .iter()
        .find_map(|key| input.and_then(|v| v.get(key)).and_then(|v| v.as_str()))
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let said = direct.or_else(|| input.and_then(|v| serde_json::to_string(v).ok()))?;
    if said.is_empty() {
        return None;
    }
    if said.chars().count() <= APPROVAL_SUMMARY_CHARS {
        return Some(said);
    }
    let mut clipped: String = said.chars().take(APPROVAL_SUMMARY_CHARS).collect();
    clipped.push('…');
    Some(clipped)
}

/// `parseQuestionsShape` (:69040): an object with a non-empty `questions`
/// array; each entry keeps its text, its header, whether it multi-selects
/// (`=== true`, not merely truthy), and the options that parsed. An entry
/// with neither text nor options is dropped; a prompt with no surviving
/// questions is not a prompt.
pub fn questions_shape(input: &serde_json::Value) -> Option<AskPrompt> {
    let raw_questions = input.get("questions")?.as_array()?;
    if raw_questions.is_empty() {
        return None;
    }
    let mut questions = Vec::new();
    for raw in raw_questions {
        if !raw.is_object() {
            continue;
        }
        let text = raw
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let options = options_shape(raw.get("options"));
        if text.is_empty() && options.is_empty() {
            continue;
        }
        questions.push(AskQuestion {
            question: text.to_string(),
            header: raw
                .get("header")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            multi_select: raw.get("multiSelect").and_then(|v| v.as_bool()) == Some(true),
            options,
        });
    }
    (!questions.is_empty()).then_some(AskPrompt { questions })
}

/// `parseOptions` (:69067): a string is a label, an object needs a string
/// label and may carry a description, and everything else is dropped.
fn options_shape(raw: Option<&serde_json::Value>) -> Vec<AskOption> {
    let Some(list) = raw.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|option| {
            if let Some(label) = option.as_str() {
                return Some(AskOption {
                    label: label.to_string(),
                    description: None,
                });
            }
            let label = option.get("label")?.as_str()?;
            Some(AskOption {
                label: label.to_string(),
                description: option
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        })
        .collect()
}

/// Whether one question got an answer (`isAnswered`, :69103): an option was
/// picked, or something was typed.
fn is_answered(selection: Option<&AskSelection>) -> bool {
    selection.is_some_and(|sel| !sel.indices.is_empty() || !sel.other.trim().is_empty())
}

/// Whether anything at all was answered (`hasAskAnswer`, :69190) — the send
/// button's own rule, and the refusal below it.
pub fn has_answer(prompt: &AskPrompt, selections: &[AskSelection]) -> bool {
    prompt
        .questions
        .iter()
        .enumerate()
        .any(|(i, _)| is_answered(selections.get(i)))
}

/// The labels one question's answer names (`answerLabels`, :69105): the
/// picked options' labels, then the typed answer.
fn answer_labels(question: &AskQuestion, selection: Option<&AskSelection>) -> Vec<String> {
    let mut labels: Vec<String> = selection
        .map(|sel| {
            sel.indices
                .iter()
                .filter_map(|&i| question.options.get(i))
                .map(|option| option.label.clone())
                .filter(|label| !label.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if let Some(other) = selection
        .map(|sel| sel.other.trim())
        .filter(|o| !o.is_empty())
    {
        labels.push(other.to_string());
    }
    labels
}

/// The whole answer as a sentence (`formatAskAnswer`, :69110) — what a TUI
/// without a measured key protocol is sent instead of keystrokes.
pub fn format_answer(prompt: &AskPrompt, selections: &[AskSelection]) -> String {
    prompt
        .questions
        .iter()
        .enumerate()
        .map(|(i, q)| answer_labels(q, selections.get(i)).join(", "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The keystrokes that answer claude's ask TUI (`buildAskAnswerKeys`,
/// :69118).
///
/// The TUI's own grammar: `String(i + 1)` picks row i and — on a
/// single-select — answers and advances in one key; the row after the last
/// option is "Type something", so its ordinal is `options.len() + 1`; `ESC[C`
/// moves to the next question's tab; and when the walk ends on the submit
/// tab (more than one question, or one multi-select question), the last
/// Enter is the submission.
pub fn build_answer_keys(prompt: &AskPrompt, selections: &[AskSelection]) -> Vec<KeyGroup> {
    let questions = &prompt.questions;
    let multi_question = questions.len() > 1;
    let mut groups = Vec::new();
    for (qi, q) in questions.iter().enumerate() {
        let sel = selections.get(qi);
        let other = sel.map(|s| s.other.trim()).unwrap_or_default();
        let type_something = (q.options.len() + 1).to_string();
        if q.multi_select {
            for &i in sel.map(|s| s.indices.as_slice()).unwrap_or_default() {
                groups.push(KeyGroup::Raw((i + 1).to_string()));
            }
            if !other.is_empty() {
                groups.push(KeyGroup::Raw(type_something));
                groups.push(KeyGroup::Text(other.to_string()));
                groups.push(KeyGroup::Raw(ENTER.to_string()));
            }
            groups.push(KeyGroup::Raw(NEXT_TAB.to_string()));
        } else if !other.is_empty() {
            groups.push(KeyGroup::Raw(type_something));
            groups.push(KeyGroup::Text(answer_labels(q, sel).join(", ")));
            groups.push(KeyGroup::Raw(ENTER.to_string()));
        } else if let Some(&first) = sel.and_then(|s| s.indices.first()) {
            groups.push(KeyGroup::Raw((first + 1).to_string()));
        } else if multi_question {
            groups.push(KeyGroup::Raw(NEXT_TAB.to_string()));
        }
    }
    let ends_on_submit_tab = multi_question || (questions.len() == 1 && questions[0].multi_select);
    if ends_on_submit_tab && !groups.is_empty() {
        groups.push(KeyGroup::Raw(ENTER.to_string()));
    }
    groups
}

/// The keystrokes that answer codex's ask TUI (`buildCodexAskAnswerKeys`,
/// :69211).
///
/// A different grammar: rows are walked with arrows — whichever direction
/// reaches the target in fewer steps, the list wrapping around — Tab opens
/// the notes field, a digit still picks a row directly, and a question left
/// unanswered is skipped with the dialog's own skip key. The skip is a real
/// keystroke, not a beat: it is what marks the question skipped, and the
/// closing confirmation this builder ends on only appears once something
/// was. (An empty group here once kept the pacing and sent nothing — the
/// dialog never learned the question was being passed over.)
pub fn build_codex_answer_keys(prompt: &AskPrompt, selections: &[AskSelection]) -> Vec<KeyGroup> {
    let mut groups = Vec::new();
    let mut has_unanswered = false;
    let total = prompt.questions.len();
    for (qi, question) in prompt.questions.iter().enumerate() {
        let selection = selections.get(qi);
        let selected_index = selection.and_then(|s| s.indices.first()).copied();
        let note = selection.map(|s| s.other.trim()).unwrap_or_default();
        if !note.is_empty() {
            let target_index = selected_index.unwrap_or(question.options.len());
            let row_count = question.options.len() + 1;
            let next_steps = target_index;
            let previous_steps = row_count - target_index;
            let use_previous = previous_steps < next_steps;
            let navigation_key = if use_previous { PREVIOUS_ROW } else { NEXT_ROW };
            let navigation_steps = if use_previous {
                previous_steps
            } else {
                next_steps
            };
            for _ in 0..navigation_steps {
                groups.push(KeyGroup::Raw(navigation_key.to_string()));
            }
            groups.push(KeyGroup::Raw(NOTES.to_string()));
            groups.push(KeyGroup::Text(note.to_string()));
            groups.push(KeyGroup::Raw(ENTER.to_string()));
            continue;
        }
        if let Some(index) = selected_index {
            groups.push(KeyGroup::Raw((index + 1).to_string()));
            continue;
        }
        has_unanswered = true;
        groups.push(KeyGroup::Raw(SKIP_QUESTION.to_string()));
        if qi < total - 1 {
            groups.push(KeyGroup::Raw(NEXT_TAB.to_string()));
        } else {
            groups.push(KeyGroup::Raw(ENTER.to_string()));
        }
    }
    if has_unanswered {
        groups.push(KeyGroup::Raw(ENTER.to_string()));
    }
    groups
}

/// A line typed the way a person submits one: the text as a paste, then —
/// after [`SUBMIT_DELAY_MS`] — its Enter as a key of its own. The shape every
/// TUI takes for a plain sentence: the formatted answer of an agent without a
/// grammar of its own, and a CLI's own exit command when its screen hands a
/// conversation over.
#[must_use]
pub fn line_keys(text: &str) -> (Vec<KeyGroup>, std::time::Duration) {
    (
        vec![
            KeyGroup::Text(text.to_string()),
            KeyGroup::Raw(ENTER.to_string()),
        ],
        std::time::Duration::from_millis(SUBMIT_DELAY_MS),
    )
}

/// Which builder an agent's TUI answers to
/// (`shouldStepNativeChatAskAnswer`/`resolveNativeChatTranscriptAgent`,
/// web-runtime-session-DN6BsdRt.js:172-186): claude's grammar for claude,
/// codex's for codex, and everyone else gets the formatted sentence.
pub fn keys_for(
    agent: &str,
    prompt: &AskPrompt,
    selections: &[AskSelection],
) -> Option<Vec<KeyGroup>> {
    match agent {
        "claude" | "openclaude" => Some(build_answer_keys(prompt, selections)),
        "codex" => Some(build_codex_answer_keys(prompt, selections)),
        _ => None,
    }
}

/// The keystrokes that can SUBMIT a question from inside the terminal
/// (`QUESTION_ANSWER_ENTER_INPUTS`, agent-question-answered-intent.ts:25-30):
/// the three newline spellings, and the kitty-protocol Enter both ways it is
/// encoded.
const SUBMIT_ENTER_INPUTS: [&str; 5] = ["\r", "\n", "\r\n", "\x1b[13u", "\x1b[13;1u"];

/// A byte that could be answering a question at the keyboard — Enter, or a
/// row digit (`isPotentialQuestionAnsweredSubmitInput`, `:33-35`).
///
/// This is the cheap pre-filter the pty write path may ask on every keystroke;
/// whether the keystroke actually FINISHES the prompt is
/// [`answers_whole_prompt`]'s question, which needs the prompt.
#[must_use]
pub fn is_potential_submit_input(data: &str) -> bool {
    SUBMIT_ENTER_INPUTS.contains(&data)
        || (data.len() == 1 && data.as_bytes()[0].is_ascii_digit() && data != "0")
}

/// How much of a pane's stop ONE keystroke could possibly finish.
///
/// The four answers Orca reaches through two layers — its tool-name gate
/// (`inferQuestionAnsweredFromEntry`, `agent-question-answered-inference.ts:34`,
/// and again in the main process at `server.ts:1014-1020`) and its
/// `readSingleSelectOptionCount` three-way (`null` / `-1` / a count,
/// `agent-question-answered-intent.ts:37-56`) — decided here as one word.
///
/// **One word rather than two fields, because the tool gate is the one a
/// reader forgets.** Its absence is invisible: every shape answer still looks
/// right, and a permission prompt quietly becomes answerable by Enter. Orca
/// writes the reason next to it — "real permission waits stay sticky".
///
/// **And decided ONCE, where the payload is.** Orca re-parses the bounded JSON
/// on every candidate keystroke; this window reads it when the event lands, so
/// the keystroke path matches a word instead of parsing a document.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmitShape {
    /// Not the ask-a-question tool at all — a permission gate, a notification,
    /// or a pane that is not stopped. Nothing typed finishes it.
    #[default]
    NotAQuestion,
    /// The question tool, with no input carried for it — an older hook. Enter
    /// remains the conservative fallback; a digit names nothing.
    Unstated,
    /// The question tool, with an input that is not ONE single-select question:
    /// several questions, a multi-select, or bytes that will not read. Nothing
    /// finishes. A length-capped multi-question payload is not the same fact as
    /// an absent field, so unreadable fails CLOSED (`:53-56`).
    Compound,
    /// One single-select question, and how many options it DECLARED. Claude
    /// adds a synthetic "Type something" row past them which opens an editor,
    /// so only the declared numbers finish (`:82-84`).
    SingleSelect(usize),
}

impl SubmitShape {
    /// Whether this word refuses every keystroke — the default, and the one a
    /// row does not need to carry onto the wire.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        *self == Self::NotAQuestion
    }
}

/// The shape of the stop this hook payload reports, for the keystroke road.
///
/// Reads the same two fields the ask card reads, through the same parser
/// (`is_ask_user_question_tool` and [`questions_shape`]) — a second reader
/// here would be a second opinion about what a question is.
///
/// The EVENT is not asked about: the caller has already decided the pane is
/// stopped, exactly as the ask and the approval leave that to their caller.
#[must_use]
pub fn submit_shape_in_payload(payload: &str) -> SubmitShape {
    submit_shape_in_parsed(&crate::payload::HookPayload::of(payload))
}

/// [`submit_shape_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn submit_shape_in_parsed(payload: &crate::payload::HookPayload<'_>) -> SubmitShape {
    let Some(value) = payload.tree() else {
        return SubmitShape::NotAQuestion;
    };
    let asks = value
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .is_some_and(crate::hook::is_ask_user_question_tool);
    if !asks {
        return SubmitShape::NotAQuestion;
    }
    let Some(input) = value.get("tool_input") else {
        return SubmitShape::Unstated;
    };
    questions_shape(input)
        .filter(|parsed| parsed.questions.len() == 1 && !parsed.questions[0].multi_select)
        .map_or(SubmitShape::Compound, |parsed| {
            SubmitShape::SingleSelect(parsed.questions[0].options.len())
        })
}

/// Whether one keystroke finished the WHOLE stop
/// (`isQuestionAnsweredSubmitInput`, `:64-85`).
///
/// The person answered claude's blocked question directly in the terminal —
/// no card involved — and the waiting indicator should clear without waiting
/// for a hook that may never come. But a digit merely advances a
/// multi-question prompt and merely toggles a multi-select, and the synthetic
/// "Type something" row past the declared options opens an editor instead of
/// submitting; clearing on those paints a ready pane over an agent still
/// blocked. [`SubmitShape`] holds which of those a pane is in; this says what
/// a keystroke does about it.
#[must_use]
pub fn answers_whole_prompt(data: &str, shape: SubmitShape) -> bool {
    if !is_potential_submit_input(data) {
        return false;
    }
    match shape {
        SubmitShape::NotAQuestion | SubmitShape::Compound => false,
        SubmitShape::Unstated => SUBMIT_ENTER_INPUTS.contains(&data),
        SubmitShape::SingleSelect(declared) => {
            SUBMIT_ENTER_INPUTS.contains(&data)
                || data.parse::<usize>().is_ok_and(|picked| picked <= declared)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain line is its text as a paste and then its Enter as a key of
    /// its own, a submit delay apart — the walk a formatted answer and a
    /// CLI's exit command both take.
    #[test]
    fn a_plain_line_is_its_text_then_enter_after_the_submit_delay() {
        let (groups, step) = line_keys("/exit");
        assert_eq!(
            groups,
            vec![
                KeyGroup::Text("/exit".to_string()),
                KeyGroup::Raw("\r".to_string())
            ]
        );
        assert_eq!(step, std::time::Duration::from_millis(SUBMIT_DELAY_MS));
    }

    fn q(question: &str, options: &[&str], multi: bool) -> AskQuestion {
        AskQuestion {
            question: question.to_string(),
            header: None,
            multi_select: multi,
            options: options
                .iter()
                .map(|label| AskOption {
                    label: (*label).to_string(),
                    description: None,
                })
                .collect(),
        }
    }

    fn picked(indices: &[usize]) -> AskSelection {
        AskSelection {
            indices: indices.to_vec(),
            other: String::new(),
        }
    }

    fn typed(other: &str) -> AskSelection {
        AskSelection {
            indices: Vec::new(),
            other: other.to_string(),
        }
    }

    fn raws(groups: &[KeyGroup]) -> Vec<String> {
        groups
            .iter()
            .map(|group| match group {
                KeyGroup::Raw(raw) => format!("raw:{raw}"),
                KeyGroup::Text(text) => format!("text:{text}"),
            })
            .collect()
    }

    /// The shape rules, each on its own row: string options become labels,
    /// objects keep their description, junk options are dropped, an entry
    /// with nothing survives as nothing, `multiSelect` must be `true` and
    /// not merely truthy, and a payload without the shape is not an ask.
    #[test]
    fn a_question_is_its_shape_not_its_tool_name() {
        let parsed = prompt_in_payload(
            r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[
                {"question":"Which way?","header":"Route","multiSelect":1,
                 "options":["Left",{"label":"Right","description":"the long way"},7,{"noLabel":true}]},
                {"question":"","options":[]},
                "not an object"
            ]}}"#,
        )
        .expect("parses");
        assert_eq!(parsed.questions.len(), 1);
        let only = &parsed.questions[0];
        assert_eq!(only.question, "Which way?");
        assert_eq!(only.header.as_deref(), Some("Route"));
        assert!(!only.multi_select, "multiSelect must be === true");
        assert_eq!(only.options.len(), 2);
        assert_eq!(only.options[1].description.as_deref(), Some("the long way"));
        assert_eq!(
            prompt_in_payload(r#"{"tool_input":{"questions":[]}}"#),
            None
        );
        assert_eq!(prompt_in_payload(r#"{"message":"no tool here"}"#), None);
    }

    /// The approval's own shape rules: the telling field wins over the JSON
    /// fallback, the clip lands at 200 characters plus the ellipsis, an
    /// empty tool is no approval, and a payload that parses as questions is
    /// a question — `parseInteractivePrompt` asks in that order.
    #[test]
    fn an_approval_is_a_tool_and_one_line_never_a_question() {
        let bash = approval_in_payload(
            r#"{"tool_name":"Bash","tool_input":{"command":"rm -rf build","timeout":5}}"#,
        )
        .expect("parses");
        assert_eq!(bash.tool, "Bash");
        assert_eq!(bash.summary.as_deref(), Some("rm -rf build"));
        let fallback =
            approval_in_payload(r#"{"tool_name":"WebSearch","tool_input":{"query_count":3}}"#)
                .expect("parses");
        assert_eq!(fallback.summary.as_deref(), Some(r#"{"query_count":3}"#));
        let long = approval_in_payload(&format!(
            r#"{{"tool_name":"Bash","tool_input":{{"command":"{}"}}}}"#,
            "x".repeat(300)
        ))
        .expect("parses");
        assert_eq!(long.summary.as_ref().map(|s| s.chars().count()), Some(201));
        assert!(long.summary.unwrap().ends_with('…'));
        assert_eq!(
            approval_in_payload(r#"{"tool_name":"","tool_input":{}}"#),
            None
        );
        assert_eq!(approval_in_payload(r#"{"message":"agent waiting"}"#), None);
        assert_eq!(
            approval_in_payload(
                r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[
                    {"question":"Which way?","options":["Left"]}]}}"#,
            ),
            None,
            "a question is not an approval, whatever tool name it wears"
        );
        assert_eq!(APPROVAL_ALLOW, "1");
        assert_eq!(APPROVAL_DENY, "\u{1b}");
        assert_eq!(bash.plan, None, "the reader fills no plan; the caller does");
    }

    /// A plan is read only where all three line up: the agent names a plan
    /// tool, the request IS that tool, and the input carries a plan worth
    /// showing. Every other shape is an ordinary approval.
    #[test]
    fn a_plan_is_read_only_from_the_agents_own_plan_tool() {
        let planning = serde_json::json!({ "plan": "## Plan\n1. read\n2. write" });
        assert_eq!(
            plan_in(Some(&planning), "ExitPlanMode", Some("ExitPlanMode")).as_deref(),
            Some("## Plan\n1. read\n2. write")
        );
        assert_eq!(
            plan_in(Some(&planning), "Bash", Some("ExitPlanMode")),
            None,
            "another tool's input is not a plan, whatever it carries"
        );
        assert_eq!(
            plan_in(Some(&planning), "ExitPlanMode", None),
            None,
            "an agent that names no plan tool has no plan card"
        );
        assert_eq!(
            plan_in(
                Some(&serde_json::json!({ "plan": "" })),
                "ExitPlanMode",
                Some("ExitPlanMode")
            ),
            None,
            "an empty plan is nothing to approve"
        );
        assert_eq!(
            plan_in(
                Some(&serde_json::json!({ "plan": 7 })),
                "ExitPlanMode",
                Some("ExitPlanMode")
            ),
            None
        );
        assert_eq!(plan_in(None, "ExitPlanMode", Some("ExitPlanMode")), None);
    }

    /// claude, one single-select question: the digit answers and submits in
    /// one key — no trailing Enter, because there is no submit tab to land on.
    #[test]
    fn one_single_select_answer_is_one_digit() {
        let prompt = AskPrompt {
            questions: vec![q("Which way?", &["Left", "Right"], false)],
        };
        let groups = build_answer_keys(&prompt, &[picked(&[1])]);
        assert_eq!(raws(&groups), vec!["raw:2"]);
    }

    /// claude, typed answer: the "Type something" row is one PAST the last
    /// option, the text carries picked labels and the note joined, and Enter
    /// closes the field — still no submit tab for a single question.
    #[test]
    fn a_typed_answer_goes_through_the_type_something_row() {
        let prompt = AskPrompt {
            questions: vec![q("Which way?", &["Left", "Right"], false)],
        };
        let sel = AskSelection {
            indices: vec![0],
            other: " around ".to_string(),
        };
        let groups = build_answer_keys(&prompt, &[sel]);
        assert_eq!(raws(&groups), vec!["raw:3", "text:Left, around", "raw:\r"]);
    }

    /// claude, one multi-select question: each digit toggles, the arrow
    /// leaves the row list, and the walk now ends on the submit tab — so the
    /// last Enter appears (`endsOnSubmitTab`, :69146).
    #[test]
    fn a_multi_select_toggles_then_submits() {
        let prompt = AskPrompt {
            questions: vec![q("Pick tools", &["A", "B", "C"], true)],
        };
        let groups = build_answer_keys(&prompt, &[picked(&[0, 2])]);
        assert_eq!(
            raws(&groups),
            vec!["raw:1", "raw:3", "raw:\x1b[C", "raw:\r"]
        );
    }

    /// claude, two questions with the second unanswered: the skip is an
    /// explicit arrow, and the multi-question walk ends on the submit tab's
    /// Enter.
    #[test]
    fn an_unanswered_question_is_skipped_with_an_arrow() {
        let prompt = AskPrompt {
            questions: vec![
                q("First?", &["Yes", "No"], false),
                q("Second?", &["Yes", "No"], false),
            ],
        };
        let groups = build_answer_keys(&prompt, &[picked(&[0])]);
        assert_eq!(raws(&groups), vec!["raw:1", "raw:\x1b[C", "raw:\r"]);
    }

    /// codex, a note on the last row: rows are walked in whichever direction
    /// is nearer — here one step UP reaches "Type something" faster than
    /// three steps down — then Tab opens the notes field.
    #[test]
    fn codex_walks_the_nearer_way_to_the_notes_row() {
        let prompt = AskPrompt {
            questions: vec![q("Which way?", &["A", "B", "C"], false)],
        };
        let groups = build_codex_answer_keys(&prompt, &[typed("a note")]);
        assert_eq!(
            raws(&groups),
            vec!["raw:\x1b[A", "raw:\t", "text:a note", "raw:\r"]
        );
    }

    /// codex, unanswered questions: each is skipped with the dialog's own
    /// key — DEL, `native-chat-ask.ts:290` — and the trailing Enter accepts
    /// the confirmation that only skipped questions summon. An empty group
    /// here once sent nothing, and the dialog stood on the question it was
    /// supposedly passing over.
    #[test]
    fn codex_skips_are_explicit_and_end_on_enter() {
        let prompt = AskPrompt {
            questions: vec![q("First?", &["Yes"], false), q("Second?", &["Yes"], false)],
        };
        let groups = build_codex_answer_keys(&prompt, &[picked(&[0])]);
        assert_eq!(raws(&groups), vec!["raw:1", "raw:\x7f", "raw:\r", "raw:\r"]);

        // And an unanswered question that is NOT last skips then steps on.
        let none_then_pick =
            build_codex_answer_keys(&prompt, &[AskSelection::default(), picked(&[0])]);
        assert_eq!(
            raws(&none_then_pick),
            vec!["raw:\x7f", "raw:\x1b[C", "raw:1", "raw:\r"]
        );
    }

    /// The sentence fallback and the button's own rule: labels join with the
    /// note per question, questions join on newlines, and an all-empty
    /// selection is not an answer.
    #[test]
    fn the_fallback_is_the_answer_as_a_sentence() {
        let prompt = AskPrompt {
            questions: vec![
                q("First?", &["Yes", "No"], false),
                q("Second?", &["A", "B"], false),
            ],
        };
        let selections = vec![picked(&[1]), typed("neither")];
        assert_eq!(format_answer(&prompt, &selections), "No\nneither");
        assert!(has_answer(&prompt, &selections));
        assert!(!has_answer(&prompt, &[AskSelection::default()]));
        assert!(keys_for("claude", &prompt, &selections).is_some());
        assert!(keys_for("codex", &prompt, &selections).is_some());
    }

    /// A single-select question answered at the keyboard: Enter and the
    /// declared digits finish it; the synthetic "Type something" row, a
    /// multi-select toggle, and a mid-walk digit do not.
    ///
    /// Every fact `b5f6bed` bought stands here, re-asked of the word instead
    /// of the raw JSON — the shapes those payloads produce are the subject of
    /// the test below.
    #[test]
    fn a_keystroke_finishes_only_the_prompt_it_can_finish() {
        for enter in ["\r", "\n", "\r\n", "\x1b[13u", "\x1b[13;1u"] {
            assert!(
                answers_whole_prompt(enter, SubmitShape::SingleSelect(3)),
                "{enter:?}"
            );
        }
        assert!(answers_whole_prompt("3", SubmitShape::SingleSelect(3)));
        // Row 4 is claude's synthetic "Type something" — it opens an editor.
        assert!(!answers_whole_prompt("4", SubmitShape::SingleSelect(3)));
        assert!(!answers_whole_prompt("0", SubmitShape::SingleSelect(3)));
        assert!(!answers_whole_prompt("a", SubmitShape::SingleSelect(3)));

        // A multi-select's digit is a toggle and two questions merely advance
        // — one word for both, and it refuses Enter too.
        assert!(!answers_whole_prompt("1", SubmitShape::Compound));
        assert!(!answers_whole_prompt("\r", SubmitShape::Compound));

        // An old hook with no tool input: Enter is the conservative
        // fallback, digits are not.
        assert!(answers_whole_prompt("\r", SubmitShape::Unstated));
        assert!(!answers_whole_prompt("1", SubmitShape::Unstated));

        // And a stop that is not a question is answered by nothing — the gate
        // Orca states as "real permission waits stay sticky".
        assert!(!answers_whole_prompt("\r", SubmitShape::NotAQuestion));
        assert!(!answers_whole_prompt("1", SubmitShape::NotAQuestion));
    }

    /// Which word each payload earns. The two that matter are the ones a
    /// single `Option` cannot tell apart: an absent input (Enter still
    /// finishes) and an unreadable one (nothing does).
    #[test]
    fn the_payload_earns_its_word_once_and_a_permission_wait_earns_none() {
        let single = r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which?","options":[{"label":"A"},{"label":"B"},{"label":"C"}]}]}}"#;
        assert_eq!(
            submit_shape_in_payload(single),
            SubmitShape::SingleSelect(3)
        );
        // Every vendor spelling of the same tool, through the one reader.
        let other = r#"{"tool_name":"request_user_input","tool_input":{"questions":[{"question":"Which?","options":[{"label":"A"}]}]}}"#;
        assert_eq!(submit_shape_in_payload(other), SubmitShape::SingleSelect(1));

        let multi = r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which?","multiSelect":true,"options":[{"label":"A"}]}]}}"#;
        assert_eq!(submit_shape_in_payload(multi), SubmitShape::Compound);
        let two = r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"1?","options":[{"label":"A"}]},{"question":"2?","options":[{"label":"B"}]}]}}"#;
        assert_eq!(submit_shape_in_payload(two), SubmitShape::Compound);

        // Present but unreadable fails CLOSED — a length-capped
        // multi-question payload is not the same fact as an absent field.
        //
        // The cap lands INSIDE the input here, because that is where Orca's
        // lands: its `interactivePrompt` is a string field carrying JSON, so a
        // clipped one leaves the envelope intact. Ours is a nested object, so
        // the same clip applied to the whole payload takes the envelope with
        // it — the line below. Both refuse; only this one can still be read.
        let capped = r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":"[{\"quest"}}"#;
        assert_eq!(submit_shape_in_payload(capped), SubmitShape::Compound);
        let torn = r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"quest"#;
        assert_eq!(submit_shape_in_payload(torn), SubmitShape::NotAQuestion);
        // Absent is the OTHER fact, and it keeps Enter.
        let bare = r#"{"tool_name":"AskUserQuestion"}"#;
        assert_eq!(submit_shape_in_payload(bare), SubmitShape::Unstated);

        // A permission gate carries a tool input and is still not a question.
        let permission = r#"{"tool_name":"Bash","tool_input":{"command":"rm -rf build"}}"#;
        assert_eq!(
            submit_shape_in_payload(permission),
            SubmitShape::NotAQuestion
        );
        // As is a permission gate that carried nothing — the case a shape-only
        // gate would hand to Enter.
        assert_eq!(
            submit_shape_in_payload(r#"{"tool_name":"Bash"}"#),
            SubmitShape::NotAQuestion
        );
        // And a notification, and bytes that are not a document at all.
        assert_eq!(
            submit_shape_in_payload(r#"{"message":"build finished"}"#),
            SubmitShape::NotAQuestion
        );
        assert_eq!(
            submit_shape_in_payload("not json"),
            SubmitShape::NotAQuestion
        );

        // The default is the refusing word, so a row nobody filled in answers
        // nothing rather than answering Enter.
        assert_eq!(SubmitShape::default(), SubmitShape::NotAQuestion);
    }
}
