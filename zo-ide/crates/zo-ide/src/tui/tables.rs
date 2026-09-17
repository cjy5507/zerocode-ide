//! 마크다운 표 렌더 — codex `tui/src/markdown_render.rs` 의 표 파이프라인과
//! `markdown_render/table_key_value.rs` 를 옮긴 것이다.
//!
//! 원본 머리말이 적는 다섯 단계를 그대로 돈다:
//!
//! 1. **흘러넘친 행 걸러내기** — pulldown-cmark 는 파이프 없는 본문 행도 받으므로
//!    뒤따르는 문단이 한 칸짜리 표 행으로 빨려 든다. 그것을 표 뒤 평문으로 뺀다.
//! 2. **열 수 맞추기** — 모든 행을 정렬 개수에 맞춰 자르거나 채운다.
//! 3. **열 폭 배분** — 내용 성격별 우선순위로 배분하고 모자라면 줄여 간다.
//! 4. **표현 고르기** — 값이 아직 읽히는 동안은 구분선 있는 격자, 더는 못 읽히면
//!    본문 행을 키/값 레코드로 세운다.
//! 5. **넘친 행 덧붙이기** — 1단계에서 뺀 것을 표 뒤에 평문으로.
//!
//! 폭 배분 규칙도 원본 그대로다: 열은 서술(Narrative)·토큰중심(TokenHeavy)·
//! 짧은값(Compact) 으로 갈리고, 토큰중심이 서술보다 먼저 폭을 내놓아 긴 경로가
//! 읽히는 산문을 짓누르지 않게 한다. 짧은 값은 마지막까지 지킨다.
//!
//! 헤더는 활성 syntect 테마의 타입 스코프에서, 구분선은 측정한 터미널
//! 전경을 배경 쪽으로 20% 블렌드해 만든다. OSC 기본색이나 팔레트 인식 색을
//! 쓸 수 없으면 캡처와 같은 dim 구분선으로 떨어진다.

use pulldown_cmark::Alignment;

use super::ansi::{Line, Span, Style};
use super::palette;
use super::wrap::wrap_line;

/// 열 사이의 빈칸 — codex `TABLE_COLUMN_GAP`.
const COLUMN_GAP: usize = 2;
/// 칸 좌우의 여백 — codex `TABLE_CELL_PADDING`.
const CELL_PADDING: usize = 1;
/// 헤더 아래 굵은 선 — codex `TABLE_HEADER_SEPARATOR_CHAR`.
const HEADER_SEPARATOR_CHAR: char = '━';
/// 본문 행 사이 가는 선 — codex `TABLE_BODY_SEPARATOR_CHAR`.
const BODY_SEPARATOR_CHAR: char = '─';
/// 열 하나가 내려갈 수 있는 바닥.
const MIN_COLUMN_WIDTH: usize = 3;

// key/value 레코드 표현의 상수 — codex `table_key_value.rs` 그대로.
const FIELD_LEADING_PADDING: usize = 1;
const FIELD_GAP: usize = 2;
const MIN_VALUE_WIDTH: usize = 3;
const MIN_ALIGNED_COMPACT_VALUE_WIDTH: usize = 12;
const MIN_ALIGNED_EXPANSIVE_VALUE_WIDTH: usize = 24;
const MIN_SCANNABLE_NARRATIVE_WIDTH: usize = 12;
const MIN_SCANNABLE_TOKEN_HEAVY_WIDTH: usize = 12;
const CRAMPED_EXPANSIVE_CELL_LINES: usize = 4;
const CATASTROPHIC_NARRATIVE_CELL_LINES: usize = 7;
const STACKED_VALUE_INDENT: usize = 2;

/// 표 한 칸의 스타일 있는 내용. 칸 안의 하드 브레이크마다 줄이 하나 늘어난다.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    lines: Vec<Line>,
}

