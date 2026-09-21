//! codex 스타일 렌더러의 골든 테스트.
//!
//! 정본은 `tests/fixtures/codex-capture-v0.149.1.txt` — codex-cli 0.149.1 을 PTY 로 뜬
//! 원본 바이트다. 핵심 테스트([`replays_the_measured_capture_byte_for_byte`])는
//! 그 캡처와 **바이트 단위로** 같은지를 본다. 캡처에서 빼는 것은 딱 셋,
//! 전부 환경 산물이다:
//!
//! - 줄끝 `\r` — PTY 의 ONLCR
//! - `hook:` 줄들 — 이 머신의 codex 훅 설정
//! - 맨 앞 `^D\u{8}\u{8}` — `script(1)` 이 EOF 를 에코한 캐럿 표기와 백스페이스
//!
//! 나머지(라벨 철자·SGR 번호·빈 줄 위치)는 전부 계약이다.

use std::sync::atomic::{AtomicU64, Ordering};

use runtime::message_stream::{
    AgentResultStatus, BashResult, BlockId, PermissionChoice, PermissionDecision, PermissionPrompt,
    QuestionOption, RenderBlock, SystemLevel, ToolCallId, ToolCallStatus, ToolPreview,
    DiffHunk, DiffLine, DiffLineKind, DiffView, ToolResultBody, UserQuestionPrompt,
};
use runtime::usage::TokenUsage;
use zo_ide::ide::render::{
    Clock, InputSource, PendingPrompt, RenderOptions, Renderer, SessionBanner,
};
use zo_ide::tui::ansi::{Color, Line, Span, Style, write_plain, write_spans_for_palette};
use zo_ide::tui::diff::{hunk_lines, hunk_lines_for_palette};
use std::time::Duration;

use zo_ide::tui::palette::{ColorLevel, FOOTER_CWD, FOOTER_MODEL, TerminalPalette, contrast_ratio};
use zo_ide::tui::shimmer::shimmer_spans_for_palette;
use zo_ide::util::ansi::strip_ansi;

/// 캡처 원본 — 설계 노트가 아니라 이 골든이 바이트로 비교하는 픽스처이므로,
/// 자기를 읽는 시험 옆에 산다(`zo-ide/docs/` 는 공개 트리에 들어가지 않는다).
const CAPTURE: &str = include_str!("fixtures/codex-capture-v0.149.1.txt");

const CAPTURE_CWD: &str = "/private/tmp/claude-501/-Users-dev-zerocode-workspaces-zerocode-zerocode-cli/c1a46d57-1e90-46fe-a537-6d276fa63892/scratchpad/codex-style";

// ============================================================================
// 시계
// ============================================================================

/// 늘 같은 값을 주는 시계 — 캡처의 ` succeeded in 0ms:` 를 재현한다.
#[derive(Debug)]
struct FrozenClock(u64);

impl Clock for FrozenClock {
    fn now_millis(&self) -> u64 {
        self.0
    }
}

/// 읽을 때마다 `step` 만큼 나아가는 시계 — 경과 ms 계산을 고정한다.
#[derive(Debug)]
struct SteppingClock {
    now: AtomicU64,
    step: u64,
}

impl SteppingClock {
    fn new(step: u64) -> Self {
        Self {
            now: AtomicU64::new(0),
            step,
        }
    }
}

impl Clock for SteppingClock {
    fn now_millis(&self) -> u64 {
        self.now.fetch_add(self.step, Ordering::Relaxed)
    }
}

// ============================================================================
// 하네스
// ============================================================================

fn render(options: RenderOptions, clock: Box<dyn Clock>, blocks: Vec<RenderBlock>) -> String {
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut renderer = Renderer::with_clock(&mut buffer, options, clock);
        for block in blocks {
            assert!(
                renderer.push(block).is_none(),
                "이 시나리오에는 파킹될 프롬프트가 없다"
            );
        }
        renderer.finish_turn();
        assert!(!renderer.write_failed(), "Vec 로의 쓰기는 실패할 수 없다");
    }
    String::from_utf8(buffer).expect("렌더러는 UTF-8 만 쓴다")
}

fn text(id: u64, body: &str) -> RenderBlock {
    RenderBlock::TextDelta {
        id: BlockId(id),
        text: body.to_string(),
        done: true,
    }
}

