//! 세션이 끝날 때·갈아 끼울 때 나가는 두 줄 — codex `app.rs::session_summary`.
//!
//! codex 는 이 요약을 **두 자리**에서 낸다.
//!
//! 1. 프로세스가 끝날 때. `main.rs::format_exit_messages` 가 화면을 놓은 뒤
//!    stdout 으로 찍는다(`println!`, 그래서 tty 의 ONLCR 이 `\n` 을 `\r\n` 로
//!    바꾼다). 실측 꼬리:
//!
//!    ```text
//!    Token usage: total=8,118 input=7,940 (+ 9,984 cached) output=178 (reasoning 133)
//!    To continue this session, run ESC[36mcodex resume 01a03d06-…ESC[39m
//!    ```
//!
//!    (`docs/captures/codex-tui-v0.149.1-turn.bin` 마지막 두 줄.)
//!
//! 2. 세션을 갈아 끼울 때 — `/resume`·`/new`·`/fork`. 그때는 **옛 세션**의
//!    같은 두 줄이 히스토리 셀로 들어간다
//!    (`app/session_lifecycle.rs::resume_target_session`: 새 스레드를 붙이고
//!    재생까지 마친 뒤 `add_plain_history_lines`). 명령 토큰은 화면 안에서
//!    cyan 스팬이다(`command.cyan()`).
//!
//! # 두 줄이 서는 조건
//!
//! `session_summary` 는 둘 다 없으면 `None` 이다. usage 줄은 총합이 0 이 아닐
//! 때만(`TokenUsage::is_zero`), 재개 힌트는 트랜스크립트 파일이 **실재하고
//! 비어 있지 않을 때만**(`rollout_path_is_resumable`) 선다 — 아직 아무것도
//! 안 쓴 세션을 재개하라고 권하지 않기 위해서다.
//!
//! # 우리 문안
//!
//! 명령만 우리 것이다: `codex resume <id>` 자리에 `zo --resume <id>`.
//! codex 는 스레드에 이름이 있으면 `codex resume, then select <name> (<id>)`
//! 갈래도 내지만, 우리 [`crate::session::SessionHandle`] 은 id 와 경로만
//! 들고 있어 그 갈래가 성립하지 않는다.

use std::path::Path;

use core_types::usage::TokenUsage;

use super::ansi::{Color, Line, Span, Style, StyleWriter};

/// 재개 명령 — codex `codex_utils_cli::resume_hint` 자리.
const RESUME_COMMAND: &str = "zo --resume";
/// usage 줄 앞머리 — codex `TokenUsage` 의 `Display`.
const USAGE_LABEL: &str = "Token usage:";
/// 재개 힌트 앞머리 — codex `main.rs::format_exit_messages`.
const RESUME_LEAD: &str = "To continue this session, run ";
/// 종료와 히스토리 재개 명령의 토큰 색. Codex 종료 문법의 `ESC[36m`을 두
/// 갈래가 공유한다.
const EXIT_COMMAND_TOKEN: Color = Color::CYAN;

/// 세 자리마다 쉼표 — codex `codex_protocol::num_format::format_with_separators`.
///
/// 실측이 `total=8,118` 이므로 구분자는 필수다. 자리별로 문자열을 새로 만들지
/// 않고 뒤에서 앞으로 한 번에 쌓는다.
fn with_separators(value: u64) -> String {
    crate::util::group_digits(value, ',')
}

/// codex 어휘로 옮긴 토큰 계수.
///
/// codex(OpenAI 결)의 `input_tokens` 는 캐시분을 **포함한** 총 입력이고
/// `cached_input_tokens` 가 그 부분집합이다. 우리 [`TokenUsage`](core_types)
/// 는 셋이 서로 겹치지 않는다(`input` · `cache_read` · `cache_creation`).
/// 그래서 캐시에서 읽은 것만 "cached" 로 세고, 새로 쓴 캐시는 값을 치른
/// 입력이므로 non-cached 쪽에 붙인다.
#[derive(Debug, Clone, Copy)]
struct Counts {
    non_cached_input: u64,
    cached_input: u64,
    output: u64,
}

impl Counts {
    fn of(usage: TokenUsage) -> Self {
        Self {
            non_cached_input: u64::from(usage.input_tokens)
                .saturating_add(u64::from(usage.cache_creation_input_tokens)),
            cached_input: u64::from(usage.cache_read_input_tokens),
            output: u64::from(usage.output_tokens),
        }
    }

    /// codex `TokenUsage::blended_total` — 값을 치른 입력 + 출력.
    fn blended_total(self) -> u64 {
        self.non_cached_input.saturating_add(self.output)
    }
}

