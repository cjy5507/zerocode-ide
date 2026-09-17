//! `Agent{subagent_type: "fork"}` — a child that inherits its PARENT's
//! conversation instead of a fresh brief (t-2875; Claude Code's fork
//! contract: the parent's model, whatever the call named; the parent's
//! permission mode; the parent's cwd).
//!
//! A fork is not a second transcript loader. The parent's live transcript —
//! published on the tool context by the session host as a
//! [`ForkSource`](crate::ForkSource) — is copied through [`Session::fork`]
//! (which records the lineage) into the child's own
//! `<store>/<id>.session.jsonl`; the tool calls of the parent's last message
//! that have no result yet are answered, the fork's own `Agent` call with the
//! brief that says what it is; and the job is marked `resume`, so both
//! executors rehydrate it by the road a resumed child already takes — the
//! inline runtime through `Session::load_from_secure_path`, a pane child
//! through `Brief.resume_transcript` (t-2513 §2.4).

use std::collections::BTreeSet;
use std::path::Path;

use core_types::session::{ContentBlock, ConversationMessage, Session};

use super::subagent_profile::{fork_brief, FORK_SIBLING_RESULT};

/// The `subagent_type` word. Not a harness of its own: a fork runs the
/// parent's system prompt and the general-purpose allow-list, on the
/// parent's model, and no `.zo/agents/fork.md` can shadow the word.
pub(crate) const FORK_SUBAGENT_TYPE: &str = "fork";

/// Why a `fork` is refused where nobody published a conversation to inherit.
pub(crate) const NO_PARENT_TO_FORK: &str = "subagent_type \"fork\" inherits the calling \
conversation, and this call has none to inherit: only an `Agent` call from a session's own \
turn can fork (not a SpawnMultiAgent member, not a workflow phase). Delegate with another \
subagent_type instead.";

/// Whether an explicit `subagent_type` asks for a fork.
pub(crate) fn is_fork_type(subagent_type: Option<&str>) -> bool {
    subagent_type
        .map(str::trim)
        .is_some_and(|word| word.eq_ignore_ascii_case(FORK_SUBAGENT_TYPE))
}

/// What the written transcript carries for the fork's first turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ForkedTranscript {
    /// The fork's own `Agent` call was among the parent's pending calls and
    /// was answered with the brief, so the prompt can stand alone. When the
    /// call's id was not known (an internal spawner), the brief rides the
    /// prompt instead ([`fork_prompt`]).
    pub brief_in_transcript: bool,
}

/// Copy the parent's transcript at `parent` into the fork's own file at
/// `child`, answering the parent's pending tool calls on the way.
///
/// Read through `Session::load_from_path` like a resume (no lock is taken by
/// a bind; the parent keeps its writer lease), forked so the header names the
/// parent session and `agent_id` as the branch, and written as one snapshot
/// — the same file shape a resumed child reads. A parent transcript that
/// cannot be read fails the fork rather than starting the child fresh under
/// a name that promises inheritance.
pub(super) fn write_fork_transcript(
    parent: &Path,
    child: &Path,
    agent_id: &str,
    own_call: Option<&str>,
) -> Result<ForkedTranscript, String> {
    let parent_session = Session::load_from_path(parent).map_err(|error| {
        format!(
            "cannot read the parent transcript `{}` to fork it: {error}",
            parent.display()
        )
    })?;
    let mut forked = parent_session.fork(Some(agent_id.to_string()));
    let mut brief_in_transcript = false;
    for (id, name) in pending_tool_uses(&forked.messages) {
        let own = own_call.is_some_and(|call| call == id);
        brief_in_transcript |= own;
        let text = if own {
            fork_brief()
        } else {
            FORK_SIBLING_RESULT.to_string()
        };
        forked
            .push_message(ConversationMessage::tool_result(id, name, text, false))
            .map_err(|error| format!("cannot answer the parent's pending call for the fork: {error}"))?;
    }
    forked.save_to_path(child).map_err(|error| {
        format!(
            "cannot write the fork's transcript `{}`: {error}",
            child.display()
        )
    })?;
    Ok(ForkedTranscript { brief_in_transcript })
}

/// The prompt the fork's first turn runs: the parent's delegated task, with
/// the brief ahead of it when no tool result in the transcript carried it.
pub(super) fn fork_prompt(prompt: &str, forked: ForkedTranscript) -> String {
    if forked.brief_in_transcript {
        prompt.to_string()
    } else {
        format!("{}\n\n{prompt}", fork_brief())
    }
}

