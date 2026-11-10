//! Claude Code's colours, from the terminal palette the window paints.
//!
//! Claude Code draws its own interface — the prompt's border, the spinner,
//! diff rows, the words for a warning or a permission — in colours of its own
//! theme, chosen without knowing what it is drawn on. In a pane whose palette
//! a person chose, those colours are somebody else's: an orange spinner on a
//! Solarized screen, a diff's green that is not the terminal's green.
//!
//! Since 2.1.119 Claude Code loads a **custom theme** from
//! `<config dir>/themes/<slug>.json` — `{name, base, overrides}`, the base one
//! of its built-in themes and the overrides its own colour keys — selected by
//! `"theme": "custom:<slug>"` in that directory's `settings.json`, and it
//! watches the folder, so a session already running recolours when the file
//! changes (measured in the 2.1.321 binary: `loadCustomThemes`, the `themes`
//! directory under the config dir, a 256 KiB cap per file, overrides kept only
//! for keys the base theme has).
//!
//! This module is the pure half: the theme a palette makes. It is Orca's
//! mapping, measured verbatim (stablyai/orca#39447,
//! `claudeCodeThemeFromTerminal`, with `rgba-contrast.ts`'s `mix`,
//! `readableOn` and WCAG `contrastRatio`): hues come from the palette's own
//! named colours, surfaces are those hues tinted into the background, and
//! every colour that carries text or a status is pushed toward black or white
//! only as far as it takes to read — so a low-contrast palette keeps its hue
//! instead of losing the colour to a contrast gate. Writing the file and
//! choosing it are the window's (`zerocode-shell`, `claude_theme_sync`).

use std::collections::BTreeMap;

/// The slug the window's theme is filed and selected under.
pub const SLUG: &str = "zerocode";

/// What `settings.json` says to select it.
pub const SETTING: &str = "custom:zerocode";

/// The name Claude Code's `/theme` list shows it by.
pub const NAME: &str = "ZeroCode";

/// The first Claude Code that loads a custom theme. Before it a `theme`
/// naming one is ignored, or — 1.0.49 to 1.0.66 — voids the whole settings
/// file (Orca's measured `CLAUDE_CUSTOM_THEME_FIRST_VERSION`).
pub const FIRST_VERSION: [u32; 3] = [2, 1, 119];

/// Whether a `claude --version` answer is of a Claude Code that loads a
/// custom theme. An answer that names no version is not: an unread version is
/// not permission to write a setting an older CLI chokes on.
///
/// The version is the first word shaped `1.2.3` — `2.1.321 (Claude Code)` —
/// and not the first three numbers of the line: a date in a warning printed
/// ahead of it is not a version.
#[must_use]
pub fn loads_custom_themes(version: &str) -> bool {
    version
        .split_whitespace()
        .find_map(dotted_triple)
        .is_some_and(|found| found >= FIRST_VERSION)
}

/// `1.2.3` at the head of a word — a `v` allowed before it, whatever follows
/// the third number ignored (`2.1.119-beta.1`).
fn dotted_triple(word: &str) -> Option<[u32; 3]> {
    let number = |digits: &str| {
        (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| digits.parse::<u32>().ok())
            .flatten()
    };
    let mut parts = word.strip_prefix('v').unwrap_or(word).splitn(3, '.');
    let major = number(parts.next()?)?;
    let minor = number(parts.next()?)?;
    let rest = parts.next()?;
    let end = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len());
    Some([major, minor, number(&rest[..end])?])
}

/// The themes a person did not pick on purpose, which the window's may stand
/// in for: Claude Code's default pair and `auto`. A colour-blind or ANSI-only
/// theme, or a custom one of the person's own, was chosen and is left alone
/// (Orca's `REPLACEABLE_THEMES`).
pub const REPLACEABLE_THEMES: [&str; 3] = ["dark", "light", "auto"];

/// One colour, as channels that may sit between whole steps while it is being
/// mixed.
type Rgb = [f64; 3];

/// The terminal palette a theme is made from: what the window paints text and
/// its sixteen named colours with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    /// The sixteen named colours, in ANSI order: black, red, green, yellow,
    /// blue, magenta, cyan, white, then their bright forms.
    pub ansi: [[u8; 3]; 16],
}

/// A Claude Code custom theme file's contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    /// One of Claude Code's built-in theme names.
    pub base: String,
    /// Claude Code's own colour keys, each `#rrggbb`.
    pub overrides: BTreeMap<String, String>,
}

