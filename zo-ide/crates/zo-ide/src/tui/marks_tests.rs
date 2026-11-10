//! The marks look, drawn: each test runs in a fresh run of this binary
//! (`style::in_fresh_run`), fixes the look there, and checks the rows — the
//! codex byte tests in this run never see it.

use std::time::Duration;

use super::ansi::Line;
use super::cells::{boot_card, prefixed, Prefix};
use super::style;
use super::tools::{Explored, Outcome, ToolCall, ToolGroup, ToolKind};
use crate::preferences::Preferences;

const WIDTH: usize = 60;

/// This test's path inside the crate, for its fresh run.
fn path(test: &str) -> String {
    let module = module_path!().split_once("::").map_or(module_path!(), |(_crate, path)| path);
    format!("{module}::{test}")
}

fn plain(lines: &[Line]) -> Vec<String> {
    lines.iter().map(|line| line.plain().trim_end().to_string()).collect()
}

fn finished(output: &str) -> Outcome {
    Outcome { ok: true, declined: false, output: output.to_string(), change: None, exit_code: None }
}

fn read_row(name: &str) -> ToolGroup {
    let mut group = ToolGroup::new(ToolCall::new(
        "a".to_string(),
        ToolKind::Explore(vec![Explored::Read { name: name.to_string() }]),
    ));
    let twelve = "line\n".repeat(12);
    assert!(group.complete("a", finished(&twelve)));
    group
}

#[test]
fn a_finished_read_is_drawn_as_a_call_with_its_result_at_the_edge() {
    if !style::in_fresh_run(&path("a_finished_read_is_drawn_as_a_call_with_its_result_at_the_edge")) {
        return;
    }
    style::init(&Preferences::default());
    let rows = plain(&read_row("src/app.py").lines(WIDTH, Duration::ZERO));
    let head = "⏺ Read(src/app.py)";
    let result = "12 lines";
    let gap = WIDTH - head.chars().count() - result.chars().count();
    assert_eq!(rows[0], format!("{head}{}{result}", " ".repeat(gap)));
}

#[test]
fn the_file_a_tool_row_names_opens_from_the_row() {
    if !style::in_fresh_run(&path("the_file_a_tool_row_names_opens_from_the_row")) {
        return;
    }
    style::init(&Preferences::default());
    let rows = read_row("src/app.py").lines(WIDTH, Duration::ZERO);
    let target = rows[0].spans.iter().find(|span| span.text == "src/app.py").expect("the target span");
    let link = target.link.as_ref().expect("a marks row links its file");
    assert!(link.as_str().starts_with("file:///") && link.as_str().ends_with("/src/app.py"), "{}", link.as_str());
    // A name's own space, `#` and `%` are escaped: left bare, a reader takes
    // `#1%.md` for the address's fragment and opens `notes/a b`.
    let rows = read_row("notes/a b#1%.md").lines(WIDTH, Duration::ZERO);
    let link = rows[0].spans.iter().find_map(|span| span.link.as_ref()).expect("the row links its file");
    assert!(link.as_str().ends_with("/notes/a%20b%231%25.md"), "{}", link.as_str());
}

#[test]
fn the_marks_glyphs_head_each_kind_of_row() {
    if !style::in_fresh_run(&path("the_marks_glyphs_head_each_kind_of_row")) {
        return;
    }
    style::init(&Preferences::default());
    assert_eq!(Prefix::hang().initial.text, "⏺ ", "an answer's first row");
    assert_eq!(Prefix::user().initial.text, "> ", "the person's words");
    assert_eq!(Prefix::tool_output().initial.text, "  ⎿ ", "a tool's hung output");
}

#[test]
fn the_session_head_is_one_row() {
    if !style::in_fresh_run(&path("the_session_head_is_one_row")) {
        return;
    }
    style::init(&Preferences::default());
    let rows = plain(&boot_card("zo", "1.1.133", "claude-opus-5", "high", "/srv/acme/project", WIDTH));
    assert_eq!(rows, vec!["✻ zo 1.1.133  ·  claude-opus-5 high  ·  /srv/acme/project".to_string()]);
}

/// The window reads `│› ` to know zo's field is empty
/// (`zerocode-shell` `composer_empty::observed`), so this look keeps it.
#[test]
fn the_composer_keeps_the_mark_the_window_reads() {
    if !style::in_fresh_run(&path("the_composer_keeps_the_mark_the_window_reads")) {
        return;
    }
    style::init(&Preferences::default());
    let (rows, _cursor) = super::composer::Composer::new().render(WIDTH, "Ask zo to do anything");
    assert!(plain(&rows).iter().any(|row| row.starts_with("│› Ask zo to do anything")), "{:?}", plain(&rows));
}

#[test]
fn a_flush_code_row_starts_at_column_zero_and_prose_keeps_its_prefix() {
    if !style::in_fresh_run(&path("a_flush_code_row_starts_at_column_zero_and_prose_keeps_its_prefix")) {
        return;
    }
    style::init(&Preferences { code_blocks_flush: Some(true), ..Preferences::default() });
    let mut code = Line::from_text("fn main() {}");
    code.code = true;
    let rows = plain(&prefixed(&[Line::from_text("Run this:"), code], 40, &Prefix::hang(), true));
    assert_eq!(rows, vec!["⏺ Run this:".to_string(), "fn main() {}".to_string()]);
}

#[test]
fn a_still_working_line_keeps_the_spinners_first_frame() {
    if !style::in_fresh_run(&path("a_still_working_line_keeps_the_spinners_first_frame")) {
        return;
    }
    style::init(&Preferences { reduce_motion: Some(true), ..Preferences::default() });
    let group = ToolGroup::new(ToolCall::new(
        "a".to_string(),
        ToolKind::Command { command: "make".to_string(), background: false },
    ));
    let early = group.lines(WIDTH, Duration::ZERO)[0].spans[0].text.clone();
    let later = group.lines(WIDTH, Duration::from_millis(410))[0].spans[0].text.clone();
    assert_eq!(early, later, "the spinner moved");
}