impl Cell {
    fn ensure_line(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(Line::empty());
        }
    }

    /// 칸의 지금 줄에 스팬을 붙인다.
    pub fn push_span(&mut self, span: Span) {
        self.ensure_line();
        if let Some(line) = self.lines.last_mut() {
            line.spans.push(span);
        }
    }

    /// 칸 안의 줄바꿈.
    pub fn hard_break(&mut self) {
        self.lines.push(Line::empty());
    }

    /// 폭 재기용 평문 — 칸 안의 줄들을 빈칸 하나로 이어 붙인다.
    #[must_use]
    fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(Line::plain)
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn display_width(&self) -> usize {
        self.lines.iter().map(Line::width).max().unwrap_or(0)
    }

    /// 칸 내용을 폭 `width` 로 접는다. 빈 칸은 빈 줄 하나를 낸다 — 격자가
    /// 어긋나지 않게.
    fn wrapped(&self, width: usize) -> Vec<Line> {
        if self.lines.is_empty() {
            return vec![Line::empty()];
        }
        let mut out = Vec::new();
        for line in &self.lines {
            out.extend(wrap_line(line, width.max(1), &Span::raw("")));
        }
        if out.is_empty() {
            out.push(Line::empty());
        }
        out
    }
}

/// 본문 한 행 — 파이프 문법으로 왔는지가 흘러넘침 판정에 쓰인다.
#[derive(Debug)]
struct BodyRow {
    cells: Vec<Cell>,
    has_pipe_syntax: bool,
}

/// pulldown-cmark 의 표 사건을 모으는 자리 — codex `TableState`.
///
/// `Tag::Table` 에서 태어나 `TagEnd::Table` 에서 소비된다. 그 사이 칸 내용은
/// `current_cell` 로 흘러들고, `TagEnd::TableCell` 에서 `current_row` 로,
/// 행이 끝나면 `header`/`rows` 로 옮겨진다.
#[derive(Debug)]
pub struct State {
    alignments: Vec<Alignment>,
    header: Option<Vec<Cell>>,
    rows: Vec<BodyRow>,
    current_row: Option<Vec<Cell>>,
    current_row_has_pipe_syntax: bool,
    current_cell: Option<Cell>,
    in_header: bool,
}

impl State {
    #[must_use]
    pub fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            header: None,
            rows: Vec::new(),
            current_row: None,
            current_row_has_pipe_syntax: false,
            current_cell: None,
            in_header: false,
        }
    }

    /// 정렬 개수 = 열 수.
    #[must_use]
    pub fn column_count(&self) -> usize {
        self.alignments.len()
    }

    pub fn start_head(&mut self) {
        self.in_header = true;
        self.current_row = Some(Vec::new());
    }

    pub fn end_head(&mut self) {
        if let Some(cell) = self.current_cell.take() {
            self.current_row.get_or_insert_with(Vec::new).push(cell);
        }
        if let Some(row) = self.current_row.take() {
            self.header = Some(row);
        }
        self.in_header = false;
    }

    pub fn start_row(&mut self, has_pipe_syntax: bool) {
        self.current_row = Some(Vec::new());
        self.current_row_has_pipe_syntax = has_pipe_syntax;
    }

    pub fn end_row(&mut self) {
        if let Some(cell) = self.current_cell.take() {
            self.current_row.get_or_insert_with(Vec::new).push(cell);
        }
        let Some(row) = self.current_row.take() else {
            return;
        };
        if self.in_header {
            self.header = Some(row);
        } else {
            self.rows.push(BodyRow {
                cells: row,
                has_pipe_syntax: self.current_row_has_pipe_syntax,
            });
        }
        self.current_row_has_pipe_syntax = false;
    }

    pub fn start_cell(&mut self) {
        self.current_cell = Some(Cell::default());
    }

    pub fn end_cell(&mut self) {
        if let Some(cell) = self.current_cell.take() {
            self.current_row.get_or_insert_with(Vec::new).push(cell);
        }
    }

    /// 열려 있는 칸 — 텍스트·코드·줄바꿈이 여기로 들어간다.
    pub fn cell_mut(&mut self) -> Option<&mut Cell> {
        self.current_cell.as_mut()
    }
}

/// 표 하나가 낸 줄들 — 접기 정책이 갈린다.
///
/// `prewrapped` 인 줄은 이미 폭에 맞춰 나왔으므로 다시 접지 않는다. 헤더만
/// 있는 표가 파이프 폴백으로 떨어질 때만 거짓이다(그때는 평문 줄이므로 보통
/// 규칙대로 접힌다).
#[derive(Debug)]
pub struct Rendered {
    pub lines: Vec<Line>,
    pub prewrapped: bool,
    pub spillover: Vec<Line>,
}

