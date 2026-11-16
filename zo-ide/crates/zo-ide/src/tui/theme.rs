//! Interactive TUI colour themes (`terminal` + built-in 30 terminal palettes).

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use serde::de::{MapAccess, Visitor};
use serde::Deserialize;
use two_face::theme::EmbeddedThemeName;

use crate::preferences::Preferences;

use super::ansi::Role;
use super::palette::{contrast_ratio, is_light_background, TerminalPalette};

/// `ZO_THEME`: overrides `"theme"` from `settings.json` for one run.
pub const THEME_ENV: &str = "ZO_THEME";
/// Built-in default theme slug: byte-identical to pre-theme rendering.
pub const TERMINAL_THEME_NAME: &str = "terminal";
/// Legacy `theme` values mapped silently to `terminal` when no theme of that
/// name exists.
const TERMINAL_ALIASES: [&str; 5] = ["dark", "light", "system", "auto", "default"];

const TERMINAL_THEMES_JSON: &str =
    include_str!("../../../../../crates/zerocode-shell/src/terminal_themes.json");

/// A TUI colour theme: 16 ANSI slots, seven semantic role overrides, and an
/// optional syntax theme name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub label: String,
    pub ansi: [Option<(u8, u8, u8)>; 16],
    pub accent: Option<(u8, u8, u8)>,
    pub code: Option<(u8, u8, u8)>,
    pub link: Option<(u8, u8, u8)>,
    pub heading: Option<(u8, u8, u8)>,
    pub caret: Option<(u8, u8, u8)>,
    pub model: Option<(u8, u8, u8)>,
    pub cwd: Option<(u8, u8, u8)>,
    pub warning: Option<(u8, u8, u8)>,
    pub syntax: Option<String>,
    pub foreground: Option<(u8, u8, u8)>,
    pub background: Option<(u8, u8, u8)>,
}

impl Theme {
    /// The default `terminal` theme — leaves all terminal colours untouched.
    #[must_use]
    pub fn terminal() -> Self {
        Self::empty(TERMINAL_THEME_NAME, "Terminal")
    }

    /// An empty theme with `name` and `label`, ready for field overrides.
    #[must_use]
    pub fn empty(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            ansi: [None; 16],
            accent: None,
            code: None,
            link: None,
            heading: None,
            caret: None,
            model: None,
            cwd: None,
            warning: None,
            syntax: None,
            foreground: None,
            background: None,
        }
    }

    /// True when this is the default `terminal` theme.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.name == TERMINAL_THEME_NAME
    }

    /// Theme RGB colour for `role`, falling back through the theme's 16-colour
    /// table when no explicit role override is set.
    #[must_use]
    pub fn role_color(&self, role: Role) -> Option<(u8, u8, u8)> {
        match role {
            Role::Code => self.code.or(self.ansi[2]),
            Role::Link => self.link.or(self.ansi[6]),
            Role::Heading => self.heading.or(self.accent).or(self.ansi[4]),
            Role::Caret => self.caret.or(self.ansi[5]),
            Role::Model => self.model.or(self.accent).or(self.ansi[4]),
            Role::Cwd => self.cwd.or(self.ansi[6]),
            Role::Warning => self.warning.or(self.ansi[3]),
        }
    }

    /// Theme RGB colour for ANSI slot `0..=15`, or `None` when the terminal's
    /// own slot is kept (`0, 7, 8, 15`, or unset).
    #[must_use]
    pub fn ansi_slot(&self, slot: u8) -> Option<(u8, u8, u8)> {
        match slot {
            1..=5 | 9..=13 => self.ansi[usize::from(slot)],
            6 => self.accent.or(self.ansi[4]),
            14 => self.accent.or(self.ansi[12]),
            _ => None,
        }
    }

    /// `two_face` syntax theme for this theme.
    #[must_use]
    pub fn syntax_theme(&self, light_background: bool) -> EmbeddedThemeName {
        let key = self.syntax.as_deref().unwrap_or(&self.name);
        syntax_theme_for_name(key).unwrap_or(if light_background {
            EmbeddedThemeName::CatppuccinLatte
        } else {
            EmbeddedThemeName::CatppuccinMocha
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTerminalTheme {
    foreground: Option<String>,
    background: Option<String>,
    black: Option<String>,
    red: Option<String>,
    green: Option<String>,
    yellow: Option<String>,
    blue: Option<String>,
    magenta: Option<String>,
    cyan: Option<String>,
    white: Option<String>,
    bright_black: Option<String>,
    bright_red: Option<String>,
    bright_green: Option<String>,
    bright_yellow: Option<String>,
    bright_blue: Option<String>,
    bright_magenta: Option<String>,
    bright_cyan: Option<String>,
    bright_white: Option<String>,
}

struct OrderedThemes(Vec<(String, RawTerminalTheme)>);

impl<'de> Deserialize<'de> for OrderedThemes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderedVisitor;
        impl<'de> Visitor<'de> for OrderedVisitor {
            type Value = OrderedThemes;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a map of terminal theme definitions")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(30));
                while let Some((key, value)) = map.next_entry::<String, RawTerminalTheme>()? {
                    entries.push((key, value));
                }
                Ok(OrderedThemes(entries))
            }
        }
        deserializer.deserialize_map(OrderedVisitor)
    }
}

