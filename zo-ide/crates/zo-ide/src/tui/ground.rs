//! What zo knows of the terminal's ground — the colour every row is drawn on.
//!
//! zo asks the terminal for its colours when it starts ([`super::palette`]).
//! A terminal that answers hands over its whole palette. One that does not —
//! every Windows console, a tmux with no client — hands over nothing, and a
//! named theme was then drawn in its own colours with no contrast floor: a
//! dark theme's pastels on a light console. The person can say whether the
//! ground is light or dark — `terminalBackground` in zo's settings,
//! `ZO_TERMINAL_BACKGROUND` for one run — and an rxvt-style `COLORFGBG` says
//! it when nothing else does. Knowing only that is a state of its own between
//! a measured palette and nothing ([`Ground::Lightness`]), and it turns three
//! things on:
//!
//! - a named theme's colours — its roles and its colour slots — keep 4.5:1
//!   against a stand-in ground: the hardest ground of that lightness among the
//!   window's thirty terminal themes, so they keep it on any of them;
//! - the syntax theme is Catppuccin Latte on a light ground and Mocha on a
//!   dark one, unless the named theme brings its own;
//! - the `terminal` theme's roles take their light-ground colours on a light
//!   ground.
//!
//! What needs the ground's own colour stays off: the bands under the person's
//! words, a code block and a list row, the inks measured against the body
//! text, and lowering colours to the terminal's depth. So the hint never
//! becomes a [`TerminalPalette`] — a palette turns all of those on at once.
//! The codex look and a sixteen-colour terminal draw what they drew.
//!
//! The person's word outranks a measured ground that disagrees with it: the
//! measured palette is then set aside and only the lightness is drawn with,
//! since a wrong answer is one reason to say it. A measured ground that agrees
//! is drawn with whole.

use std::cell::RefCell;
use std::sync::OnceLock;

use crate::preferences::Preferences;

use super::palette::{ColorLevel, TerminalPalette, MIN_SECONDARY_CONTRAST};
use super::style::TuiStyle;

/// `ZO_TERMINAL_BACKGROUND`: overrides `"terminalBackground"` from
/// `settings.json` for one run.
pub const TERMINAL_BACKGROUND_ENV: &str = "ZO_TERMINAL_BACKGROUND";

/// The rxvt convention some terminals export: the colour slots of the default
/// foreground and background, `fg;bg` (rxvt's own `fg;default;bg`).
pub const COLORFGBG_ENV: &str = "COLORFGBG";

/// What the person said of the terminal's background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalBackground {
    /// Nothing: the terminal's answer decides, else `COLORFGBG`.
    #[default]
    Auto,
    Light,
    Dark,
}

impl TerminalBackground {
    /// The setting's word, in any case, with blanks around it or not.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    /// Whether the person said the ground is light — `None` for `auto`.
    #[must_use]
    pub const fn light(self) -> Option<bool> {
        match self {
            Self::Auto => None,
            Self::Light => Some(true),
            Self::Dark => Some(false),
        }
    }
}

/// The person's word: `env_value` (`ZO_TERMINAL_BACKGROUND`), else
/// `preferences.terminal_background`, else `auto`. The word that decides is
/// `auto`, `light` or `dark`; any other is `auto`, with a warning. An empty
/// value, or blanks alone, says nothing — an empty `ZO_TERMINAL_BACKGROUND`
/// leaves the choice to the setting — and a word the environment gets wrong
/// is not passed over for the setting's.
#[must_use]
pub fn choose(
    preferences: &Preferences,
    env_value: Option<&str>,
) -> (TerminalBackground, Option<String>) {
    fn said(value: Option<&str>) -> Option<&str> {
        value.filter(|value| !value.trim().is_empty())
    }
    let Some(word) =
        said(env_value).or_else(|| said(preferences.terminal_background.as_deref()))
    else {
        return (TerminalBackground::Auto, None);
    };
    TerminalBackground::parse(word).map_or_else(
        || {
            let warning = format!("unknown terminalBackground {word:?}; using auto (auto, light or dark)");
            (TerminalBackground::Auto, Some(warning))
        },
        |background| (background, None),
    )
}

/// Whether a `COLORFGBG` value names a light ground, read as pi and termbg
/// read it: its last field is the background's colour slot — 0–6 and 8 are
/// dark, 7 and 9–15 light. Two or three fields (`fg;bg`, rxvt's
/// `fg;default;bg`), each a slot number or `default`; any other shape, or a
/// last field that is no slot of the sixteen, names nothing.
#[must_use]
pub fn colorfgbg_is_light(value: &str) -> Option<bool> {
    let fields: Vec<&str> = value.trim().split(';').collect();
    let (background, before) = fields.split_last()?;
    if !(1..=2).contains(&before.len())
        || !before
            .iter()
            .all(|field| *field == "default" || colour_slot(field).is_some())
    {
        return None;
    }
    match colour_slot(background)? {
        0..=6 | 8 => Some(false),
        7 | 9..=15 => Some(true),
        _ => None,
    }
}

