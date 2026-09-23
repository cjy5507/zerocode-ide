//! Live todo re-injection for long-horizon goal loops.
//!
//! The `TodoWrite` tool persists the session's task list to a JSON store
//! (`tools::task_tools`), but that state only ever reached the model as the
//! original tool-result message. Once automatic compaction summarized older
//! messages away -- exactly what happens on a long, many-step task -- the live
//! todo list silently dropped out of the model's context, so the agent "lost
//! the plot" on resume. This module re-reads the store at compaction time and
//! renders it into a system reminder, so the current task state survives the
//! one event that would otherwise erase it.
//!
//! Both halves are best-effort and total: any IO or parse failure yields an
//! empty list / `None`, never an error, so re-injection can never fail a turn.

use std::path::Path;

use serde::Deserialize;

/// The tool whose result IS the plan, whichever name the model called it by.
pub(crate) const PLAN_WRITING_TOOLS: [&str; 2] = ["TodoWrite", "todo_write"];

/// One persisted todo, mirroring the canonical `tools::task_tools::TodoItem`
/// shape (`{content, activeForm, status}`). Duplicated here -- rather than
/// shared via a new crate dependency -- because `runtime` must not depend on
/// `tools` (the dependency runs the other way). The field contract is pinned
/// by [`TodoStatus`]'s `serde(rename_all = "snake_case")` and the `activeForm`
/// rename, so a drift in the writer's JSON surfaces as a parse miss (skipped
/// entry), never a silent mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TodoSnapshot {
    pub content: String,
    #[serde(rename = "activeForm")]
    pub active_form: String,
    pub status: TodoStatus,
}

