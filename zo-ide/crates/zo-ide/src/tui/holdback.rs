//! 스트림 원문의 파이프 표 홀드백 스캐너 — codex
//! `tui/src/streaming/table_holdback.rs` 와 `tui/src/table_detect.rs` 를 옮긴
//! 것이다.
//!
//! 표 렌더는 본질적으로 증분이 아니다: 행 하나가 늘면 모든 열 폭이 바뀌고 앞선
//! 행이 다시 잡힌다. 그래서 codex 는 표가 열려 있는 동안 그 구역을 **미확정
//! 꼬리**로 붙잡아 두고(`controller.rs` 머리말: "keeps content from the table
//! header onward as mutable tail until the stream finalizes"), 끝날 때 한 번에
//! 커밋한다.
//!
//! 스캐너는 일부러 보수적이다 — 꼬리가 어디서 시작해야 하는지만 정하고 표
//! 전체를 검증하거나 최종 배치를 예측하지 않는다(원본 머리말).
//!
//! GFM 파이프 표의 정의는 `table_detect.rs` 머리말 그대로다:
//! **헤더 줄**은 파이프로 갈린 칸이 있고 그중 하나 이상이 비어 있지 않다,
//! **구분 줄**은 헤더 바로 아래에서 정렬 표식(`---`·`:---`·`---:`·`:---:`,
//! 대시 셋 이상)만으로 이루어진다, 그 뒤가 **본문 행**이다.

/// 홀드백 판정 — codex `TableHoldbackState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// 표가 없다 — 렌더된 줄 전부가 확정 큐로 흐른다.
    None,
    /// 마지막 비지 않은 줄이 헤더처럼 보이지만 구분 줄이 아직 안 왔다. 다음
    /// 델타가 구분 줄일 수 있으니 그 자리부터 붙잡아 둔다.
    PendingHeader { header_start: usize },
    /// 헤더 + 구분 줄 짝을 봤다 — 헤더부터가 미확정이다.
    Confirmed { table_start: usize },
}

/// 줄이 담장(fenced code block) 안인지 — codex `FenceKind`.
///
/// 홀드백은 `Outside`·`Markdown` 줄만 본다. `Other`(예: `sh`·`rust`) 안의
/// 파이프는 표 문법이 아니라 코드다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceKind {
    Outside,
    Markdown,
    Other,
}

/// 담장의 열림·닫힘을 증분으로 따라가는 것 — codex `FenceTracker`.
///
/// [`Self::advance`] 로 줄을 하나씩 먹이고 [`Self::kind`] 로 지금 문맥을 묻는다.
/// `kind()` 는 **그 줄을 먹이기 전**의 문맥이다 — 지금 줄이 표를 열 수 있는지를
/// 판단하는 쪽이 그것에 기댄다.
#[derive(Debug, Default)]
pub struct FenceTracker {
    state: Option<(char, usize, FenceKind)>,
}

impl FenceTracker {
    #[must_use]
    pub const fn new() -> Self {
        Self { state: None }
    }

    /// 원시 줄 하나로 담장 상태를 갱신한다. 앞 빈칸이 넷 이상이면 담장이 아니라
    /// 들여쓴 코드다(무시). 인용 접두어(`>`)는 먼저 벗긴다.
    pub fn advance(&mut self, raw_line: &str) {
        let leading = raw_line
            .as_bytes()
            .iter()
            .take_while(|byte| **byte == b' ')
            .count();
        if leading > 3 {
            return;
        }
        let trimmed = &raw_line[leading..];
        let scan = strip_blockquote_prefix(trimmed);
        let Some((marker, len)) = parse_fence_marker(scan) else {
            return;
        };
        if let Some((open_char, open_len, _)) = self.state {
            if marker == open_char && len >= open_len && scan[len..].trim().is_empty() {
                self.state = None;
            }
        } else {
            let kind = if is_markdown_fence_info(scan, len) {
                FenceKind::Markdown
            } else {
                FenceKind::Other
            };
            self.state = Some((marker, len, kind));
        }
    }

