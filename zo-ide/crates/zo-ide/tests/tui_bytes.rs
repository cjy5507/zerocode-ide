//! 캡처에 고정한 TUI painter 바이트 테스트.
//!
//! 여기 있는 기대값은 **원시 캡처에서 뽑아 온다** — 손으로 옮겨 적은 상수가
//! 아니라 `docs/captures/codex-tui-v0.149.1-turn.bin` 안에 실제로 흐른
//! 바이트다. codex 0.149.1 이 같은 화면에서 낸 것과 우리 painter 가 내는 것을
//! 나란히 놓는다.
//!
//! 여덟 가지를 핀한다:
//! 1. 히스토리 삽입 이스케이프 문법
//! 2. 스피너(shimmer) 한 프레임의 회색 사다리
//! 3. 마크다운 헤딩 스타일 — 해시를 남긴 bold+italic
//! 4. `/model` 자동완성 팝업과 피커 화면(`…-model-picker.bin`)
//! 5. 스트리밍의 두 영역 — 개행에서만 커밋, 미확정 꼬리는 활성 칸
//! 6. 도구 셀 — 접힌 `Ran` 셀과 라이브 `Exploring` 셀
//!    (`…-tool-turn.bin`)
//! 7. effort 피커 여덟 행
//! 8. 표 — 커밋된 격자 바이트와 홀드백 중 꼬리 미리보기
//!    (`…-table.bin`)
//! 9. 코드 담장 — 문법 강조 스팬과 미지원 언어 폴백
//!    (`…-codeblock.bin`)
//! 10. 종료·재개 뒤 세션 요약 두 줄
//!     (`…-resume-summary.bin` · `…-turn.bin` 의 stdout 꼬리)
//! 11. 부팅 카드 경로의 가운데 자르기(`…-turn.bin`)
//! 12. 증분 렌더 — 흘려 넣은 것과 한 번에 넣은 것의 바이트가 같다
//! 13. `/status` 사용량 카드 — 게이지·리셋 문구·라벨 열
//!     (`…-v0.150.1-status.bin`)
//! 14. `AskUserQuestion` 오버레이 세 갈래 — 단일·다중·자유 서술
//!     (codex `bottom_pane/request_user_input` 의 스냅샷 문법)
//! 15. `/status`의 지속 목표와 autonomous 네 한도 행

use std::time::{Duration, Instant};

use zo_ide::effort::Effort;
use zo_ide::autonomy::limits::{
    DEFAULT_QUIET_AFTER_SECS, DEFAULT_STREAM_PHASE_AFTER_SECS,
};
use zo_ide::tui::ansi::{write_spans, Line, Span, Style};
use zo_ide::tui::activity::Activity;
use zo_ide::status_format;
use zo_ide::tui::cells::{self, MarkdownStream};
use zo_ide::tui::composer::{Composer, Submission};
use zo_ide::tui::fast;
use zo_ide::tui::effort_effect::{EffortEffect, EffortTier};
use zo_ide::tui::folds::{FoldIds, FoldMode};
use zo_ide::tui::markdown;
use zo_ide::tui::painter::Painter;
use zo_ide::tui::pending_input::PendingInputs;
use zo_ide::tui::permissions;
use zo_ide::tui::sessions;
use zo_ide::tui::summary::session_summary;
use zo_ide::tui::strings;
use zo_ide::tui::shimmer::shimmer_spans_for_palette;
use zo_ide::tui::slash;
use zo_ide::tui::tools::{Explored, Outcome, ToolCall, ToolGroup, ToolKind};
use zo_ide::tui::question;
use zo_ide::tui::view::{
    self, Frame, Picker, PickerRow, Popup, PopupRow, Status, StatusDetailsCapitalization,
};

/// 캡처 하나를 읽는다 — 열 벌이던 같은 세 줄을 한 자리로 모았다.
/// 캡처가 옮겨지면 여기 한 줄만 고친다.
fn capture_named(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/captures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// 캡처 원본. 이 시험 옆 `tests/fixtures/captures/` 에 있다.
fn capture() -> Vec<u8> {
    capture_named("codex-tui-v0.149.1-turn.bin")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

// ============================================================================
// 1. 히스토리 삽입 문법
// ============================================================================

/// 캡처의 2.83s 프레임: 뷰포트가 13행에서 시작하고 화면이 40행일 때 두 줄을
/// 넣는 시퀀스다 — 머리를 아래로 밀고(DECSTBM 머리 행..끝 · CUP · ESC M ·
/// DECSTBM 해제), 위쪽만 영역으로 묶고(DECSTBM 1..머리), 첫 줄이 갈 행 바로
/// 위에서 시작한다(CUP). 낱말과 순서는 그대로 쓰되 **자리**는 다르다(사용자
/// 결정 2026-09-06): codex 는 줄 수만큼 두 번 밀지만 여기 머리는 바닥까지
/// 밀리고(스물한 번), 줄은 머리 바로 위가 아니라 띠의 첫 행에 간다. 바닥에
/// 있는 머리 — 평소 — 는 밀 것이 없어 뒤쪽 두 토큰만 낸다.
#[test]
fn history_insertion_grammar_is_the_captured_one() {
    let measured = b"\x1b[13;40r\x1b[13;1H\x1bM\x1bM\x1b[r\x1b[1;14r\x1b[12;1H";
    assert!(
        contains(&capture(), measured),
        "the capture no longer holds the insertion sequence this test is pinned to"
    );

    // 화면 40행, 뷰포트 높이 7 → 바닥 33행(0 기준)에 앉고, 커서 행 12 부터가 띠다.
    let mut painter = Painter::new(Vec::new(), 120, 40, 12, true);
    painter.set_height(7);
    painter.take_frame();
    painter.insert_history(&[Line::empty(), Line::from_text("warn")]);
    let frame = painter.take_frame();
    assert!(
        frame.starts_with("\u{1b}[?2026h\u{1b}[1;33r\u{1b}[12;1H\r\n"),
        "a head on the floor: region above it, cursor above the band: {frame:?}"
    );
    assert!(!frame.contains("\u{1b}M"), "nothing to push: {frame:?}");

    // 캡처와 같은 기하 — 머리가 13행(1 기준)에 있고 아래가 비어 있다 — 는
    // 팝업이 링에 없는 행만큼 화면을 밀고 닫힌 뒤에 생긴다. 그때의 밀기가
    // 캡처의 앞쪽 토큰이다: 머리 행부터 끝까지를 영역으로, 머리 행에서 ESC M.
    let mut painter = Painter::new(Vec::new(), 120, 40, 22, true);
    painter.set_height(7);
    painter.set_popup_height(39);
    painter.set_height(7);
    painter.take_frame();
    painter.insert_history(&[Line::empty(), Line::from_text("warn")]);
    let frame = painter.take_frame();
    let push = format!("\u{1b}[?2026h\u{1b}[13;40r\u{1b}[13;1H{}\u{1b}[r", "\u{1b}M".repeat(21));
    assert!(frame.starts_with(&push), "the head is pushed onto the floor: {frame:?}");
    assert!(
        frame[push.len()..].starts_with("\u{1b}[1;33r\u{1b}[1;1H\r\n"),
        "then the band is filled from its first row: {frame:?}"
    );
}

/// 캡처의 빈 히스토리 줄 한 개 — `\r\n` + 색 리셋 + EL + 되돌림.
#[test]
fn a_history_line_is_framed_exactly_like_the_capture() {
    let measured = b"\r\n\x1b[39;49m\x1b[K\x1b[39m\x1b[49m\x1b[0m";
    assert!(contains(&capture(), measured), "capture drifted");

    let mut painter = Painter::new(Vec::new(), 120, 40, 12, true);
    painter.set_height(7);
    painter.take_frame();
    painter.insert_history(&[Line::empty()]);
    let frame = painter.take_frame();
    assert!(
        frame.contains(&String::from_utf8_lossy(measured).into_owned()),
        "frame was {frame:?}"
    );
}

/// Five noop iterations occupy the painter's one live-cell row. Each newer
/// count replaces that row; none of the discarded iteration candidates is
/// inserted into scrollback.
#[test]
fn five_quiet_loop_iterations_are_one_cell_and_one_row_delta() {
    let first = cells::quiet_loop_cell("loop-1", 1, "04:12", 80);
    let fifth = cells::quiet_loop_cell("loop-1", 5, "04:12", 80);
    assert_eq!(first.len(), 1);
    assert_eq!(fifth.len(), 1);
    assert_eq!(fifth[0].plain(), "• loop-1 · quiet ×5 (last 04:12)");

    let mut painter = Painter::new(Vec::new(), 80, 20, 19, true);
    painter.set_height(5);
    painter.take_frame();
    painter.paint(&first);
    painter.take_frame();
    painter.paint(&fifth);
    let delta = painter.take_frame();

    assert_eq!(
        delta.matches("\u{1b}[K").count(),
        1,
        "only the changed live row may be redrawn: {delta:?}"
    );
    assert_eq!(
        delta,
        concat!(
            "\u{1b}[?2026h\u{1b}[16;1H\u{1b}[0m\u{1b}[K",
            "\u{1b}[2m• \u{1b}[22m\u{1b}[1mloop-1\u{1b}[22m",
            "\u{1b}[2m\u{1b}[2m · quiet ×5 (last 04:12)\u{1b}[0m",
        ),
        "the one-row live-slot delta is a byte golden"
    );
}

/// A grow repaint is a byte-level bottom anchor, even when the inline head was
/// above the old floor. Zed can report exactly that DSR/size ordering; leaving
/// the old absolute row makes the new rows a blank band below the footer.
#[test]
fn a_grow_repaints_an_above_floor_footer_on_the_new_last_row() {
    let mut painter = Painter::new(Vec::new(), 120, 40, 17, true);
    painter.set_height(7);
    painter.take_frame();

    painter.resize(120, 60);
    let mut rows = vec![Line::empty(); 7];
    rows[6] = Line::from_text("footer");
    painter.paint(&rows);
    let frame = painter.take_frame();

    assert!(
        frame.starts_with("\u{1b}[?2026h\u{1b}[1;1H\u{1b}[J"),
        "a grow must clear and rebuild independently of the terminal's resize policy: {frame:?}"
    );
    assert!(
        frame.contains("\u{1b}[60;1H\u{1b}[0m\u{1b}[Kfooter\u{1b}[0m"),
        "the footer did not land on terminal row 60: {frame:?}"
    );
}

// ============================================================================
// 2. 스피너 한 프레임
// ============================================================================

/// codex 가 `Working` 을 훑을 때 쓰는 회색 사다리. 캡처에 나오는 여섯 단계를
/// 뽑아 우리 shimmer 가 같은 여섯 단계를 만드는지 본다.
#[test]
fn the_shimmer_ladder_is_the_captured_one() {
    let capture = capture();
    let mut measured: Vec<u8> = Vec::new();
    for level in [128u8, 138, 167, 202, 231, 242] {
        let needle = format!("\u{1b}[38;2;{level};{level};{level};49m");
        assert!(
            contains(&capture, needle.as_bytes()),
            "capture no longer holds level {level}"
        );
        measured.push(level);
    }

    // 밴드가 문구 한가운데 오는 시점을 골라 한 프레임을 뽑는다.
    let mut seen: Vec<u8> = Vec::new();
    for step in 0u8..64 {
        let elapsed = Duration::from_secs_f32(2.0 * f32::from(step) / 64.0);
        for span in shimmer_spans_for_palette("Working", elapsed, None) {
            if let Some(zo_ide::tui::ansi::Color::Rgb(r, g, b)) = span.style.fg {
                assert!(r == g && g == b, "shimmer must stay on the grey axis");
                if !seen.contains(&r) {
                    seen.push(r);
                }
            }
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, measured);
}

/// codex 는 **맨 트루컬러 전경을 한 번도 내지 않는다.**
///
/// 캡처의 문법 어휘를 통째로 세면 `ESC[38;2;R;G;Bm` 이 0건이고
/// `ESC[38;2;R;G;B;49m` 만 나온다 — crossterm `SetColors` 가 전경과 배경을 한
/// SGR 로 묶기 때문이다. 색이 같아도 바이트가 다르면 같은 화면이 아니다:
/// `;49` 가 빠지면 앞 셀이 깔아 둔 배경이 shimmer 글자 밑에 그대로 남는다.
///
/// 기존 shimmer 골든은 **회색 레벨만** 봤고(r==g==b 여섯 값), 스피너 골든은
/// `ESC[1m ESC[38;2;` 까지만 봤다 — 꼬리가 어디서 끝나는지는 아무도 안
/// 붙들었다. 8/27 자 zo 캡처에는 맨 형태가 2,427건 들어 있다. 지금 코드는
/// `StyleWriter::transition` 이 `set_colors` 를 거치므로 짝 형태를 내지만,
/// 그 사실을 지키는 것이 없었다.
#[test]
fn no_span_ever_emits_a_bare_truecolor_foreground() {
    let capture = capture();
    assert!(
        !contains_bare_truecolor(&capture),
        "the codex capture itself now holds a bare truecolor SGR — re-derive this rule"
    );

    let mut rendered = String::new();
    for step in 0u8..8 {
        let elapsed = Duration::from_secs_f32(2.0 * f32::from(step) / 8.0);
        let line = Line {
            spans: shimmer_spans_for_palette("Working", elapsed, None),
            ..Line::empty()
        };
        write_spans(&line, &mut rendered);
    }
    write_spans(&Status::working(Duration::ZERO).line(80), &mut rendered);

    assert!(
        rendered.contains("\u{1b}[38;2;"),
        "precondition: the shimmer paints in truecolor: {rendered:?}"
    );
    assert!(
        !contains_bare_truecolor(rendered.as_bytes()),
        "a span emitted `ESC[38;2;R;G;Bm` without the `;49` codex always pairs it \
         with — the colour is right and the bytes are not: {rendered:?}"
    );
}

/// `ESC[38;2;R;G;Bm` — a truecolor foreground with no background parameter.
fn contains_bare_truecolor(bytes: &[u8]) -> bool {
    let mut index = 0;
    while let Some(found) = bytes[index..]
        .windows(7)
        .position(|window| window == b"\x1b[38;2;")
    {
        let start = index + found + 7;
        let mut semicolons = 0;
        let mut cursor = start;
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'0'..=b'9' => {}
                b';' => semicolons += 1,
                // `m` ends the SGR; anything else means this was not one.
                _ => break,
            }
            cursor += 1;
        }
        // `R;G;B` is two semicolons; codex's `R;G;B;49` is three.
        if cursor < bytes.len() && bytes[cursor] == b'm' && semicolons == 2 {
            return true;
        }
        index = start;
    }
    false
}

// ============================================================================
// r34 — pending steer / queued follow-up preview
// ============================================================================

#[test]
fn pending_steer_preview_is_the_captured_three_line_widget() {
    let capture = capture_named("codex-tui-v0.150.1-r34-steer.bin");
    let measured_header = concat!(
        "\u{1b}[2m• \u{1b}[22mMessages to be submitted after next tool call",
        "\u{1b}[2m (press esc to interrupt and send immediately)"
    );
    assert!(
        contains(&capture, measured_header.as_bytes()),
        "r34 capture no longer holds the pending-steer header"
    );
    for measured in ["  ↳ first steer line", "    …"] {
        assert!(
            contains(&capture, measured.as_bytes()),
            "r34 capture no longer holds {measured:?}"
        );
    }

    let mut pending = PendingInputs::default();
    pending.push_steer(
        "first steer line\nsecond steer line\nthird steer line\nfourth hidden line"
            .to_string(),
    );
    let lines = pending.lines(120);
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();
    assert_eq!(
        plain,
        [
            "• Messages to be submitted after next tool call (press esc to interrupt and send immediately)",
            "  ↳ first steer line",
            "    second steer line",
            "    third steer line",
            "    …",
        ]
    );
    let mut header = String::new();
    write_spans(&lines[0], &mut header);
    assert_eq!(header, measured_header);
    let mut continuation = String::new();
    write_spans(&lines[2], &mut continuation);
    assert_eq!(continuation, "    \u{1b}[2msecond steer line");
}

#[test]
fn queued_follow_up_preview_has_the_captured_italic_and_alt_up_hint() {
    let capture = capture_named("codex-tui-v0.150.1-r34-queue-edit.bin");
    for measured in [
        "\u{1b}[2m• \u{1b}[22mQueued follow-up inputs",
        "\u{1b}[2m  ↳ \u{1b}[3mR34_QUEUED_DRAFT",
        "    ⌥ + ↑ edit last queued message",
    ] {
        assert!(
            contains(&capture, measured.as_bytes()),
            "r34 capture no longer holds {measured:?}"
        );
    }

    let mut pending = PendingInputs::default();
    pending.push_queued(Submission {
        text: "R34_QUEUED_DRAFT".to_string(),
        image_paths: Vec::new(),
    });
    let lines = pending.lines(120);
    let mut rendered = Vec::new();
    for line in &lines {
        let mut bytes = String::new();
        write_spans(line, &mut bytes);
        rendered.push(bytes);
    }
    assert_eq!(
        rendered,
        [
            "\u{1b}[2m• \u{1b}[22mQueued follow-up inputs",
            "\u{1b}[2m  ↳ \u{1b}[3mR34_QUEUED_DRAFT",
            "\u{1b}[2m    ⌥ + ↑ edit last queued message",
        ]
    );
}

/// 스피너 줄 전체 — 캡처의 `(0s • esc to interrupt)` 꼬리까지.
#[test]
fn the_working_row_carries_the_captured_interrupt_hint() {
    let measured = "\u{1b}[2m(0s • esc to interrupt)";
    assert!(contains(&capture(), measured.as_bytes()), "capture drifted");

    let mut rendered = String::new();
    write_spans(&Status::working(Duration::ZERO).line(80), &mut rendered);
    assert!(rendered.contains(measured), "rendered {rendered:?}");
    // 앞머리는 shimmer 가 훑는 `•` 와 `Working` 이다. 색 깊이에 따라
    // truecolor 또는 DIM/plain/BOLD 램프가 되므로 여기서는 어휘만 핀한다.
    assert!(rendered.contains("Working"));
}

/// A long tool call keeps the turn timer and says what is running, where, and
/// for how long. The suffix is dim like the captured interrupt hint; the
/// shimmer prefix remains independently pinned above.
#[test]
fn the_working_row_names_the_current_tool_target_and_elapsed_time() {
    let mut status = Status::working(Duration::from_secs(12));
    status.note_tool_started(
        "toolu_read",
        "Read",
        Some("workspace/crates/zo-ide/src/tui/view.rs"),
    );
    status.elapsed = Duration::from_secs(19);

    let line = status.line(120);
    assert_eq!(
        line.plain(),
        "• Working (19s • esc to interrupt) · Read · tui/view.rs · 7s"
    );
    let mut rendered = String::new();
    write_spans(&line, &mut rendered);
    // The shimmer prefix is truecolor or a DIM/plain/BOLD ramp depending on
    // the colour depth of the terminal running the test (see the interrupt
    // hint case above): the plain text above pins its vocabulary, and only
    // the dim suffix pins its bytes.
    assert!(
        rendered.ends_with("\u{1b}[2m(19s • esc to interrupt) · Read · tui/view.rs · 7s"),
        "tool suffix lost its byte grammar: {rendered:?}"
    );
}

#[test]
fn the_working_row_compacts_a_command_to_its_first_word() {
    let mut status = Status::working(Duration::from_secs(3));
    status.note_tool_started(
        "toolu_bash",
        "Bash",
        Some("cargo test -p zo-ide --test tui_bytes"),
    );
    status.elapsed = Duration::from_secs(5);

    assert_eq!(
        status.line(120).plain(),
        "• Working (5s • esc to interrupt) · Bash · cargo · 2s"
    );
}

#[test]
fn the_stream_phase_fallback_waits_fifteen_seconds() {
    use runtime::message_stream::types::StreamPhase;

    let mut status = Status::working(Duration::ZERO);
    status.note_stream_phase(StreamPhase::RequestSent { attempt: 1 });
    status.elapsed = Duration::from_secs(DEFAULT_STREAM_PHASE_AFTER_SECS - 1);
    assert!(
        !status.line(120).plain().contains("waiting for the model"),
        "a healthy first-token wait must not flicker onto the row"
    );

    status.elapsed = Duration::from_secs(DEFAULT_STREAM_PHASE_AFTER_SECS);
    assert_eq!(
        status.line(120).plain(),
        "• Working (15s • esc to interrupt) · waiting for the model 15s"
    );
}

#[test]
fn sixty_seconds_without_a_token_or_tool_event_becomes_quiet() {
    let mut status = Status::working(Duration::ZERO);
    let tool_event_at = 5;
    status.elapsed = Duration::from_secs(tool_event_at);
    status.note_tool_started(
        "toolu_read",
        "Read",
        Some("workspace/crates/zo-ide/src/tui/view.rs"),
    );
    status.note_tool_finished("toolu_read");

    status.elapsed = Duration::from_secs(tool_event_at + DEFAULT_QUIET_AFTER_SECS - 1);
    assert!(!status.line(120).plain().contains("quiet"));
    status.elapsed = Duration::from_secs(tool_event_at + DEFAULT_QUIET_AFTER_SECS);
    let line = status.line(120);
    assert_eq!(
        line.plain(),
        "• Working (1m 05s • esc to interrupt) · quiet 1m 00s · last: Read tui/view.rs"
    );
    let mut rendered = String::new();
    write_spans(&line, &mut rendered);
    assert!(
        rendered.ends_with(" · quiet 1m 00s · last: Read tui/view.rs"),
        "quiet suffix lost its byte grammar: {rendered:?}"
    );
}

/// Status details are separate dim rows, not one inline string. This is the
/// byte contract that lets a fan-out show each agent beneath the short header.
#[test]
fn status_detail_bytes_pin_multiple_rows() {
    let mut status = Status::working(Duration::ZERO);
    status.set_details([
        "scout · reading src/lib.rs · 12s".to_string(),
        "reviewer · thinking · 9s".to_string(),
    ], StatusDetailsCapitalization::Preserve);
    let mut rendered = String::new();
    for (index, line) in status.detail_lines(80).iter().enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        write_spans(line, &mut rendered);
    }
    assert_eq!(
        rendered,
        "\u{1b}[2m  └ scout · reading src/lib.rs · 12s\n\u{1b}[2m  └ reviewer · thinking · 9s"
    );
}