/// Lifecycle state of a persisted todo. Same three states and wire tokens as
/// the canonical tool enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    /// The wire token a hook payload carries — the same three words the tool
    /// serializes, so a hook script matches on what it already reads.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    /// A compact checkbox marker for the reminder: `[ ]` pending, `[~]`
    /// in-progress, `[x]` completed.
    const fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Completed => "[x]",
        }
    }

    /// Incomplete work sorts before completed so the reminder leads with what
    /// still needs doing; in-progress leads pending.
    const fn order(self) -> u8 {
        match self {
            Self::InProgress => 0,
            Self::Pending => 1,
            Self::Completed => 2,
        }
    }

    const fn is_complete(self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// Resolve the todo store path for `cwd`, honoring the same overrides the
/// writer uses: an explicit (non-empty) `ZO_TODO_STORE`, else
/// `zo_state_base(cwd)/.zo-todos.json` (which itself honors
/// `ZO_STATE_DIR`). Centralizing this keeps reader and writer in lockstep.
/// Read the current todo list from the store, best-effort. A missing store,
/// unreadable file, or malformed JSON all yield an empty list -- re-injection
/// degrades to a no-op rather than ever failing the enclosing turn.
#[must_use]
pub fn current_todos(cwd: &Path) -> Vec<TodoSnapshot> {
    let path = crate::todo_store::resolve_readable_store(cwd);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<TodoSnapshot>>(&raw).unwrap_or_default()
}

/// The plan a successful result of a plan-writing tool (`TodoWrite`) carries —
/// the list the model just wrote (`newTodos`) — or `None` for a result of
/// another shape. Reads the result's envelope (`compact::result_envelope`),
/// so text a hook appended to the result never hides the plan.
#[must_use]
pub fn todos_written(output: &str) -> Option<Vec<TodoSnapshot>> {
    let mut envelope = crate::compact::result_envelope(output)?;
    serde_json::from_value(envelope.get_mut("newTodos")?.take()).ok()
}

/// Render the live todo list as a system reminder for re-injection after
/// compaction, or `None` when there is nothing worth re-asserting.
///
/// Returns `None` when the list is empty *or* every item is already completed
/// -- in both cases an empty `# Current todos` block would only burn tokens.
/// Otherwise emits incomplete-first, with a checkbox marker and the active
/// form for the in-progress item so the model sees the current task state
/// without being prompted to emit visible continuation filler.
#[must_use]
pub fn render_todos_reminder(todos: &[TodoSnapshot]) -> Option<String> {
    render_todo_lines(todos).map(|lines| {
        format!(
            "[system: Current task list (live state preserved across compaction). In-progress marks the active item; completed items are historical state.]\n# Current todos\n{lines}"
        )
    })
}

/// The list [`render_todos_reminder`] re-asserts, one item a line, without
/// the reminder's header — for a reader that carries the plan somewhere other
/// than the model's own context (the patch review's task, t-6232). `None`
/// when the list is empty or every item is already completed.
#[must_use]
pub fn render_todo_lines(todos: &[TodoSnapshot]) -> Option<String> {
    if todos.is_empty() || todos.iter().all(|todo| todo.status.is_complete()) {
        return None;
    }

    let mut ordered: Vec<&TodoSnapshot> = todos.iter().collect();
    // Stable sort preserves the author's order within a status bucket.
    ordered.sort_by_key(|todo| todo.status.order());

    let mut out = String::new();
    for todo in ordered {
        let label = if todo.status == TodoStatus::InProgress {
            &todo.active_form
        } else {
            &todo.content
        };
        out.push_str(todo.status.marker());
        out.push(' ');
        out.push_str(label);
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        render_todo_lines, render_todos_reminder, todos_written, TodoSnapshot, TodoStatus,
    };

    fn todo(content: &str, active: &str, status: TodoStatus) -> TodoSnapshot {
        TodoSnapshot {
            content: content.to_string(),
            active_form: active.to_string(),
            status,
        }
    }

    #[test]
    fn empty_list_renders_nothing() {
        assert_eq!(render_todos_reminder(&[]), None);
    }

    #[test]
    fn all_completed_renders_nothing() {
        let todos = vec![
            todo("a", "doing a", TodoStatus::Completed),
            todo("b", "doing b", TodoStatus::Completed),
        ];
        assert_eq!(render_todos_reminder(&todos), None);
    }

    #[test]
    fn mixed_statuses_render_incomplete_first_with_markers() {
        let todos = vec![
            todo("write code", "writing code", TodoStatus::Completed),
            todo("ship it", "shipping it", TodoStatus::Pending),
            todo("run tests", "running tests", TodoStatus::InProgress),
        ];
        let rendered = render_todos_reminder(&todos).expect("mixed list must render");
        assert!(rendered.contains("# Current todos"));
        let in_progress = rendered
            .find("[~] running tests")
            .expect("in-progress present");
        let pending = rendered.find("[ ] ship it").expect("pending present");
        let completed = rendered.find("[x] write code").expect("completed present");
        // Incomplete (in-progress, then pending) must precede completed.
        assert!(in_progress < pending, "in-progress before pending");
        assert!(pending < completed, "incomplete before completed");
        // The in-progress item shows its active form, not its content.
        assert!(
            !rendered.contains("run tests"),
            "in-progress uses active form"
        );
        let header = rendered.lines().next().unwrap_or_default().to_lowercase();
        assert!(
            !header.contains("continue") && !header.contains("restart"),
            "todo reminder must describe state, not prompt visible continuation filler: {rendered}"
        );
    }

    /// The reminder is its header over the lines a reader elsewhere carries
    /// (the patch review's task, t-6232) — one rendering of the list.
    #[test]
    fn the_reminder_is_a_header_over_the_lines() {
        let todos = vec![
            todo("write code", "writing code", TodoStatus::Completed),
            todo("run tests", "running tests", TodoStatus::InProgress),
        ];
        let lines = render_todo_lines(&todos).expect("an open list");
        assert_eq!(lines, "[~] running tests\n[x] write code");
        let reminder = render_todos_reminder(&todos).expect("an open list");
        assert!(
            reminder.ends_with(&format!("\n# Current todos\n{lines}")),
            "{reminder}"
        );
        assert_eq!(
            render_todo_lines(&[todo("a", "doing a", TodoStatus::Completed)]),
            None
        );
    }

    /// A `TodoWrite` result's plan is its `newTodos`, read out of the result's
    /// envelope, so text a hook appended after it hides nothing.
    #[test]
    fn a_written_plan_is_the_results_new_todos() {
        let output = r#"{"oldTodos":[{"content":"old","activeForm":"olding","status":"pending"}],"newTodos":[{"content":"do it","activeForm":"doing it","status":"in_progress"}],"verificationNudgeNeeded":null}"#;
        let written = todos_written(output).expect("a plan");
        assert_eq!(
            written,
            vec![todo("do it", "doing it", TodoStatus::InProgress)]
        );
        assert_eq!(
            todos_written(&format!("{output}\n\nhook: noted")),
            Some(written)
        );
        assert_eq!(
            todos_written("todos must not be empty"),
            None,
            "not an envelope"
        );
        assert_eq!(todos_written(r#"{"tasks":[]}"#), None, "another shape");
    }

    #[test]
    fn parses_writer_json_shape() {
        // The exact on-disk shape the `TodoWrite` tool persists.
        let raw = r#"[{"content":"do it","activeForm":"doing it","status":"in_progress"}]"#;
        let parsed: Vec<TodoSnapshot> =
            serde_json::from_str(raw).expect("writer JSON must deserialize");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].status, TodoStatus::InProgress);
        assert_eq!(parsed[0].active_form, "doing it");
    }
}
