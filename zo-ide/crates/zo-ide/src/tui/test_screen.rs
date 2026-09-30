//! 시험용 터미널 — 바이트가 아니라 **화면이 어떻게 보이는가**를 재는 자.
//!
//! painter 시험 중 몇은 어떤 바이트가 나갔는지가 아니라 그 바이트가 화면을
//! 어떻게 만드는지를 물어야 한다: 바뀐 칸만 다시 쓴 프레임이 행을 통째로 다시
//! 쓴 프레임과 같은 화면을 만드는가. 이 터미널은 painter 가 내는 문법만
//! 안다 — CUP·CUF·CHA, EL·ED, SGR, DECSTBM, 줄바꿈·역인덱스 — 그리고
//! **글자·너비·색·속성**을 칸마다 든다. 모르는 시퀀스나 폭 0 글자는 조용히
//! 넘기지 않고 멈춘다: 시험이 모르는 바이트로 초록이 되면 안 된다.
//!
//! 넓은 글자는 xterm 처럼 두 칸을 쓴다. 한 칸만 덮어쓰면 남은 반쪽이 비워진다.

use super::ansi::{char_width, Color, Line, Span, Style};

/// 한 칸이 담은 것.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Glyph {
    /// 지워진 칸.
    Blank,
    Narrow(char),
    /// 넓은 글자의 앞 칸. 뒤 칸은 [`Glyph::WideTail`] 이다.
    WideHead(char),
    WideTail,
}

/// 한 칸: 담은 것과 그 스타일.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ScreenCell {
    pub(super) glyph: Glyph,
    pub(super) style: Style,
}

impl ScreenCell {
    const BLANK: Self = Self { glyph: Glyph::Blank, style: Style::new() };

    /// 눈에 보이는 모양만 — 빈칸에서는 글자색·굵기·희미함·기울임이 안 보이므로
    /// 떼고, 지워진 칸과 글자 없는 공백 한 칸을 같은 것으로 본다.
    pub(super) fn looks(self) -> (Glyph, Style) {
        match self.glyph {
            Glyph::Blank | Glyph::Narrow(' ') => (
                Glyph::Blank,
                Style { fg: None, bold: false, dim: false, italic: false, ..self.style },
            ),
            glyph => (glyph, self.style),
        }
    }
}

/// 격자와 스크롤백을 가진 작은 터미널.
#[derive(Clone, Debug)]
pub(super) struct TestScreen {
    cols: usize,
    grid: Vec<Vec<ScreenCell>>,
    /// 맨 위 행을 밀어낸 줄들 — 위쪽 여백이 화면 첫 행일 때만 쌓인다.
    pub(super) scrollback: Vec<Vec<ScreenCell>>,
    row: usize,
    col: usize,
    pen: Style,
    top_margin: usize,
    bottom_margin: usize,
}

impl TestScreen {
    pub(super) fn new(cols: u16, rows: u16) -> Self {
        let (cols, rows) = (usize::from(cols), usize::from(rows));
        Self {
            cols,
            grid: vec![vec![ScreenCell::BLANK; cols]; rows],
            scrollback: Vec::new(),
            row: 0,
            col: 0,
            pen: Style::new(),
            top_margin: 0,
            bottom_margin: rows - 1,
        }
    }

    pub(super) fn rows(&self) -> usize {
        self.grid.len()
    }

    /// 한 행의 모양 — 서로 다른 바이트로 그린 두 화면을 비교하는 값.
    pub(super) fn looks(&self, row: usize) -> Vec<(Glyph, Style)> {
        self.grid[row].iter().map(|cell| cell.looks()).collect()
    }

    /// 터미널의 커서 (열, 행), 0 기준.
    pub(super) fn cursor(&self) -> (usize, usize) {
        (self.col, self.row)
    }

