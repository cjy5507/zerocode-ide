//! Max/Ultra effort 전환 때 컴포저와 푸터에서 한 번만 도는 이펙트.
//!
//! 정본은 실제 `codex-cli 0.150.1` PTY 캡처
//! (`docs/captures/codex-tui-v0.150.1-effort-max-ultra.bin`)와 Codex의
//! `bottom_pane/effort_status_line.rs`다. 이전 푸터가 620ms 동안 오른쪽으로
//! 밀려난 뒤, tier 글자가 700ms 동안 모이고 360ms 머물고 340ms 사라지며,
//! 새 푸터가 480ms 동안 돌아온다. 프레임 틱은 원본과 같은 33ms다.
//!
//! Codex에는 Smart tier가 없다. Smart 라벨과 청록 accent는 `ZeroCode`의
//! 동적 effort를 같은 피드백 계약에 넣기 위한 의도된 확장이다.

use std::cell::Cell;
use std::time::{Duration, Instant};

use crate::effort::Effort;

use super::ansi::{char_width, Color, Line, Span, Style};

const SCROLL_OUT: Duration = Duration::from_millis(620);
const LABEL_ASSEMBLE: Duration = Duration::from_millis(700);
const LABEL_HOLD: Duration = Duration::from_millis(360);
const LABEL_FADE_OUT: Duration = Duration::from_millis(340);
const STATUS_FADE_IN: Duration = Duration::from_millis(480);
const TOTAL: Duration = Duration::from_millis(2_500);

/// Codex가 실제 전환에서 예약하는 케이던스.
pub const FRAME_TICK: Duration = Duration::from_millis(33);

const DEFAULT_FG: (u8, u8, u8) = (224, 224, 224);
const DEFAULT_BG: (u8, u8, u8) = (30, 32, 36);
const BORDER_IDLE: (u8, u8, u8) = (128, 128, 128);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffortTier {
    Max,
    Ultra,
    Smart,
}

impl EffortTier {
    #[must_use]
    pub const fn from_effort(effort: Effort) -> Option<Self> {
        match effort {
            Effort::Max => Some(Self::Max),
            Effort::Ultra => Some(Self::Ultra),
            Effort::Smart => Some(Self::Smart),
            Effort::Off | Effort::Low | Effort::Medium | Effort::High | Effort::Xhigh => None,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Max => "MAX",
            Self::Ultra => "ULTRA",
            Self::Smart => "SMART",
        }
    }

    const fn accent(self) -> (u8, u8, u8) {
        match self {
            // Codex dark-background hues from `EffortTier::hues`.
            Self::Max => (255, 178, 66),
            Self::Ultra => (186, 130, 255),
            // ZeroCode-only Smart extension: distinct from both static pins.
            Self::Smart => (80, 210, 190),
        }
    }

    /// tier 의 마커 **글자 하나** — codex `EffortTier::prompt_glyph`
    /// (`bottom_pane/effort_ignition.rs`: `Max => "›"`, `Ultra => "»"`).
    /// 뒤의 빈칸은 여기 없다: 그것은 스타일 없는 별개 스팬이다
    /// (`super::composer::CARET_GAP`).
    const fn caret(self) -> &'static str {
        match self {
            Self::Max => "›",
            Self::Ultra => "»",
            Self::Smart => "◆",
        }
    }

    #[must_use]
    pub fn caret_span(self) -> Span {
        let (red, green, blue) = self.accent();
        Span::new(
            self.caret(),
            Style::new().bold().fg(Color::Rgb(red, green, blue)),
        )
    }
}

/// 한 전환의 시작 tier와, 밀려날 직전 푸터.
#[derive(Debug)]
pub struct EffortEffect {
    tier: EffortTier,
    previous_footer: Line,
    // Codex처럼 실제 컴포저/푸터가 처음 보인 프레임에 시계를 시작한다.
    started_at: Cell<Option<Instant>>,
}

impl EffortEffect {
    #[must_use]
    pub const fn new(tier: EffortTier, previous_footer: Line) -> Self {
        Self {
            tier,
            previous_footer,
            started_at: Cell::new(None),
        }
    }

    #[must_use]
    pub fn is_finished_at(&self, now: Instant) -> bool {
        self.started_at
            .get()
            .is_some_and(|started| now.saturating_duration_since(started) >= TOTAL)
    }

    fn elapsed_at(&self, now: Instant) -> Duration {
        let started = if let Some(started) = self.started_at.get() {
            started
        } else {
            self.started_at.set(Some(now));
            now
        };
        now.saturating_duration_since(started)
    }

    /// 현재 푸터 자리에 놓을 원본 Codex 전환 프레임.
    #[must_use]
    pub fn footer_line_at(&self, current: &Line, width: usize, now: Instant) -> Line {
        transition_line_at(
            self.tier,
            Some(&self.previous_footer),
            Some(current),
            self.elapsed_at(now),
            width,
        )
        .unwrap_or_else(Line::empty)
    }

