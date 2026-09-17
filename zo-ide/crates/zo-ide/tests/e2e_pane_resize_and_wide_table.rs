//! The IDE pane report (2026-08-30), reproduced and pinned. An answer whose markdown
//! table has Korean (wide, multi-byte) cells showed its header row and then
//! NOTHING — not the separator, not the body rows, not the sections after the
//! table — although the transcript held all 12,123 characters and the turn
//! ended normally. The ASCII table golden passes, so the shape under test is
//! the same table with wide cells and prose after it.

// The shared pty harness (`e2e/harness.rs`) speaks `nix`; this target is
// unix-only, like `e2e_hermetic` — the Windows cross-check (`just win-check`)
// compiles every test target and had nothing to compile it against.
#![cfg(unix)]

// The shared harness is larger than this suite uses (measurement helpers,
// the pipe runner, the contract checker); each test binary takes its part.
#![allow(dead_code)]

mod e2e;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use e2e::harness::{history_rows_raw, PtyRun, Screen};
use e2e::scripted::ScriptedAnthropicService;
use tempfile::TempDir;

const TEST_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a state the TUI reaches on its own clock (the one-second size
/// poll, a settle) may take under load before the test calls it missing.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

const KOREAN_TABLE_ANSWER: &str = "영향도만 정리하면, 기능적으로 바뀌는 건 \"어느 지점에서 발급 가능한가\" 하나뿐이고 나머지 로직은 전부 동일합니다. 대신 재고 통계와 신청 대장에 흔적이 남습니다.\n\n## 의도한 변화\n\n| 대상 | 변경 전 | 변경 후 |\n|---|---|---|\n| 발급 가능 지점 | 동대문만 | 평택만 |\n| 어드민 목록 | 동대문 직원에게 보임 | 평택 직원에게 보임 |\n| 재고 수량 | 동대문 750장 | 동대문 550장 / 평택 +200장 |\n\n발급 조회가 직원 소속 지점으로 필터되기 때문입니다.\n\n## 영향 없음 (확인 완료)\n\n- **모바일지점 판정**: 두 지점 모두 `MOBILE_BRANCH_YN='N'`이라 발급 흐름이 동일합니다.\n- **BC 대사**: 일련번호를 바꾸지 않으므로 영향 없습니다.\n";

/// The longest run of blank rows strictly inside `rows`.
fn longest_blank_run(rows: &[String]) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for row in rows {
        if row.trim().is_empty() {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

struct Layout {
    _root: TempDir,
    cwd: PathBuf,
    home: PathBuf,
    sessions: PathBuf,
    state: PathBuf,
}

impl Layout {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary e2e root");
        let cwd = root.path().join("workspace");
        let home = root.path().join("home");
        let sessions = root.path().join("sessions");
        let state = root.path().join("state");
        for path in [&cwd, &home, &sessions, &state] {
            fs::create_dir_all(path).expect("e2e directory");
        }
        fs::write(
            home.join("settings.json"),
            br#"{"smart":{"autoClassifier":"off"}}"#,
        )
        .expect("disable routing probe in e2e home");
        Self {
            _root: root,
            cwd,
            home,
            sessions,
            state,
        }
    }
}

fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC … BEL or ST
                    while let Some(n) = chars.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn korean_table_body_and_following_sections_reach_the_screen() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text(KOREAN_TABLE_ANSWER)
        .await
        .expect("start korean table script");
    let mut run = PtyRun::spawn(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &["--permission-mode", "danger-full-access"],
    )
    .expect("spawn zo in PTY");

    run.wait_for("directory:", TEST_TIMEOUT);
    let before = run.output_len();
    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r")
        .expect("send prompt");
    // The last line of the answer, if it is ever painted.
    let last_line_seen = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run.wait_for_after("영향 없습니다", before, TEST_TIMEOUT);
    }))
    .is_ok();
    let capture = run.finish();

    let rows: Vec<String> = history_rows_raw(&capture)
        .iter()
        .map(|row| strip_ansi(row))
        .collect();
    eprintln!("--- committed history rows ({}):", rows.len());
    for (i, row) in rows.iter().enumerate() {
        eprintln!("{i:3} |{row}");
    }
    let joined = rows.join("\n");
    assert!(
        last_line_seen,
        "the final line of the answer never reached the screen"
    );
    for needle in ["대상", "발급 가능 지점", "재고 수량", "영향 없음", "영향 없습니다"] {
        assert!(
            joined.contains(needle),
            "history is missing {needle:?} — the table or what follows it was dropped"
        );
    }
}


