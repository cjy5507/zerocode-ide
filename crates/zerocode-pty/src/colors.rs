//! The colours a terminal answers colour queries with.
//!
//! A program is allowed to ask what it is being drawn in — `OSC 11 ; ? ST`
//! asks for the background, `OSC 4 ; n ; ? ST` for one palette entry, and
//! `CSI ? 996 n` asks only whether the answer is dark or light. Programs that
//! ask are the ones that adapt: `termenv` and everything built on it, `delta`,
//! `bat`, and the agent CLIs this product hosts. A terminal that stays silent
//! does not look neutral to them — it looks like a terminal whose background
//! they must guess, and the guess is what paints a light theme's colours onto
//! a dark screen.
//!
//! The grid cannot invent these. It is handed a resolved palette or it is
//! handed nothing, and while it has nothing it answers nothing: a fabricated
//! default would become the child's idea of the screen for the rest of its
//! life, and a fabricated black is worse than no answer at all.

/// One colour, eight bits a channel — the resolution a theme is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// The palette's length: sixteen named colours, the 6×6×6 cube, the grey ramp.
pub const ANSI_COLOR_COUNT: usize = 256;

/// A terminal's whole answer to "what am I being drawn in".
///
/// Held behind an [`std::sync::Arc`] by the grid: one resolved palette serves
/// every lane in the window, and a theme change replaces one pointer rather
/// than copying 768 bytes per terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalColors {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub ansi: [Rgb; ANSI_COLOR_COUNT],
}

impl TerminalColors {
    /// The 240 entries above the named sixteen, which are arithmetic rather
    /// than design: the xterm 6×6×6 cube and the 24-step grey ramp. Every
    /// terminal computes them the same way, so a theme only ever supplies the
    /// first sixteen and this fills the rest.
    #[must_use]
    pub fn with_standard_tail(
        foreground: Rgb,
        background: Rgb,
        cursor: Rgb,
        named: [Rgb; 16],
    ) -> Self {
        let mut ansi = [Rgb::default(); ANSI_COLOR_COUNT];
        ansi[..16].copy_from_slice(&named);
        for (index, slot) in ansi.iter_mut().enumerate().take(232).skip(16) {
            let cube = index - 16;
            *slot = Rgb(
                cube_level(cube / 36),
                cube_level((cube / 6) % 6),
                cube_level(cube % 6),
            );
        }
        for (index, slot) in ansi.iter_mut().enumerate().skip(232) {
            let grey = u8::try_from(8 + (index - 232) * 10).unwrap_or(u8::MAX);
            *slot = Rgb(grey, grey, grey);
        }
        Self {
            foreground,
            background,
            cursor,
            ansi,
        }
    }
}

const fn cube_level(part: usize) -> u8 {
    if part == 0 { 0 } else { (55 + part * 40) as u8 }
}

/// Format one colour the way a terminal reports it: `rgb:RRRR/GGGG/BBBB`.
///
/// Sixteen bits a channel with the eight-bit value repeated, which is what
/// XParseColor's widest form means and what the original replies with
/// byte-for-byte (`formatXColorRgbSpec`, terminal-view-attributes.ts).
#[must_use]
pub fn format_x_color_rgb_spec(rgb: Rgb) -> String {
    format!(
        "rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}",
        rgb.0, rgb.1, rgb.2
    )
}

/// XParseColor's grammar, as far as a terminal has to read it back.
///
/// `rgb:h/h/h` through `rgb:hhhh/hhhh/hhhh`, and `#RGB` through
/// `#RRRRGGGGBBBB`. Named colours and `rgbi:` are refused exactly as the
/// original refuses them — a terminal that half-understands a colour spec sets
/// a colour nobody asked for.
#[must_use]
pub fn parse_x_color_spec(spec: &str) -> Option<Rgb> {
    let spec = spec.trim();
    if let Some(body) = strip_prefix_ignore_ascii_case(spec, "rgb:") {
        let mut parts = body.split('/');
        let (red, green, blue) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let width = red.len();
        if width != green.len() || width != blue.len() || !(1..=4).contains(&width) {
            return None;
        }
        return Some(Rgb(
            scale_hex_channel(red, width)?,
            scale_hex_channel(green, width)?,
            scale_hex_channel(blue, width)?,
        ));
    }
    let body = spec.strip_prefix('#')?;
    if !matches!(body.len(), 3 | 6 | 9 | 12) {
        return None;
    }
    let width = body.len() / 3;
    Some(Rgb(
        scale_hex_channel(&body[..width], width)?,
        scale_hex_channel(&body[width..width * 2], width)?,
        scale_hex_channel(&body[width * 2..], width)?,
    ))
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let head = value.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &value[prefix.len()..])
}

