//! What a turn turned out to be — the routing seat's label, written by code
//! when the turn ends (t-6346).
//!
//! The first label asked whether the route stood — no quota wall, refusal or
//! person moved the model before the turn ended — and it said yes ten times
//! in ten: a label that cannot say no is not evidence (t-6324 §3, C10). This
//! one reads what the turn DID, off its own messages — how many tools it
//! called, how many files its edits wrote, whether it started agents — as
//! the level of the routing Score the judgment answered
//! (`zerocode_core::jev::questions::ROUTING_COMPLEXITY_LEVELS`), and marks
//! the judgment by whether the router, reading its level, would have picked
//! the tier that work needed. The same facts grade the keyword tables beside
//! it, so the seat is held against its baseline on one set of marks.

use runtime::{ContentBlock, ConversationMessage, RouteTaskComplexity};
use serde::{Deserialize, Serialize};

/// What one turn did, counted off its own messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnWork {
    /// Every tool call the turn's assistant messages made.
    pub tool_calls: usize,
    /// The distinct files its edits wrote (`runtime::edited_file_paths`).
    pub files_edited: usize,
    /// The agents it started (`runtime::is_fan_out_tool`).
    pub spawns: usize,
}

/// The turn's work: its calls, the files its edits wrote, the agents it
/// started. The same reading live, at the turn's end, and in a replay of the
/// transcript.
#[must_use]
pub fn turn_work(turn: &[ConversationMessage]) -> TurnWork {
    let (tool_calls, spawns) = turn
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .fold((0, 0), |(calls, spawns), name| (calls + 1, spawns + usize::from(runtime::is_fan_out_tool(name))));
    TurnWork { tool_calls, files_edited: runtime::edited_file_paths(turn).len(), spawns }
}

/// A count a rung is never reached by.
const NEVER: usize = usize::MAX;

/// One rung of [`WORK_RUNGS`]: a level, and the counts from which a turn's
/// work is at least that level — any one of them reached is enough.
struct Rung {
    level: RouteTaskComplexity,
    tool_calls: usize,
    files_edited: usize,
    spawns: usize,
}

/// The routing Score's four situations, read as counts — one table, the one
/// place the label's numbers are.
///
/// Below every rung is `Trivial`: "a question answered from what is already
/// known" ran nothing. `Small` — "a short look at one file or command", "a
/// small change in one place" — is one call or one file. `Medium` — "several
/// files in one part", "an investigation that needs running and reading
/// several things" — is three calls or two files. `Large` — "several parts
/// of the product" — is a started agent, five files, or thirty calls.
///
/// The words decide the first two rungs; the third is this machine's: of the
/// 711 person turns its transcripts held for the week to 2026-09-24, 201 ran
/// nothing, 86 (12%) made thirty calls or more, 18 of the 139 that edited
/// wrote five files or more, and 22 started an agent.
const WORK_RUNGS: [Rung; 3] = [
    Rung { level: RouteTaskComplexity::Small, tool_calls: 1, files_edited: 1, spawns: NEVER },
    Rung { level: RouteTaskComplexity::Medium, tool_calls: 3, files_edited: 2, spawns: NEVER },
    Rung { level: RouteTaskComplexity::Large, tool_calls: 30, files_edited: 5, spawns: 1 },
];

/// The level a turn's work reads as by [`WORK_RUNGS`]: the highest rung any
/// of its counts reaches.
#[must_use]
pub fn observed_level(work: &TurnWork) -> RouteTaskComplexity {
    WORK_RUNGS
        .iter()
        .rev()
        .find(|rung| {
            work.tool_calls >= rung.tool_calls || work.files_edited >= rung.files_edited || work.spawns >= rung.spawns
        })
        .map_or(RouteTaskComplexity::Trivial, |rung| rung.level)
}