/// The same answer after a tool turn inside an IDE pane. Pane identity is
/// transport metadata, so it must not add OSC 7788 or alter the history.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn korean_table_after_a_tool_turn_in_an_ide_pane_uses_bare_tui_bytes() {
    let layout = Layout::new();
    fs::write(layout.cwd.join("fixture.txt"), "fixture body\n").expect("fixture");
    let service = ScriptedAnthropicService::bash_read(KOREAN_TABLE_ANSWER)
        .await
        .expect("start bash_read script");
    let mut run = PtyRun::spawn_with_env(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &["--permission-mode", "danger-full-access"],
        &[("ZEROCODE_PANE_KEY", "e2e-pane")],
    )
    .expect("spawn zo in PTY");

    run.wait_for("directory:", TEST_TIMEOUT);
    let before = run.output_len();
    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r")
        .expect("send prompt");
    let last_line_seen = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run.wait_for_after("영향 없습니다", before, TEST_TIMEOUT);
    }))
    .is_ok();
    let capture = run.finish();
    if let Ok(path) = std::env::var("ZO_E2E_CAPTURE_OUT") {
        fs::write(&path, &capture).expect("write capture");
        eprintln!("--- capture written to {path} ({} bytes)", capture.len());
    }

    let raw_rows = history_rows_raw(&capture);
    eprintln!("--- committed history rows ({}), stripped | raw-escaped:", raw_rows.len());
    for (i, row) in raw_rows.iter().enumerate() {
        let stripped = strip_ansi(row);
        let raw: String = String::from_utf8_lossy(row)
            .chars()
            .map(|c| if c == '\x1b' { "<ESC>".to_string() } else if c == '\x07' { "<BEL>".to_string() } else { c.to_string() })
            .collect();
        eprintln!("{i:3} |{stripped}");
        if raw.contains("7788") {
            eprintln!("    raw: {raw}");
        }
    }
    assert!(
        !capture.windows(4).any(|window| window == b"7788"),
        "a pane key enabled private fold-marker bytes"
    );
    let joined = raw_rows.iter().map(|r| strip_ansi(r)).collect::<Vec<_>>().join("\n");
    assert!(last_line_seen, "the final line of the answer never reached the screen");
    for needle in ["대상", "발급 가능 지점", "재고 수량", "영향 없음", "영향 없습니다"] {
        assert!(joined.contains(needle), "history is missing {needle:?}");
    }
}


/// Token-sized deltas instead of one text event: the incremental renderer
/// sees multi-byte boundaries land anywhere, as it does against the real API.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn korean_table_streamed_in_eighty_pieces() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::dribbled_text(
        KOREAN_TABLE_ANSWER,
        80,
        Duration::from_millis(12),
    )
    .await
    .expect("start dribbled script");
    let mut run = PtyRun::spawn(
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &["--permission-mode", "danger-full-access"],
    )
    .expect("spawn zo in PTY");
    run.wait_for("directory:", TEST_TIMEOUT);
    let before = run.output_len();
    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r").expect("send prompt");
    let last_line_seen = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run.wait_for_after("영향 없습니다", before, TEST_TIMEOUT);
    }))
    .is_ok();
    let capture = run.finish();
    let rows: Vec<String> = history_rows_raw(&capture).iter().map(|r| strip_ansi(r)).collect();
    eprintln!("--- dribbled: committed history rows ({}):", rows.len());
    for (i, row) in rows.iter().enumerate() {
        eprintln!("{i:3} |{row}");
    }
    let joined = rows.join("\n");
    assert!(last_line_seen, "the final line of the answer never reached the screen");
    for needle in ["대상", "발급 가능 지점", "재고 수량", "영향 없음", "영향 없습니다"] {
        assert!(joined.contains(needle), "history is missing {needle:?}");
    }
}


