//! The terminal's look, as the window's page resolved it (`dom-theme.json`,
//! written by `phase0/dom-replay.mjs`): the GPUI grid paints with the same
//! palette, ink, ground, face and cell, so a difference between the two
//! renderers is theirs and not the theme's.

use std::fs;
use std::path::Path;

use gpui::{Hsla, Rgba};
use serde_json::Value;
use zerocode_pty::{CellStyle, Color};

#[derive(Debug, Clone)]
pub struct Theme {
    pub palette: Vec<Rgba>,
    pub fg: Rgba,
    pub bg: Rgba,
    pub cursor: Rgba,
    pub font_size: f32,
    pub weight: f32,
    pub bold_weight: f32,
    pub dim_ink: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

/// `rgb(1, 2, 3)` or `rgba(1, 2, 3, 0.5)`, as `getComputedStyle` writes them.
fn css_color(text: &str) -> Result<Rgba, String> {
    let inner = text
        .trim()
        .strip_prefix("rgba(")
        .or_else(|| text.trim().strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or_else(|| format!("not an rgb() colour: {text}"))?;
    let parts: Vec<f32> = inner
        .split([',', ' ', '/'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<f32>()
                .map_err(|error| format!("{text}: {error}"))
        })
        .collect::<Result<_, _>>()?;
    let [r, g, b] = [parts.first(), parts.get(1), parts.get(2)]
        .map(|part| part.copied().unwrap_or(0.0) / 255.0);
    Ok(Rgba {
        r,
        g,
        b,
        a: parts.get(3).copied().unwrap_or(1.0),
    })
}

fn css_px(value: &Value, what: &str) -> Result<f32, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{what} is not text"))?;
    text.trim()
        .trim_end_matches("px")
        .parse::<f32>()
        .map_err(|error| format!("{what} {text}: {error}"))
}

fn number(value: &Value, what: &str) -> Result<f32, String> {
    value
        .as_f64()
        .map(|number| number as f32)
        .ok_or_else(|| format!("{what} is not a number"))
}

impl Theme {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let json: Value =
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        let palette = json["palette"]
            .as_array()
            .ok_or("the theme has no palette")?
            .iter()
            .map(|entry| css_color(entry.as_str().unwrap_or_default()))
            .collect::<Result<Vec<_>, _>>()?;
        if palette.len() != 256 {
            return Err(format!(
                "the palette holds {} colours, not 256",
                palette.len()
            ));
        }
        let weight = json["font"]["weight"]
            .as_str()
            .and_then(|weight| weight.parse::<f32>().ok())
            .unwrap_or(400.0);
        Ok(Self {
            palette,
            fg: css_color(json["fg"].as_str().unwrap_or_default())?,
            bg: css_color(json["bgSolid"].as_str().unwrap_or_default())?,
            cursor: css_color(json["cursor"].as_str().unwrap_or_default())?,
            font_size: css_px(&json["font"]["size"], "font size")?,
            weight,
            bold_weight: json["boldWeight"]
                .as_str()
                .and_then(|weight| weight.parse::<f32>().ok())
                .unwrap_or(700.0),
            dim_ink: json["dimInk"]
                .as_str()
                .and_then(|ink| {
                    let ink = ink.trim();
                    ink.strip_suffix('%')
                        .and_then(|percent| percent.trim().parse::<f32>().ok())
                        .map(|percent| percent / 100.0)
                        .or_else(|| ink.parse::<f32>().ok())
                })
                .unwrap_or(0.5),
            cell_width: number(&json["cell"]["width"], "cell width")?,
            cell_height: number(&json["cell"]["height"], "cell height")?,
        })
    }

    fn color(&self, color: Color) -> Option<Rgba> {
        match color {
            Color::Default => None,
            Color::Indexed(index) => self.palette.get(usize::from(index)).copied(),
            Color::Rgb(r, g, b) => Some(Rgba {
                r: f32::from(r) / 255.0,
                g: f32::from(g) / 255.0,
                b: f32::from(b) / 255.0,
                a: 1.0,
            }),
        }
    }

    /// The ink and ground one cell is painted with, by the rules the window's
    /// runs follow (`runsOf`, ui/shell-term.js): reverse swaps the two first,
    /// then bold lifts an ink from the low half of the 16-colour palette into
    /// its bright twin, and dim keeps `dim_ink` of the ink. The window's
    /// legibility lift (a minimum contrast against the ground) is not done
    /// here; the comparison counts the cells it changed.
    pub fn paint(&self, style: &CellStyle) -> (Rgba, Option<Rgba>) {
        let (mut fg_spec, mut bg_spec) = (style.fg, style.bg);
        let (mut fg_fall, mut bg_fall) = (self.fg, None);
        if style.reverse {
            std::mem::swap(&mut fg_spec, &mut bg_spec);
            fg_fall = self.bg;
            bg_fall = Some(self.fg);
        }
        if style.bold
            && let Color::Indexed(index) = fg_spec
            && index < 8
        {
            fg_spec = Color::Indexed(index + 8);
        }
        let mut fg = self.color(fg_spec).unwrap_or(fg_fall);
        if style.dim {
            fg.a *= self.dim_ink;
        }
        let bg = self.color(bg_spec).or(bg_fall);
        (fg, bg)
    }
}

pub fn hsla(color: Rgba) -> Hsla {
    Hsla::from(color)
}

/// `#rrggbbaa`, for the record a capture writes.
pub fn hex(color: Rgba) -> String {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        byte(color.r),
        byte(color.g),
        byte(color.b),
        byte(color.a)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computed_colours_parse_in_both_spellings() {
        let red = css_color("rgb(204, 102, 102)").unwrap();
        assert!((red.r - 0.8).abs() < 0.01 && (red.g - 0.4).abs() < 0.01);
        let half = css_color("rgba(0, 0, 0, 0.5)").unwrap();
        assert!((half.a - 0.5).abs() < 0.001);
        let spaced = css_color("rgb(1 2 3)").unwrap();
        assert!((spaced.b - 3.0 / 255.0).abs() < 0.001);
        assert!(css_color("red").is_err());
    }
}