/// A colour slot's number: one to three ASCII digits, no more than 255.
fn colour_slot(field: &str) -> Option<u8> {
    if field.is_empty() || field.len() > 3 || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

/// What zo knows of the ground it draws on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ground {
    /// Nothing: no answer, no word from the person, no `COLORFGBG`.
    Unknown,
    /// Whether the ground is light, and no more.
    Lightness { light: bool },
    /// The palette the terminal answered with.
    Measured(TerminalPalette),
}

impl Ground {
    /// The measured palette, when there is one — never one made up from a
    /// lightness.
    #[must_use]
    pub const fn palette(self) -> Option<TerminalPalette> {
        match self {
            Self::Measured(palette) => Some(palette),
            Self::Unknown | Self::Lightness { .. } => None,
        }
    }

    /// Whether the ground is known to be light, measured or said.
    #[must_use]
    pub fn is_light(self) -> bool {
        match self {
            Self::Measured(palette) => palette.has_light_background(),
            Self::Lightness { light } => light,
            Self::Unknown => false,
        }
    }
}

impl From<Option<TerminalPalette>> for Ground {
    /// A palette handed in alone: measured, or nothing known.
    fn from(palette: Option<TerminalPalette>) -> Self {
        palette.map_or(Self::Unknown, Self::Measured)
    }
}

/// What the interactive front read of the ground when it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Hint {
    /// The person's word.
    pub word: TerminalBackground,
    /// What `COLORFGBG` says — `None` when it is unset or of another shape.
    pub colorfgbg: Option<bool>,
}

/// The ground a process in `look` draws on at colour depth `level`, from what
/// it read when it started (`hint`) and the palette its terminal answered
/// with (`measured`):
///
/// - the person's `light` or `dark` keeps a measured palette of that
///   lightness and sets aside one of the other; with no palette, only the
///   lightness is known;
/// - `auto` takes the measured palette, else what `COLORFGBG` says, else
///   nothing.
///
/// The codex look draws codex's bytes and a sixteen-colour terminal draws
/// what it drew, whatever was said — and so does a process that read no hint
/// (the pipe front, a test): each has the measured palette or nothing.
#[must_use]
pub fn resolve(
    look: TuiStyle,
    level: ColorLevel,
    hint: Option<Hint>,
    measured: Option<TerminalPalette>,
) -> Ground {
    let Some(hint) = hint.filter(|_| look == TuiStyle::Marks && level != ColorLevel::Ansi16) else {
        return Ground::from(measured);
    };
    let said = hint.word.light();
    if let Some(palette) = measured {
        // The answer stands unless the person said the other lightness.
        return match said {
            Some(light) if palette.has_light_background() != light => Ground::Lightness { light },
            _ => Ground::Measured(palette),
        };
    }
    said.or(hint.colorfgbg)
        .map_or(Ground::Unknown, |light| Ground::Lightness { light })
}

static HINT: OnceLock<Hint> = OnceLock::new();

/// Reads the person's word (`ZO_TERMINAL_BACKGROUND`, then the setting) and
/// `COLORFGBG` for the interactive front, before its first frame, queuing a
/// startup warning for a word it does not know. A second call changes
/// nothing.
pub fn init(preferences: &Preferences) {
    let env_word = std::env::var(TERMINAL_BACKGROUND_ENV).ok();
    let (word, warning) = choose(preferences, env_word.as_deref());
    let colorfgbg = std::env::var(COLORFGBG_ENV)
        .ok()
        .as_deref()
        .and_then(colorfgbg_is_light);
    set(Hint { word, colorfgbg });
    if let Some(warning) = warning {
        crate::runtime_support::push_startup_warning(warning);
    }
}

/// Fixes this process's hint: what [`init`] read, or what a test's fresh run
/// hands in. The first call wins.
pub fn set(hint: Hint) {
    let _ = HINT.set(hint);
}

/// The ground this process draws on: the hint [`init`] read against the
/// palette the startup probe measured. Worked out on each call and never
/// kept — the hint is read before the probe runs.
#[must_use]
pub(crate) fn current() -> Ground {
    let measured = super::palette::measured_palette();
    let Some(hint) = HINT.get().copied() else {
        return Ground::from(measured);
    };
    let level = measured.map_or_else(
        super::palette::color_level_from_env,
        TerminalPalette::color_level,
    );
    resolve(super::style::current().style, level, Some(hint), measured)
}