/// One Workflow call may own many helpers. Its Working viewport keeps the
/// wave tally and a byte-stable row for each live helper.
#[test]
fn a_live_workflow_pins_the_wave_and_two_helper_rows() {
    let mut status = Status::working(Duration::from_secs(12));
    status.set_details(
        [
            strings::helper_wave(2, 2),
            strings::helper(
                "scout",
                3,
                Duration::from_secs(9),
                &Activity::new("Read", Some("src/tui/view.rs")),
            ),
            strings::helper(
                "reviewer",
                1,
                Duration::from_secs(4),
                &Activity::new("Bash", Some("cargo test -p zo-ide")),
            ),
        ],
        StatusDetailsCapitalization::Preserve,
    );

    let mut rendered = String::new();
    for (index, line) in status.detail_lines(120).iter().enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        write_spans(line, &mut rendered);
    }
    assert_eq!(
        rendered,
        concat!(
            "\u{1b}[2m  └ agents 2 · running 2 · done 0\n",
            "\u{1b}[2m  └ scout · 3 tool uses · 9s · Read · tui/view.rs\n",
            "\u{1b}[2m  └ reviewer · 1 tool use · 4s · Bash · cargo",
        )
    );
}

/// The cap ellipsizes the final wrapped row instead of inventing a count row.
#[test]
fn status_detail_bytes_pin_the_hidden_count() {
    let mut status = Status::working(Duration::ZERO);
    status.set_details(
        (1..=5).map(|number| format!("agent-{number}")),
        StatusDetailsCapitalization::Preserve,
    );
    let mut rendered = String::new();
    for (index, line) in status.detail_lines(80).iter().enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        write_spans(line, &mut rendered);
    }
    assert_eq!(
        rendered,
        "\u{1b}[2m  └ agent-1\n\u{1b}[2m  └ agent-2\n\u{1b}[2m  └ agent-3…"
    );
}

/// No details means no detail bytes and preserves the old status height.
#[test]
fn status_detail_bytes_pin_the_empty_case() {
    let status = Status::working(Duration::ZERO);
    assert!(status.detail_lines(80).is_empty());
}

// ============================================================================
// 3. 마크다운 헤딩
// ============================================================================

/// 캡처의 답변 첫 줄. codex 는 `### ` 를 지우지 않고 h3 스타일만 입힌다.
#[test]
fn a_heading_keeps_its_hashes_and_takes_bold_italic() {
    let measured = "\u{1b}[1m\u{1b}[3m### 자기소개";
    assert!(contains(&capture(), measured.as_bytes()), "capture drifted");

    let mut rendered = String::new();
    write_spans(&markdown::render("### 자기소개")[0], &mut rendered);
    assert_eq!(rendered, measured);
}

/// 답변 셀 전체 — dim `• ` 마커가 앞에 붙는다(캡처: `ESC[2m• ` 뒤에 헤딩).
#[test]
fn the_answer_cell_prefixes_the_heading_with_a_dim_bullet() {
    let measured = "\u{1b}[2m• ";
    assert!(contains(&capture(), measured.as_bytes()), "capture drifted");

    let mut stream = MarkdownStream::answer(80);
    stream.push("### 자기소개\n");
    let lines = stream.finish();
    let mut rendered = String::new();
    write_spans(lines.last().expect("body"), &mut rendered);
    // 캡처(`codex-tui-v0.149.1-turn.bin`) 히스토리 22행 그대로:
    // `ESC[2m• ESC[22mESC[1mESC[3m### 자기소개`. dim 을 끄는 것은 `ESC[0m` 이
    // 아니라 `ESC[22m` 이다(codex `ModifierDiff::queue`).
    let measured_row = "\u{1b}[2m• \u{1b}[22m\u{1b}[1m\u{1b}[3m### 자기소개";
    assert!(
        contains(&capture(), measured_row.as_bytes()),
        "capture no longer frames the answer heading this way"
    );
    assert_eq!(rendered, measured_row);
}

/// 유저 셀의 `› ` — 캡처는 bold+dim 이다.
#[test]
fn the_user_cell_marker_is_bold_dim() {
    let measured = "\u{1b}[1m\u{1b}[2m› ";
    assert!(contains(&capture(), measured.as_bytes()), "capture drifted");

    let mut rendered = String::new();
    write_spans(
        &Line::new(vec![Span::new("› ", Style::new().bold().dim())]),
        &mut rendered,
    );
    assert_eq!(rendered, measured);
}

// ============================================================================
// 4. `/model` 자동완성 팝업과 피커 — model-picker 캡처
// ============================================================================

/// `/model` 캡처 원본.
fn picker_capture() -> Vec<u8> {
    capture_named("codex-tui-v0.149.1-model-picker.bin")
}

/// Codex 0.150.0's fresh fast/permissions PTY capture. The earlier 0.149.1
/// captures remain untouched and are used by the older parity tests below.
fn r8_codex_capture() -> Vec<u8> {
    capture_named("codex-tui-v0.150.0-fast-permissions-colored.bin")
}

fn r8_unsupported_capture() -> Vec<u8> {
    capture_named("codex-tui-v0.150.0-fast-unsupported.bin")
}

fn r8_zo_capture() -> Vec<u8> {
    capture_named("zo-tui-v0.1-r8-fast-permissions.bin")
}

// ============================================================================
// 4b. r8 `/fast` and `/permissions` — Codex 0.150.0
// ============================================================================

#[test]
fn the_r8_fast_popup_matches_codex_0150() {
    let capture = r8_codex_capture();
    let measured = "/fast  1.5x speed, increased usage";
    assert!(contains(&capture, measured.as_bytes()), "capture drifted");

    let popup = Popup {
        rows: slash::matches_for_model("/fast", true)
            .into_iter()
            .map(|command| PopupRow {
                name: command.name().to_string(),
                description: command.description().to_string(),
            })
            .collect(),
        selected: 0,
    };
    let lines = popup.lines(120);
    assert_eq!(lines[0].plain(), format!("  {measured}"));
    let mut rendered = String::new();
    write_spans(&lines[0], &mut rendered);
    assert_eq!(
        rendered,
        format!("\u{1b}[1m\u{1b}[38;5;6;49m  {measured}")
    );
}

/// codex draws the context value on the right of the composer footer as
/// `"{percent}% context left"`, dim
/// (`bottom_pane/footer.rs::context_window_line`, rust-v0.150.1). The value is
/// right-aligned, so the gap before it is padding, not a separator.
#[test]
fn the_footer_carries_the_codex_context_value() {
    let line = zo_ide::tui::view::footer(
        "claude-opus-5", "high", None, "~/work", 80, None, Some(42), None, false, None, None,
    );
    let plain = line.plain();
    assert!(
        plain.starts_with("  claude-opus-5 high · ~/work"),
        "footer was {plain:?}"
    );
    assert!(plain.ends_with("42% context left"), "footer was {plain:?}");

    let mut rendered = String::new();
    write_spans(&line, &mut rendered);
    assert!(
        rendered.contains("\u{1b}[2m42% context left"),
        "the context value must be dim; footer was {rendered:?}"
    );
}

/// With no window to divide by, codex falls back to the used-token form —
/// `"{compact} used"` — rather than claiming a percentage it cannot know.
#[test]
fn the_footer_falls_back_to_used_tokens_without_a_window() {
    let line = zo_ide::tui::view::footer(
        "claude-opus-5", "high", None, "~/work", 80, None, None, Some(6_575), false, None,
        None,
    );
    assert!(
        line.plain().ends_with("6.6K used"),
        "footer was {:?}",
        line.plain()
    );
}

/// Knowing nothing draws nothing. codex renders an empty line while the window
/// is still pending, and "100% context left" would be a lie here.
#[test]
fn the_footer_says_nothing_when_the_context_is_unknown() {
    let line = zo_ide::tui::view::footer(
        "claude-opus-5", "high", None, "~/work", 80, None, None, None, false, None, None,
    );
    assert_eq!(line.plain(), "  claude-opus-5 high · ~/work");
}

/// Plan is the sole collaboration-mode variant.  It deliberately does not
/// borrow Codex's Shift+Tab wording: in zo that key cycles permissions.
#[test]
fn the_footer_pins_plan_mode_without_a_false_key_hint() {
    let line = zo_ide::tui::view::footer(
        "claude-opus-5",
        "high", None,
        "~/work",
        120,
        None,
        Some(42),
        None,
        true,
        Some(zo_ide::goal::GoalFooterStatus::Pursuing),
        None,
    );
    let plain = line.plain();
    assert!(plain.contains("Plan mode · 42% context left"), "footer was {plain:?}");
    assert!(!plain.contains("Pursuing goal"), "plan must win: {plain:?}");
    assert!(!plain.contains("shift+tab"), "footer was {plain:?}");

    let mut rendered = String::new();
    write_spans(&line, &mut rendered);
    assert!(
        rendered.contains("\u{1b}[35;49mPlan mode"),
        "plan indicator must be magenta; footer was {rendered:?}"
    );
}

