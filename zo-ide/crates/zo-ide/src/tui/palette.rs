//! 터미널 기본색과 출력 깊이에 맞춘 TUI 팔레트.
//!
//! OSC 10/11 응답은 시작할 때 한 번, 100ms의 공통 기한 안에서만 읽는다.
//! 응답이 없거나 입력이 TTY가 아니면 팔레트는 `None`이다. 이 값은 중요한
//! 바이트 계약이다: `None`일 때 [`Color`]는 양자화도 치환도 거치지 않아 기존
//! 캡처와 골든이 한 글자도 달라지지 않는다. 테스트는 실제 터미널 대신
//! [`TerminalPalette`]를 렌더러에 주입해 적응 갈래만 별도로 고정한다.
//!
//! 조회 동안은 아직 crossterm 이벤트 스트림이 없어서 `/dev/tty` 입력을 이
//! 코드가 잠깐 독점한다. 그 사이 섞인 키 입력은 응답과 함께 소비될 수 있으므로
//! 두 조회를 한 번에 보내 한 기한만 치르고, 실패를 캐시해 redraw마다 재시도하지
//! 않는다. 이 제한된 비용을 감수하는 대신 실제 밝은/어두운 배경에 적응한다.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::ansi::{Color, Role, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedOscPalette {
    foreground: (u8, u8, u8),
    background: (u8, u8, u8),
    ansi16: [Option<(u8, u8, u8)>; 16],
}

#[cfg(any(unix, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OscPaletteRead {
    complete: bool,
    palette: Option<ParsedOscPalette>,
}

/// OSC 조회가 기다리는 전체 시간. Codex startup probe와 같은 상한이다.
#[cfg(unix)]
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

/// shimmer 밴드의 바닥색 — 응답이 없을 때의 캡처 값.
pub const SHIMMER_BASE: (u8, u8, u8) = (128, 128, 128);
/// shimmer 밴드의 꼭대기 재료 — 응답이 없을 때의 캡처 값.
pub const SHIMMER_PEAK: (u8, u8, u8) = (255, 255, 255);
/// Minimum WCAG contrast ratio against the terminal background for muted
/// marks elements (table rules and the deepest shimmer trough).
pub const MIN_MUTED_CONTRAST: f64 = 3.0;
/// Minimum WCAG contrast ratio for secondary (dim) text in the marks look.
pub const MIN_SECONDARY_CONTRAST: f64 = 4.5;
/// Maximum share of the body-text contrast ratio that secondary text may
/// require on low-contrast themes (e.g. Solarized), so dim text stays softer
/// than body text.
pub const SECONDARY_CONTRAST_BODY_SHARE: f64 = 0.75;
const SECONDARY_INK_START_SHARE: f32 = 0.60;
const SECONDARY_INK_STEP: f32 = 0.05;

/// Max tier 컴포저 caret `›` 의 캡처 폴백.
pub const COMPOSER_CARET: Color = Color::Role(Role::Caret);
/// 푸터의 `<model> <effort>` 캡처 폴백.
pub const FOOTER_MODEL: Color = Color::Role(Role::Model);
/// 푸터의 cwd 캡처 폴백.
pub const FOOTER_CWD: Color = Color::Role(Role::Cwd);

const DARK_COMPOSER_CARET: Color = Role::Caret.terminal_color();
const DARK_FOOTER_MODEL: Color = Role::Model.terminal_color();
const DARK_FOOTER_CWD: Color = Role::Cwd.terminal_color();
const DARK_TABLE_HEADER: Color = Role::Heading.terminal_color();
const DARK_WARNING_NOTICE: Color = Role::Warning.terminal_color();

// OSC 11이 밝은 배경을 돌려줄 때의 화면용 색. `None` 팔레트에서는 절대로
// 이 값으로 바꾸지 않는다. 그래야 응답하지 않는 터미널의 캡처 바이트가
// 기존과 완전히 같고, 실제 밝은 화면에서만 이 값이 쓰인다.
const LIGHT_COMPOSER_CARET: Color = Color::Rgb(143, 83, 0);
const LIGHT_FOOTER_MODEL: Color = Color::Rgb(106, 78, 17);
const LIGHT_FOOTER_CWD: Color = Color::Rgb(40, 96, 48);
const LIGHT_TABLE_HEADER: Color = Color::Rgb(108, 76, 21);
const LIGHT_WARNING_NOTICE: Color = Color::Rgb(139, 98, 20);

/// codex 가 `.cyan()` 으로 칠하는 명령 토큰.
pub const COMMAND_TOKEN: Color = Color::Indexed(6);
/// The `@` popup's file name and `File` tag — codex `mentions_v2/render.rs`
/// `base_style.fg(Color::Cyan)` and `candidate.rs` `MentionType::File`.
pub const MENTION_FILE: Color = Color::Indexed(6);
/// The `@` popup's `Vault` tag and the active `Skills` mode — the magenta
/// codex gives its non-file rows (`MentionType::Plugin`, `SearchMode::Tools`).
pub const MENTION_PAGE: Color = Color::Indexed(5);
/// 부팅 카드에서 effort 토큰.
pub const CARD_EFFORT: Color = Color::Indexed(5);
/// 경고 셀 `⚠`.
pub const NOTICE_WARN: Color = Color::Indexed(3);
/// The footer badge's `N warnings` — codex `style.rs::warning_notice_style`
/// amber on a dark background (the light one is `LIGHT_WARNING_NOTICE`); in
/// the ZeroCode window the `terminal` theme draws it in the slot of `⚠` and
/// `Full Access` (`NOTICE_WARN`, `Role::terminal_ink_in`).
pub const WARNING_NOTICE: Color = Color::Role(Role::Warning);
/// 끝난 도구 셀의 성공 불릿.
pub const TOOL_OK: Color = Color::Indexed(2);
/// 끝난 도구 셀의 실패 불릿.
pub const TOOL_FAIL: Color = Color::Indexed(1);
/// `Exploring` 목록과 MCP 셀의 도구 이름.
pub const TOOL_LABEL: Color = Color::Indexed(6);
/// Accent for marks headings (H1/H2) and table headers.
pub const HEADING_ACCENT: Color = Color::Role(Role::Heading);
/// 표 헤더 행의 전경색.
pub const TABLE_HEADER: Color = HEADING_ACCENT;
/// `/resume` 목록에서 고른 행 — 감사에서 확인된 어두운 배경 갈래
/// (`Color::Yellow`). 행 배경 tint는 측정값에 적응하지만, 밝은 테마의 전경
/// remap은 #19와 함께 유예되어 이 토큰은 보존한다.
pub const SESSION_SELECTED: Color = Color::Indexed(3);
/// `/resume` 툴바에서 **초점이 간** 컨트롤의 켜진 값 — codex
/// `toolbar_value` 의 `value.magenta()`.
pub const TOOLBAR_FOCUS: Color = Color::Indexed(5);

/// stdout이 실제로 표시할 수 있는 색 깊이.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorLevel {
    TrueColor,
    Ansi256,
    Ansi16,
}