/// The ground a named theme's colours keep their contrast against when only
/// a dark ground is known: the lightest dark background among the window's
/// thirty terminal themes — Everforest Dark, `#2d353b`. Contrast is counted
/// from relative luminance, so a colour that keeps 4.5:1 on it keeps it on
/// every darker ground (`the_stand_in_grounds_are_the_hardest_of_the_windows_thirty`
/// counts it again from `terminal_themes.json`).
pub(crate) const DARK_STAND_IN: (u8, u8, u8) = (45, 53, 59);

/// The ground a named theme's colours keep their contrast against when only
/// a light ground is known: the darkest light background among the window's
/// thirty terminal themes — Tokyo Night Light, `#d5d6db`.
pub(crate) const LIGHT_STAND_IN: (u8, u8, u8) = (213, 214, 219);

/// The floor a named theme's colour is held to when only the ground's
/// lightness is known. The measured floor is the smaller of 4.5:1 and three
/// quarters of the body text's own contrast
/// ([`TerminalPalette::text_contrast_floor`]); with no body ink to count, it
/// is the 4.5:1 cap.
const LIGHTNESS_ONLY_FLOOR: f64 = MIN_SECONDARY_CONTRAST;

/// The stand-in ground for a light ground, or for a dark one.
#[must_use]
pub(crate) const fn stand_in(light: bool) -> (u8, u8, u8) {
    if light {
        LIGHT_STAND_IN
    } else {
        DARK_STAND_IN
    }
}

struct FitEntry {
    revision: u64,
    color: (u8, u8, u8),
    background: (u8, u8, u8),
    fitted: (u8, u8, u8),
}

thread_local! {
    static FITS: RefCell<Vec<FitEntry>> = const { RefCell::new(Vec::new()) };
}

/// A named theme's `color` on `background` — the stand-in ground, or a
/// span's own — when only the ground's lightness is known: its lightness
/// moved until it keeps 4.5:1 there, hue and saturation kept. Kept for one
/// theme revision at a time, as the measured fit is
/// (`palette::ensure_text_contrast_cached`).
#[must_use]
pub(crate) fn fit_without_body_ink(
    color: (u8, u8, u8),
    background: (u8, u8, u8),
) -> (u8, u8, u8) {
    let revision = super::theme::revision();
    FITS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(entry) = cache.iter().find(|entry| {
            entry.revision == revision && entry.color == color && entry.background == background
        }) {
            return entry.fitted;
        }
        let fitted = super::theme::fit_lightness_to_floor(color, background, LIGHTNESS_ONLY_FLOOR);
        cache.retain(|entry| entry.revision == revision);
        cache.push(FitEntry {
            revision,
            color,
            background,
            fitted,
        });
        fitted
    })
}

#[cfg(test)]
mod tests {
    use super::{
        choose, colorfgbg_is_light, fit_without_body_ink, resolve, stand_in, Ground, Hint,
        TerminalBackground, DARK_STAND_IN, LIGHT_STAND_IN,
    };
    use crate::preferences::Preferences;
    use crate::tui::palette::{contrast_ratio, is_light_background, ColorLevel, TerminalPalette};
    use crate::tui::style::TuiStyle;
    use crate::tui::theme;

    fn setting(word: &str) -> Preferences {
        Preferences {
            terminal_background: Some(word.to_string()),
            ..Preferences::default()
        }
    }

    fn palette_on(background: (u8, u8, u8)) -> TerminalPalette {
        TerminalPalette::new((128, 128, 128), background, ColorLevel::TrueColor)
    }