    pub(super) fn feed(&mut self, bytes: &str) {
        let mut chars = bytes.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\u{1b}' => self.escape(&mut chars),
                '\r' => self.col = 0,
                '\n' => self.line_feed(),
                ch if ch.is_control() => panic!("test screen: unexpected control {ch:?}"),
                ch => self.print(ch),
            }
        }
    }

    fn print(&mut self, ch: char) {
        let width = char_width(ch);
        assert!(width == 1 || width == 2, "test screen: {ch:?} has width {width}");
        assert!(
            self.col + width <= self.cols,
            "test screen: {ch:?} written past the last column (col {}, width {width}, cols {})",
            self.col,
            self.cols
        );
        let (row, col) = (self.row, self.col);
        for at in [col, col + width - 1] {
            self.break_glyph_at(row, at);
        }
        let style = self.pen;
        if width == 1 {
            self.grid[row][col] = ScreenCell { glyph: Glyph::Narrow(ch), style };
        } else {
            self.grid[row][col] = ScreenCell { glyph: Glyph::WideHead(ch), style };
            self.grid[row][col + 1] = ScreenCell { glyph: Glyph::WideTail, style };
        }
        self.col += width;
    }

    /// A write into one half of a wide glyph blanks the other half.
    fn break_glyph_at(&mut self, row: usize, col: usize) {
        match self.grid[row][col].glyph {
            Glyph::WideHead(_) if col + 1 < self.cols => {
                self.grid[row][col + 1] = ScreenCell::BLANK;
            }
            Glyph::WideTail if col > 0 => self.grid[row][col - 1] = ScreenCell::BLANK,
            _ => {}
        }
    }

    fn escape(&mut self, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
        match chars.next() {
            Some('[') => {
                let mut params = String::new();
                let mut final_byte = None;
                for ch in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        final_byte = Some(ch);
                        break;
                    }
                    params.push(ch);
                }
                self.csi(&params, final_byte.expect("test screen: a CSI that never ends"));
            }
            Some(']') => {
                let mut previous = '\0';
                for ch in chars.by_ref() {
                    if ch == '\u{7}' || (previous == '\u{1b}' && ch == '\\') {
                        break;
                    }
                    previous = ch;
                }
            }
            Some('M') => self.reverse_index(),
            other => panic!("test screen: unexpected escape {other:?}"),
        }
    }

    fn csi(&mut self, params: &str, final_byte: char) {
        // Private modes (`?2026`, `?25`) and DECSCUSR (`0 q`) draw nothing.
        if params.starts_with('?') || params.contains(' ') {
            return;
        }
        let values: Vec<usize> = params.split(';').map(|value| value.parse().unwrap_or(0)).collect();
        let arg = |index: usize, default: usize| {
            values.get(index).copied().filter(|value| *value > 0).unwrap_or(default)
        };
        let last = self.grid.len() - 1;
        match final_byte {
            'H' => {
                self.row = (arg(0, 1) - 1).min(last);
                self.col = (arg(1, 1) - 1).min(self.cols - 1);
            }
            'C' => self.col = (self.col + arg(0, 1)).min(self.cols - 1),
            'G' => self.col = (arg(0, 1) - 1).min(self.cols - 1),
            'K' => self.erase_in_line(values.first().copied().unwrap_or(0)),
            'J' => self.erase_in_display(values.first().copied().unwrap_or(0)),
            'm' => self.sgr(params),
            'r' => {
                if params.is_empty() {
                    self.top_margin = 0;
                    self.bottom_margin = last;
                } else {
                    let top = (arg(0, 1) - 1).min(last);
                    let bottom = (arg(1, last + 1) - 1).min(last);
                    if top < bottom {
                        self.top_margin = top;
                        self.bottom_margin = bottom;
                    }
                }
                self.row = 0;
                self.col = 0;
            }
            other => panic!("test screen: unexpected CSI {params:?} {other:?}"),
        }
    }

    /// What an erase leaves: blank, in the pen's background only.
    fn erased(&self) -> ScreenCell {
        ScreenCell { glyph: Glyph::Blank, style: Style { bg: self.pen.bg, ..Style::new() } }
    }

    fn erase_in_line(&mut self, how: usize) {
        let (from, to) = match how {
            0 => (self.col, self.cols),
            1 => (0, (self.col + 1).min(self.cols)),
            _ => (0, self.cols),
        };
        let row = self.row;
        if from < to {
            self.break_glyph_at(row, from);
            self.break_glyph_at(row, to - 1);
        }
        let blank = self.erased();
        for cell in &mut self.grid[row][from..to] {
            *cell = blank;
        }
    }

    fn erase_in_display(&mut self, how: usize) {
        let blank = self.erased();
        match how {
            0 => {
                self.erase_in_line(0);
                for row in &mut self.grid[self.row + 1..] {
                    row.fill(blank);
                }
            }
            2 => {
                for row in &mut self.grid {
                    row.fill(blank);
                }
            }
            _ => {}
        }
    }

    fn sgr(&mut self, params: &str) {
        let mut values = params.split(';').map(|value| value.parse::<u16>().unwrap_or(0));
        let mut any = false;
        while let Some(code) = values.next() {
            any = true;
            let pen = &mut self.pen;
            match code {
                0 => *pen = Style::new(),
                1 => pen.bold = true,
                2 => pen.dim = true,
                3 => pen.italic = true,
                4 => pen.underline = true,
                9 => pen.strike = true,
                22 => {
                    pen.bold = false;
                    pen.dim = false;
                }
                23 => pen.italic = false,
                24 => pen.underline = false,
                29 => pen.strike = false,
                30..=37 => pen.fg = Some(Color::Base(u8::try_from(code - 30).unwrap_or(0))),
                90..=97 => pen.fg = Some(Color::BrightBase(u8::try_from(code - 90).unwrap_or(0))),
                40..=47 => pen.bg = Some(Color::Base(u8::try_from(code - 40).unwrap_or(0))),
                100..=107 => {
                    pen.bg = Some(Color::BrightBase(u8::try_from(code - 100).unwrap_or(0)));
                }
                39 => pen.fg = None,
                49 => pen.bg = None,
                38 | 48 => {
                    let color = match values.next() {
                        Some(5) => Color::Indexed(u8::try_from(values.next().unwrap_or(0)).unwrap_or(0)),
                        Some(2) => {
                            let mut channel = || u8::try_from(values.next().unwrap_or(0)).unwrap_or(0);
                            Color::Rgb(channel(), channel(), channel())
                        }
                        other => panic!("test screen: extended colour {other:?}"),
                    };
                    if code == 38 {
                        self.pen.fg = Some(color);
                    } else {
                        self.pen.bg = Some(color);
                    }
                }
                other => panic!("test screen: unexpected SGR {other}"),
            }
        }
        if !any {
            self.pen = Style::new();
        }
    }

    fn line_feed(&mut self) {
        if self.row == self.bottom_margin {
            let gone = self.grid.remove(self.top_margin);
            if self.top_margin == 0 {
                self.scrollback.push(gone);
            }
            let blank = self.erased();
            self.grid.insert(self.bottom_margin, vec![blank; self.cols]);
        } else if self.row < self.grid.len() - 1 {
            self.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.row == self.top_margin {
            self.grid.remove(self.bottom_margin);
            let blank = self.erased();
            self.grid.insert(self.top_margin, vec![blank; self.cols]);
        } else if self.row > 0 {
            self.row -= 1;
        }
    }
}