/// One channel of `width` hex digits, scaled to eight bits.
fn scale_hex_channel(digits: &str, width: usize) -> Option<u8> {
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    let full = match width {
        1 => 0xf_u32,
        2 => 0xff,
        3 => 0xfff,
        _ => 0xffff,
    };
    u8::try_from((u64::from(value) * 255 + u64::from(full) / 2) / u64::from(full)).ok()
}

/// The WCAG relative luminance of one colour.
///
/// The formula xterm itself uses (`rgb.relativeLuminance2`), because the only
/// question it answers here is the one xterm answers with it: is this terminal
/// dark or light.
#[must_use]
pub fn relative_luminance(rgb: Rgb) -> f64 {
    fn linear(channel: u8) -> f64 {
        let value = f64::from(channel) / 255.0;
        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    linear(rgb.0) * 0.2126 + linear(rgb.1) * 0.7152 + linear(rgb.2) * 0.0722
}

/// Whether a terminal drawn in these two colours is a dark one.
///
/// Judged from the colours in force, not from the app's light/dark setting:
/// a dark terminal theme inside a light window is a dark terminal to the
/// program running in it, and it is the program's own colours that are at
/// stake.
#[must_use]
pub fn reads_as_dark(background: Rgb, foreground: Rgb) -> bool {
    relative_luminance(background) < relative_luminance(foreground)
}

#[cfg(test)]
mod tests {
    use super::{Rgb, TerminalColors, format_x_color_rgb_spec, parse_x_color_spec, reads_as_dark};

    #[test]
    fn a_reported_colour_repeats_each_byte_into_sixteen_bits() {
        assert_eq!(
            format_x_color_rgb_spec(Rgb(0x2e, 0x34, 0x34)),
            "rgb:2e2e/3434/3434"
        );
        assert_eq!(
            format_x_color_rgb_spec(Rgb(0xff, 0xff, 0xff)),
            "rgb:ffff/ffff/ffff"
        );
    }

    #[test]
    fn every_width_x_parse_color_allows_lands_on_the_same_colour() {
        for spec in [
            "rgb:f/0/0",
            "rgb:ff/00/00",
            "rgb:fff/000/000",
            "rgb:ffff/0000/0000",
        ] {
            assert_eq!(parse_x_color_spec(spec), Some(Rgb(0xff, 0, 0)), "{spec}");
        }
        for spec in ["#f00", "#ff0000", "#fff000000", "#ffff00000000"] {
            assert_eq!(parse_x_color_spec(spec), Some(Rgb(0xff, 0, 0)), "{spec}");
        }
        assert_eq!(parse_x_color_spec("RGB:FF/00/00"), Some(Rgb(0xff, 0, 0)));
    }

    #[test]
    fn a_spec_this_grammar_does_not_cover_is_refused_rather_than_guessed() {
        for spec in [
            "red",
            "rgbi:1.0/0.0/0.0",
            "rgb:ff/00",
            "rgb:ff/00/00/00",
            "rgb:fff/00/00",
            "#ff00",
            "#gg0000",
            "",
        ] {
            assert_eq!(parse_x_color_spec(spec), None, "{spec}");
        }
    }

    #[test]
    fn the_palette_above_sixteen_is_the_cube_and_the_grey_ramp() {
        let colors = TerminalColors::with_standard_tail(
            Rgb(0xb3, 0xb1, 0xad),
            Rgb(0x0a, 0x0e, 0x14),
            Rgb(0xe6, 0xb4, 0x50),
            [Rgb(1, 2, 3); 16],
        );
        assert_eq!(colors.ansi[0], Rgb(1, 2, 3));
        assert_eq!(colors.ansi[16], Rgb(0, 0, 0));
        assert_eq!(colors.ansi[21], Rgb(0, 0, 255));
        assert_eq!(colors.ansi[231], Rgb(255, 255, 255));
        assert_eq!(colors.ansi[232], Rgb(8, 8, 8));
        assert_eq!(colors.ansi[255], Rgb(238, 238, 238));
    }

    #[test]
    fn dark_is_read_from_the_colours_in_force() {
        assert!(reads_as_dark(Rgb(0x0a, 0x0e, 0x14), Rgb(0xb3, 0xb1, 0xad)));
        assert!(!reads_as_dark(Rgb(0xff, 0xff, 0xff), Rgb(0x2e, 0x34, 0x34)));
    }
}