/// 요약 한 벌. 두 줄 다 없으면 [`session_summary`] 가 아예 `None` 을 준다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    usage_line: Option<String>,
    /// 이 세션이 남긴 기억 한 줄(`crate::dream::session_end_line`). 승격된 것이
    /// 없으면 `None` 이고 줄은 아예 안 나간다 — 종료 요약은 사람이 창을 닫는
    /// 순간에 지나가는 두 줄이라 할 말이 없으면 자리를 비운다.
    dream_line: Option<String>,
    resume_hint: Option<String>,
}

impl SessionSummary {
    /// 종료 경로의 줄들 — codex `main.rs::format_exit_messages`.
    ///
    /// 화면을 이미 놓은 자리라 차분기가 없어 원본이
    /// `ESC[36m…ESC[39m` 를 **손으로** 만든다(실측
    /// `codex-tui-v0.149.1-turn.bin` 마지막 줄).
    #[must_use]
    pub fn exit_lines(&self, color: bool) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(usage_line) = &self.usage_line {
            lines.push(usage_line.clone());
        }
        if let Some(dream_line) = &self.dream_line {
            lines.push(dream_line.clone());
        }
        if let Some(command) = &self.resume_hint {
            let command = if color {
                let mut painted = String::new();
                StyleWriter::set_foreground(Some(EXIT_COMMAND_TOKEN), &mut painted);
                painted.push_str(command);
                StyleWriter::set_foreground(None, &mut painted);
                painted
            } else {
                command.clone()
            };
            lines.push(format!("{RESUME_LEAD}{command}"));
        }
        lines
    }

    /// [`Self::exit_lines`] 를, 이미 사라졌을 수 있는 터미널에 쓴다.
    ///
    /// 요약은 화면을 놓은 **뒤**에 나가는데, 판에서는 그때가 창이 판을 닫은
    /// 뒤이기도 하다 — 판을 닫으면 세션이 hang up 되고, zo 는 그 신호로 대화를
    /// 끝낸 다음 이 줄들을 쓴다. 닫힌 터미널은 EIO 로 답하고 `println!` 은 EIO 에
    /// panic 한다: 창이 띄운 zo 판의 마지막 300번 중 129번이 여기서 panic 으로
    /// 끝났다(`~/.zo/logs/zo-ide.log`, 2026-09-17). 읽을 사람이 없으니 첫 실패한
    /// 쓰기가 줄을 끝내고, 그 오류는 부른 쪽이 버리도록 돌려준다.
    ///
    /// # Errors
    /// 실패한 쓰기나 flush.
    pub fn write_exit_lines(&self, out: &mut impl std::io::Write, color: bool) -> std::io::Result<()> {
        for line in self.exit_lines(color) {
            writeln!(out, "{line}")?;
        }
        out.flush()
    }

    /// 히스토리 셀의 줄들 — codex 가 세션을 갈아 끼운 뒤 미는 것
    /// (`chatwidget::add_plain_history_lines` → `PlainHistoryCell`).
    /// 접두어가 없어 0열에서 시작한다(실측 참조는 모듈 머리말).
    #[must_use]
    pub fn history_lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        if let Some(usage_line) = &self.usage_line {
            lines.push(Line::from_text(usage_line.clone()));
        }
        if let Some(dream_line) = &self.dream_line {
            lines.push(Line::from_text(dream_line.clone()));
        }
        if let Some(command) = &self.resume_hint {
            lines.push(Line::new(vec![
                Span::raw(RESUME_LEAD),
                Span::new(command.clone(), Style::new().fg(EXIT_COMMAND_TOKEN)),
            ]));
        }
        lines
    }
}

/// usage 줄 — codex `impl Display for TokenUsage`.
///
/// 캐시가 0 이면 `(+ N cached)` 가, reasoning 이 0 이면 `(reasoning N)` 이
/// 통째로 빠진다. 우리는 reasoning 토큰을 따로 세지 않으므로 그 꼬리는 늘
/// 없다.
fn usage_line(usage: TokenUsage) -> Option<String> {
    if usage.total_tokens_u64() == 0 {
        return None;
    }
    let counts = Counts::of(usage);
    let cached = if counts.cached_input > 0 {
        format!(" (+ {} cached)", with_separators(counts.cached_input))
    } else {
        String::new()
    };
    Some(format!(
        "{USAGE_LABEL} total={} input={}{cached} output={}",
        with_separators(counts.blended_total()),
        with_separators(counts.non_cached_input),
        with_separators(counts.output),
    ))
}

