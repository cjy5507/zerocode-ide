//! What a conversation page shows of a turn its CLI has not written yet, when
//! the CLI writes its session once a turn has ended
//! ([`crate::vault::Format::settles_after_turn`]) — built from the hooks the
//! CLI fires as the turn goes.
//!
//! Devin writes its whole ATIF document about 30 ms after a turn's `Stop` and
//! nothing while the turn runs (t-124593), so a page reading that document
//! had nothing to show for a whole turn (t-125261: the person's page stood on
//! its first turn while the second ran for two minutes). Its hooks come as
//! the turn goes and carry what a person reads (measured 2026-11-21,
//! `reports/summon-every-cli-172/samples/devin-probe-watch/probe-facts.json`):
//! `UserPromptSubmit.prompt`, `PreToolUse.tool_name` · `tool_input` ·
//! `tool_use_id`, `PostToolUse.tool_response` `{success, output, error}`,
//! `Stop.last_assistant_message`, and on every one the turn's `prompt_id`.
//! They are folded into the transcript's own turn shape through its own tool
//! readers (`transcript::tool_call_turn`, `transcript::tool_result_turn`), so
//! a call's kind and file are read where every transcript's are.
//!
//! The document wins. Once it holds the turn the buffer lets the turn go
//! ([`LiveTurnBuffer::reconcile_turns`]), and a hook of a turn let go — one
//! that comes late, or twice — draws nothing, so no step stands twice.
//! Nothing here branches on an agent's name or touches the disk.

use crate::hook::{normalized_event, worker_call_id_in};
use crate::payload::HookPayload;
use crate::transcript::{
    Detail, TranscriptTurn, message_text, prompt_words_in_parsed, tool_call_turn, tool_result_turn,
};

/// How many let-go turns' `prompt_id`s a buffer remembers, so that a hook of
/// one of them arriving after the turns that followed it began still draws
/// nothing: the turns just before the running one.
const LET_GO_PROMPTS_KEPT: usize = 4;

/// The turn a pane's CLI is running and has not written yet, as its hooks
/// told it — or nothing, between turns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveTurnBuffer {
    /// The session these turns are of; a hook of another starts afresh.
    session_id: Option<String>,
    /// Whether a turn is held: its prompt was heard, or a hook of a turn the
    /// window met mid-way.
    begun: bool,
    /// The running turn's `prompt_id`, when its hooks carry one.
    prompt_id: Option<String>,
    /// The person's words, whitespace collapsed, to know the turn in the
    /// document by.
    prompt_said: Option<String>,
    /// The document's settled steps when the turn began: the steps before it
    /// are earlier turns'.
    base_steps: usize,
    /// The settled steps when the turn's `Stop` came. The CLI writes the turn
    /// after its `Stop`, so a document past this holds it.
    stop_steps: Option<usize>,
    /// The turns to show, in order.
    turns: Vec<TranscriptTurn>,
    /// The calls of the running turn drawn as a call, and as a result.
    seen_calls: Vec<String>,
    seen_results: Vec<String>,
    /// The turns let go, by `prompt_id`, and the calls of the last one: a
    /// hook of theirs draws nothing.
    let_go_prompts: Vec<String>,
    let_go_calls: Vec<String>,
}

impl LiveTurnBuffer {
    /// The turns to show after the document's, in order.
    #[must_use]
    pub fn turns(&self) -> &[TranscriptTurn] {
        &self.turns
    }

    /// Whether there is nothing to show.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// One hook of the pane's CLI, heard when its document held `settled`
    /// steps; `true` when the turns to show changed. A hook twice, a result
    /// before its call, a result after the `Stop`, and a hook of a turn let
    /// go each leave one row per step.
    pub fn note_hook(
        &mut self,
        event_name: &str,
        payload: &HookPayload<'_>,
        session_id: Option<&str>,
        settled: usize,
        at_ms: Option<i64>,
    ) -> bool {
        if let Some(session) = session_id
            && self.session_id.as_deref() != Some(session)
        {
            *self = Self {
                session_id: Some(session.to_string()),
                ..Self::default()
            };
        }
        let Some(tree) = payload.tree() else {
            return false;
        };
        let event = normalized_event(event_name);
        if event == "userpromptsubmit" {
            return self.note_prompt(payload, tree, settled, at_ms);
        }
        if !matches!(event.as_str(), "pretooluse" | "posttooluse" | "stop") {
            return false;
        }
        let call_id = worker_call_id_in(tree);
        if self.let_go_of(tree, call_id.as_deref()) {
            return false;
        }
        if !self.begun {
            // A turn the window met mid-way — it began before the window
            // heard its prompt: its steps from here on.
            self.begun = true;
            self.base_steps = settled;
            self.prompt_id = prompt_id_in(tree);
        }
        match event.as_str() {
            "pretooluse" => self.note_call(tree, call_id, at_ms),
            "posttooluse" => self.note_result(tree, call_id, at_ms),
            _ => self.note_stop(tree, settled, at_ms),
        }
    }