/// 한 번 조회해 한 렌더 패스 전체에 주입하는 터미널 팔레트.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalPalette {
    foreground: (u8, u8, u8),
    background: (u8, u8, u8),
    color_level: ColorLevel,
    ansi16: [Option<(u8, u8, u8)>; 16],
}

impl TerminalPalette {
    /// 순수 렌더 테스트와 startup probe가 공유하는 생성자.
    #[must_use]
    pub const fn new(
        foreground: (u8, u8, u8),
        background: (u8, u8, u8),
        color_level: ColorLevel,
    ) -> Self {
        Self {
            foreground,
            background,
            color_level,
            ansi16: [None; 16],
        }
    }

    /// Attaches measured RGB values for ANSI 16-colour slots `0..=15`.
    #[must_use]
    pub const fn with_ansi16(mut self, ansi16: [Option<(u8, u8, u8)>; 16]) -> Self {
        self.ansi16 = ansi16;
        self
    }

    /// Returns the measured RGB for an ANSI 16-colour slot (`0..=15`), if known.
    #[must_use]
    pub const fn ansi_color(self, index: u8) -> Option<(u8, u8, u8)> {
        if index < 16 {
            self.ansi16[index as usize]
        } else {
            None
        }
    }

    #[must_use]
    pub const fn foreground(self) -> (u8, u8, u8) {
        self.foreground
    }

    #[must_use]
    pub const fn background(self) -> (u8, u8, u8) {
        self.background
    }

    #[must_use]
    pub const fn color_level(self) -> ColorLevel {
        self.color_level
    }

    /// Whether the OSC 11 background is bright enough that the capture's
    /// pastel foregrounds would lose their contrast.
    #[must_use]
    pub fn has_light_background(self) -> bool {
        is_light_background(self.background)
    }

    /// Colours for the moving status band. Codex uses the terminal foreground
    /// as the base and sweeps toward the terminal background at the peak. The
    /// old implementation mixed a dim base and then swept back toward the
    /// foreground, which reversed that polarity.
    #[must_use]
    pub fn shimmer_colors(self) -> ((u8, u8, u8), (u8, u8, u8)) {
        let peak = if super::style::current().style == super::style::TuiStyle::Marks {
            self.muted_foreground_for_contrast(MIN_MUTED_CONTRAST)
        } else {
            self.background
        };
        (self.foreground, peak)
    }

    /// Lightest blend of `foreground` over `background` (starting at 20 % in
    /// 5 % steps) that still clears `min_contrast` against `background`.
    #[must_use]
    pub fn muted_foreground_for_contrast(self, min_contrast: f64) -> (u8, u8, u8) {
        for step in 4..=20 {
            #[allow(clippy::cast_precision_loss)]
            let alpha = step as f32 * 0.05;
            let candidate = blend_rgb(self.foreground, self.background, alpha);
            if contrast_ratio(candidate, self.background) >= min_contrast {
                return candidate;
            }
        }
        self.foreground
    }

    pub(crate) fn text_contrast_floor(self, background: (u8, u8, u8)) -> f64 {
        let body_contrast = contrast_ratio(self.foreground, background);
        (body_contrast * SECONDARY_CONTRAST_BODY_SHARE).min(MIN_SECONDARY_CONTRAST)
    }

    /// Measured dimmed ink for `ink` against `background`: blends `ink` over
    /// `background` starting at 60 % in 5 % steps until it clears the text
    /// contrast floor, then pulls toward `self.foreground` if `ink` itself
    /// falls under the floor.
    #[must_use]
    pub fn dim_foreground(
        self,
        ink: (u8, u8, u8),
        background: (u8, u8, u8),
    ) -> (u8, u8, u8) {
        let floor = self.text_contrast_floor(background);
        for step in 0..8 {
            #[allow(clippy::cast_precision_loss)]
            let alpha = SECONDARY_INK_START_SHARE + step as f32 * SECONDARY_INK_STEP;
            let candidate = blend_rgb(ink, background, alpha);
            if contrast_ratio(candidate, background) >= floor {
                return candidate;
            }
        }
        self.ensure_text_contrast(ink, background)
    }

    /// Measured secondary-text ink against `background`: blends `foreground`
    /// over `background` starting at 60 % in 5 % steps until it clears
    /// `min(4.5, body_contrast * 0.75)`.
    #[must_use]
    pub fn secondary_foreground(self, background: (u8, u8, u8)) -> (u8, u8, u8) {
        self.dim_foreground(self.foreground, background)
    }

    /// Pull `color` toward `self.foreground` in 5 % steps until its contrast
    /// against `background` reaches the text contrast floor, returning `color`
    /// unchanged when it already clears the floor.
    #[must_use]
    pub fn ensure_text_contrast(
        self,
        color: (u8, u8, u8),
        background: (u8, u8, u8),
    ) -> (u8, u8, u8) {
        let floor = self.text_contrast_floor(background);
        if contrast_ratio(color, background) >= floor {
            return color;
        }
        for step in 1..20 {
            #[allow(clippy::cast_precision_loss)]
            let alpha = step as f32 * SECONDARY_INK_STEP;
            let candidate = blend_rgb(self.foreground, color, alpha);
            if contrast_ratio(candidate, background) >= floor {
                return candidate;
            }
        }
        self.foreground
    }

    /// 실제 깊이에서 표현할 수 있는 전경/배경색으로 낮춘다.
    ///
    /// ANSI-16에서 RGB나 확장 인덱스를 기본색으로 억지 근사하면 사용자가 바꾼
    /// 16색 표를 추측하게 된다. Codex처럼 그 경우는 terminal default(`None`)로
    /// 두고, 0~7 인덱스만 동일한 base SGR로 옮긴다.
    #[must_use]
    pub fn resolve(self, color: Color) -> Option<Color> {
        let color = match color {
            Color::Role(role) => role.terminal_ink()?,
            other => other,
        };
        let color = self.adapt_for_background(color);
        self.resolve_depth(color)
    }

    /// Lowers `color` to `self.color_level` without running `terminal`-theme
    /// background adaptation.
    #[must_use]
    pub(crate) fn resolve_depth(self, color: Color) -> Option<Color> {
        let color = match color {
            Color::Role(role) => role.terminal_ink()?,
            other => other,
        };
        match (self.color_level, color) {
            (ColorLevel::Ansi256, Color::Rgb(red, green, blue)) => {
                Some(Color::Indexed(nearest_xterm_index((red, green, blue))))
            }
            (ColorLevel::TrueColor | ColorLevel::Ansi256, color) => Some(color),
            (ColorLevel::Ansi16, Color::BrightBase(index)) => Some(Color::BrightBase(index)),
            (ColorLevel::Ansi16, Color::Base(index) | Color::Indexed(index @ 0..=7)) => {
                Some(Color::Base(index))
            }
            (ColorLevel::Ansi16, Color::Indexed(_) | Color::Rgb(..)) => None,
            (_, Color::Role(_)) => unreachable!("Role was lowered above"),
        }
    }

    fn adapt_for_background(self, color: Color) -> Color {
        if self.has_light_background() {
            light_ground_color(color)
        } else {
            color
        }
    }
}

