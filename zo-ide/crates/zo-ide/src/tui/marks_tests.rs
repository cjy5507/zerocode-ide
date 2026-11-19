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
    Outcome { ok: true, declined: false, output: output.to_string(), stderr: String::new(), change: None, exit_code: None }
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
fn the_composer_placeholder_keeps_sgr_dim_under_a_measured_palette() {
    use super::ansi::write_spans_for_palette;
    use super::palette::{ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("the_composer_placeholder_keeps_sgr_dim_under_a_measured_palette")) {
        return;
    }
    style::init(&Preferences::default());
    let horizon = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
    let (rows, _cursor) = super::composer::Composer::new().render(WIDTH, "Ask zo to do anything");
    let rendered: Vec<String> = rows
        .iter()
        .map(|row| {
            let mut out = String::new();
            write_spans_for_palette(row, Some(horizon), &mut out);
            out
        })
        .collect();
    assert!(
        rendered.iter().any(|row| row.contains("\u{1b}[2mAsk zo to do anything")),
        "{rendered:?}"
    );
}

#[test]
fn marks_secondary_text_uses_measured_ink_and_respects_fallbacks() {
    use super::ansi::{write_spans_for_palette, Color, Span, Style};
    use super::palette::{ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("marks_secondary_text_uses_measured_ink_and_respects_fallbacks")) {
        return;
    }
    style::init(&Preferences::default());
    let horizon = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
    let ansi16 = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::Ansi16);

    let render = |spans: Vec<Span>, palette: Option<TerminalPalette>| {
        let mut out = String::new();
        write_spans_for_palette(&Line::new(spans), palette, &mut out);
        out
    };

    let dim_horizon = render(vec![Span::dim("12 lines")], Some(horizon));
    assert!(dim_horizon.contains("38;2;") && !dim_horizon.contains("\u{1b}[2m"), "{dim_horizon:?}");

    let sgr_dim_horizon = render(vec![Span::new("Ask zo to do anything", Style::new().sgr_dim())], Some(horizon));
    assert!(sgr_dim_horizon.contains("\u{1b}[2m") && !sgr_dim_horizon.contains("38;2;"), "{sgr_dim_horizon:?}");

    let dim_none = render(vec![Span::dim("12 lines")], None);
    assert!(dim_none.contains("\u{1b}[2m") && !dim_none.contains("38;2;"), "{dim_none:?}");

    let dim_ansi16 = render(vec![Span::dim("12 lines")], Some(ansi16));
    assert!(dim_ansi16.contains("\u{1b}[2m") && !dim_ansi16.contains("38;2;"), "{dim_ansi16:?}");

    let colored_dim_horizon = render(vec![Span::new("warn", Style::new().fg(Color::CYAN).dim())], Some(horizon));
    let colored_dim_none = render(vec![Span::new("warn", Style::new().fg(Color::CYAN).dim())], None);
    assert_eq!(colored_dim_horizon, colored_dim_none);

    let adjacent = render(
        vec![Span::dim("a"), Span::new("b", Style::new().sgr_dim())],
        None,
    );
    assert_eq!(adjacent, "\u{1b}[2mab");
}

#[test]
fn marks_lifts_low_contrast_ansi_inks_and_keeps_compliant_and_fallback_bytes() {
    use super::ansi::{write_spans_for_palette, Color, Span, Style};
    use super::palette::{contrast_ratio, ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("marks_lifts_low_contrast_ansi_inks_and_keeps_compliant_and_fallback_bytes")) {
        return;
    }
    style::init(&Preferences::default());

    let mut solarized_slots = [None; 16];
    solarized_slots[1] = Some((220, 50, 47));
    solarized_slots[2] = Some((133, 153, 0));
    solarized_slots[3] = Some((181, 137, 0));
    solarized_slots[6] = Some((42, 161, 152));
    let solarized_ground = (253, 246, 227);
    let solarized_ink = (101, 123, 131);
    let solarized = TerminalPalette::new(solarized_ink, solarized_ground, ColorLevel::TrueColor)
        .with_ansi16(solarized_slots);
    let solarized_no_ansi16 =
        TerminalPalette::new(solarized_ink, solarized_ground, ColorLevel::TrueColor);
    let solarized_ansi16_level =
        TerminalPalette::new(solarized_ink, solarized_ground, ColorLevel::Ansi16)
            .with_ansi16(solarized_slots);
    let solarized_floor = contrast_ratio(solarized_ink, solarized_ground) * 0.75;

    let mut horizon_slots = [None; 16];
    horizon_slots[1] = Some((233, 86, 120));
    horizon_slots[6] = Some((89, 225, 227));
    let horizon = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor)
        .with_ansi16(horizon_slots);

    let render = |style: Style, palette: Option<TerminalPalette>| {
        let mut out = String::new();
        write_spans_for_palette(&Line::new(vec![Span::new("x", style)]), palette, &mut out);
        out
    };
    let parse_fg_rgb = |rendered: &str| -> (u8, u8, u8) {
        let (_, rest) = rendered
            .split_once("38;2;")
            .unwrap_or_else(|| panic!("expected 38;2; RGB SGR in {rendered:?}"));
        let (params, _) = rest.split_once('m').expect("SGR terminator");
        let mut parts = params.split(';');
        (
            parts.next().expect("red").parse().expect("u8"),
            parts.next().expect("green").parse().expect("u8"),
            parts.next().expect("blue").parse().expect("u8"),
        )
    };

    for color in [Color::CYAN, Color::GREEN, Color::YELLOW] {
        let rendered = render(Style::new().fg(color), Some(solarized));
        let rgb = parse_fg_rgb(&rendered);
        assert!(
            contrast_ratio(rgb, solarized_ground) >= solarized_floor,
            "{color:?} lifted to {rgb:?} should clear floor {solarized_floor}: {rendered:?}"
        );
    }

    for color in [Color::CYAN, Color::RED] {
        assert_eq!(
            render(Style::new().fg(color), Some(horizon)),
            render(Style::new().fg(color), None),
            "compliant Horizon Dark {color:?} should keep slot bytes"
        );
    }

    let dim_cyan = render(Style::new().fg(Color::CYAN).dim(), Some(solarized));
    assert!(!dim_cyan.contains("\u{1b}[2m"), "dim should turn off: {dim_cyan:?}");
    let dim_rgb = parse_fg_rgb(&dim_cyan);
    assert!(
        contrast_ratio(dim_rgb, solarized_ground) >= solarized_floor,
        "dim cyan {dim_rgb:?} should clear floor {solarized_floor}"
    );

    let cyan = Style::new().fg(Color::CYAN);
    let baseline = render(cyan, None);
    assert_eq!(render(cyan, Some(solarized_no_ansi16)), baseline);
    assert_eq!(render(cyan, Some(solarized_ansi16_level)), baseline);

    assert_eq!(
        render(Style::new().fg(Color::Indexed(6)), Some(solarized)),
        render(Style::new().fg(Color::Base(6)), Some(solarized)),
    );
    assert_eq!(
        render(Style::new().fg(Color::Indexed(200)), Some(solarized)),
        render(Style::new().fg(Color::Indexed(200)), None),
    );
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
    Outcome { ok: false, declined: false, output: output.to_string(), stderr: String::new(), change: None, exit_code: None }
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
            "      HTTP 404 Not Found",
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
            "      No such file: gone.rs",
            "    Read(f7.rs)  1 line",
            "    Read(f8.rs)  1 line",
            "    Read(f9.rs)  1 line",
        ]
    );
    let head = plain(&super::turns::cell(&steps, 100, std::time::Instant::now()));
    assert!(head[1].starts_with("⏺ Worked · 10 steps · 1 failed"), "{head:?}");
    assert!(head.iter().any(|row| row.trim() == "No such file: gone.rs"), "{head:?}");
    // The failed call reads in the fold as its row read while the turn ran.
    assert!(head.iter().any(|row| row.ends_with("Read(gone.rs)  failed")), "{head:?}");
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

#[test]
fn turns_live_rows_aligns_finished_and_running_calls_and_marks_multiline_commands() {
    if !style::in_fresh_run(&path("turns_live_rows_aligns_finished_and_running_calls_and_marks_multiline_commands")) {
        return;
    }
    style::init(&Preferences::default());
    let twelve = "line\n".repeat(12);
    let mut group = ToolGroup::new(ToolCall::new(
        "r1".to_string(),
        ToolKind::Explore(vec![Explored::Read { name: "controlled/types.go:420-579".to_string() }]),
    ));
    assert!(group.complete("r1", finished(&twelve)));
    group.push(ToolCall::new(
        "r2".to_string(),
        ToolKind::Explore(vec![Explored::Read { name: "server/controlled.go:550-699".to_string() }]),
    ));
    assert!(group.complete("r2", finished(&twelve)));
    group.push(ToolCall::new(
        "cmd".to_string(),
        ToolKind::Command {
            command: "docker compose exec -T exploit-runner sh -c 'python3 -c \"\nimport sys\nprint(1)\n\"'".to_string(),
            background: false,
        },
    ));
    let rows = plain(&group.turns_live_rows(98, Duration::ZERO, false));
    assert_eq!(
        rows,
        [
            "  ⎿ Read(controlled/types.go:420-579)  12 lines",
            "    Read(server/controlled.go:550-699)  12 lines",
            "  ⠋ Run(docker compose exec -T exploit-runner sh -c 'python3 -c \" …)  +3 lines",
        ]
    );
    let continued = plain(&group.turns_live_rows(98, Duration::ZERO, true));
    assert!(continued[0].starts_with("    Read("), "{continued:?}");
}

#[test]
fn a_failed_bare_tool_strips_redundant_prefix_and_keeps_its_result_two_columns_off() {
    if !style::in_fresh_run(&path("a_failed_bare_tool_strips_redundant_prefix_and_keeps_its_result_two_columns_off")) {
        return;
    }
    style::init(&Preferences::default());
    let steps = steps_of(vec![
        (
            ToolKind::Call { name: "session_recall".to_string(), detail: String::new() },
            failed("invalid input: session_recall: provide `query` to search across sessions, or `session_id`"),
        ),
        (
            ToolKind::Command { command: "psql -c 'select 1'".to_string(), background: false },
            Outcome {
                ok: false,
                declined: false,
                output: "ERROR:  function left(jsonb, integer) does not exist".to_string(),
                stderr: String::new(),
                change: None,
                exit_code: Some(1),
            },
        ),
    ]);
    let mut fold = super::turns::TurnFold::default();
    fold.take_steps(steps);
    assert_eq!(
        plain(&fold.live_rows(98, 16)),
        [
            "  ⎿ session_recall  failed",
            "      provide `query` to search across sessions, or `session_id`",
            "    Run(psql -c 'select 1')  exit 1",
            "      ERROR:  function left(jsonb, integer) does not exist",
        ]
    );
}