fn bash_call(call_id: &str, command: &str, status: ToolCallStatus) -> RenderBlock {
    RenderBlock::ToolCall {
        id: BlockId(900),
        tool_call_id: ToolCallId(call_id.to_string()),
        name: "bash".to_string(),
        summary: command.to_string(),
        preview: ToolPreview::Bash {
            command: command.to_string(),
        },
        status,
    }
}

fn bash_result(call_id: &str, exit_code: i32, stdout: &str, stderr: &str) -> RenderBlock {
    RenderBlock::ToolResult {
        id: BlockId(901),
        tool_call_id: ToolCallId(call_id.to_string()),
        is_error: exit_code != 0,
        body: ToolResultBody::Bash(BashResult {
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            truncated: false,
        }),
    }
}

fn diff_view(language: Option<&str>, lines: Vec<(DiffLineKind, &str)>) -> DiffView {
    DiffView {
        old_path: Some("src/lib.rs".into()), new_path: Some("src/lib.rs".into()),
        language: language.map(str::to_string), hunks: vec![DiffHunk {
            old_start: 1, old_lines: 1, new_start: 1, new_lines: 1,
            lines: lines.into_iter().map(|(kind, text)| DiffLine { kind, text: text.into() }).collect(),
        }],
    }
}

fn usage(total: u32) -> RenderBlock {
    let counts = TokenUsage {
        input_tokens: total,
        output_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
        output_tokens_details: None,
    };
    RenderBlock::Usage {
        ctx_tokens: u64::from(total),
        cumulative: counts,
        current: counts,
    }
}