/// 열의 성격 — 폭을 내놓는 순서를 정한다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColumnKind {
    /// 긴 산문(칸당 평균 4낱말 이상 또는 평균 폭 28 이상).
    Narrative,
    /// 경로·URL·해시처럼 긴 토큰이 지배하는 열.
    TokenHeavy,
    /// 수·상태 낱말처럼 짧아서 접히면 안 되는 값.
    Compact,
}

/// 폭 배분이 보는 열별 통계 — 줄이기 전에 한 번에 모은다.
#[derive(Clone, Debug)]
struct ColumnMetrics {
    /// 헤더와 본문을 통틀어 가장 넓은 칸의 표시 폭.
    max_width: usize,
    /// 헤더에서 가장 긴 낱말의 폭.
    header_token_width: usize,
    /// 본문에서 가장 긴 낱말의 폭.
    body_token_width: usize,
    kind: ColumnKind,
}

/// 헤더 스타일 — codex 는 syntect 스코프에서 만들고 `.bold()` 를 얹는다.
fn header_style() -> Style {
    super::highlight::foreground_style_for_scopes(&[
        "entity.name.type",
        "support.type",
        "variable",
    ])
    .unwrap_or_else(|| Style::new().fg(palette::TABLE_HEADER))
    .bold()
}

/// 구분선 스타일 — codex `table_separator_style_for` 는 터미널 전경·배경을
/// 모르면 `dim` 으로 떨어진다. PTY 캡처가 OSC 10/11 에 답하지 않으므로 실측도
/// `ESC[2m` 이다.
fn separator_style() -> Style {
    separator_style_for_palette(palette::terminal_palette())
}

fn separator_style_for_palette(terminal_palette: Option<palette::TerminalPalette>) -> Style {
    let Some(terminal_palette) = terminal_palette else {
        return Style::new().dim();
    };
    if terminal_palette.color_level() == palette::ColorLevel::Ansi16 {
        return Style::new().dim();
    }
    let (red, green, blue) = palette::blend_rgb(
        terminal_palette.foreground(),
        terminal_palette.background(),
        0.20,
    );
    Style::new().fg(super::ansi::Color::Rgb(red, green, blue))
}

/// 표 하나를 줄로. `table_width` 는 여백·간격을 뺀 **내용** 예산이고
/// `record_width` 는 레코드 폴백이 쓰는 전체 예산이다(둘 다 codex 의
/// `available_table_width`/`available_record_width`).
#[must_use]
pub fn render(mut state: State, table_width: Option<usize>, record_width: Option<usize>) -> Rendered {
    let column_count = state.alignments.len();
    if column_count == 0 {
        return Rendered {
            lines: Vec::new(),
            prewrapped: true,
            spillover: Vec::new(),
        };
    }

    let mut spillover_rows: Vec<Cell> = Vec::new();
    let mut rows: Vec<Vec<Cell>> = Vec::with_capacity(state.rows.len());
    for index in 0..state.rows.len() {
        let row = &state.rows[index];
        let next = state.rows.get(index + 1);
        if column_count > 1 && is_spillover_row(row, next) {
            if let Some(cell) = row.cells.first().cloned() {
                spillover_rows.push(cell);
            }
        } else {
            rows.push(row.cells.clone());
        }
    }

    let mut header = state
        .header
        .take()
        .unwrap_or_else(|| vec![Cell::default(); column_count]);
    normalize_row(&mut header, column_count);
    for row in &mut rows {
        normalize_row(row, column_count);
    }

    let metrics = collect_metrics(&header, &rows, column_count);
    let widths = compute_column_widths(&metrics, table_width);
    let spillover: Vec<Line> = spillover_rows
        .into_iter()
        .flat_map(|cell| cell.lines)
        .collect();

    let Some(column_widths) = widths else {
        if rows.is_empty() {
            return Rendered {
                lines: pipe_fallback(&header, &rows, &state.alignments),
                prewrapped: false,
                spillover,
            };
        }
        return Rendered {
            lines: render_records(&header, &rows, &metrics, record_width),
            prewrapped: true,
            spillover,
        };
    };

    if should_render_records(&rows, &column_widths, &metrics) {
        return Rendered {
            lines: render_records(&header, &rows, &metrics, record_width),
            prewrapped: true,
            spillover,
        };
    }

    let mut out = Vec::with_capacity(2 + rows.len() * 2);
    out.extend(render_row(
        &header,
        &column_widths,
        &state.alignments,
        header_style(),
    ));
    out.push(render_separator(&column_widths, HEADER_SEPARATOR_CHAR));
    for (index, row) in rows.iter().enumerate() {
        out.extend(render_row(
            row,
            &column_widths,
            &state.alignments,
            Style::new(),
        ));
        if index + 1 < rows.len() {
            out.push(render_separator(&column_widths, BODY_SEPARATOR_CHAR));
        }
    }
    Rendered {
        lines: out,
        prewrapped: true,
        spillover,
    }
}