#[test]
fn marks_prose_keeps_a_right_gutter_on_medium_panes() {
    if !style::in_fresh_run(&path("marks_prose_keeps_a_right_gutter_on_medium_panes")) {
        return;
    }
    style::init(&Preferences::default());
    let prose = Line::from_text("word ".repeat(30).trim_end().to_string());
    let rows = super::cells::prefixed_prose(&[prose], 98, &Prefix::hang(), true);
    assert!(rows.len() >= 2, "{rows:?}");
    assert!(rows.iter().all(|row| row.width() <= 95), "{rows:?}");
}

/// A call reads the same while it runs and once it ends, and is cut the same
/// way: an address too long for its row gives up its middle on both rows,
/// keeping its host and its last part. The running row cut it at its end
/// (f81e7c8a4), so the row changed its words the moment the call ended.
#[test]
fn a_long_address_gives_up_its_middle_while_it_runs_and_after() {
    if !style::in_fresh_run(&path("a_long_address_gives_up_its_middle_while_it_runs_and_after")) {
        return;
    }
    style::init(&Preferences::default());
    let url = "https://huggingface.co/api/models/acme-research/zq-coder-7b-instruct-preview/tree/main/config.json";
    let mut group = ToolGroup::new(ToolCall::new("a".to_string(), ToolKind::WebFetch { url: url.to_string() }));
    let running = plain(&group.turns_live_rows(60, Duration::ZERO, false));
    assert!(group.complete("a", finished(&"x".repeat(3100))));
    let done = plain(&group.turns_live_rows(60, Duration::ZERO, false));
    // The call's words on each row: from its verb to its closing parenthesis.
    let call = |row: &str| -> String {
        let from = row.find("Fetch(").unwrap_or_else(|| panic!("{row:?}"));
        let to = row.rfind(')').unwrap_or_else(|| panic!("{row:?}"));
        row[from..=to].to_string()
    };
    let (shown, said) = (call(&running[0]), call(&done[0]));
    for words in [&shown, &said] {
        assert!(words.starts_with("Fetch(huggingface.co/"), "{running:?} {done:?}");
        assert!(words.ends_with("config.json)"), "{running:?} {done:?}");
        assert_eq!(words.matches('…').count(), 1, "{running:?} {done:?}");
    }
    // The finished row gives its result room from the same middle.
    let (head, tail) = said.split_once('…').expect("cut");
    assert!(shown.starts_with(head) && shown.ends_with(tail), "{shown:?} / {said:?}");
    for row in running.iter().chain(&done) {
        assert!(super::ansi::str_width(row) <= 60, "{row:?}");
    }
}

#[test]
fn multiline_shell_continuations_show_ellipsis_in_turns_view() {
    if !style::in_fresh_run(&path("multiline_shell_continuations_show_ellipsis_in_turns_view")) {
        return;
    }
    style::init(&Preferences::default());
    for line1 in ["cd repo &&", "test -f a ||", "bash -c (", "if true; then {"] {
        let group = ToolGroup::new(ToolCall::new(
            "cmd".to_string(),
            ToolKind::Command { command: format!("{line1}\n  echo ok"), background: false },
        ));
        let rows = plain(&group.turns_live_rows(98, Duration::ZERO, false));
        assert!(
            rows[0].contains(&format!("Run({line1} …)  +1 line")),
            "expected continuation ellipsis for {line1:?}: {rows:?}"
        );
    }
    for escaped in ["echo \\(", "echo \\{"] {
        let group = ToolGroup::new(ToolCall::new(
            "cmd".to_string(),
            ToolKind::Command { command: format!("{escaped}\n  echo ok"), background: false },
        ));
        let rows = plain(&group.turns_live_rows(98, Duration::ZERO, false));
        assert!(
            rows[0].contains(&format!("Run({escaped})  +1 line")),
            "escaped delimiter must not look unclosed for {escaped:?}: {rows:?}"
        );
    }
}

#[test]
fn failed_bash_prefers_stderr_and_error_lines_over_leading_stdout_headers() {
    use runtime::message_stream::{BashResult, ToolResultBody};
    if !style::in_fresh_run(&path("failed_bash_prefers_stderr_and_error_lines_over_leading_stdout_headers")) {
        return;
    }
    style::init(&Preferences::default());
    let separate_stderr = Outcome::from_result(
        false,
        &ToolResultBody::Bash(BashResult {
            exit_code: 1,
            stdout: "count\n-----\n10\n".to_string(),
            stderr: "ERROR: column \"outcome\" does not exist\n".to_string(),
            truncated: false,
        }),
        "",
    );
    let merged_stdout = Outcome::from_result(
        false,
        &ToolResultBody::Bash(BashResult {
            exit_code: 1,
            stdout: "id | status | goal\nERROR: column \"task_id\" does not exist\n".to_string(),
            stderr: String::new(),
            truncated: false,
        }),
        "",
    );
    let steps = steps_of(vec![
        (
            ToolKind::Command {
                command: "docker exec exploit-lab-postgres-1 psql -U artex -d artex -c \"SELECT COUNT(*) FROM exploration_nodes\"".to_string(),
                background: false,
            },
            separate_stderr,
        ),
        (
            ToolKind::Command {
                command: "docker exec exploit-lab-postgres-1 psql -U artex -d artex -c \"SELECT id, worker, kind FROM tasks\"".to_string(),
                background: false,
            },
            merged_stdout,
        ),
    ]);
    let cell = plain(&super::turns::cell(&steps, 78, std::time::Instant::now()));
    assert!(
        cell.iter().any(|row| row.contains(")  exit 1") && row.contains("Run(docker exec")),
        "failed command row must keep closing paren and exit 1 on 78-col pane: {cell:#?}"
    );
    assert!(
        cell.iter().any(|row| row.contains("ERROR: column \"outcome\" does not exist")),
        "must pick stderr error over stdout count header: {cell:#?}"
    );
    assert!(
        cell.iter().any(|row| row.contains("ERROR: column \"task_id\" does not exist")),
        "must pick ERROR line over stdout table header when merged: {cell:#?}"
    );
    assert!(!cell.iter().any(|row| row.trim() == "count"), "{cell:#?}");
    assert!(!cell.iter().any(|row| row.trim() == "id | status | goal"), "{cell:#?}");
}

#[test]
fn committed_fold_caps_long_failure_runs_while_keeping_total_in_head() {
    if !style::in_fresh_run(&path("committed_fold_caps_long_failure_runs_while_keeping_total_in_head")) {
        return;
    }
    style::init(&Preferences::default());
    let steps = steps_of(
        (0..10)
            .map(|i| {
                (
                    ToolKind::Command { command: format!("probe_{i}"), background: false },
                    Outcome {
                        ok: false,
                        declined: false,
                        output: format!("ERROR: probe {i} failed"),
                        stderr: String::new(),
                        change: None,
                        exit_code: Some(1),
                    },
                )
            })
            .collect(),
    );
    let cell = plain(&super::turns::cell(&steps, 98, std::time::Instant::now()));
    assert!(cell[1].contains("10 steps · 10 failed"), "{cell:#?}");
    assert!(cell.iter().any(|row| row.contains("… +6 earlier failures")), "{cell:#?}");
    assert!(!cell.iter().any(|row| row.contains("probe_5")), "{cell:#?}");
    assert!(cell.iter().any(|row| row.contains("probe_6")), "{cell:#?}");
    assert!(cell.iter().any(|row| row.contains("probe_9")), "{cell:#?}");
}

#[test]
fn marks_blockquote_uses_dim_bar_and_body_ink() {
    if !style::in_fresh_run(&path("marks_blockquote_uses_dim_bar_and_body_ink")) {
        return;
    }
    style::init(&Preferences::default());
    let lines = super::markdown::render("> 주의");
    assert_eq!(lines.len(), 1, "{lines:?}");
    let prefix = &lines[0].spans[0];
    assert_eq!(prefix.text, "│ ");
    assert!(prefix.style.dim);
    assert_eq!(prefix.style.fg, None);
    let body = &lines[0].spans[1];
    assert_eq!(body.text, "주의");
    assert_eq!(body.style.fg, None);

    let two_lines = super::markdown::render_wrapped("> first line\n> second line", Some(80));
    assert_eq!(two_lines.len(), 2, "{two_lines:?}");
    for line in &two_lines {
        assert_eq!(line.spans[0].text, "│ ");
        assert!(line.spans[0].style.dim);
        assert_eq!(line.spans[0].style.fg, None);
    }

    let quote = super::markdown::render("> one two three four five six seven");
    let hanging = quote[0]
        .continuation
        .as_ref()
        .expect("blockquote carries hanging bar");
    assert_eq!(hanging.text, "│ ");
    assert!(hanging.style.dim);
    assert_eq!(hanging.style.fg, None);
    let wrapped = super::wrap::wrap_line(&quote[0], 16, hanging);
    assert!(wrapped.len() >= 2, "{wrapped:?}");
    for line in &wrapped {
        assert_eq!(line.spans[0].text, "│ ");
        assert!(line.spans[0].style.dim);
        assert_eq!(line.spans[0].style.fg, None);
    }
    let cell_rows = prefixed(&quote, 16, &Prefix::hang(), true);
    assert!(cell_rows.len() >= 2, "{cell_rows:?}");
    for row in &cell_rows {
        assert_eq!(row.spans[1].text, "│ ");
        assert!(row.spans[1].style.dim);
        assert_eq!(row.spans[1].style.fg, None);
    }
}

#[test]
fn marks_headings_drop_italic_and_keep_no_colour_without_palette() {
    if !style::in_fresh_run(&path("marks_headings_drop_italic_and_keep_no_colour_without_palette")) {
        return;
    }
    style::init(&Preferences::default());
    let lines = super::markdown::render("# One\n\n## Two\n\n### Three\n\n#### Four");
    let headings: Vec<_> = lines.iter().filter(|line| !line.spans.is_empty()).collect();
    assert_eq!(headings.len(), 4, "{headings:?}");
    for (idx, line) in headings.iter().enumerate() {
        for span in &line.spans {
            assert!(span.style.bold, "heading {idx} should be bold: {span:?}");
            assert!(!span.style.italic, "heading {idx} should not be italic: {span:?}");
            assert_eq!(span.style.underline, idx == 0, "only H1 is underlined: {span:?}");
            assert_eq!(span.style.fg, None, "no palette means no heading colour: {span:?}");
        }
    }
    let palette = super::palette::TerminalPalette::new(
        (224, 224, 224),
        (28, 30, 38),
        super::palette::ColorLevel::TrueColor,
    );
    let h1 = super::markdown::heading_style_for_palette(
        pulldown_cmark::HeadingLevel::H1,
        Some(palette),
    );
    let h2 = super::markdown::heading_style_for_palette(
        pulldown_cmark::HeadingLevel::H2,
        Some(palette),
    );
    let h3 = super::markdown::heading_style_for_palette(
        pulldown_cmark::HeadingLevel::H3,
        Some(palette),
    );
    assert!(h1.bold && h1.underline && !h1.italic);
    assert_eq!(h1.fg, Some(super::palette::HEADING_ACCENT));
    assert!(h2.bold && !h2.underline && !h2.italic);
    assert_eq!(h2.fg, Some(super::palette::HEADING_ACCENT));
    assert!(h3.bold && !h3.underline && !h3.italic);
    assert_eq!(h3.fg, None);
}

