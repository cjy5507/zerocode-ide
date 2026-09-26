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

use crossterm::event::{KeyCode, KeyEvent};

use super::ansi::{Color, Style};

type DefaultColors = ((u8, u8, u8), (u8, u8, u8));

/// OSC 조회가 기다리는 전체 시간. Codex startup probe와 같은 상한이다.
#[cfg(unix)]
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(100);

/// shimmer 밴드의 바닥색 — 응답이 없을 때의 캡처 값.
pub const SHIMMER_BASE: (u8, u8, u8) = (128, 128, 128);
/// shimmer 밴드의 꼭대기 재료 — 응답이 없을 때의 캡처 값.
pub const SHIMMER_PEAK: (u8, u8, u8) = (255, 255, 255);

/// Max tier 컴포저 caret `›` 의 캡처 폴백.
pub const COMPOSER_CARET: Color = Color::Rgb(255, 178, 66);
/// 푸터의 `<model> <effort>` 캡처 폴백.
pub const FOOTER_MODEL: Color = Color::Rgb(246, 226, 183);
/// 푸터의 cwd 캡처 폴백.
pub const FOOTER_CWD: Color = Color::Rgb(171, 223, 167);

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
/// amber on a dark background (the light one is `LIGHT_WARNING_NOTICE`).
pub const WARNING_NOTICE: Color = Color::Rgb(196, 167, 103);
/// 끝난 도구 셀의 성공 불릿.
pub const TOOL_OK: Color = Color::Indexed(2);
/// 끝난 도구 셀의 실패 불릿.
pub const TOOL_FAIL: Color = Color::Indexed(1);
/// `Exploring` 목록과 MCP 셀의 도구 이름.
pub const TOOL_LABEL: Color = Color::Indexed(6);
/// 표 헤더 행의 전경색.
pub const TABLE_HEADER: Color = Color::Rgb(249, 226, 175);
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
        let (red, green, blue) = self.background;
        u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114 > 128_000
    }

    /// Colours for the moving status band. Codex uses the terminal foreground
    /// as the base and sweeps toward the terminal background at the peak. The
    /// old implementation mixed a dim base and then swept back toward the
    /// foreground, which reversed that polarity.
    #[must_use]
    pub fn shimmer_colors(self) -> ((u8, u8, u8), (u8, u8, u8)) {
        (self.foreground, self.background)
    }

    /// 실제 깊이에서 표현할 수 있는 전경/배경색으로 낮춘다.
    ///
    /// ANSI-16에서 RGB나 확장 인덱스를 기본색으로 억지 근사하면 사용자가 바꾼
    /// 16색 표를 추측하게 된다. Codex처럼 그 경우는 terminal default(`None`)로
    /// 두고, 0~7 인덱스만 동일한 base SGR로 옮긴다.
    #[must_use]
    pub fn resolve(self, color: Color) -> Option<Color> {
        let color = self.adapt_for_background(color);
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
        }
    }

    fn adapt_for_background(self, color: Color) -> Color {
        if !self.has_light_background() {
            return color;
        }
        match color {
            COMPOSER_CARET => LIGHT_COMPOSER_CARET,
            FOOTER_MODEL => LIGHT_FOOTER_MODEL,
            FOOTER_CWD => LIGHT_FOOTER_CWD,
            TABLE_HEADER => LIGHT_TABLE_HEADER,
            WARNING_NOTICE => LIGHT_WARNING_NOTICE,
            _ => color,
        }
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

/// startup probe가 성공했을 때만 적응 팔레트를 돌려준다.
#[must_use]
pub(crate) fn terminal_palette() -> Option<TerminalPalette> {
    TERMINAL_PALETTE.get().copied().flatten()
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
            // `ESC \` or BEL closes the string; the terminator's own bytes go
            // with it.
            if matches!(key.code, KeyCode::Char('\\')) || matches!(key.code, KeyCode::Char('\u{7}')) {
                self.inside = false;
                self.until = now;
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
    let (foreground, background) = query_default_colors()?;
    Some(TerminalPalette::new(
        foreground,
        background,
        color_level_from_env(),
    ))
}

pub(crate) fn color_level_from_env() -> ColorLevel {
    if std::env::var_os("WT_SESSION").is_some() && std::env::var_os("FORCE_COLOR").is_none() {
        return ColorLevel::TrueColor;
    }
    let colorterm = std::env::var("COLORTERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return ColorLevel::TrueColor;
    }
    if std::env::var("TERM")
        .is_ok_and(|term| term.to_ascii_lowercase().contains("256color"))
    {
        return ColorLevel::Ansi256;
    }
    ColorLevel::Ansi16
}

#[cfg(unix)]
fn query_default_colors() -> Option<DefaultColors> {
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
    tty.write_all(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\").ok()?;
    tty.flush().ok()?;

    // The reply may still be in flight when the budget runs out — a tmux relay,
    // an SSH hop, or a terminal still booting can all miss 100ms. Record that
    // so the event loop can swallow the answer instead of typing it into the
    // composer: measured, a reply that arrived at 150ms became 46 characters of
    // `]10;rgb:…` sitting in the prompt, one Enter away from being sent.
    PROBE_SENT.store(true, std::sync::atomic::Ordering::Release);
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut response = Vec::with_capacity(128);
    let mut chunk = [0u8; 256];
    while Instant::now() < deadline {
        match tty.read(&mut chunk) {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(1)),
            Ok(read) => {
                response.extend_from_slice(&chunk[..read]);
                if let (Some(foreground), Some(background)) =
                    (parse_osc_color(&response, 10), parse_osc_color(&response, 11))
                {
                    PROBE_ANSWERED.store(true, std::sync::atomic::Ordering::Release);
                    return Some((foreground, background));
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
    None
}

#[cfg(not(unix))]
const fn query_default_colors() -> Option<DefaultColors> {
    None
}

#[cfg(any(unix, test))]
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

fn nearest_xterm_index(target: (u8, u8, u8)) -> u8 {
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

}
