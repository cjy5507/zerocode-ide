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
//! Each turn is held on its own, by its `prompt_id` (t-125261, the fix round
//! after the cross-provider review). A turn's hooks touch only its rows; a
//! turn stands until the document holds it — the next prompt does not take
//! it — and the document is asked of the turns in order
//! ([`LiveTurnBuffer::reconcile`]): by the turn's own prompt among the steps
//! written since it began, or, for a turn whose words the document keeps
//! otherwise, by a write after its `Stop`. A write is known by the session
//! file's size and time ([`DocStamp`]) as the shell saw them; a document not
//! written since a turn began cannot hold it, whatever words it repeats.
//!
//! A hook of a turn that has ended — its `Stop` heard, a newer turn begun, or
//! the document holding it — adds what it says to that turn and says nothing
//! of what the pane does now ([`HookFold::late`]): a result that comes after
//! its turn's `Stop` joins its call, and the pane stays done. A prompt heard
//! already draws nothing. Nothing here branches on an agent's name or touches
//! the disk.

use crate::hook::{normalized_event, worker_call_id_in};
use crate::payload::HookPayload;
use crate::transcript::{
    Detail, TranscriptTurn, message_text, prompt_words_in_parsed, tool_call_turn, tool_result_turn,
};

/// A session file's size and time of last write, as the shell read them: the
/// one mark of a write this module compares, never reads.
pub type DocStamp = (u64, Option<std::time::SystemTime>);

/// How many turns a buffer remembers, the newest last: those the document does
/// not hold yet stand, and those it does are kept by their ids, so a hook of
/// one that comes late draws nothing. A page that reads lets a turn go within
/// a turn's end; more than this stand only while nothing reads, and the
/// document holds the oldest of them by then.
const TURNS_KEPT: usize = 4;

/// The session document as a read that parsed it found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocRead {
    /// Its file's size and time when the read began.
    pub stamp: DocStamp,
    /// Its settled steps ([`crate::transcript::DocumentRead::settled`]).
    pub settled: usize,
}

/// What one hook did to the turns a page shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HookFold {
    /// The rows to show changed.
    pub drawn: bool,
    /// The hook is of a turn that has ended — its `Stop` was heard, a newer
    /// turn began, or the document holds it — or is a prompt heard already:
    /// it says nothing of what the pane does now.
    pub late: bool,
}

/// One turn, as its hooks told it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LiveTurn {
    /// The turn's `prompt_id`, when its hooks carry one.
    prompt_id: Option<String>,
    /// The person's words, whitespace collapsed, to know the turn in the
    /// document by.
    said: Option<String>,
    /// The document's settled steps as last read when the turn began: its
    /// prompt stands at this step or after it.
    base: usize,
    /// The document's stamp when the turn began (`None`: no file yet).
    began: Option<DocStamp>,
    /// Once the turn has ended — its `Stop`, or a newer turn begun — the
    /// document's stamp then. The CLI writes the turn after that.
    stopped: Option<Option<DocStamp>>,
    /// The document holds the turn: its rows are let go.
    written: bool,
    /// The rows to show, in order.
    rows: Vec<TranscriptTurn>,
    /// The calls drawn as a call, and as a result.
    calls: Vec<String>,
    results: Vec<String>,
}

impl LiveTurn {
    fn ended(&self) -> bool {
        self.stopped.is_some() || self.written
    }

    fn drew_call(&self, id: &str) -> bool {
        self.calls
            .iter()
            .chain(&self.results)
            .any(|drawn| drawn == id)
    }

    fn note_call(
        &mut self,
        tree: &serde_json::Value,
        call_id: Option<String>,
        at_ms: Option<i64>,
    ) -> bool {
        if call_id.as_ref().is_some_and(|id| self.calls.contains(id)) {
            return false;
        }
        let Some(call) = hook_call(tree, call_id.as_deref(), at_ms) else {
            return false;
        };
        if let Some(id) = call_id {
            self.calls.push(id);
        }
        let at = self.before_the_answer();
        self.rows.insert(at, call);
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
        if call_id.as_ref().is_some_and(|id| self.results.contains(id)) {
            return false;
        }
        let drawn = call_id.as_deref().and_then(|id| {
            self.rows.iter().position(|row| {
                row.role == "tool" && row.tool.as_ref().is_some_and(|tool| tool.call_id == id)
            })
        });
        let at = if let Some(call) = drawn {
            call + 1
        } else {
            let at = self.before_the_answer();
            match hook_call(tree, call_id.as_deref(), at_ms) {
                Some(call) => {
                    if let Some(id) = call_id.as_ref() {
                        self.calls.push(id.clone());
                    }
                    self.rows.insert(at, call);
                    at + 1
                }
                None => at,
            }
        };
        self.rows
            .insert(at, hook_result(tree, call_id.as_deref(), at_ms));
        if let Some(id) = call_id {
            self.results.push(id);
        }
        true
    }