/// 캡처에서 환경 산물을 걷어낸, 계약으로서의 바이트열.
fn capture_contract() -> String {
    let mut out = String::new();
    let body = CAPTURE.strip_prefix("^D\u{8}\u{8}").unwrap_or(CAPTURE);
    for line in body.lines() {
        if strip_ansi(line).starts_with("hook:") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 캡처를 그대로 낳는 블록 시나리오.
fn capture_scenario() -> Vec<RenderBlock> {
    vec![
        RenderBlock::UserMessage {
            id: BlockId(1),
            text: "Run 'ls -la' in the current directory with your shell tool, then summarize what you see in one short sentence. Also include a tiny markdown sample in your final answer: a heading, a bullet list of two items, and one inline `code` span.".to_string(),
        },
        text(
            2,
            "I\u{2019}ll inspect the current directory, then give you the requested compact summary and Markdown sample.",
        ),
        bash_call("call_ls", "/bin/zsh -lc 'ls -la'", ToolCallStatus::Running),
        bash_result(
            "call_ls",
            0,
            "total 0\ndrwxr-xr-x@  3 dev  wheel   96 Aug 26 11:57 .\ndrwx------@ 15 dev  wheel  480 Aug 26 11:57 ..\n-rw-r--r--@  1 dev  wheel    0 Aug 26 11:57 capture-ansi.txt\n",
            "",
        ),
        text(
            3,
            "The directory contains one empty file, `capture-ansi.txt`.\n\n### Sample\n\n- First item\n- Second item with `code`",
        ),
        usage(6_575),
    ]
}

fn capture_banner() -> SessionBanner {
    SessionBanner {
        header: "OpenAI Codex v0.149.1".to_string(),
        workdir: CAPTURE_CWD.to_string(),
        model: "gpt-5.6-sol".to_string(),
        provider: "openai".to_string(),
        approval: "never".to_string(),
        sandbox: "read-only".to_string(),
        reasoning_effort: "xhigh".to_string(),
        reasoning_summaries: "none".to_string(),
        session_id: "01a03c00-7270-7852-ad34-f628d7c0051f".to_string(),
    }
}

fn capture_options() -> RenderOptions {
    RenderOptions {
        cwd: CAPTURE_CWD.to_string(),
        ..RenderOptions::default()
    }
}

// ============================================================================
// 골든
// ============================================================================

#[test]
fn replays_the_measured_capture_byte_for_byte() {
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut renderer =
            Renderer::with_clock(&mut buffer, capture_options(), Box::new(FrozenClock(0)));
        renderer.banner(&capture_banner());
        for block in capture_scenario() {
            assert!(renderer.push(block).is_none());
        }
        renderer.finish_turn();
    }
    let rendered = String::from_utf8(buffer).expect("UTF-8");
    let expected = capture_contract();
    assert_eq!(
        rendered, expected,
        "\n--- 렌더 ---\n{rendered:?}\n--- 캡처 ---\n{expected:?}\n"
    );
}

#[test]
fn label_spelling_and_sgr_numbers_are_pinned() {
    let rendered = render(
        capture_options(),
        Box::new(FrozenClock(0)),
        capture_scenario(),
    );
    // 라벨: user 는 fg36 + 리셋 하나, 어시스턴트 계열은 fg35 + italic + 리셋 둘.
    assert!(rendered.contains("\u{1b}[36muser\u{1b}[0m\n"));
    assert!(rendered.contains("\u{1b}[35m\u{1b}[3mcodex\u{1b}[0m\u{1b}[0m\n"));
    assert!(rendered.contains("\u{1b}[35m\u{1b}[3mexec\u{1b}[0m\u{1b}[0m\n"));
    assert!(rendered.contains("\u{1b}[32m succeeded in 0ms:\u{1b}[0m\n"));
    assert!(rendered.contains("\u{1b}[2mtokens used\u{1b}[0m\n6,575\n"));
    // 브라이트(90번대)·256색·커서 제어는 한 바이트도 없다.
    assert!(!rendered.contains("\u{1b}[9"));
    assert!(!rendered.contains("38;5;"));
    for forbidden in ["\u{1b}[2J", "\u{1b}[K", "\u{1b}[?2026", "\u{1b}[?1049", "\r"] {
        assert!(!rendered.contains(forbidden), "금지 시퀀스: {forbidden:?}");
    }
}

#[test]
fn tool_output_is_followed_by_exactly_one_blank_line() {
    let rendered = render(
        capture_options(),
        Box::new(FrozenClock(0)),
        capture_scenario(),
    );
    let plain = strip_ansi(&rendered);
    let lines: Vec<&str> = plain.lines().collect();
    let last_output = lines
        .iter()
        .position(|line| line.contains("capture-ansi.txt") && line.starts_with("-rw-"))
        .expect("ls 출력 마지막 줄");
    assert_eq!(lines[last_output + 1], "");
    assert_eq!(lines[last_output + 2], "codex");
}

#[test]
fn markdown_is_verbatim_by_default() {
    let rendered = render(
        RenderOptions::default(),
        Box::new(FrozenClock(0)),
        vec![text(1, "### Sample\n\n- First item\n- Second with `code`")],
    );
    assert_eq!(
        rendered,
        "\u{1b}[35m\u{1b}[3mcodex\u{1b}[0m\u{1b}[0m\n### Sample\n\n- First item\n- Second with `code`\n"
    );
}

#[test]
fn markdown_rendering_is_opt_in() {
    let blocks = vec![text(1, "### Sample\n\n- First item\n")];
    let verbatim = render(RenderOptions::default(), Box::new(FrozenClock(0)), blocks);
    let options = RenderOptions {
        render_markdown: true,
        ..RenderOptions::default()
    };
    let rendered = render(
        options,
        Box::new(FrozenClock(0)),
        vec![text(1, "### Sample\n\n- First item\n")],
    );
    assert!(verbatim.contains("### Sample"));
    assert_ne!(verbatim, rendered, "opt-in 이 켜지면 출력이 달라져야 한다");
    assert!(strip_ansi(&rendered).contains("Sample"));
}

// ============================================================================
// 패인(tty) 대 파이프
// ============================================================================

fn tty_options() -> RenderOptions {
    RenderOptions {
        input: InputSource::Tty,
        ..capture_options()
    }
}

/// cooked tty 는 타이핑을 이미 에코했다 — 렌더러가 다시 찍으면 `dd` 가 두 번
/// 보인다. 파이프에는 에코가 없으니 그대로 찍는다.
#[test]
fn a_tty_does_not_re_echo_the_user_message() {
    let blocks = || {
        vec![RenderBlock::UserMessage {
            id: BlockId(1),
            text: "dd".to_string(),
        }]
    };
    let piped = render(capture_options(), Box::new(FrozenClock(0)), blocks());
    assert_eq!(piped, "\u{1b}[36muser\u{1b}[0m\ndd\n");

    let tty = render(tty_options(), Box::new(FrozenClock(0)), blocks());
    assert_eq!(tty, "", "tty 에서는 재에코가 한 바이트도 없어야 한다");
}

/// 재에코를 생략해도 그 뒤의 어시스턴트 라벨은 제자리다.
#[test]
fn suppressing_the_echo_does_not_disturb_the_next_label() {
    let rendered = render(
        tty_options(),
        Box::new(FrozenClock(0)),
        vec![
            RenderBlock::UserMessage {
                id: BlockId(1),
                text: "dd".to_string(),
            },
            text(2, "done"),
        ],
    );
    assert_eq!(rendered, "\u{1b}[35m\u{1b}[3mcodex\u{1b}[0m\u{1b}[0m\ndone\n");
}

/// 마커는 줄바꿈 없이 한 줄을 연다 — tty 에코가 그 뒤를 잇는다.
#[test]
fn the_idle_marker_is_a_dim_prompt_without_a_newline() {
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut renderer =
            Renderer::with_clock(&mut buffer, tty_options(), Box::new(FrozenClock(0)));
        renderer.idle_marker();
    }
    assert_eq!(
        String::from_utf8(buffer).expect("UTF-8"),
        "\u{1b}[2m❯ \u{1b}[0m"
    );
}

