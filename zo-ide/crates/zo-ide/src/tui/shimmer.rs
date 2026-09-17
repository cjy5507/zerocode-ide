//! 왼→오 대비 웨이브 — codex `tui/src/shimmer.rs` 의 규칙 그대로.
//!
//! 원본 알고리즘: 글자 앞뒤로 10칸 패딩을 두고 `len + 20` 주기를 2초에 한 번
//! 훑는다. 밴드 반폭은 5칸, 세기는 `t = 0.5·(1 + cos(π·d/5))`, 색은
//! `blend(background, foreground, t·0.9)` 를 절삭한다. OSC 기본색을 알 수
//! 없을 때만 회색→흰색 캡처 폴백을 쓴다. 트루컬러가 아니면 RGB를 양자화하지
//! 않고 DIM/plain/BOLD 세 단계로 같은 세기를 표현한다.
//!
//! 시간은 인자로 받는다 — 프로세스 시작 시각을 감추면 프레임 하나를 바이트로
//! 핀할 수 없다.

use std::time::Duration;

use super::ansi::{Color, Span, Style};
use super::palette::{self, ColorLevel, SHIMMER_BASE, SHIMMER_PEAK, TerminalPalette};

/// 글자 앞뒤 패딩 — 웨이브가 문구 밖에서 들어오고 나간다.
const PADDING: usize = 10;
/// 한 바퀴에 걸리는 시간.
const SWEEP: f32 = 2.0;
/// 밴드 반폭(글자 수).
const BAND_HALF_WIDTH: f32 = 5.0;
/// 최고 세기에서도 바닥색을 완전히 덮지는 않는다 — codex 의 계수.
const PEAK_ALPHA: f32 = 0.9;

/// 두 색을 섞는다 — codex `color::blend`. 절삭(`as u8`)까지 같아야 캡처와
/// 바이트가 맞는다.
#[must_use]
pub fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    palette::blend_rgb(fg, bg, alpha)
}

/// `elapsed` 시점의 shimmer 스팬들. 빈 문자열이면 빈 벡터.
#[must_use]
pub fn shimmer_spans(text: &str, elapsed: Duration) -> Vec<Span> {
    let terminal_palette = palette::terminal_palette();
    let color_level = terminal_palette.map_or_else(
        palette::color_level_from_env,
        TerminalPalette::color_level,
    );
    shimmer_spans_for_palette_and_level(text, elapsed, terminal_palette, color_level)
}

/// `elapsed` 시점의 shimmer 스팬들. 명시한 팔레트는 순수 테스트 이음매이고,
/// `None`은 기존 트루컬러 캡처의 회색→흰색 밴드를 정확히 보존한다. Production
/// 깊이 게이트는 [`shimmer_spans`]가 별도로 적용한다.
#[must_use]
pub fn shimmer_spans_for_palette(
    text: &str,
    elapsed: Duration,
    terminal_palette: Option<TerminalPalette>,
) -> Vec<Span> {
    let color_level = terminal_palette.map_or(ColorLevel::TrueColor, TerminalPalette::color_level);
    shimmer_spans_for_palette_and_level(text, elapsed, terminal_palette, color_level)
}

fn shimmer_spans_for_palette_and_level(
    text: &str,
    elapsed: Duration,
    terminal_palette: Option<TerminalPalette>,
    color_level: ColorLevel,
) -> Vec<Span> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let (base, peak) = terminal_palette
        .map_or((SHIMMER_BASE, SHIMMER_PEAK), TerminalPalette::shimmer_colors);
    let period = chars.len() + PADDING * 2;
    #[allow(clippy::cast_precision_loss)] // 주기는 수십 단위 — f32 로 충분하다.
    let period_f = period as f32;
    let position = (elapsed.as_secs_f32() % SWEEP) / SWEEP * period_f;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let position = position as isize;

    chars
        .iter()
        .enumerate()
        .map(|(index, ch)| {
            #[allow(clippy::cast_possible_wrap)]
            let cell = index as isize + PADDING as isize;
            #[allow(clippy::cast_precision_loss)]
            let distance = (cell - position).abs() as f32;
            let intensity = if distance <= BAND_HALF_WIDTH {
                let x = std::f32::consts::PI * (distance / BAND_HALF_WIDTH);
                0.5 * (1.0 + x.cos())
            } else {
                0.0
            };
            let style = if color_level == ColorLevel::TrueColor {
                let (r, g, b) = blend(
                    peak,
                    base,
                    intensity.clamp(0.0, 1.0) * PEAK_ALPHA,
                );
                Style::new().fg(Color::Rgb(r, g, b)).bold()
            } else {
                color_for_level(intensity)
            };
            Span::new(ch.to_string(), style)
        })
        .collect()
}