#[test]
fn marks_ordered_marker_has_no_foreground_colour() {
    if !style::in_fresh_run(&path("marks_ordered_marker_has_no_foreground_colour")) {
        return;
    }
    style::init(&Preferences::default());
    let lines = super::markdown::render("1. 가");
    assert_eq!(lines.len(), 1, "{lines:?}");
    let marker = &lines[0].spans[0];
    assert_eq!(marker.text, "1. ");
    assert_eq!(marker.style.fg, None);
}

#[test]
fn marks_table_header_drops_syntect_colour_without_palette() {
    if !style::in_fresh_run(&path("marks_table_header_drops_syntect_colour_without_palette")) {
        return;
    }
    style::init(&Preferences::default());
    let lines = super::markdown::render("| a | b |\n| - | - |\n| 1 | 2 |");
    assert!(lines.len() >= 3, "{lines:?}");
    assert!(lines[0].style.bold, "header row should be bold: {:?}", lines[0].style);
    assert_eq!(lines[0].style.fg, None, "no palette means no header colour: {:?}", lines[0].style);
    for span in &lines[0].spans {
        assert_eq!(span.style.fg, None, "no palette means no header span colour: {span:?}");
    }
    let dark = super::palette::TerminalPalette::new(
        (224, 224, 224),
        (28, 30, 38),
        super::palette::ColorLevel::TrueColor,
    );
    let light = super::palette::TerminalPalette::new(
        (46, 52, 52),
        (255, 255, 255),
        super::palette::ColorLevel::TrueColor,
    );
    for palette in [dark, light] {
        let header = super::tables::header_style_for_palette(Some(palette));
        assert!(header.bold);
        assert_eq!(header.fg, Some(super::palette::HEADING_ACCENT));
        let sep = super::tables::separator_style_for_palette(Some(palette));
        let Some(super::ansi::Color::Rgb(r, g, b)) = sep.fg else {
            panic!("truecolor separator must emit RGB fg: {sep:?}");
        };
        let ratio = super::palette::contrast_ratio((r, g, b), palette.background());
        assert!(
            (3.0..4.5).contains(&ratio),
            "separator contrast {ratio:.2} out of [3.0, 4.5) for {:?}",
            palette.background()
        );
    }
}

#[test]
fn marks_quiet_fold_edit_counts_are_not_dimmed() {
    use runtime::message_stream::{DiffHunk, DiffLine, DiffLineKind, DiffView, ToolResultBody};
    if !style::in_fresh_run(&path("marks_quiet_fold_edit_counts_are_not_dimmed")) {
        return;
    }
    style::init(&Preferences::default());
    let view = DiffView {
        old_path: Some("src/lib.rs".to_string()),
        new_path: Some("src/lib.rs".to_string()),
        language: None,
        hunks: vec![DiffHunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: "old".to_string(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "new".to_string(),
                },
            ],
        }],
    };
    let mut group = ToolGroup::new(ToolCall::new(
        "a".to_string(),
        ToolKind::Edit {
            path: "src/lib.rs".to_string(),
        },
    ));
    assert!(group.complete(
        "a",
        Outcome::from_result(false, &ToolResultBody::Diff(view), "src/lib.rs"),
    ));
    let mut fold = super::turns::TurnFold::default();
    fold.take_steps(group.steps());
    let rows = fold.live_rows(100, 16);
    let plus = rows[0]
        .spans
        .iter()
        .find(|s| s.text == "+1")
        .expect("+1 span in quiet fold row");
    let minus = rows[0]
        .spans
        .iter()
        .find(|s| s.text == "-1")
        .expect("-1 span in quiet fold row");
    assert!(!plus.style.dim, "+1 must not be dimmed: {plus:?}");
    assert_eq!(plus.style.fg, Some(super::ansi::Color::GREEN));
    assert!(!minus.style.dim, "-1 must not be dimmed: {minus:?}");
    assert_eq!(minus.style.fg, Some(super::ansi::Color::RED));
}

#[test]
fn marks_shimmer_keeps_contrast_at_least_three_to_one() {
    if !style::in_fresh_run(&path("marks_shimmer_keeps_contrast_at_least_three_to_one")) {
        return;
    }
    style::init(&Preferences::default());
    let dark = super::palette::TerminalPalette::new(
        (224, 224, 224),
        (28, 30, 38),
        super::palette::ColorLevel::TrueColor,
    );
    let light = super::palette::TerminalPalette::new(
        (46, 52, 52),
        (255, 255, 255),
        super::palette::ColorLevel::TrueColor,
    );
    for palette in [dark, light] {
        for ms in (0..=2000).step_by(50) {
            let spans = super::shimmer::shimmer_spans_for_palette(
                "Working",
                Duration::from_millis(ms),
                Some(palette),
            );
            for span in spans {
                let Some(super::ansi::Color::Rgb(r, g, b)) = span.style.fg else {
                    panic!("truecolor shimmer must emit RGB fg: {span:?}");
                };
                let ratio = super::palette::contrast_ratio((r, g, b), palette.background());
                assert!(
                    ratio >= 3.0,
                    "contrast {ratio:.2} < 3.0 at {ms}ms for {:?}: {span:?}",
                    palette.background()
                );
            }
        }
    }
}

#[test]
fn marks_syntax_highlight_keeps_text_contrast_floor_on_light_palette() {
    use super::ansi::Color;
    use super::palette::{code_background, contrast_ratio, ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("marks_syntax_highlight_keeps_text_contrast_floor_on_light_palette")) {
        return;
    }
    style::init(&Preferences::default());
    // Catppuccin Latte's inks on a light palette: the theme of that name
    // draws code in them, fitted where they fall short. (`terminal` drew them
    // too until it drew code in the terminal's own slots, which the writer
    // fits against the measured ground instead.)
    let _ = super::theme::set(super::theme::find("catppuccin-latte").expect("catppuccin-latte"));
    let light = TerminalPalette::new((46, 52, 52), (255, 255, 255), ColorLevel::TrueColor);
    let Color::Rgb(bg_r, bg_g, bg_b) = code_background(light) else {
        panic!("code_background must return RGB");
    };
    let code = "fn main() {\n    let x = \"hi\"; // note\n    println!(\"{x}\");\n}\n";
    let lines = super::highlight::highlight_code_to_lines_for_palette(code, "rust", Some(light));
    assert!(
        lines.iter().flat_map(|line| &line.spans).any(|span| matches!(span.style.fg, Some(Color::Rgb(..)))),
        "the check below has RGB inks to check"
    );
    for line in &lines {
        for span in &line.spans {
            if let Some(Color::Rgb(r, g, b)) = span.style.fg {
                let ratio = contrast_ratio((r, g, b), (bg_r, bg_g, bg_b));
                assert!(
                    ratio >= 4.5,
                    "syntax span {span:?} has contrast {ratio:.2} < 4.5 on ({bg_r}, {bg_g}, {bg_b})"
                );
            }
        }
    }
}

#[test]
fn marks_footer_colours_keep_contrast_floor_on_light_and_keep_dark_values() {
    use super::ansi::Color;
    use super::palette::{self, contrast_ratio, ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("marks_footer_colours_keep_contrast_floor_on_light_and_keep_dark_values")) {
        return;
    }
    style::init(&Preferences::default());
    let light = TerminalPalette::new((46, 52, 52), (255, 255, 255), ColorLevel::TrueColor);
    let dark = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
    let model_scopes = &["entity.name.type", "support.type", "variable"];
    let cwd_scopes = &["string", "markup.underline.link"];

    let light_model = super::view::footer_theme_style_for_palette(
        model_scopes,
        palette::FOOTER_MODEL,
        Some(light),
    );
    let Some(Color::Rgb(r, g, b)) = light_model.fg else {
        panic!("light model must be RGB: {light_model:?}");
    };
    assert!(contrast_ratio((r, g, b), light.background()) >= 4.5);

    let light_cwd = super::view::footer_theme_style_for_palette(
        cwd_scopes,
        palette::FOOTER_CWD,
        Some(light),
    );
    let Some(Color::Rgb(r, g, b)) = light_cwd.fg else {
        panic!("light cwd must be RGB: {light_cwd:?}");
    };
    assert!(contrast_ratio((r, g, b), light.background()) >= 4.5);

    let dark_model = super::view::footer_theme_style_for_palette(
        model_scopes,
        palette::FOOTER_MODEL,
        Some(dark),
    );
    assert_eq!(dark_model.fg, Some(Color::Rgb(246, 226, 183)));

    let dark_cwd = super::view::footer_theme_style_for_palette(
        cwd_scopes,
        palette::FOOTER_CWD,
        Some(dark),
    );
    assert_eq!(dark_cwd.fg, Some(Color::Rgb(171, 223, 167)));
}