    /// The person's word: the environment's, else the setting's, else
    /// `auto`, in any case and with blanks around it. An empty value says
    /// nothing. Any other word is `auto` with one warning that names it — and
    /// a wrong word in the environment is not passed over for the setting's.
    #[test]
    fn terminal_background_is_auto_light_or_dark_and_anything_else_warns() {
        use TerminalBackground::{Auto, Dark, Light};
        let none = Preferences::default();
        assert_eq!(choose(&none, None), (Auto, None));
        for (word, said) in [
            ("auto", Auto),
            ("light", Light),
            ("dark", Dark),
            ("LIGHT", Light),
            (" Dark ", Dark),
            ("Auto", Auto),
        ] {
            assert_eq!(choose(&setting(word), None), (said, None), "setting {word:?}");
            assert_eq!(choose(&none, Some(word)), (said, None), "environment {word:?}");
        }
        // The environment over the setting, for one run — `auto` too.
        assert_eq!(choose(&setting("light"), Some("dark")), (Dark, None));
        assert_eq!(choose(&setting("dark"), Some("auto")), (Auto, None));
        // An empty value says nothing: an empty environment value leaves the
        // choice to the setting, and an empty setting is `auto`.
        for empty in ["", " ", "\t "] {
            assert_eq!(choose(&setting("light"), Some(empty)), (Light, None), "{empty:?}");
            assert_eq!(choose(&setting(empty), None), (Auto, None), "{empty:?}");
            assert_eq!(choose(&setting(empty), Some(empty)), (Auto, None), "{empty:?}");
        }
        // Anything else is `auto`, with a warning naming the word.
        for wrong in ["blue", "lite", "system", "0", "light dark"] {
            for (preferences, env) in [(setting(wrong), None), (setting("light"), Some(wrong))] {
                let (said, warning) = choose(&preferences, env);
                assert_eq!(said, Auto, "{wrong:?} (environment {env:?})");
                let warning = warning.unwrap_or_else(|| panic!("{wrong:?} warns"));
                assert!(
                    warning.contains(&format!("{wrong:?}")) && warning.contains("using auto"),
                    "{warning}"
                );
            }
        }
    }

    /// The picture's branches: the person's `light` or `dark` keeps a
    /// measured ground of that lightness and sets aside one of the other; with
    /// no answer only the lightness is known; `auto` takes the answer as it
    /// was. The codex look, a sixteen-colour terminal and a process that read
    /// no hint draw what they drew, whatever was said.
    #[test]
    fn the_persons_word_beats_a_measured_ground_that_disagrees_and_keeps_one_that_agrees() {
        use TerminalBackground::{Auto, Dark, Light};
        let dark = palette_on((28, 30, 38));
        let light = palette_on((253, 246, 227));
        let said = |word| Some(Hint { word, colorfgbg: None });
        let marks = |word, measured| resolve(TuiStyle::Marks, ColorLevel::TrueColor, said(word), measured);
        // Agreeing: the measured ground, whole.
        assert_eq!(marks(Light, Some(light)), Ground::Measured(light));
        assert_eq!(marks(Dark, Some(dark)), Ground::Measured(dark));
        // Disagreeing: the measured ground is set aside, the word's lightness kept.
        assert_eq!(marks(Light, Some(dark)), Ground::Lightness { light: true });
        assert_eq!(marks(Dark, Some(light)), Ground::Lightness { light: false });
        // No answer: the word's lightness.
        assert_eq!(marks(Light, None), Ground::Lightness { light: true });
        assert_eq!(marks(Dark, None), Ground::Lightness { light: false });
        // `auto`: the answer as it was, or nothing.
        assert_eq!(marks(Auto, Some(light)), Ground::Measured(light));
        assert_eq!(marks(Auto, Some(dark)), Ground::Measured(dark));
        assert_eq!(marks(Auto, None), Ground::Unknown);
        // 256 colours draw as truecolour does.
        assert_eq!(
            resolve(TuiStyle::Marks, ColorLevel::Ansi256, said(Light), Some(dark)),
            Ground::Lightness { light: true }
        );

        for word in [Auto, Light, Dark] {
            for colorfgbg in [None, Some(true), Some(false)] {
                let hint = Some(Hint { word, colorfgbg });
                for measured in [None, Some(dark), Some(light)] {
                    let as_today = Ground::from(measured);
                    for level in [ColorLevel::TrueColor, ColorLevel::Ansi256, ColorLevel::Ansi16] {
                        assert_eq!(resolve(TuiStyle::Codex, level, hint, measured), as_today, "codex look");
                        assert_eq!(resolve(TuiStyle::Marks, level, None, measured), as_today, "no hint read");
                    }
                    assert_eq!(
                        resolve(TuiStyle::Marks, ColorLevel::Ansi16, hint, measured),
                        as_today,
                        "16 colours, {word:?}, COLORFGBG {colorfgbg:?}"
                    );
                }
            }
        }

        // The ground's lightness, as the syntax theme asks it; a lightness is
        // never a palette.
        assert!(Ground::Measured(light).is_light() && !Ground::Measured(dark).is_light());
        assert!(Ground::Lightness { light: true }.is_light());
        assert!(!Ground::Lightness { light: false }.is_light() && !Ground::Unknown.is_light());
        assert_eq!(Ground::Lightness { light: true }.palette(), None);
        assert_eq!(Ground::Unknown.palette(), None);
        assert_eq!(Ground::Measured(dark).palette(), Some(dark));
    }