/// `color` as a light ground draws it: each of the five colours the captures
/// give the `terminal` theme's roles on a dark ground becomes its light-ground
/// pair; any other colour stays. A measured light palette asks it, and so does
/// a ground only known to be light (`ground::Ground::Lightness`).
#[must_use]
pub(crate) fn light_ground_color(color: Color) -> Color {
    match color {
        DARK_COMPOSER_CARET => LIGHT_COMPOSER_CARET,
        DARK_FOOTER_MODEL => LIGHT_FOOTER_MODEL,
        DARK_FOOTER_CWD => LIGHT_FOOTER_CWD,
        DARK_TABLE_HEADER => LIGHT_TABLE_HEADER,
        DARK_WARNING_NOTICE => LIGHT_WARNING_NOTICE,
        _ => color,
    }
}

/// Blend `foreground` over `background` using Codex's truncating channel math.
#[must_use]
pub(crate) fn blend_rgb(
    foreground: (u8, u8, u8),
    background: (u8, u8, u8),
    alpha: f32,
) -> (u8, u8, u8) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mix = |front: u8, back: u8| {
        (f32::from(front) * alpha + f32::from(back) * (1.0 - alpha)) as u8
    };
    (
        mix(foreground.0, background.0),
        mix(foreground.1, background.1),
        mix(foreground.2, background.2),
    )
}

const STATUS_LINE_COLOR_SATURATION_PERCENT: u16 = 85;
const STATUS_LINE_COLOR_BRIGHTNESS_PERCENT: u16 = 100;

/// How far the band stands from the terminal's ground, as the share of white
/// (on a dark ground) or of black (on a light one) laid over it: Geist's
/// `gray-alpha-100` for the marks look, codex's own steps for codex's.
const BAND_SHARE_GEIST_DARK: f32 = 0.06;
const BAND_SHARE_GEIST_LIGHT: f32 = 0.05;
const BAND_SHARE_CODEX_DARK: f32 = 0.12;
const BAND_SHARE_CODEX_LIGHT: f32 = 0.04;

/// The band the person's words and the composer stand on, in a terminal
/// whose colours are known: its background moved a step toward its opposite.
///
/// The marks look takes the step a surface takes over its ground in Geist,
/// the design system the window is drawn in — `gray-alpha-100`: 6 % of white
/// on a dark ground, 5 % of black on a light one. The band then reads as the
/// pane's own colour raised a little. Codex's 12 % made it a block of another
/// grey beside the window's surfaces, and any cell a terminal draws a shade
/// off — dim text whose ground it dims with the ink — stood out of it ("회색
/// 바탕이 이상해 색이 안맞아", 2026-11-11). Codex's look keeps codex's steps.
#[must_use]
pub(crate) fn band_background(terminal_palette: TerminalPalette, style: super::style::TuiStyle) -> Color {
    let marks = style == super::style::TuiStyle::Marks;
    let (overlay, alpha) = match (terminal_palette.has_light_background(), marks) {
        (true, true) => ((0, 0, 0), BAND_SHARE_GEIST_LIGHT),
        (true, false) => ((0, 0, 0), BAND_SHARE_CODEX_LIGHT),
        (false, true) => ((255, 255, 255), BAND_SHARE_GEIST_DARK),
        (false, false) => ((255, 255, 255), BAND_SHARE_CODEX_DARK),
    };
    let (red, green, blue) = blend_rgb(overlay, terminal_palette.background(), alpha);
    Color::Rgb(red, green, blue)
}

/// The band a code block stands on: the terminal's background moved a step
/// toward its opposite — less than the band the person's words stand on.
#[must_use]
pub(crate) fn code_background_rgb(terminal_palette: TerminalPalette) -> (u8, u8, u8) {
    let (overlay, alpha) = if terminal_palette.has_light_background() {
        ((0, 0, 0), 0.03)
    } else {
        ((255, 255, 255), 0.05)
    };
    blend_rgb(overlay, terminal_palette.background(), alpha)
}

/// The band a code block stands on, wrapped as an RGB [`Color`].
#[must_use]
pub(crate) fn code_background(terminal_palette: TerminalPalette) -> Color {
    let (red, green, blue) = code_background_rgb(terminal_palette);
    Color::Rgb(red, green, blue)
}

/// A list row's ground in a terminal whose colours are known: the background
/// moved a little toward its opposite, further for the chosen row. The
/// `/resume` list and the start page's list stand on the same.
#[must_use]
pub(crate) fn row_background(terminal_palette: TerminalPalette, selected: bool) -> Color {
    let (overlay, alpha) = if terminal_palette.has_light_background() {
        ((0, 0, 0), if selected { 0.12 } else { 0.04 })
    } else {
        ((255, 255, 255), if selected { 0.12 } else { 0.055 })
    };
    let (red, green, blue) = blend_rgb(overlay, terminal_palette.background(), alpha);
    Color::Rgb(red, green, blue)
}

/// Reduce theme-colour saturation the same way Codex softens footer accents.
#[must_use]
pub(crate) fn soften_status_line_style(mut style: Style) -> Style {
    if let Some(Color::Rgb(red, green, blue)) = style.fg {
        let luma = (77 * u16::from(red) + 150 * u16::from(green) + 29 * u16::from(blue)) / 256;
        let soften = |channel: u8| {
            let channel = u16::from(channel);
            let softened = (channel * STATUS_LINE_COLOR_SATURATION_PERCENT
                + luma * (100 - STATUS_LINE_COLOR_SATURATION_PERCENT)
                + 50)
                / 100;
            u8::try_from(
                (softened * STATUS_LINE_COLOR_BRIGHTNESS_PERCENT + 50) / 100,
            )
            .expect("softened RGB channel fits in u8")
        };
        style.fg = Some(Color::Rgb(soften(red), soften(green), soften(blue)));
    }
    style
}

/// Whether a ground is bright enough that the capture's pastel foregrounds
/// would lose their contrast on it — the one place this rule is written
/// (`TerminalPalette::has_light_background` and the theme's lightness fit
/// both ask it).
#[must_use]
pub(crate) fn is_light_background((red, green, blue): (u8, u8, u8)) -> bool {
    u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114 > 128_000
}