/// The `tool_use` blocks no `tool_result` answers, in order: the parent's
/// batch still running when the fork was taken, the fork's own `Agent` call
/// among them. A transcript with every call answered yields nothing.
fn pending_tool_uses(messages: &[ConversationMessage]) -> Vec<(String, String)> {
    let answered: BTreeSet<&str> = messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    messages
        .iter()
        .flat_map(|message| message.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, .. } if !answered.contains(id.as_str()) => {
                Some((id.clone(), name.clone()))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent_at(path: &Path) -> Session {
        let mut session = Session::new().with_persistence_path(path.to_path_buf());
        session
            .push_message(ConversationMessage::user_text("Read fixture.txt, then fork"))
            .expect("user turn");
        session
            .push_message(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "toolu_read".to_string(),
                name: "read_file".to_string(),
                input: r#"{"path":"fixture.txt"}"#.to_string(),
            }]))
            .expect("read call");
        session
            .push_message(ConversationMessage::tool_result(
                "toolu_read",
                "read_file",
                "MARKER_LINE",
                false,
            ))
            .expect("read result");
        session
            .push_message(ConversationMessage::assistant(vec![
                ContentBlock::ToolUse {
                    id: "toolu_fork".to_string(),
                    name: "Agent".to_string(),
                    input: r#"{"subagent_type":"fork","prompt":"which marker?"}"#.to_string(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_sibling".to_string(),
                    name: "bash".to_string(),
                    input: r#"{"command":"sleep 30"}"#.to_string(),
                },
            ]))
            .expect("fork batch");
        session
    }

    #[test]
    fn the_word_is_read_loosely_and_nothing_else_is() {
        assert!(is_fork_type(Some("fork")));
        assert!(is_fork_type(Some(" Fork ")));
        assert!(!is_fork_type(Some("forked")));
        assert!(!is_fork_type(Some("general-purpose")));
        assert!(!is_fork_type(None));
    }

    /// The copy carries every parent message, names the parent as its
    /// lineage, answers the fork's own call with the brief and the sibling
    /// with the sibling text, and leaves the already-answered read alone.
    #[test]
    fn the_fork_copy_is_the_parent_plus_answers_for_its_pending_calls() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent_path = dir.path().join("parent.jsonl");
        let parent = parent_at(&parent_path);
        let child_path = dir.path().join("agent-1.session.jsonl");

        let forked = write_fork_transcript(&parent_path, &child_path, "agent-1", Some("toolu_fork"))
            .expect("fork the transcript");
        assert!(forked.brief_in_transcript);

        let child = Session::load_from_path(&child_path).expect("the fork copy loads");
        assert_ne!(child.session_id, parent.session_id, "a fork is its own session");
        assert_eq!(
            child.fork.as_ref().map(|fork| fork.parent_session_id.as_str()),
            Some(parent.session_id.as_str())
        );
        assert_eq!(
            child.fork.as_ref().and_then(|fork| fork.branch_name.as_deref()),
            Some("agent-1")
        );
        assert_eq!(&child.messages[..4], &parent.messages[..]);
        let answers: Vec<(&str, &str)> = child.messages[4..]
            .iter()
            .flat_map(|message| message.blocks.iter())
            .filter_map(|block| match block {
                ContentBlock::ToolResult { tool_use_id, output, .. } => {
                    Some((tool_use_id.as_str(), output.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(answers.len(), 2, "one answer per pending call: {answers:?}");
        assert_eq!(answers[0].0, "toolu_fork");
        assert!(answers[0].1.starts_with("[fork] You are a fork"), "{}", answers[0].1);
        assert_eq!(answers[1], ("toolu_sibling", FORK_SIBLING_RESULT));
        assert!(pending_tool_uses(&child.messages).is_empty());
        // The parent's own file is untouched by the copy.
        assert_eq!(
            Session::load_from_path(&parent_path).expect("parent reloads").messages,
            parent.messages
        );
    }

    /// Without the fork's own call id the brief cannot ride a tool result,
    /// so it rides the prompt; with it, the prompt stands alone.
    #[test]
    fn the_brief_rides_the_prompt_only_when_no_result_carried_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent_path = dir.path().join("parent.jsonl");
        let _parent = parent_at(&parent_path);
        let child_path = dir.path().join("agent-2.session.jsonl");
        let forked = write_fork_transcript(&parent_path, &child_path, "agent-2", None)
            .expect("fork without an own call id");
        assert!(!forked.brief_in_transcript);
        let prompt = fork_prompt("which marker?", forked);
        assert!(prompt.starts_with("[fork] You are a fork"));
        assert!(prompt.ends_with("which marker?"));
        assert_eq!(
            fork_prompt("which marker?", ForkedTranscript { brief_in_transcript: true }),
            "which marker?"
        );
    }

    #[test]
    fn an_unreadable_parent_fails_the_fork_instead_of_starting_fresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = write_fork_transcript(
            &dir.path().join("missing.jsonl"),
            &dir.path().join("agent-3.session.jsonl"),
            "agent-3",
            None,
        )
        .expect_err("a missing parent transcript is refused");
        assert!(error.contains("cannot read the parent transcript"), "{error}");
        assert!(!dir.path().join("agent-3.session.jsonl").exists());
    }
}