    #[must_use]
    pub fn kind(&self) -> FenceKind {
        self.state.map_or(FenceKind::Outside, |(_, _, kind)| kind)
    }
}

/// 앞 줄에 대해 기억하는 것 — 표는 "헤더 바로 아래 구분 줄" 로 확정되므로
/// 한 줄 뒤돌아보기로 충분하다.
#[derive(Clone, Copy, Debug)]
struct PreviousLine {
    source_start: usize,
    fence_kind: FenceKind,
    is_header: bool,
}

/// append-only 원문 스트림용 증분 스캐너 — codex `TableHoldbackScanner`.
///
/// [`Self::push_source_chunk`] 는 원문이 스트림에 붙는 **그 순서로** 와야 한다.
/// 스캐너는 그 논리적 버퍼의 바이트 오프셋을 기억하므로 순서가 뒤바뀌면 나중의
/// 꼬리 경계가 엉뚱한 렌더 구역을 가리킨다.
#[derive(Debug)]
pub struct Scanner {
    source_offset: usize,
    fence_tracker: FenceTracker,
    previous_line: Option<PreviousLine>,
    pending_header_start: Option<usize>,
    confirmed_table_start: Option<usize>,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            source_offset: 0,
            fence_tracker: FenceTracker::new(),
            previous_line: None,
            pending_header_start: None,
            confirmed_table_start: None,
        }
    }

    /// 셀 하나가 끝났다 — 홀드백 판정은 비우되 **오프셋은 원문 길이에 맞춘다**.
    ///
    /// 오프셋을 0 으로 되감으면 같은 스트림에 이어 push 했을 때 다음 표 경계가
    /// 원문 좌표와 어긋난 자리를 가리킨다(경계는 누적 원문의 절대 오프셋이다).
    /// 앞부분을 다시 훑지는 않는다: `finish` 가 이미 전부 커밋했으므로 그 앞에
    /// 붙들고 있을 표는 없다.
    pub fn settle(&mut self, source_len: usize) {
        *self = Self::new();
        self.source_offset = source_len;
    }

    /// 확정된 원문 앞부분에 대한 지금의 홀드백 판정.
    #[must_use]
    pub const fn state(&self) -> State {
        if let Some(table_start) = self.confirmed_table_start {
            State::Confirmed { table_start }
        } else if let Some(header_start) = self.pending_header_start {
            State::PendingHeader { header_start }
        } else {
            State::None
        }
    }

    /// 새로 확정된 원문으로 스캐너를 전진시킨다. 조각에는 이제 커밋해도 되는
    /// 원문(보통 개행으로 끝나는 줄들)만 온다 — 끝나지 않은 행을 구조 신호로
    /// 읽지 않게 하기 위한 규율이다(원본 주석).
    pub fn push_source_chunk(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        for line in chunk.split_inclusive('\n') {
            self.push_line(line);
        }
    }

    fn push_line(&mut self, source_line: &str) {
        let line = source_line.strip_suffix('\n').unwrap_or(source_line);
        let source_start = self.source_offset;
        let fence_kind = self.fence_tracker.kind();

        let candidate = if fence_kind == FenceKind::Other {
            None
        } else {
            table_candidate_text(line)
        };
        let is_header = candidate.is_some_and(is_table_header_line);
        let is_delimiter = candidate.is_some_and(is_table_delimiter_line);

        if self.confirmed_table_start.is_none() {
            if let Some(previous) = self.previous_line {
                if previous.fence_kind != FenceKind::Other
                    && fence_kind != FenceKind::Other
                    && previous.is_header
                    && is_delimiter
                {
                    self.confirmed_table_start = Some(previous.source_start);
                    self.pending_header_start = None;
                }
            }
        }

        if self.confirmed_table_start.is_none() && !line.trim().is_empty() {
            if fence_kind != FenceKind::Other && is_header {
                self.pending_header_start = Some(source_start);
            } else {
                self.pending_header_start = None;
            }
        }

        self.previous_line = Some(PreviousLine {
            source_start,
            fence_kind,
            is_header,
        });

        self.fence_tracker.advance(line);
        self.source_offset = self.source_offset.saturating_add(source_line.len());
    }
}