/// 여백·간격을 뺀 내용 예산 — codex `available_table_width`.
#[must_use]
pub fn content_budget(wrap_width: usize, prefix_width: usize, column_count: usize) -> usize {
    let reserved = prefix_width
        + (column_count.saturating_sub(1) * COLUMN_GAP)
        + (column_count * CELL_PADDING * 2);
    wrap_width.saturating_sub(reserved)
}

fn normalize_row(row: &mut Vec<Cell>, column_count: usize) {
    row.truncate(column_count);
    row.resize(column_count, Cell::default());
}

/// 열 폭을 배분한다. 열마다 자연 폭에서 시작해 우선순위대로 줄이고, 열당 최소
/// 폭(3칸)조차 못 들어가면 `None` — 호출자가 레코드/파이프 폴백으로 간다.
fn compute_column_widths(
    metrics: &[ColumnMetrics],
    available_width: Option<usize>,
) -> Option<Vec<usize>> {
    let mut widths: Vec<usize> = metrics
        .iter()
        .map(|column| column.max_width.max(MIN_COLUMN_WIDTH))
        .collect();

    // 폭을 모르면 자연 폭 그대로다 — codex `compute_column_widths` 의
    // `let Some(max_width) = available_width else { return Some(widths) }`.
    let Some(max_width) = available_width else {
        return Some(widths);
    };
    let minimum_total = metrics.len() * MIN_COLUMN_WIDTH;
    if max_width < minimum_total {
        return None;
    }

    let mut floors: Vec<usize> = metrics.iter().map(preferred_column_floor).collect();
    let floor_total: usize = floors.iter().sum();
    if floor_total > max_width {
        let minimums = vec![MIN_COLUMN_WIDTH; floors.len()];
        shrink_columns(&mut floors, &minimums, metrics, floor_total - max_width);
    }

    let total_width: usize = widths.iter().sum();
    if total_width > max_width {
        let remaining = shrink_columns(&mut widths, &floors, metrics, total_width - max_width);
        if remaining > 0 {
            return None;
        }
    }

    Some(widths)
}

fn collect_metrics(header: &[Cell], rows: &[Vec<Cell>], column_count: usize) -> Vec<ColumnMetrics> {
    let mut metrics = Vec::with_capacity(column_count);
    for column in 0..column_count {
        let header_cell = &header[column];
        let header_plain = header_cell.plain_text();
        let header_token_width = longest_token_width(&header_plain);
        let mut max_width = header_cell.display_width();
        let mut body_token_width = 0usize;
        let mut body_token_count = 0usize;
        let mut long_body_token_count = 0usize;
        let mut total_words = 0usize;
        let mut total_cells = 0usize;
        let mut total_cell_width = 0usize;

        for row in rows {
            let cell = &row[column];
            max_width = max_width.max(cell.display_width());
            let plain = cell.plain_text();
            let mut word_count = 0usize;
            for token in plain.split_whitespace() {
                let token_width = display_width(token);
                body_token_width = body_token_width.max(token_width);
                long_body_token_count += usize::from(token_width >= 20);
                word_count += 1;
            }
            if word_count > 0 {
                body_token_count += word_count;
                total_words += word_count;
                total_cells += 1;
                total_cell_width += display_width(&plain);
            }
        }

        #[allow(clippy::cast_precision_loss)] // codex 도 f64 로 평균을 본다.
        let avg_words_per_cell = if total_cells == 0 {
            header_plain.split_whitespace().count() as f64
        } else {
            total_words as f64 / total_cells as f64
        };
        #[allow(clippy::cast_precision_loss)]
        let avg_cell_width = if total_cells == 0 {
            display_width(&header_plain) as f64
        } else {
            total_cell_width as f64 / total_cells as f64
        };
        let kind = if long_body_token_count > 0
            && long_body_token_count >= body_token_count.saturating_sub(long_body_token_count)
        {
            ColumnKind::TokenHeavy
        } else if avg_words_per_cell >= 4.0 || avg_cell_width >= 28.0 {
            ColumnKind::Narrative
        } else {
            ColumnKind::Compact
        };

        metrics.push(ColumnMetrics {
            max_width,
            header_token_width,
            body_token_width,
            kind,
        });
    }
    metrics
}