/// 마커 뒤에 오는 블록은 빈 줄을 끼우지 않는다 — 에코가 이미 줄을 닫았다.
#[test]
fn the_next_block_lands_on_the_line_after_the_marker() {
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut renderer =
            Renderer::with_clock(&mut buffer, tty_options(), Box::new(FrozenClock(0)));
        renderer.idle_marker();
        renderer.push(text(1, "done"));
        renderer.finish_turn();
    }
    assert_eq!(
        String::from_utf8(buffer).expect("UTF-8"),
        "\u{1b}[2m❯ \u{1b}[0m\u{1b}[35m\u{1b}[3mcodex\u{1b}[0m\u{1b}[0m\ndone\n"
    );
}

/// 파이프에서는 마커가 없다 — 골든 바이트 불변의 다른 쪽 절반.
#[test]
fn a_pipe_gets_no_idle_marker() {
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut renderer =
            Renderer::with_clock(&mut buffer, capture_options(), Box::new(FrozenClock(0)));
        renderer.idle_marker();
        renderer.idle_marker();
    }
    assert!(buffer.is_empty(), "파이프 출력에 마커가 새면 안 된다");
}

// ============================================================================
// NO_COLOR
// ============================================================================

#[test]
fn no_color_emits_zero_escape_bytes() {
    let options = RenderOptions {
        color: false,
        ..capture_options()
    };
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut renderer = Renderer::with_clock(&mut buffer, options, Box::new(FrozenClock(0)));
        renderer.banner(&capture_banner());
        for block in capture_scenario() {
            assert!(renderer.push(block).is_none());
        }
        renderer.finish_turn();
    }
    let rendered = String::from_utf8(buffer).expect("UTF-8");
    assert!(
        !rendered.as_bytes().contains(&0x1b),
        "NO_COLOR 출력에 ESC 가 남았다: {rendered:?}"
    );
    // 라벨 철자와 줄 배치는 그대로다 — 색만 빠진다.
    assert_eq!(rendered, strip_ansi(&capture_contract()));
}

// Diff pins live alongside the byte contract because the interactive painter
// is the component that decides whether style data becomes ANSI at all.
#[test]
fn diff_highlighting_is_pinned_for_a_complete_hunk() {
    let lines = hunk_lines(&diff_view(Some("rust"), vec![
        (DiffLineKind::Context, "let text = \"open"),
        (DiffLineKind::Added, "still string\";"),
    ]), 100);
    assert!(lines[1].spans[2..].iter().any(|span| span.style.fg.is_some()));
}

#[test]
fn a_large_diff_has_no_syntax_tokens_to_recompute() {
    let source = "x".repeat(512 * 1024 + 1);
    let lines = hunk_lines(&diff_view(Some("rust"), vec![(DiffLineKind::Added, &source)]), 100);
    assert!(lines[0].spans[2..].iter().all(|span| span.style.fg == Some(zo_ide::tui::ansi::Color::GREEN)));
}

#[test]
fn deleted_diff_syntax_is_dimmed() {
    let lines = hunk_lines(&diff_view(Some("rust"), vec![(DiffLineKind::Removed, "let value = 1;")]), 100);
    assert!(lines[0].spans[2..].iter().all(|span| span.style.dim));
}