    /// The document read again: `settled` steps, of which `new_turns` are the
    /// ones this read brought. Once the document holds the running turn it
    /// is let go — the document's rows stand for it — and until then a
    /// document that grew holds an earlier turn written late.
    ///
    /// The turn is written once the document grew past where it stood at the
    /// turn's `Stop` (its CLI writes it after that), or holds the person's
    /// words as a prompt of its own; a document that shrank was replaced,
    /// and what it holds now stands for the turn too.
    pub fn reconcile_turns(&mut self, settled: usize, new_turns: &[&TranscriptTurn]) {
        if !self.begun || settled == self.base_steps {
            return;
        }
        let written = settled < self.base_steps
            || self.stop_steps.is_some_and(|at| settled > at)
            || self.prompt_said.as_deref().is_some_and(|said| {
                new_turns
                    .iter()
                    .any(|turn| turn.role == "user" && collapsed(&turn.text) == said)
            });
        if written {
            self.let_go();
        } else {
            self.base_steps = settled;
        }
    }

    /// Whether a hook is of a turn let go — by its `prompt_id`, or by a call
    /// of the last turn let go — or of another prompt than the running one.
    fn let_go_of(&self, tree: &serde_json::Value, call_id: Option<&str>) -> bool {
        let prompt = prompt_id_in(tree);
        if let Some(prompt) = prompt.as_deref() {
            if self.let_go_prompts.iter().any(|gone| gone == prompt) {
                return true;
            }
            if self.begun && self.prompt_id.as_deref().is_some_and(|held| held != prompt) {
                return true;
            }
        }
        call_id.is_some_and(|id| self.let_go_calls.iter().any(|gone| gone == id))
    }

    /// Lets the running turn go: its rows leave, and its `prompt_id` and
    /// calls are remembered so that its late hooks draw nothing.
    fn let_go(&mut self) {
        if let Some(prompt) = self.prompt_id.take() {
            if self.let_go_prompts.len() == LET_GO_PROMPTS_KEPT {
                self.let_go_prompts.remove(0);
            }
            self.let_go_prompts.push(prompt);
        }
        let mut calls = std::mem::take(&mut self.seen_calls);
        for id in std::mem::take(&mut self.seen_results) {
            if !calls.contains(&id) {
                calls.push(id);
            }
        }
        if !calls.is_empty() {
            self.let_go_calls = calls;
        }
        self.begun = false;
        self.prompt_said = None;
        self.base_steps = 0;
        self.stop_steps = None;
        self.turns.clear();
    }

    /// A prompt begins a turn of its own, letting the one before it go.
    fn note_prompt(
        &mut self,
        payload: &HookPayload<'_>,
        tree: &serde_json::Value,
        settled: usize,
        at_ms: Option<i64>,
    ) -> bool {
        let had = !self.turns.is_empty();
        self.let_go();
        self.begun = true;
        self.base_steps = settled;
        self.prompt_id = prompt_id_in(tree);
        let Some(words) = prompt_words_in_parsed(payload) else {
            return had;
        };
        self.prompt_said = Some(collapsed(&words));
        self.turns.push(said("user", words, at_ms));
        true
    }

    fn note_call(
        &mut self,
        tree: &serde_json::Value,
        call_id: Option<String>,
        at_ms: Option<i64>,
    ) -> bool {
        if call_id
            .as_ref()
            .is_some_and(|id| self.seen_calls.contains(id))
        {
            return false;
        }
        let Some(call) = hook_call(tree, call_id.as_deref(), at_ms) else {
            return false;
        };
        if let Some(id) = call_id {
            self.seen_calls.push(id);
        }
        let at = self.before_the_answer();
        self.turns.insert(at, call);
        true
    }