/// 줄이기 전에 지켜 줄 바닥 — 서술·토큰중심은 16칸, 짧은값은 헤더 낱말과
/// 본문 낱말(16 상한) 중 큰 쪽. 결과는 `[3, max_width]` 로 조인다.
fn preferred_column_floor(metrics: &ColumnMetrics) -> usize {
    let token_target = match metrics.kind {
        ColumnKind::Narrative | ColumnKind::TokenHeavy => 16,
        ColumnKind::Compact => metrics
            .header_token_width
            .max(metrics.body_token_width.min(16)),
    };
    token_target.max(MIN_COLUMN_WIDTH).min(metrics.max_width)
}

/// 우선순위대로 열을 줄인다 — 토큰중심 → 서술 → 짧은값. 같은 성격 안에서는
/// 바닥 위 여유가 큰 열이 먼저 내놓아 비슷한 열끼리 균형이 남는다. 한 칸씩
/// 반복해서 줄이는 것과 같은 결과를 이분 탐색으로 낸다(codex 주석).
fn shrink_columns(
    widths: &mut [usize],
    floors: &[usize],
    metrics: &[ColumnMetrics],
    mut amount: usize,
) -> usize {
    for kind in [
        ColumnKind::TokenHeavy,
        ColumnKind::Narrative,
        ColumnKind::Compact,
    ] {
        let slack = |widths: &[usize], index: usize| widths[index].saturating_sub(floors[index]);
        let slack_total: usize = (0..widths.len())
            .filter(|index| metrics[*index].kind == kind)
            .map(|index| slack(widths, index))
            .sum();
        let to_remove = amount.min(slack_total);
        if to_remove == 0 {
            continue;
        }

        let mut low = 0usize;
        let mut high = (0..widths.len())
            .filter(|index| metrics[*index].kind == kind)
            .map(|index| slack(widths, index))
            .max()
            .unwrap_or(0);
        while low < high {
            let cap = low + (high - low) / 2;
            let removed: usize = (0..widths.len())
                .filter(|index| metrics[*index].kind == kind)
                .map(|index| slack(widths, index).saturating_sub(cap))
                .sum();
            if removed > to_remove {
                low = cap + 1;
            } else {
                high = cap;
            }
        }

        let cap = low;
        let mut removed = 0usize;
        for (index, width) in widths.iter_mut().enumerate() {
            if metrics[index].kind != kind {
                continue;
            }
            let reduction = width.saturating_sub(floors[index]).saturating_sub(cap);
            *width -= reduction;
            removed += reduction;
        }

        let mut remainder = to_remove - removed;
        for (index, width) in widths.iter_mut().enumerate() {
            if remainder == 0 {
                break;
            }
            if metrics[index].kind == kind && width.saturating_sub(floors[index]) == cap {
                *width -= 1;
                remainder -= 1;
            }
        }

        amount -= to_remove;
        if amount == 0 {
            break;
        }
    }
    amount
}

fn render_separator(column_widths: &[usize], separator: char) -> Line {
    let segment = separator.to_string();
    let gap = " ".repeat(COLUMN_GAP);
    let text = column_widths
        .iter()
        .map(|width| segment.repeat(*width + (CELL_PADDING * 2)))
        .collect::<Vec<_>>()
        .join(&gap);
    Line::new(vec![Span::new(text, separator_style())])
}