    /// `COLORFGBG`'s last field is the background's colour slot — 0–6 and 8
    /// dark, 7 and 9–15 light — in `fg;bg` or rxvt's `fg;default;bg`; any
    /// other shape names nothing. It is read only when the person said
    /// `auto` and the terminal did not answer.
    #[test]
    fn colorfgbg_is_read_only_when_nothing_else_says() {
        use TerminalBackground::{Auto, Dark, Light};
        for slot in 0u8..=15 {
            let light = slot == 7 || slot >= 9;
            assert_eq!(colorfgbg_is_light(&format!("15;{slot}")), Some(light), "slot {slot}");
            assert_eq!(colorfgbg_is_light(&format!("0;default;{slot}")), Some(light), "rxvt, slot {slot}");
        }
        assert_eq!(colorfgbg_is_light("default;15"), Some(true));
        assert_eq!(colorfgbg_is_light(" 15;0\n"), Some(false), "blanks around the value");
        for other in [
            "", "0", "15;", ";0", "15;16", "15;255", "15;default", "15;-1", "15; 0", "15;0;",
            "1;2;3;4", "15;0x0", "256;0", "15;\u{661}", "fifteen;0",
        ] {
            assert_eq!(colorfgbg_is_light(other), None, "{other:?}");
        }

        let dark = palette_on((28, 30, 38));
        let read = |word, colorfgbg, measured| {
            resolve(TuiStyle::Marks, ColorLevel::TrueColor, Some(Hint { word, colorfgbg }), measured)
        };
        // Nothing else says: COLORFGBG does.
        assert_eq!(read(Auto, Some(true), None), Ground::Lightness { light: true });
        assert_eq!(read(Auto, Some(false), None), Ground::Lightness { light: false });
        assert_eq!(read(Auto, None, None), Ground::Unknown);
        // The terminal's answer outranks it, and so does the person's word.
        assert_eq!(read(Auto, Some(true), Some(dark)), Ground::Measured(dark));
        assert_eq!(read(Dark, Some(true), None), Ground::Lightness { light: false });
        assert_eq!(read(Light, Some(false), None), Ground::Lightness { light: true });
        assert_eq!(read(Dark, Some(true), Some(dark)), Ground::Measured(dark));
    }

    /// The stand-ins are the hardest grounds of their lightness among the
    /// window's thirty terminal themes (`terminal_themes.json`), counted again
    /// here: the dark background of the most luminance and the light one of
    /// the least. What that buys: every colour of every built-in theme, fitted
    /// on its stand-in, keeps 4.5:1 on each of the thirty of that lightness.
    #[test]
    fn the_stand_in_grounds_are_the_hardest_of_the_windows_thirty() {
        let builtins: Vec<theme::Theme> = theme::builtin_names()
            .into_iter()
            .filter(|name| *name != theme::TERMINAL_THEME_NAME)
            .filter_map(theme::find_builtin)
            .collect();
        assert_eq!(builtins.len(), 30);
        let grounds: Vec<(u8, u8, u8)> = builtins
            .iter()
            .map(|builtin| builtin.background.expect("every built-in theme names its background"))
            .collect();
        let (light_grounds, dark_grounds): (Vec<_>, Vec<_>) =
            grounds.iter().copied().partition(|ground| is_light_background(*ground));
        // Contrast is counted from luminance, and the ratio to black grows with it.
        let luminance = |ground: &(u8, u8, u8)| contrast_ratio(*ground, (0, 0, 0));
        let lightest_dark = dark_grounds
            .iter()
            .copied()
            .max_by(|a, b| luminance(a).total_cmp(&luminance(b)))
            .expect("dark grounds");
        let darkest_light = light_grounds
            .iter()
            .copied()
            .min_by(|a, b| luminance(a).total_cmp(&luminance(b)))
            .expect("light grounds");
        assert_eq!((DARK_STAND_IN, LIGHT_STAND_IN), (lightest_dark, darkest_light));
        assert_eq!((stand_in(false), stand_in(true)), (DARK_STAND_IN, LIGHT_STAND_IN));

        let inks: Vec<(u8, u8, u8)> = builtins
            .iter()
            .flat_map(|builtin| builtin.ansi.into_iter().flatten().chain(builtin.foreground))
            .collect();
        for (light, kind) in [(false, &dark_grounds), (true, &light_grounds)] {
            for ink in &inks {
                let fitted = fit_without_body_ink(*ink, stand_in(light));
                for ground in kind {
                    assert!(
                        contrast_ratio(fitted, *ground) >= 4.5,
                        "{ink:?}, fitted to {fitted:?} on the stand-in, on {ground:?}"
                    );
                }
            }
        }
    }
}