/// A sixteen-colour terminal that answered no colour question has no
/// palette, and there the writer draws a named theme as `terminal`. So every
/// place that asks the syntax theme for an ink at a depth it was handed — the
/// footer's model and folder, the headings, the table head, a code block —
/// must draw the same bytes as `terminal` there too.
#[test]
fn a_named_theme_on_a_sixteen_colour_terminal_that_answers_no_colour_draws_the_footer_as_terminal_does() {
    use super::ansi::{write_spans_for_palette_and_level, Span};
    use super::palette::{self, ColorLevel};
    use pulldown_cmark::HeadingLevel;

    if !style::in_fresh_run(&path(
        "a_named_theme_on_a_sixteen_colour_terminal_that_answers_no_colour_draws_the_footer_as_terminal_does",
    )) {
        return;
    }
    style::init(&Preferences::default());
    let depth = ColorLevel::Ansi16;
    let write = |line: &Line, out: &mut String| write_spans_for_palette_and_level(line, None, depth, out);
    let footer = || {
        let mut out = String::new();
        for (scopes, fallback) in [
            (&["entity.name.type", "support.type", "variable"][..], palette::FOOTER_MODEL),
            (&["string", "markup.underline.link"][..], palette::FOOTER_CWD),
        ] {
            let style = super::view::footer_theme_style_with_level(scopes, fallback, None, depth);
            write(&Line::new(vec![Span::new("footer", style)]), &mut out);
        }
        out
    };
    let the_rest = || {
        let mut out = String::new();
        for level in [HeadingLevel::H1, HeadingLevel::H2] {
            let style = super::markdown::heading_style_with_level(level, None, depth);
            write(&Line::new(vec![Span::new("heading", style)]), &mut out);
        }
        let head = super::tables::header_style_with_level(None, depth);
        write(&Line::new(vec![Span::new("head", head)]), &mut out);
        for line in super::highlight::highlight_code_to_lines_with_level(
            "fn main() { if true { let x = \"hi\"; } }\n",
            "rust",
            None,
            depth,
        ) {
            write(&line, &mut out);
        }
        out
    };

    let _ = super::theme::set(super::theme::Theme::terminal());
    let terminal_footer = footer();
    let terminal_rest = the_rest();
    assert!(terminal_footer.contains("38;2;"), "the footer asks the syntax theme: {terminal_footer:?}");

    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    assert_eq!(footer(), terminal_footer, "the footer's model and folder draw as `terminal` does");
    assert_eq!(the_rest(), terminal_rest, "headings, the table head and a code block draw as `terminal` does");
}

fn parse_fg_rgb(rendered: &str) -> (u8, u8, u8) {
    let (_, rest) = rendered
        .split_once("38;2;")
        .unwrap_or_else(|| panic!("expected 38;2; RGB SGR in {rendered:?}"));
    let (params, _) = rest.split_once('m').expect("SGR terminator");
    let mut parts = params.split(';');
    let r = parts.next().expect("red").parse().expect("u8");
    let g = parts.next().expect("green").parse().expect("u8");
    let b_raw = parts.next().expect("blue");
    let b = b_raw.split(';').next().unwrap_or(b_raw).parse().expect("u8");
    (r, g, b)
}

#[test]
fn terminal_theme_resolves_every_role_to_todays_colour() {
    use super::ansi::{write_spans_for_palette, Color, Role, Span, Style};
    use super::palette::{ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("terminal_theme_resolves_every_role_to_todays_colour")) {
        return;
    }
    style::init(&Preferences::default());
    let _ = super::theme::set(super::theme::Theme::terminal());

    let dark = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
    let light = TerminalPalette::new((46, 52, 52), (255, 255, 255), ColorLevel::TrueColor);

    let render = |role: Role, palette: Option<TerminalPalette>| {
        let mut out = String::new();
        write_spans_for_palette(
            &Line::new(vec![Span::new("x", Style::new().fg(Color::Role(role)))]),
            palette,
            &mut out,
        );
        out
    };

    let cases = [
        (Role::Code, "\u{1b}[34;49mx", "\u{1b}[34;49mx", "\u{1b}[34;49mx"),
        (Role::Link, "\u{1b}[36;49mx", "\u{1b}[36;49mx", "\u{1b}[36;49mx"),
        (
            Role::Heading,
            "\u{1b}[38;2;249;226;175;49mx",
            "\u{1b}[38;2;249;226;175;49mx",
            "\u{1b}[38;2;108;76;21;49mx",
        ),
        (
            Role::Caret,
            "\u{1b}[38;2;255;178;66;49mx",
            "\u{1b}[38;2;255;178;66;49mx",
            "\u{1b}[38;2;143;83;0;49mx",
        ),
        (
            Role::Model,
            "\u{1b}[38;2;246;226;183;49mx",
            "\u{1b}[38;2;246;226;183;49mx",
            "\u{1b}[38;2;106;78;17;49mx",
        ),
        (
            Role::Cwd,
            "\u{1b}[38;2;171;223;167;49mx",
            "\u{1b}[38;2;171;223;167;49mx",
            "\u{1b}[38;2;40;96;48;49mx",
        ),
        (
            Role::Warning,
            "\u{1b}[38;2;196;167;103;49mx",
            "\u{1b}[38;2;196;167;103;49mx",
            "\u{1b}[38;2;139;98;20;49mx",
        ),
    ];

    for (role, none_sgr, dark_sgr, light_sgr) in cases {
        assert_eq!(render(role, None), none_sgr, "{role:?} None");
        assert_eq!(render(role, Some(dark)), dark_sgr, "{role:?} dark");
        assert_eq!(render(role, Some(light)), light_sgr, "{role:?} light");
    }
}

#[test]
fn dracula_draws_roles_and_slots_in_theme_colours_on_a_dark_ground() {
    use super::ansi::{write_spans_for_palette, Color, Span, Style};
    use super::palette::{self, ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("dracula_draws_roles_and_slots_in_theme_colours_on_a_dark_ground")) {
        return;
    }
    style::init(&Preferences::default());
    let dracula = super::theme::find("dracula").expect("dracula theme");
    let _ = super::theme::set(dracula);

    let dark = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
    let render_line = |line: &Line| {
        let mut out = String::new();
        write_spans_for_palette(line, Some(dark), &mut out);
        out
    };
    let render_style = |style: Style| render_line(&Line::new(vec![Span::new("x", style)]));

    // Inline code = #50fa7b
    let code_lines = super::markdown::render("`code`");
    let code_sgr = render_line(&code_lines[0]);
    assert!(code_sgr.contains("38;2;80;250;123"), "inline code: {code_sgr:?}");

    // Link = #8be9fd + underline
    let link_lines = super::markdown::render("[site](https://example.com)");
    let link_sgr = render_line(&link_lines[0]);
    assert!(
        link_sgr.contains("\u{1b}[4m") && link_sgr.contains("38;2;139;233;253"),
        "link: {link_sgr:?}"
    );

    // H1, H2 and table header = #bd93f9
    for level in [
        pulldown_cmark::HeadingLevel::H1,
        pulldown_cmark::HeadingLevel::H2,
    ] {
        let heading = super::markdown::heading_style_for_palette(level, Some(dark));
        let heading_sgr = render_style(heading);
        assert!(
            heading_sgr.contains("38;2;189;147;249"),
            "heading {level:?}: {heading_sgr:?}"
        );
    }
    let header_sgr = render_style(super::tables::header_style_for_palette(Some(dark)));
    assert!(header_sgr.contains("38;2;189;147;249"), "table header: {header_sgr:?}");

    // Cyan slots -> accent (#bd93f9), bright cyan -> brightBlue (#d6acff)
    for cyan in [Color::CYAN, Color::Indexed(6)] {
        let sgr = render_style(Style::new().fg(cyan));
        assert!(sgr.contains("38;2;189;147;249"), "{cyan:?}: {sgr:?}");
    }
    for bright_cyan in [Color::BrightBase(6), Color::Indexed(14)] {
        let sgr = render_style(Style::new().fg(bright_cyan));
        assert!(sgr.contains("38;2;214;172;255"), "{bright_cyan:?}: {sgr:?}");
    }

    // Red, green, yellow, magenta slots
    for (slot, expected) in [
        (Color::RED, "38;2;255;85;85"),
        (Color::GREEN, "38;2;80;250;123"),
        (Color::YELLOW, "38;2;241;250;140"),
        (Color::MAGENTA, "38;2;255;121;198"),
    ] {
        let sgr = render_style(Style::new().fg(slot));
        assert!(sgr.contains(expected), "{slot:?}: {sgr:?}");
    }

    // Footer model = #bd93f9, cwd = #8be9fd, caret = #ff79c6, warning = #f1fa8c
    let model_style = super::view::footer_theme_style_for_palette(
        &["entity.name.type", "support.type", "variable"],
        palette::FOOTER_MODEL,
        Some(dark),
    );
    assert!(render_style(model_style).contains("38;2;189;147;249"));
    let cwd_style = super::view::footer_theme_style_for_palette(
        &["string", "markup.underline.link"],
        palette::FOOTER_CWD,
        Some(dark),
    );
    assert!(render_style(cwd_style).contains("38;2;139;233;253"));
    assert!(render_style(Style::new().fg(palette::COMPOSER_CARET)).contains("38;2;255;121;198"));
    assert!(render_style(Style::new().fg(palette::WARNING_NOTICE)).contains("38;2;241;250;140"));

    // Dimmed ANSI slot mixes 40% toward background, clears floor, and turns off SGR 2.
    let dim_red = render_style(Style::new().fg(Color::RED).dim());
    assert!(!dim_red.contains("\u{1b}[2m"), "dimmed slot must turn off SGR 2: {dim_red:?}");
    let dim_red_rgb = parse_fg_rgb(&dim_red);
    assert!(
        palette::contrast_ratio(dim_red_rgb, dark.background()) >= 4.5,
        "dimmed red {dim_red_rgb:?} below floor"
    );

    // Code block uses Dracula syntax theme (keyword `if` = #ff79c6)
    let syntax_lines = super::highlight::highlight_code_to_lines_for_palette(
        "fn main() { if true {} }\n",
        "rust",
        Some(dark),
    );
    let if_span = syntax_lines[0]
        .spans
        .iter()
        .find(|span| span.text == "if")
        .expect("if span");
    assert_eq!(if_span.style.fg, Some(Color::Rgb(255, 121, 198)));
}