impl Theme {
    /// The file's text: the three keys Claude Code reads, two-space indented
    /// with a closing newline, so the same theme is the same bytes and a pass
    /// that changes nothing writes nothing.
    #[must_use]
    pub fn to_file_text(&self) -> String {
        let value = serde_json::json!({
            "name": self.name,
            "base": self.base,
            "overrides": self.overrides,
        });
        let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
        text.push('\n');
        text
    }

    /// The look the window's theme file takes when the window lets go of it,
    /// so a session still running shows what was there before: the replaced
    /// built-in theme with no overrides — `auto` as the terminal's own base,
    /// and no theme at all as Claude Code's default, dark (Orca's
    /// `releasedTheme`).
    #[must_use]
    pub fn released(previous: Option<&str>, terminal_base: &str) -> Self {
        let base = match previous {
            None => "dark",
            Some("auto") => terminal_base,
            Some(named) => named,
        };
        Self {
            name: NAME.to_string(),
            base: base.to_string(),
            overrides: BTreeMap::new(),
        }
    }
}

const MIN_TEXT_CONTRAST: f64 = 4.5;
/// Hues only tint status words and borders (Orca: "the same floor Orca's own
/// theme gate uses").
const MIN_HUE_CONTRAST: f64 = 3.0;
/// A focus ring or a cursor must stand out: the accent's own floor.
const MIN_ACCENT_CONTRAST: f64 = 3.0;

const BLACK: Rgb = [0.0, 0.0, 0.0];
const WHITE: Rgb = [255.0, 255.0, 255.0];

fn rgb(color: [u8; 3]) -> Rgb {
    [
        f64::from(color[0]),
        f64::from(color[1]),
        f64::from(color[2]),
    ]
}

fn luminance(color: Rgb) -> f64 {
    let channel = |value: f64| {
        let c = value / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color[0]) + 0.7152 * channel(color[1]) + 0.0722 * channel(color[2])
}

/// WCAG contrast ratio between two opaque colours.
fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    let (first, second) = (luminance(a), luminance(b));
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

/// `color-mix(in srgb, a percent%, b)`.
fn mix(a: Rgb, b: Rgb, percent: f64) -> Rgb {
    let p = percent / 100.0;
    [
        a[0] * p + b[0] * (1.0 - p),
        a[1] * p + b[1] * (1.0 - p),
        a[2] * p + b[2] * (1.0 - p),
    ]
}

/// Push `color` toward black (on a light screen) or white until it reads on
/// every surface — in twentieths, the first that passes.
fn readable_on(color: Rgb, surfaces: &[Rgb], light: bool, minimum: f64) -> Rgb {
    let extreme = if light { BLACK } else { WHITE };
    for step in 0..=20 {
        let candidate = mix(extreme, color, f64::from(step) / 20.0 * 100.0);
        if surfaces
            .iter()
            .all(|surface| contrast_ratio(candidate, *surface) >= minimum)
        {
            return candidate;
        }
    }
    extreme
}

fn hex(color: Rgb) -> String {
    let channel = |value: f64| {
        // Clamped to a byte before the cast, as `rgbaToHex` rounds and clamps.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let byte = value.round().clamp(0.0, 255.0) as u8;
        byte
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color[0]),
        channel(color[1]),
        channel(color[2])
    )
}

/// Whether a background is a light one: black reads on it at least as well
/// as white does (`isTerminalBackgroundLight`).
#[must_use]
pub fn background_is_light(background: [u8; 3]) -> bool {
    let background = rgb(background);
    contrast_ratio(BLACK, background) >= contrast_ratio(WHITE, background)
}

/// The built-in theme a palette's background calls for.
#[must_use]
pub fn base_of(background: [u8; 3]) -> &'static str {
    if background_is_light(background) {
        "light"
    } else {
        "dark"
    }
}

