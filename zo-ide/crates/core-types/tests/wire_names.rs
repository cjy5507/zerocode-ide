//! A resumed session's calls keep their tool links when the request names the
//! tools the general way (t-96186): the session stores `read_file` while the
//! wire advertises `Read`, and an older session may hold `Read` itself.

use std::collections::BTreeSet;
use std::sync::Arc;

use core_types::session::reconcile_tool_history;
use core_types::{ContentBlock, ConversationMessage};

fn read_call(name: &str) -> Arc<Vec<ConversationMessage>> {
    Arc::new(vec![
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: name.to_string(),
            input: r#"{"path":"src/a.rs"}"#.to_string(),
        }]),
        ConversationMessage::tool_result("t1", name, "fn a() {}", false),
    ])
}

fn names(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

#[test]
fn a_stored_call_is_kept_linked_whichever_spelling_either_side_holds() {
    for (stored, known) in [
        // A strict provider is told the wire names; the session holds the handler's.
        ("read_file", names(&["Read", "Bash"])),
        // Anthropic is told every resolvable name; an older session holds `Read`.
        ("Read", names(&["read_file", "bash"])),
    ] {
        let messages = read_call(stored);
        let view = reconcile_tool_history(&messages, &known, &known);
        assert!(
            Arc::ptr_eq(&view, &messages),
            "{stored} against {known:?} was archived as prose: {view:?}"
        );
    }
}

/// A call the provider can no longer take is archived as prose — under the
/// name and field the model would use today, not the handler's.
#[test]
fn an_archived_call_reads_the_way_the_model_would_make_it() {
    let messages = read_call("read_file");
    let view = reconcile_tool_history(&messages, &names(&[]), &names(&["read_file"]));
    let ContentBlock::Text { text } = &view[0].blocks[0] else {
        panic!("the call was not archived: {view:?}");
    };
    assert!(text.contains(r#"the tool "Read""#), "{text}");
    assert!(text.contains("file_path"), "{text}");
}