    /// The `Stop`'s last answer, once: the same answer again changes nothing.
    fn note_answer(&mut self, tree: &serde_json::Value, at_ms: Option<i64>) -> bool {
        let Some(text) = message_text(tree.get("last_assistant_message")) else {
            return false;
        };
        if let Some(last) = self.rows.last_mut().filter(|row| row.role == "assistant") {
            if last.text == text {
                return false;
            }
            last.text = text;
            last.at_ms = at_ms;
            return true;
        }
        self.rows.push(said("assistant", text, at_ms));
        true
    }

    /// Where a step goes: before the last answer once the `Stop` drew one,
    /// else at the end.
    fn before_the_answer(&self) -> usize {
        let answered =
            self.stopped.is_some() && self.rows.last().is_some_and(|row| row.role == "assistant");
        self.rows.len() - usize::from(answered)
    }
}

/// The turns a pane's CLI is running or has ended and not written yet, as its
/// hooks told them — and the document as the page last read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveTurnBuffer {
    /// The session these turns are of: a hook of another starts afresh, and a
    /// read of another leaves them be ([`Self::reconcile`]).
    session_id: Option<String>,
    /// The turns remembered, oldest first, at most [`TURNS_KEPT`].
    turns: Vec<LiveTurn>,
    /// The last read of the session document that parsed it.
    read: Option<DocRead>,
    /// How many reads parsed the document since the buffer began.
    reads: u64,
}

impl LiveTurnBuffer {
    /// The rows to show after the document's, in order: every turn it does
    /// not hold yet.
    #[must_use]
    pub fn rows(&self) -> Vec<TranscriptTurn> {
        self.turns
            .iter()
            .filter(|turn| !turn.written)
            .flat_map(|turn| turn.rows.iter().cloned())
            .collect()
    }

    /// [`Self::rows`], when they are of `session_id`'s conversation — none for
    /// another's: a pane whose CLI or conversation changed shows no row of
    /// the one before.
    #[must_use]
    pub fn rows_of(&self, session_id: Option<&str>) -> Vec<TranscriptTurn> {
        if self.session_id.as_deref() == session_id {
            self.rows()
        } else {
            Vec::new()
        }
    }