#[test]
fn no_color_diff_path_writes_no_escape_bytes() {
    let lines = hunk_lines(&diff_view(Some("rust"), vec![(DiffLineKind::Added, "fn main() {}")]), 100);
    let mut plain = String::new();
    for line in &lines { write_plain(line, &mut plain); }
    assert!(!plain.as_bytes().contains(&0x1b), "{plain:?}");
}

// The adaptive cases inject a pure palette. They never depend on the terminal
// running the test, while `None` proves that an unanswered OSC query leaves the
// old capture bytes untouched.
#[test]
fn a_failed_palette_query_keeps_the_existing_bytes_exactly() {
    let line = Line::new(vec![Span::new(
        "model",
        Style::new().fg(FOOTER_MODEL),
    )]);
    let mut rendered = String::new();
    write_spans_for_palette(&line, None, &mut rendered);

    assert_eq!(rendered, "\u{1b}[38;2;246;226;183;49mmodel");
}

#[test]
fn light_palette_footer_and_shimmer_meet_measured_contrast() {
    let palette = TerminalPalette::new(
        (0, 0, 0),
        (255, 255, 255),
        ColorLevel::TrueColor,
    );
    let mut rendered = String::new();
    write_spans_for_palette(
        &Line::new(vec![Span::new("model", Style::new().fg(FOOTER_MODEL))]),
        Some(palette),
        &mut rendered,
    );
    assert_eq!(rendered, "\u{1b}[38;2;106;78;17;49mmodel");
    assert!(contrast_ratio((106, 78, 17), palette.background()) >= 4.5);

    let mut rendered = String::new();
    write_spans_for_palette(
        &Line::new(vec![Span::new("cwd", Style::new().fg(FOOTER_CWD))]),
        Some(palette),
        &mut rendered,
    );
    assert_eq!(rendered, "\u{1b}[38;2;40;96;48;49mcwd");
    assert!(contrast_ratio((40, 96, 48), palette.background()) >= 4.5);

    let shimmer = shimmer_spans_for_palette("W", Duration::ZERO, Some(palette));
    let Color::Rgb(red, green, blue) = shimmer[0].style.fg.expect("shimmer colour") else {
        panic!("shimmer must stay truecolour");
    };
    assert!(contrast_ratio((red, green, blue), palette.background()) >= 4.5);

    let dark_palette = TerminalPalette::new((255, 255, 255), (0, 0, 0), ColorLevel::TrueColor);
    let dark_shimmer = shimmer_spans_for_palette("W", Duration::ZERO, Some(dark_palette));
    let Color::Rgb(red, green, blue) = dark_shimmer[0].style.fg.expect("shimmer colour") else {
        panic!("shimmer must stay truecolour");
    };
    assert!(contrast_ratio((red, green, blue), dark_palette.background()) >= 4.5);
}

#[test]
fn a_light_background_selects_the_light_truecolor_diff_palette() {
    let palette = TerminalPalette::new(
        (0, 0, 0),
        (255, 255, 255),
        ColorLevel::TrueColor,
    );
    let lines = hunk_lines_for_palette(
        &diff_view(None, vec![(DiffLineKind::Added, "added")]),
        100,
        Some(palette),
    );
    let mut rendered = String::new();
    write_spans_for_palette(&lines[0], Some(palette), &mut rendered);

    assert_eq!(
        rendered,
        "\u{1b}[2m1 \u{1b}[22m\u{1b}[32;48;2;218;251;225m+added"
    );
}

#[test]
fn an_ansi256_terminal_gets_the_indexed_diff_palette() {
    let palette = TerminalPalette::new((255, 255, 255), (0, 0, 0), ColorLevel::Ansi256);
    let lines = hunk_lines_for_palette(
        &diff_view(None, vec![(DiffLineKind::Added, "added")]),
        100,
        Some(palette),
    );
    let mut rendered = String::new();
    write_spans_for_palette(&lines[0], Some(palette), &mut rendered);

    assert_eq!(
        rendered,
        "\u{1b}[2m1 \u{1b}[22m\u{1b}[32;48;5;22m+added"
    );
}