fn builtin_themes() -> &'static [Theme] {
    static BUILTIN: OnceLock<Vec<Theme>> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        let parsed: OrderedThemes =
            serde_json::from_str(TERMINAL_THEMES_JSON).expect("valid terminal_themes.json");
        parsed
            .0
            .into_iter()
            .map(|(label, raw)| {
                let slug = label.to_ascii_lowercase().replace(' ', "-");
                let mut theme = Theme::empty(slug, label);
                let hex = |opt: Option<String>| opt.as_deref().and_then(parse_hex_color);
                theme.foreground = hex(raw.foreground);
                theme.background = hex(raw.background);
                theme.ansi = [
                    hex(raw.black),
                    hex(raw.red),
                    hex(raw.green),
                    hex(raw.yellow),
                    hex(raw.blue),
                    hex(raw.magenta),
                    hex(raw.cyan),
                    hex(raw.white),
                    hex(raw.bright_black),
                    hex(raw.bright_red),
                    hex(raw.bright_green),
                    hex(raw.bright_yellow),
                    hex(raw.bright_blue),
                    hex(raw.bright_magenta),
                    hex(raw.bright_cyan),
                    hex(raw.bright_white),
                ];
                theme
            })
            .collect()
    })
}

/// Maps a theme slug or `two_face` name to an embedded syntax theme.
#[must_use]
pub fn syntax_theme_for_name(name: &str) -> Option<EmbeddedThemeName> {
    match name {
        "dracula" => Some(EmbeddedThemeName::Dracula),
        "catppuccin-mocha" => Some(EmbeddedThemeName::CatppuccinMocha),
        "catppuccin-latte" => Some(EmbeddedThemeName::CatppuccinLatte),
        "nord" => Some(EmbeddedThemeName::Nord),
        "gruvbox-dark" => Some(EmbeddedThemeName::GruvboxDark),
        "gruvbox-light" => Some(EmbeddedThemeName::GruvboxLight),
        "solarized-dark" => Some(EmbeddedThemeName::SolarizedDark),
        "solarized-light" => Some(EmbeddedThemeName::SolarizedLight),
        "one-dark" => Some(EmbeddedThemeName::OneHalfDark),
        "one-light" => Some(EmbeddedThemeName::OneHalfLight),
        "github-light" => Some(EmbeddedThemeName::Github),
        "monokai" => Some(EmbeddedThemeName::MonokaiExtended),
        "snazzy" => Some(EmbeddedThemeName::SublimeSnazzy),
        other => two_face::theme::EmbeddedLazyThemeSet::theme_names()
            .iter()
            .copied()
            .find(|candidate| candidate.as_name().eq_ignore_ascii_case(other)),
    }
}

static ACTIVE_THEME: RwLock<Option<Theme>> = RwLock::new(None);
static IS_TERMINAL: AtomicBool = AtomicBool::new(true);
static THEME_REVISION: AtomicU64 = AtomicU64::new(0);