/// The pane report's geometry: `stty size` on the live pane said 44x176 while
/// the screenshot showed the viewport mid-screen with blank rows below it and
/// a paragraph clipped at the right edge — the shape of a zo laying out for a
/// SMALLER terminal than the pty now is. Start small, grow the pty under a
/// running zo (the kernel sends SIGWINCH), and check the next turn is laid out
/// for the new size: the paragraph that wraps at 133 columns must come out as
/// ONE row at 176, and the viewport must be painted at the bottom of 44 rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pane_resized_under_a_running_zo_is_laid_out_for_the_new_size() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text(KOREAN_TABLE_ANSWER)
        .await
        .expect("start script");
    let mut run = PtyRun::spawn_controlling_sized(
        24,
        133,
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &["--permission-mode", "danger-full-access"],
        &[("ZEROCODE_PANE_KEY", "e2e-pane")],
    )
    .expect("spawn zo in PTY");
    run.wait_for("directory:", TEST_TIMEOUT);
    let before_first = run.output_len();
    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r").expect("send prompt 1");
    run.wait_for_after("영향 없습니다", before_first, TEST_TIMEOUT);

    // Grow the pane: 24x133 -> 44x176, while zo is idle.
    let before_resize = run.output_len();
    run.resize(44, 176).expect("resize pty");
    std::thread::sleep(Duration::from_millis(600));
    let before_second = run.output_len();
    let idle_repaint = &run.snapshot_output()[before_resize..before_second];
    let idle_max_row = max_cursor_row(idle_repaint);
    eprintln!("--- idle repaint after resize: {} bytes, highest cursor row {idle_max_row}", idle_repaint.len());
    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r").expect("send prompt 2");
    run.wait_for_after("영향 없습니다", before_second, TEST_TIMEOUT);
    let capture = run.finish();

    let first = &capture[..before_second];
    let second = &capture[before_second..];
    let rows_first: Vec<String> = history_rows_raw(first).iter().map(|r| strip_ansi(r)).collect();
    let rows_second: Vec<String> = history_rows_raw(second).iter().map(|r| strip_ansi(r)).collect();
    let para_rows = paragraph_rows;
    eprintln!("--- turn 1 @24x133: {} history rows, paragraph rows = {}", rows_first.len(), para_rows(&rows_first));
    for r in &rows_first { eprintln!("  1|{r}"); }
    eprintln!("--- turn 2 @44x176: {} history rows, paragraph rows = {}", rows_second.len(), para_rows(&rows_second));
    for r in &rows_second { eprintln!("  2|{r}"); }
    // Where did the viewport go after the resize? Look at the cursor rows zo addressed.
    let max_row = max_cursor_row(second);
    eprintln!("--- highest cursor row addressed after the resize: {max_row}");
    assert_eq!(para_rows(&rows_first), 2, "at 133 columns the paragraph wraps into two rows");
    assert_eq!(para_rows(&rows_second), 1, "at 176 columns the paragraph must be one row — zo still laid out for the old width");
    assert!(max_row >= 40, "the viewport must be painted near the bottom of 44 rows, got {max_row}");
    assert!(idle_max_row >= 40, "an idle zo must repaint its viewport at the new bottom right after the resize, got row {idle_max_row} in {} bytes", idle_repaint.len());
    let joined = rows_second.join("\n");
    for needle in ["대상", "발급 가능 지점", "재고 수량", "영향 없음", "영향 없습니다"] {
        assert!(joined.contains(needle), "second turn is missing {needle:?}");
    }
}

