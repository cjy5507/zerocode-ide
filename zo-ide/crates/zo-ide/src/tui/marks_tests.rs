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

/// The result ends where prose ends — the reading width, 100 columns — on a
/// pane wider than that. A target that does not fit there is not cut there:
/// the row takes what the pane has, and is cut only at the pane's edge
/// ("글이 짤리는 현상", 2026-11-12 — a row cut at 100 on a pane of 200).
#[test]
fn a_tool_row_gives_up_its_column_before_it_cuts_its_target() {
    if !style::in_fresh_run(&path("a_tool_row_gives_up_its_column_before_it_cuts_its_target")) {
        return;
    }
    style::init(&Preferences::default());
    let column = style::DEFAULT_PROSE_WIDTH;
    // A short target: the result stands on the prose column, as before.
    let short = plain(&read_row("src/app.py").lines(200, Duration::ZERO));
    assert!(short[0].ends_with("12 lines") && short[0].chars().count() == column, "{short:?}");
    // A target of 150 columns on a pane of 200: shown whole, result after it.
    let long = format!("{}app.py", "crates/app/src/session/".repeat(6));
    assert!(long.len() > 130 && long.len() < 160, "{}", long.len());
    let wide = plain(&read_row(&long).lines(200, Duration::ZERO));
    assert_eq!(wide[0], format!("⏺ Read({long}) 12 lines"));
    // The same target on a pane of 120: cut at the pane's edge, not at 100.
    let tight = plain(&read_row(&long).lines(120, Duration::ZERO));
    assert!(tight[0].contains('…') && tight[0].ends_with("12 lines"), "{tight:?}");
    assert_eq!(tight[0].chars().count(), 120, "{tight:?}");
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

/// The words a running call's row says, without its spinner.
fn running_words(kind: ToolKind) -> String {
    let group = ToolGroup::new(ToolCall::new("a".to_string(), kind));
    let row = group.lines(100, Duration::ZERO)[0].plain();
    row.strip_prefix("⠋ ").unwrap_or_else(|| panic!("a running row opens with the spinner: {row:?}")).to_string()
}

/// The words the same call says after it ended, as the `turns` view shows
/// it among the steps (its row's hung mark and a result's tail left off).
fn ended_words(kind: ToolKind) -> String {
    let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));
    assert!(group.complete("a", finished("")));
    let mut fold = super::turns::TurnFold::default();
    fold.take_steps(group.steps());
    let row = fold.live_rows(100, 16)[0].plain();
    let words = row.strip_prefix("  ⎿ ").unwrap_or_else(|| panic!("a step opens hung under the mark: {row:?}"));
    // What the call brought back follows its words after two spaces.
    words.split("  ").next().unwrap_or_default().trim_end().to_string()
}

/// One call is one text while it runs and after it ends (v1.1.149): the
/// running row said `Search(… in …)` and the finished step `Search … in …`.
/// Both are built from the same words now, and a search's words — the
/// pattern or question the model wrote — are quoted so the row does not read
/// as zo speaking.
#[test]
fn a_call_reads_the_same_while_it_runs_and_after_it_ends() {
    if !style::in_fresh_run(&path("a_call_reads_the_same_while_it_runs_and_after_it_ends")) {
        return;
    }
    style::init(&Preferences::default());
    let search = |query: &str, path: Option<&str>| {
        ToolKind::Explore(vec![Explored::Search { query: query.to_string(), path: path.map(str::to_string) }])
    };
    let cases = [
        (search("끝났다고 하기 전에 확인했는가", Some(".zo")), "Search(\"끝났다고 하기 전에 확인했는가\" in .zo)"),
        (search("끝났다고 하기 전에 확인했는가", None), "Search(\"끝났다고 하기 전에 확인했는가\")"),
        (search("do_(GET|POST)", Some("src")), "Search(\"do_(GET|POST)\" in src)"),
        (
            ToolKind::WebSearch { detail: "rust async".to_string() },
            "Search(\"rust async\" on the web)",
        ),
        (
            ToolKind::WebSearch { detail: "https://example.com/a".to_string() },
            "Search(\"https://example.com/a\" on the web)",
        ),
        (ToolKind::Explore(vec![Explored::Read { name: "src/app.py".to_string() }]), "Read(src/app.py)"),
        (ToolKind::Explore(vec![Explored::List { path: "src/*.rs".to_string() }]), "List(src/*.rs)"),
        (ToolKind::Command { command: "ls -la".to_string(), background: false }, "Run(ls -la)"),
    ];
    for (kind, words) in cases {
        assert_eq!(running_words(kind.clone()), words, "running");
        assert_eq!(ended_words(kind), words, "ended");
    }
}

