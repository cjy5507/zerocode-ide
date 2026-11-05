//! The window hears zo's tool calls under the names every agent's hooks use
//! (t-96186): `Read`, not the handler's `read_file`, whichever spelling the
//! pane's render row carries. The window reads both, so an older zo and a
//! newer window — and the reverse — still agree.

use super::channel::state::ActivityCard;
use super::reporter::{HookReporter, ToolCallFacts};

#[test]
fn a_tool_start_and_its_end_name_the_tool_the_general_way() {
    let (reporter, captured) = HookReporter::capturing();
    let card = ActivityCard {
        verb: "read".to_string(),
        target: Some("src/a.rs".to_string()),
        phase: "started".to_string(),
        elapsed_secs: 0,
    };
    let read = ToolCallFacts {
        call_id: "call-1",
        name: "read_file",
        file: Some("/repo/src/a.rs"),
    };
    reporter.pre_tool_use(read, "src/a.rs", None, &card, "s-1");
    reporter.post_tool_use(read, false, "s-1");
    reporter.permission_request("edit_file", "writes src/a.rs", "s-1");

    let posted = captured.lock().expect("capture");
    let names: Vec<&str> = posted
        .iter()
        .map(|envelope| envelope.payload["tool_name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, ["Read", "Read", "Edit"]);
}