fn color_for_level(intensity: f32) -> Style {
    if intensity < 0.2 {
        Style::new().dim()
    } else if intensity < 0.6 {
        Style::new()
    } else {
        Style::new().bold()
    }
}

/// 활동 표시 점. codex 는 같은 shimmer 의 첫 글자를 쓴다.
#[must_use]
pub fn activity_marker(elapsed: Duration) -> Span {
    shimmer_spans("•", elapsed)
        .into_iter()
        .next()
        .unwrap_or_else(|| Span::dim("•"))
}

/// 경과 시간 표기 — codex `fmt_elapsed_compact`.
///
/// 문안은 [`core_types::helper_run`] 한 곳에 있다. 상태줄·스폰 셀·완료 요약이
/// 같은 초를 다르게 적으면 안 되기 때문이다.
pub use core_types::helper_run::elapsed_compact as fmt_elapsed_compact;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{blend, fmt_elapsed_compact, shimmer_spans, shimmer_spans_for_palette};
    use crate::tui::ansi::{Color, Style};
    use crate::tui::palette::{ColorLevel, SHIMMER_BASE, SHIMMER_PEAK, TerminalPalette};

    /// 캡처(`codex-tui-v0.149.1-turn.bin`)가 `Working` 한 프레임에 흘린 6단계.
    #[test]
    fn band_levels_match_the_capture() {
        let levels: Vec<u8> = (0u8..=5)
            .map(|distance| {
                let x = std::f32::consts::PI * (f32::from(distance) / 5.0);
                let intensity = 0.5 * (1.0 + x.cos());
                blend(SHIMMER_PEAK, SHIMMER_BASE, intensity * 0.9).0
            })
            .collect();
        assert_eq!(levels, vec![242, 231, 202, 167, 138, 128]);
    }

    #[test]
    fn the_band_peak_sits_under_the_sweep_position() {
        // 10칸 패딩 + 7글자 = 주기 27. 2초를 27로 나눈 한 칸이 곧 한 글자다.
        let spans = shimmer_spans_for_palette(
            "Working",
            Duration::from_secs_f32(2.0 * 12.0 / 27.0),
            None,
        );
        let peak = spans
            .iter()
            .position(|span| span.style.fg == Some(Color::Rgb(242, 242, 242)))
            .expect("peak span");
        assert_eq!(peak, 2);
        assert!(spans.iter().all(|span| span.style.bold));
    }

    #[test]
    fn a_measured_palette_darkens_from_foreground_toward_background() {
        let palette = TerminalPalette::new(
            (240, 240, 240),
            (10, 10, 10),
            ColorLevel::TrueColor,
        );
        let elapsed = Duration::from_secs_f32(2.0 * 10.0 / 21.0);
        let spans = shimmer_spans_for_palette("W", elapsed, Some(palette));

        assert_eq!(spans[0].style.fg, Some(Color::Rgb(33, 33, 33)));
        assert!(spans[0].style.bold);
    }

    #[test]
    fn non_truecolor_shimmer_uses_the_dim_plain_bold_ramp() {
        let palette = TerminalPalette::new(
            (240, 240, 240),
            (10, 10, 10),
            ColorLevel::Ansi256,
        );
        let style_at = |position: f32| {
            shimmer_spans_for_palette(
                "W",
                Duration::from_secs_f32(2.0 * position / 21.0),
                Some(palette),
            )[0]
            .style
        };

        assert_eq!(style_at(0.0), Style::new().dim());
        assert_eq!(style_at(7.0), Style::new());
        assert_eq!(style_at(10.0), Style::new().bold());
    }

    #[test]
    fn empty_text_has_no_spans() {
        assert!(shimmer_spans("", Duration::ZERO).is_empty());
    }

    #[test]
    fn elapsed_formatting_matches_codex() {
        assert_eq!(fmt_elapsed_compact(0), "0s");
        assert_eq!(fmt_elapsed_compact(59), "59s");
        assert_eq!(fmt_elapsed_compact(60), "1m 00s");
        assert_eq!(fmt_elapsed_compact(3600), "1h 00m 00s");
        assert_eq!(fmt_elapsed_compact(25 * 3600 + 2 * 60 + 3), "25h 02m 03s");
    }
}