/// Claude Code's theme for `palette`.
#[must_use]
pub fn theme_from_terminal(palette: &Palette) -> Theme {
    const RED: usize = 1;
    const GREEN: usize = 2;
    const YELLOW: usize = 3;
    const BLUE: usize = 4;
    const MAGENTA: usize = 5;
    const CYAN: usize = 6;
    const BRIGHT_BLUE: usize = 12;
    const BRIGHT_MAGENTA: usize = 13;
    const BRIGHT_CYAN: usize = 14;

    let background = rgb(palette.background);
    let foreground = rgb(palette.foreground);
    let color = |index: usize| rgb(palette.ansi[index]);
    let light = background_is_light(palette.background);
    let tint = |hue: Rgb, percent: f64| mix(hue, background, percent);

    let surfaces: [(&str, Rgb); 11] = [
        ("diffAdded", tint(color(GREEN), 15.0)),
        ("diffRemoved", tint(color(RED), 15.0)),
        ("diffAddedDimmed", tint(color(GREEN), 8.0)),
        ("diffRemovedDimmed", tint(color(RED), 8.0)),
        ("diffAddedWord", tint(color(GREEN), 32.0)),
        ("diffRemovedWord", tint(color(RED), 32.0)),
        ("userMessageBackground", mix(foreground, background, 6.0)),
        (
            "userMessageBackgroundHover",
            mix(foreground, background, 10.0),
        ),
        (
            "bashMessageBackgroundColor",
            mix(foreground, background, 6.0),
        ),
        ("memoryBackgroundColor", mix(foreground, background, 6.0)),
        ("rate_limit_empty", mix(foreground, background, 20.0)),
    ];
    let mut text_surfaces = vec![background];
    text_surfaces.extend(surfaces.iter().map(|(_, surface)| *surface));
    let text = readable_on(foreground, &text_surfaces, light, MIN_TEXT_CONTRAST);
    let readable = |value: Rgb| readable_on(value, &[background], light, MIN_HUE_CONTRAST);
    let readable_text = |value: Rgb| readable_on(value, &[background], light, MIN_TEXT_CONTRAST);
    // Blue, else bright blue, when it stands out on the background.
    let accent = [color(BLUE), color(BRIGHT_BLUE)]
        .into_iter()
        .find(|candidate| contrast_ratio(*candidate, background) >= MIN_ACCENT_CONTRAST)
        .map(readable);
    let shimmer = |value: Rgb| readable(mix(text, value, 35.0));
    let yellow = readable(color(YELLOW));
    let blue = readable(color(BLUE));
    let cyan = readable(color(CYAN));

    let mut overrides: BTreeMap<String, String> = BTreeMap::new();
    let mut set = |key: &str, value: Rgb| {
        overrides.insert(key.to_string(), hex(value));
    };
    if let Some(accent) = accent {
        set("claude", accent);
        set("claudeShimmer", shimmer(accent));
        set("promptBorder", accent);
        set("promptBorderShimmer", shimmer(accent));
        set("rate_limit_fill", accent);
        set("briefLabelClaude", accent);
    }
    set("permission", blue);
    set("permissionShimmer", shimmer(blue));
    set("suggestion", cyan);
    set("planMode", cyan);
    set("ide", readable(color(BRIGHT_CYAN)));
    set("remember", yellow);
    set("warning", yellow);
    set("warningShimmer", shimmer(yellow));
    set("autoAccept", yellow);
    set("briefLabelYou", yellow);
    set("success", readable(color(GREEN)));
    set("error", readable(color(RED)));
    set("merged", readable(color(MAGENTA)));
    set("bashBorder", readable(color(BRIGHT_MAGENTA)));
    set("text", text);
    set("inverseText", background);
    set("inactive", readable_text(mix(foreground, background, 60.0)));
    set(
        "inactiveShimmer",
        readable_text(mix(foreground, background, 75.0)),
    );
    set("subtle", readable(mix(foreground, background, 35.0)));
    for (key, surface) in surfaces {
        set(key, surface);
    }
    Theme {
        name: NAME.to_string(),
        base: base_of(palette.background).to_string(),
        overrides,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokyo Night, as a terminal theme writes it.
    fn tokyo_night() -> Palette {
        Palette {
            foreground: [0xc0, 0xca, 0xf5],
            background: [0x1a, 0x1b, 0x26],
            ansi: [
                [0x15, 0x16, 0x1e],
                [0xf7, 0x76, 0x8e],
                [0x9e, 0xce, 0x6a],
                [0xe0, 0xaf, 0x68],
                [0x7a, 0xa2, 0xf7],
                [0xbb, 0x9a, 0xf7],
                [0x7d, 0xcf, 0xff],
                [0xa9, 0xb1, 0xd6],
                [0x41, 0x48, 0x68],
                [0xf7, 0x76, 0x8e],
                [0x9e, 0xce, 0x6a],
                [0xe0, 0xaf, 0x68],
                [0x7a, 0xa2, 0xf7],
                [0xbb, 0x9a, 0xf7],
                [0x7d, 0xcf, 0xff],
                [0xc0, 0xca, 0xf5],
            ],
        }
    }

    /// Solarized Light: the low-contrast palette the "keep the hue" rule is
    /// for.
    fn solarized_light() -> Palette {
        Palette {
            foreground: [0x65, 0x7b, 0x83],
            background: [0xfd, 0xf6, 0xe3],
            ansi: [
                [0x07, 0x36, 0x42],
                [0xdc, 0x32, 0x2f],
                [0x85, 0x99, 0x00],
                [0xb5, 0x89, 0x00],
                [0x26, 0x8b, 0xd2],
                [0xd3, 0x36, 0x82],
                [0x2a, 0xa1, 0x98],
                [0xee, 0xe8, 0xd5],
                [0x00, 0x2b, 0x36],
                [0xcb, 0x4b, 0x16],
                [0x58, 0x6e, 0x75],
                [0x65, 0x7b, 0x83],
                [0x83, 0x94, 0x96],
                [0x6c, 0x71, 0xc4],
                [0x93, 0xa1, 0xa1],
                [0xfd, 0xf6, 0xe3],
            ],
        }
    }

    fn parsed(hex: &str) -> Rgb {
        let channel = |at: usize| f64::from(u8::from_str_radix(&hex[at..at + 2], 16).expect("hex"));
        [channel(1), channel(3), channel(5)]
    }

    /// The theme takes its hues from the palette's own named colours, its
    /// base from the background, and its accent from the palette's blue —
    /// which on Tokyo Night already reads, so it is the palette's blue to the
    /// digit (Orca's own test theme: `claude: '#7aa2f7'`).
    #[test]
    fn a_dark_palette_lends_its_own_colours() {
        let theme = theme_from_terminal(&tokyo_night());
        assert_eq!(
            (theme.name.as_str(), theme.base.as_str()),
            ("ZeroCode", "dark")
        );
        let said = |key: &str| theme.overrides.get(key).map(String::as_str);
        assert_eq!(said("claude"), Some("#7aa2f7"));
        assert_eq!(said("promptBorder"), Some("#7aa2f7"));
        assert_eq!(said("permission"), Some("#7aa2f7"));
        assert_eq!(said("success"), Some("#9ece6a"));
        assert_eq!(said("error"), Some("#f7768e"));
        assert_eq!(said("warning"), Some("#e0af68"));
        assert_eq!(said("suggestion"), Some("#7dcfff"));
        assert_eq!(said("merged"), Some("#bb9af7"));
        assert_eq!(said("text"), Some("#c0caf5"));
        assert_eq!(said("inverseText"), Some("#1a1b26"));
        // A surface is its hue tinted into the background, nearer the
        // background than the hue.
        let background = rgb(tokyo_night().background);
        let added = parsed(said("diffAdded").expect("a diff surface"));
        let green = rgb(tokyo_night().ansi[2]);
        assert!(contrast_ratio(added, background) < contrast_ratio(green, background));
        assert!(added[1] > background[1], "the tint leans green: {added:?}");
        // Every key is a `#rrggbb`, and none is one Claude Code does not have.
        assert!(theme.overrides.values().all(|value| {
            value.len() == 7
                && value.starts_with('#')
                && value[1..].chars().all(|ch| ch.is_ascii_hexdigit())
        }));
        assert_eq!(theme.overrides.len(), 36, "{:?}", theme.overrides.keys());
    }

    /// On a light, low-contrast palette every colour that carries words reads
    /// — text at 4.5:1 on the background and on every surface it is drawn
    /// over, status hues at 3:1 — and each keeps the side of the colour wheel
    /// it came from.
    #[test]
    fn a_light_low_contrast_palette_is_pushed_only_as_far_as_reading_takes() {
        let palette = solarized_light();
        let theme = theme_from_terminal(&palette);
        assert_eq!(theme.base, "light");
        let background = rgb(palette.background);
        let said = |key: &str| parsed(theme.overrides.get(key).expect(key));
        let text = said("text");
        for surface in [
            "diffAdded",
            "diffRemoved",
            "diffAddedWord",
            "diffRemovedWord",
            "userMessageBackground",
            "rate_limit_empty",
        ] {
            assert!(
                contrast_ratio(text, said(surface)) >= MIN_TEXT_CONTRAST - 0.05,
                "text on {surface}: {}",
                contrast_ratio(text, said(surface))
            );
        }
        assert!(contrast_ratio(text, background) >= MIN_TEXT_CONTRAST - 0.05);
        for hue in [
            "claude",
            "permission",
            "success",
            "error",
            "warning",
            "suggestion",
            "merged",
        ] {
            assert!(
                contrast_ratio(said(hue), background) >= MIN_HUE_CONTRAST - 0.05,
                "{hue}: {}",
                contrast_ratio(said(hue), background)
            );
        }
        // Solarized's yellow does not read at 3:1 on its own base3; it is
        // darkened, and still more red-and-green than blue.
        let warning = said("warning");
        assert_ne!(theme.overrides["warning"], "#b58900");
        assert!(
            warning[0] > warning[2] && warning[1] > warning[2],
            "{warning:?}"
        );
        // Its red already reads and is the palette's own.
        assert_eq!(theme.overrides["error"], "#dc322f");
    }

    /// A palette whose blues do not stand out names no accent, and Claude
    /// Code keeps its own for the keys an accent would have set.
    #[test]
    fn a_palette_with_no_blue_that_stands_out_leaves_claudes_own_accent() {
        let mut palette = tokyo_night();
        palette.ansi[4] = [0x1c, 0x1d, 0x2a];
        palette.ansi[12] = [0x20, 0x21, 0x2e];
        let theme = theme_from_terminal(&palette);
        for key in [
            "claude",
            "claudeShimmer",
            "promptBorder",
            "promptBorderShimmer",
            "rate_limit_fill",
            "briefLabelClaude",
        ] {
            assert!(!theme.overrides.contains_key(key), "{key}");
        }
        assert!(theme.overrides.contains_key("permission"));
        assert_eq!(theme.overrides.len(), 30);
    }

    /// The file is the three keys Claude Code reads, the same bytes for the
    /// same theme; the released file is the replaced built-in with nothing
    /// over it.
    #[test]
    fn the_file_is_stable_and_a_released_one_is_the_theme_it_replaced() {
        let theme = theme_from_terminal(&tokyo_night());
        let text = theme.to_file_text();
        assert_eq!(text, theme_from_terminal(&tokyo_night()).to_file_text());
        assert!(text.ends_with("}\n"));
        let read: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(read["name"], "ZeroCode");
        assert_eq!(read["base"], "dark");
        assert_eq!(read["overrides"]["claude"], "#7aa2f7");
        assert_eq!(
            read.as_object().map(|keys| keys.len()),
            Some(3),
            "only what Claude Code reads"
        );

        for (previous, terminal, base) in [
            (None, "light", "dark"),
            (Some("auto"), "light", "light"),
            (Some("auto"), "dark", "dark"),
            (Some("light"), "dark", "light"),
            (Some("dark"), "light", "dark"),
        ] {
            let released = Theme::released(previous, terminal);
            assert_eq!(released.base, base, "{previous:?} on {terminal}");
            assert!(released.overrides.is_empty());
        }
        assert_eq!(base_of([0xfd, 0xf6, 0xe3]), "light");
        assert_eq!(base_of([0x1a, 0x1b, 0x26]), "dark");
    }

    /// Only a Claude Code that loads custom themes is handed one: 2.1.119 and
    /// later, read off whatever `claude --version` prints.
    #[test]
    fn only_a_claude_code_that_loads_custom_themes_is_handed_one() {
        for yes in [
            "2.1.119",
            "2.1.321 (Claude Code)",
            "2.2.0",
            "3.0.0",
            "claude 2.1.119-beta.1",
            "v2.1.119",
        ] {
            assert!(loads_custom_themes(yes), "{yes}");
        }
        for no in [
            "2.1.118 (Claude Code)",
            "1.0.66",
            "2.0.999",
            "",
            "unknown",
            "2.1",
            "2.1.",
            // Three numbers that are not a version, and a version that is
            // too old said before one that is not.
            "2026-11-10 09:00:01 warning",
            "2026 11 10",
            "2.0.5 (was 3.0.0)",
        ] {
            assert!(!loads_custom_themes(no), "{no}");
        }
        assert_eq!(SETTING, format!("custom:{SLUG}"));
        assert!(
            REPLACEABLE_THEMES.contains(&"auto")
                && !REPLACEABLE_THEMES.contains(&"dark-daltonized")
        );
    }
}