#[test]
fn an_ansi16_terminal_keeps_diffs_foreground_only() {
    let palette = TerminalPalette::new((0, 0, 0), (255, 255, 255), ColorLevel::Ansi16);
    let lines = hunk_lines_for_palette(
        &diff_view(None, vec![(DiffLineKind::Added, "added")]),
        100,
        Some(palette),
    );
    let mut rendered = String::new();
    write_spans_for_palette(&lines[0], Some(palette), &mut rendered);

    assert_eq!(rendered, "\u{1b}[2m1 \u{1b}[22m\u{1b}[32;49m+added");
}

// ============================================================================
// 툴콜 수명
// ============================================================================

#[test]
fn one_call_announces_exec_exactly_once() {
    let rendered = render(
        capture_options(),
        Box::new(FrozenClock(0)),
        vec![
            bash_call("call_1", "ls -la", ToolCallStatus::Pending),
            bash_call("call_1", "ls -la", ToolCallStatus::Running),
            bash_call("call_1", "ls -la", ToolCallStatus::Ok),
            bash_result("call_1", 0, "total 0\n", ""),
        ],
    );
    assert_eq!(rendered.matches("exec\u{1b}[0m\u{1b}[0m").count(), 1);
    assert_eq!(rendered.matches("ls -la").count(), 1);
}

#[test]
fn a_call_that_skips_running_still_announces() {
    let rendered = render(
        capture_options(),
        Box::new(FrozenClock(0)),
        vec![
            bash_call("call_1", "ls -la", ToolCallStatus::Ok),
            bash_result("call_1", 0, "total 0\n", ""),
        ],
    );
    assert_eq!(rendered.matches("exec\u{1b}[0m\u{1b}[0m").count(), 1);
}

#[test]
fn pending_only_calls_are_never_announced() {
    let rendered = render(
        capture_options(),
        Box::new(FrozenClock(0)),
        vec![bash_call("call_1", "ls -la", ToolCallStatus::Pending)],
    );
    assert_eq!(rendered, "");
}

#[test]
fn elapsed_millis_come_from_the_clock() {
    let rendered = render(
        capture_options(),
        Box::new(SteppingClock::new(37)),
        vec![
            bash_call("call_1", "ls -la", ToolCallStatus::Running),
            bash_result("call_1", 0, "total 0\n", ""),
        ],
    );
    assert!(
        rendered.contains("\u{1b}[32m succeeded in 37ms:\u{1b}[0m"),
        "{rendered:?}"
    );
}

#[test]
fn a_failing_command_uses_the_red_exit_header() {
    let rendered = render(
        capture_options(),
        Box::new(FrozenClock(0)),
        vec![
            bash_call("call_1", "false", ToolCallStatus::Running),
            bash_result("call_1", 2, "", "boom\n"),
        ],
    );
    assert!(
        rendered.contains("\u{1b}[31m exited 2 in 0ms:\u{1b}[0m\nboom\n\n"),
        "{rendered:?}"
    );
}

#[test]
fn a_non_bash_call_borrows_the_exec_grammar() {
    let rendered = render(
        capture_options(),
        Box::new(FrozenClock(0)),
        vec![
            RenderBlock::ToolCall {
                id: BlockId(1),
                tool_call_id: ToolCallId("call_read".to_string()),
                name: "read_file".to_string(),
                summary: "src/lib.rs".to_string(),
                preview: ToolPreview::Read {
                    path: "src/lib.rs".to_string(),
                    range: Some((1, 40)),
                },
                status: ToolCallStatus::Running,
            },
            RenderBlock::ToolResult {
                id: BlockId(2),
                tool_call_id: ToolCallId("call_read".to_string()),
                is_error: false,
                body: ToolResultBody::Read {
                    path: "src/lib.rs".to_string(),
                    content: "fn main() {}\n".to_string(),
                    language: Some("rust".to_string()),
                    truncated: false,
                },
            },
        ],
    );
    assert_eq!(
        rendered,
        "\u{1b}[35m\u{1b}[3mtool\u{1b}[0m\u{1b}[0m\n\u{1b}[1mread_file\u{1b}[0m src/lib.rs:1-40\n\u{1b}[32m succeeded in 0ms:\u{1b}[0m\nfn main() {}\n\n"
    );
}

// ============================================================================
// 파킹
// ============================================================================

