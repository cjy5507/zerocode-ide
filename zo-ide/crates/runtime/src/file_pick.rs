//! Turn-start file suggestions. Candidate discovery and wire execution live
//! with the host's Jev seat; the bounded request shape, its one batched
//! question set and the code-task filter shared by those layers live in the
//! core since t-11349 (`zerocode_core::jev::file_pick`), where the window asks
//! the same seat of its panes' agents. This module keeps zo's own seat
//! contract and its typed questions.

use std::collections::BTreeMap;

use api::SystemOneQuestion;
use futures_util::future::BoxFuture;
pub use zerocode_core::jev::file_pick::{
    candidate_id, hint, is_code_edit_intent, rank_candidates, read_answers, rubric_words,
    select_candidates, state, FilePickAsk, FilePickCandidate, FilePickHint, FilePickReadings,
    FILE_PICK_ANY_QUESTION, FILE_PICK_NOTE_PREFIX,
};
#[cfg(test)]
use zerocode_core::jev::file_pick::{candidate_instructions, FILE_PICK_FILE_KEYS, FILE_PICK_STATE_KEYS};
#[cfg(test)]
use zerocode_core::jev::FILE_PICK_REQUEST_CHAR_CAP;

const FILE_PICK_SEARCH_TOOL_WORDS: &[&str] = &["read", "grep", "glob", "search"];

/// The host seat runs a request and later receives the turn's actual edits.
/// Recording seats return without waiting; an acting seat may return a hint.
pub trait FilePickSeat: Send + Sync {
    fn suggest(&self, ask: FilePickAsk) -> BoxFuture<'_, Option<FilePickHint>>;

    /// Label the start-of-turn judgment with files the same turn actually edited.
    fn label(&self, attempt: &str, edited_paths: &[String], search_calls_before_first_edit: Option<usize>);
}

/// One Noul for the no-match case and one for each candidate in the state,
/// as zo's client asks them ([`zerocode_core::jev::file_pick::questions`]).
#[must_use]
pub fn questions(candidates: &[FilePickCandidate]) -> BTreeMap<String, SystemOneQuestion> {
    zerocode_core::jev::file_pick::questions(candidates, SystemOneQuestion::noul)
}

/// Count first-class read/search tool results before the turn's first edit.
/// A turn with no edit has no search-before-edit comparison.
#[must_use]
pub fn search_calls_before_first_edit(
    messages: &[crate::session::ConversationMessage],
) -> Option<usize> {
    let mut calls = 0usize;
    for message in messages {
        if !crate::edited_file_paths(std::slice::from_ref(message)).is_empty() {
            return Some(calls);
        }
        for block in &message.blocks {
            if let crate::session::ContentBlock::ToolResult { tool_name, .. } = block {
                let name = tool_name.to_ascii_lowercase();
                if FILE_PICK_SEARCH_TOOL_WORDS
                    .iter()
                    .any(|word| name.contains(word))
                {
                    calls = calls.saturating_add(1);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        FilePickCandidate, FILE_PICK_REQUEST_CHAR_CAP, is_code_edit_intent, questions,
        rank_candidates, select_candidates, state,
    };

    fn candidate(path: &str) -> FilePickCandidate {
        FilePickCandidate {
            path: path.to_string(),
            about: "A source file.".to_string(),
        }
    }

    /// The file pick seat's words are pinned to its version (t-9469): both
    /// questions, what yes and no mean, and the keys the state carries. A word
    /// changed without a version is red here, and a candidate's question names
    /// every key the state carries.
    #[test]
    fn the_version_is_pinned_to_the_words() {
        use super::{candidate_instructions, rubric_words, FILE_PICK_FILE_KEYS, FILE_PICK_STATE_KEYS};
        assert_eq!(zerocode_core::jev::questions::FILE_PICK_RUBRIC_VERSION, 1);
        assert_eq!(zerocode_core::jev::rubric_fingerprint(rubric_words), "5132dc056a853c32");
        let asked = candidate_instructions("F01");
        for key in FILE_PICK_STATE_KEYS {
            assert!(asked.contains(&format!("`{key}`")), "{key}: {asked}");
        }
        let sent = state("fix the cut", &[candidate("src/lib.rs")]);
        let mut keys: Vec<&str> = sent.as_object().expect("an object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["files", "request"]);
        let mut file: Vec<&str> = sent["files"][0].as_object().expect("a file").keys().map(String::as_str).collect();
        file.sort_unstable();
        let mut expected = FILE_PICK_FILE_KEYS.to_vec();
        expected.sort_unstable();
        assert_eq!(file, expected);
    }

    #[test]
    fn one_batch_has_one_noul_per_file_and_a_no_match_question() {
        let files = vec![candidate("src/alpha.rs"), candidate("src/beta.rs")];
        let questions = questions(&files);

        assert_eq!(questions.len(), 3);
        assert!(questions.contains_key("any"));
        assert!(questions.contains_key("F01"));
        assert!(questions.contains_key("F02"));
        assert!(questions["any"].instructions.contains("any file"));
        assert!(questions["F01"].instructions.contains("F01"));
    }

    #[test]
    fn state_keeps_the_request_head_paths_and_short_about_lines() {
        let request = "x".repeat(3_000);
        let files: Vec<FilePickCandidate> = (0..35)
            .map(|i| candidate(&format!("src/file_{i}.rs")))
            .collect();
        let state = state(&request, &files);

        assert_eq!(state["request"].as_str().unwrap().chars().count(), FILE_PICK_REQUEST_CHAR_CAP);
        assert_eq!(state["files"].as_array().unwrap().len(), 30);
        assert_eq!(state["files"][0]["id"], "F01");
        assert_eq!(state["files"][29]["id"], "F30");
        assert!(state["files"][0].get("body").is_none());
    }

    #[test]
    fn only_confident_candidate_files_can_be_suggested() {
        let files = vec![
            candidate("src/a.rs"),
            candidate("src/b.rs"),
            candidate("src/c.rs"),
            candidate("src/d.rs"),
        ];
        let answers = json!({
            "any": { "type": "noul", "noul": 0.95 },
            "F01": { "type": "noul", "noul": 0.82 },
            "F02": { "type": "noul", "noul": 0.92 },
            "F03": { "type": "noul", "noul": 0.75 },
            "F04": { "type": "noul", "noul": 0.68 },
        });

        assert_eq!(select_candidates(&files, &answers), ["src/b.rs", "src/a.rs", "src/c.rs"]);
        let no_match = json!({
            "any": { "type": "noul", "noul": 0.25 },
            "F01": { "type": "noul", "noul": 0.88 },
            "F02": { "type": "noul", "noul": 0.91 },
            "F03": { "type": "noul", "noul": 0.86 },
            "F04": { "type": "noul", "noul": 0.79 },
        });
        assert_eq!(rank_candidates(&files, &no_match), ["src/b.rs", "src/a.rs", "src/c.rs"]);
        assert!(select_candidates(&files, &no_match).is_empty());
    }

    #[test]
    fn task_filter_keeps_implementation_and_debugging_requests() {
        assert!(is_code_edit_intent("Please fix the retry path"));
        assert!(is_code_edit_intent("이 함수 동작을 고쳐 주세요"));
        assert!(!is_code_edit_intent("What does the exchange setting mean?"));
        assert!(!is_code_edit_intent("What does this setting mean?"));
    }
}