/// WCAG relative-luminance contrast ratio for a measured RGB foreground and
/// background. Keeping this here makes the light-palette pins prove their
/// readability instead of merely asserting a hand-picked RGB triplet.
#[must_use]
pub fn contrast_ratio(foreground: (u8, u8, u8), background: (u8, u8, u8)) -> f64 {
    fn luminance((red, green, blue): (u8, u8, u8)) -> f64 {
        fn linear(component: u8) -> f64 {
            let component = f64::from(component) / 255.0;
            if component <= 0.040_45 {
                component / 12.92
            } else {
                ((component + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
    }

    let foreground = luminance(foreground);
    let background = luminance(background);
    (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
}

static TERMINAL_PALETTE: OnceLock<Option<TerminalPalette>> = OnceLock::new();
static DEFAULT_SECONDARY_INK: OnceLock<(TerminalPalette, (u8, u8, u8))> = OnceLock::new();

/// startup probe가 성공했고 그 팔레트로 그릴 때만 적응 팔레트를 돌려준다 —
/// 사람이 말한 바탕의 밝기가 잰 바탕과 어긋나면 잰 팔레트는 쓰지 않는다
/// ([`super::ground`]).
#[must_use]
pub(crate) fn terminal_palette() -> Option<TerminalPalette> {
    super::ground::current().palette()
}

/// The palette the startup probe measured, whatever the person said of the
/// ground — what [`super::ground::current`] weighs the person's word against.
#[must_use]
pub(crate) fn measured_palette() -> Option<TerminalPalette> {
    TERMINAL_PALETTE.get().copied().flatten()
}

/// Returns the cached secondary-text RGB for `palette`'s default background
/// when `palette` matches the probed terminal palette.
#[must_use]
pub(crate) fn secondary_foreground_cached(palette: TerminalPalette) -> (u8, u8, u8) {
    if let Some(active) = terminal_palette() {
        if active == palette {
            return DEFAULT_SECONDARY_INK
                .get_or_init(|| (active, active.secondary_foreground(active.background())))
                .1;
        }
    }
    palette.secondary_foreground(palette.background())
}

struct ContrastCacheEntry {
    revision: u64,
    palette: TerminalPalette,
    color: (u8, u8, u8),
    background: (u8, u8, u8),
    adjusted: (u8, u8, u8),
}

#[cfg(test)]
thread_local! {
    static CONTRAST_COMPUTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn contrast_computes() -> usize {
    CONTRAST_COMPUTES.with(std::cell::Cell::get)
}

thread_local! {
    static ADJUSTED_TEXT_COLORS: std::cell::RefCell<Vec<ContrastCacheEntry>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static ANSI16_INKS: std::cell::RefCell<Vec<Ansi16InkCacheEntry>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// How many entries this thread's contrast cache and ANSI-16 ink cache hold.
#[cfg(test)]
pub(crate) fn cached_colour_entries() -> (usize, usize) {
    (
        ADJUSTED_TEXT_COLORS.with(|cache| cache.borrow().len()),
        ANSI16_INKS.with(|cache| cache.borrow().len()),
    )
}

/// Cached contrast adjustment keyed by `(revision, palette, color, background)`.
/// In `terminal` theme it blends toward `palette.foreground`; in a named theme
/// it shifts HSL lightness while keeping hue and saturation. A miss drops the
/// entries an earlier theme revision left, so switching themes keeps one
/// revision's worth.
#[must_use]
pub(crate) fn ensure_text_contrast_cached(
    palette: TerminalPalette,
    color: (u8, u8, u8),
    background: (u8, u8, u8),
) -> (u8, u8, u8) {
    let revision = super::theme::revision();
    let named_theme = !super::theme::is_terminal();
    ADJUSTED_TEXT_COLORS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(entry) = cache.iter().find(|entry| {
            entry.revision == revision
                && entry.palette == palette
                && entry.color == color
                && entry.background == background
        }) {
            return entry.adjusted;
        }
        #[cfg(test)]
        CONTRAST_COMPUTES.with(|count| count.set(count.get() + 1));
        let adjusted = if named_theme {
            super::theme::fit_lightness(color, palette, background)
        } else {
            palette.ensure_text_contrast(color, background)
        };
        cache.retain(|entry| entry.revision == revision);
        cache.push(ContrastCacheEntry {
            revision,
            palette,
            color,
            background,
            adjusted,
        });
        adjusted
    })
}

/// Adjusts an RGB foreground `color` to meet the marks text contrast floor
/// against `background_of(palette)` when running in marks style on a
/// non-ANSI-16 palette; leaves all other cases untouched.
#[must_use]
pub(crate) fn ensure_marks_fg_contrast(
    palette: Option<TerminalPalette>,
    color: Color,
    background_of: impl FnOnce(TerminalPalette) -> (u8, u8, u8),
) -> Color {
    if super::style::current().style != super::style::TuiStyle::Marks {
        return color;
    }
    let (Some(palette), Color::Rgb(red, green, blue)) = (palette, color) else {
        return color;
    };
    if palette.color_level() == ColorLevel::Ansi16 {
        return color;
    }
    let (red, green, blue) =
        ensure_text_contrast_cached(palette, (red, green, blue), background_of(palette));
    Color::Rgb(red, green, blue)
}

struct Ansi16InkCacheEntry {
    revision: u64,
    palette: TerminalPalette,
    slot: u8,
    dim: bool,
    resolved: Option<(u8, u8, u8)>,
}

/// Whether this process draws in the ZeroCode window. Its renderer lifts an
/// indexed colour that falls under its contrast bar and lifts it again when
/// the person changes the terminal's theme (`ui/shell-term.js`, Orca's
/// `minimumContrastRatio`), so there a slot zo would lift stays a slot:
/// lifted to RGB it would keep the palette zo measured at start, and a change
/// of the window's theme would leave it behind. The `terminal` theme's fixed
/// roles follow the window there too, as its own ink or a slot
/// ([`super::ansi::Role::terminal_ink_in`]). Read once; a unit-test build
/// answers `false` wherever it runs.
#[cfg(not(test))]
pub(crate) fn window_lifts_slot_colours() -> bool {
    /// The value the window gives `TERM_PROGRAM` in every pane
    /// (`TERMINAL_NAME`, `crates/zerocode-pty/src/grid.rs`).
    const WINDOW_TERM_PROGRAM: &str = "ZeroCode";
    static IN_WINDOW: OnceLock<bool> = OnceLock::new();
    *IN_WINDOW.get_or_init(|| {
        std::env::var("TERM_PROGRAM").is_ok_and(|name| name == WINDOW_TERM_PROGRAM)
    })
}

#[cfg(test)]
pub(crate) const fn window_lifts_slot_colours() -> bool {
    false
}

/// An ANSI-16 slot's ink on `background`: `None` keeps the slot's own index
/// bytes — when it clears the text floor, or when `window_lifts` (the terminal
/// lifts it, and keeps lifting it through a change of theme) — else the
/// measured slot lifted toward the body ink. A dim slot is always the
/// measured dim ink.
fn compute_ansi16_ink(
    palette: TerminalPalette,
    slot: u8,
    dim: bool,
    background: (u8, u8, u8),
    window_lifts: bool,
) -> Option<(u8, u8, u8)> {
    let ink = palette.ansi_color(slot)?;
    if dim {
        return Some(palette.dim_foreground(ink, background));
    }
    let floor = palette.text_contrast_floor(background);
    if window_lifts || contrast_ratio(ink, background) >= floor {
        None
    } else {
        Some(palette.ensure_text_contrast(ink, background))
    }
}

/// Resolves an ANSI-16 slot's foreground RGB when its colour is known and
/// either `dim` is set or the slot falls below the text contrast floor —
/// outside the ZeroCode window, which lifts such a slot itself.
/// Returns `None` when the slot should keep its original ANSI index bytes.
/// Caches `(palette, slot, dim)` for the terminal's default background, for
/// one theme revision at a time.
#[must_use]
pub(crate) fn resolved_ansi16_ink(
    palette: TerminalPalette,
    slot: u8,
    dim: bool,
    explicit_background: Option<(u8, u8, u8)>,
) -> Option<(u8, u8, u8)> {
    if let Some(background) = explicit_background {
        return compute_ansi16_ink(palette, slot, dim, background, window_lifts_slot_colours());
    }
    let revision = super::theme::revision();
    ANSI16_INKS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(entry) = cache.iter().find(|entry| {
            entry.revision == revision
                && entry.palette == palette
                && entry.slot == slot
                && entry.dim == dim
        }) {
            return entry.resolved;
        }
        let resolved =
            compute_ansi16_ink(palette, slot, dim, palette.background(), window_lifts_slot_colours());
        cache.retain(|entry| entry.revision == revision);
        cache.push(Ansi16InkCacheEntry {
            revision,
            palette,
            slot,
            dim,
            resolved,
        });
        resolved
    })
}

/// raw TTY의 이벤트 스트림이 열리기 전에 팔레트를 한 번 초기화한다.
///
/// `render::no_color_env`가 `App::new` 안에서 호출되는 것이 현재 허용된 startup
/// 이음매다. cooked/plain 호출에서는 아무것도 캐시하지 않아 파이프와 기존
/// append-only 출력이 OSC 바이트나 지연을 전혀 보지 않는다.
/// Whether a background query went out and its reply was **not** collected.
///
/// The bytes are then in the terminal's input queue, where the event stream
/// reads them as keystrokes. Only the event loop can undo that, so it has to
/// be told.
static PROBE_SENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PROBE_ANSWERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `true` when a query was sent and never answered in time.
#[must_use]
fn probe_reply_may_be_late() -> bool {
    use std::sync::atomic::Ordering;
    PROBE_SENT.load(Ordering::Acquire) && !PROBE_ANSWERED.load(Ordering::Acquire)
}

/// Swallows a palette reply that arrived after its probe gave up.
///
/// The probe closes its `/dev/tty` handle at the deadline, so a later reply
/// stays in the terminal's input queue and the event stream reads it as
/// keystrokes. Measured with a 150ms reply: 46 characters of
/// `]10;rgb:eeee/eeee/ecec\]11;rgb:ffff/ffff/ffff\` landed in the composer,
/// one Enter away from being sent as a prompt. Waiting longer cannot fix this —
/// a reply can always be later than any budget — so the answer is to recognize
/// it on the way in.
///
/// Armed only when a probe actually went unanswered, and only briefly: after
/// the window it disarms and every key is the operator's again.
#[derive(Debug)]
pub(super) struct LateOscGuard {
    pub(super) until: Instant,
    pub(super) saw_escape: bool,
    pub(super) inside: bool,
}

impl LateOscGuard {
    /// How long a reply is still plausibly in flight. Generous next to the
    /// 100ms probe budget and short enough that a real `Esc` `]` typed later
    /// is never touched.
    const WINDOW: Duration = Duration::from_secs(3);

    pub(super) fn armed_if_needed() -> Option<Self> {
        probe_reply_may_be_late().then(|| Self {
            until: Instant::now() + Self::WINDOW,
            saw_escape: false,
            inside: false,
        })
    }

    /// Whether this key belongs to a late reply and should be dropped.
    pub(super) fn swallows(&mut self, key: &KeyEvent, now: Instant) -> bool {
        if now >= self.until {
            return false;
        }
        if self.inside {
            // `ESC \` (parsed as `Alt+\` or `Esc` then `\`) or `BEL` (parsed
            // as `Ctrl+g` by crossterm) closes this reply. Keep the guard
            // armed until `self.until` because `query_default_colors` sends
            // two queries (OSC 10 and 11) and multiple replies can follow.
            let is_st = matches!(key.code, KeyCode::Char('\\'));
            let is_bel = matches!(key.code, KeyCode::Char('\u{7}'))
                || (matches!(key.code, KeyCode::Char('g'))
                    && key.modifiers.contains(KeyModifiers::CONTROL));
            if is_st || is_bel {
                self.inside = false;
            }
            return true;
        }
        if self.saw_escape {
            self.saw_escape = false;
            if matches!(key.code, KeyCode::Char(']')) {
                self.inside = true;
                return true;
            }
            // Not a reply after all. The Esc that opened this was already
            // swallowed; disarm so the next one reaches the operator.
            self.until = now;
            return false;
        }
        // Crossterm parses `ESC ]` arriving in one read as `Alt+]`.
        if matches!(key.code, KeyCode::Char(']')) && key.modifiers.contains(KeyModifiers::ALT) {
            self.inside = true;
            return true;
        }
        if matches!(key.code, KeyCode::Esc) {
            // The reply begins with ESC, and letting that through fires an
            // interrupt the operator never asked for. Inside the window the Esc
            // is swallowed too: losing one Esc in the three seconds after boot
            // is a smaller harm than a spurious cancel plus 46 characters of
            // escape codes in the prompt.
            self.saw_escape = true;
            return true;
        }
        false
    }
}

pub(crate) fn initialize_if_raw_tty() {
    use std::io::IsTerminal as _;

    if !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
        || !crossterm::terminal::is_raw_mode_enabled().unwrap_or(false)
    {
        return;
    }
    let _ = TERMINAL_PALETTE.get_or_init(query_terminal_palette);
}

fn query_terminal_palette() -> Option<TerminalPalette> {
    let parsed = query_default_colors()?;
    Some(
        TerminalPalette::new(
            parsed.foreground,
            parsed.background,
            color_level_from_env(),
        )
        .with_ansi16(parsed.ansi16),
    )
}

/// The colour depth this process's environment names.
///
/// A terminal that answered no colour question has no palette, and then every
/// row the painter writes asks for this depth. The environment does not change
/// under a running zo, so it is read once.
pub(crate) fn color_level_from_env() -> ColorLevel {
    static LEVEL: OnceLock<ColorLevel> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        color_level_for(
            std::env::var_os("WT_SESSION").is_some(),
            std::env::var_os("FORCE_COLOR").is_some(),
            &std::env::var("COLORTERM").unwrap_or_default(),
            std::env::var("TERM").ok().as_deref(),
        )
    })
}

/// The colour depth these environment values name: Windows Terminal draws
/// truecolour unless `FORCE_COLOR` is set, then `COLORTERM` (`truecolor`,
/// `24bit`), then a `TERM` with `256color`; anything else is ANSI-16.
fn color_level_for(
    wt_session: bool,
    force_color: bool,
    colorterm: &str,
    term: Option<&str>,
) -> ColorLevel {
    if wt_session && !force_color {
        return ColorLevel::TrueColor;
    }
    let colorterm = colorterm.to_ascii_lowercase();
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return ColorLevel::TrueColor;
    }
    if term.is_some_and(|term| term.to_ascii_lowercase().contains("256color")) {
        return ColorLevel::Ansi256;
    }
    ColorLevel::Ansi16
}

