//! The tool limits a zo launch put on a conversation travel from zo's
//! reporter to the window's resume (t-112277) under names each side spells
//! once: the payload key and the two field names.

use super::support::{block_after, strip_rust_comments};

const REPORTER: &str = include_str!("../../../../zo-ide/crates/zo-ide/src/ide/reporter.rs");

/// zo says its launch limits under the key the window reads
/// ([`zerocode_core::launch_limits::PAYLOAD_KEY`]), with the field names the
/// window's limits read. A name spelled otherwise on one side would leave a
/// pane opened on a short list coming back on every tool after a restart,
/// with no error anywhere.
#[test]
fn zo_says_its_launch_limits_under_the_names_the_window_reads() {
    let key = zerocode_core::launch_limits::PAYLOAD_KEY;
    assert!(
        REPORTER.contains(&format!("pub const LAUNCH_LIMITS_KEY: &str = \"{key}\";")),
        "zo's reporter does not name the limits' key `{key}` the way the window does"
    );
    let start = strip_rust_comments(block_after(REPORTER, "pub fn session_start("));
    assert!(
        start.contains("LAUNCH_LIMITS_KEY"),
        "zo's start does not write its limits under the named key:\n{start}"
    );
    let said = strip_rust_comments(block_after(
        REPORTER,
        "fn payload(&self) -> serde_json::Value {",
    ));
    let read = serde_json::to_value(zerocode_core::launch_limits::LaunchLimits {
        allowed_tools: Some(vec!["read_file".to_string()]),
        no_spawn: true,
    })
    .expect("the window's limits as JSON");
    let fields: Vec<&String> = read.as_object().expect("an object").keys().collect();
    assert_eq!(fields.len(), 2, "{fields:?}");
    for field in fields {
        assert!(
            said.contains(&format!("\"{field}\"")),
            "zo's limits do not say `{field}` the way the window reads it:\n{said}"
        );
    }
}