/// The two turns of the 1.1.148 screen, drawn again: the fold of the first
/// (its mark on its first row only — the rule of every hung block in these
/// files: a tool's output, a diff, a wrapped status detail, a transcript
/// body), and the second's steps among the model's lines, the search quoted.
#[test]
fn the_screens_two_turns_are_drawn_again_in_one_vocabulary() {
    use std::time::Instant;

    use super::turns::{cell, Step, StepKind, TurnFold};

    if !style::in_fresh_run(&path("the_screens_two_turns_are_drawn_again_in_one_vocabulary")) {
        return;
    }
    style::init(&Preferences::default());
    let now = Instant::now();
    let step = |kind, verb: &str, target: &str| Step {
        kind,
        verb: verb.to_string(),
        target: target.to_string(),
        change: None,
        ok: true,
        declined: false,
        failure: None,
        error: None,
        result: None,
        started: now.checked_sub(Duration::from_secs(39)).unwrap_or(now),
        took: None,
    };
    let steps = vec![
        step(StepKind::Read, "Read", "a.md"),
        step(StepKind::Read, "Read", "b.md"),
        step(StepKind::Read, "Read", "c.md"),
        step(StepKind::Run, "Run", "ls -la"),
        step(StepKind::Run, "Run", "ls -d crates/* zo-ide/crates/* ui/*"),
    ];
    let rows = plain(&cell(&steps, 80, now));
    assert_eq!(
        rows,
        [
            "",
            "⏺ Worked for 39s · 5 steps                                      ctrl+o to expand",
            "  ⎿ Read 3 files",
            "    Ran ls -la",
            "    Ran ls -d crates/* zo-ide/crates/* ui/*",
        ]
    );

    let mut fold = TurnFold::default();
    let said = |fold: &mut TurnFold, words: &str| {
        assert!(fold.take_text(vec![Line::from_text(format!("⏺ {words}"))]).is_none());
        fold.text_closed();
        fold.preamble_joins_the_work();
    };
    // The steps come from finished calls, as the screen's did.
    let found = |query: &str, path: Option<&str>| {
        let mut group = ToolGroup::new(ToolCall::new(
            "a".to_string(),
            ToolKind::Explore(vec![Explored::Search { query: query.to_string(), path: path.map(str::to_string) }]),
        ));
        assert!(group.complete("a", finished("")));
        group.steps()
    };
    let query = "끝났다고 하기 전에 확인했는가";
    said(&mut fold, "프로젝트 내 규칙을 확인하기 위해 설정을 봅니다");
    fold.take_steps(found(query, Some(".zo")));
    said(&mut fold, "판단 기준이 정의된 위치를 찾습니다");
    fold.take_steps(found(query, None));
    let wide = plain(&fold.live_rows(80, 16));
    assert_eq!(
        wide,
        [
            "⏺ 프로젝트 내 규칙을 확인하기 위해 설정을 봅니다",
            "  ⎿ Search(\"끝났다고 하기 전에 확인했는가\" in .zo)  no matches",
            "",
            "⏺ 판단 기준이 정의된 위치를 찾습니다",
            "  ⎿ Search(\"끝났다고 하기 전에 확인했는가\")  no matches",
        ]
    );
    // On a pane 40 columns wide a full-width (two-column) query is cut at the
    // edge by columns, not by characters: no row is wider than the pane.
    for row in fold.live_rows(40, 16) {
        assert!(row.width() <= 40, "{:?} is {} columns", row.plain(), row.width());
    }
    let running = ToolGroup::new(ToolCall::new(
        "a".to_string(),
        ToolKind::Explore(vec![Explored::Search { query: query.to_string(), path: Some("/work/.zo".to_string()) }]),
    ));
    for width in [40, 60] {
        for row in running.lines(width, Duration::ZERO) {
            assert!(row.width() <= width, "{:?} is {} columns", row.plain(), row.width());
        }
    }
}