/// 트랜스크립트가 재개할 만한지 — codex `rollout_path_is_resumable`:
/// "파일이고 길이가 0 이 아니다".
fn transcript_is_resumable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// 두 줄을 만든다. 둘 다 설 자리가 없으면 `None`.
#[must_use]
/// `dream_line` 은 호출자가 읽어 넘긴다([`crate::dream::session_end_line`]) —
/// 이 함수는 순수하게 남아 시계도 전역 상태도 읽지 않는다.
pub fn session_summary(
    usage: TokenUsage,
    session_id: &str,
    transcript: &Path,
    dream_line: Option<String>,
) -> Option<SessionSummary> {
    let usage_line = usage_line(usage);
    let resume_hint = transcript_is_resumable(transcript)
        .then(|| format!("{RESUME_COMMAND} {session_id}"));

    if usage_line.is_none() && dream_line.is_none() && resume_hint.is_none() {
        return None;
    }
    Some(SessionSummary {
        usage_line,
        dream_line,
        resume_hint,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use core_types::usage::TokenUsage;

    use super::{session_summary, with_separators};

    fn transcript(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(bytes).expect("write");
        (dir, path)
    }

    /// codex `session_summary_skips_when_no_usage_or_resume_hint`.
    #[test]
    fn nothing_to_say_is_no_summary() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(session_summary(
            TokenUsage::default(),
            "01a03d06",
            &dir.path().join("missing.jsonl"),
            None
        )
        .is_none());
    }

    /// codex `session_summary_skips_resume_hint_until_rollout_exists` —
    /// 빈 트랜스크립트는 재개를 권하지 않는다.
    #[test]
    fn an_empty_transcript_earns_no_resume_hint() {
        let (_dir, path) = transcript(b"");
        assert!(session_summary(TokenUsage::default(), "01a03d06", &path, None).is_none());
    }

    /// codex `session_summary_includes_resume_hint_for_persisted_rollout`.
    #[test]
    fn a_written_transcript_earns_both_lines() {
        let (_dir, path) = transcript(b"{}\n");
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 2,
            ..TokenUsage::default()
        };
        let summary = session_summary(usage, "01a03d06", &path, None).expect("summary");
        assert_eq!(
            summary.exit_lines(/*color*/ false),
            vec![
                "Token usage: total=12 input=10 output=2".to_string(),
                "To continue this session, run zo --resume 01a03d06".to_string(),
            ]
        );
    }

    /// 캡처의 실측 문법 — 쉼표 구분자와 `(+ N cached)` 꼬리.
    #[test]
    fn the_cached_tail_matches_the_capture() {
        let (_dir, path) = transcript(b"{}\n");
        let usage = TokenUsage {
            input_tokens: 7_940,
            output_tokens: 178,
            cache_read_input_tokens: 9_984,
            cache_creation_input_tokens: 0,
            output_tokens_details: None,
        };
        let summary = session_summary(usage, "x", &path, None).expect("summary");
        assert_eq!(
            summary.exit_lines(/*color*/ false)[0],
            "Token usage: total=8,118 input=7,940 (+ 9,984 cached) output=178"
        );
    }

    /// 새로 쓴 캐시는 값을 치른 입력이다 — non-cached 쪽에 붙는다.
    #[test]
    fn cache_writes_count_as_paid_input() {
        let (_dir, path) = transcript(b"{}\n");
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 10,
            cache_creation_input_tokens: 400,
            cache_read_input_tokens: 1_000,
            output_tokens_details: None,
        };
        let summary = session_summary(usage, "x", &path, None).expect("summary");
        assert_eq!(
            summary.exit_lines(/*color*/ false)[0],
            "Token usage: total=510 input=500 (+ 1,000 cached) output=10"
        );
    }

    /// 색을 켜면 명령 토큰만 cyan 이다 — codex 의 `ESC[36m…ESC[39m`.
    #[test]
    fn colour_wraps_only_the_command() {
        let (_dir, path) = transcript(b"{}\n");
        let summary = session_summary(TokenUsage::default(), "01a03d06", &path, None).expect("summary");
        assert_eq!(
            summary.exit_lines(/*color*/ true),
            vec![
                "To continue this session, run \u{1b}[36mzo --resume 01a03d06\u{1b}[39m"
                    .to_string()
            ]
        );
    }

    /// 히스토리 갈래는 같은 문안을 스팬으로 나눈다.
    #[test]
    fn the_history_lines_carry_the_same_words() {
        let (_dir, path) = transcript(b"{}\n");
        let summary = session_summary(TokenUsage::default(), "01a03d06", &path, None).expect("summary");
        let lines = summary.history_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0].plain(),
            "To continue this session, run zo --resume 01a03d06"
        );
        assert_eq!(lines[0].spans[1].style.fg, Some(crate::tui::ansi::Color::CYAN));
    }

    #[test]
    fn separators_land_every_three_digits() {
        assert_eq!(with_separators(0), "0");
        assert_eq!(with_separators(999), "999");
        assert_eq!(with_separators(1_000), "1,000");
        assert_eq!(with_separators(8_118), "8,118");
        assert_eq!(with_separators(1_234_567), "1,234,567");
    }
}