/// Pin every goal-state phrase at the footer boundary.  The autonomous-limit
/// phrase is zo's documented minimal extension of Codex's usage-limit state.
#[test]
fn the_footer_pins_every_goal_status_phrase() {
    use zo_ide::goal::GoalFooterStatus;

    for (status, expected) in [
        (GoalFooterStatus::Pursuing, "Pursuing goal"),
        (GoalFooterStatus::Paused, "Goal paused (/goal resume)"),
        (GoalFooterStatus::Stalled, "Goal stalled (/goal resume)"),
        (
            GoalFooterStatus::HitAutonomousLimits,
            "Goal hit autonomous limits (/goal resume)",
        ),
        (GoalFooterStatus::Unmet, "Goal unmet"),
        (GoalFooterStatus::Achieved, "Goal achieved"),
    ] {
        let line = zo_ide::tui::view::footer(
            "m", "", None, "", 100, None, None, None, false, Some(status), None,
        );
        assert!(line.plain().ends_with(expected), "footer was {:?}", line.plain());

        let mut rendered = String::new();
        write_spans(&line, &mut rendered);
        assert!(
            rendered.contains(&format!("\u{1b}[35;49m{expected}")),
            "goal indicator must be magenta; footer was {rendered:?}"
        );
    }
}

#[test]
fn loop_footer_is_visible_but_yields_to_goal() {
    let loop_only = zo_ide::tui::view::footer(
        "m",
        "", None,
        "",
        100,
        None,
        None,
        None,
        false,
        None,
        Some("loop: every 10m · next 04:12 · quiet ×3"),
    );
    assert!(loop_only.plain().contains("loop: every 10m"));

    let with_goal = zo_ide::tui::view::footer(
        "m",
        "", None,
        "",
        100,
        None,
        None,
        None,
        false,
        Some(zo_ide::goal::GoalFooterStatus::Pursuing),
        Some("loop: every 10m"),
    );
    assert!(with_goal.plain().contains("Pursuing goal"));
    assert!(!with_goal.plain().contains("loop: every 10m"));
}

/// When the keyboard hint and the context value cannot both fit, the context
/// value is the one that stays — it is the answer the operator came for.
#[test]
fn a_narrow_footer_keeps_the_context_and_drops_the_hint() {
    let wide = zo_ide::tui::view::footer(
        "claude-opus-5",
        "high", None,
        "~/work",
        120,
        Some("? for shortcuts"),
        Some(7),
        None,
        false,
        None,
        None,
    );
    assert!(wide.plain().contains("? for shortcuts"), "footer was {:?}", wide.plain());
    assert!(wide.plain().contains("7% context left"));

    let narrow = zo_ide::tui::view::footer(
        "claude-opus-5",
        "high", None,
        "~/work",
        40,
        Some("? for shortcuts"),
        Some(7),
        None,
        false,
        None,
        None,
    );
    assert!(
        !narrow.plain().contains("? for shortcuts"),
        "the hint must yield first; footer was {:?}",
        narrow.plain()
    );
    assert!(narrow.plain().contains("7% context left"));
}

/// Once the hint has yielded, an active goal keeps its status before the
/// context value yields.  This makes the new always-visible state stable at
/// narrow widths rather than letting a changing context percentage displace it.
#[test]
fn a_narrow_footer_keeps_goal_before_context_and_hint() {
    let line = zo_ide::tui::view::footer(
        "m",
        "", None,
        "",
        30,
        Some("? for shortcuts"),
        Some(7),
        None,
        false,
        Some(zo_ide::goal::GoalFooterStatus::Pursuing),
        None,
    );
    let plain = line.plain();
    assert!(plain.contains("Pursuing goal"), "footer was {plain:?}");
    assert!(!plain.contains("7% context left"), "footer was {plain:?}");
    assert!(!plain.contains("? for shortcuts"), "footer was {plain:?}");
}

#[test]
fn the_r8_fast_footer_token_matches_codex_0150() {
    let capture = r8_codex_capture();
    assert!(contains(capture.as_slice(), b"gpt-5.4 medium fast"));
    assert!(contains(
        capture.as_slice(),
        b"\x1b[38;2;246;226;183;49mgpt-5.4 medium fast"
    ));

    let line = zo_ide::tui::view::footer(
        "gpt-5.4",
        &fast::display_effort("medium", true), None,
        "~/work",
        120,
        None,
        None,
        None,
        false,
        None,
        None,
    );
    assert_eq!(line.plain(), "  gpt-5.4 medium fast · ~/work");
    let mut rendered = String::new();
    write_spans(&line, &mut rendered);
    assert!(
        rendered.contains("\u{1b}[38;2;246;226;183;49mgpt-5.4 medium fast"),
        "footer was {rendered:?}"
    );
}

#[test]
fn the_r8_permissions_picker_matches_codex_preset_copy() {
    let capture = r8_codex_capture();
    for measured in [
        "Update Model Permissions",
        "Ask for approval (current)",
        "Approve for me",
        "Full Access",
        "Press enter to confirm or esc to go back",
    ] {
        assert!(contains(&capture, measured.as_bytes()), "capture drifted at {measured}");
    }

    let choices = permissions::choices(runtime::PermissionMode::WorkspaceWrite);
    let picker = Picker {
        title: permissions::TITLE.to_string(),
        title_style: Style::new().bold(),
        note: String::new(),
        rows: choices
            .iter()
            .map(|choice| PickerRow {
                label: choice.label.clone(),
                description: choice.description.to_string(),
                dim: false,
            })
            .collect(),
        selected: 0,
        footer: "Press enter to confirm or esc to go back".to_string(),
    };
    let lines = picker.lines(120, 40);
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();
    assert_eq!(plain[2], "  Update Model Permissions");
    assert!(plain.iter().any(|line| line.contains("Ask for approval (current)")));
    assert!(plain.iter().any(|line| line.contains("Approve for me")));
    assert!(plain.iter().any(|line| line.contains("Full Access")));
    assert_eq!(
        plain.last().map(String::as_str),
        Some("  Press enter to confirm or esc to go back")
    );

    let mut title = String::new();
    write_spans(&lines[2], &mut title);
    assert_eq!(title, "  \u{1b}[1mUpdate Model Permissions");
    let mut selected = String::new();
    write_spans(&lines[4], &mut selected);
    assert!(
        selected.starts_with("\u{1b}[1m\u{1b}[38;5;6;49m› 1. Ask for approval (current)"),
        "selected row was {selected:?}"
    );

    // The freshly captured zo screen carries the same visible words and
    // proves the TUI path exercised the picker, not only the pure renderer.
    let zo_capture = r8_zo_capture();
    assert!(contains(&zo_capture, b"Update Model Permissions"));
    assert!(contains(&zo_capture, b"Approve for me"));
}

#[test]
fn r8_unsupported_fast_follows_codex_unrecognized_command_path() {
    assert!(slash::matches_for_model("/fast", false).is_empty());
    assert_eq!(
        fast::UNSUPPORTED_COMMAND_MESSAGE,
        "Unrecognized command '/fast'. Type \"/\" for a list of supported commands."
    );
    assert!(contains(
        &r8_unsupported_capture(),
        fast::UNSUPPORTED_COMMAND_MESSAGE.as_bytes()
    ));
}

/// 캡처의 8.24s 프레임 — 타이핑 중 컴포저 아래에 뜬 한 줄 팝업이다:
/// `ESC[1m ESC[38;5;6;49m /model  choose what model and reasoning effort to use`.
/// 5라운드에서 `SetColors` 를 옮겨 온 뒤로는 `;49` 까지 같다 — 원본은 컴포저
/// 3열에 CUP 으로 놓고 우리는 뷰포트 행을 통째로 그리므로 앞의 두 칸만 우리
/// 것이다.
#[test]
fn the_slash_popup_row_is_the_captured_one() {
    let measured = "/model  choose what model and reasoning effort to use";
    assert!(contains(&picker_capture(), measured.as_bytes()), "capture drifted");
    assert!(
        contains(&picker_capture(), b"\x1b[1m\x1b[38;5;6;49m/model  choose"),
        "capture no longer bolds the popup row in cyan"
    );

    let popup = Popup {
        rows: slash::matches("/model")
            .into_iter()
            .map(|command| PopupRow {
                name: command.name().to_string(),
                description: command.description().to_string(),
            })
            .collect(),
        selected: 0,
    };
    let lines = popup.lines(120);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].plain(), format!("  {measured}"));

    let mut rendered = String::new();
    write_spans(&lines[0], &mut rendered);
    assert_eq!(rendered, format!("\u{1b}[1m\u{1b}[38;5;6;49m  {measured}"));
}

/// 캡처의 11.03s 프레임 — `Select Model and Effort` 피커 전체. 번호 행의
/// 설명 열, `(default)`/`(current)` 표기, `›` 커서, 푸터가 모두 여기서 나온다.
#[test]
fn the_model_picker_matches_the_captured_screen() {
    let capture = picker_capture();
    for measured in [
        "\u{1b}[1mSelect Model and Effort",
        "  1. gpt-5.6-sol (default)   ",
        "  2. gpt-5.6-terra           ",
        "\u{1b}[38;5;6;49m› 3. gpt-5.6-luna (current)  Fast and affordable agentic coding model.",
        "\u{1b}[2mPress enter to confirm or esc to go back",
    ] {
        assert!(
            contains(&capture, measured.as_bytes()),
            "capture no longer holds {measured:?}"
        );
    }

    // 캡처와 같은 세 줄을 우리 피커에 넣으면 같은 열에 선다.
    let picker = Picker {
        title: "Select Model and Effort".to_string(),
        title_style: Style::new().bold(),
        note: "Only providers with credentials are listed.".to_string(),
        rows: vec![
            PickerRow {
                label: "gpt-5.6-sol (default)".to_string(),
                description: "Latest frontier agentic coding model.".to_string(),
                dim: false,
            },
            PickerRow {
                label: "gpt-5.6-terra".to_string(),
                description: "Balanced agentic coding model for everyday work.".to_string(),
                dim: false,
            },
            PickerRow {
                label: "gpt-5.6-luna (current)".to_string(),
                description: "Fast and affordable agentic coding model.".to_string(),
                dim: false,
            },
        ],
        selected: 2,
        footer: "Press enter to confirm or esc to go back".to_string(),
    };
    let lines = picker.lines(120, 39);
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();
    assert_eq!(plain[0], "");
    assert_eq!(plain[1], "");
    assert_eq!(plain[2], "  Select Model and Effort");
    assert_eq!(plain[3], "  Only providers with credentials are listed.");
    assert_eq!(plain[4], "");
    assert_eq!(plain[5], "  1. gpt-5.6-sol (default)   Latest frontier agentic coding model.");
    assert_eq!(
        plain[6],
        "  2. gpt-5.6-terra           Balanced agentic coding model for everyday work."
    );
    assert_eq!(
        plain[7],
        "› 3. gpt-5.6-luna (current)  Fast and affordable agentic coding model."
    );
    assert_eq!(plain[8], "");
    assert_eq!(plain[9], "  Press enter to confirm or esc to go back");
    assert_eq!(lines.len(), 10);

    // 제목·푸터·고른 줄의 바이트 문법.
    let mut title = String::new();
    write_spans(&lines[2], &mut title);
    assert_eq!(title, "  \u{1b}[1mSelect Model and Effort");
    let mut footer = String::new();
    write_spans(&lines[9], &mut footer);
    assert_eq!(footer, "  \u{1b}[2mPress enter to confirm or esc to go back");
    let mut selected = String::new();
    write_spans(&lines[7], &mut selected);
    assert_eq!(
        selected,
        "\u{1b}[1m\u{1b}[38;5;6;49m› 3. gpt-5.6-luna (current)  Fast and affordable agentic coding model."
    );
}

// ============================================================================
// 5. 스트리밍 — codex 의 두 영역(확정 큐 + 미확정 꼬리)
// ============================================================================

/// 사용자가 본 "뭉텅이"의 재현: Claude 가 쓰는 개행 없는 긴 문단.
///
/// 3라운드는 안전 접두어를 커밋했고 4라운드는 미확정 원문을 꼬리 셀에 그렸다.
/// 5라운드의 기준은 바이트라 **codex 를 그대로** 따른다: 개행이 오기 전까지
/// 화면에는 아무것도 안 나온다. 정본 두 곳이 그렇게 말한다 —
/// `chatwidget/streaming.rs:485` ("Unterminated source is buffered by the
/// controller and cannot change the visible tail.") 와
/// `streaming/controller.rs::push_delta`(개행이 온 델타에서만 `render` 를
/// 키운다) · `active_tail_budget_lines`(표가 없으면 홀드백 0).
#[test]
fn a_long_unbroken_paragraph_stays_silent_until_its_newline() {
    let width = 40;
    let paragraph = "The sea holds a patience nothing on land can match, \
                     folding the same water over the same stones for millennia \
                     without hurry or complaint. It is loud and it is silent, \
                     depending on how far down you go.";

    let mut stream = MarkdownStream::answer(width);
    let mut tail_widths: Vec<usize> = Vec::new();
    for ch in paragraph.chars() {
        stream.push(&ch.to_string());
        // 개행이 하나도 없으니 스크롤백은 한 행도 안 받는다.
        assert!(
            stream.tick(std::time::Instant::now()).is_empty(),
            "nothing may be committed before a source newline"
        );
        tail_widths.push(stream.tail().len());
    }

    // 1. 꼬리는 한 번도 채워지지 않는다 — 활성 셀 칸은 `Working` 의 것이다.
    assert!(
        tail_widths.iter().all(|width| *width == 0),
        "the tail must stay empty while the source has no newline: {tail_widths:?}"
    );

    // 2. 개행이 오면 그 전부가 큐를 타고 스크롤백으로 내려간다.
    stream.push("\n");
    let mut rows: Vec<String> = Vec::new();
    let start = std::time::Instant::now();
    for step in 0..64 {
        rows.extend(
            stream
                .tick(start + Duration::from_millis(step))
                .iter()
                .map(Line::plain),
        );
    }
    assert!(stream.tail().is_empty(), "the tail must empty at the newline");
    // 5. append-only 로 잃거나 겹친 글자가 없다 — 접두어를 벗겨 이으면 원문이다.
    let body: Vec<String> = rows
        .iter()
        .filter(|row| !row.is_empty())
        .map(|row| row[row.char_indices().nth(2).map_or(row.len(), |(at, _)| at)..].to_string())
        .collect();
    assert_eq!(body.join(" "), paragraph);
}

/// 개행이 있는 원문의 커밋 규칙은 그대로다 — 골든이 기대는 동작.
#[test]
fn newline_terminated_source_still_commits_at_the_newline() {
    let mut stream = MarkdownStream::answer(60);
    stream.push("- one");
    assert!(
        stream.tick(std::time::Instant::now()).is_empty(),
        "a list line never flows early"
    );
    stream.push("\n");
    let mut out: Vec<String> = Vec::new();
    let start = std::time::Instant::now();
    for step in 0..8 {
        out.extend(
            stream
                .tick(start + Duration::from_millis(step))
                .iter()
                .map(Line::plain),
        );
    }
    assert_eq!(out, vec![String::new(), "• - one".to_string()]);
}

// ============================================================================
// 6. 도구 셀 — tool-turn 캡처
// ============================================================================

/// 도구 턴 캡처 원본.
fn tool_capture() -> Vec<u8> {
    capture_named("codex-tui-v0.149.1-tool-turn.bin")
}

fn bash(id: &str, command: &str) -> ToolCall {
    ToolCall::new(
        id.to_string(),
        ToolKind::Command {
            command: command.to_string(),
            background: false,
        },
    )
}

fn ok(output: &str) -> Outcome {
    Outcome {
        declined: false,
        ok: true,
        output: output.to_string(),
        change: None,
    }
}