/// Returns true if `slug` matches `[a-z0-9][a-z0-9-]{0,63}`.
#[must_use]
pub fn is_valid_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    (1..=64).contains(&bytes.len())
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes[1..]
            .iter()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

/// Parses `#rgb` or `#rrggbb` into an RGB tuple.
#[must_use]
pub fn parse_hex_color(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    // Hex digits only: `from_str_radix` also takes a leading `+`, and a
    // multi-byte character would split a slice below.
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 => {
            let red = u8::from_str_radix(&hex[0..1], 16).ok()?;
            let green = u8::from_str_radix(&hex[1..2], 16).ok()?;
            let blue = u8::from_str_radix(&hex[2..3], 16).ok()?;
            Some((red * 17, green * 17, blue * 17))
        }
        6 => {
            let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((red, green, blue))
        }
        _ => None,
    }
}

/// Every theme slug: `terminal` first, the 30 built-in themes, then the
/// person's own themes from `<zo home>/themes` by name — the folder is read
/// again on each call, so a file written since the last call is listed.
#[must_use]
pub fn names() -> Vec<String> {
    super::theme_files::names()
}

/// Looks up a theme by slug: `terminal`, else the person's own theme of that
/// name, else the built-in one.
#[must_use]
pub fn find(slug: &str) -> Option<Theme> {
    super::theme_files::find(slug)
}

/// The built-in theme slugs: `terminal` first, followed by the 30 terminal themes.
#[must_use]
pub(crate) fn builtin_names() -> Vec<&'static str> {
    let builtins = builtin_themes();
    let mut list = Vec::with_capacity(1 + builtins.len());
    list.push(TERMINAL_THEME_NAME);
    list.extend(builtins.iter().map(|theme| theme.name.as_str()));
    list
}

/// Looks up a built-in theme (`terminal` or one of the 30) by slug.
#[must_use]
pub(crate) fn find_builtin(slug: &str) -> Option<Theme> {
    if !is_valid_slug(slug) {
        return None;
    }
    if slug == TERMINAL_THEME_NAME {
        return Some(Theme::terminal());
    }
    builtin_themes()
        .iter()
        .find(|theme| theme.name == slug)
        .cloned()
}

/// Returns the active theme for this process.
#[must_use]
pub fn active() -> Theme {
    let guard = ACTIVE_THEME
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clone().unwrap_or_else(Theme::terminal)
}

/// Fast lock-free check whether the active theme is `terminal`.
#[must_use]
pub fn is_terminal() -> bool {
    IS_TERMINAL.load(Ordering::Acquire)
}

/// Runs `f` against the active theme without cloning it.
pub fn with_active<R>(f: impl FnOnce(&Theme) -> R) -> R {
    let guard = ACTIVE_THEME
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match guard.as_ref() {
        Some(theme) => f(theme),
        None => f(&Theme::terminal()),
    }
}

/// Sets the active theme; returns `true` when the active theme changed.
pub fn set(theme: Theme) -> bool {
    let mut guard = ACTIVE_THEME
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unchanged = match guard.as_ref() {
        None => theme.is_terminal(),
        Some(current) => current == &theme,
    };
    if unchanged {
        return false;
    }
    IS_TERMINAL.store(theme.is_terminal(), Ordering::Release);
    *guard = if theme.is_terminal() {
        None
    } else {
        Some(theme)
    };
    THEME_REVISION.fetch_add(1, Ordering::AcqRel);
    true
}

/// Monotonic revision bumped whenever [`set`] changes the active theme.
#[must_use]
pub fn revision() -> u64 {
    THEME_REVISION.load(Ordering::Acquire)
}