fn failed(output: &str) -> Outcome {
    Outcome { ok: false, declined: false, output: output.to_string(), change: None, exit_code: None }
}

/// Finished calls as the fold takes them: `(kind, outcome)` each in a cell of
/// its own, in the order they are given.
fn steps_of(calls: Vec<(ToolKind, Outcome)>) -> Vec<super::turns::Step> {
    let mut group: Option<ToolGroup> = None;
    for (at, (kind, _)) in calls.iter().enumerate() {
        let call = ToolCall::new(format!("c{at}"), kind.clone());
        match group.as_mut() {
            Some(group) => group.push(call),
            None => group = Some(ToolGroup::new(call)),
        }
    }
    let mut group = group.expect("a call");
    for (at, (_, outcome)) in calls.into_iter().enumerate() {
        assert!(group.complete(&format!("c{at}"), outcome));
    }
    group.steps()
}

fn fetch(url: &str) -> ToolKind {
    ToolKind::WebFetch { url: url.to_string() }
}

/// The second screen of 1.1.148 (v1.1.149): addresses fetched one after
/// another, a tool of an MCP server, one that failed. A fetch says `Fetch`
/// and its address without the scheme, an MCP tool its server and its name,
/// a finished call what it brought back at the end of its row, and a failed
/// one why, under it.
#[test]
fn a_run_of_fetches_reads_as_calls_with_their_results() {
    if !style::in_fresh_run(&path("a_run_of_fetches_reads_as_calls_with_their_results")) {
        return;
    }
    style::init(&Preferences::default());
    let page = "x".repeat(3174);
    let steps = steps_of(vec![
        (fetch("https://huggingface.co/api/models/acme/alpha"), finished(&page)),
        (fetch("https://huggingface.co/api/models/acme/beta"), failed("HTTP 404 Not Found\nbody the model reads")),
        (
            ToolKind::WebSearch { detail: "https://example.test/a".to_string() },
            finished(&"y".repeat(812)),
        ),
        (
            ToolKind::Call { name: "mcp__almanac__read_webpage".to_string(), detail: "https://example.test/page".to_string() },
            finished("text"),
        ),
        (ToolKind::Call { name: "mcp__almanac__whoami".to_string(), detail: String::new() }, finished("me")),
        (
            ToolKind::Explore(vec![Explored::Search { query: "TODO".to_string(), path: Some("src".to_string()) }]),
            finished("src/a.rs:1:// TODO\nsrc/b.rs:2:// TODO\n"),
        ),
        (ToolKind::Command { command: "ls -la".to_string(), background: false }, finished("a\nb\nc\n")),
    ]);
    let mut fold = super::turns::TurnFold::default();
    fold.take_steps(steps);
    assert_eq!(
        plain(&fold.live_rows(80, 16)),
        [
            "  ⎿ Fetch(huggingface.co/api/models/acme/alpha)  3.1 KB",
            "    Fetch(huggingface.co/api/models/acme/beta)  failed",
            "    HTTP 404 Not Found",
            "    Search(\"https://example.test/a\" on the web)  812 B",
            "    almanac · read_webpage(https://example.test/page)",
            "    almanac · whoami",
            "    Search(\"TODO\" in src)  2 matches",
            "    Run(ls -la)  3 lines",
        ]
    );
}