/// 한 행 — 칸을 접고, 가장 오른쪽의 **내용 있는** 열까지만 채운다(그 뒤의
/// 여백은 찍지 않는다). 행 스타일은 줄 스타일로 나간다.
fn render_row(
    row: &[Cell],
    column_widths: &[usize],
    alignments: &[Alignment],
    row_style: Style,
) -> Vec<Line> {
    let wrapped: Vec<Vec<Line>> = row
        .iter()
        .zip(column_widths)
        .map(|(cell, width)| cell.wrapped(*width))
        .collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);

    let mut out = Vec::with_capacity(height);
    for index in 0..height {
        let Some(last_visible) = wrapped
            .iter()
            .rposition(|lines| lines.get(index).is_some_and(|line| line.width() > 0))
        else {
            out.push(Line::empty().styled(row_style));
            continue;
        };
        let mut spans = Vec::new();
        for (column, width) in column_widths.iter().enumerate().take(last_visible + 1) {
            spans.push(Span::raw(" ".repeat(CELL_PADDING)));
            let mut line = wrapped[column].get(index).cloned().unwrap_or_default();
            let remaining = width.saturating_sub(line.width());
            let (left, right) = match alignments.get(column).copied().unwrap_or(Alignment::None) {
                Alignment::Left | Alignment::None => (0, remaining),
                Alignment::Center => (remaining / 2, remaining - (remaining / 2)),
                Alignment::Right => (remaining, 0),
            };
            if left > 0 {
                spans.push(Span::raw(" ".repeat(left)));
            }
            spans.append(&mut line.spans);
            if column == last_visible {
                continue;
            }
            if right > 0 {
                spans.push(Span::raw(" ".repeat(right)));
            }
            spans.push(Span::raw(" ".repeat(CELL_PADDING)));
            spans.push(Span::raw(" ".repeat(COLUMN_GAP)));
        }
        out.push(Line::new(spans).styled(row_style));
    }
    out
}

/// 헤더만 있는 표는 파이프 줄 그대로 — codex `render_table_pipe_fallback`.
/// 칸 안의 `|` 는 `\|` 로 이스케이프해 경계가 살아 있게 한다.
fn pipe_fallback(header: &[Cell], rows: &[Vec<Cell>], alignments: &[Alignment]) -> Vec<Line> {
    let mut out = vec![row_to_pipe_line(header)];
    out.push(Line::from_text(alignments_to_pipe_delimiter(alignments)));
    out.extend(rows.iter().map(|row| row_to_pipe_line(row)));
    out
}

fn row_to_pipe_line(row: &[Cell]) -> Line {
    let mut text = String::from("|");
    for cell in row {
        text.push(' ');
        for (index, line) in cell.lines.iter().enumerate() {
            if index > 0 {
                text.push(' ');
            }
            for ch in line.plain().chars() {
                if ch == '|' {
                    text.push_str("\\|");
                } else {
                    text.push(ch);
                }
            }
        }
        text.push_str(" |");
    }
    Line::from_text(text)
}

fn alignments_to_pipe_delimiter(alignments: &[Alignment]) -> String {
    let mut out = String::from("|");
    for alignment in alignments {
        out.push_str(match alignment {
            Alignment::Left => ":---",
            Alignment::Center => ":---:",
            Alignment::Right => "---:",
            Alignment::None => "---",
        });
        out.push('|');
    }
    out
}

/// pulldown-cmark 의 느슨한 표 파싱이 만든 행인지 — codex `is_spillover_row`.
///
/// 파이프 없는 본문 행이 허용되므로 표 뒤 문단이 한 칸짜리 행으로 빨려 든다.
/// 판정: 내용 있는 칸이 첫 칸뿐이고, 그리고 (한 칸 행인데 파이프 문법이
/// 아니었거나, 내용이 HTML 처럼 보이거나, HTML 이 뒤따르는 이름표 줄이거나,
/// 꼬리의 HTML 안내 이름표) 일 때다.
fn is_spillover_row(row: &BodyRow, next: Option<&BodyRow>) -> bool {
    let Some(first_text) = first_non_empty_only_text(&row.cells) else {
        return false;
    };
    if row.cells.len() == 1 && !row.has_pipe_syntax {
        return true;
    }
    if looks_like_html_content(&first_text) {
        return true;
    }
    if first_text.trim_end().ends_with(':') {
        if next
            .and_then(|row| first_non_empty_only_text(&row.cells))
            .is_some_and(|text| looks_like_html_content(&text))
        {
            return true;
        }
        if next.is_none() && looks_like_html_label_line(&first_text) {
            return true;
        }
    }
    false
}