/// Resize while text deltas are still arriving, not only between turns. The
/// final committed cell must reflow at the new width and the live viewport
/// must move to the new floor without dropping the stream tail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streaming_pane_reflows_when_it_grows() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::dribbled_text(
        KOREAN_TABLE_ANSWER,
        80,
        Duration::from_millis(20),
    )
    .await
    .expect("start dribbled script");
    let mut run = PtyRun::spawn_controlling_sized(
        24,
        133,
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &["--permission-mode", "danger-full-access"],
        &[("ZEROCODE_PANE_KEY", "e2e-pane")],
    )
    .expect("spawn zo in PTY");
    run.wait_for("directory:", TEST_TIMEOUT);
    let prompt_at = run.output_len();
    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r")
        .expect("send prompt");
    run.wait_for_after("영향도만 정리하면", prompt_at, TEST_TIMEOUT);

    let resize_at = run.output_len();
    run.resize(44, 176).expect("grow pty during stream");
    run.wait_for_after("영향 없습니다", resize_at, TEST_TIMEOUT);
    // The final text first appears in the mutable tail. Give the completion
    // event and commit animation several frame intervals to replace it with
    // the source-backed history cell before taking the live snapshot.
    std::thread::sleep(Duration::from_millis(400));
    let capture = run.snapshot_output();
    let _ = run.finish();
    let before_resize = &capture[..resize_at.min(capture.len())];
    let after_resize = &capture[resize_at.min(capture.len())..];

    if let Ok(path) = std::env::var("ZO_E2E_DUMP") {
        fs::write(format!("{path}.before"), before_resize).expect("dump pre-resize bytes");
        fs::write(format!("{path}.after"), after_resize).expect("dump post-resize bytes");
    }

    // The settled SCREEN is what a person sees, and the one place the answer
    // can be judged whole: whether the paragraph was committed before the
    // resize (the rows above the head are rebuilt from the line, at the new
    // width) or after it (the stream commits it at the new width), it stands
    // on one row of 176 columns exactly once. Counting only rows the stream
    // committed after the resize raced the dribble and went red under load.
    let mut screen = Screen::new(24);
    screen.feed(before_resize);
    screen.resize(44);
    screen.feed(after_resize);
    let visible = screen.visible();
    let paragraph: Vec<&String> = visible
        .iter()
        .filter(|row| row.contains("영향도만 정리하면"))
        .collect();
    assert_eq!(
        paragraph.len(),
        1,
        "the settled screen shows the paragraph once, on one row: {visible:#?}"
    );
    assert!(
        paragraph[0].contains("신청 대장에 흔적이 남습니다"),
        "the paragraph is whole at 176 columns: {:?}",
        paragraph[0]
    );
    // The head is on the floor — the footer on the last row — and the
    // transcript is one unbroken column from its first row to its last, with
    // no void inside it. The blank rows between the transcript's last row and
    // the head's first are the band the next turn fills (the head sits on the
    // floor like Claude Code, not inline after the transcript like codex:
    // user decision 2026-09-06), so they are not counted.
    let footer = visible
        .iter()
        .rposition(|row| row.contains("? for shortcuts"))
        .expect("the footer is on screen");
    assert_eq!(footer, visible.len() - 1, "the footer is not on the last row: {visible:#?}");
    let head_top = visible
        .iter()
        .rposition(|row| row.contains('\u{256d}'))
        .expect("the composer's box is on screen");
    let first_content = visible
        .iter()
        .position(|row| !row.trim().is_empty())
        .expect("something on screen");
    let last_content = visible[..head_top]
        .iter()
        .rposition(|row| !row.trim().is_empty())
        .expect("the transcript is above the head");
    let mut blank_run = 0usize;
    let mut longest_void = 0usize;
    for row in &visible[first_content..=last_content] {
        if row.trim().is_empty() {
            blank_run += 1;
            longest_void = longest_void.max(blank_run);
        } else {
            blank_run = 0;
        }
    }
    assert!(
        longest_void <= 3,
        "a void of {longest_void} blank rows opened inside the transcript: {visible:#?}"
    );
    let joined = visible.join("\n");
    for needle in ["대상", "발급 가능 지점", "재고 수량", "영향 없음", "영향 없습니다"] {
        assert!(joined.contains(needle), "streamed answer is missing {needle:?}: {visible:#?}");
    }
}


/// The highest row any CUP (`ESC [ row ; col H`) in `bytes` addressed. Only a
/// CSI whose final byte is `H` counts — an SGR like `ESC[36;49m` followed by
/// text containing an `H` is not a cursor move.
fn max_cursor_row(bytes: &[u8]) -> usize {
    let text = String::from_utf8_lossy(bytes);
    let mut max_row = 0usize;
    for cap in text.split("\x1b[").skip(1) {
        let params_len = cap
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_digit() || *c == ';' || *c == '?'))
            .map_or(cap.len(), |(i, _)| i);
        let final_byte = cap[params_len..].chars().next();
        if final_byte != Some('H') {
            continue;
        }
        let params = &cap[..params_len];
        let row = params.split(';').next().unwrap_or("").parse::<usize>().unwrap_or(1);
        max_row = max_row.max(row);
    }
    max_row
}