#[test]
fn a_named_theme_fits_by_lightness_on_a_light_ground_and_keeps_roles_apart() {
    use super::ansi::{write_spans_for_palette, Color, Role, Span, Style};
    use super::palette::{contrast_ratio, ColorLevel, TerminalPalette};
    use super::theme::rgb_to_hsl;

    if !style::in_fresh_run(&path("a_named_theme_fits_by_lightness_on_a_light_ground_and_keeps_roles_apart")) {
        return;
    }
    style::init(&Preferences::default());
    let dracula = super::theme::find("dracula").expect("dracula theme");
    let _ = super::theme::set(dracula.clone());

    let bg = (255, 255, 255);
    let fg = (46, 52, 52);
    let light = TerminalPalette::new(fg, bg, ColorLevel::TrueColor);
    let floor = (contrast_ratio(fg, bg) * 0.75).min(4.5);

    let resolve_rgb = |color: Color| -> (u8, u8, u8) {
        let mut out = String::new();
        write_spans_for_palette(&Line::new(vec![Span::new("x", Style::new().fg(color))]), Some(light), &mut out);
        parse_fg_rgb(&out)
    };

    let hue_diff = |a: f64, b: f64| {
        let d = (a - b).abs() % 360.0;
        d.min(360.0 - d)
    };

    let role_colors = [
        (Role::Code, dracula.role_color(Role::Code).unwrap()),
        (Role::Link, dracula.role_color(Role::Link).unwrap()),
        (Role::Heading, dracula.role_color(Role::Heading).unwrap()),
        (Role::Caret, dracula.role_color(Role::Caret).unwrap()),
        (Role::Model, dracula.role_color(Role::Model).unwrap()),
        (Role::Cwd, dracula.role_color(Role::Cwd).unwrap()),
        (Role::Warning, dracula.role_color(Role::Warning).unwrap()),
    ];

    let mut fitted_roles = Vec::new();
    for (role, orig) in role_colors {
        let fitted = resolve_rgb(Color::Role(role));
        assert!(
            contrast_ratio(fitted, bg) >= floor,
            "{role:?} {fitted:?} below floor {floor:.2}"
        );
        let (h0, s0, _) = rgb_to_hsl(orig);
        let (h1, s1, _) = rgb_to_hsl(fitted);
        assert!(hue_diff(h0, h1) <= 3.0, "{role:?} hue drift: {h0:.2} -> {h1:.2}");
        assert!((s0 - s1).abs() <= 0.05, "{role:?} sat drift: {s0:.3} -> {s1:.3}");
        fitted_roles.push((orig, fitted));
    }

    // Distinct role colours stay > 25° apart in hue.
    for i in 0..fitted_roles.len() {
        for j in (i + 1)..fitted_roles.len() {
            if fitted_roles[i].0 == fitted_roles[j].0 {
                continue;
            }
            let h_a = rgb_to_hsl(fitted_roles[i].1).0;
            let h_b = rgb_to_hsl(fitted_roles[j].1).0;
            assert!(
                hue_diff(h_a, h_b) > 25.0,
                "roles {i} and {j} too close in hue: {h_a:.2} vs {h_b:.2}"
            );
        }
    }

    for slot in [Color::RED, Color::GREEN, Color::YELLOW, Color::MAGENTA, Color::CYAN] {
        let fitted = resolve_rgb(slot);
        assert!(
            contrast_ratio(fitted, bg) >= floor,
            "{slot:?} {fitted:?} below floor {floor:.2}"
        );
    }
}

#[test]
fn a_named_theme_is_off_in_the_codex_look_and_on_sixteen_colour_terminals() {
    use super::ansi::{write_spans_for_palette, Color, Role, Span, Style};
    use super::palette::{self, ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("a_named_theme_is_off_in_the_codex_look_and_on_sixteen_colour_terminals")) {
        return;
    }

    let dark = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
    let ansi16 = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::Ansi16);

    let snapshot = |pal: Option<TerminalPalette>| {
        let mut out = String::new();
        for color in [
            Color::Role(Role::Code),
            Color::Role(Role::Link),
            Color::Role(Role::Heading),
            Color::Role(Role::Caret),
            Color::Role(Role::Model),
            Color::Role(Role::Cwd),
            Color::Role(Role::Warning),
            Color::CYAN,
            Color::Indexed(6),
            Color::RED,
            Color::GREEN,
            Color::YELLOW,
            Color::MAGENTA,
        ] {
            write_spans_for_palette(
                &Line::new(vec![Span::new("x", Style::new().fg(color))]),
                pal,
                &mut out,
            );
        }
        let footer_model = super::view::footer_theme_style_for_palette(
            &["entity.name.type", "support.type", "variable"],
            palette::FOOTER_MODEL,
            pal,
        );
        write_spans_for_palette(&Line::new(vec![Span::new("m", footer_model)]), pal, &mut out);
        for line in super::highlight::highlight_code_to_lines_for_palette("fn main() {}\n", "rust", pal) {
            write_spans_for_palette(&line, pal, &mut out);
        }
        out
    };

    // Before style::init, style::current().style is TuiStyle::Codex.
    assert_eq!(style::current().style, style::TuiStyle::Codex);
    let _ = super::theme::set(super::theme::Theme::terminal());
    let codex_terminal = snapshot(Some(dark));
    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    let codex_dracula = snapshot(Some(dark));
    assert_eq!(codex_dracula, codex_terminal, "codex look ignores named theme");

    // Now fix marks look and check 16-colour terminals.
    style::init(&Preferences::default());
    let _ = super::theme::set(super::theme::Theme::terminal());
    let ansi16_terminal = snapshot(Some(ansi16));
    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    let ansi16_dracula = snapshot(Some(ansi16));
    assert_eq!(ansi16_dracula, ansi16_terminal, "16-colour terminal ignores named theme");
}

#[test]
fn switching_themes_twice_leaves_no_stale_colour() {
    use std::fmt::Write as _;

    use super::ansi::{write_spans_for_palette, Color, Role, Span, Style};
    use super::palette::{self, ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("switching_themes_twice_leaves_no_stale_colour")) {
        return;
    }
    style::init(&Preferences::default());
    let dark = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);

    let capture = || {
        let mut writer_out = String::new();
        for color in [
            Color::CYAN,
            Color::RED,
            Color::Role(Role::Code),
            Color::Role(Role::Model),
            Color::Role(Role::Warning),
        ] {
            write_spans_for_palette(
                &Line::new(vec![Span::new("x", Style::new().fg(color))]),
                Some(dark),
                &mut writer_out,
            );
        }
        let footer_model = super::view::footer_theme_style_for_palette(
            &["entity.name.type", "support.type", "variable"],
            palette::FOOTER_MODEL,
            Some(dark),
        );
        write_spans_for_palette(&Line::new(vec![Span::new("m", footer_model)]), Some(dark), &mut writer_out);

        let mut highlight_out = String::new();
        for line in super::highlight::highlight_code_to_lines_for_palette(
            "fn main() { if true {} }\n",
            "rust",
            Some(dark),
        ) {
            write_spans_for_palette(&line, Some(dark), &mut highlight_out);
        }
        (writer_out, highlight_out)
    };

    let expected_writer = |slots: [(u8, u8, u8); 5], footer: (u8, u8, u8)| {
        let mut expected = String::new();
        for (red, green, blue) in slots {
            let _ = write!(expected, "\u{1b}[38;2;{red};{green};{blue};49mx");
        }
        let (red, green, blue) = footer;
        let _ = write!(expected, "\u{1b}[38;2;{red};{green};{blue};49mm");
        expected
    };

    let _ = super::theme::set(super::theme::Theme::terminal());
    let (term_writer_first, term_hl_first) = capture();

    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    let (dracula_writer_first, dracula_hl_first) = capture();
    assert_eq!(
        dracula_writer_first,
        expected_writer(
            [(189, 147, 249), (255, 85, 85), (80, 250, 123), (189, 147, 249), (241, 250, 140)],
            (189, 147, 249),
        ),
        "dracula writer bytes"
    );
    assert_ne!(dracula_hl_first, term_hl_first);
    assert!(dracula_hl_first.contains("38;2;255;121;198"), "dracula keyword pink #ff79c6: {dracula_hl_first:?}");

    let _ = super::theme::set(super::theme::find("nord").expect("nord"));
    let (nord_writer, nord_hl) = capture();
    // Nord's #bf616a (191, 97, 106) has 4.06:1 contrast against #1c1e26,
    // so fit_lightness lifts it 3%p to (195, 108, 116) to clear the 4.5 floor.
    assert_eq!(
        nord_writer,
        expected_writer(
            [(129, 161, 193), (195, 108, 116), (143, 188, 187), (129, 161, 193), (235, 203, 139)],
            (129, 161, 193),
        ),
        "nord writer bytes"
    );
    assert_ne!(nord_hl, dracula_hl_first);
    assert_ne!(nord_hl, term_hl_first);
    assert!(nord_hl.contains("38;2;129;161;193"), "nord accent #81a1c1: {nord_hl:?}");

    let _ = super::theme::set(super::theme::Theme::terminal());
    let (term_writer_second, term_hl_second) = capture();
    assert_eq!(term_writer_second, term_writer_first, "returning to terminal leaves no stale writer colour");
    assert_eq!(term_hl_second, term_hl_first, "returning to terminal leaves no stale highlight colour");

    // A second round (terminal -> dracula -> nord -> dracula) bumps theme
    // revisions further and checks that nord leaves no stale colour behind.
    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    let _ = capture();
    let _ = super::theme::set(super::theme::find("nord").expect("nord"));
    let _ = capture();
    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    let (dracula_writer_second, dracula_hl_second) = capture();
    assert_eq!(dracula_writer_second, dracula_writer_first, "dracula -> nord -> dracula writer bytes");
    assert_eq!(dracula_hl_second, dracula_hl_first, "dracula -> nord -> dracula highlight bytes");
}

#[test]
fn syntax_theme_follows_the_named_theme() {
    use super::ansi::Color;
    use super::palette::{ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("syntax_theme_follows_the_named_theme")) {
        return;
    }
    style::init(&Preferences::default());
    let dark = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);

    let keyword_color = || {
        let lines = super::highlight::highlight_code_to_lines_for_palette(
            "fn main() { if true {} }\n",
            "rust",
            Some(dark),
        );
        lines[0]
            .spans
            .iter()
            .find(|span| span.text == "if")
            .and_then(|span| span.style.fg)
    };

    // `terminal` draws code in the terminal's own sixteen colours: every ink a
    // slot, which the terminal paints from its own palette, so changing the
    // window's theme changes the code too (the person, 2026-11-18: 「창 테마를
    // 기준으로」). It drew Catppuccin Mocha's, which no theme change reached.
    let _ = super::theme::set(super::theme::Theme::terminal());
    let inks: Vec<Color> = super::highlight::highlight_code_to_lines_for_palette(
        "fn main() { if true {} }\n",
        "rust",
        Some(dark),
    )[0]
        .spans
        .iter()
        .filter_map(|span| span.style.fg)
        .collect();
    assert!(
        !inks.is_empty() && inks.iter().all(|ink| matches!(ink, Color::Base(_) | Color::Indexed(0..=15))),
        "terminal draws code in the terminal's colour slots: {inks:?}"
    );

    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    assert_eq!(keyword_color(), Some(Color::Rgb(255, 121, 198)));
}