/// 캡처의 접힌 셀 — 초록 bold 불릿 + bold `Ran 2 commands`. 트랜스크립트 힌트
/// (`· ctrl + t to view transcript`)는 오버레이가 없는 이번 라운드에서 뺐다.
#[test]
fn the_collapsed_ran_cell_is_the_captured_one() {
    let capture = tool_capture();
    assert!(
        contains(&capture, "Ran 2 commands".as_bytes()),
        "the capture no longer holds the collapsed cell"
    );
    assert!(
        contains(&capture, b"\x1b[38;5;2;49m\xe2\x80\xa2"),
        "the capture no longer paints the success bullet green"
    );

    let mut group = ToolGroup::new(bash("a", "ls -la"));
    assert!(group.complete("a", ok("total 16")));
    group.push(bash("b", "cat README.md"));
    assert!(group.complete("b", ok("hello")));
    let lines = group.lines(120, Duration::ZERO);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].plain(), "• Ran 2 commands");

    // 5라운드에서 `write_spans` 를 codex 것으로 옮긴 뒤로는 헤더 조각이
    // **캡처 바이트 그대로**다 — 캡처에서 잘라 와 비교한다(트랜스크립트 힌트
    // 이전까지).
    let measured = "\u{1b}[1m\u{1b}[38;5;2;49m•\u{1b}[22m\u{1b}[39;49m \u{1b}[1mRan 2 commands";
    assert!(
        contains(&capture, measured.as_bytes()),
        "the capture no longer frames the collapsed header this way"
    );
    let mut rendered = String::new();
    write_spans(&lines[0], &mut rendered);
    assert_eq!(rendered, measured);
    assert!(
        !rendered.contains("ctrl + t"),
        "no transcript overlay exists this round"
    );
}

/// 캡처의 라이브 셀 — `• Exploring` + `  └ Read README.md`. 항목 이름표는
/// 시안(`ESC[38;5;6;49mRead`).
#[test]
fn the_live_exploring_cell_is_the_captured_one() {
    // 캡처의 두 행(24·25행 프레임):
    //   `ESC[1m ESC[38;2;167;167;167;49m• ESC[22m ESC[39;49m ' ' ESC[1mExploring`
    //   `ESC[2m  └ ESC[22m ESC[38;5;6;49mRead ESC[39;49m ' README.md'`
    // 이름표와 이름 사이에 SGR 이 끼므로 `Read README.md` 는 연속 바이트가
    // 아니다 — 실제로 흐른 그 조각을 그대로 핀한다.
    let capture = tool_capture();
    assert!(
        contains(&capture, b"\x1b[1mExploring"),
        "the capture no longer bolds the live header"
    );
    assert!(
        contains(&capture, b"\x1b[38;5;6;49mRead\x1b[39;49m README.md"),
        "the capture no longer paints the Read label cyan before its name"
    );

    let group = ToolGroup::new(ToolCall::new(
        "a".to_string(),
        ToolKind::Explore(vec![Explored::Read {
            name: "README.md".to_string(),
        }]),
    ));
    let lines = group.lines(120, Duration::ZERO);
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();
    assert_eq!(
        plain,
        vec!["• Exploring".to_string(), "  └ Read README.md".to_string()]
    );

    // 헤더의 낱말은 bold, 목록 이름표는 시안.
    let mut header = String::new();
    write_spans(&lines[0], &mut header);
    assert!(header.contains("\u{1b}[1mExploring"), "header was {header:?}");
    let mut item = String::new();
    write_spans(&lines[1], &mut item);
    // 캡처의 그 행 그대로 — dim 을 끄는 것은 `ESC[22m`, 색 되돌림은
    // `ESC[39;49m` 이다.
    assert_eq!(
        item,
        "\u{1b}[2m  └ \u{1b}[22m\u{1b}[38;5;6;49mRead\u{1b}[39;49m README.md"
    );
}

/// Generic calls retain their semantic target while running, but narrow panes
/// consume one bounded row rather than wrapping raw-looking argument detail.
#[test]
fn a_running_generic_call_is_compact_at_narrow_width_with_cjk_detail() {
    let detail = "select:ToolSearch,CapabilityInvoke — 도구 표시 회귀를 확인하는 매우 긴 검색어";
    let group = ToolGroup::new(ToolCall::new(
        "search".to_string(),
        ToolKind::Call {
            name: "ToolSearch".to_string(),
            detail: detail.to_string(),
        },
    ));

    let lines = group.lines(24, Duration::ZERO);
    assert_eq!(lines.len(), 1, "running generic calls must not grow the viewport");
    assert!(lines[0].plain().starts_with("• Calling ToolSearch"));
    assert!(lines[0].width() <= 24, "line was {:?}", lines[0].plain());

    let lines = group.lines(80, Duration::ZERO);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].plain().contains("ToolSearch select:ToolSearch"));
}

#[test]
fn a_running_agent_keeps_only_the_latest_bounded_progress_rows() {
    let group = ToolGroup::new(ToolCall::new(
        "agent".to_string(),
        ToolKind::Spawn {
            label: "scout".to_string(),
            role: None,
            request: None,
            prompt: String::new(),
            progress: (0..20).map(|index| format!("progress {index}")).collect(),
        },
    ));

    let plain: Vec<String> = group
        .lines(80, Duration::ZERO)
        .iter()
        .map(Line::plain)
        .collect();
    assert_eq!(plain.len(), 6, "header plus the existing output-row budget");
    assert!(!plain.iter().any(|line| line.contains("progress 0")));
    assert!(plain.last().is_some_and(|line| line.contains("progress 19")));
}

/// 커밋된 도구 셀의 접기 마커 — `docs/fold-markers.md` 의 OSC 7788.
/// 헤더 앞 `begin`, 본문 뒤 `end`, 요약은 헤더 문안(퍼센트 인코딩).
#[test]
fn a_committed_tool_cell_carries_the_fold_markers() {
    let mut group = ToolGroup::new(bash("a", "grep -n 'a;b' src"));
    assert!(group.complete("a", ok("src/a.rs:1:a;b")));
    let mut ids = FoldIds::default();
    let rows = group.committed(120, &mut ids, FoldMode::Markers);

    // 앞 빈 줄은 구간 밖 — 접어도 셀 사이의 숨은 남는다.
    assert_eq!(rows[0].plain(), "");
    assert_eq!(rows[0].lead, None);
    assert_eq!(rows[1].plain(), "• Ran grep -n 'a;b' src");
    assert_eq!(
        rows[1].lead.as_deref(),
        Some("\u{1b}]7788;begin;0;collapsed,teaser=1;• Ran grep -n 'a%3Bb' src\u{1b}\\")
    );
    assert_eq!(rows[2].plain(), "  └ … +1 lines");
    assert_eq!(rows[3].plain(), "  └ src/a.rs:1:a;b");
    assert_eq!(rows[3].trail.as_deref(), Some("\u{1b}]7788;end;0\u{1b}\\"));

    // painter 를 통과해도 그 바이트가 그대로 나간다 — 마커를 모르는
    // 터미널에서는 OSC 가 버려지고 본문만 보인다.
    let mut painter = Painter::new(Vec::new(), 120, 40, 12, true);
    painter.set_height(7);
    painter.take_frame();
    painter.insert_history(&rows);
    let frame = painter.take_frame();
    assert!(
        frame.contains("\u{1b}]7788;begin;0;collapsed,teaser=1;• Ran grep -n 'a%3Bb' src\u{1b}\\\u{1b}[39;49m\u{1b}[K"),
        "frame was {frame:?}"
    );
    assert!(frame.contains("\u{1b}[0m\u{1b}]7788;end;0\u{1b}\\"), "frame was {frame:?}");
}

/// The normal codex-compatible road never emits `ZeroCode`'s private OSC,
/// including a single command whose visible result has several rows.
#[test]
fn a_bare_committed_tool_cell_carries_no_fold_markers() {
    let mut group = ToolGroup::new(bash("a", "grep -n 'a;b' src"));
    assert!(group.complete("a", ok("src/a.rs:1:a;b")));
    let mut ids = FoldIds::default();
    let rows = group.committed(120, &mut ids, FoldMode::Bare);

    assert!(rows.iter().all(|row| row.lead.is_none() && row.trail.is_none()));
    let mut painter = Painter::new(Vec::new(), 120, 40, 12, true);
    painter.set_height(7);
    painter.take_frame();
    painter.insert_history(&rows);
    let frame = painter.take_frame();
    assert!(!frame.contains("7788"), "frame was {frame:?}");
    assert_eq!(ids.mint(), 0, "the disabled protocol consumed a fold id");
}

/// Explicit marker mode의 묶음 셀 — `• Ran 2 commands` 가 헤더가 되고,
/// 접힌 명령들의 출력이 그 아래 본문으로 붙는다.
#[test]
fn marker_mode_folds_group_output_into_the_body() {
    let mut group = ToolGroup::new(bash("a", "ls -la"));
    assert!(group.complete("a", ok("total 16")));
    group.push(bash("b", "cat README.md"));
    assert!(group.complete("b", ok("hello")));
    let mut ids = FoldIds::default();
    let rows = group.committed(120, &mut ids, FoldMode::Markers);

    let plain: Vec<String> = rows.iter().map(Line::plain).collect();
    assert_eq!(
        plain,
        vec![
            String::new(),
            "• Ran 2 commands".to_string(),
            // 접힌 동안 보이는 밀도 줄(teaser) — 펼치면 사라지고 본문이 선다.
            "  └ … +4 lines".to_string(),
            "  └ ls -la".to_string(),
            "    total 16".to_string(),
            "  └ cat README.md".to_string(),
            "    hello".to_string(),
        ]
    );

    // 헤더 = begin 뒤 첫 행(teaser 한 줄을 선언), 나머지가 본문 — opt-in renderer가
    // 그대로 접는다.
    assert_eq!(rows[0].lead, None);
    assert_eq!(
        rows[1].lead.as_deref(),
        Some("\u{1b}]7788;begin;0;collapsed,teaser=1;• Ran 2 commands\u{1b}\\")
    );
    assert_eq!(rows[6].trail.as_deref(), Some("\u{1b}]7788;end;0\u{1b}\\"));

    // painter 를 통과한 바이트에도 두 마커가 그대로 실린다.
    let mut painter = Painter::new(Vec::new(), 120, 40, 12, true);
    painter.set_height(7);
    painter.take_frame();
    painter.insert_history(&rows);
    let frame = painter.take_frame();
    assert!(
        frame.contains("\u{1b}]7788;begin;0;collapsed,teaser=1;• Ran 2 commands\u{1b}\\"),
        "frame was {frame:?}"
    );
    assert!(
        frame.contains("\u{1b}]7788;end;0\u{1b}\\"),
        "frame was {frame:?}"
    );
    assert!(frame.contains("total 16") && frame.contains("hello"), "frame was {frame:?}");
    assert!(frame.contains("  └ … +4 lines"), "the teaser row rides the frame: {frame:?}");
}

/// 기본 모드에서는 같은 묶음이 codex 원본 그대로다 — 본문도 마커도 **0 바이트**.
/// 파이프 골든과 같은 모양을 지키는 핀이다.
#[test]
fn default_mode_grouped_cell_emits_no_fold_bytes() {
    let mut group = ToolGroup::new(bash("a", "ls -la"));
    assert!(group.complete("a", ok("total 16")));
    group.push(bash("b", "cat README.md"));
    assert!(group.complete("b", ok("hello")));
    let mut ids = FoldIds::default();
    let rows = group.committed(120, &mut ids, FoldMode::Bare);

    let plain: Vec<String> = rows.iter().map(Line::plain).collect();
    assert_eq!(plain, vec![String::new(), "• Ran 2 commands".to_string()]);
    assert!(rows.iter().all(|row| row.lead.is_none() && row.trail.is_none()));
    // id 도 소비하지 않는다 — 기본에서는 접기 자체가 없는 일이다.
    assert_eq!(ids.mint(), 0);

    let mut painter = Painter::new(Vec::new(), 120, 40, 12, true);
    painter.set_height(7);
    painter.take_frame();
    painter.insert_history(&rows);
    let frame = painter.take_frame();
    assert!(!frame.contains("7788"), "frame was {frame:?}");
    assert!(!frame.contains("total 16"), "frame was {frame:?}");
}

// ============================================================================
// 7. effort 피커 — 사다리 여덟 단
// ============================================================================

/// `/model` 2단계는 forge `Effort::ALL` 여덟 단을 순서 그대로 제공하고, 화면이
/// 허락하는 만큼 창을 써서 여덟 단이 한눈에 보인다 — 세 줄 창에 잘려 "3개씩
/// 밖에 안 보이던" 것(2026-09-02)의 반대. 사용자가 부른 "ultracode" 는
/// `smart` 의 옛 이름이라 설명에 병기한다.
#[test]
fn the_effort_picker_offers_all_eight_rungs_in_the_bounded_window() {
    let rows: Vec<PickerRow> = Effort::ALL
        .iter()
        .map(|effort| PickerRow {
            label: effort.canonical().to_string(),
            description: String::new(),
            dim: false,
        })
        .collect();
    let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(
        labels,
        vec!["off", "low", "medium", "high", "xhigh", "max", "ultra", "smart"]
    );

    let picker = Picker {
        title: "Select Reasoning Effort".to_string(),
        title_style: Style::new().bold(),
        note: "claude-opus-5 — this effort applies to every turn from here on.".to_string(),
        rows,
        selected: 3,
        footer: "Press enter to confirm or esc to go back".to_string(),
    };
    let lines = picker.lines(120, 39);
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();
    // 빈 줄 둘 · 제목 · 안내 · 빈 줄 · 여덟 단 전부 · 빈 줄 · 푸터.
    assert_eq!(lines.len(), 8 + 7);
    assert_eq!(plain[2], "  Select Reasoning Effort");
    assert_eq!(plain[5].trim_end(), "  1. off");
    assert_eq!(plain[8].trim_end(), "› 4. high");
    assert_eq!(plain[12].trim_end(), "  8. smart");
    // 화면이 좁으면 선택을 품은 창만 남는다 — 세 줄이면 2·3·4단.
    let short: Vec<String> = picker.lines(120, 10).iter().map(Line::plain).collect();
    assert_eq!(short.len(), 3 + 7);
    assert_eq!(short[5].trim_end(), "  2. low");
    assert_eq!(short[7].trim_end(), "› 4. high");
    // 고른 줄은 모델 피커와 같은 문법 — `›` 커서에 줄 전체 bold+시안.
    let mut selected = String::new();
    write_spans(&lines[8], &mut selected);
    assert!(
        selected.starts_with("\u{1b}[1m\u{1b}[38;5;6;49m› 4. high"),
        "selected row was {selected:?}"
    );
}

/// 사용자가 찾을 이름은 `ultracode` 다 — 라벨은 `smart`, 설명이 그것을 담는다.
#[test]
fn smart_carries_its_old_ultracode_name() {
    assert_eq!(Effort::from_token("ultracode"), Some(Effort::Smart));
    assert_eq!(Effort::from_token("smart"), Some(Effort::Smart));
    // 원문 꼬리(`+ parallel agent orchestration`)는 zo-ide 가 spawn 을 끄므로
    // 화면에서 뺐다 — 그 판정은 `app::effort_description` 에 산다.
    assert!(Effort::Smart.description().contains("dynamic top band"));
}

// ============================================================================
// 7. `/resume` — resume-picker 캡처
// ============================================================================

/// `/resume` 캡처 원본(이번 라운드에 직접 떴다).
fn resume_capture() -> Vec<u8> {
    capture_named("codex-tui-v0.149.1-resume-picker.bin")
}

/// 캡처의 26.07s 프레임 — 타이핑 중의 한 줄 팝업이다:
/// `ESC[1m ESC[38;5;6;49m /resume  resume a saved chat`. `/model` 팝업과
/// 같은 문법이고 설명은 codex `slash_command.rs:95` 의 문안 그대로다.
#[test]
fn the_resume_popup_row_is_the_captured_one() {
    let capture = resume_capture();
    let measured = "/resume  resume a saved chat";
    assert!(
        contains(&capture, format!("\u{1b}[1m\u{1b}[38;5;6;49m{measured}").as_bytes()),
        "capture no longer holds the /resume popup row"
    );

    let popup = Popup {
        rows: slash::matches("/resume")
            .into_iter()
            .map(|command| PopupRow {
                name: command.name().to_string(),
                description: command.description().to_string(),
            })
            .collect(),
        selected: 0,
    };
    let lines = popup.lines(120);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].plain(), format!("  {measured}"));
    let mut rendered = String::new();
    write_spans(&lines[0], &mut rendered);
    assert_eq!(rendered, format!("\u{1b}[1m\u{1b}[38;5;6;49m  {measured}"));
}