/// Whether the router, reading `said`, would have picked the tier the work
/// `was` needed (`runtime::default_difficulty_tier`); `None` where either
/// side has no tier (`Unknown`).
///
/// The tier, not the band: "within one band of the work" agreed with a
/// reader that always said small on 401 of the 488 person turns this
/// machine's transcripts held for the thirty days to 2026-09-24 — the
/// keyword tables on 392 — so it could hardly say no. By the tier the same
/// reader agrees on 204 and the tables on 193: a constant answer is held to
/// the share of turns that were its tier's work.
#[must_use]
pub fn same_tier(said: RouteTaskComplexity, was: RouteTaskComplexity) -> Option<bool> {
    Some(runtime::default_difficulty_tier(said)? == runtime::default_difficulty_tier(was)?)
}

#[cfg(test)]
mod tests {
    use runtime::{ContentBlock, ConversationMessage, RouteTaskComplexity as C};

    use super::{observed_level, same_tier, turn_work, TurnWork};

    fn call(name: &str) -> ContentBlock {
        ContentBlock::ToolUse { id: format!("toolu-{name}"), name: name.to_string(), input: "{}".to_string() }
    }

    fn edited(path: &str) -> ConversationMessage {
        let mut result = ConversationMessage::user_text("");
        result.role = runtime::MessageRole::Tool;
        result.blocks = vec![ContentBlock::ToolResult {
            tool_use_id: "toolu-edit".to_string(),
            tool_name: "edit_file".to_string(),
            output: format!("{{\n  \"type\": \"update\",\n  \"filePath\": \"{path}\"\n}}"),
            is_error: false,
            images: Vec::new(),
        }];
        result
    }

    /// The counts a turn's own messages carry: every call it made, the files
    /// its edits wrote (each once), and the agents it started.
    #[test]
    fn a_turns_work_is_counted_off_its_own_messages() {
        let turn = vec![
            ConversationMessage::user_text("fix the flaky test"),
            ConversationMessage::assistant(vec![call("read_file"), call("bash")]),
            ConversationMessage::assistant(vec![call("edit_file"), call("edit_file"), call("Agent")]),
            edited("/work/app/src/a.rs"),
            edited("/work/app/src/a.rs"),
            edited("/work/app/src/b.rs"),
        ];
        assert_eq!(turn_work(&turn), TurnWork { tool_calls: 5, files_edited: 2, spawns: 1 });
        assert_eq!(turn_work(&[ConversationMessage::user_text("what does this flag do?")]), TurnWork::default());
    }

    /// The four levels of the routing Score, read off the counts by one
    /// table: nothing run is what was already known, a call or two or one
    /// file is a short look or one place, several calls or files is one
    /// part's work, and a long run, many files or a started agent is work
    /// across parts.
    #[test]
    fn the_work_is_read_by_one_table() {
        let at = |tool_calls, files_edited, spawns| observed_level(&TurnWork { tool_calls, files_edited, spawns });
        assert_eq!(at(0, 0, 0), C::Trivial);
        assert_eq!(at(1, 0, 0), C::Small);
        assert_eq!(at(2, 1, 0), C::Small);
        assert_eq!(at(3, 0, 0), C::Medium);
        assert_eq!(at(2, 2, 0), C::Medium);
        assert_eq!(at(29, 4, 0), C::Medium);
        assert_eq!(at(30, 0, 0), C::Large);
        assert_eq!(at(4, 5, 0), C::Large);
        assert_eq!(at(1, 0, 1), C::Large, "a started agent is work across parts");
    }

    /// A mark agrees when the router, reading it, would have picked the tier
    /// the work turned out to need: trivial and small are both the fast
    /// tier's work, so a trivial mark on a small turn agrees, and a medium
    /// mark on a turn that started an agent does not. A side that could not
    /// say has no mark.
    #[test]
    fn a_turn_label_agrees_on_the_tier_the_router_would_pick() {
        assert_eq!(same_tier(C::Trivial, C::Small), Some(true), "both are the fast tier's work");
        assert_eq!(same_tier(C::Large, C::Large), Some(true));
        assert_eq!(same_tier(C::Medium, C::Large), Some(false), "the ordinary tier for work across parts");
        assert_eq!(same_tier(C::Small, C::Medium), Some(false), "a quick answer that became an investigation");
        assert_eq!(same_tier(C::Large, C::Trivial), Some(false), "a brief that turned out to need nothing");
        assert_eq!(same_tier(C::Unknown, C::Small), None, "the tables could not say");
    }
}