/// A named theme with no syntax theme of its own — seventeen of the thirty,
/// Ayu Dark among them — draws code in its own sixteen colours: every ink of
/// a highlighted line is a colour slot, which the writer draws in the theme's
/// colour for it, so two such themes no longer draw code alike. They drew it
/// all in Catppuccin's (the person, 2026-11-18: 「theme 색상 글씨가 바꿔도
/// 다똑같음」).
#[test]
fn a_theme_without_a_syntax_theme_of_its_own_draws_code_in_its_own_colours() {
    use super::ansi::Color;
    use super::palette::{ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("a_theme_without_a_syntax_theme_of_its_own_draws_code_in_its_own_colours")) {
        return;
    }
    style::init(&Preferences::default());
    let dark = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
    let inks = || -> Vec<Color> {
        super::highlight::highlight_code_to_lines_for_palette(
            "fn main() { let name = \"zo\"; if name.len() > 1 { println!(\"{name}\"); } }\n",
            "rust",
            Some(dark),
        )[0]
            .spans
            .iter()
            .filter_map(|span| span.style.fg)
            .collect()
    };
    let ayu = super::theme::find("ayu-dark").expect("ayu-dark");
    let tokyo = super::theme::find("tokyo-night").expect("tokyo-night");
    let _ = super::theme::set(ayu.clone());
    let slots: Vec<u8> = inks()
        .into_iter()
        .map(|ink| match ink {
            Color::Base(slot) | Color::Indexed(slot @ 0..=15) => slot,
            other => panic!("ayu-dark draws code in its colour slots, not {other:?}"),
        })
        .collect();
    let coloured: Vec<u8> = slots.iter().copied().filter(|slot| ayu.ansi_slot(*slot).is_some()).collect();
    assert!(!coloured.is_empty(), "some of the code is in slots the theme colours: {slots:?}");
    assert!(
        coloured.iter().any(|slot| ayu.ansi_slot(*slot) != tokyo.ansi_slot(*slot)),
        "ayu-dark and tokyo-night draw that code apart: {coloured:?}"
    );
}

#[test]
fn the_composer_placeholder_keeps_sgr_dim_under_a_named_theme() {
    use super::ansi::write_spans_for_palette;
    use super::palette::{ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("the_composer_placeholder_keeps_sgr_dim_under_a_named_theme")) {
        return;
    }
    style::init(&Preferences::default());
    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    let horizon = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
    let (rows, _cursor) = super::composer::Composer::new().render(WIDTH, "Ask zo to do anything");
    let rendered: Vec<String> = rows
        .iter()
        .map(|row| {
            let mut out = String::new();
            write_spans_for_palette(row, Some(horizon), &mut out);
            out
        })
        .collect();
    assert!(
        rendered.iter().any(|row| row.contains("\u{1b}[2mAsk zo to do anything")),
        "{rendered:?}"
    );
    let placeholder_row = rendered
        .iter()
        .find(|row| row.contains("Ask zo to do anything"))
        .expect("placeholder row");
    let (_, after_caret) = placeholder_row.split_once('›').expect("composer caret");
    let (before_placeholder, _) = after_caret
        .split_once("Ask zo to do anything")
        .expect("placeholder text");
    assert!(
        before_placeholder.contains("\u{1b}[2m") && !before_placeholder.contains("38;2;"),
        "placeholder must keep SGR 2 without RGB fg: {before_placeholder:?}"
    );
}

#[test]
fn named_theme_lightness_fit_is_cached_per_revision_and_pair() {
    use super::ansi::{write_spans_for_palette, Color, Role, Span, Style};
    use super::palette::{contrast_computes, ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("named_theme_lightness_fit_is_cached_per_revision_and_pair")) {
        return;
    }
    style::init(&Preferences::default());
    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    let light = TerminalPalette::new((46, 52, 52), (255, 255, 255), ColorLevel::TrueColor);
    let line = Line::new(vec![Span::new("x", Style::new().fg(Color::Role(Role::Code)))]);

    let before = contrast_computes();
    let mut first = String::new();
    write_spans_for_palette(&line, Some(light), &mut first);
    let after_first = contrast_computes();
    assert_eq!(after_first - before, 1, "first resolve computes once");

    let mut second = String::new();
    write_spans_for_palette(&line, Some(light), &mut second);
    assert_eq!(second, first);
    assert_eq!(
        contrast_computes(),
        after_first,
        "second resolve for same (revision, palette, colour, background) hits cache"
    );

    let _ = super::theme::set(super::theme::find("nord").expect("nord"));
    let mut third = String::new();
    write_spans_for_palette(&line, Some(light), &mut third);
    assert_eq!(
        contrast_computes(),
        after_first + 1,
        "theme revision change recomputes contrast fit"
    );
}

/// On a light ground `terminal` and `catppuccin-latte` both take Catppuccin
/// Latte as their syntax theme, so the same syntax ink comes in under both —
/// and `terminal` blends it toward the body ink while a named theme moves only
/// its lightness. The contrast cache must keep the theme's revision in its
/// key, or one theme gets the other's answer.
#[test]
fn the_same_syntax_ink_fits_by_blend_under_terminal_and_by_lightness_under_catppuccin_latte() {
    use super::ansi::Color;
    use super::palette::{code_background_rgb, ensure_text_contrast_cached, ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path(
        "the_same_syntax_ink_fits_by_blend_under_terminal_and_by_lightness_under_catppuccin_latte",
    )) {
        return;
    }
    style::init(&Preferences::default());
    let light = TerminalPalette::new((46, 52, 52), (255, 255, 255), ColorLevel::TrueColor);
    let terminal = super::theme::Theme::terminal();
    let latte = super::theme::find("catppuccin-latte").expect("catppuccin-latte");
    let type_ink = |theme: &super::theme::Theme| {
        let _ = super::theme::set(theme.clone());
        super::highlight::foreground_style_for_scopes_with_level(
            &["entity.name.type"],
            Some(light),
            ColorLevel::TrueColor,
        )
        .and_then(|style| style.fg)
    };
    let ink = match (type_ink(&terminal), type_ink(&latte)) {
        (Some(Color::Rgb(red, green, blue)), named) => {
            assert_eq!(named, Some(Color::Rgb(red, green, blue)), "both themes hand in the same Latte ink");
            (red, green, blue)
        }
        other => panic!("a type's ink is RGB under both themes: {other:?}"),
    };
    let ground = code_background_rgb(light);
    let blended = light.ensure_text_contrast(ink, ground);
    let lightness_fit = super::theme::fit_lightness(ink, light, ground);
    let values = format!("ink {ink:?} on {ground:?}: blend {blended:?}, lightness {lightness_fit:?}");
    assert_ne!(blended, lightness_fit, "the two fits must differ for this ink — {values}");

    let _ = super::theme::set(terminal.clone());
    let first = ensure_text_contrast_cached(light, ink, ground);
    assert_eq!(first, blended, "terminal blends — {values}");
    let _ = super::theme::set(latte);
    assert_eq!(
        ensure_text_contrast_cached(light, ink, ground),
        lightness_fit,
        "catppuccin-latte moves the lightness, not terminal's cached blend — {values}"
    );
    let _ = super::theme::set(terminal);
    assert_eq!(ensure_text_contrast_cached(light, ink, ground), first, "back on terminal — {values}");

    // The values the report quotes: Latte's yellow (#df8e1d) on the code band.
    assert_eq!(
        (ink, ground, blended, lightness_fit),
        ((223, 142, 29), (247, 247, 247), (143, 101, 39), (155, 99, 20)),
        "{values}"
    );
}

/// Switching themes back and forth keeps one revision's worth of cached
/// colours: the contrast cache, the ANSI-16 ink cache and the syntax scope
/// cache drop the entries an earlier theme left once a new theme asks.
#[test]
fn switching_themes_back_and_forth_keeps_one_revision_of_cached_colours() {
    use super::ansi::{write_spans_for_palette, Color, Role, Span, Style};
    use super::palette::{cached_colour_entries, ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("switching_themes_back_and_forth_keeps_one_revision_of_cached_colours")) {
        return;
    }
    style::init(&Preferences::default());
    let light = TerminalPalette::new((46, 52, 52), (255, 255, 255), ColorLevel::TrueColor);
    let draw = || {
        let mut out = String::new();
        for color in [
            Color::Role(Role::Code),
            Color::Role(Role::Link),
            Color::Role(Role::Heading),
            Color::Role(Role::Caret),
            Color::Role(Role::Model),
            Color::Role(Role::Cwd),
            Color::Role(Role::Warning),
            Color::Base(0),
            Color::Base(7),
        ] {
            write_spans_for_palette(&Line::new(vec![Span::new("x", Style::new().fg(color))]), Some(light), &mut out);
        }
        for scopes in [&["entity.name.type", "support.type", "variable"][..], &["string", "markup.underline.link"][..]] {
            let _ = super::highlight::foreground_style_for_scopes_with_level(scopes, Some(light), ColorLevel::TrueColor);
        }
        let (contrast, inks) = cached_colour_entries();
        (contrast, inks, super::highlight::cached_scope_answers())
    };
    let dracula = super::theme::find("dracula").expect("dracula");
    let nord = super::theme::find("nord").expect("nord");

    let _ = super::theme::set(dracula.clone());
    let one_dracula = draw();
    let _ = super::theme::set(nord.clone());
    let one_nord = draw();
    assert!(
        one_dracula.0 > 0 && one_dracula.1 > 0 && one_dracula.2 > 0,
        "each cache is asked: {one_dracula:?}"
    );
    for round in 0..3 {
        let _ = super::theme::set(dracula.clone());
        assert_eq!(draw(), one_dracula, "dracula again, round {round}: (contrast, ANSI-16 inks, scopes)");
        let _ = super::theme::set(nord.clone());
        assert_eq!(draw(), one_nord, "nord again, round {round}: (contrast, ANSI-16 inks, scopes)");
    }
}

#[test]
fn an_empty_named_theme_resolves_roles_like_terminal_on_a_light_ground() {
    use super::ansi::{write_spans_for_palette, Color, Role, Span, Style};
    use super::palette::{ColorLevel, TerminalPalette};

    if !style::in_fresh_run(&path("an_empty_named_theme_resolves_roles_like_terminal_on_a_light_ground")) {
        return;
    }
    style::init(&Preferences::default());
    let light = TerminalPalette::new((46, 52, 52), (255, 255, 255), ColorLevel::TrueColor);

    let render_all = || {
        [
            Role::Code,
            Role::Link,
            Role::Heading,
            Role::Caret,
            Role::Model,
            Role::Cwd,
            Role::Warning,
        ]
        .map(|role| {
            let mut out = String::new();
            write_spans_for_palette(
                &Line::new(vec![Span::new("x", Style::new().fg(Color::Role(role)))]),
                Some(light),
                &mut out,
            );
            out
        })
    };

    let _ = super::theme::set(super::theme::Theme::terminal());
    let expected = render_all();

    let _ = super::theme::set(super::theme::Theme::empty("sparse", "Sparse"));
    let actual = render_all();
    assert_eq!(actual, expected);
}