// ----------------------------------------------------------------------
// 무작위 행 — 시험이 같은 재료로 행을 만들고 살짝 바꾼다.
// ----------------------------------------------------------------------

/// 씨앗이 같으면 같은 수열이 나오는 난수 — 시험이 재현되어야 한다.
pub(super) struct Lcg(pub(super) u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    /// `0..bound` 의 한 수.
    pub(super) fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next()).unwrap_or(0) % bound
    }
}

/// 좁은 글자, 넓은 글자, 상자 선, 공백 — 행이 실제로 담는 것들.
const WORDS: [&str; 12] =
    ["abc", "hello", "•", "│", "한글", "日本語", "x", " ", "  ", "→", "the quick", "0123"];

fn style_of(pick: usize) -> Style {
    let rgb = |red: u8| Style::new().fg(Color::Rgb(red, 20, 30)).bold();
    match pick {
        0 | 1 => Style::new(),
        2 => Style::new().dim(),
        3 => Style::new().bold(),
        4 => rgb(10),
        5 => rgb(11),
        6 => Style::new().bg(Color::Rgb(40, 40, 40)),
        7 => Style::new().underline(),
        8 => Style::new().italic().fg(Color::CYAN),
        9 => Style::new().fg(Color::Base(2)).bg(Color::Rgb(40, 40, 40)),
        _ => Style::new().strike().dim(),
    }
}

fn word(random: &mut Lcg) -> Span {
    Span::new(WORDS[random.below(WORDS.len())], style_of(random.below(11)))
}

/// 폭이 `cols` 를 넘지 않는 무작위 한 줄.
pub(super) fn random_line(random: &mut Lcg, cols: usize) -> Line {
    let mut spans: Vec<Span> = Vec::new();
    for _ in 0..random.below(8) {
        spans.push(word(random));
        if Line::new(spans.clone()).width() > cols {
            spans.pop();
            break;
        }
    }
    Line::new(spans)
}

/// 줄의 몇 군데를 바꾼다 — 한 스팬의 스타일이나 글자, 스팬 하나를 지우거나 끼운다.
pub(super) fn mutate(line: &Line, random: &mut Lcg, cols: usize) -> Line {
    let mut spans = line.spans.clone();
    for _ in 0..=random.below(3) {
        match random.below(4) {
            0 if !spans.is_empty() => {
                let at = random.below(spans.len());
                spans[at].style = style_of(random.below(11));
            }
            1 if !spans.is_empty() => {
                let at = random.below(spans.len());
                spans[at].text = WORDS[random.below(WORDS.len())].to_string();
            }
            2 if !spans.is_empty() => {
                spans.remove(random.below(spans.len()));
            }
            _ => {
                let at = random.below(spans.len() + 1);
                spans.insert(at, word(random));
            }
        }
    }
    while Line::new(spans.clone()).width() > cols {
        spans.pop();
    }
    Line::new(spans)
}