/// 캡처의 29.19s 프레임 — codex 의 `/resume` 화면을 옮긴 것.
///
/// 원본은 alt-screen 오버레이(`ESC[?1049h`)에 얹은 `resume_picker.rs`(6,911줄)
/// 이고 zo 에는 alt-screen 도 pager 도 없어서(`tui` 모듈 머리말의 화면 관리
/// 계약) 같은 문법을 **뷰포트**에 앉혔다. 무엇을 옮기고 무엇을 안 옮겼는지는
/// `tui/sessions.rs` 머리말의 표에 있다.
///
/// 여기 기대값은 손으로 적은 상수가 아니다 — 캡처 안에서 다시 찾아 우리 것과
/// 견준다.
#[test]
fn the_resume_picker_chrome_is_the_captured_one() {
    let capture = resume_capture();
    // 1) 캡처가 아직 이 문법을 들고 있는가.
    for measured in [
        // 제목: bold + 시안.
        "\u{1b}[1m\u{1b}[38;5;6;49mResume a previous session",
        // 검색 자리표시자, 그리고 그 오른쪽 툴바가 3행 46열에서 시작한다.
        "Type to search\u{1b}[3;46HFilter: ",
        // 초점이 간 켜진 값은 magenta, 꺼진 값은 dim 한 ` label `.
        "\u{1b}[22m\u{1b}[38;5;5;49m[Cwd]\u{1b}[2m\u{1b}[39;49m All    ",
        // 초점이 없는 켜진 값은 색 없이 굵기만 벗는다.
        "Sort: \u{1b}[22m[Updated]\u{1b}[2m Created",
        // 진행 구분선: 대시들 · 라벨 · 대시 하나.
        "\u{2500}\u{2500}\u{2500} 0 / 0\u{2026} \u{b7} 0% \u{2500}",
        // 힌트 두 줄: 앞 한 칸, 키는 굵기를 벗고, 낱말은 dim, 사이는 세 칸.
        "\u{1b}[22menter\u{1b}[2m resume   \u{1b}[22mesc\u{1b}[2m exit   ",
        "\u{1b}[22mctrl+c\u{1b}[2m exit   \u{1b}[22mtab\u{1b}[2m focus sort/fil",
        "\u{1b}[22mctrl+o\u{1b}[2m comfortable view   ",
        "\u{1b}[22m\u{2191}/\u{2193}\u{1b}[2m browse",
    ] {
        assert!(
            contains(&capture, measured.as_bytes()),
            "capture no longer holds {measured:?}"
        );
    }

    // 2) 우리 화면.
    let now = 10_000_000_000u128;
    let picker = sessions::SessionPicker::new(
        vec![
            sessions::SessionRow {
                id: "session-9999999880000-0".to_string(),
                cwd: Some(std::path::PathBuf::from("/w/one")),
                preview: "fix the parser".to_string(),
                created_millis: 9_999_999_880_000,
                updated_millis: now - 120_000,
                held_by: None,
            },
            sessions::SessionRow {
                id: "session-9999989200000-1".to_string(),
                cwd: Some(std::path::PathBuf::from("/w/one")),
                preview: "port the picker".to_string(),
                created_millis: 9_999_989_200_000,
                updated_millis: now - 10_800_000,
                held_by: None,
            },
        ],
        Some(std::path::PathBuf::from("/w/one")),
        now,
    );
    let lines = picker.lines(120, 40);
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();

    assert_eq!(plain[0], "");
    assert_eq!(plain[1], "");
    assert_eq!(plain[2], "  Resume a previous session");
    assert_eq!(plain[3], "");
    // 검색은 왼쪽, 툴바는 오른쪽. `Status` 컨트롤이 없으므로 codex 의 46열보다
    // 그만큼(29칸) 오른쪽에서 시작한다 — 같은 계산의 다른 입력이다.
    assert_eq!(
        plain[4],
        format!(
            "  Type to search{}Filter: [Cwd] All    Sort: [Updated] Created ",
            " ".repeat(59)
        )
    );
    assert_eq!(plain[5], "");
    // 행: 마커 · 상대 시각(12칸 열) · 첫 프롬프트 — codex `dense_summary_line`.
    assert!(plain[6].starts_with("  ❯ 2m ago      fix the parser"), "{:?}", plain[6]);
    assert!(plain[7].starts_with("    3h ago      port the picker"), "{:?}", plain[7]);

    // 구분선과 힌트 두 줄이 마지막 셋이다.
    let tail = &plain[plain.len() - 3..];
    assert!(tail[0].ends_with(" 1 / 2 · 100% ─"), "{:?}", tail[0]);
    assert!(tail[0].starts_with("──────"), "{:?}", tail[0]);
    assert_eq!(
        tail[1],
        " enter resume   esc exit   ctrl+c exit   tab focus sort/filter   ←/→ change option"
    );
    assert_eq!(tail[2], " ctrl+o comfortable view   ↑/↓ browse");

    // 제목 바이트는 캡처의 그것이다(앞의 두 칸은 목록 들여쓰기).
    let mut title = String::new();
    write_spans(&lines[2], &mut title);
    assert_eq!(
        title,
        "  \u{1b}[1m\u{1b}[38;5;6;49mResume a previous session"
    );

    // 툴바 바이트: 초점이 간 `[Cwd]` 는 magenta, 꺼진 값은 dim 한 ` label `,
    // 초점이 없는 `[Updated]` 는 색 없이 — 캡처의 세 갈래 그대로.
    let mut toolbar = String::new();
    write_spans(&lines[4], &mut toolbar);
    assert!(toolbar.contains("\u{1b}[2mFilter: \u{1b}[22m\u{1b}[38;5;5;49m[Cwd]"), "{toolbar:?}");
    // 꺼진 값은 dim 한 ` label ` 이고 magenta 는 그 앞에서 닫힌다 — 캡처의
    // `[Cwd]ESC[2mESC[39;49m All    Status:` 와 같은 이음매다.
    assert!(toolbar.contains("[Cwd]\u{1b}[2m\u{1b}[39;49m All    "), "{toolbar:?}");
    // 이 조각은 캡처 안의 것과 **바이트가 같다**(위 1) 에서 찾은 그것).
    assert!(toolbar.contains("Sort: \u{1b}[22m[Updated]\u{1b}[2m Created"), "{toolbar:?}");
    // 캡처의 그 자리와 나란히 놓으면 `Status` 컨트롤 하나만 빠져 있다.
    assert!(
        contains(
            &capture,
            "Filter: \u{1b}[22m\u{1b}[38;5;5;49m[Cwd]\u{1b}[2m\u{1b}[39;49m All    Status: "
                .as_bytes()
        ),
        "capture no longer holds the three-control toolbar this one reduces"
    );
}

/// 검색·필터·정렬·밀도가 실제로 행을 바꾼다.
///
/// 크롬만 그리고 아무 일도 하지 않는 툴바는 옮긴 것이 아니라 그린 것이다.
#[test]
fn the_resume_picker_controls_change_the_rows() {
    let now = 10_000_000_000u128;
    let rows = vec![
        // 이 워크스페이스의 세션. 오래 살아서 생성과 수정이 멀다.
        sessions::SessionRow {
            id: "session-9000000000000-0".to_string(),
            cwd: Some(std::path::PathBuf::from("/w/here")),
            preview: "long lived session".to_string(),
            created_millis: 9_000_000_000_000,
            updated_millis: now - 1_000,
            held_by: None,
        },
        // 남의 워크스페이스. 방금 만들어졌다.
        sessions::SessionRow {
            id: "session-9999999000000-0".to_string(),
            cwd: Some(std::path::PathBuf::from("/w/elsewhere")),
            preview: "another tree".to_string(),
            created_millis: 9_999_999_000_000,
            updated_millis: now - 2_000,
            held_by: None,
        },
    ];
    let mut picker = sessions::SessionPicker::new(
        rows,
        Some(std::path::PathBuf::from("/w/here")),
        now,
    );

    // 기본은 `Filter: [Cwd]` — 이 트리의 세션 하나만.
    assert_eq!(picker.selected_id(), Some("session-9000000000000-0"));
    assert_eq!(picker.visible_ids(), vec!["session-9000000000000-0"]);

    // ←/→ 가 `[All]` 로 넘기면 남의 트리도 보인다. 정렬은 Updated 라
    // 최근에 쓴 것이 위다.
    picker.change_option();
    assert_eq!(
        picker.visible_ids(),
        vec!["session-9000000000000-0", "session-9999999000000-0"]
    );

    // tab 이 Sort 로 초점을 옮기고 ←/→ 가 Created 로 바꾸면 순서가 뒤집힌다 —
    // 오래 산 세션은 **만들어진** 것이 훨씬 이르다. 정렬이 정말 다른 축이라는
    // 증거이고, 그 축은 트랜스크립트 머리글의 `created_at_ms` 다.
    picker.focus_next();
    picker.change_option();
    assert_eq!(
        picker.visible_ids(),
        vec!["session-9999999000000-0", "session-9000000000000-0"]
    );
    // 보던 세션은 그대로 따라간다(codex `clear_query_preserving_selection`).
    assert_eq!(picker.selected_id(), Some("session-9000000000000-0"));

    // 타이핑은 검색으로 간다 — 숫자도 마찬가지다(예전의 번호 선택은 없다).
    for ch in "elsewhere".chars() {
        picker.push_char(ch);
    }
    assert_eq!(picker.visible_ids(), vec!["session-9999999000000-0"]);
    assert!(picker.has_query());
    // cwd 로도 걸린다 — codex `matches_query` 의 축 하나다.
    picker.clear_query();
    assert!(!picker.has_query());
    assert_eq!(picker.visible_ids().len(), 2);

    // ctrl+o 는 행 높이를 바꾼다: dense 는 행마다 한 줄,
    // comfortable 은 제목 줄 + 메타 줄 + 행 사이 빈 줄.
    let dense = picker.lines(120, 40);
    assert!(dense[6].plain().contains("another tree"), "{:?}", dense[6].plain());
    assert!(
        dense[7].plain().contains("long lived session"),
        "{:?}",
        dense[7].plain()
    );

    picker.toggle_density();
    let comfortable = picker.lines(120, 40);
    let title = comfortable[6].plain();
    assert!(title.trim_end().ends_with("another tree"), "{title:?}");
    // 메타 줄에는 상대 시각과, `[All]` 이므로 cwd 가 온다.
    let meta = comfortable[7].plain();
    assert!(meta.contains('⌁') && meta.contains("/w/elsewhere"), "{meta:?}");
    assert_eq!(comfortable[8].plain(), "", "행 사이 빈 줄");
    assert!(
        comfortable[9].plain().trim_end().ends_with("long lived session"),
        "{:?}",
        comfortable[9].plain()
    );
    // 힌트도 따라 뒤집힌다.
    let hints = comfortable[comfortable.len() - 1].plain();
    assert!(hints.starts_with(" ctrl+o dense view"), "{hints:?}");
}

// ============================================================================
// 8. 표 — table 캡처
// ============================================================================

/// 표 답변 캡처 원본. 드라이버는 `docs/captures/pty-driver-table.py` 이고
/// 프롬프트는 아래 [`TABLE_SOURCE`] 를 그대로 출력하라는 것이었다 — 같은 원문을
/// 양쪽에 넣어야 바이트를 나란히 놓을 수 있다.
fn table_capture() -> Vec<u8> {
    capture_named("codex-tui-v0.149.1-table.bin")
}

/// 캡처를 뜬 그 표. 3열 4행이다.
const TABLE_SOURCE: &str = concat!(
    "| Name | Count | Status |\n",
    "| --- | --- | --- |\n",
    "| alpha | 1 | ok |\n",
    "| beta | 22 | pending |\n",
    "| gamma | 333 | failed |\n",
    "| delta | 4444 | ok |\n",
);

/// 히스토리 한 행을 codex `write_history_line` 문법으로 — 줄 색으로 시작하는
/// `SetColors`, `ESC[K`, 스팬, 그리고 되돌림 셋.
fn history_row(line: &Line) -> String {
    let mut out = zo_ide::tui::ansi::line_lead(line);
    out.push_str("\u{1b}[K");
    write_spans(line, &mut out);
    out.push_str("\u{1b}[39m\u{1b}[49m\u{1b}[0m");
    out
}

/// 캡처의 커밋된 표 — 히스토리로 내려간 여섯 행 전부가 바이트까지 같다.
///
/// 헤더 행이 이 라운드의 새 문법을 셋 보여 준다: (1) 행 머리의 `SetColors` 가
/// **줄 스타일**의 색을 낸다(`ESC[38;2;249;226;175;49m`), (2) 그래서 스팬 차분의
/// 시작 상태(`Reset`)와 어긋나 첫 스팬에서 같은 색이 한 번 더 나간다,
/// (3) `• ` 마커는 줄 스타일을 `patch` 해 bold+dim 이 되고 다음 스팬이 dim 만
/// 끄므로 `ESC[22m` 이 **한 번**만 나간다(codex `ModifierDiff::queue`).
#[test]
fn the_committed_table_is_the_captured_grid() {
    let capture = table_capture();
    let measured = [
        "\u{1b}[38;2;249;226;175;49m\u{1b}[K\u{1b}[1m\u{1b}[2m\u{1b}[38;2;249;226;175;49m• \u{1b}[22m Name     Count    Status\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K  \u{1b}[2m━━━━━━━  ━━━━━━━  ━━━━━━━━━\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K   alpha    1        ok\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K  \u{1b}[2m───────  ───────  ─────────\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K   beta     22       pending\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K  \u{1b}[2m───────  ───────  ─────────\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K   gamma    333      failed\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K  \u{1b}[2m───────  ───────  ─────────\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K   delta    4444     ok\u{1b}[39m\u{1b}[49m\u{1b}[0m",
    ];
    for row in measured {
        assert!(
            contains(&capture, row.as_bytes()),
            "capture no longer holds {row:?}"
        );
    }

    // 캡처는 120열 PTY 다.
    let mut stream = MarkdownStream::answer(120);
    stream.push(TABLE_SOURCE);
    let rows: Vec<String> = stream.finish().iter().map(history_row).collect();
    // 첫 행은 답변 셀을 여는 빈 줄이다(캡처도 그렇다).
    assert_eq!(rows[0], "\u{1b}[39;49m\u{1b}[K\u{1b}[39m\u{1b}[49m\u{1b}[0m");
    assert_eq!(&rows[1..], &measured[..]);
}