/// 인용 접두어를 벗기고, 파이프 표 조각이 있으면 다듬은 본문을 준다.
/// 인용 안의 표도 표로 보지만, 접두어를 벗긴 뒤 파이프 모양이어야 한다.
fn table_candidate_text(line: &str) -> Option<&str> {
    let stripped = strip_blockquote_prefix(line).trim();
    parse_table_segments(stripped).map(|_| stripped)
}

/// 파이프로 갈린 조각들. 비었거나 이스케이프 안 된 구분자가 없으면 `None`.
/// 바깥 파이프는 벗기고 나눈다. 이스케이프된 파이프(`\|`)는 조각 안에 남는다 —
/// 구조 판정이지 렌더가 아니다.
#[must_use]
pub fn parse_table_segments(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let has_outer_pipe = trimmed.starts_with('|') || trimmed.ends_with('|');
    let content = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let content = content.strip_suffix('|').unwrap_or(content);
    let raw = split_unescaped_pipe(content);
    if !has_outer_pipe && raw.len() <= 1 {
        return None;
    }
    let segments: Vec<&str> = raw.into_iter().map(str::trim).collect();
    (!segments.is_empty()).then_some(segments)
}

fn split_unescaped_pipe(content: &str) -> Vec<&str> {
    let mut segments = Vec::with_capacity(8);
    let bytes = content.as_bytes();
    let mut start = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
        } else if bytes[index] == b'|' {
            segments.push(&content[start..index]);
            start = index + 1;
            index += 1;
        } else {
            index += 1;
        }
    }
    segments.push(&content[start..]);
    segments
}

/// 표 헤더 줄로 보이는지 — 파이프로 갈린 칸 중 하나 이상이 비어 있지 않다.
#[must_use]
pub fn is_table_header_line(line: &str) -> bool {
    parse_table_segments(line)
        .is_some_and(|segments| segments.iter().any(|segment| !segment.is_empty()))
}

fn is_table_delimiter_segment(segment: &str) -> bool {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return false;
    }
    let without_leading = trimmed.strip_prefix(':').unwrap_or(trimmed);
    let without_ends = without_leading
        .strip_suffix(':')
        .unwrap_or(without_leading);
    without_ends.len() >= 3 && without_ends.chars().all(|ch| ch == '-')
}

/// 정렬 표식만으로 이루어진 구분 줄인지.
#[must_use]
pub fn is_table_delimiter_line(line: &str) -> bool {
    parse_table_segments(line)
        .is_some_and(|segments| segments.into_iter().all(is_table_delimiter_segment))
}

/// 담장 표식과 그 길이. 백틱·물결 셋 이상만 담장이다.
#[must_use]
pub fn parse_fence_marker(line: &str) -> Option<(char, usize)> {
    let first = line.as_bytes().first().copied()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let len = line.bytes().take_while(|byte| *byte == first).count();
    (len >= 3).then_some((first as char, len))
}

/// 담장의 정보 문자열이 마크다운을 가리키는지(`md`·`markdown`, 대소문자 무시).
#[must_use]
pub fn is_markdown_fence_info(trimmed_line: &str, marker_len: usize) -> bool {
    let info = trimmed_line[marker_len..]
        .split_whitespace()
        .next()
        .unwrap_or_default();
    info.eq_ignore_ascii_case("md") || info.eq_ignore_ascii_case("markdown")
}