/// Only the ground's lightness known — the person said it, or `COLORFGBG`,
/// and the terminal did not answer: a named theme's roles and colour slots
/// move their lightness until they keep 4.5:1 on the stand-in ground of that
/// lightness, hue and saturation kept, and a colour that keeps it already
/// stays. A dim span stays SGR 2 over the same colour (no ground to blend it
/// toward), 256 colours take the fitted colour's nearest index as a measured
/// ground does, and a span on a colour of its own is fitted to that colour.
#[test]
fn with_only_the_lightness_known_a_named_theme_clears_four_point_five_on_the_stand_in_ground() {
    use super::ansi::{write_spans_for_ground_and_level, Color, Role, Span, Style};
    use super::ground::{stand_in, Ground};
    use super::palette::{contrast_ratio, nearest_xterm_index, ColorLevel};
    use super::theme::rgb_to_hsl;

    if !style::in_fresh_run(&path(
        "with_only_the_lightness_known_a_named_theme_clears_four_point_five_on_the_stand_in_ground",
    )) {
        return;
    }
    style::init(&Preferences::default());
    let dracula = super::theme::find("dracula").expect("dracula theme");
    let _ = super::theme::set(dracula.clone());
    let draw = |style: Style, ground: Ground, level: ColorLevel| {
        let mut out = String::new();
        write_spans_for_ground_and_level(&Line::new(vec![Span::new("x", style)]), ground, level, &mut out);
        out
    };
    let hue_apart = |a: f64, b: f64| {
        let apart = (a - b).abs() % 360.0;
        apart.min(360.0 - apart)
    };
    let mut colours: Vec<(Color, (u8, u8, u8))> =
        [Role::Code, Role::Link, Role::Heading, Role::Caret, Role::Model, Role::Cwd, Role::Warning]
            .into_iter()
            .map(|role| (Color::Role(role), dracula.role_color(role).expect("Dracula colours every role")))
            .collect();
    for (color, slot) in [
        (Color::RED, 1),
        (Color::GREEN, 2),
        (Color::YELLOW, 3),
        (Color::MAGENTA, 5),
        (Color::CYAN, 6),
        (Color::BrightBase(1), 9),
        (Color::LIGHT_BLUE, 12),
        (Color::Indexed(13), 13),
    ] {
        colours.push((color, dracula.ansi_slot(slot).expect("Dracula colours the slot")));
    }

    for light in [true, false] {
        let ground = Ground::Lightness { light };
        let behind = stand_in(light);
        let mut moved = 0;
        for (color, theirs) in &colours {
            let drawn = draw(Style::new().fg(*color), ground, ColorLevel::TrueColor);
            let fitted = parse_fg_rgb(&drawn);
            assert!(
                contrast_ratio(fitted, behind) >= 4.5,
                "{color:?} drawn {fitted:?} on {behind:?}: {drawn:?}"
            );
            if contrast_ratio(*theirs, behind) >= 4.5 {
                assert_eq!(fitted, *theirs, "{color:?} keeps 4.5:1 already and stays");
                continue;
            }
            moved += 1;
            let ((hue, saturation, _), (fitted_hue, fitted_saturation, _)) =
                (rgb_to_hsl(*theirs), rgb_to_hsl(fitted));
            assert!(hue_apart(hue, fitted_hue) <= 3.0, "{color:?} hue {hue:.1} -> {fitted_hue:.1}");
            assert!(
                (saturation - fitted_saturation).abs() <= 0.05,
                "{color:?} saturation {saturation:.3} -> {fitted_saturation:.3}"
            );
            let dim = draw(Style::new().fg(*color).dim(), ground, ColorLevel::TrueColor);
            assert!(dim.contains("\u{1b}[2m") && parse_fg_rgb(&dim) == fitted, "{color:?} dim: {dim:?}");
            let indexed = draw(Style::new().fg(*color), ground, ColorLevel::Ansi256);
            assert_eq!(indexed, format!("\u{1b}[38;5;{};49mx", nearest_xterm_index(fitted)), "{color:?}");
        }
        assert!(moved > 0, "a Dracula colour moves on the stand-in for light={light}");
    }

    // A span on a colour of its own keeps the floor on that colour, not on
    // the stand-in: Dracula's red on white, the ground said to be dark.
    let on_white = draw(
        Style::new().fg(Color::RED).bg(Color::Rgb(255, 255, 255)),
        Ground::Lightness { light: false },
        ColorLevel::TrueColor,
    );
    let red = parse_fg_rgb(&on_white);
    assert!(
        contrast_ratio(red, (255, 255, 255)) >= 4.5 && on_white.contains(";48;2;255;255;255m"),
        "{on_white:?}"
    );
}

/// Only the ground's lightness known, no palette is made up from it: the
/// person's words, a code block and the composer stand on no band, a dim
/// span stays SGR 2, and the `terminal` theme's colours are never lowered to
/// the depth — at 256 colours its roles are still RGB (today's bytes on a
/// ground said to be dark, their light-ground pairs on one said to be
/// light), and its colour slots keep their slot bytes.
#[test]
fn with_only_the_lightness_known_no_band_is_drawn_and_no_depth_is_lowered() {
    use super::ansi::{line_lead, write_spans, write_spans_for_ground_and_level, Color, Role, Span, Style};
    use super::ground::{self, Ground, Hint, TerminalBackground};
    use super::palette::{self, ColorLevel};

    if !style::in_fresh_run_with(
        &path("with_only_the_lightness_known_no_band_is_drawn_and_no_depth_is_lowered"),
        &[("COLORTERM", "truecolor")],
    ) {
        return;
    }
    style::init(&Preferences::default());
    ground::set(Hint { word: TerminalBackground::Light, colorfgbg: None });
    assert_eq!(ground::current(), Ground::Lightness { light: true });
    assert_eq!(palette::terminal_palette(), None, "a lightness is never a palette");

    let mut code = Line::from_text("fn main() {}");
    code.code = true;
    let mut rows = super::cells::user_cell("fix the parser", WIDTH);
    rows.extend(prefixed(&[Line::from_text("Run this:"), code], WIDTH, &Prefix::hang(), true));
    let (composer, _cursor) = super::composer::Composer::new().render(WIDTH, "Ask zo to do anything");
    rows.extend(composer);
    for row in &rows {
        let mut out = line_lead(row);
        write_spans(row, &mut out);
        assert!(!out.contains("48;"), "a band under {:?}: {out:?}", row.plain());
    }

    let draw = |style: Style, ground: Ground, level: ColorLevel| {
        let mut out = String::new();
        write_spans_for_ground_and_level(&Line::new(vec![Span::new("x", style)]), ground, level, &mut out);
        out
    };
    for level in [ColorLevel::TrueColor, ColorLevel::Ansi256] {
        for (role, dark, light) in [
            (Role::Code, "\u{1b}[34;49mx", "\u{1b}[34;49mx"),
            (Role::Link, "\u{1b}[36;49mx", "\u{1b}[36;49mx"),
            (Role::Heading, "\u{1b}[38;2;249;226;175;49mx", "\u{1b}[38;2;108;76;21;49mx"),
            (Role::Caret, "\u{1b}[38;2;255;178;66;49mx", "\u{1b}[38;2;143;83;0;49mx"),
            (Role::Model, "\u{1b}[38;2;246;226;183;49mx", "\u{1b}[38;2;106;78;17;49mx"),
            (Role::Cwd, "\u{1b}[38;2;171;223;167;49mx", "\u{1b}[38;2;40;96;48;49mx"),
            (Role::Warning, "\u{1b}[38;2;196;167;103;49mx", "\u{1b}[38;2;139;98;20;49mx"),
        ] {
            let style = Style::new().fg(Color::Role(role));
            assert_eq!(draw(style, Ground::Lightness { light: false }, level), dark, "{role:?} {level:?}");
            assert_eq!(draw(style, Ground::Lightness { light: true }, level), light, "{role:?} {level:?}");
        }
        for light in [true, false] {
            let ground = Ground::Lightness { light };
            assert_eq!(draw(Style::new().fg(Color::CYAN), ground, level), "\u{1b}[36;49mx");
            assert_eq!(draw(Style::new().fg(Color::Indexed(200)), ground, level), "\u{1b}[38;5;200;49mx");
            assert_eq!(draw(Style::new().dim(), ground, level), "\u{1b}[2mx", "{level:?} light={light}");
        }
    }
}