fn first_non_empty_only_text(row: &[Cell]) -> Option<String> {
    let first = row.first()?.plain_text();
    if first.trim().is_empty() {
        return None;
    }
    let rest_empty = row[1..]
        .iter()
        .all(|cell| cell.plain_text().trim().is_empty());
    rest_empty.then_some(first)
}

fn looks_like_html_content(text: &str) -> bool {
    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'<' {
            continue;
        }
        let mut start = index + 1;
        if bytes.get(start).is_some_and(|byte| *byte == b'/' || *byte == b'!') {
            start += 1;
        }
        if bytes.get(start).is_some_and(u8::is_ascii_alphabetic)
            && bytes
                .get(start + 1..)
                .is_some_and(|suffix| suffix.contains(&b'>'))
        {
            return true;
        }
    }
    false
}

fn looks_like_html_label_line(text: &str) -> bool {
    let trimmed = text.trim();
    if !trimmed.ends_with(':') {
        return false;
    }
    trimmed
        .trim_end_matches(':')
        .split_whitespace()
        .any(|word| word.eq_ignore_ascii_case("html"))
}

// ============================================================================
// key/value 레코드 표현 — codex `markdown_render/table_key_value.rs`
// ============================================================================

/// 격자로는 더 이상 못 읽는 행이 충분히 쌓였는지 — 값이 쓸모 있는 덩어리로
/// 안 나뉘거나, 넓은 칸이 좁고 긴 띠로 무너질 때 표현을 바꾼다.
fn should_render_records(
    rows: &[Vec<Cell>],
    column_widths: &[usize],
    metrics: &[ColumnMetrics],
) -> bool {
    if rows.is_empty() {
        return false;
    }
    let affected = rows
        .iter()
        .filter(|row| {
            let fragmented = row
                .iter()
                .zip(column_widths)
                .zip(metrics)
                .any(|((cell, width), metrics)| {
                    let has_fragmented_token = cell
                        .plain_text()
                        .split_whitespace()
                        .any(|token| display_width(token) > *width);
                    match metrics.kind {
                        ColumnKind::Compact => has_fragmented_token,
                        ColumnKind::TokenHeavy => {
                            *width < MIN_SCANNABLE_TOKEN_HEAVY_WIDTH && has_fragmented_token
                        }
                        ColumnKind::Narrative => false,
                    }
                });
            fragmented || expansive_cells_are_starved(row, column_widths, metrics)
        })
        .count();
    let threshold = if rows.len() == 1 {
        1
    } else {
        2.max(rows.len().div_ceil(3))
    };
    affected >= threshold
}

fn expansive_cells_are_starved(
    row: &[Cell],
    column_widths: &[usize],
    metrics: &[ColumnMetrics],
) -> bool {
    let expansive: Vec<(ColumnKind, usize, usize)> = row
        .iter()
        .zip(column_widths)
        .zip(metrics)
        .filter(|&((_, _), metrics)| metrics.kind != ColumnKind::Compact)
        .map(|((cell, width), metrics)| (metrics.kind, *width, cell.wrapped(*width).len()))
        .collect();

    expansive
        .iter()
        .filter(|(_, _, height)| *height >= CRAMPED_EXPANSIVE_CELL_LINES)
        .count()
        >= 2
        || expansive.iter().any(|(kind, width, height)| {
            *kind == ColumnKind::Narrative
                && *width < MIN_SCANNABLE_NARRATIVE_WIDTH
                && *height >= CATASTROPHIC_NARRATIVE_CELL_LINES
        })
}