    /// A result stands right after its call — the call drawn from the result
    /// itself when the result came first, and before the last answer when it
    /// came after the `Stop`.
    fn note_result(
        &mut self,
        tree: &serde_json::Value,
        call_id: Option<String>,
        at_ms: Option<i64>,
    ) -> bool {
        if call_id
            .as_ref()
            .is_some_and(|id| self.seen_results.contains(id))
        {
            return false;
        }
        let drawn = call_id.as_deref().and_then(|id| {
            self.turns.iter().position(|turn| {
                turn.role == "tool" && turn.tool.as_ref().is_some_and(|tool| tool.call_id == id)
            })
        });
        let at = if let Some(call) = drawn {
            call + 1
        } else {
            let at = self.before_the_answer();
            match hook_call(tree, call_id.as_deref(), at_ms) {
                Some(call) => {
                    if let Some(id) = call_id.as_ref() {
                        self.seen_calls.push(id.clone());
                    }
                    self.turns.insert(at, call);
                    at + 1
                }
                None => at,
            }
        };
        self.turns
            .insert(at, hook_result(tree, call_id.as_deref(), at_ms));
        if let Some(id) = call_id {
            self.seen_results.push(id);
        }
        true
    }

    fn note_stop(&mut self, tree: &serde_json::Value, settled: usize, at_ms: Option<i64>) -> bool {
        self.stop_steps = Some(settled);
        let Some(text) = message_text(tree.get("last_assistant_message")) else {
            return false;
        };
        if let Some(last) = self
            .turns
            .last_mut()
            .filter(|turn| turn.role == "assistant")
        {
            if last.text == text {
                return false;
            }
            last.text = text;
            last.at_ms = at_ms;
            return true;
        }
        self.turns.push(said("assistant", text, at_ms));
        true
    }

    /// Where a step goes: before the last answer once the `Stop` drew one,
    /// else at the end.
    fn before_the_answer(&self) -> usize {
        let answered = self.stop_steps.is_some()
            && self
                .turns
                .last()
                .is_some_and(|turn| turn.role == "assistant");
        self.turns.len() - usize::from(answered)
    }
}