/// Chooses the startup theme from `env_theme` (`ZO_THEME`) then `preferences.theme`,
/// returning a warning message if an explicit value was invalid or unknown.
#[must_use]
pub fn choose(preferences: &Preferences, env_theme: Option<&str>) -> (Theme, Option<String>) {
    let Some(name) = env_theme.or(preferences.theme.as_deref()) else {
        return (Theme::terminal(), None);
    };
    if !is_valid_slug(name) {
        return (
            Theme::terminal(),
            Some(format!("invalid theme name {name:?}; using terminal")),
        );
    }
    if let Some(theme) = find(name) {
        return (theme, None);
    }
    if TERMINAL_ALIASES.contains(&name) {
        return (Theme::terminal(), None);
    }
    (
        Theme::terminal(),
        Some(format!("unknown theme {name:?}; using terminal")),
    )
}

/// Initializes the process theme from `ZO_THEME` and `preferences`, queuing any
/// startup warning.
pub fn init(preferences: &Preferences) {
    super::theme_files::read_at_start();
    let env_theme = std::env::var(THEME_ENV).ok();
    let (theme, warning) = choose(preferences, env_theme.as_deref());
    let _ = set(theme);
    if let Some(warning) = warning {
        crate::runtime_support::push_startup_warning(warning);
    }
}

/// Converts `(red, green, blue)` to HSL `(hue_deg in 0..360, sat in 0..=1, light in 0..=1)`.
#[must_use]
pub(crate) fn rgb_to_hsl((red, green, blue): (u8, u8, u8)) -> (f64, f64, f64) {
    let red_f = f64::from(red) / 255.0;
    let green_f = f64::from(green) / 255.0;
    let blue_f = f64::from(blue) / 255.0;
    let max_c = red_f.max(green_f).max(blue_f);
    let min_c = red_f.min(green_f).min(blue_f);
    let lightness = f64::midpoint(max_c, min_c);
    let delta = max_c - min_c;
    if delta == 0.0 {
        return (0.0, 0.0, lightness);
    }
    let saturation = if lightness > 0.5 {
        delta / (2.0 - max_c - min_c)
    } else {
        delta / (max_c + min_c)
    };
    let hue = if (max_c - red_f).abs() < f64::EPSILON {
        let shift = if green_f < blue_f { 6.0 } else { 0.0 };
        ((green_f - blue_f) / delta + shift) * 60.0
    } else if (max_c - green_f).abs() < f64::EPSILON {
        ((blue_f - red_f) / delta + 2.0) * 60.0
    } else {
        ((red_f - green_f) / delta + 4.0) * 60.0
    };
    (hue.rem_euclid(360.0), saturation, lightness)
}

fn unit_to_u8(value: f64) -> u8 {
    let scaled = (value.clamp(0.0, 1.0) * 255.0).round();
    for candidate in 0u8..=255 {
        if f64::from(candidate) >= scaled - 0.5 {
            return candidate;
        }
    }
    255
}

/// Converts HSL `(hue_deg, sat in 0..=1, light in 0..=1)` back to `(red, green, blue)`.
#[must_use]
pub(crate) fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (u8, u8, u8) {
    if saturation <= 0.0 {
        let gray = unit_to_u8(lightness);
        return (gray, gray, gray);
    }
    let upper = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let lower = 2.0 * lightness - upper;
    let hue_turn = hue.rem_euclid(360.0) / 360.0;
    let channel = |offset: f64| {
        let shifted = (hue_turn + offset).rem_euclid(1.0);
        let value = if shifted < 1.0 / 6.0 {
            lower + (upper - lower) * 6.0 * shifted
        } else if shifted < 0.5 {
            upper
        } else if shifted < 2.0 / 3.0 {
            lower + (upper - lower) * (2.0 / 3.0 - shifted) * 6.0
        } else {
            lower
        };
        unit_to_u8(value)
    };
    (
        channel(1.0 / 3.0),
        channel(0.0),
        channel(-1.0 / 3.0),
    )
}