    /// Whether there is nothing to show.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turns
            .iter()
            .all(|turn| turn.written || turn.rows.is_empty())
    }

    /// How many reads parsed the document since the buffer began: what the
    /// looks after a turn's end take at its `Stop` ([`Self::read_since`]).
    #[must_use]
    pub fn reads(&self) -> u64 {
        self.reads
    }

    /// Whether, since the buffer had counted `reads`, the page read a document
    /// that parsed and whose file is not the one stamped `at_stop` — one
    /// written since the turn ended, which holds it: what the looks after a
    /// turn's end ask before they tell the page again.
    #[must_use]
    pub fn read_since(&self, reads: u64, at_stop: Option<DocStamp>) -> bool {
        self.reads > reads && self.read.is_some_and(|read| Some(read.stamp) != at_stop)
    }

    /// Whether a read on from `after` steps of `session_id`'s document, its
    /// file at `stamp`, finds what the last read found — nothing new — so the
    /// document need not be read and parsed again.
    #[must_use]
    pub fn read_unchanged(&self, session_id: Option<&str>, stamp: DocStamp, after: usize) -> bool {
        self.session_id.as_deref() == session_id
            && self.read
                == Some(DocRead {
                    stamp,
                    settled: after,
                })
    }

    /// One hook of the pane's CLI, heard while its session file stood at
    /// `doc` (`None`: no file yet). A hook twice, a result before its call, a
    /// result after the `Stop`, and a hook of a turn the document holds each
    /// leave one row per step.
    pub fn note_hook(
        &mut self,
        event_name: &str,
        payload: &HookPayload<'_>,
        session_id: Option<&str>,
        doc: Option<DocStamp>,
        at_ms: Option<i64>,
    ) -> HookFold {
        if let Some(session) = session_id
            && self.session_id.as_deref() != Some(session)
        {
            *self = Self {
                session_id: Some(session.to_string()),
                ..Self::default()
            };
        }
        let Some(tree) = payload.tree() else {
            return HookFold::default();
        };
        let event = normalized_event(event_name);
        let prompt_id = prompt_id_in(tree);
        let fold = match event.as_str() {
            "userpromptsubmit" => self.note_prompt(payload, prompt_id, doc, at_ms),
            "pretooluse" | "posttooluse" => {
                let call_id = worker_call_id_in(tree);
                let at = self.turn_of(prompt_id, call_id.as_deref(), false, doc);
                let late = at + 1 != self.turns.len() || self.turns[at].ended();
                let turn = &mut self.turns[at];
                let drawn = !turn.written
                    && if event == "pretooluse" {
                        turn.note_call(tree, call_id, at_ms)
                    } else {
                        turn.note_result(tree, call_id, at_ms)
                    };
                HookFold { drawn, late }
            }
            "stop" => {
                let at = self.turn_of(prompt_id, None, true, doc);
                let late = at + 1 != self.turns.len() || self.turns[at].stopped.is_some();
                let turn = &mut self.turns[at];
                if turn.stopped.is_none() {
                    turn.stopped = Some(doc);
                }
                let drawn = !turn.written && turn.note_answer(tree, at_ms);
                HookFold { drawn, late }
            }
            _ => return HookFold::default(),
        };
        // A hook begins one turn at most: the oldest goes past the count.
        if self.turns.len() > TURNS_KEPT {
            self.turns.remove(0);
        }
        fold
    }

    /// The document read again, `read` what it found and `brought` the rows
    /// this read brought, each with its step's place. The turns the document
    /// does not hold yet are asked in order, and the first it does not hold
    /// ends the asking — the CLI writes its turns in order:
    ///
    /// - a document not written since the turn began does not hold it;
    /// - one that brings the turn's prompt, at or after where the turn began
    ///   and after the prompt of the turn before it, holds it — a running
    ///   turn too, whose `Stop` is still on its way;
    /// - one written after the turn's `Stop` holds it, whatever its words.
    ///
    /// A document with fewer settled steps than the last read was replaced: a
    /// prompt may stand anywhere in it. A read of another session than the
    /// hooks told says nothing of their turns — the pane's record of its
    /// conversation can trail its newest hook by a moment — and a buffer no
    /// hook named a session takes the read's.
    pub fn reconcile(
        &mut self,
        session_id: Option<&str>,
        read: DocRead,
        brought: &[(usize, &TranscriptTurn)],
    ) {
        if self.session_id.as_deref() != session_id {
            if self.session_id.is_some() {
                return;
            }
            self.session_id = session_id.map(str::to_string);
        }
        if self.read.is_some_and(|last| read.settled < last.settled) {
            for turn in &mut self.turns {
                turn.base = 0;
            }
        }
        let mut from = 0;
        for turn in self.turns.iter_mut().filter(|turn| !turn.written) {
            if turn.began == Some(read.stamp) {
                break;
            }
            let prompt = turn.said.as_deref().and_then(|said| {
                brought
                    .iter()
                    .find(|(place, row)| {
                        *place >= turn.base.max(from)
                            && row.role == "user"
                            && collapsed(&row.text) == said
                    })
                    .map(|(place, _)| *place)
            });
            if let Some(place) = prompt {
                from = place + 1;
            } else if turn
                .stopped
                .is_none_or(|at_stop| at_stop == Some(read.stamp))
            {
                break;
            }
            turn.written = true;
            turn.rows.clear();
        }
        self.read = Some(read);
        self.reads = self.reads.saturating_add(1);
    }

    /// A prompt begins a turn of its own — unless its turn is told already. A
    /// prompt heard already, late or twice, draws nothing; one that comes
    /// after its turn's first call, the hooks out of order, gives the turn its
    /// words.
    fn note_prompt(
        &mut self,
        payload: &HookPayload<'_>,
        prompt_id: Option<String>,
        doc: Option<DocStamp>,
        at_ms: Option<i64>,
    ) -> HookFold {
        let words = prompt_words_in_parsed(payload);
        if let Some(prompt) = prompt_id.as_deref()
            && let Some(at) = self
                .turns
                .iter()
                .position(|turn| turn.prompt_id.as_deref() == Some(prompt))
        {
            let newest = at + 1 == self.turns.len();
            let turn = &mut self.turns[at];
            let late = !newest || turn.ended();
            let Some(words) = words.filter(|_| turn.said.is_none() && !turn.written) else {
                return HookFold {
                    drawn: false,
                    late: true,
                };
            };
            turn.said = Some(collapsed(&words));
            turn.rows.insert(0, said("user", words, at_ms));
            return HookFold { drawn: true, late };
        }
        let at = self.begin(prompt_id, words.as_deref().map(collapsed), doc);
        let Some(words) = words else {
            return HookFold::default();
        };
        self.turns[at].rows.push(said("user", words, at_ms));
        HookFold {
            drawn: true,
            late: false,
        }
    }

    /// The turn a call, a result or a `Stop` is of: the one its `prompt_id`
    /// names; without one, the turn that drew its call, else the newest — a
    /// `Stop` the newest whatever it is, a call only while it runs; else a
    /// turn the window meets mid-way, begun now.
    fn turn_of(
        &mut self,
        prompt_id: Option<String>,
        call_id: Option<&str>,
        stop: bool,
        doc: Option<DocStamp>,
    ) -> usize {
        let newest = self.turns.len().checked_sub(1);
        let held = match prompt_id.as_deref() {
            Some(prompt) => self
                .turns
                .iter()
                .position(|turn| turn.prompt_id.as_deref() == Some(prompt)),
            None => call_id
                .and_then(|id| self.turns.iter().position(|turn| turn.drew_call(id)))
                .or_else(|| newest.filter(|&at| stop || !self.turns[at].ended())),
        };
        held.unwrap_or_else(|| self.begin(prompt_id, None, doc))
    }

    /// A turn begins, where the document as last read ends. A turn still
    /// running ended unheard — its `Stop` never came — and waits for the
    /// document from now, as a stopped one does.
    fn begin(
        &mut self,
        prompt_id: Option<String>,
        said: Option<String>,
        doc: Option<DocStamp>,
    ) -> usize {
        for turn in &mut self.turns {
            if !turn.ended() {
                turn.stopped = Some(doc);
            }
        }
        self.turns.push(LiveTurn {
            prompt_id,
            said,
            base: self.read.map_or(0, |read| read.settled),
            began: doc,
            ..LiveTurn::default()
        });
        self.turns.len() - 1
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

    const SESSION: &str = "example-session";

    /// A file stamp a test makes up: `n` bytes, no time.
    fn stamp(n: u64) -> Option<DocStamp> {
        Some((n, None))
    }

    /// One hook into `buffer`, its payload written as JSON, the session file
    /// at `doc`.
    fn hook(
        buffer: &mut LiveTurnBuffer,
        event: &str,
        body: &serde_json::Value,
        doc: Option<DocStamp>,
    ) -> HookFold {
        let raw = body.to_string();
        buffer.note_hook(
            event,
            &HookPayload::of(&raw),
            Some(SESSION),
            doc,
            Some(1_000),
        )
    }

    /// One read of the document: its file at `doc`, `settled` steps, and the
    /// rows it brought with their steps' places.
    fn read(
        buffer: &mut LiveTurnBuffer,
        doc: u64,
        settled: usize,
        brought: &[(usize, TranscriptTurn)],
    ) {
        let brought: Vec<(usize, &TranscriptTurn)> =
            brought.iter().map(|(place, row)| (*place, row)).collect();
        buffer.reconcile(
            Some(SESSION),
            DocRead {
                stamp: (doc, None),
                settled,
            },
            &brought,
        );
    }

    fn roles(buffer: &LiveTurnBuffer) -> Vec<String> {
        buffer.rows().into_iter().map(|row| row.role).collect()
    }

    fn prompt(text: &str, id: &str) -> serde_json::Value {
        serde_json::json!({"prompt": text, "prompt_id": id})
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

    fn stop(prompt: &str, answer: &str) -> serde_json::Value {
        serde_json::json!({"prompt_id": prompt, "last_assistant_message": answer})
    }

    fn user(text: &str) -> TranscriptTurn {
        said("user", text.to_string(), Some(1))
    }

    fn answer(text: &str) -> TranscriptTurn {
        said("assistant", text.to_string(), Some(2))
    }

    /// A turn told by its hooks — the prompt, a call, its result, the last
    /// answer — stands in order while the document does not hold it, and
    /// goes once it does.
    #[test]
    fn a_turn_told_by_its_hooks_stands_until_the_document_holds_it() {
        let mut buffer = LiveTurnBuffer::default();
        read(&mut buffer, 10, 3, &[]);
        assert!(
            hook(
                &mut buffer,
                "UserPromptSubmit",
                &prompt("list\n  the files", "p-1"),
                stamp(10)
            )
            .drawn
        );
        assert!(hook(&mut buffer, "PreToolUse", &call("c-1", "p-1"), stamp(10)).drawn);
        assert!(
            hook(
                &mut buffer,
                "PostToolUse",
                &result("c-1", "p-1", true),
                stamp(10)
            )
            .drawn
        );
        assert_eq!(
            hook(&mut buffer, "Stop", &stop("p-1", "Found a.rs."), stamp(10)),
            HookFold {
                drawn: true,
                late: false
            }
        );
        assert_eq!(roles(&buffer), ["user", "tool", "tool_result", "assistant"]);
        let rows = buffer.rows();
        assert_eq!(collapsed(&rows[0].text), "list the files");
        assert_eq!(
            rows[1].tool.as_ref().map(|tool| tool.call_id.as_str()),
            Some("c-1")
        );
        assert_eq!(rows[2].text, "a.rs");
        assert_eq!(rows[2].tool.as_ref().map(|tool| tool.is_error), Some(false));
        assert_eq!(rows[3].text, "Found a.rs.");
        // What the looks after the Stop take, and ask.
        let at_stop = buffer.reads();
        assert!(!buffer.read_since(at_stop, stamp(10)));

        // The document as it stood: nothing of the turn, and not a document
        // written since the Stop.
        read(&mut buffer, 10, 3, &[]);
        assert_eq!(buffer.rows().len(), 4);
        assert!(!buffer.read_since(at_stop, stamp(10)));
        // The document written after the Stop holds it: the turn goes.
        read(
            &mut buffer,
            11,
            6,
            &[(3, user("list the files")), (5, answer("Found a.rs."))],
        );
        assert!(buffer.is_empty(), "the document's rows stand for the turn");
        assert!(buffer.read_since(at_stop, stamp(10)));
        assert!(
            !buffer.read_since(buffer.reads(), stamp(10)),
            "no read since"
        );
    }

    /// Hooks that come twice, a result before its call, and a result after
    /// the `Stop` leave one row per step, a result before the last answer.
    #[test]
    fn hooks_out_of_order_or_twice_leave_one_row_a_step() {
        let mut buffer = LiveTurnBuffer::default();
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &prompt("check status", "p-1"),
            None,
        );
        assert!(hook(&mut buffer, "PreToolUse", &call("c-1", "p-1"), None).drawn);
        assert!(!hook(&mut buffer, "PreToolUse", &call("c-1", "p-1"), None).drawn);
        // The Stop before the call's result.
        assert!(hook(&mut buffer, "Stop", &stop("p-1", "Not a repository."), None).drawn);
        assert!(
            hook(
                &mut buffer,
                "PostToolUse",
                &result("c-1", "p-1", false),
                None
            )
            .drawn
        );
        assert!(
            !hook(
                &mut buffer,
                "PostToolUse",
                &result("c-1", "p-1", false),
                None
            )
            .drawn
        );
        assert_eq!(roles(&buffer), ["user", "tool", "tool_result", "assistant"]);
        let rows = buffer.rows();
        assert_eq!(rows[2].text, "not a repository");
        assert_eq!(rows[2].tool.as_ref().map(|tool| tool.is_error), Some(true));

        // A result before its call draws the call from itself, once.
        let mut first = LiveTurnBuffer::default();
        assert!(hook(&mut first, "PostToolUse", &result("c-9", "p-9", true), None).drawn);
        assert!(!hook(&mut first, "PreToolUse", &call("c-9", "p-9"), None).drawn);
        assert_eq!(roles(&first), ["tool", "tool_result"]);
    }

    /// F1: a result that comes after its turn's `Stop` joins its call before
    /// the last answer, and says nothing of the pane — it is late; so are a
    /// `Stop` heard twice and a call of a turn the document holds.
    #[test]
    fn a_result_after_its_turns_stop_joins_its_call_and_is_late() {
        let mut buffer = LiveTurnBuffer::default();
        assert!(
            !hook(
                &mut buffer,
                "UserPromptSubmit",
                &prompt("build it", "p-1"),
                None
            )
            .late
        );
        assert!(!hook(&mut buffer, "PreToolUse", &call("c-1", "p-1"), None).late);
        assert!(!hook(&mut buffer, "Stop", &stop("p-1", "Built."), None).late);
        assert_eq!(
            hook(
                &mut buffer,
                "PostToolUse",
                &result("c-1", "p-1", true),
                None
            ),
            HookFold {
                drawn: true,
                late: true
            },
            "a result after the Stop moved the pane"
        );
        assert_eq!(roles(&buffer), ["user", "tool", "tool_result", "assistant"]);
        assert_eq!(
            hook(&mut buffer, "Stop", &stop("p-1", "Built."), None),
            HookFold {
                drawn: false,
                late: true
            },
            "a Stop heard twice"
        );
        // Without a prompt id, by the call's id.
        let mut bare = result("c-1", "p-1", true);
        bare.as_object_mut().expect("an object").remove("prompt_id");
        assert!(hook(&mut buffer, "PostToolUse", &bare, None).late);
        read(
            &mut buffer,
            1,
            4,
            &[(0, user("build it")), (3, answer("Built."))],
        );
        assert!(buffer.is_empty());
        assert_eq!(
            hook(&mut buffer, "PreToolUse", &call("c-2", "p-1"), stamp(1)),
            HookFold {
                drawn: false,
                late: true
            },
            "a call of a written turn drew a row"
        );
    }

    /// F2: a prompt heard already — a hook sent twice, or one that comes after
    /// its turn was written — draws nothing and is late, and the next
    /// prompt's own hooks still stand.
    #[test]
    fn a_prompt_heard_already_draws_nothing() {
        let mut buffer = LiveTurnBuffer::default();
        read(&mut buffer, 10, 3, &[]);
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &prompt("build it", "p-1"),
            stamp(10),
        );
        assert_eq!(
            hook(
                &mut buffer,
                "UserPromptSubmit",
                &prompt("build it", "p-1"),
                stamp(10)
            ),
            HookFold {
                drawn: false,
                late: true
            }
        );
        hook(&mut buffer, "Stop", &stop("p-1", "Built."), stamp(10));
        read(
            &mut buffer,
            11,
            6,
            &[(3, user("build it")), (5, answer("Built."))],
        );
        assert!(buffer.is_empty());
        assert_eq!(
            hook(
                &mut buffer,
                "UserPromptSubmit",
                &prompt("build it", "p-1"),
                stamp(11)
            ),
            HookFold {
                drawn: false,
                late: true
            },
            "a written question stood again"
        );
        assert!(buffer.is_empty(), "{:?}", roles(&buffer));
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &prompt("test it", "p-2"),
            stamp(11),
        );
        assert!(hook(&mut buffer, "PreToolUse", &call("c-2", "p-2"), stamp(11)).drawn);
        assert_eq!(roles(&buffer), ["user", "tool"]);
    }

    /// Hooks out of order: a prompt that comes after its turn's first call
    /// gives the turn its words, ahead of the call, as the running turn's —
    /// not late — and the same prompt again draws nothing.
    #[test]
    fn a_prompt_after_its_turns_first_call_gives_the_turn_its_words() {
        let mut buffer = LiveTurnBuffer::default();
        hook(&mut buffer, "PreToolUse", &call("c-1", "p-1"), None);
        assert_eq!(
            hook(
                &mut buffer,
                "UserPromptSubmit",
                &prompt("list", "p-1"),
                None
            ),
            HookFold {
                drawn: true,
                late: false
            }
        );
        assert_eq!(roles(&buffer), ["user", "tool"]);
        assert_eq!(
            hook(
                &mut buffer,
                "UserPromptSubmit",
                &prompt("list", "p-1"),
                None
            ),
            HookFold {
                drawn: false,
                late: true
            }
        );
    }

    /// F2: the next prompt — queued behind a turn that has ended — leaves
    /// that turn standing until the document holds it, and the document
    /// that holds only the earlier turn leaves the next one standing, the
    /// same words or not.
    #[test]
    fn each_turn_stands_until_the_document_holds_it_the_same_words_too() {
        for words in ["check", "check status"] {
            let mut buffer = LiveTurnBuffer::default();
            read(&mut buffer, 10, 3, &[]);
            hook(
                &mut buffer,
                "UserPromptSubmit",
                &prompt("check", "p-1"),
                stamp(10),
            );
            hook(&mut buffer, "Stop", &stop("p-1", "clean"), stamp(10));
            // The next prompt before the CLI wrote the first turn.
            hook(
                &mut buffer,
                "UserPromptSubmit",
                &prompt(words, "p-2"),
                stamp(10),
            );
            assert_eq!(
                roles(&buffer),
                ["user", "assistant", "user"],
                "the turn before went at the next prompt ({words})"
            );
            hook(&mut buffer, "PreToolUse", &call("c-2", "p-2"), stamp(10));
            // The first turn written, late.
            read(
                &mut buffer,
                11,
                6,
                &[(3, user("check")), (5, answer("clean"))],
            );
            assert_eq!(
                roles(&buffer),
                ["user", "tool"],
                "the running turn went with the earlier one ({words})"
            );
            assert_eq!(buffer.rows()[0].text, words);
            // Then the second, after its Stop.
            hook(&mut buffer, "Stop", &stop("p-2", "done"), stamp(11));
            read(&mut buffer, 12, 9, &[(6, user(words)), (8, answer("done"))]);
            assert!(buffer.is_empty(), "{words}");
        }
    }

    /// F2: a document that shrank — replaced — holds a running turn only when
    /// it brings its prompt; a document not written since a turn began never
    /// holds it, though an older prompt repeats its words.
    #[test]
    fn a_replaced_or_unwritten_document_lets_no_running_turn_go_unasked() {
        let mut buffer = LiveTurnBuffer::default();
        read(&mut buffer, 10, 10, &[]);
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &prompt("go on", "p-1"),
            stamp(10),
        );
        hook(&mut buffer, "PreToolUse", &call("c-1", "p-1"), stamp(10));
        read(
            &mut buffer,
            4,
            4,
            &[(0, user("other")), (1, answer("an answer"))],
        );
        assert_eq!(
            roles(&buffer),
            ["user", "tool"],
            "a replaced document let the running turn go"
        );
        read(
            &mut buffer,
            5,
            6,
            &[
                (0, user("other")),
                (1, answer("an answer")),
                (4, user("go on")),
            ],
        );
        assert!(
            buffer.is_empty(),
            "the replaced document that brings the prompt holds it"
        );

        let mut fresh = LiveTurnBuffer::default();
        hook(
            &mut fresh,
            "UserPromptSubmit",
            &prompt("continue", "p-1"),
            stamp(7),
        );
        // A page opened mid-turn reads the document whole: an older turn
        // said the same, but nothing was written since this one began.
        read(
            &mut fresh,
            7,
            8,
            &[(2, user("continue")), (4, answer("an older answer"))],
        );
        assert_eq!(
            roles(&fresh),
            ["user"],
            "an older prompt's words let the running turn go"
        );
    }

    /// A turn whose words the document keeps otherwise goes once the
    /// document was written after its `Stop`; a turn ended unheard — a newer
    /// prompt came before its `Stop` — waits from that prompt.
    #[test]
    fn a_turn_kept_otherwise_goes_once_written_after_it_ended() {
        let mut buffer = LiveTurnBuffer::default();
        read(&mut buffer, 10, 4, &[]);
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &prompt("go on", "p-1"),
            stamp(10),
        );
        hook(&mut buffer, "Stop", &stop("p-1", "Went on."), stamp(10));
        read(&mut buffer, 10, 4, &[]);
        assert!(!buffer.is_empty(), "the document as it stood at the Stop");
        read(&mut buffer, 11, 6, &[(4, user("<context>go on</context>"))]);
        assert!(buffer.is_empty());

        // No Stop heard for p-2: p-3's prompt ends it.
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &prompt("first", "p-2"),
            stamp(11),
        );
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &prompt("second", "p-3"),
            stamp(12),
        );
        read(&mut buffer, 12, 8, &[]);
        assert_eq!(roles(&buffer), ["user", "user"]);
        read(
            &mut buffer,
            13,
            10,
            &[(6, user("<context>first</context>")), (8, user("second"))],
        );
        assert!(buffer.is_empty(), "{:?}", roles(&buffer));
    }

    /// A read that brings a turn's prompt before its `Stop` was heard holds
    /// the turn; the `Stop` that comes after it is still the turn's end.
    #[test]
    fn a_stop_heard_after_its_turn_was_read_is_not_late() {
        let mut buffer = LiveTurnBuffer::default();
        read(&mut buffer, 10, 3, &[]);
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &prompt("list", "p-1"),
            stamp(10),
        );
        read(
            &mut buffer,
            11,
            5,
            &[(3, user("list")), (4, answer("listed"))],
        );
        assert!(buffer.is_empty());
        assert_eq!(
            hook(&mut buffer, "Stop", &stop("p-1", "listed"), stamp(11)),
            HookFold {
                drawn: false,
                late: false
            }
        );
        assert!(buffer.is_empty(), "the Stop stood the written turn again");
    }

    /// F3: the rows are of the session the hooks told; a pane on another
    /// conversation shows none, and a read of another session leaves them
    /// as they are. F5: a read on from where the last one ended, of the same
    /// file, is known unchanged.
    #[test]
    fn rows_and_reads_are_of_one_session() {
        let mut buffer = LiveTurnBuffer::default();
        hook(
            &mut buffer,
            "UserPromptSubmit",
            &prompt("hello", "p-1"),
            None,
        );
        assert_eq!(buffer.rows_of(Some(SESSION)).len(), 1);
        assert!(buffer.rows_of(Some("another-session")).is_empty());
        assert!(buffer.rows_of(None).is_empty());

        read(&mut buffer, 10, 3, &[]);
        assert!(buffer.read_unchanged(Some(SESSION), (10, None), 3));
        assert!(
            !buffer.read_unchanged(Some(SESSION), (11, None), 3),
            "the file moved"
        );
        assert!(
            !buffer.read_unchanged(Some(SESSION), (10, None), 2),
            "another place"
        );
        assert!(!buffer.read_unchanged(Some("another-session"), (10, None), 3));
        buffer.reconcile(
            Some("another-session"),
            DocRead {
                stamp: (4, None),
                settled: 1,
            },
            &[],
        );
        assert_eq!(
            buffer.rows_of(Some(SESSION)).len(),
            1,
            "a read of another conversation took this one's turns"
        );
        assert!(!buffer.read_unchanged(Some("another-session"), (4, None), 1));
        // A buffer a read began takes the read's session.
        let mut read_first = LiveTurnBuffer::default();
        read_first.reconcile(
            Some("another-session"),
            DocRead {
                stamp: (4, None),
                settled: 1,
            },
            &[],
        );
        assert!(read_first.read_unchanged(Some("another-session"), (4, None), 1));
    }

    /// A buffer remembers its last few turns: one that nothing read for
    /// longer is let go, and a late hook of a turn it remembers is still
    /// known late.
    #[test]
    fn a_buffer_remembers_its_last_turns() {
        let mut buffer = LiveTurnBuffer::default();
        for at in 0..=TURNS_KEPT {
            let id = format!("p-{at}");
            hook(
                &mut buffer,
                "UserPromptSubmit",
                &prompt(&format!("turn {at}"), &id),
                None,
            );
            hook(&mut buffer, "Stop", &stop(&id, "done"), None);
        }
        assert_eq!(buffer.turns.len(), TURNS_KEPT);
        assert_eq!(buffer.rows()[0].text, "turn 1");
        assert!(
            hook(
                &mut buffer,
                "PostToolUse",
                &result("c-9", "p-2", true),
                None
            )
            .late
        );
    }
}