/// The turn's `prompt_id`, which Devin's hooks carry on every event.
fn prompt_id_in(tree: &serde_json::Value) -> Option<String> {
    tree.get("prompt_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Words with every run of whitespace one space, to know a prompt by.
fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn said(role: &str, text: String, at_ms: Option<i64>) -> TranscriptTurn {
    TranscriptTurn {
        role: role.into(),
        text,
        at_ms,
        tool: None,
        images: Vec::new(),
    }
}

/// A call as the transcript's tool reader draws one, out of a hook's
/// `tool_name` and `tool_input`.
fn hook_call(
    tree: &serde_json::Value,
    call_id: Option<&str>,
    at_ms: Option<i64>,
) -> Option<TranscriptTurn> {
    let name = tree
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())?;
    let part = serde_json::json!({
        "name": name,
        "arguments": tree.get("tool_input"),
        "call_id": call_id,
    });
    tool_call_turn(&part, at_ms, Detail::Clipped)
}

/// A result as the transcript's tool reader draws one, out of a hook's
/// `tool_response`: its output, else its error, and failed when it says it
/// did not succeed or names an error.
fn hook_result(
    tree: &serde_json::Value,
    call_id: Option<&str>,
    at_ms: Option<i64>,
) -> TranscriptTurn {
    let response = tree.get("tool_response");
    let text_of = |key: &str| {
        response
            .and_then(|response| response.get(key))
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.trim().is_empty())
    };
    let error = text_of("error");
    let failed = error.is_some()
        || response
            .and_then(|response| response.get("success"))
            .and_then(serde_json::Value::as_bool)
            == Some(false);
    let part = serde_json::json!({
        "tool_name": tree.get("tool_name"),
        "call_id": call_id,
        "content": text_of("output").or(error),
        "is_error": failed,
    });
    tool_result_turn(&part, at_ms, Detail::Clipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One hook into `buffer`, its payload written as JSON.
    fn hook(
        buffer: &mut LiveTurnBuffer,
        event: &str,
        body: &serde_json::Value,
        settled: usize,
        at_ms: i64,
    ) -> bool {
        let raw = body.to_string();
        buffer.note_hook(
            event,
            &HookPayload::of(&raw),
            Some("example-session"),
            settled,
            Some(at_ms),
        )
    }

    fn roles(buffer: &LiveTurnBuffer) -> Vec<&str> {
        buffer
            .turns()
            .iter()
            .map(|turn| turn.role.as_str())
            .collect()
    }

    fn call(id: &str, prompt: &str) -> serde_json::Value {
        serde_json::json!({"prompt_id": prompt, "tool_name": "exec", "tool_input": {"command": "ls"}, "tool_use_id": id})
    }

    fn result(id: &str, prompt: &str, success: bool) -> serde_json::Value {
        let response = if success {
            serde_json::json!({"success": true, "output": "a.rs", "error": null})
        } else {
            serde_json::json!({"success": false, "output": "", "error": "not a repository"})
        };
        serde_json::json!({
            "prompt_id": prompt, "tool_name": "exec", "tool_input": {"command": "ls"}, "tool_use_id": id,
            "tool_response": response,
        })
    }

    fn user(text: &str) -> TranscriptTurn {
        said("user", text.to_string(), Some(1))
    }

    /// A turn told by its hooks — the prompt, a call, its result, the last
    /// answer — stands in order while the document does not hold it, and
    /// goes once it does.
    #[test]
    fn a_turn_told_by_its_hooks_stands_until_the_document_holds_it() {
        let mut buffer = LiveTurnBuffer::default();
        assert!(hook(
            &mut buffer,
            "UserPromptSubmit",
            &serde_json::json!({"prompt": "list\n  the files", "prompt_id": "p-1"}),
            3,
            1_000
        ));
        assert!(hook(
            &mut buffer,
            "PreToolUse",
            &call("c-1", "p-1"),
            3,
            2_000
        ));
        assert!(hook(
            &mut buffer,
            "PostToolUse",
            &result("c-1", "p-1", true),
            3,
            2_100
        ));
        assert!(hook(
            &mut buffer,
            "Stop",
            &serde_json::json!({"prompt_id": "p-1", "last_assistant_message": "Found a.rs."}),
            3,
            3_000
        ));
        assert_eq!(roles(&buffer), ["user", "tool", "tool_result", "assistant"]);
        assert_eq!(collapsed(&buffer.turns()[0].text), "list the files");
        assert_eq!(
            buffer.turns()[1]
                .tool
                .as_ref()
                .map(|tool| tool.call_id.as_str()),
            Some("c-1")
        );
        assert_eq!(buffer.turns()[2].text, "a.rs");
        assert_eq!(
            buffer.turns()[2].tool.as_ref().map(|tool| tool.is_error),
            Some(false)
        );
        assert_eq!(buffer.turns()[3].text, "Found a.rs.");

        // A read that brought nothing new keeps the turn standing.
        buffer.reconcile_turns(3, &[]);
        assert_eq!(buffer.turns().len(), 4);
        // The document written after the Stop holds it: the turn goes.
        let written = [user("list the files")];
        buffer.reconcile_turns(5, &written.iter().collect::<Vec<_>>());
        assert!(buffer.is_empty(), "the document's rows stand for the turn");
    }

    /// Hooks that come twice, a result before its call, and a result after
    /// the `Stop` leave one row per step, a result before the last answer.
    #[test]
    fn hooks_out_of_order_or_twice_leave_one_row_a_step() {
        let mut buffer = LiveTurnBuffer::default();
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &serde_json::json!({"prompt": "check status", "prompt_id": "p-1"}),
            0,
            1_000,
        );
        assert!(hook(
            &mut buffer,
            "PreToolUse",
            &call("c-1", "p-1"),
            0,
            1_100
        ));
        assert!(!hook(
            &mut buffer,
            "PreToolUse",
            &call("c-1", "p-1"),
            0,
            1_105
        ));
        // The Stop before the call's result.
        assert!(hook(
            &mut buffer,
            "Stop",
            &serde_json::json!({"prompt_id": "p-1", "last_assistant_message": "Not a repository."}),
            0,
            1_200
        ));
        assert!(hook(
            &mut buffer,
            "PostToolUse",
            &result("c-1", "p-1", false),
            0,
            1_250
        ));
        assert!(!hook(
            &mut buffer,
            "PostToolUse",
            &result("c-1", "p-1", false),
            0,
            1_260
        ));
        assert_eq!(roles(&buffer), ["user", "tool", "tool_result", "assistant"]);
        assert_eq!(buffer.turns()[2].text, "not a repository");
        assert_eq!(
            buffer.turns()[2].tool.as_ref().map(|tool| tool.is_error),
            Some(true)
        );

        // A result before its call draws the call from itself, once.
        let mut first = LiveTurnBuffer::default();
        assert!(hook(
            &mut first,
            "PostToolUse",
            &result("c-9", "p-9", true),
            0,
            2_000
        ));
        assert!(!hook(
            &mut first,
            "PreToolUse",
            &call("c-9", "p-9"),
            0,
            2_010
        ));
        assert_eq!(roles(&first), ["tool", "tool_result"]);
    }

    /// A hook of a turn the document already holds — late, or sent again —
    /// draws nothing: not by its `prompt_id`, not by its call's id when it
    /// carries no `prompt_id`.
    #[test]
    fn a_hook_that_comes_after_its_turn_was_written_draws_nothing() {
        let mut buffer = LiveTurnBuffer::default();
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &serde_json::json!({"prompt": "build it", "prompt_id": "p-1"}),
            3,
            1_000,
        );
        hook(&mut buffer, "PreToolUse", &call("c-1", "p-1"), 3, 1_100);
        hook(
            &mut buffer,
            "Stop",
            &serde_json::json!({"prompt_id": "p-1", "last_assistant_message": "Built."}),
            3,
            1_200,
        );
        let written = [user("build it")];
        buffer.reconcile_turns(6, &written.iter().collect::<Vec<_>>());
        assert!(buffer.is_empty());

        assert!(!hook(
            &mut buffer,
            "PostToolUse",
            &result("c-1", "p-1", true),
            6,
            1_300
        ));
        assert!(!hook(
            &mut buffer,
            "Stop",
            &serde_json::json!({"prompt_id": "p-1", "last_assistant_message": "Built."}),
            6,
            1_310
        ));
        let mut bare = result("c-1", "p-1", true);
        bare.as_object_mut().expect("an object").remove("prompt_id");
        assert!(!hook(&mut buffer, "PostToolUse", &bare, 6, 1_320));
        assert!(
            buffer.is_empty(),
            "a late hook stood a step the document holds: {:?}",
            roles(&buffer)
        );

        // The next prompt's own hooks still stand.
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &serde_json::json!({"prompt": "test it", "prompt_id": "p-2"}),
            6,
            2_000,
        );
        assert!(hook(
            &mut buffer,
            "PreToolUse",
            &call("c-2", "p-2"),
            6,
            2_100
        ));
        // A hook of another prompt joins no turn but its own.
        assert!(!hook(
            &mut buffer,
            "PreToolUse",
            &call("c-3", "p-1"),
            6,
            2_200
        ));
        assert_eq!(roles(&buffer), ["user", "tool"]);
    }

    /// An earlier turn the CLI wrote late, as the next one began, leaves the
    /// running turn standing — a prompt that only begins like the earlier
    /// one too — and the running turn goes once the document holds it.
    #[test]
    fn an_earlier_turn_written_late_leaves_the_running_turn_standing() {
        let mut buffer = LiveTurnBuffer::default();
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &serde_json::json!({"prompt": "check", "prompt_id": "p-2"}),
            0,
            5_000,
        );
        let earlier = [
            user("check status"),
            said("assistant", "clean".into(), Some(2)),
        ];
        buffer.reconcile_turns(3, &earlier.iter().collect::<Vec<_>>());
        assert_eq!(
            roles(&buffer),
            ["user"],
            "the running turn went with an earlier one"
        );
        let running = [user("check"), said("assistant", "done".into(), Some(6))];
        buffer.reconcile_turns(5, &running.iter().collect::<Vec<_>>());
        assert!(buffer.is_empty());
    }

    /// A turn whose words the document keeps otherwise is let go once the
    /// document grows past where it stood at the turn's `Stop`.
    #[test]
    fn a_stopped_turn_goes_once_the_document_grows_past_its_stop() {
        let mut buffer = LiveTurnBuffer::default();
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &serde_json::json!({"prompt": "go on", "prompt_id": "p-1"}),
            4,
            1_000,
        );
        hook(
            &mut buffer,
            "Stop",
            &serde_json::json!({"prompt_id": "p-1", "last_assistant_message": "Went on."}),
            4,
            2_000,
        );
        let kept_otherwise = [user("<context>go on</context>")];
        buffer.reconcile_turns(6, &kept_otherwise.iter().collect::<Vec<_>>());
        assert!(buffer.is_empty());
    }
}