/// Adjusts `color`'s HSL lightness in 1%p steps away from `background` until
/// contrast clears `palette.text_contrast_floor(background)`.
#[must_use]
pub fn fit_lightness(
    color: (u8, u8, u8),
    palette: TerminalPalette,
    background: (u8, u8, u8),
) -> (u8, u8, u8) {
    let floor = palette.text_contrast_floor(background);
    if contrast_ratio(color, background) >= floor {
        return color;
    }
    let light = is_light_background(background);
    let (hue, saturation, lightness) = rgb_to_hsl(color);
    for step in 1..=100 {
        let next_lightness = if light {
            (lightness - f64::from(step) * 0.01).max(0.0)
        } else {
            (lightness + f64::from(step) * 0.01).min(1.0)
        };
        let candidate = hsl_to_rgb(hue, saturation, next_lightness);
        if contrast_ratio(candidate, background) >= floor {
            return candidate;
        }
        if (light && next_lightness <= 0.0) || (!light && next_lightness >= 1.0) {
            break;
        }
    }
    if light {
        (0, 0, 0)
    } else {
        (255, 255, 255)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        choose, find, fit_lightness, hsl_to_rgb, is_valid_slug, names, parse_hex_color,
        rgb_to_hsl,
    };
    use crate::preferences::Preferences;
    use crate::tui::palette::{contrast_ratio, ColorLevel, TerminalPalette};

    fn hue_diff(left: f64, right: f64) -> f64 {
        let delta = (left - right).abs() % 360.0;
        delta.min(360.0 - delta)
    }

    #[test]
    fn lightness_fit_keeps_hue_and_meets_the_floor() {
        let colors = [
            parse_hex_color("#bd93f9").unwrap(),
            parse_hex_color("#ff79c6").unwrap(),
            parse_hex_color("#50fa7b").unwrap(),
            parse_hex_color("#8be9fd").unwrap(),
            parse_hex_color("#ff5555").unwrap(),
            parse_hex_color("#ffb86c").unwrap(),
            parse_hex_color("#f1fa8c").unwrap(),
        ];
        let grounds = [
            ((40, 42, 54), (248, 248, 242)),
            ((28, 30, 38), (224, 224, 224)),
            ((255, 255, 255), (46, 52, 52)),
            ((253, 246, 227), (101, 123, 131)),
        ];
        for (bg, fg) in grounds {
            let palette = TerminalPalette::new(fg, bg, ColorLevel::TrueColor);
            let floor = palette.text_contrast_floor(bg);
            let is_light = u32::from(bg.0) * 299 + u32::from(bg.1) * 587 + u32::from(bg.2) * 114
                > 128_000;
            for orig in colors {
                let fitted = fit_lightness(orig, palette, bg);
                assert!(
                    contrast_ratio(fitted, bg) >= floor,
                    "fitted {fitted:?} below floor {floor:.2} on {bg:?}"
                );
                let (h0, s0, l0) = rgb_to_hsl(orig);
                let (h1, s1, l1) = rgb_to_hsl(fitted);
                assert!(
                    hue_diff(h0, h1) <= 3.0,
                    "hue drifted for {orig:?} -> {fitted:?} on {bg:?}: {h0:.2} vs {h1:.2}"
                );
                assert!(
                    (s0 - s1).abs() <= 0.05,
                    "sat drifted for {orig:?} -> {fitted:?} on {bg:?}: {s0:.3} vs {s1:.3}"
                );
                if contrast_ratio(orig, bg) >= floor {
                    assert_eq!(fitted, orig, "compliant colour must stay unchanged");
                } else {
                    let prev_l = if is_light {
                        (l1 + 0.01).min(l0)
                    } else {
                        (l1 - 0.01).max(l0)
                    };
                    let prev_rgb = hsl_to_rgb(h0, s0, prev_l);
                    assert!(
                        contrast_ratio(prev_rgb, bg) < floor,
                        "one step less moved ({prev_rgb:?}) must be below floor {floor:.2}"
                    );
                }
            }
        }
    }

    #[test]
    fn builtin_themes_are_terminal_plus_the_windows_thirty() {
        let list = names();
        assert_eq!(list.len(), 31, "expected 31 themes: {list:?}");
        assert_eq!(list[0], "terminal");
        for slug in &list {
            assert!(is_valid_slug(slug), "invalid slug {slug:?}");
        }
        let dracula = find("dracula").expect("dracula builtin");
        assert_eq!(dracula.ansi[1], Some((0xff, 0x55, 0x55)));
    }

    #[test]
    fn theme_name_is_a_slug_and_an_unknown_one_falls_back_with_a_warning() {
        let default = Preferences::default();
        let (theme, warn) = choose(&default, None);
        assert!(theme.is_terminal());
        assert_eq!(warn, None);

        let dracula_pref = Preferences {
            theme: Some("dracula".to_string()),
            ..Preferences::default()
        };
        let (theme, warn) = choose(&dracula_pref, None);
        assert_eq!(theme.name, "dracula");
        assert_eq!(warn, None);

        // ZO_THEME overrides preferences.
        let (theme, warn) = choose(&dracula_pref, Some("nord"));
        assert_eq!(theme.name, "nord");
        assert_eq!(warn, None);

        // Invalid slugs and unknown names fall back to terminal with distinct warnings,
        // and `find` rejects non-slugs directly.
        let invalid = ["../x", "/tmp/a", "", "Dracula"];
        for bad in invalid {
            assert!(find(bad).is_none(), "find({bad:?}) must reject non-slug");
            let (theme, warn) = choose(&dracula_pref, Some(bad));
            assert!(theme.is_terminal(), "{bad:?} should fall back to terminal");
            assert!(
                warn.as_deref().is_some_and(|msg| msg.contains("invalid theme name")),
                "{bad:?} should warn about invalid theme name: {warn:?}"
            );

            let bad_pref = Preferences {
                theme: Some(bad.to_string()),
                ..Preferences::default()
            };
            let (theme, warn) = choose(&bad_pref, None);
            assert!(theme.is_terminal(), "pref {bad:?} should fall back to terminal");
            assert!(
                warn.as_deref().is_some_and(|msg| msg.contains("invalid theme name")),
                "pref {bad:?} should warn about invalid theme name: {warn:?}"
            );
        }

        assert!(find("no-such-theme").is_none());
        let (theme, warn) = choose(&dracula_pref, Some("no-such-theme"));
        assert!(theme.is_terminal());
        assert!(
            warn.as_deref().is_some_and(|msg| msg.contains("unknown theme")),
            "unknown slug should warn about unknown theme: {warn:?}"
        );
        let unknown_pref = Preferences {
            theme: Some("no-such-theme".to_string()),
            ..Preferences::default()
        };
        let (theme, warn) = choose(&unknown_pref, None);
        assert!(theme.is_terminal());
        assert!(
            warn.as_deref().is_some_and(|msg| msg.contains("unknown theme")),
            "unknown pref slug should warn about unknown theme: {warn:?}"
        );
    }

    #[test]
    fn an_old_light_dark_value_is_terminal_without_a_warning() {
        let default = Preferences::default();
        for alias in ["dark", "light", "system", "auto", "default"] {
            let pref = Preferences {
                theme: Some(alias.to_string()),
                ..Preferences::default()
            };
            let (theme, warn) = choose(&pref, None);
            assert!(theme.is_terminal(), "pref {alias:?} should choose terminal");
            assert_eq!(warn, None, "pref {alias:?} must not warn");

            let (theme, warn) = choose(&default, Some(alias));
            assert!(theme.is_terminal(), "env {alias:?} should choose terminal");
            assert_eq!(warn, None, "env {alias:?} must not warn");
        }

        let unknown_pref = Preferences {
            theme: Some("darkk".to_string()),
            ..Preferences::default()
        };
        for (theme, warn) in [choose(&unknown_pref, None), choose(&default, Some("darkk"))] {
            assert!(theme.is_terminal());
            assert!(
                warn.as_deref().is_some_and(|msg| msg.contains("unknown theme")),
                "\"darkk\" should still warn about unknown theme: {warn:?}"
            );
        }

        let invalid_pref = Preferences {
            theme: Some("Dark".to_string()),
            ..Preferences::default()
        };
        for (theme, warn) in [choose(&invalid_pref, None), choose(&default, Some("Dark"))] {
            assert!(theme.is_terminal());
            assert!(
                warn.as_deref()
                    .is_some_and(|msg| msg.contains("invalid theme name")),
                "\"Dark\" should still warn about invalid theme name: {warn:?}"
            );
        }
    }
}
