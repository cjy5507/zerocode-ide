//! What the screen calls a session.
//!
//! A session id is `session-1785722030389-0`. It is correct, it is what
//! `zo attach` takes, and it is useless to a person scanning five lanes for the
//! one that needs them — every id looks like every other id, and the digits that
//! differ are a timestamp nobody reads.
//!
//! So the screen shows **what they asked** instead, and keeps the id for the
//! places that address a session rather than name it. The derivation is
//! deliberately the same rule as a task title ([`crate::task::WorktreeTask`]):
//! first non-empty line, whitespace collapsed, capped by characters. A session
//! and a dispatched task are the same thing to a reader, so they must not be
//! summarized two different ways.

use serde::{Deserialize, Serialize};

use crate::task::WorktreeTask;

/// A session's addressable id and its human name, together.
///
/// Kept as one value rather than two loose strings because the pair has an
/// invariant: the name is always safe to show and the id is always safe to
/// address with, and code that carries only one of them inevitably shows the
/// wrong one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLabel {
    /// What `zo attach <id>` takes.
    pub id: String,
    /// What the rail, the switcher and the window title show.
    ///
    /// Falls back to the id, because a session that has not been asked anything
    /// yet still has to appear in a list. A blank name would be worse than an
    /// ugly one.
    pub name: String,
}

impl SessionLabel {
    /// Name a session from the first thing asked in it.
    ///
    /// `None` — or a question that is blank or whitespace — leaves the name as
    /// the id. That is the honest state for a session nobody has spoken to yet;
    /// inventing "Untitled" would put the same word on every new lane.
    pub fn new(id: impl Into<String>, first_question: Option<&str>) -> Self {
        let id = id.into();
        let name = first_question
            .map(|question| WorktreeTask::from_spec(question, None, None).task_title)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.clone());
        Self { id, name }
    }

    /// True when a real question backed the name, rather than the id standing in
    /// for one. The view uses this to decide whether to show the id as well.
    #[must_use]
    pub fn is_named(&self) -> bool {
        self.name != self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TASK_TITLE_MAX_CHARS;

    #[test]
    fn the_first_question_becomes_the_name() {
        let label = SessionLabel::new(
            "session-1785722030389-0",
            Some("드레인 게이트 봉인 로직을 리팩터링해줘"),
        );
        assert_eq!(label.name, "드레인 게이트 봉인 로직을 리팩터링해줘");
        assert_eq!(label.id, "session-1785722030389-0");
        assert!(label.is_named());
    }

    /// The whole point: what a person reads must not be the id.
    #[test]
    fn a_named_session_never_shows_its_id_as_the_name() {
        let label = SessionLabel::new("session-1785722030389-0", Some("fix the flaky drain test"));
        assert_eq!(label.name, "fix the flaky drain test");
        assert!(!label.name.contains("session-"));
    }

    #[test]
    fn only_the_first_line_of_a_pasted_question_is_used() {
        let label = SessionLabel::new(
            "session-1",
            Some("\n\n  Refactor the drain gate  \n\nand then run the tests\nand report back"),
        );
        assert_eq!(label.name, "Refactor the drain gate");
    }

    #[test]
    fn a_session_nobody_has_asked_anything_keeps_its_id() {
        let unasked = SessionLabel::new("session-1", None);
        assert_eq!(unasked.name, "session-1");
        assert!(!unasked.is_named());

        // Whitespace is not a question either.
        let blank = SessionLabel::new("session-1", Some("   \n\t "));
        assert_eq!(blank.name, "session-1");
        assert!(!blank.is_named());
    }

    #[test]
    fn a_long_question_is_capped_the_same_way_a_task_title_is() {
        let question = "레인 레일을 구현해줘 ".repeat(40);
        let label = SessionLabel::new("session-1", Some(&question));

        // At most the cap, and sometimes one under it: a cut that lands on a
        // space has the space trimmed before the ellipsis goes on.
        assert!(label.name.chars().count() <= TASK_TITLE_MAX_CHARS);
        assert!(label.name.chars().count() >= TASK_TITLE_MAX_CHARS - 1);
        assert!(label.name.ends_with('…'), "{}", label.name);
        assert!(!label.name.contains(" …"), "{}", label.name);
        // Far more bytes than characters, so the cut was by character.
        assert!(label.name.len() > TASK_TITLE_MAX_CHARS);
    }

    /// A first question is untrusted text that reaches a terminal, so the name
    /// must arrive disarmed. This delegates to the one place titles are derived
    /// rather than sanitizing again here — two sanitizers is how one of them
    /// gets forgotten.
    #[test]
    fn a_question_carrying_escape_sequences_yields_a_name_safe_to_print() {
        let label = SessionLabel::new(
            "session-1",
            Some("\x1b]0;pwned\x07\x1b[2J드레인 게이트 리팩터링\x1b[31m\r\u{9b}"),
        );
        assert!(
            !label.name.chars().any(char::is_control),
            "a control character reached the screen: {:?}",
            label.name
        );
        assert!(
            label.name.contains("드레인 게이트 리팩터링"),
            "{:?}",
            label.name
        );
        assert!(label.is_named());
    }

    /// A question that is only escape sequences names nothing, so the id has to
    /// stand in — not a line of leftover punctuation.
    #[test]
    fn a_question_of_only_control_characters_falls_back_to_the_id() {
        let label = SessionLabel::new("session-1", Some("\x1b\x07\x08\x7f\u{9b}"));
        assert_eq!(label.name, "session-1");
        assert!(!label.is_named());
    }

    /// The same rule for *complete* sequences, whose parameters are printable.
    /// Removing only the escape byte would name this lane `[2J [31m`.
    #[test]
    fn a_question_of_only_escape_sequences_falls_back_to_the_id() {
        let label = SessionLabel::new("session-1", Some("\x1b[2J\x1b[31m"));
        assert_eq!(label.name, "session-1");
        assert!(!label.is_named());
    }

    /// An `OSC` payload is a string its sender chose. It must not become the
    /// name of the lane a person is looking at.
    #[test]
    fn an_osc_payload_cannot_smuggle_itself_into_the_name() {
        let label = SessionLabel::new("session-1", Some("\x1b]0;a string the sender chose\x07"));
        assert_eq!(label.name, "session-1");
        assert!(!label.is_named());
    }

    /// The charset designators are three characters, not two, so their final
    /// byte used to survive and name the lane `B`.
    #[test]
    fn a_question_of_only_charset_escapes_falls_back_to_the_id() {
        let label = SessionLabel::new("session-1", Some("\x1b(B\x1b#8"));
        assert_eq!(label.name, "session-1");
        assert!(!label.is_named());
    }

    #[test]
    fn a_label_round_trips_as_snake_case_json() {
        let label = SessionLabel::new("session-1", Some("fix the tests"));
        let json = serde_json::to_string(&label).expect("serialize");
        assert!(json.contains("\"id\""), "{json}");
        assert!(json.contains("\"name\""), "{json}");
        assert_eq!(
            serde_json::from_str::<SessionLabel>(&json).expect("deserialize"),
            label
        );
    }
}