    /// 정적 테두리에 composer-band ignition의 tier 색을 한 번 실어 보낸다.
    ///
    /// 이 렌더러는 셀 배경색을 소유하지 않으므로 Codex의 배경 Wave/Aurora/Pulse를
    /// 새 테두리의 전경 glow로 옮긴다. 범위는 컴포저 블록 안뿐이고, 1.3초 뒤
    /// 반드시 중립색으로 돌아온다.
    #[must_use]
    pub fn border_style_at(&self, now: Instant) -> Style {
        let elapsed = self.elapsed_at(now).as_secs_f32();
        let total = match self.tier {
            EffortTier::Max => 1.0,
            EffortTier::Ultra => 1.3,
            EffortTier::Smart => 1.5,
        };
        let progress = (elapsed / total).clamp(0.0, 1.0);
        let glow = (std::f32::consts::PI * progress).sin().max(0.0);
        let rgb = blend(self.tier.accent(), BORDER_IDLE, 0.18 + 0.82 * glow);
        Style::new().fg(Color::Rgb(rgb.0, rgb.1, rgb.2))
    }
}

#[must_use]
pub fn idle_border_style() -> Style {
    Style::new().fg(Color::Rgb(BORDER_IDLE.0, BORDER_IDLE.1, BORDER_IDLE.2))
}

fn transition_line_at(
    tier: EffortTier,
    previous: Option<&Line>,
    current: Option<&Line>,
    elapsed: Duration,
    width: usize,
) -> Option<Line> {
    if width == 0 {
        return None;
    }

    if elapsed < SCROLL_OUT {
        let progress = elapsed.as_secs_f32() / SCROLL_OUT.as_secs_f32();
        let width_f32 = f32::from(u16::try_from(width).unwrap_or(u16::MAX));
        // `progress` is clamped to 0..=1 and width came through u16, so this
        // rounded cell coordinate is non-negative and representable.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let offset = (width_f32 * ease_in_cubic(progress)).round() as usize;
        let mut line = previous.cloned()?;
        let tint = 0.12 + 0.88 * progress;
        style_line(&mut line, tier, 1.0 - progress * progress, tint);
        line.spans.insert(0, Span::raw(" ".repeat(offset)));
        return Some(clip_line(line, width));
    }

    let after_scroll = elapsed.saturating_sub(SCROLL_OUT);
    let label_duration = LABEL_ASSEMBLE
        .saturating_add(LABEL_HOLD)
        .saturating_add(LABEL_FADE_OUT);
    if after_scroll < label_duration {
        let (assemble, opacity) = if after_scroll < LABEL_ASSEMBLE {
            let progress = after_scroll.as_secs_f32() / LABEL_ASSEMBLE.as_secs_f32();
            (ease_out_cubic(progress), (progress / 0.55).clamp(0.0, 1.0))
        } else if after_scroll < LABEL_ASSEMBLE.saturating_add(LABEL_HOLD) {
            (1.0, 1.0)
        } else {
            let fade = after_scroll.saturating_sub(LABEL_ASSEMBLE.saturating_add(LABEL_HOLD));
            (1.0, 1.0 - fade.as_secs_f32() / LABEL_FADE_OUT.as_secs_f32())
        };
        return Some(tier_label_line(tier, width, assemble, opacity));
    }

    let fade_elapsed = after_scroll.saturating_sub(label_duration);
    let opacity = (fade_elapsed.as_secs_f32() / STATUS_FADE_IN.as_secs_f32()).clamp(0.0, 1.0);
    let mut line = current.cloned()?;
    style_line(&mut line, tier, opacity, 0.0);
    Some(clip_line(line, width))
}

fn ease_in_cubic(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    progress * progress * progress
}

fn ease_out_cubic(progress: f32) -> f32 {
    let inverse = 1.0 - progress.clamp(0.0, 1.0);
    1.0 - inverse * inverse * inverse
}

fn tier_label_line(tier: EffortTier, width: usize, assemble: f32, opacity: f32) -> Line {
    let letters = tier.label().chars().collect::<Vec<_>>();
    let gap_count = letters.len().saturating_sub(1);
    let compact_width = letters.len().saturating_add(gap_count);
    let available = u16::try_from(width.saturating_sub(compact_width)).unwrap_or(u16::MAX);
    // `assemble` is clamped and `available` is a terminal-width u16.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let spread = (f32::from(available) * (1.0 - assemble.clamp(0.0, 1.0))).round() as usize;
    let gap_weights = (0..gap_count)
        .map(|index| {
            let position = index.saturating_mul(2).saturating_add(1);
            position.abs_diff(gap_count).saturating_add(1)
        })
        .collect::<Vec<_>>();
    let weight_total = gap_weights.iter().sum::<usize>().max(1);
    let gaps = gap_weights
        .iter()
        .map(|weight| 1 + spread.saturating_mul(*weight) / weight_total)
        .collect::<Vec<_>>();
    let label_width = letters.len().saturating_add(gaps.iter().sum::<usize>());
    let mut spans = vec![Span::raw(" ".repeat(width.saturating_sub(label_width) / 2))];

    for (index, letter) in letters.iter().enumerate() {
        let center = letters.len().saturating_sub(1);
        let edge = if center == 0 {
            0.0
        } else {
            let distance = u16::try_from(index.saturating_mul(2).abs_diff(center))
                .unwrap_or(u16::MAX);
            let center = u16::try_from(center).unwrap_or(u16::MAX);
            f32::from(distance) / f32::from(center)
        };
        let stagger = 0.22 * edge;
        let letter_opacity = ((opacity - stagger) / (1.0 - stagger)).clamp(0.0, 1.0);
        let mut style = Style::new().bold();
        apply_fade(&mut style, tier, letter_opacity, 1.0);
        spans.push(Span::new(letter.to_string(), style));
        if let Some(gap) = gaps.get(index) {
            spans.push(Span::raw(" ".repeat(*gap)));
        }
    }
    clip_line(Line::new(spans), width)
}