/// 홀드백 중의 꼬리 — 캡처의 뷰포트 프레임(12.6s·12.8s·13.0s)은 본문 행이
/// 하나씩 늘 때마다 표 **전체**를 활성 셀 칸에 다시 그리고, 히스토리에는 한 행도
/// 내려보내지 않는다. codex `active_tail_budget_lines` 가 표 시작 오프셋부터를
/// 꼬리로 붙잡기 때문이다 — 이 라운드에 처음으로 0 이 아닌 값을 낸다.
///
/// 캡처에서 그대로 가져오는 것: 두 번째 본문 행까지 온 프레임의 꼬리 다섯 줄
/// (헤더·굵은 선·alpha·가는 선·beta)과 그 열 폭이 alpha 행만 있던 프레임보다
/// 넓어진 것(`Status` 열이 7→9 로 다시 잡힌다).
#[test]
fn a_streaming_table_lives_in_the_tail_until_it_finishes() {
    let capture = table_capture();
    // 캡처의 꼬리 프레임 둘 — 열 폭이 행이 늘며 다시 잡힌 증거다.
    for measured in [
        "   alpha    1        ok\u{1b}[32;1H  \u{1b}[2m───────  ───────  ─────────",
        "   beta     22       pending",
    ] {
        assert!(
            contains(&capture, measured.as_bytes()),
            "capture no longer holds {measured:?}"
        );
    }

    let mut stream = MarkdownStream::answer(120);
    let mut committed: Vec<String> = Vec::new();
    let start = std::time::Instant::now();
    let mut step = 0u64;
    for source_line in TABLE_SOURCE.split_inclusive('\n') {
        stream.push(source_line);
        for _ in 0..8 {
            step += 1;
            committed.extend(
                stream
                    .tick(start + Duration::from_millis(step))
                    .iter()
                    .map(Line::plain),
            );
        }
    }
    // 표가 열려 있는 동안 스크롤백은 한 행도 받지 않는다.
    assert!(
        committed.is_empty(),
        "a live table must not reach scrollback: {committed:?}"
    );

    // 꼬리는 지금 렌더의 표 전부다 — 마커까지 붙어 있다.
    let tail: Vec<String> = stream.tail().iter().map(Line::plain).collect();
    assert_eq!(
        tail,
        vec![
            "•  Name     Count    Status".to_string(),
            "  ━━━━━━━  ━━━━━━━  ━━━━━━━━━".to_string(),
            "   alpha    1        ok".to_string(),
            "  ───────  ───────  ─────────".to_string(),
            "   beta     22       pending".to_string(),
            "  ───────  ───────  ─────────".to_string(),
            "   gamma    333      failed".to_string(),
            "  ───────  ───────  ─────────".to_string(),
            "   delta    4444     ok".to_string(),
        ]
    );

    // 꼬리 첫 줄의 바이트는 커밋될 줄의 바이트와 같다 — 같은 렌더 경로다.
    let mut head = String::new();
    write_spans(&stream.tail()[0], &mut head);
    assert_eq!(
        head,
        "\u{1b}[1m\u{1b}[2m\u{1b}[38;2;249;226;175;49m• \u{1b}[22m Name     Count    Status"
    );

    // 끝나면 한 번에 내려간다.
    let flushed: Vec<String> = stream.finish().iter().map(Line::plain).collect();
    assert_eq!(flushed.len(), 10, "the opening blank plus nine table rows");
    assert!(stream.tail().is_empty());
}

/// 표가 없는 스트림의 꼬리는 여전히 **0 바이트**다 — 홀드백이 들어와도
/// `TableHoldbackState::None` 이면 예산이 0 이고 확정 렌더 전부가 큐로 간다
/// (codex `active_tail_budget_lines`). 활성 셀 칸은 `Working` 의 것이다.
///
/// "표가 없다" 는 **파이프 모양 줄이 하나도 없다** 는 뜻이다 — codex 의
/// `PendingHeader` 는 헤더처럼 보이는 줄이면 구분 줄이 올지 모른 채로 붙잡아
/// 두므로(`is_table_header_line` 은 바깥 파이프 없는 `A | B` 도 받는다),
/// 파이프 하나가 섞인 산문은 그 자리에서 꼬리를 갖는 것이 원본의 모양이다.
#[test]
fn a_table_less_stream_still_paints_an_empty_tail() {
    let mut stream = MarkdownStream::answer(120);
    for delta in [
        "### 자기소개\n",
        "\n",
        "- 저는 개발자입니다\n",
        "- 복잡한 내용을 쉽게 설명하는 것을 좋아합니다\n",
    ] {
        stream.push(delta);
        let mut rendered = String::new();
        for line in stream.tail() {
            rendered.push_str(&history_row(&line));
        }
        assert_eq!(rendered, "", "the tail must stay empty after {delta:?}");
    }
    // 큐를 다 흘려도 그대로다.
    let start = std::time::Instant::now();
    for step in 0..32 {
        stream.tick(start + Duration::from_millis(step));
    }
    assert!(stream.tail().is_empty());
}


// ============================================================================
// 9. 코드 담장 — 문법 강조와 폴백
// ============================================================================

fn codeblock_capture() -> Vec<u8> {
    capture_named("codex-tui-v0.149.1-codeblock.bin")
}

/// 캡처를 뜬 그 답변 — 아는 언어 담장 하나와 모르는 언어 담장 하나.
const CODEBLOCK_SOURCE: &str = concat!(
    "```rust\n",
    "fn main() {\n",
    "    let name = \"zo\";\n",
    "    println!(\"hello {name}\");\n",
    "}\n",
    "```\n",
    "\n",
    "```nosuchlang\n",
    "plain fallback line\n",
    "```\n",
);

/// 캡처의 커밋된 담장 여섯 행이 바이트까지 같다.
///
/// 아는 언어(`rust`)는 codex `markdown_render.rs::end_codeblock` 이 본문을
/// 모아 `render/highlight.rs::highlight_code_to_lines` 로 넘긴 결과다 — 색은
/// syntect 가 catppuccin-mocha 테마에서 뽑은 트루컬러이고(`ESC[38;2;…;49m`),
/// 배경·italic·underline 은 원본이 일부러 버린다. 모르는 언어
/// (`nosuchlang`)는 같은 함수의 폴백이라 색 없는 평문 한 줄이고, 꼬리 개행이
/// 만드는 유령 빈 줄이 **없다**(원본이 `split('\n')` 이 아니라 `lines()` 를
/// 쓴다).
#[test]
fn the_committed_code_fence_is_the_captured_highlighting() {
    let capture = codeblock_capture();
    let measured = [
        "\u{1b}[39;49m\u{1b}[K\u{1b}[2m• \u{1b}[22m\u{1b}[38;2;203;166;247;49mfn\u{1b}[38;2;205;214;244;49m \u{1b}[38;2;137;180;250;49mmain\u{1b}[38;2;147;153;178;49m()\u{1b}[38;2;205;214;244;49m \u{1b}[38;2;147;153;178;49m{\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K  \u{1b}[38;2;205;214;244;49m    \u{1b}[38;2;203;166;247;49mlet\u{1b}[38;2;205;214;244;49m name \u{1b}[38;2;148;226;213;49m=\u{1b}[38;2;205;214;244;49m \u{1b}[38;2;166;227;161;49m\"zo\"\u{1b}[38;2;147;153;178;49m;\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K  \u{1b}[38;2;205;214;244;49m    \u{1b}[38;2;137;180;250;49mprintln!\u{1b}[38;2;147;153;178;49m(\u{1b}[38;2;166;227;161;49m\"hello \u{1b}[38;2;245;194;231;49m{name}\u{1b}[38;2;166;227;161;49m\"\u{1b}[38;2;147;153;178;49m);\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K  \u{1b}[38;2;147;153;178;49m}\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[K  plain fallback line\u{1b}[39m\u{1b}[49m\u{1b}[0m",
    ];
    for row in measured {
        assert!(
            contains(&capture, row.as_bytes()),
            "capture no longer holds {row:?}"
        );
    }

    let mut stream = MarkdownStream::answer(120);
    stream.push(CODEBLOCK_SOURCE);
    let rows: Vec<String> = stream.finish().iter().map(history_row).collect();
    // 첫 행은 답변 셀을 여는 빈 줄이다(캡처의 32행).
    assert_eq!(rows[0], "\u{1b}[39;49m\u{1b}[K\u{1b}[39m\u{1b}[49m\u{1b}[0m");
    assert_eq!(&rows[1..], &measured[..]);
}

// ============================================================================
// 10. 세션 요약 두 줄
// ============================================================================

fn resume_summary_capture() -> Vec<u8> {
    capture_named("codex-tui-v0.149.1-resume-summary.bin")
}

/// 캡처와 같은 계수를 우리 어휘로. codex 는 `input_tokens` 에 캐시분을
/// 포함시키고 우리는 나눠 세므로, non-cached 7,605 · cached 9,984 · output 22
/// 로 놓으면 같은 줄이 나온다.
fn captured_usage() -> core_types::usage::TokenUsage {
    core_types::usage::TokenUsage {
        input_tokens: 7_605,
        output_tokens: 22,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 9_984,
        output_tokens_details: None,
    }
}

fn written_transcript(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("transcript.jsonl");
    std::fs::write(&path, b"{}\n").expect("write transcript");
    path
}