/// 앞의 `>` 인용 표식을 모두 벗긴다 — 표는 인용 안에도 온다(`> | A | B |`).
#[must_use]
pub fn strip_blockquote_prefix(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        let Some(stripped) = rest.strip_prefix('>') else {
            return rest;
        };
        rest = stripped.strip_prefix(' ').unwrap_or(stripped).trim_start();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_table_delimiter_line, is_table_header_line, parse_table_segments, FenceKind,
        FenceTracker, Scanner, State,
    };

    #[test]
    fn segments_split_on_unescaped_pipes_only() {
        assert_eq!(parse_table_segments("| A | B | C |"), Some(vec!["A", "B", "C"]));
        assert_eq!(parse_table_segments("A | B | C"), Some(vec!["A", "B", "C"]));
        assert_eq!(parse_table_segments("just text"), None);
        assert_eq!(parse_table_segments(r"| A \| B | C |"), Some(vec![r"A \| B", "C"]));
    }

    #[test]
    fn a_delimiter_row_needs_three_dashes_per_segment() {
        assert!(is_table_delimiter_line("| --- | --- |"));
        assert!(is_table_delimiter_line("|:---:|---:|"));
        assert!(!is_table_delimiter_line("| -- | -- |"));
        assert!(!is_table_delimiter_line("| A | B |"));
        assert!(is_table_header_line("| A | B |"));
        assert!(!is_table_header_line("| | |"));
    }

    /// codex 의 확정 규칙: 헤더 **바로 아래** 구분 줄이 오면 헤더 오프셋부터
    /// 미확정이다.
    #[test]
    fn a_header_then_delimiter_confirms_the_table_at_the_header_offset() {
        let mut scanner = Scanner::new();
        scanner.push_source_chunk("intro\n");
        assert_eq!(scanner.state(), State::None);
        scanner.push_source_chunk("| A | B |\n");
        assert_eq!(scanner.state(), State::PendingHeader { header_start: 6 });
        scanner.push_source_chunk("| --- | --- |\n");
        assert_eq!(scanner.state(), State::Confirmed { table_start: 6 });
        scanner.push_source_chunk("| 1 | 2 |\n");
        assert_eq!(scanner.state(), State::Confirmed { table_start: 6 });
    }

    /// 헤더처럼 보였지만 구분 줄이 안 오면 붙잡기를 놓는다.
    #[test]
    fn a_pending_header_is_released_when_the_next_line_is_prose() {
        let mut scanner = Scanner::new();
        scanner.push_source_chunk("a | b\n");
        assert!(matches!(scanner.state(), State::PendingHeader { .. }));
        scanner.push_source_chunk("just prose\n");
        assert_eq!(scanner.state(), State::None);
    }

    /// 담장 안의 파이프는 코드다 — codex 는 `Other` 담장을 건너뛴다.
    #[test]
    fn pipes_inside_a_shell_fence_are_not_a_table() {
        let mut scanner = Scanner::new();
        scanner.push_source_chunk("```sh\n");
        scanner.push_source_chunk("ls | wc -l\n");
        scanner.push_source_chunk("--- | ---\n");
        assert_eq!(scanner.state(), State::None);
    }

    #[test]
    fn a_markdown_fence_still_holds_its_tables() {
        let mut scanner = Scanner::new();
        scanner.push_source_chunk("```md\n| A | B |\n| --- | --- |\n");
        assert!(matches!(scanner.state(), State::Confirmed { .. }));
    }

    #[test]
    fn the_fence_tracker_reports_the_context_of_the_line_it_was_given() {
        let mut tracker = FenceTracker::new();
        assert_eq!(tracker.kind(), FenceKind::Outside);
        tracker.advance("````sh");
        assert_eq!(tracker.kind(), FenceKind::Other);
        tracker.advance("```");
        assert_eq!(tracker.kind(), FenceKind::Other, "a shorter marker cannot close");
        tracker.advance("````");
        assert_eq!(tracker.kind(), FenceKind::Outside);
    }
}
