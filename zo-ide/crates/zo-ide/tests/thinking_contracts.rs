//! Source contracts for thinking and helper rows on screen (t-5872).
//!
//! These ask the source for names, not for literals: the one function the
//! status word is derived through, the one default the interactive front
//! shows thinking by, the row constants codex spells, and the one phrase
//! table every helper row goes through. A second heading rule, a second row
//! spelling or a bare `240` would pass every golden and still be the drift
//! this file exists to refuse.

use std::path::Path;

fn source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|why| panic!("{}: {why}", path.display()))
}

/// The shipped half of a file, comments stripped, so prose cannot satisfy a
/// count that code should.
fn shipped(relative: &str) -> String {
    let text = source(relative);
    let (code, _) = text
        .split_once("#[cfg(test)]\nmod tests")
        .unwrap_or((&text, ""));
    code.lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The status word comes from one function, in one place. `app.rs` derives
/// nothing itself: it asks `thinking::live_heading` once and keeps codex's
/// `first_bold_heading` only through the same module.
#[test]
fn the_status_word_has_one_derivation() {
    let app = shipped("src/tui/app.rs");
    assert_eq!(app.matches("thinking::live_heading(").count(), 1, "app.rs asks live_heading once");
    assert_eq!(app.matches("fn first_bold_heading").count(), 0, "the codex rule lives in thinking.rs");
    let thinking = shipped("src/tui/thinking.rs");
    assert!(thinking.contains("pub fn live_heading(text: &str, max_columns: usize) -> Option<String>"));
    assert!(thinking.contains("pub fn first_bold_heading(text: &str) -> Option<&str>"));
    assert!(thinking.contains("pub const LIVE_HEADING_MAX_COLUMNS: usize"));
    assert!(thinking.contains("pub const SCAN_KEEP_BYTES: usize"));
}

/// The interactive front shows thinking by default; the setting and the
/// command are the two ways off, and the pipe front keeps its flag.
#[test]
fn thinking_is_shown_by_default_and_turned_off_by_name() {
    let thinking = shipped("src/tui/thinking.rs");
    assert!(thinking.contains("pub const SHOW_BY_DEFAULT: bool = true;"));
    let app = shipped("src/tui/app.rs");
    assert!(app.contains("thinking::SHOW_BY_DEFAULT"), "the front reads the one default");
    assert!(app.contains("fn toggle_thinking(&mut self)"));
    assert_eq!(app.matches("Some(Slash::Thinking) =>").count(), 2, "idle and mid-turn dispatch");
    let preferences = shipped("src/preferences.rs");
    assert!(preferences.contains("pub const SHOW_THINKING_KEY: &str = \"showThinking\";"));
    let slash = shipped("src/slash.rs");
    assert!(slash.contains("Self::Thinking => \"/thinking\""));
    let render = shipped("src/ide/render.rs");
    assert!(render.contains("show_thinking: false,"), "the pipe renderer's default is codex exec's");
}

/// A headerless block is a thinking cell: the stream renders it and says so,
/// and the one place that commits reasoning asks that question.
#[test]
fn headerless_thinking_becomes_a_titled_cell_in_one_place() {
    let cells = shipped("src/tui/cells.rs");
    assert!(cells.contains("pub const fn finished_headerless_thinking(&self) -> bool"));
    assert!(!cells.contains("transcript_only"), "headerless reasoning is no longer withheld");
    let app = shipped("src/tui/app.rs");
    assert_eq!(app.matches("thinking::cell(").count(), 1);
    let thinking = shipped("src/tui/thinking.rs");
    assert!(thinking.contains("pub const THINKING_PREVIEW_ROWS: usize = super::tools::OUTPUT_MAX_ROWS;"));
    assert!(thinking.contains("folds::wrap_cell_with_teaser("), "the pane fold is the tool cell's");
}

/// Every helper row — status detail, spawn-cell row — is spelled by
/// `strings::helper` through one formatter, and the finished spawn's preview
/// carries codex's constants by name.
#[test]
fn helper_rows_have_one_spelling_and_codex_constants() {
    let app = shipped("src/tui/app.rs");
    assert_eq!(app.matches("super::strings::helper(").count(), 1, "one call, in subagent_progress_line");
    assert_eq!(app.matches("subagent_progress_line(progress, true)").count(), 1);
    assert_eq!(app.matches("subagent_progress_line(progress, false)").count(), 1);
    assert!(app.contains("const HELPER_OUTPUT_TAIL_MAX_COLUMNS: usize"));
    let tools = shipped("src/tui/tools.rs");
    assert!(tools.contains("const COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES: usize = 240;"));
    assert!(tools.contains("const COLLAB_PROMPT_PREVIEW_GRAPHEMES: usize = 160;"));
    assert_eq!(tools.matches("fn completed_preview(").count(), 1);
    let strings = shipped("src/tui/strings.rs");
    assert!(strings.contains("pub fn helper("));
    assert!(strings.contains("output_tail: Option<&str>"));
}

/// The route fact is read where it is already known — the smart install and
/// the step observer — and reaches the row through one channel, never a
/// question of its own.
#[test]
fn the_route_fact_is_read_from_the_seats_that_already_hold_it() {
    let smart = shipped("src/session/smart_runtime.rs");
    assert_eq!(smart.matches("RouteFact::turn_start(").count(), 1);
    assert_eq!(smart.matches("RouteFact::step(").count(), 1);
    assert!(!smart.contains("SystemOneClient"), "no new question to Jev");
    let fact = shipped("src/session/route_fact.rs");
    assert!(fact.contains("pub(crate) fn turn_start("));
    assert!(fact.contains("pub(crate) fn step(row: &runtime::StepRow)"));
    assert!(fact.contains("watch::Sender<Option<RouteFact>>"));
    let app = shipped("src/tui/app.rs");
    assert_eq!(app.matches("fn set_route_fact(").count(), 1);
    assert!(app.contains("status.set_inline_message(word)"), "codex's inline slot after the interrupt hint");
    let turn = shipped("../tools/src/misc_tools/smart_router/turn.rs");
    assert!(turn.contains("pub judged: bool"));
}