#[test]
fn a_permission_prompt_is_parked_not_answered() {
    let (responder, receiver) = tokio::sync::oneshot::channel();
    let mut buffer: Vec<u8> = Vec::new();
    let parked = {
        let mut renderer =
            Renderer::with_clock(&mut buffer, capture_options(), Box::new(FrozenClock(0)));
        renderer.push(RenderBlock::PermissionPrompt(PermissionPrompt {
            id: BlockId(1),
            tool_call_id: ToolCallId("call_1".to_string()),
            tool_name: "bash".to_string(),
            reasoning: "run ls".to_string(),
            audit_hint: None,
            choices: vec![PermissionChoice {
                key: 'y',
                label: "Allow once".to_string(),
                decision: PermissionDecision::AllowOnce,
            }],
            responder,
        }))
    };
    let rendered = String::from_utf8(buffer).expect("UTF-8");
    assert_eq!(
        rendered,
        "\u{1b}[1m⏸ bash\u{1b}[0m — [y]once [a]always [n]deny\n"
    );
    let Some(PendingPrompt::Permission(prompt)) = parked else {
        panic!("권한 프롬프트는 파킹되어 돌아와야 한다");
    };
    // responder 가 살아 있다 — 여기서 drop 되면 런타임은 hard deny 로 읽는다.
    prompt
        .responder
        .send(PermissionDecision::AllowOnce)
        .expect("파킹된 responder 는 아직 살아 있다");
    let mut receiver = receiver;
    assert_eq!(
        receiver.try_recv().expect("결정이 런타임에 닿는다"),
        PermissionDecision::AllowOnce
    );
}

#[test]
fn a_question_prompt_is_parked_not_answered() {
    let (responder, _receiver) = tokio::sync::oneshot::channel();
    let mut buffer: Vec<u8> = Vec::new();
    let parked = {
        let mut renderer =
            Renderer::with_clock(&mut buffer, capture_options(), Box::new(FrozenClock(0)));
        renderer.push(RenderBlock::UserQuestionPrompt(UserQuestionPrompt {
            id: BlockId(1),
            question: "Which auth method?".to_string(),
            header: Some("Auth".to_string()),
            options: vec![
                QuestionOption {
                    label: "OAuth".to_string(),
                    description: Some("Browser flow.".to_string()),
                    preview: None,
                },
                QuestionOption::plain("API key"),
            ],
            multi_select: false,
            responder,
        }))
    };
    let rendered = String::from_utf8(buffer).expect("UTF-8");
    // 파킹 줄 뒤에 **보기**가 선다. 이 로드는 append-only 라 TUI 의 오버레이를
    // 흉내 낼 수 없지만, 무엇을 고를 수 있는지가 빠지면 답할 수가 없다.
    assert_eq!(
        rendered,
        concat!(
            "\u{1b}[1m⏸ Which auth method?\u{1b}[0m — [1]-[2] or free text\n",
            "  1. OAuth  \u{1b}[2mBrowser flow.\u{1b}[0m\n",
            "  2. API key\n",
        )
    );
    assert!(matches!(parked, Some(PendingPrompt::Question(_))));
}

/// 여러 개를 고르는 질문은 쉼표 목록도 답이 된다 — 파킹 줄이 그렇게 말한다.
#[test]
fn a_multi_select_prompt_offers_the_comma_list() {
    let (responder, _receiver) = tokio::sync::oneshot::channel();
    let mut buffer: Vec<u8> = Vec::new();
    let _parked = {
        let mut renderer =
            Renderer::with_clock(&mut buffer, capture_options(), Box::new(FrozenClock(0)));
        renderer.push(RenderBlock::UserQuestionPrompt(UserQuestionPrompt {
            id: BlockId(1),
            question: "Which checks?".to_string(),
            header: None,
            options: vec![
                QuestionOption::plain("fmt"),
                QuestionOption::plain("clippy"),
            ],
            multi_select: true,
            responder,
        }))
    };
    let rendered = String::from_utf8(buffer).expect("UTF-8");
    assert_eq!(
        rendered,
        concat!(
            "\u{1b}[1m⏸ Which checks?\u{1b}[0m — [1]-[2] (comma for several) or free text\n",
            "  1. fmt\n",
            "  2. clippy\n",
        )
    );
}

// ============================================================================
// 나머지 블록
// ============================================================================