fn render_records(
    headers: &[Cell],
    rows: &[Vec<Cell>],
    metrics: &[ColumnMetrics],
    available_width: Option<usize>,
) -> Vec<Line> {
    let label_width = headers
        .iter()
        .map(|header| display_width(&header.plain_text()))
        .max()
        .unwrap_or(0);
    let minimum_value_width = if metrics
        .iter()
        .any(|metrics| metrics.kind != ColumnKind::Compact)
    {
        MIN_ALIGNED_EXPANSIVE_VALUE_WIDTH
    } else {
        MIN_ALIGNED_COMPACT_VALUE_WIDTH
    };
    let aligned = available_width.is_none_or(|width| {
        FIELD_LEADING_PADDING + label_width + FIELD_GAP + minimum_value_width <= width
    });

    let mut out: Vec<Line> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        for (header, value) in headers.iter().zip(row) {
            if aligned {
                render_aligned_field(&mut out, header, value, label_width, available_width);
            } else {
                render_stacked_field(&mut out, header, value, available_width);
            }
        }
        if index + 1 < rows.len() {
            let width = available_width.unwrap_or_else(|| widest_line_width(&out));
            out.push(Line::new(vec![Span::new(
                BODY_SEPARATOR_CHAR.to_string().repeat(width),
                separator_style(),
            )]));
        }
    }
    out
}

fn render_aligned_field(
    out: &mut Vec<Line>,
    header: &Cell,
    value: &Cell,
    label_width: usize,
    available_width: Option<usize>,
) {
    let value_indent = FIELD_LEADING_PADDING + label_width + FIELD_GAP;
    let value_width = available_width.map_or_else(
        || value.display_width().max(MIN_VALUE_WIDTH),
        |width| width.saturating_sub(value_indent).max(MIN_VALUE_WIDTH),
    );
    for (index, mut line) in value.wrapped(value_width).into_iter().enumerate() {
        let mut spans = Vec::new();
        if index == 0 {
            let label = header.plain_text();
            spans.push(Span::raw(" ".repeat(FIELD_LEADING_PADDING)));
            let pad = label_width.saturating_sub(display_width(&label)) + FIELD_GAP;
            spans.push(Span::new(label, header_style()));
            spans.push(Span::raw(" ".repeat(pad)));
        } else {
            spans.push(Span::raw(" ".repeat(value_indent)));
        }
        spans.append(&mut line.spans);
        out.push(Line::new(spans));
    }
}

fn render_stacked_field(
    out: &mut Vec<Line>,
    header: &Cell,
    value: &Cell,
    available_width: Option<usize>,
) {
    let label_width = available_width.map_or_else(
        || display_width(&header.plain_text()).max(1),
        |width| width.saturating_sub(FIELD_LEADING_PADDING).max(1),
    );
    let label = Line::new(vec![Span::new(header.plain_text(), header_style())]);
    for mut wrapped in wrap_line(&label, label_width, &Span::raw("")) {
        let mut spans = vec![Span::raw(" ".repeat(FIELD_LEADING_PADDING))];
        spans.append(&mut wrapped.spans);
        out.push(Line::new(spans));
    }

    let value_width = available_width.map_or_else(
        || value.display_width().max(1),
        |width| width.saturating_sub(STACKED_VALUE_INDENT).max(1),
    );
    for mut line in value.wrapped(value_width) {
        let mut spans = vec![Span::raw(" ".repeat(STACKED_VALUE_INDENT))];
        spans.append(&mut line.spans);
        out.push(Line::new(spans));
    }
}

fn widest_line_width(lines: &[Line]) -> usize {
    lines.iter().map(Line::width).max().unwrap_or(0)
}

fn display_width(text: &str) -> usize {
    super::ansi::str_width(text)
}

fn longest_token_width(text: &str) -> usize {
    text.split_whitespace().map(display_width).max().unwrap_or(0)
}

#[cfg(test)]
mod palette_tests {
    use super::separator_style_for_palette;
    use crate::tui::ansi::{Color, Style};
    use crate::tui::palette::{ColorLevel, TerminalPalette};

    #[test]
    fn separator_blends_twenty_percent_foreground_when_colour_is_available() {
        let palette = TerminalPalette::new(
            (240, 240, 240),
            (20, 20, 20),
            ColorLevel::TrueColor,
        );
        assert_eq!(
            separator_style_for_palette(Some(palette)),
            Style::new().fg(Color::Rgb(64, 64, 64))
        );
    }

    #[test]
    fn separator_dims_without_palette_aware_colour() {
        let ansi16 = TerminalPalette::new(
            (240, 240, 240),
            (20, 20, 20),
            ColorLevel::Ansi16,
        );
        assert_eq!(separator_style_for_palette(None), Style::new().dim());
        assert_eq!(separator_style_for_palette(Some(ansi16)), Style::new().dim());
    }
}