fn style_line(line: &mut Line, tier: EffortTier, opacity: f32, tint: f32) {
    apply_fade(&mut line.style, tier, opacity, tint);
    for span in &mut line.spans {
        apply_fade(&mut span.style, tier, opacity, tint);
    }
}

fn apply_fade(style: &mut Style, tier: EffortTier, opacity: f32, tint: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    let tint = tint.clamp(0.0, 1.0);
    if matches!(style.fg, Some(color) if !matches!(color, Color::Rgb(..))) {
        style.dim |= opacity < 0.7;
        return;
    }
    let foreground = match style.fg {
        Some(Color::Rgb(red, green, blue)) => (red, green, blue),
        _ => DEFAULT_FG,
    };
    let tinted = blend(tier.accent(), foreground, tint);
    let faded = blend(tinted, DEFAULT_BG, opacity);
    style.fg = Some(Color::Rgb(faded.0, faded.1, faded.2));
    style.dim |= opacity < 0.7;
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn blend(foreground: (u8, u8, u8), background: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let alpha = alpha.clamp(0.0, 1.0);
    let channel = |front: u8, back: u8| {
        // Both inputs and the clamped interpolation remain within 0..=255.
        (f32::from(front) * alpha + f32::from(back) * (1.0 - alpha)).round() as u8
    };
    (
        channel(foreground.0, background.0),
        channel(foreground.1, background.1),
        channel(foreground.2, background.2),
    )
}

fn clip_line(line: Line, width: usize) -> Line {
    if line.width() <= width {
        return line;
    }
    let (style, lead, trail) = (line.style, line.lead, line.trail);
    let mut used = 0usize;
    let mut spans = Vec::new();
    for span in line.spans {
        if used >= width {
            break;
        }
        let mut text = String::new();
        for ch in span.text.chars() {
            let cells = char_width(ch);
            if used + cells > width {
                break;
            }
            used += cells;
            text.push(ch);
        }
        if !text.is_empty() {
            spans.push(Span::new(text, span.style));
        }
    }
    Line {
        spans,
        style,
        lead,
        trail,
        continuation: None,
        origin: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_status_phases_keep_the_codex_order() {
        let previous = Line::from_text("gpt-5.6-sol medium · /tmp");
        let current = Line::from_text("gpt-5.6-sol ultra · /tmp");
        let label = transition_line_at(
            EffortTier::Ultra,
            Some(&previous),
            Some(&current),
            Duration::from_millis(1_500),
            32,
        )
        .expect("label");
        let refreshed = transition_line_at(
            EffortTier::Ultra,
            Some(&previous),
            Some(&current),
            Duration::from_millis(2_250),
            32,
        )
        .expect("current");
        assert_eq!(label.plain(), "           U L T R A");
        assert_eq!(refreshed.plain(), "gpt-5.6-sol ultra · /tmp");
    }

    #[test]
    fn all_effect_lines_fit_narrow_widths() {
        let previous = Line::from_text("긴 footer 👩‍💻");
        let current = Line::from_text("smart footer");
        for width in 0..=12 {
            for elapsed in [0, 300, 700, 1_500, 2_250, 2_500] {
                let line = transition_line_at(
                    EffortTier::Smart,
                    Some(&previous),
                    Some(&current),
                    Duration::from_millis(elapsed),
                    width,
                );
                assert!(line.is_none_or(|line| line.width() <= width));
            }
        }
    }

    #[test]
    fn transition_returns_to_idle_after_exactly_two_and_a_half_seconds() {
        let start = Instant::now();
        let effect = EffortEffect::new(EffortTier::Max, Line::from_text("old footer"));
        assert!(!effect.is_finished_at(start));
        let _ = effect.footer_line_at(&Line::from_text("new footer"), 40, start);
        assert!(!effect.is_finished_at(start + Duration::from_millis(2_499)));
        assert!(effect.is_finished_at(start + Duration::from_millis(2_500)));
    }
}