fn paragraph_rows(rows: &[String]) -> usize {
    rows.iter()
        .filter(|r| r.contains("영향도만 정리하면") || r.trim_start().starts_with("신청 대장에"))
        .count()
}

fn spawn_pane(layout: &Layout, base_url: &str, rows: u16, cols: u16) -> PtyRun {
    PtyRun::spawn_controlling_sized(
        rows,
        cols,
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        base_url,
        &["--permission-mode", "danger-full-access"],
        &[("ZEROCODE_PANE_KEY", "e2e-pane")],
    )
    .expect("spawn zo in PTY")
}

/// The signal never comes. The pane grows from 24x133 to 44x176 under an idle
/// zo and nobody tells it: within the size poll's cadence the viewport must be
/// repainted at the new bottom, and the next turn must wrap at 176.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pane_grown_without_sigwinch_is_noticed_by_the_size_poll() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text(KOREAN_TABLE_ANSWER)
        .await
        .expect("start script");
    let mut run = spawn_pane(&layout, service.base_url(), 24, 133);
    run.wait_for("directory:", TEST_TIMEOUT);
    let before_first = run.output_len();
    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r").expect("prompt 1");
    run.wait_for_after("영향 없습니다", before_first, TEST_TIMEOUT);

    let before_resize = run.output_len();
    run.resize_silently(44, 176).expect("resize pty silently");
    // The size poll runs on its own clock: wait for the repaint it makes at
    // the new bottom rather than betting a fixed sleep against the load.
    run.wait_until(before_resize, SETTLE_TIMEOUT, |bytes| max_cursor_row(bytes) >= 40);
    let before_second = run.output_len();
    let idle_repaint = &run.snapshot_output()[before_resize..before_second];
    let idle_max_row = max_cursor_row(idle_repaint);
    eprintln!("--- silent grow: idle repaint {} bytes, highest cursor row {idle_max_row}", idle_repaint.len());

    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r").expect("prompt 2");
    run.wait_for_after("영향 없습니다", before_second, TEST_TIMEOUT);
    let capture = run.finish();
    let second = &capture[before_second..];
    let rows_second: Vec<String> = history_rows_raw(second).iter().map(|r| strip_ansi(r)).collect();
    for r in &rows_second { eprintln!("  2|{r}"); }

    assert!(idle_max_row >= 40, "the size poll must repaint the viewport at the new bottom without a SIGWINCH, got row {idle_max_row}");
    assert_eq!(paragraph_rows(&rows_second), 1, "after the silent grow the next turn must wrap at 176 columns");
    let joined = rows_second.join("\n");
    for needle in ["대상", "발급 가능 지점", "재고 수량", "영향 없음", "영향 없습니다"] {
        assert!(joined.contains(needle), "second turn is missing {needle:?}");
    }
}