/// The row keeps its result before its target: on a pane 40 columns wide a
/// long address gives up its middle, a long query its end, and the result
/// stays; no row is wider than the pane.
#[test]
fn a_steps_result_is_kept_when_its_target_is_cut() {
    if !style::in_fresh_run(&path("a_steps_result_is_kept_when_its_target_is_cut")) {
        return;
    }
    style::init(&Preferences::default());
    let steps = steps_of(vec![
        (fetch("https://huggingface.co/api/models/acme/a-very-long-model-name-for-this-test"), finished(&"x".repeat(3174))),
        (
            ToolKind::Explore(vec![Explored::Search {
                query: "끝났다고 하기 전에 확인했는가 끝났다고 하기 전에 확인했는가".to_string(),
                path: None,
            }]),
            finished("a:1:x\n"),
        ),
    ]);
    let mut fold = super::turns::TurnFold::default();
    fold.take_steps(steps);
    for row in fold.live_rows(40, 16) {
        assert!(row.width() <= 40, "{:?} is {} columns", row.plain(), row.width());
    }
    let rows = plain(&fold.live_rows(40, 16));
    assert!(rows[0].starts_with("  ⎿ Fetch(") && rows[0].contains('…') && rows[0].ends_with(")  3.1 KB"), "{rows:?}");
    assert!(rows[1].starts_with("    Search(\"끝났다고") && rows[1].ends_with("…)  1 match"), "{rows:?}");
}

/// A step that failed is not folded away into `… N earlier`, and the fold's
/// head counts it; the calls stand in the order they were made.
#[test]
fn a_failed_step_stays_when_the_rest_fold_away() {
    if !style::in_fresh_run(&path("a_failed_step_stays_when_the_rest_fold_away")) {
        return;
    }
    style::init(&Preferences::default());
    let mut calls: Vec<(ToolKind, Outcome)> = (0..10)
        .map(|n| (ToolKind::Explore(vec![Explored::Read { name: format!("f{n}.rs") }]), finished("l\n")))
        .collect();
    calls[1] = (ToolKind::Explore(vec![Explored::Read { name: "gone.rs".to_string() }]), failed("No such file: gone.rs"));
    let steps = steps_of(calls);
    let mut fold = super::turns::TurnFold::default();
    fold.take_steps(steps.clone());
    assert_eq!(
        plain(&fold.live_rows(80, 6)),
        [
            "  ⎿ … 6 earlier",
            "    Read(gone.rs)  failed",
            "    No such file: gone.rs",
            "    Read(f7.rs)  1 line",
            "    Read(f8.rs)  1 line",
            "    Read(f9.rs)  1 line",
        ]
    );
    let head = plain(&super::turns::cell(&steps, 100, std::time::Instant::now()));
    assert!(head[1].starts_with("⏺ Worked · 10 steps · 1 failed"), "{head:?}");
    assert!(head.iter().any(|row| row.trim() == "No such file: gone.rs"), "{head:?}");
}

/// Calls made together stand in the order they were made, not in the order
/// they ended.
#[test]
fn calls_made_together_stand_in_the_order_they_were_made() {
    if !style::in_fresh_run(&path("calls_made_together_stand_in_the_order_they_were_made")) {
        return;
    }
    style::init(&Preferences::default());
    let read = |name: &str| ToolKind::Explore(vec![Explored::Read { name: name.to_string() }]);
    let mut group = ToolGroup::new(ToolCall::new("a".to_string(), read("a.rs")));
    group.push(ToolCall::new("b".to_string(), read("b.rs")));
    group.push(ToolCall::new("c".to_string(), read("c.rs")));
    for id in ["c", "a", "b"] {
        assert!(group.complete(id, finished("l\n")));
    }
    let targets: Vec<String> = group.steps().into_iter().map(|step| step.target).collect();
    assert_eq!(targets, ["a.rs", "b.rs", "c.rs"]);
}