/// `auto` on a terminal that did not answer, with no `COLORFGBG`: what was
/// drawn before, byte for byte — the `terminal` theme's roles, colour slots
/// and dim text, the footer's model and folder, and Dracula's roles and slots
/// in Dracula's own colours. A code block is no longer codex's Catppuccin
/// Mocha (`codex-tui-v0.149.1-codeblock.bin`) here: the marks look draws it
/// in the terminal's own sixteen colours, so the window's theme reaches it
/// (the person, 2026-11-18: 「창 테마를 기준으로」).
#[test]
fn auto_with_no_answer_writes_todays_bytes() {
    use super::ansi::{write_spans, Color, Role, Span, Style};
    use super::ground::{self, Ground, Hint};
    use super::palette::{self, ColorLevel};

    if !style::in_fresh_run_with(&path("auto_with_no_answer_writes_todays_bytes"), &[("COLORTERM", "truecolor")]) {
        return;
    }
    style::init(&Preferences::default());
    ground::set(Hint::default());
    assert_eq!(ground::current(), Ground::Unknown);
    let draw = |style: Style| {
        let mut out = String::new();
        write_spans(&Line::new(vec![Span::new("x", style)]), &mut out);
        out
    };
    let roles = [Role::Code, Role::Link, Role::Heading, Role::Caret, Role::Model, Role::Cwd, Role::Warning];
    let slots = [Color::RED, Color::GREEN, Color::YELLOW, Color::MAGENTA, Color::CYAN];
    let every = || -> Vec<String> {
        roles
            .iter()
            .map(|role| draw(Style::new().fg(Color::Role(*role))))
            .chain(slots.iter().map(|slot| draw(Style::new().fg(*slot))))
            .chain([draw(Style::new().dim())])
            .collect()
    };

    let _ = super::theme::set(super::theme::Theme::terminal());
    assert_eq!(
        every(),
        [
            "\u{1b}[34;49mx",
            "\u{1b}[36;49mx",
            "\u{1b}[38;2;249;226;175;49mx",
            "\u{1b}[38;2;255;178;66;49mx",
            "\u{1b}[38;2;246;226;183;49mx",
            "\u{1b}[38;2;171;223;167;49mx",
            "\u{1b}[38;2;196;167;103;49mx",
            "\u{1b}[31;49mx",
            "\u{1b}[32;49mx",
            "\u{1b}[33;49mx",
            "\u{1b}[35;49mx",
            "\u{1b}[36;49mx",
            "\u{1b}[2mx",
        ]
    );
    let code: Vec<String> = super::highlight::highlight_code_to_lines("fn main() {\n    let name = \"zo\";\n", "rust")
        .iter()
        .map(|line| {
            let mut out = String::new();
            write_spans(line, &mut out);
            out
        })
        .collect();
    // Every colour the code is written in is one of the terminal's sixteen —
    // no truecolour ink of zo's own — and there is more than one: it is still
    // highlighted.
    let sgrs: Vec<&str> = code
        .iter()
        .flat_map(|line| line.split("\u{1b}[").skip(1).filter_map(|rest| rest.split_once('m').map(|(sgr, _)| sgr)))
        .collect();
    assert!(sgrs.iter().all(|sgr| !sgr.contains("38;2;")), "a code block in truecolour: {code:?}");
    assert!(
        sgrs.iter().collect::<std::collections::BTreeSet<_>>().len() > 1,
        "a code block in one colour: {code:?}"
    );
    assert_eq!(code.len(), 2, "two lines of code: {code:?}");
    let footer = |scopes: &[&str], fallback| {
        draw(super::view::footer_theme_style_on(scopes, fallback, ground::current(), ColorLevel::TrueColor))
    };
    assert_eq!(
        footer(&["entity.name.type", "support.type", "variable"], palette::FOOTER_MODEL),
        "\u{1b}[38;2;246;226;183;49mx"
    );
    assert_eq!(footer(&["string", "markup.underline.link"], palette::FOOTER_CWD), "\u{1b}[38;2;171;223;167;49mx");

    let _ = super::theme::set(super::theme::find("dracula").expect("dracula"));
    assert_eq!(
        every(),
        [
            "\u{1b}[38;2;80;250;123;49mx",
            "\u{1b}[38;2;139;233;253;49mx",
            "\u{1b}[38;2;189;147;249;49mx",
            "\u{1b}[38;2;255;121;198;49mx",
            "\u{1b}[38;2;189;147;249;49mx",
            "\u{1b}[38;2;139;233;253;49mx",
            "\u{1b}[38;2;241;250;140;49mx",
            "\u{1b}[38;2;255;85;85;49mx",
            "\u{1b}[38;2;80;250;123;49mx",
            "\u{1b}[38;2;241;250;140;49mx",
            "\u{1b}[38;2;255;121;198;49mx",
            "\u{1b}[38;2;189;147;249;49mx",
            "\u{1b}[2mx",
        ]
    );
}

/// The person says the ground is light and the terminal does not answer:
/// under `terminal` the roles take their light-ground colours, the footer's
/// model and folder too, and code is drawn in the terminal's own sixteen
/// colours — as on a measured light ground and on one nothing is known of:
/// the terminal paints them from its own palette, light or dark. Headings and
/// the table head stay uncoloured, as with no answer. Said dark, each byte is
/// the one `auto` draws with no answer.
#[test]
fn light_turns_on_the_light_role_colours_and_code_stays_the_terminals_for_terminal() {
    use super::ansi::{write_spans, write_spans_for_ground_and_level, Color, Role, Span, Style};
    use super::ground::{self, Ground, Hint, TerminalBackground};
    use super::palette::{self, ColorLevel, TerminalPalette};
    use pulldown_cmark::HeadingLevel;

    if !style::in_fresh_run_with(
        &path("light_turns_on_the_light_role_colours_and_code_stays_the_terminals_for_terminal"),
        &[("COLORTERM", "truecolor")],
    ) {
        return;
    }
    style::init(&Preferences::default());
    ground::set(Hint { word: TerminalBackground::Light, colorfgbg: None });
    let light = ground::current();
    assert_eq!(light, Ground::Lightness { light: true });
    let _ = super::theme::set(super::theme::Theme::terminal());

    let draw = |style: Style| {
        let mut out = String::new();
        write_spans(&Line::new(vec![Span::new("x", style)]), &mut out);
        out
    };
    for (role, expected) in [
        (Role::Code, "\u{1b}[34;49mx"),
        (Role::Link, "\u{1b}[36;49mx"),
        (Role::Heading, "\u{1b}[38;2;108;76;21;49mx"),
        (Role::Caret, "\u{1b}[38;2;143;83;0;49mx"),
        (Role::Model, "\u{1b}[38;2;106;78;17;49mx"),
        (Role::Cwd, "\u{1b}[38;2;40;96;48;49mx"),
        (Role::Warning, "\u{1b}[38;2;139;98;20;49mx"),
    ] {
        assert_eq!(draw(Style::new().fg(Color::Role(role))), expected, "{role:?}");
    }
    let model_scopes = ["entity.name.type", "support.type", "variable"];
    let cwd_scopes = ["string", "markup.underline.link"];
    let footer = |scopes: &[&str], fallback| {
        draw(super::view::footer_theme_style_on(scopes, fallback, light, ColorLevel::TrueColor))
    };
    assert_eq!(footer(&model_scopes, palette::FOOTER_MODEL), "\u{1b}[38;2;106;78;17;49mx");
    assert_eq!(footer(&cwd_scopes, palette::FOOTER_CWD), "\u{1b}[38;2;40;96;48;49mx");
    for level in [HeadingLevel::H1, HeadingLevel::H2] {
        let heading = super::markdown::heading_style_with_level(level, palette::terminal_palette(), ColorLevel::TrueColor);
        assert_eq!(heading.fg, None, "{level:?}");
    }
    assert_eq!(super::tables::header_style_with_level(palette::terminal_palette(), ColorLevel::TrueColor).fg, None);

    // Code: the terminal's own colour slots on every ground — said light,
    // measured light at sixteen colours, and unknown — span for span, and no
    // ink of zo's own among them. It was Catppuccin Latte here and Mocha with
    // nothing known, and neither followed the terminal's theme.
    let code = "fn main() {\n    let name = \"zo\";\n}\n";
    let said_light = super::highlight::highlight_code_to_lines(code, "rust");
    let measured_light = super::highlight::highlight_code_lines_on(
        code,
        "rust",
        Ground::Measured(TerminalPalette::new((76, 79, 105), (239, 241, 245), ColorLevel::Ansi16)),
        ColorLevel::Ansi16,
    );
    let unknown = super::highlight::highlight_code_lines_on(code, "rust", Ground::Unknown, ColorLevel::TrueColor);
    assert_eq!(said_light, measured_light, "said light and measured light draw the same slots");
    assert_eq!(said_light, unknown, "and so does a ground nothing is known of");
    let inks: Vec<Color> = said_light.iter().flat_map(|line| line.spans.iter().filter_map(|span| span.style.fg)).collect();
    assert!(
        !inks.is_empty() && inks.iter().all(|ink| matches!(ink, Color::Base(_) | Color::Indexed(0..=15))),
        "code in the terminal's colour slots: {inks:?}"
    );

    // Said dark: every byte as `auto` with no answer.
    let dark = Ground::Lightness { light: false };
    let written = |ground: Ground, lines: &[Line]| -> String {
        let mut out = String::new();
        for line in lines {
            write_spans_for_ground_and_level(line, ground, ColorLevel::TrueColor, &mut out);
        }
        out
    };
    let screen = |ground: Ground| -> String {
        let mut lines: Vec<Line> = [Role::Code, Role::Heading, Role::Caret, Role::Model, Role::Cwd, Role::Warning]
            .into_iter()
            .map(|role| Line::new(vec![Span::new("x", Style::new().fg(Color::Role(role)))]))
            .collect();
        lines.push(Line::new(vec![Span::new("x", Style::new().fg(Color::RED))]));
        lines.push(Line::new(vec![Span::dim("x")]));
        for (scopes, fallback) in [(&model_scopes[..], palette::FOOTER_MODEL), (&cwd_scopes[..], palette::FOOTER_CWD)] {
            let style = super::view::footer_theme_style_on(scopes, fallback, ground, ColorLevel::TrueColor);
            lines.push(Line::new(vec![Span::new("x", style)]));
        }
        lines.extend(super::highlight::highlight_code_lines_on(code, "rust", ground, ColorLevel::TrueColor));
        written(ground, &lines)
    };
    assert_eq!(screen(dark), screen(Ground::Unknown));
}

/// Every built-in theme's inline code, drawn on that theme's own ground,
/// clears the text floor there and keeps its hue: the colour the theme's
/// sources give a code span, or its blue slot, lifted by lightness alone
/// where it falls short.
#[test]
fn every_builtin_themes_inline_code_clears_the_floor_on_its_own_ground() {
    use super::ansi::{write_spans_for_palette, Color, Role, Span, Style};
    use super::palette::{contrast_ratio, ColorLevel, TerminalPalette};
    use super::theme::rgb_to_hsl;

    if !style::in_fresh_run(&path("every_builtin_themes_inline_code_clears_the_floor_on_its_own_ground")) {
        return;
    }
    style::init(&Preferences::default());
    let hue_diff = |a: f64, b: f64| {
        let d = (a - b).abs() % 360.0;
        d.min(360.0 - d)
    };
    for slug in super::theme::builtin_names() {
        let theme = super::theme::find_builtin(slug).expect("a built-in theme");
        if theme.is_terminal() {
            continue;
        }
        let (fg, bg) = (theme.foreground.expect("its ink"), theme.background.expect("its ground"));
        let palette = TerminalPalette::new(fg, bg, ColorLevel::TrueColor);
        let floor = palette.text_contrast_floor(bg);
        let given = theme.role_color(Role::Code).expect("every built-in theme colours inline code");
        let _ = super::theme::set(theme);
        let mut out = String::new();
        write_spans_for_palette(
            &Line::new(vec![Span::new("x", Style::new().fg(Color::Role(Role::Code)))]),
            Some(palette),
            &mut out,
        );
        let drawn = parse_fg_rgb(&out);
        assert!(contrast_ratio(drawn, bg) >= floor, "{slug}: {drawn:?} under {floor:.2} on {bg:?}");
        let (hue_given, sat_given, _) = rgb_to_hsl(given);
        let (hue_drawn, sat_drawn, _) = rgb_to_hsl(drawn);
        assert!(hue_diff(hue_given, hue_drawn) <= 3.0, "{slug}: hue {hue_given:.1} -> {hue_drawn:.1}");
        assert!((sat_given - sat_drawn).abs() <= 0.05, "{slug}: saturation {sat_given:.3} -> {sat_drawn:.3}");
    }
    let _ = super::theme::set(super::theme::Theme::terminal());
}