/// The report's direction: the pane SHRINKS and the signal never comes. Rows
/// laid out for the old, taller terminal would be clamped onto the pane's last
/// row and overwrite each other — the table body vanishing. With the poll the
/// next turn is laid out for 24 rows, addresses nothing beyond them, wraps at
/// 133, and every row of the answer reaches the screen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pane_shrunk_without_sigwinch_loses_no_rows() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::text(KOREAN_TABLE_ANSWER)
        .await
        .expect("start script");
    let mut run = spawn_pane(&layout, service.base_url(), 44, 176);
    run.wait_for("directory:", TEST_TIMEOUT);
    let before_first = run.output_len();
    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r").expect("prompt 1");
    run.wait_for_after("영향 없습니다", before_first, TEST_TIMEOUT);

    let before_resize = run.output_len();
    run.resize_silently(24, 133).expect("resize pty silently");
    // The poll's repaint at the new floor is the sign it noticed the shrink.
    run.wait_until(before_resize, SETTLE_TIMEOUT, |bytes| !bytes.is_empty());
    let before_second = run.output_len();
    run.send(b"\xec\x98\x81\xed\x96\xa5\xeb\x8f\x84?\r").expect("prompt 2");
    run.wait_for_after("영향 없습니다", before_second, TEST_TIMEOUT);
    let capture = run.finish();
    let second = &capture[before_second..];
    // The raw bytes of the second turn, for replaying through a terminal
    // emulator when the row numbers alone do not say what went where.
    if let Ok(path) = std::env::var("ZO_E2E_DUMP") {
        std::fs::write(&path, second).expect("dump the capture");
    }
    let rows_second: Vec<String> = history_rows_raw(second).iter().map(|r| strip_ansi(r)).collect();
    let max_row = max_cursor_row(second);
    eprintln!("--- silent shrink: {} history rows in turn 2, highest cursor row {max_row}", rows_second.len());
    for r in &rows_second { eprintln!("  2|{r}"); }

    assert!(max_row <= 24, "after the silent shrink zo must not address rows the pane no longer has, got {max_row}");
    assert_eq!(paragraph_rows(&rows_second), 2, "after the silent shrink the next turn must wrap at 133 columns");
    let joined = rows_second.join("\n");
    for needle in ["대상", "발급 가능 지점", "재고 수량", "영향 없음", "영향 없습니다"] {
        assert!(joined.contains(needle), "second turn is missing {needle:?}");
    }
}

/// A tool cell that filled the pane — a bash run whose live output grows the
/// head to the ceiling — commits, the head shrinks to the floor and leaves the
/// rows it gave up blank, and the answer that streams next grows the head
/// again. Before, that growth scrolled the whole screen and carried the blank
/// rows up into the transcript, so the answer stood under a band of empty rows
/// (the 2026-09-03 screenshot, a split pane). On the settled screen the only
/// blank rows between the cell and the answer are the cell's own margins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_cell_that_fills_the_pane_leaves_no_blank_band_before_the_answer() {
    let layout = Layout::new();
    let service = ScriptedAnthropicService::bash(
        "for i in $(seq 1 60); do echo \"output line $i\"; sleep 0.02; done",
        "### After the tool\n\n- the first point after the cell\n- the second point after the cell\n- the third point after the cell\n\nThe answer paragraph closes the turn.\n",
    )
    .await
    .expect("start bash script");
    let mut run = PtyRun::spawn_sized(
        24,
        100,
        &layout.cwd,
        &layout.home,
        &layout.sessions,
        &layout.state,
        service.base_url(),
        &["--permission-mode", "danger-full-access"],
        &[("ZEROCODE_PANE_KEY", "e2e-pane")],
    )
    .expect("spawn zo in PTY");
    run.wait_for("directory:", TEST_TIMEOUT);
    let prompt_at = run.output_len();
    run.send(b"run the loop\r").expect("send prompt");
    run.wait_for_after("closes the turn", prompt_at, TEST_TIMEOUT);
    // Let the commit animation and the head's final resize settle.
    std::thread::sleep(Duration::from_millis(600));
    let capture = run.snapshot_output();
    let _ = run.finish();

    if let Ok(path) = std::env::var("ZO_E2E_DUMP") {
        fs::write(format!("{path}.blank-band"), &capture).expect("dump bytes");
    }

    let mut screen = Screen::new(24);
    screen.feed(&capture);
    let visible = screen.visible();
    let shown = visible.join("\n");
    let cell = visible
        .iter()
        .rposition(|row| row.contains("seq 1 60") || row.contains("output line"))
        .unwrap_or_else(|| panic!("the tool cell is on screen:\n{shown}"));
    let answer = visible
        .iter()
        .position(|row| row.contains("first point after the cell"))
        .unwrap_or_else(|| panic!("the answer is on screen:\n{shown}"));
    assert!(answer > cell, "the answer follows the cell:\n{shown}");
    let band = longest_blank_run(&visible[cell + 1..answer]);
    assert!(
        band <= 2,
        "a band of {band} blank rows stands between the tool cell and the answer:\n{shown}"
    );
}