#[test]
fn reasoning_is_hidden_unless_asked_for() {
    let hidden = render(
        RenderOptions::default(),
        Box::new(FrozenClock(0)),
        vec![RenderBlock::Reasoning {
            id: BlockId(1),
            text: "weighing options".to_string(),
            signature: None,
            done: true,
        }],
    );
    assert_eq!(hidden, "");

    let options = RenderOptions {
        show_thinking: true,
        ..RenderOptions::default()
    };
    let shown = render(
        options,
        Box::new(FrozenClock(0)),
        vec![RenderBlock::Reasoning {
            id: BlockId(1),
            text: "weighing options".to_string(),
            signature: None,
            done: true,
        }],
    );
    assert_eq!(shown, "\u{1b}[2mweighing options\u{1b}[0m\n");
}

#[test]
fn system_lines_are_dim_bullets() {
    let rendered = render(
        RenderOptions::default(),
        Box::new(FrozenClock(0)),
        vec![RenderBlock::System {
            id: BlockId(1),
            level: SystemLevel::Warn,
            text: "context 80% full".to_string(),
        }],
    );
    assert_eq!(rendered, "\u{1b}[2m· context 80% full\u{1b}[0m\n");
}

/// 멀티라인 System 은 codex 에 없다 — 정보성 통지는 첫 줄만 남는다.
/// (`· provider overloaded; retrying …` 가 문단째 늘어지던 자리.)
#[test]
fn an_informational_system_block_keeps_only_its_first_line() {
    for level in [SystemLevel::Info, SystemLevel::Success, SystemLevel::Warn] {
        let rendered = render(
            RenderOptions::default(),
            Box::new(FrozenClock(0)),
            vec![RenderBlock::System {
                id: BlockId(1),
                level,
                text: "provider overloaded; retrying in 2s\nattempt 2 of 5\nrequest id abc123"
                    .to_string(),
            }],
        );
        assert_eq!(
            rendered, "\u{1b}[2m· provider overloaded; retrying in 2s\u{1b}[0m\n",
            "{level:?} 는 첫 줄만 남아야 한다"
        );
    }
}

/// 위험을 알리는 줄은 자르지 않는다 — 잘린 오류로는 사람이 대응할 수 없다.
#[test]
fn an_error_system_block_is_never_truncated() {
    let rendered = render(
        RenderOptions::default(),
        Box::new(FrozenClock(0)),
        vec![RenderBlock::System {
            id: BlockId(1),
            level: SystemLevel::Error,
            text: "turn ended: stream closed\nlast tool: bash".to_string(),
        }],
    );
    assert_eq!(
        rendered,
        "\u{1b}[2m· turn ended: stream closed\u{1b}[0m\n\u{1b}[2m· last tool: bash\u{1b}[0m\n"
    );
}

#[test]
fn an_agent_result_borrows_the_tool_grammar() {
    let rendered = render(
        RenderOptions::default(),
        Box::new(FrozenClock(0)),
        vec![RenderBlock::AgentResult {
            id: BlockId(1),
            label: "runtime-scout".to_string(),
            status: AgentResultStatus::Completed,
            summary: None,
            body: "found three call sites".to_string(),
        }],
    );
    assert_eq!(
        rendered,
        "\u{1b}[35m\u{1b}[3mtool\u{1b}[0m\u{1b}[0m\n\u{1b}[1mruntime-scout\u{1b}[0m completed\nfound three call sites\n\n"
    );
}

#[test]
fn live_ledger_blocks_write_nothing() {
    let rendered = render(
        RenderOptions::default(),
        Box::new(FrozenClock(0)),
        vec![
            RenderBlock::Image {
                id: BlockId(1),
                data: vec![0, 1, 2],
                media_type: "image/png".to_string(),
            },
            RenderBlock::CompactionProgress {
                streamed_chars: 120,
            },
        ],
    );
    assert_eq!(rendered, "");
}

#[test]
fn the_footer_is_absent_without_usage() {
    let rendered = render(
        RenderOptions::default(),
        Box::new(FrozenClock(0)),
        vec![text(1, "done")],
    );
    assert_eq!(rendered, "\u{1b}[35m\u{1b}[3mcodex\u{1b}[0m\u{1b}[0m\ndone\n");
}

#[test]
fn each_text_segment_gets_its_own_label() {
    let rendered = render(
        RenderOptions::default(),
        Box::new(FrozenClock(0)),
        vec![text(1, "first"), text(2, "second")],
    );
    assert_eq!(rendered.matches("codex\u{1b}[0m\u{1b}[0m").count(), 2);
}
