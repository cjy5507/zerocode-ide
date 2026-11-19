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
/// Each built-in theme's inline-code colour, by slug: the colour the theme's
/// own sources give a markdown code span, where they give one. A theme not
/// named here draws inline code in its blue slot ([`Theme::role_color`]).
const THEME_INLINE_CODE_JSON: &str = include_str!("theme_inline_code.json");

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
    /// table when no explicit role override is set. Inline code falls back to
    /// the blue slot, as `terminal` draws it ([`Role::terminal_ink`]): the
    /// green one made it one green in nearly every theme (the person,
    /// 2026-11-19: 「녹색으로 전부 똑같아서 theme를 바꿔도」).
    #[must_use]
    pub fn role_color(&self, role: Role) -> Option<(u8, u8, u8)> {
        match role {
            Role::Code => self.code.or(self.ansi[4]),
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

    /// `two_face` syntax theme for this theme: its own — the one its file
    /// names, or the one named like it — else ANSI, whose colours are slots
    /// of the sixteen, so the code is drawn in this theme's colours as its
    /// roles are. Seventeen of the thirty had no syntax theme of their own
    /// and all drew code in one palette, Catppuccin's (the person,
    /// 2026-11-18: 「theme 색상 글씨가 바꿔도 다똑같음」).
    #[must_use]
    pub fn syntax_theme(&self) -> EmbeddedThemeName {
        let key = self.syntax.as_deref().unwrap_or(&self.name);
        syntax_theme_for_name(key).unwrap_or(EmbeddedThemeName::Ansi)
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
        let inline_code: std::collections::BTreeMap<String, String> =
            serde_json::from_str(THEME_INLINE_CODE_JSON).expect("valid theme_inline_code.json");
        parsed
            .0
            .into_iter()
            .map(|(label, raw)| {
                let slug = label.to_ascii_lowercase().replace(' ', "-");
                let code = inline_code.get(&slug).map(String::as_str).and_then(parse_hex_color);
                let mut theme = Theme::empty(slug, label);
                let hex = |opt: Option<String>| opt.as_deref().and_then(parse_hex_color);
                theme.code = code;
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
/// returning a warning message if an explicit value was invalid or unknown —
/// or names a theme whose file was not read, which the warning says, and why.
/// An empty value in either place is no value.
#[must_use]
pub fn choose(preferences: &Preferences, env_theme: Option<&str>) -> (Theme, Option<String>) {
    /// An empty value, or blanks alone, is no choice.
    fn chosen(value: Option<&str>) -> Option<&str> {
        value.filter(|value| !value.trim().is_empty())
    }
    // An empty `ZO_THEME` leaves the choice to the setting, and an empty
    // setting to `terminal`.
    let Some(name) = chosen(env_theme).or_else(|| chosen(preferences.theme.as_deref())) else {
        return (Theme::terminal(), None);
    };
    // The reading `/theme <name>` uses: a visible name names its theme too.
    match resolve_name(name) {
        Ok(theme) => (theme, None),
        Err(ThemeResolveError::Invalid) => (
            Theme::terminal(),
            Some(format!("invalid theme name {name:?}; using terminal")),
        ),
        Err(ThemeResolveError::Unknown) => (
            Theme::terminal(),
            Some(format!("unknown theme {name:?}; using terminal")),
        ),
        Err(ThemeResolveError::PastCap { cap }) => (
            Theme::terminal(),
            Some(format!(
                "{}; using terminal",
                super::strings::theme_past_cap(name, cap)
            )),
        ),
    }
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
    fit_lightness_to_floor(color, background, palette.text_contrast_floor(background))
}

/// Adjusts `color`'s HSL lightness in 1%p steps away from `background` until
/// its contrast against `background` clears `floor` — the rule
/// [`fit_lightness`] follows with the floor a measured body ink gives, and
/// `ground` with the fixed floor it takes when only the ground's lightness is
/// known. Hue and saturation stay; a colour that clears `floor` already is
/// returned as it is.
#[must_use]
pub(crate) fn fit_lightness_to_floor(
    color: (u8, u8, u8),
    background: (u8, u8, u8),
    floor: f64,
) -> (u8, u8, u8) {
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

/// Why [`resolve_name`] took no theme for a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeResolveError {
    /// Empty, or shaped like a path: it holds `/`, `\` or a dot.
    Invalid,
    /// A name no theme has.
    Unknown,
    /// The name of a theme the person has a file for, which was not read:
    /// the themes folder holds more theme files than `cap`, and this one is
    /// past the first `cap` by name.
    PastCap {
        /// The most theme files read from the themes folder.
        cap: usize,
    },
}

/// The theme a person's word names — one reading for the `theme` setting,
/// `ZO_THEME` and `/theme <name>`: a slug (`dracula`), a visible name
/// without case (`Dracula`), or either with spaces where the slug has
/// hyphens (`Tokyo Night`, `solarized dark`). An old `theme` value (`dark`,
/// `light`, `system`, `auto`, `default`, in any case) that names no theme is
/// `terminal`.
///
/// # Errors
/// [`ThemeResolveError::Invalid`] for an empty or path-shaped name, which is
/// never looked up; [`ThemeResolveError::Unknown`] for a name no theme has;
/// [`ThemeResolveError::PastCap`] for one whose file is in the person's
/// themes folder, unread past the cap.
pub fn resolve_name(name: &str) -> Result<Theme, ThemeResolveError> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\', '.']) {
        return Err(ThemeResolveError::Invalid);
    }
    if let Some(theme) = find(name) {
        return Ok(theme);
    }
    // `Tokyo Night`, `solarized dark`, `DRACULA`: the slug the name is spelled as.
    let folded = name.to_lowercase();
    let slug = folded.split_whitespace().collect::<Vec<_>>().join("-");
    if let Some(theme) = find(&slug) {
        return Ok(theme);
    }
    // A theme whose visible name is not its slug respelled.
    for listed in &names() {
        let listed: &str = listed;
        if let Some(theme) = find(listed).filter(|theme| theme.label.to_lowercase() == folded) {
            return Ok(theme);
        }
    }
    if TERMINAL_ALIASES.contains(&slug.as_str()) {
        return Ok(Theme::terminal());
    }
    if !is_valid_slug(&slug) {
        return Err(ThemeResolveError::Invalid);
    }
    // No theme has the name. Whether its file is in the themes folder,
    // unread past the cap, is asked here alone: a name that is known never
    // comes this far, so a start that knows its theme pays nothing for it.
    Err(super::theme_files::past_cap(&slug)
        .map_or(ThemeResolveError::Unknown, |cap| ThemeResolveError::PastCap { cap }))
}

/// The name a theme is said by on the screen: its visible name, and for
/// `terminal` the slug — the word the setting takes.
#[must_use]
pub fn shown_name(theme: &Theme) -> &str {
    if theme.is_terminal() {
        &theme.name
    } else {
        &theme.label
    }
}

/// What keeps a named theme off the screen (it is saved all the same): the
/// codex look draws codex's bytes under every theme, and a 16-colour
/// terminal cannot draw a theme's colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unseen {
    CodexLook,
    SixteenColours,
}

/// Whether a named theme shows in `look` at colour depth `level` — `None`
/// when it does. The rule the drawing side follows where it resolves a role.
#[must_use]
pub fn unseen(
    look: super::style::TuiStyle,
    level: super::palette::ColorLevel,
) -> Option<Unseen> {
    if look != super::style::TuiStyle::Marks {
        Some(Unseen::CodexLook)
    } else if level == super::palette::ColorLevel::Ansi16 {
        Some(Unseen::SixteenColours)
    } else {
        None
    }
}

/// Every theme's slug on one line, `terminal` first — what an unknown name
/// is answered with.
#[must_use]
pub fn names_line() -> String {
    let mut line = String::new();
    for name in &names() {
        let name: &str = name;
        if !line.is_empty() {
            line.push_str(", ");
        }
        line.push_str(name);
    }
    line
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
        // A visible name is not a slug either, but it names its theme
        // (`a_label_is_taken_for_its_theme_at_start`).
        assert!(find("Dracula").is_none(), "find takes slugs only");
        let invalid = ["../x", "/tmp/a"];
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

        // An old value in another case is the same old value.
        let cased_pref = Preferences {
            theme: Some("Dark".to_string()),
            ..Preferences::default()
        };
        for (theme, warn) in [choose(&cased_pref, None), choose(&default, Some("Dark"))] {
            assert!(theme.is_terminal());
            assert_eq!(warn, None, "\"Dark\" must not warn");
        }
    }

    #[test]
    fn a_label_is_taken_for_its_theme_at_start() {
        use super::{resolve_name, ThemeResolveError};
        let default = Preferences::default();
        for (input, expected_slug) in [
            ("Dracula", "dracula"),
            ("DRACULA", "dracula"),
            ("Tokyo Night", "tokyo-night"),
            ("tokyo night", "tokyo-night"),
            ("solarized dark", "solarized-dark"),
            (" dracula ", "dracula"),
            ("Terminal", "terminal"),
            ("Dark", "terminal"),
            ("AUTO", "terminal"),
        ] {
            let pref = Preferences {
                theme: Some(input.to_string()),
                ..Preferences::default()
            };
            for (theme, warn) in [choose(&pref, None), choose(&default, Some(input))] {
                assert_eq!(theme.name, expected_slug, "{input:?}");
                assert_eq!(warn, None, "{input:?}");
            }
            assert_eq!(resolve_name(input).map(|theme| theme.name).as_deref(), Ok(expected_slug), "{input:?}");
        }
        // A name shaped like a path is never looked up, in any spelling.
        for bad in ["../x", "a.b", "a/b", "a\\b", "..", "themes/dracula", "dracula.json"] {
            let pref = Preferences {
                theme: Some(bad.to_string()),
                ..Preferences::default()
            };
            for (theme, warn) in [choose(&pref, None), choose(&default, Some(bad))] {
                assert!(theme.is_terminal(), "{bad:?}");
                assert!(
                    warn.as_deref().is_some_and(|msg| msg.contains("invalid theme name")),
                    "{bad:?} should warn invalid theme name: {warn:?}"
                );
            }
            assert_eq!(resolve_name(bad).map(|theme| theme.name), Err(ThemeResolveError::Invalid), "{bad:?}");
        }
        for empty in ["", "  "] {
            assert_eq!(resolve_name(empty).map(|theme| theme.name), Err(ThemeResolveError::Invalid), "{empty:?}");
        }
        for unknown in ["no-such-theme", "No Such Theme"] {
            assert_eq!(
                resolve_name(unknown).map(|theme| theme.name),
                Err(ThemeResolveError::Unknown),
                "{unknown:?}"
            );
        }
        // Every theme whose slug is its visible name in small letters and
        // hyphens — each built-in one — is named by that name, in any case.
        for slug in &names() {
            let slug: &str = slug;
            let theme = find(slug).expect("a listed theme");
            if theme.label.to_lowercase().replace(' ', "-") != slug {
                continue;
            }
            for word in [theme.label.clone(), theme.label.to_uppercase(), slug.to_uppercase()] {
                assert_eq!(resolve_name(&word).map(|found| found.name).as_deref(), Ok(slug), "{word:?}");
            }
        }
    }

    /// An empty value — `ZO_THEME=""`, `"theme": ""`, blanks alone — is no
    /// choice, not a bad name: an empty environment value leaves the choice
    /// to the setting, and an empty setting is `terminal`. Neither warns.
    #[test]
    fn an_empty_theme_value_is_no_choice() {
        let dracula = Preferences {
            theme: Some("dracula".to_string()),
            ..Preferences::default()
        };
        for empty in ["", " ", "\t "] {
            let (theme, warn) = choose(&dracula, Some(empty));
            assert_eq!(theme.name, "dracula", "an empty ZO_THEME ({empty:?}) leaves the setting");
            assert_eq!(warn, None, "{empty:?}");

            let unset = Preferences {
                theme: Some(empty.to_string()),
                ..Preferences::default()
            };
            for (theme, warn) in [choose(&unset, None), choose(&unset, Some(empty))] {
                assert!(theme.is_terminal(), "{empty:?}");
                assert_eq!(warn, None, "an empty value ({empty:?}) must not warn");
            }
            let (theme, warn) = choose(&unset, Some("nord"));
            assert_eq!(theme.name, "nord", "a named ZO_THEME over an empty setting ({empty:?})");
            assert_eq!(warn, None, "{empty:?}");
        }
    }

    #[test]
    fn a_named_theme_is_unseen_in_the_codex_look_and_on_sixteen_colours() {
        use super::{unseen, Unseen};
        use crate::tui::style::TuiStyle;
        for level in [ColorLevel::TrueColor, ColorLevel::Ansi256] {
            assert_eq!(unseen(TuiStyle::Marks, level), None, "{level:?}");
            assert_eq!(unseen(TuiStyle::Codex, level), Some(Unseen::CodexLook), "{level:?}");
        }
        assert_eq!(unseen(TuiStyle::Marks, ColorLevel::Ansi16), Some(Unseen::SixteenColours));
        // The look is said first: it is the one the person can change from here.
        assert_eq!(unseen(TuiStyle::Codex, ColorLevel::Ansi16), Some(Unseen::CodexLook));
    }

    #[test]
    fn the_names_line_lists_every_theme_terminal_first() {
        let line = super::names_line();
        let listed: Vec<&str> = line.split(", ").collect();
        let slugs = names();
        assert_eq!(listed.len(), slugs.len(), "{line}");
        for (listed, slug) in listed.iter().zip(&slugs) {
            let slug: &str = slug;
            assert_eq!(*listed, slug);
        }
        assert!(line.starts_with("terminal, "), "{line}");
    }

    /// Every name in the inline-code table is a built-in theme's and every
    /// value is a colour: a misspelt name would leave its theme on the blue
    /// slot without a word.
    #[test]
    fn every_inline_code_colour_names_a_builtin_theme_and_is_a_colour() {
        let table: std::collections::BTreeMap<String, String> =
            serde_json::from_str(super::THEME_INLINE_CODE_JSON).expect("the table is JSON");
        let builtins = super::builtin_names();
        assert_eq!(table.len(), 26, "{table:?}");
        for (slug, value) in &table {
            assert!(builtins.contains(&slug.as_str()), "{slug} is no built-in theme");
            assert!(parse_hex_color(value).is_some(), "{slug}: {value} is no colour");
            let theme = super::find_builtin(slug).expect("built-in");
            assert_eq!(theme.code, parse_hex_color(value), "{slug}");
        }
    }

    /// A built-in theme draws inline code in the colour its own sources give
    /// a code span, and a theme without one in its blue slot — no longer in
    /// its green slot, the one green nearly every theme drew it in. The nine
    /// that are still green are green by their own sources.
    #[test]
    fn a_builtin_theme_draws_inline_code_in_its_own_colour_else_its_blue() {
        use crate::tui::ansi::Role;
        let code = |slug: &str| super::find_builtin(slug).expect(slug).role_color(Role::Code);
        assert_eq!(code("dracula"), Some((0x50, 0xfa, 0x7b)));
        assert_eq!(code("ayu-dark"), Some((0xf2, 0x96, 0x68)));
        assert_eq!(code("nord"), Some((0x8f, 0xbc, 0xbb)));
        assert_eq!(code("gruvbox-dark"), Some((0x8e, 0xc0, 0x7c)));
        assert_eq!(code("tokyo-night"), Some((0x89, 0xdd, 0xff)));
        assert_eq!(code("homebrew"), Some((0x00, 0xa6, 0xb2)));
        // No colour of their own: the blue slot.
        assert_eq!(code("ghostty-default-style-dark"), Some((0x81, 0xa2, 0xbe)));
        assert_eq!(code("github-light"), Some((0x03, 0x66, 0xd6)));
        assert_eq!(code("snazzy"), Some((0x57, 0xc7, 0xff)));
        assert_eq!(code("tokyo-night-light"), Some((0x34, 0x54, 0x8a)));
        // `terminal` has no colour of its own: the role is drawn in a slot.
        assert_eq!(super::Theme::terminal().role_color(Role::Code), None);

        let green: Vec<String> = super::builtin_themes()
            .iter()
            .filter(|theme| theme.role_color(Role::Code) == theme.ansi[2])
            .map(|theme| theme.name.clone())
            .collect();
        assert_eq!(
            green,
            [
                "catppuccin-latte",
                "catppuccin-mocha",
                "dracula",
                "everforest-dark",
                "everforest-light",
                "material-dark",
                "one-dark",
                "one-light",
                "palenight",
            ]
        );
    }
}