#[cfg(unix)]
const CODEX_OSC_QUERY: &[u8] = b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\";
#[cfg(unix)]
const MARKS_OSC_QUERY: &[u8] = concat!(
    "\x1b]10;?\x1b\\\x1b]11;?\x1b\\",
    "\x1b]4;0;?\x1b\\\x1b]4;1;?\x1b\\\x1b]4;2;?\x1b\\\x1b]4;3;?\x1b\\",
    "\x1b]4;4;?\x1b\\\x1b]4;5;?\x1b\\\x1b]4;6;?\x1b\\\x1b]4;7;?\x1b\\",
    "\x1b]4;8;?\x1b\\\x1b]4;9;?\x1b\\\x1b]4;10;?\x1b\\\x1b]4;11;?\x1b\\",
    "\x1b]4;12;?\x1b\\\x1b]4;13;?\x1b\\\x1b]4;14;?\x1b\\\x1b]4;15;?\x1b\\",
    "\x1b]10;?\x1b\\",
)
.as_bytes();

#[cfg(unix)]
fn query_default_colors() -> Option<ParsedOscPalette> {
    use std::fs::OpenOptions;
    use std::io::{ErrorKind, Read as _, Write as _};
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::time::Instant;

    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open("/dev/tty")
        .ok()?;
    let query_ansi16 = super::style::current().style == super::style::TuiStyle::Marks;
    let query = if query_ansi16 {
        MARKS_OSC_QUERY
    } else {
        CODEX_OSC_QUERY
    };
    tty.write_all(query).ok()?;
    tty.flush().ok()?;

    // The reply may still be in flight when the budget runs out — a tmux relay,
    // an SSH hop, or a terminal still booting can all miss 100ms. Record that
    // so the event loop can swallow the answer instead of typing it into the
    // composer: measured, a reply that arrived at 150ms became 46 characters of
    // `]10;rgb:…` sitting in the prompt, one Enter away from being sent.
    PROBE_SENT.store(true, std::sync::atomic::Ordering::Release);
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut response = Vec::with_capacity(if query_ansi16 { 512 } else { 128 });
    let mut chunk = [0u8; 256];
    while Instant::now() < deadline {
        match tty.read(&mut chunk) {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(1)),
            Ok(read) => {
                response.extend_from_slice(&chunk[..read]);
                let status = parse_osc_palette_response(&response, query_ansi16);
                if status.complete {
                    PROBE_ANSWERED.store(true, std::sync::atomic::Ordering::Release);
                    return status.palette;
                }
                if response.len() > 4_096 {
                    response.drain(..response.len() - 4_096);
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(_) => return None,
        }
    }
    parse_osc_palette_response(&response, query_ansi16).palette
}

#[cfg(not(unix))]
const fn query_default_colors() -> Option<ParsedOscPalette> {
    None
}

#[cfg(any(unix, test))]
fn parse_osc_palette_response(
    response: &[u8],
    expect_ansi16_sentinel: bool,
) -> OscPaletteRead {
    let mut foreground = None;
    let mut background = None;
    let mut ansi16 = [None; 16];
    let mut osc10_count = 0u8;
    let mut offset = 0usize;

    while let Some(rel_start) = response[offset..]
        .windows(2)
        .position(|window| window == b"\x1b]")
    {
        let body_start = offset + rel_start + 2;
        let payload = &response[body_start..];
        let string_end = payload.windows(2).position(|window| window == b"\x1b\\");
        let bell = payload.iter().position(|byte| *byte == 0x07);
        let (end, term_len) = match (string_end, bell) {
            (Some(st), Some(b)) if st <= b => (st, 2),
            (Some(st), None) => (st, 2),
            (Some(_) | None, Some(b)) => (b, 1),
            (None, None) => break,
        };
        if let Ok(text) = std::str::from_utf8(&payload[..end]) {
            if let Some((code, rest)) = text.split_once(';') {
                if code == "10" {
                    if let Some(rgb) = parse_osc_rgb(rest) {
                        osc10_count = osc10_count.saturating_add(1);
                        if foreground.is_none() {
                            foreground = Some(rgb);
                        }
                    }
                } else if code == "11" {
                    if let Some(rgb) = parse_osc_rgb(rest) {
                        if background.is_none() {
                            background = Some(rgb);
                        }
                    }
                } else if code == "4" {
                    if let Some((slot_str, rgb_str)) = rest.split_once(';') {
                        if let (Ok(slot), Some(rgb)) =
                            (slot_str.parse::<usize>(), parse_osc_rgb(rgb_str))
                        {
                            if slot < 16 {
                                ansi16[slot] = Some(rgb);
                            }
                        }
                    }
                }
            }
        }
        offset = body_start + end + term_len;
    }

    let palette = match (foreground, background) {
        (Some(foreground), Some(background)) => Some(ParsedOscPalette {
            foreground,
            background,
            ansi16,
        }),
        _ => None,
    };
    let required_osc10 = if expect_ansi16_sentinel { 2 } else { 1 };
    OscPaletteRead {
        complete: palette.is_some() && osc10_count >= required_osc10,
        palette,
    }
}

#[cfg(test)]
fn parse_osc_color(response: &[u8], slot: u8) -> Option<(u8, u8, u8)> {
    let prefix = format!("\x1b]{slot};");
    let start = response
        .windows(prefix.len())
        .position(|window| window == prefix.as_bytes())?;
    let payload = &response[start + prefix.len()..];
    let string_end = payload.windows(2).position(|window| window == b"\x1b\\");
    let bell = payload.iter().position(|byte| *byte == 0x07);
    let end = match (string_end, bell) {
        (Some(string_end), Some(bell)) => string_end.min(bell),
        (Some(end), None) | (None, Some(end)) => end,
        (None, None) => return None,
    };
    parse_osc_rgb(std::str::from_utf8(&payload[..end]).ok()?)
}

#[cfg(any(unix, test))]
fn parse_osc_rgb(payload: &str) -> Option<(u8, u8, u8)> {
    let (kind, components) = payload.trim().split_once(':')?;
    if !kind.eq_ignore_ascii_case("rgb") && !kind.eq_ignore_ascii_case("rgba") {
        return None;
    }
    let mut components = components.split('/');
    let red = parse_component(components.next()?)?;
    let green = parse_component(components.next()?)?;
    let blue = parse_component(components.next()?)?;
    if kind.eq_ignore_ascii_case("rgba") {
        parse_component(components.next()?)?;
    }
    components.next().is_none().then_some((red, green, blue))
}

#[cfg(any(unix, test))]
fn parse_component(component: &str) -> Option<u8> {
    match component.len() {
        2 => u8::from_str_radix(component, 16).ok(),
        4 => u16::from_str_radix(component, 16)
            .ok()
            .map(|value| (value / 257) as u8),
        _ => None,
    }
}

pub(crate) fn nearest_xterm_index(target: (u8, u8, u8)) -> u8 {
    (16u8..=255)
        .min_by_key(|index| perceptual_distance(target, xterm_rgb(*index)))
        .unwrap_or(16)
}

fn xterm_rgb(index: u8) -> (u8, u8, u8) {
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match index {
        16..=231 => {
            let offset = index - 16;
            (
                CUBE[usize::from(offset / 36)],
                CUBE[usize::from((offset % 36) / 6)],
                CUBE[usize::from(offset % 6)],
            )
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
        _ => (0, 0, 0),
    }
}

fn perceptual_distance(left: (u8, u8, u8), right: (u8, u8, u8)) -> u32 {
    let red = i32::from(left.0) - i32::from(right.0);
    let green = i32::from(left.1) - i32::from(right.1);
    let blue = i32::from(left.2) - i32::from(right.2);
    (red * red * 30 + green * green * 59 + blue * blue * 11).cast_unsigned()
}

/// 히스토리 셀의 마커 스타일 — 답변·reasoning 의 `• `.
#[must_use]
pub fn cell_marker() -> Style {
    Style::new().dim()
}

/// 유저 셀의 `› ` — 캡처는 bold+dim 이다.
#[must_use]
pub fn user_marker() -> Style {
    Style::new().bold().dim()
}

#[cfg(test)]
mod tests {
    use super::{Color, ColorLevel, TerminalPalette, parse_osc_color, soften_status_line_style};
    use crate::tui::ansi::Style;

    /// The band under the person's words takes Geist's surface step in the
    /// marks look and codex's in codex's, on a dark ground and on a light one.
    #[test]
    fn the_band_takes_geists_step_in_the_marks_look_and_codexs_in_codexs() {
        use crate::tui::style::TuiStyle;
        let on = |background| TerminalPalette::new((128, 128, 128), background, ColorLevel::TrueColor);
        let (black, white) = (on((0, 0, 0)), on((255, 255, 255)));
        assert_eq!(super::band_background(black, TuiStyle::Marks), Color::Rgb(15, 15, 15), "6 % of white");
        assert_eq!(super::band_background(black, TuiStyle::Codex), Color::Rgb(30, 30, 30), "12 % of white");
        assert_eq!(super::band_background(white, TuiStyle::Marks), Color::Rgb(242, 242, 242), "5 % of black");
        assert_eq!(super::band_background(white, TuiStyle::Codex), Color::Rgb(244, 244, 244), "4 % of black");
        // The pane the complaint was about: the band stands between its
        // ground and the block codex's step made of it.
        let pane = on((29, 30, 37));
        let (Color::Rgb(red, green, blue), Color::Rgb(was_red, was_green, was_blue)) =
            (super::band_background(pane, TuiStyle::Marks), super::band_background(pane, TuiStyle::Codex))
        else {
            panic!("a band is an RGB colour");
        };
        assert_eq!((was_red, was_green, was_blue), (56, 57, 63), "the block the window drew");
        assert!(29 < red && red < was_red && 30 < green && green < was_green && 37 < blue && blue < was_blue);
    }

    #[test]
    fn parses_eight_and_sixteen_bit_osc_components() {
        let response = b"\x1b]10;rgb:12/34/56\x07\x1b]11;rgb:ffff/8080/0000\x1b\\";
        assert_eq!(parse_osc_color(response, 10), Some((0x12, 0x34, 0x56)));
        assert_eq!(parse_osc_color(response, 11), Some((0xff, 0x80, 0x00)));
    }

    #[test]
    fn ansi16_keeps_only_terminal_defined_base_colours() {
        let palette =
            TerminalPalette::new((255, 255, 255), (0, 0, 0), ColorLevel::Ansi16);
        assert_eq!(palette.resolve(Color::Indexed(6)), Some(Color::Base(6)));
        assert_eq!(palette.resolve(Color::Rgb(1, 2, 3)), None);
    }

    #[test]
    fn status_line_softening_reproduces_the_audited_mocha_values() {
        assert_eq!(
            soften_status_line_style(Style::new().fg(Color::Rgb(249, 226, 175))).fg,
            Some(Color::Rgb(246, 226, 183))
        );
        assert_eq!(
            soften_status_line_style(Style::new().fg(Color::Rgb(166, 227, 161))).fg,
            Some(Color::Rgb(171, 223, 167))
        );
    }

    #[test]
    fn secondary_foreground_keeps_a_contrast_floor_across_themes() {
        let horizon = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
        let horizon_ink = horizon.secondary_foreground(horizon.background());
        assert_eq!(horizon_ink, super::blend_rgb((224, 224, 224), (28, 30, 38), 0.60));
        assert!(super::contrast_ratio(horizon_ink, (28, 30, 38)) >= 4.5);

        let light = TerminalPalette::new((46, 52, 52), (255, 255, 255), ColorLevel::TrueColor);
        let light_ink = light.secondary_foreground(light.background());
        assert!(super::contrast_ratio(light_ink, (255, 255, 255)) >= 4.5);
        assert!(
            super::contrast_ratio(light_ink, (255, 255, 255))
                < super::contrast_ratio((46, 52, 52), (255, 255, 255))
        );

        let solarized_light =
            TerminalPalette::new((101, 123, 131), (253, 246, 227), ColorLevel::TrueColor);
        let solarized_light_body = super::contrast_ratio((101, 123, 131), (253, 246, 227));
        let solarized_light_ink = solarized_light.secondary_foreground(solarized_light.background());
        let solarized_light_contrast =
            super::contrast_ratio(solarized_light_ink, (253, 246, 227));
        assert!(solarized_light_contrast >= solarized_light_body * 0.75);
        assert!(solarized_light_contrast < solarized_light_body);

        let solarized_dark =
            TerminalPalette::new((131, 148, 150), (0, 43, 54), ColorLevel::TrueColor);
        let solarized_dark_body = super::contrast_ratio((131, 148, 150), (0, 43, 54));
        let solarized_dark_ink = solarized_dark.secondary_foreground(solarized_dark.background());
        let solarized_dark_contrast = super::contrast_ratio(solarized_dark_ink, (0, 43, 54));
        assert!(solarized_dark_contrast >= solarized_dark_body * 0.75);
        assert!(solarized_dark_contrast < solarized_dark_body);

        let band = (42, 43, 50);
        let band_ink = horizon.secondary_foreground(band);
        assert!(super::contrast_ratio(band_ink, band) >= 4.5);
    }

    #[test]
    fn ensure_text_contrast_pulls_low_contrast_colours_and_keeps_compliant_ones() {
        let light = TerminalPalette::new((46, 52, 52), (255, 255, 255), ColorLevel::TrueColor);
        let model = light.ensure_text_contrast((213, 144, 48), (255, 255, 255));
        assert_ne!(model, (213, 144, 48));
        assert!(super::contrast_ratio(model, (255, 255, 255)) >= 4.5);

        let cyan = light.ensure_text_contrast((23, 146, 153), (247, 247, 247));
        assert_ne!(cyan, (23, 146, 153));
        assert!(super::contrast_ratio(cyan, (247, 247, 247)) >= 4.5);

        let dark = TerminalPalette::new((224, 224, 224), (28, 30, 38), ColorLevel::TrueColor);
        assert_eq!(dark.ensure_text_contrast((147, 153, 178), (39, 41, 48)), (147, 153, 178));
        assert_eq!(dark.ensure_text_contrast((246, 226, 183), (28, 30, 38)), (246, 226, 183));

        let solarized = TerminalPalette::new((101, 123, 131), (253, 246, 227), ColorLevel::TrueColor);
        let solarized_bg = (253, 246, 227);
        let body_contrast = super::contrast_ratio((101, 123, 131), solarized_bg);
        let adjusted = solarized.ensure_text_contrast((213, 144, 48), solarized_bg);
        assert!(super::contrast_ratio(adjusted, solarized_bg) >= body_contrast * 0.75);
    }

    #[test]
    fn parses_osc_palette_with_sixteen_ansi_slots_and_end_sentinel() {
        let mut response = b"\x1b]10;rgb:6565/7b7b/8383\x1b\\\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\".to_vec();
        for slot in 0u8..16 {
            response.extend_from_slice(
                format!("\x1b]4;{slot};rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}\x1b\\", slot * 10, slot * 10 + 1, slot * 10 + 2).as_bytes(),
            );
        }
        response.extend_from_slice(b"\x1b]10;rgb:6565/7b7b/8383\x1b\\");

        let read = super::parse_osc_palette_response(&response, true);
        assert!(read.complete, "complete 19-reply response should finish");
        let parsed = read.palette.expect("palette should be present");
        assert_eq!(parsed.foreground, (0x65, 0x7b, 0x83));
        assert_eq!(parsed.background, (0xfd, 0xf6, 0xe3));
        for slot in 0u8..16 {
            assert_eq!(
                parsed.ansi16[usize::from(slot)],
                Some((slot * 10, slot * 10 + 1, slot * 10 + 2))
            );
        }
    }

    #[test]
    fn completes_at_end_sentinel_when_terminal_ignores_osc_4() {
        let response = b"\x1b]10;rgb:e0e0/e0e0/e0e0\x1b\\\x1b]11;rgb:1c1c/1e1e/2626\x1b\\\x1b]10;rgb:e0e0/e0e0/e0e0\x1b\\";
        let read = super::parse_osc_palette_response(response, true);
        assert!(read.complete, "end sentinel should finish even without OSC 4 replies");
        let parsed = read.palette.expect("palette should be present");
        assert_eq!(parsed.foreground, (0xe0, 0xe0, 0xe0));
        assert_eq!(parsed.background, (0x1c, 0x1e, 0x26));
        assert_eq!(parsed.ansi16, [None; 16]);
    }

    #[test]
    fn waits_for_end_sentinel_while_osc_4_replies_are_still_arriving() {
        let mut response = b"\x1b]10;rgb:e0e0/e0e0/e0e0\x1b\\\x1b]11;rgb:1c1c/1e1e/2626\x1b\\".to_vec();
        for slot in 0..5 {
            response.extend_from_slice(format!("\x1b]4;{slot};rgb:1111/2222/3333\x1b\\").as_bytes());
        }
        let read = super::parse_osc_palette_response(&response, true);
        assert!(!read.complete, "without end sentinel, probe is not complete");
        let parsed = read
            .palette
            .expect("partial response with OSC 10 and 11 should still build a palette");
        assert_eq!(parsed.foreground, (0xe0, 0xe0, 0xe0));
        assert_eq!(parsed.background, (0x1c, 0x1e, 0x26));
        for slot in 0..5 {
            assert_eq!(parsed.ansi16[slot], Some((0x11, 0x22, 0x33)));
        }
        for slot in 5..16 {
            assert_eq!(parsed.ansi16[slot], None);
        }
    }

    #[test]
    fn returns_no_palette_when_only_osc_4_replies_arrived() {
        let response = b"\x1b]4;0;rgb:1111/2222/3333\x1b\\\x1b]4;1;rgb:4444/5555/6666\x1b\\\x1b]4;2;rgb:7777/8888/9999\x1b\\";
        let read = super::parse_osc_palette_response(response, true);
        assert!(!read.complete);
        assert_eq!(read.palette, None);
    }

    #[test]
    fn parses_osc_4_replies_with_bel_and_8_or_16_bit_components() {
        let response = b"\x1b]10;rgb:65/7b/83\x07\x1b]11;rgb:fdfd/f6f6/e3e3\x07\x1b]4;1;rgb:dc/32/2f\x07\x1b]4;6;rgb:2a2a/a1a1/9898\x07\x1b]10;rgb:65/7b/83\x07";
        let read = super::parse_osc_palette_response(response, true);
        assert!(read.complete, "BEL-terminated replies should finish at second OSC 10");
        let parsed = read.palette.expect("palette should be present");
        assert_eq!(parsed.foreground, (0x65, 0x7b, 0x83));
        assert_eq!(parsed.background, (0xfd, 0xf6, 0xe3));
        assert_eq!(parsed.ansi16[1], Some((0xdc, 0x32, 0x2f)));
        assert_eq!(parsed.ansi16[6], Some((0x2a, 0xa1, 0x98)));
        assert_eq!(parsed.ansi16[0], None);
    }

    /// The depth the environment names, from the values handed in — the
    /// process reads its own environment once and asks the same rule.
    #[test]
    fn the_colour_depth_follows_the_values_the_environment_names() {
        use super::color_level_for;
        assert_eq!(color_level_for(true, false, "", None), ColorLevel::TrueColor, "Windows Terminal");
        assert_eq!(
            color_level_for(true, true, "", Some("xterm-256color")),
            ColorLevel::Ansi256,
            "FORCE_COLOR hands Windows Terminal back to the other names"
        );
        assert_eq!(color_level_for(false, false, "TrueColor", None), ColorLevel::TrueColor);
        assert_eq!(color_level_for(false, false, "24bit", Some("xterm")), ColorLevel::TrueColor);
        assert_eq!(color_level_for(false, false, "", Some("XTERM-256COLOR")), ColorLevel::Ansi256);
        assert_eq!(color_level_for(false, false, "yes", Some("xterm")), ColorLevel::Ansi16);
        assert_eq!(color_level_for(false, true, "", None), ColorLevel::Ansi16);
    }

    /// A slot under the text floor is lifted to measured RGB — except in the
    /// ZeroCode window, which lifts it itself and keeps lifting it through a
    /// change of the terminal's theme: there the slot keeps its index, so the
    /// window's next palette draws it. A slot over the floor keeps its index
    /// everywhere, and a dim slot is the measured dim ink everywhere.
    /// (Catppuccin Latte's blue, 4.34:1 on its ground.)
    #[test]
    fn a_slot_under_the_floor_keeps_its_index_where_the_window_lifts_it() {
        use super::{compute_ansi16_ink, contrast_ratio};
        let ground = (0xef, 0xf1, 0xf5);
        let mut slots = [None; 16];
        slots[4] = Some((0x1e, 0x66, 0xf5));
        slots[0] = Some((0x5c, 0x5f, 0x77));
        let latte = TerminalPalette::new((0x4c, 0x4f, 0x69), ground, ColorLevel::TrueColor).with_ansi16(slots);

        let lifted = compute_ansi16_ink(latte, 4, false, ground, false).expect("lifted outside the window");
        assert!(contrast_ratio(lifted, ground) >= latte.text_contrast_floor(ground), "{lifted:?}");
        assert_eq!(compute_ansi16_ink(latte, 4, false, ground, true), None);
        for window in [false, true] {
            assert_eq!(compute_ansi16_ink(latte, 0, false, ground, window), None, "over the floor");
            assert!(compute_ansi16_ink(latte, 4, true, ground, window).is_some(), "dim");
            assert_eq!(compute_ansi16_ink(latte, 6, false, ground, window), None, "an unknown slot");
        }
    }
}