/// 재개가 세션을 갈아 끼운 뒤 히스토리로 들어가는 두 줄.
///
/// 실측(`…-resume-summary.bin` 84~86행): 앞 빈 줄 하나, 그 다음 두 줄이
/// **접두어 없이** 0열에서 시작한다(codex `PlainHistoryCell`). 명령 토큰은
/// 캡처의 히스토리는 Indexed cyan이지만 zo는 #20 내부 통일로 종료 경로와
/// 같은 base cyan을 쓴다. 화면색은 같고 명령 토큰의 내부 어휘만 하나다.
#[test]
fn the_resume_summary_rows_are_the_captured_ones() {
    let capture = resume_summary_capture();
    let measured = [
        "\u{1b}[39;49m\u{1b}[KToken usage: total=7,627 input=7,605 (+ 9,984 cached) output=22 (reasoning 15)\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "\u{1b}[39;49m\u{1b}[KTo continue this session, run \u{1b}[38;5;6;49mcodex resume 01a0406b-c6ec-72d1-957b-511f138cd908\u{1b}[39m\u{1b}[49m\u{1b}[0m",
    ];
    for row in measured {
        assert!(
            contains(&capture, row.as_bytes()),
            "capture no longer holds {row:?}"
        );
    }

    let dir = tempfile::tempdir().expect("temp dir");
    let transcript = written_transcript(dir.path());
    let summary = session_summary(
        captured_usage(),
        "01a0406b-c6ec-72d1-957b-511f138cd908",
        &transcript,
        None,
    )
    .expect("summary");
    let rows: Vec<String> = cells::plain_cell(&summary.history_lines(), 120)
        .iter()
        .map(history_row)
        .collect();

    // 앞 빈 줄 하나(캡처 84행)와 두 줄.
    assert_eq!(rows[0], "\u{1b}[39;49m\u{1b}[K\u{1b}[39m\u{1b}[49m\u{1b}[0m");
    // 우리는 reasoning 토큰을 따로 세지 않으므로 그 꼬리만 없다.
    assert_eq!(rows[1], measured[0].replace(" (reasoning 15)", ""));
    // 명령 문안은 우리 것이고, 색 어휘는 종료 경로와 같은 base cyan이다.
    assert_eq!(
        rows[2],
        measured[1]
            .replace("\u{1b}[38;5;6;49m", "\u{1b}[36;49m")
            .replace("codex resume ", "zo --resume ")
    );
    assert_eq!(rows.len(), 3);
}

/// 프로세스가 끝날 때 stdout 으로 나가는 두 줄 — 화면 밖이라 색 문법이 다르다
/// (`ESC[36m…ESC[39m`, codex `main.rs::format_exit_messages`).
#[test]
fn the_exit_summary_lines_are_the_captured_ones() {
    let measured = "To continue this session, run \u{1b}[36mcodex resume 01a03d06-be3b-7980-88a0-ba4fdc91b49e\u{1b}[39m";
    assert!(
        contains(&capture(), measured.as_bytes()),
        "capture no longer holds {measured:?}"
    );

    let dir = tempfile::tempdir().expect("temp dir");
    let transcript = written_transcript(dir.path());
    let summary = session_summary(
        captured_usage(),
        "01a03d06-be3b-7980-88a0-ba4fdc91b49e",
        &transcript,
        None,
    )
    .expect("summary");

    let coloured = summary.exit_lines(/*color*/ true);
    assert_eq!(
        coloured,
        vec![
            "Token usage: total=7,627 input=7,605 (+ 9,984 cached) output=22".to_string(),
            measured.replace("codex resume ", "zo --resume "),
        ]
    );
    // `NO_COLOR` 갈래는 같은 문안에서 이스케이프만 빠진다.
    assert_eq!(
        summary.exit_lines(/*color*/ false)[1],
        "To continue this session, run zo --resume 01a03d06-be3b-7980-88a0-ba4fdc91b49e"
    );
}

// ============================================================================
// 11. 부팅 카드 경로의 가운데 자르기
// ============================================================================

/// 캡처의 `directory:` 행은 오른쪽이 아니라 **가운데**를 잃었다 — codex
/// `history_cell/session.rs::format_directory_inner` 가 안쪽 폭에서 라벨
/// 접두어(11칸)를 뺀 45칸을 넘는 경로를
/// `text_formatting::center_truncate_path` 로 넘기기 때문이다.
#[test]
fn the_boot_card_path_loses_its_middle_like_the_capture() {
    let measured = "directory: \u{1b}[22m/private/tmp/\u{2026}/codex-tui-capture/work";
    assert!(
        contains(&capture(), measured.as_bytes()),
        "capture no longer holds {measured:?}"
    );

    // 캡처와 같은 모양의 긴 경로 — 45칸을 넘는다.
    let card = cells::boot_card(
        "zo",
        "0.1.0",
        "claude-opus-5",
        "smart",
        "/private/tmp/zerocode-capture-scratch/zo-tui-capture/work",
        120,
    );
    // 캡처와 같은 자리·같은 모양으로 잘린다: 머리 두 조각이 남고, 가운데가
    // `…` 하나로 접히고, 꼬리 두 조각(폴더 이름 포함)이 산다.
    let dir_row = card[4].plain();
    assert!(
        dir_row.contains("directory: /private/tmp/\u{2026}/zo-tui-capture/work"),
        "the card row was {dir_row:?}"
    );
}

// ============================================================================
// 12. 증분 렌더 — 흘려 넣은 것과 한 번에 넣은 것의 바이트가 같다
// ============================================================================

/// 답변 하나에 마크다운의 "뒤가 앞을 바꾸는" 문법을 모아 둔 것. 스트림이
/// 확정 접두어를 보존한 채 꼬리만 다시 렌더해도 이 원문의 바이트는 그대로여야
/// 한다 — 리스트 번호(빈 줄로 갈려도 한 리스트다)·담장(빈 줄과 파이프를
/// 삼킨다)·표(행이 늘면 열 폭이 다시 잡힌다)가 각각 한 자리씩 있다.
const INCREMENTAL_SOURCE: &str = concat!(
    "### 자기소개\n",
    "\n",
    "저는 개발자입니다. 복잡한 내용을 쉽게 설명하는 것을 좋아합니다.\n",
    "\n",
    "1. 하나\n",
    "\n",
    "1. 둘\n",
    "\n",
    "```rust\n",
    "fn main() {\n",
    "\n",
    "    let a = 1 | 2;\n",
    "}\n",
    "```\n",
    "\n",
    "| Name | Count |\n",
    "| --- | --- |\n",
    "| alpha | 1 |\n",
    "| beta | 22 |\n",
    "\n",
    "마지막 문단입니다.\n",
);

/// 한 줄씩 흘려 넣어 히스토리로 내려간 바이트는, 같은 원문을 한 번에 넣어
/// 렌더한 바이트와 **같다**.
///
/// 스트리밍 렌더가 증분이 된 뒤로 이것이 계약이다: 확정된 접두어의 렌더 결과를
/// 보존하고 가변 꼬리만 다시 그리되, 화면에 나가는 바이트는 누적 원문을 통째로
/// 렌더한 것과 한 글자도 다르지 않아야 한다. 표처럼 지난 행을 다시 잡는 구역은
/// 홀드백 경계부터 통째로 다시 렌더되므로 여기서도 같은 값이 나온다.
#[test]
fn a_streamed_answer_renders_the_same_bytes_as_a_single_shot_answer() {
    let drain = |stream: &mut MarkdownStream, rows: &mut Vec<String>| {
        let start = std::time::Instant::now();
        for step in 0..64 {
            rows.extend(
                stream
                    .tick(start + Duration::from_millis(step))
                    .iter()
                    .map(history_row),
            );
        }
    };

    // 한 줄씩 — 실제 스트림이 개행마다 커밋하는 그 길.
    let mut streamed: Vec<String> = Vec::new();
    let mut stream = MarkdownStream::answer(120);
    for line in INCREMENTAL_SOURCE.split_inclusive('\n') {
        stream.push(line);
        drain(&mut stream, &mut streamed);
    }
    streamed.extend(stream.finish().iter().map(history_row));
    assert!(stream.tail().is_empty(), "끝나면 활성 칸은 빈다");

    // 한 번에 — 접두어 보존이 낄 자리가 없는 길.
    let mut once = MarkdownStream::answer(120);
    once.push(INCREMENTAL_SOURCE);
    let single: Vec<String> = once.finish().iter().map(history_row).collect();

    assert_eq!(streamed, single);
    // 그 바이트가 실제로 무엇인지도 못 박는다. 표 헤더 행의 머리는 **줄 색**
    // (`ESC[38;2;249;226;175;49m`)이고, 이 셀에서는 `• ` 마커를 헤딩이 이미
    // 가져갔으므로 접두어가 색 없는 두 칸이라 스팬 차분이 bold 를 먼저 낸다.
    assert_eq!(
        streamed[0],
        "\u{1b}[39;49m\u{1b}[K\u{1b}[39m\u{1b}[49m\u{1b}[0m",
        "답변 셀은 빈 줄 하나로 열린다"
    );
    assert_eq!(
        streamed[1],
        "\u{1b}[39;49m\u{1b}[K\u{1b}[2m• \u{1b}[22m\u{1b}[1m\u{1b}[3m### 자기소개\u{1b}[39m\u{1b}[49m\u{1b}[0m"
    );
    let table_header = streamed
        .iter()
        .find(|row| row.contains("Name"))
        .expect("표 헤더 행");
    assert_eq!(
        table_header,
        "\u{1b}[38;2;249;226;175;49m\u{1b}[K\u{1b}[1m\u{1b}[38;2;249;226;175;49m   Name     Count\u{1b}[39m\u{1b}[49m\u{1b}[0m"
    );
}

/// 폭이 도중에 바뀌어도 같다 — 경계는 원문 좌표라 살아남고, 확정 접두어만 새
/// 폭으로 한 번 다시 선다(codex `streaming/controller.rs::set_width`).
///
/// 여기서 재는 것은 **꼬리와 남은 히스토리**다: 폭을 바꾼 뒤 끝까지 흘린
/// 결과가, 처음부터 새 폭이었던 스트림의 같은 구간과 바이트까지 같다.
#[test]
fn a_resize_mid_stream_lands_on_the_new_width_bytes() {
    let lines: Vec<&str> = INCREMENTAL_SOURCE.split_inclusive('\n').collect();
    let mut resized = MarkdownStream::answer(60);
    for line in &lines {
        resized.push(line);
    }
    resized.set_width(120);
    let after_resize: Vec<String> = resized.finish().iter().map(history_row).collect();

    let mut native = MarkdownStream::answer(120);
    for line in &lines {
        native.push(line);
    }
    let native_rows: Vec<String> = native.finish().iter().map(history_row).collect();

    assert_eq!(after_resize, native_rows);
}

// ============================================================================
// 13. `/status` 사용량 카드 — codex 0.150.1 의 문법
// ============================================================================

/// 카드가 흐른 캡처. 120x40 PTY 에서 `/status` 를 친 화면이고, 재현 드라이버는
/// `docs/captures/pty-driver-codex-status.py` 다.
fn status_capture() -> Vec<u8> {
    capture_named("codex-tui-v0.150.1-status.bin")
}

/// 게이지는 스무 칸이고 **채운 칸이 남은 양**이다.
///
/// 캡처의 세 줄이 그 방향과 반올림을 함께 못 박는다: 93% 가 19칸(내림이면 18),
/// 52% 가 10칸(올림이면 11), 5% 가 1칸이다. 채움을 반대로 읽으면 한도가 바닥일
/// 때 게이지가 가득 차 보인다.
#[test]
fn the_status_gauge_fills_what_is_left_like_the_capture() {
    for (percent, measured) in [
        (5u8, "[█░░░░░░░░░░░░░░░░░░░] 5% left"),
        (93, "[███████████████████░] 93% left"),
        (52, "[██████████░░░░░░░░░░] 52% left"),
    ] {
        assert!(
            contains(&status_capture(), measured.as_bytes()),
            "capture no longer holds {measured:?}"
        );
        assert_eq!(format!("{} {percent}% left", status_format::gauge(percent)), measured);
    }
    assert_eq!(status_format::GAUGE_CELLS, 20);
}

/// 리셋 문구는 같은 날이면 시각만, 다른 날이면 `on D Mon` 을 붙인다.
/// 캡처에는 두 모양이 나란히 있다 — 5시간 창은 그날 저녁, 주간 창은 다음 달이다.
#[test]
fn the_status_reset_phrase_matches_the_capture() {
    let capture = status_capture();
    for measured in [" (resets 20:11)", " (resets 20:22 on 1 Sep)", " (resets 01:02 on 2 Sep)"] {
        assert!(
            contains(&capture, measured.as_bytes()),
            "capture no longer holds {measured:?}"
        );
    }
    // 캡처를 뜬 순간(2026-08-27 17:25 UTC)을 기준으로 같은 세 문구가 나온다.
    let now = 1_787_851_500;
    assert_eq!(status_format::format_reset_at(1_787_861_460, now, 0), "resets 20:11");
    assert_eq!(
        status_format::format_reset_at(1_788_294_120, now, 0),
        "resets 20:22 on 1 Sep"
    );
    assert_eq!(
        status_format::format_reset_at(1_788_310_920, now, 0),
        "resets 01:02 on 2 Sep"
    );
}

/// 값 열은 상수가 아니라 **행 목록**에서 나온다.
///
/// 캡처의 값 열은 상자 안쪽 31칸째다: 상자 패딩 한 칸 + 행 자신의 한 칸 + 가장
/// 긴 라벨(값 없는 머리글 `GPT-5.3-Codex-Spark limit:`, 26칸) + 여백 세 칸.
/// 머리글을 빼고 세면 그 자리가 나오지 않는다.
#[test]
fn the_status_label_column_is_the_captured_one() {
    let capture = status_capture();
    // 가장 긴 라벨은 값이 없는 머리글이다 — 라벨에서 곧장 끝난다.
    let header = "│  GPT-5.3-Codex-Spark limit:  ";
    assert!(
        contains(&capture, header.as_bytes()),
        "capture no longer holds {header:?}"
    );
    // 그 열에 값이 붙는 모습. `Model:` 은 여섯 칸이고 값은 31칸째에서 시작한다.
    let measured = "│  Model:                       \u{1b}[22mgpt-5.3-codex-spark";
    assert!(
        contains(&capture, measured.as_bytes()),
        "capture no longer holds {measured:?}"
    );

    let labels = [
        "Model:",
        "Directory:",
        "Permissions:",
        "Agents.md:",
        "Account:",
        "Collaboration mode:",
        "Session:",
        "Context window:",
        "Weekly limit:",
        "GPT-5.3-Codex-Spark limit:",
        "5h limit:",
    ];
    // 상자 패딩 한 칸 + 행의 한 칸 = 2, 거기에 라벨 열을 더하면 캡처의 31이다.
    assert_eq!(2 + status_format::label_column(labels), 31);
}

/// 카드 상자는 부팅 카드와 **한 벌**이다 — 캡처의 두 카드가 같은 문법이다:
/// 둥근 모서리, 안쪽 폭 = 가장 긴 줄, 바깥변 = 그 폭 + 2, 여백까지 dim.
#[test]
fn the_status_card_box_is_the_boot_card_box() {
    let rows = vec![
        Line::new(vec![Span::dim(" >_ "), Span::bold("zo")]),
        Line::empty(),
        Line::new(vec![Span::dim(" Model:   "), Span::raw("claude-opus-5")]),
    ];
    let inner = rows.iter().map(Line::width).max().expect("rows");
    let card = cells::card(rows, 120);
    let plain: Vec<String> = card.iter().map(Line::plain).collect();
    assert_eq!(plain.len(), 5);
    assert_eq!(plain[0], format!("╭{}╮", "─".repeat(inner + 2)));
    assert_eq!(plain[4], format!("╰{}╯", "─".repeat(inner + 2)));
    // 빈 행은 dim 한 덩어리다 — 부팅 카드 핀과 같은 바이트다.
    let mut blank = String::new();
    write_spans(&card[2], &mut blank);
    assert_eq!(blank, format!("\u{1b}[2m│ {} │", " ".repeat(inner)));
}

/// W15 screen pin: the new rows are part of the rendered card bytes, not only
/// an internal controller report.
#[test]
fn the_status_card_renders_persistent_goal_and_all_autonomous_limits() {
    let usage = zo_ide::usage::UsageReport::default();
    let rows = status_format::status_card(&status_format::StatusCard {
        product: "zo",
        version: "0.1.0",
        model: "gpt-5.6-sol",
        effort: Some("high"),
        cwd: "/tmp/project",
        permissions: "workspace-write",
        permissions_pending: false,
        session_id: "session-w15",
        goal: "ship autonomous continuity (running)",
        autonomous:
            "on · continuations 1/8 · turns 2/12 · tokens 300/32000 · time 4s/1800s",
        loops: "none",
        context_tokens: 0,
        context_limit: 200_000,
        usage: &usage,
        accounts: &[],
        dream: None,
        now_unix: 0,
        width: 120,
    });
    let plain = rows.iter().map(Line::plain).collect::<Vec<_>>().join("\n");
    assert!(plain.contains("Goal:"));
    assert!(plain.contains("ship autonomous continuity (running)"));
    for limit in ["continuations 1/8", "turns 2/12", "tokens 300/32000", "time 4s/1800s"] {
        assert!(plain.contains(limit), "missing {limit:?} in {plain}");
    }
}

// ============================================================================
// 14. `AskUserQuestion` 오버레이 — codex `bottom_pane/request_user_input`
// ============================================================================
//
// 이 화면의 정본 스냅샷은 codex 저장소 안에 있다
// (`tui/src/bottom_pane/request_user_input/snapshots/…`). 120칸 `__options`
// 스냅샷이 이렇게 말한다:
//
// ```text
// ""
// "  Question 1/1 (1 unanswered)"
// "  Choose an option."
// ""
// "  › 1. Option 1  First choice."
// "    2. Option 2  Second choice."
// ""
// "  tab to add notes | enter to submit answer | esc to interrupt"
// ```
//
// zo 가 다르게 두는 레이아웃 자리는 둘뿐이고 둘 다 **이미 이식한 세 피커와 같은
// 문법**이라야 하기 때문이다: 앞 빈 줄이 둘(codex 의 이 오버레이는
// `render_menu_surface` 위 패딩 한 줄 위에 앉지만 zo 에는 그 표면이 없다),
// 그리고 행의 `›` 가 0열(같은 인셋 두 칸만큼의 차이). 색·번호·설명 열·푸터
// 문법은 그대로다. 옵션 목록 끝에는 codex 의 `is_other` 행을 항상 켠다 — zo
// 스키마에는 그 플래그가 없고 AskUserQuestion 은 자유 답을 늘 허용하기 때문이다.

fn question_prompt(
    question: &str,
    options: Vec<runtime::message_stream::QuestionOption>,
    multi_select: bool,
    header: Option<&str>,
) -> runtime::message_stream::UserQuestionPrompt {
    let (responder, receiver) = tokio::sync::oneshot::channel();
    // 답을 기다리는 쪽을 살려 둔다 — drop 되면 responder 가 닫힌다.
    std::mem::forget(receiver);
    runtime::message_stream::UserQuestionPrompt {
        id: runtime::message_stream::BlockId(1),
        question: question.to_string(),
        header: header.map(str::to_string),
        options,
        multi_select,
        responder,
    }
}

fn described(label: &str, description: &str) -> runtime::message_stream::QuestionOption {
    runtime::message_stream::QuestionOption {
        label: label.to_string(),
        description: Some(description.to_string()),
        preview: None,
    }
}

fn question_frame<'a>(composer: &'a Composer, question: &'a view::Question) -> Frame<'a> {
    Frame {
        composer,
        effort_tier: None,
        effort_effect: None,
        now: Instant::now(),
        status: None,
        pending_input: None,
        active: None,
        tail_owns_slot: false,
        dialog: None,
        question: Some(question),
        picker: None,
        sessions: None,
        pager: None,
        popup: None,
        shortcuts: None,
        model: "claude-opus-5",
        effort: "high",
        model_note: None,
        cwd: "/tmp/x",
        context_left: None,
        context_used_tokens: None,
        plan_mode: false,
        goal_status: None,
        loop_status: None,
        dream: None,
        width: 120,
        max_rows: 39,
    }
}

/// 단일 선택 — codex `__options` 스냅샷의 행·색·푸터 문법 그대로.
#[test]
fn the_single_select_question_is_the_codex_overlay() {
    let prompt = question_prompt(
        "Which auth method?",
        vec![
            described("OAuth", "Browser flow."),
            described("API key", "Static credential."),
        ],
        false,
        Some("Auth"),
    );
    let view = question::view(&prompt);
    let lines = view.lines(120, 40);
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();
    assert_eq!(
        plain,
        vec![
            String::new(),
            String::new(),
            "  Question 1/1 (1 unanswered) · Auth".to_string(),
            "  Which auth method?".to_string(),
            String::new(),
            "› 1. OAuth              Browser flow.".to_string(),
            "  2. API key            Static credential.".to_string(),
            "  3. None of the above  Type your own answer.".to_string(),
            String::new(),
            "  enter to submit answer | esc to interrupt".to_string(),
        ]
    );

    // 진행 줄은 dim, 질문은 시안 — codex `render_ui_at` 의 두 줄이다.
    let mut progress = String::new();
    write_spans(&lines[2], &mut progress);
    assert_eq!(progress, "  \u{1b}[2mQuestion 1/1 (1 unanswered) · Auth");
    let mut question_row = String::new();
    write_spans(&lines[3], &mut question_row);
    assert_eq!(question_row, "  \u{1b}[38;5;6;49mWhich auth method?");

    // 고른 줄은 세 피커와 **한 벌**이다 — `/model` 핀과 같은 바이트 머리다.
    let mut selected = String::new();
    write_spans(&lines[5], &mut selected);
    assert_eq!(
        selected,
        "\u{1b}[1m\u{1b}[38;5;6;49m› 1. OAuth              Browser flow."
    );

    // 푸터: 줄은 dim 이고 지금 눌러야 할 키 하나만 시안+bold
    // (codex `request_user_input/render.rs`: `tip.text.cyan().bold().not_dim()`).
    let mut footer = String::new();
    write_spans(&lines[9], &mut footer);
    assert_eq!(
        footer,
        // `ESC[2m` 이 두 번인 것은 codex 의 차분 방출 그대로다 — bold 를 끄면서
        // 새 스타일이 dim 이면 `ESC[22m ESC[2m` 을 내고, 이어 "dim 을 켠다" 가
        // 한 번 더 낸다(`tui::ansi` 머리말의 그 지문).
        "  \u{1b}[1m\u{1b}[38;5;6;49menter to submit answer\u{1b}[22m\u{1b}[2m\u{1b}[2m\u{1b}[39;49m | esc to interrupt"
    );
}

/// 좁은 화면에서 푸터는 힌트 단위로 접힌다 — codex `wrap_footer_tips` 의
/// 규칙이고, 그 스냅샷(`__footer_wrap`, 52칸)이 힌트 중간을 자르지 않는다.
#[test]
fn the_question_footer_wraps_by_tip_not_by_character() {
    let prompt = question_prompt("Which one?", vec![described("a", "first")], false, None);
    let view = question::view(&prompt);
    let plain: Vec<String> = view.lines(40, 40).iter().map(Line::plain).collect();
    let tail: Vec<&String> = plain.iter().rev().take(2).collect();
    assert_eq!(tail[1], "  enter to submit answer");
    assert_eq!(tail[0], "  esc to interrupt");
}

/// 다중 선택 — codex `multi_select_picker::build_rows` 의 `[x]`/`[ ]`.
#[test]
fn the_multi_select_question_carries_codex_checkboxes() {
    let prompt = question_prompt(
        "Which checks?",
        vec![
            described("fmt", "rustfmt --check."),
            described("clippy", "No warnings."),
        ],
        true,
        None,
    );
    let mut view = question::view(&prompt);
    assert!(view.is_multi());
    let plain: Vec<String> = view.lines(120, 40).iter().map(Line::plain).collect();
    assert_eq!(plain[2], "  Question 1/1 (1 unanswered)");
    assert_eq!(plain[5], "› 1. [ ] fmt            rustfmt --check.");
    assert_eq!(plain[6], "  2. [ ] clippy         No warnings.");
    assert_eq!(plain[7], "  3. None of the above  Type your own answer.");
    assert_eq!(
        plain[9],
        "  space to toggle | enter to submit answer | esc to interrupt"
    );

    // space 로 켜고, 커서를 내려 하나 더 켠다 — 답은 켠 순서가 아니라 목록 순서다.
    view.toggle();
    view.down();
    view.toggle();
    let plain: Vec<String> = view.lines(120, 40).iter().map(Line::plain).collect();
    assert_eq!(plain[5], "  1. [x] fmt            rustfmt --check.");
    assert_eq!(plain[6], "› 2. [x] clippy         No warnings.");
    assert_eq!(view.answers(), vec!["fmt".to_string(), "clippy".to_string()]);

    // 하나를 끄면 남은 하나가 답이다.
    view.toggle();
    assert_eq!(view.answers(), vec!["fmt".to_string()]);
    // 다 끄면 커서 줄 하나로 떨어진다 — 빈 답은 responder 계약에 없다.
    view.up();
    view.toggle();
    assert_eq!(view.answers(), vec!["fmt".to_string()], "커서 줄이 남는다");
}

/// 프레임 한 장을 조립하는 값.
///
/// ```sh
/// cargo test -p zo-ide --test tui_bytes frame_build_cost -- --ignored --nocapture
/// ```
///
/// 32ms 틱마다 뷰포트를 통째로 다시 조립한다 — 컴포저·상태·푸터의 모든 줄이
/// 새 `Line`/`Span` 이다. 페인터가 그중 안 바뀐 것을 버리지만(실측 723
/// ns/프레임), 버려질 것을 만드는 값은 여기서 든다.
#[ignore = "measurement, not a contract"]
#[test]
fn frame_build_cost() {
    const FRAMES: usize = 20_000;
    let composer = Composer::new();
    let frame = Frame {
        composer: &composer,
        effort_tier: None,
        effort_effect: None,
        now: Instant::now(),
        status: None,
        pending_input: None,
        active: None,
        tail_owns_slot: false,
        dialog: None,
        question: None,
        picker: None,
        sessions: None,
        pager: None,
        popup: None,
        shortcuts: None,
        model: "claude-opus-5",
        effort: "high",
        model_note: None,
        cwd: "/private/tmp/some/workspace/path",
        context_left: Some(87),
        context_used_tokens: Some(26_000),
        plan_mode: false,
        goal_status: None,
        loop_status: None,
        dream: None,
        width: 120,
        max_rows: 39,
    };
    let started = Instant::now();
    let mut rows = 0usize;
    for _ in 0..FRAMES {
        let (built, _) = view::build(&frame);
        rows += built.len();
    }
    let elapsed = started.elapsed();
    eprintln!(
        "[frame] {FRAMES} builds · {elapsed:?} · {} ns/frame · {} rows/frame",
        elapsed.as_nanos() / FRAMES as u128,
        rows / FRAMES
    );
}

/// 자유 서술 — 질문이 서고 컴포저가 답을 받는다(codex `__freeform` 스냅샷).
#[test]
fn a_freeform_question_stands_over_the_composer() {
    let prompt = question_prompt("Say more about the failure.", Vec::new(), false, None);
    let view = question::view(&prompt);
    assert!(view.is_freeform());

    let composer = Composer::new();
    let frame = question_frame(&composer, &view);
    let (rows, cursor) = view::build(&frame);
    let plain: Vec<String> = rows.iter().map(Line::plain).collect();
    assert_eq!(
        plain,
        vec![
            String::new(),
            String::new(),
            "  Question 1/1 (1 unanswered)".to_string(),
            "  Say more about the failure.".to_string(),
            String::new(),
            format!("╭{}╮", "─".repeat(118)),
            format!("│› Type your answer{}│", " ".repeat(100)),
            format!("╰{}╯", "─".repeat(118)),
            String::new(),
            "  enter to submit answer | esc to interrupt".to_string(),
        ]
    );
    // 커서는 컴포저 안이다 — 피커·다이얼로그와 달리 여기서는 사람이 친다.
    assert_eq!(cursor, Some((3, 6)));
    // 모델·cwd 푸터는 물러난다 — codex 의 오버레이도 bottom pane 을 통째로 쓴다.
    assert!(
        !plain.iter().any(|row| row.contains("claude-opus-5")),
        "rows were {plain:?}"
    );
}

#[test]
fn selecting_other_keeps_the_options_above_an_inline_composer() {
    let prompt = question_prompt(
        "Which auth method?",
        vec![
            described("OAuth", "Browser flow."),
            described("API key", "Static credential."),
        ],
        false,
        None,
    );
    let mut question = question::view(&prompt);
    question.down();
    question.down();
    assert!(question.begin_other_input());
    let mut composer = Composer::new();
    composer.insert_str("device flow");

    let (rows, cursor) = view::build(&question_frame(&composer, &question));
    let plain: Vec<String> = rows.iter().map(Line::plain).collect();

    assert_eq!(plain[7], "› 3. None of the above  Type your own answer.");
    assert!(plain[9].starts_with('╭'), "rows were {plain:?}");
    assert!(plain[10].contains("› device flow"), "rows were {plain:?}");
    assert_eq!(
        plain[13],
        "  esc to return to options | enter to submit answer"
    );
    assert!(cursor.is_some(), "inline composer must own the cursor");
}

/// 답한 질문은 스크롤백에 codex `RequestUserInputResultCell` 로 남는다.
#[test]
fn an_answered_question_commits_the_codex_result_cell() {
    let cell = cells::question_cell("Which auth method?", &["OAuth".to_string()], 120);
    let plain: Vec<String> = cell.iter().map(Line::plain).collect();
    assert_eq!(
        plain,
        vec![
            String::new(),
            "• Questions 1/1 answered".to_string(),
            "  • Which auth method?".to_string(),
            "    answer: OAuth".to_string(),
        ]
    );
    let mut header = String::new();
    write_spans(&cell[1], &mut header);
    assert_eq!(
        header,
        "\u{1b}[2m• \u{1b}[22m\u{1b}[1mQuestions\u{1b}[22m\u{1b}[2m\u{1b}[2m 1/1 answered"
    );
    let mut answer = String::new();
    write_spans(&cell[3], &mut answer);
    assert_eq!(
        answer,
        "\u{1b}[2m    answer: \u{1b}[22m\u{1b}[38;5;6;49mOAuth"
    );

    // 끊긴 질문도 흔적을 남긴다 — codex `interrupted: true` 갈래.
    let dropped = cells::question_cell("Which auth method?", &[], 120);
    let plain: Vec<String> = dropped.iter().map(Line::plain).collect();
    assert_eq!(
        plain,
        vec![
            String::new(),
            "• Questions 0/1 answered (interrupted)".to_string(),
            "  • Which auth method? (unanswered)".to_string(),
            "  ↳ interrupted with 1 unanswered".to_string(),
        ]
    );
}

/// 제출된 플랜은 제 화면을 얻는다 — codex `history_cell/plans.rs::ProposedPlanCell`
/// 의 머리(`• Proposed Plan`), 앞뒤 빈 줄, 두 칸 들여쓴 마크다운 본문.
/// 마지막 힌트 줄만 우리 것이다: `ExitPlanModeV2` 는 쓰기 권한을 돌려주지 않고
/// 승인은 사람이 `/plan off` 로 하는 별개의 행동이라, 그 사실이 화면에 없으면
/// 승인해야 할 사람이 무엇을 해야 하는지 모른다.
#[test]
fn a_submitted_plan_gets_the_codex_proposed_plan_cell() {
    let lines = cells::proposed_plan_cell(
        "## Update plan\n\n1. Check the target\n2. Apply the change",
        Some("/x/.zo/plans/plan-abc.md"),
        80,
    );
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();

    assert_eq!(plain[0], "");
    assert_eq!(plain[1], "• Proposed Plan");
    assert_eq!(plain[2], "");
    assert!(
        plain[3..].iter().any(|line| line.starts_with("  ") && line.contains("Update plan")),
        "the body must be indented two columns; got {plain:?}"
    );
    assert_eq!(
        plain.last().map(String::as_str),
        Some("  ↳ /plan off to approve · /x/.zo/plans/plan-abc.md")
    );

    let mut header = String::new();
    write_spans(&lines[1], &mut header);
    assert_eq!(header, "\u{1b}[2m• \u{1b}[22m\u{1b}[1mProposed Plan");
}

/// 플랜 본문이 비면 화면도 비는 것이 아니라 codex 처럼 `(empty)` 를 말한다.
#[test]
fn an_empty_plan_says_so_instead_of_drawing_nothing() {
    let lines = cells::proposed_plan_cell("   ", None, 80);
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();
    assert!(plain.iter().any(|line| line.contains("(empty)")), "got {plain:?}");
    // 경로를 모르면 힌트에 경로 칸이 아예 없다 — 빈 자리를 그리지 않는다.
    assert_eq!(plain.last().map(String::as_str), Some("  ↳ /plan off to approve"));
}

/// Codex image paste inserts consecutive local attachment labels in the composer.
/// This is a new screen-visible byte pin: it keeps the labels literal rather than
/// hiding them behind an implementation-only attachment counter.
#[test]
fn image_paste_placeholders_use_codex_composer_bytes() {
    let mut composer = Composer::new();
    composer.attach_image("/tmp/one.png".into());
    composer.attach_image("/tmp/two.png".into());
    let (lines, cursor) = composer.render(120, "Ask zo to do anything");
    assert_eq!(lines[1].plain(), format!("│› [Image #1][Image #2]{}│", " ".repeat(96)));
    assert_eq!(cursor, (23, 1));

    let mut rendered = String::new();
    write_spans(&lines[1], &mut rendered);
    // 마커의 굵기·색은 `›` 에서 끝나고 빈칸은 스타일 없는 스팬이다 — codex 의
    // 경계다(`composer::CARET`; 잔여 ① 을 옮기며 이 한 칸이 옮겨졌다).
    assert!(rendered.starts_with(
        "\u{1b}[38;2;128;128;128;49m│\u{1b}[1m\u{1b}[39;49m›\u{1b}[22m "
    ));
    assert!(rendered.contains("[Image #1][Image #2]"));
    assert!(rendered.ends_with("\u{1b}[38;2;128;128;128;49m│"));
}

/// 16. 컴포저 마커의 스팬 경계 — `›` 와 뒤 빈칸은 다른 스팬이다.
///
/// `docs/codex-tui-mechanics.md` 의 잔여 ① 이었다. codex 는 마커를
/// `buf.set_span(textarea_rect.x - LIVE_PREFIX_COLS, …)` 로 **한 글자만** 찍고
/// (`bottom_pane/chat_composer.rs`, `prompt` = `"›".bold()` 또는 tier 의
/// `prompt_glyph()`), 남는 한 칸은 텍스트 영역의 스타일 없는 여백이다
/// (`ui_consts.rs::LIVE_PREFIX_COLS = 2`). 우리는 `"› "` 를 한 스팬으로 내서
/// 그 빈칸이 굵기와 색을 물려받고 있었다.
///
/// 기대값은 손으로 적은 상수가 아니라 **캡처에서 뽑는다**: 세 캡처 안에 실제로
/// 흐른 바이트를 여기서 다시 찾아 우리 것과 견준다.
#[test]
fn the_composer_marker_span_boundary_is_the_captured_one() {
    // 1) codex 가 낸 바이트 — 색 없는 갈래(`status`·`inventory` 캡처)와
    //    tier 색이 붙은 갈래(`turn` 캡처).
    let plain_marker = "\u{1b}[1m\u{203a}\u{1b}[22m \u{1b}[2mAsk Codex to do anything";
    for name in [
        "codex-tui-v0.150.1-status.bin",
        "codex-tui-v0.150.1-inventory.bin",
    ] {
        assert!(
            contains(&capture_named(name), plain_marker.as_bytes()),
            "{name}: the capture must carry the marker/gap boundary this pin copies"
        );
    }
    let tier_marker = concat!(
        "\u{1b}[1m\u{1b}[38;2;255;178;66;49m\u{203a}",
        "\u{1b}[22m\u{1b}[39;49m \u{1b}[2mAsk Codex to do anything",
    );
    assert!(
        contains(&capture(), tier_marker.as_bytes()),
        "the tier-coloured branch ends its SGR at the glyph too"
    );

    // 2) 우리 바이트 — 무티어는 bold만, Max는 주황 tier 갈래를 지킨다.
    // 라운드 보더의 회색에서 기본 전경으로 돌아가는 `39;49`만 캡처의
    // 보더 없는 plain 갈래 앞에 더 붙는다.
    let plain_boundary = concat!(
        "\u{1b}[1m\u{1b}[39;49m\u{203a}",
        "\u{1b}[22m ",
    );
    let tier_boundary = concat!(
        "\u{1b}[1m\u{1b}[38;2;255;178;66;49m\u{203a}",
        "\u{1b}[22m\u{1b}[39;49m ",
    );
    let composer = Composer::new();
    let (lines, _) = composer.render(40, "Ask zo to do anything");
    let mut idle = String::new();
    write_spans(&lines[1], &mut idle);
    assert!(
        idle.contains(&format!("{plain_boundary}\u{1b}[2mAsk zo to do anything")),
        "idle composer: {idle:?}"
    );

    let (tier_lines, _) = composer.render_with_effort(
        40,
        "Ask zo to do anything",
        Some(EffortTier::Max),
        None,
        Instant::now(),
    );
    let mut tiered = String::new();
    write_spans(&tier_lines[1], &mut tiered);
    assert!(
        tiered.contains(&format!("{tier_boundary}\u{1b}[2mAsk zo to do anything")),
        "max tier composer: {tiered:?}"
    );

    // 3) 본문이 있는 갈래도 같다 — 빈칸이 본문 스팬에 붙어서도 안 된다.
    let mut typed = Composer::new();
    typed.insert_str("hello");
    let (typed_lines, _) = typed.render(40, "Ask zo to do anything");
    let mut body = String::new();
    write_spans(&typed_lines[1], &mut body);
    assert!(
        body.contains(&format!("{plain_boundary}hello")),
        "typed composer: {body:?}"
    );
}

/// Codex 0.150.1에는 없는, 사용자 선호로 추가한 컴포저 경계의 바이트 핀.
#[test]
fn composer_border_is_confined_to_its_own_block() {
    let composer = Composer::new();
    let (lines, cursor) = composer.render(12, "Ask");
    let plain: Vec<String> = lines.iter().map(Line::plain).collect();
    assert_eq!(plain, vec!["╭──────────╮", "│› Ask     │", "╰──────────╯"]);
    assert_eq!(cursor, (3, 1));

    let mut border = String::new();
    write_spans(&lines[0], &mut border);
    assert_eq!(
        border,
        "\u{1b}[38;2;128;128;128;49m╭──────────╮"
    );

    // 같은 핀에서 실측 푸터 이펙트와 ZeroCode-only Smart 확장도 고정한다.
    let previous = Line::from_text("gpt-5.6-sol medium · /tmp");
    let current = Line::from_text("gpt-5.6-sol ultra · /tmp");
    let frame_zero = Instant::now();
    let ultra = EffortEffect::new(EffortTier::Ultra, previous.clone());
    let _ = ultra.footer_line_at(&current, 32, frame_zero);
    assert_eq!(
        ultra
            .footer_line_at(&current, 32, frame_zero + Duration::from_millis(1_500))
            .plain(),
        "           U L T R A"
    );

    let smart = EffortEffect::new(EffortTier::Smart, previous);
    let _ = smart.footer_line_at(&current, 32, frame_zero);
    assert_eq!(
        smart
            .footer_line_at(&current, 32, frame_zero + Duration::from_millis(1_500))
            .plain(),
        "           S M A R T"
    );
}
