//! The person's own colour themes: `<zo home>/themes/<slug>.json`, each laid
//! over a built-in theme (`base`) or over the terminal's own colours. The
//! folder is read at start and again whenever the theme names are asked (the
//! picker opening), never watched. A mistake in a file is said as one warning
//! line and the rest of the file is still used; a file that cannot be read at
//! all is no theme. Only the zo home is read — never a project folder.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::{Map, Value};

use super::theme::{self, Theme, TERMINAL_THEME_NAME};

/// The largest theme file read: 256 KiB.
pub const MAX_THEME_FILE_BYTES: u64 = 256 * 1024;

/// The folder under the zo home that holds the theme files.
const THEMES_FOLDER: &str = "themes";

/// The sixteen colour slot names a theme file's `ansi` takes, in slot order.
const ANSI_SLOT_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
];

/// Where one colour name of a section (`ansi` or `colors`) lands in a theme.
type ColourField = for<'t> fn(&'t mut Theme, &str) -> Option<&'t mut Option<(u8, u8, u8)>>;

/// Reads every `<slug>.json` directly inside `dir` into a theme, sorted by
/// name, and says each mistake as one warning line. A missing folder is no
/// themes and no warning.
#[must_use]
pub fn load_from(dir: &Path) -> (Vec<Theme>, Vec<String>) {
    let mut warnings = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (Vec::new(), warnings),
        Err(error) => {
            warnings.push(format!(
                "theme folder {}: cannot be read ({error})",
                dir.display()
            ));
            return (Vec::new(), warnings);
        }
    };
    let folder = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    let mut themes = BTreeMap::new();
    for path in &paths {
        if let Some(theme) = read_theme_file(path, &folder, &mut warnings) {
            themes.insert(theme.name.clone(), theme);
        }
    }
    (themes.into_values().collect(), warnings)
}

/// One theme file's warnings, each naming the file.
struct Notes<'a> {
    file: String,
    out: &'a mut Vec<String>,
}

impl Notes<'_> {
    fn say(&mut self, what: &str) {
        self.out.push(format!("theme file {:?}: {what}", self.file));
    }
}

/// The theme in `path`, or `None` — said, unless it is no theme file at all
/// (another extension, or a folder).
fn read_theme_file(path: &Path, folder: &Path, warnings: &mut Vec<String>) -> Option<Theme> {
    if path.extension().and_then(OsStr::to_str) != Some("json") {
        return None;
    }
    let mut notes = Notes {
        file: path.file_name().map_or_else(String::new, |name| {
            name.to_string_lossy().into_owned()
        }),
        out: warnings,
    };
    let target = match fs::metadata(path) {
        Ok(target) => target,
        Err(error) => {
            notes.say(&format!("cannot be read ({error}); skipped"));
            return None;
        }
    };
    if target.is_dir() {
        return None;
    }
    let Some(slug) = path
        .file_stem()
        .and_then(OsStr::to_str)
        .filter(|stem| theme::is_valid_slug(stem))
    else {
        notes.say("its name is not a theme name (lowercase letters, digits and `-`); skipped");
        return None;
    };
    if slug == TERMINAL_THEME_NAME {
        notes.say("`terminal` is the terminal's own colours and cannot be replaced; skipped");
        return None;
    }
    let is_link = fs::symlink_metadata(path).is_ok_and(|link| link.file_type().is_symlink());
    if is_link && !fs::canonicalize(path).is_ok_and(|real| real.starts_with(folder)) {
        notes.say("links to a file outside the themes folder; skipped");
        return None;
    }
    if !target.is_file() {
        notes.say("is not a regular file; skipped");
        return None;
    }
    let too_large = format!(
        "is larger than {} KiB; skipped",
        MAX_THEME_FILE_BYTES / 1024
    );
    if target.len() > MAX_THEME_FILE_BYTES {
        notes.say(&too_large);
        return None;
    }
    let bytes = match read_capped(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            notes.say(&format!("cannot be read ({error}); skipped"));
            return None;
        }
    };
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_THEME_FILE_BYTES {
        notes.say(&too_large);
        return None;
    }
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let fields = match serde_json::from_slice::<Value>(text) {
        Ok(Value::Object(fields)) => fields,
        Ok(_) => {
            notes.say("is not a JSON object; skipped");
            return None;
        }
        Err(error) => {
            notes.say(&format!("is not JSON ({error}); skipped"));
            return None;
        }
    };
    Some(build_theme(slug, &fields, &mut notes))
}

/// The file's bytes, at most one past [`MAX_THEME_FILE_BYTES`] — a file that
/// grew after it was measured is still caught.
fn read_capped(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_THEME_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The theme a file's fields describe: its `base` first, then its own
/// colours and syntax theme over it.
fn build_theme(slug: &str, fields: &Map<String, Value>, notes: &mut Notes<'_>) -> Theme {
    let mut made = Theme::empty(slug, slug);
    if let Some(base) = fields.get("base") {
        match base.as_str().map(|name| (name, theme::find_builtin(name))) {
            Some((_, Some(builtin))) => {
                made = Theme {
                    name: made.name,
                    label: made.label,
                    // The base's syntax theme, which a built-in theme takes
                    // from its own name.
                    syntax: builtin.syntax.clone().or_else(|| {
                        theme::syntax_theme_for_name(&builtin.name).map(|_| builtin.name.clone())
                    }),
                    ..builtin
                };
            }
            Some((name, None)) => notes.say(&format!(
                "base {} is not a built-in theme; the terminal's colours are used instead",
                short(name)
            )),
            None => notes.say(&format!(
                "\"base\" is a built-in theme's name, not {}; ignored",
                shown(base)
            )),
        }
    }
    for (key, value) in fields {
        match key.as_str() {
            "base" => {}
            "ansi" => read_colours(&mut made, key, value, ansi_field, "colour slot", notes),
            "colors" => read_colours(&mut made, key, value, role_field, "role", notes),
            "syntax" => match value.as_str() {
                Some(name) if theme::syntax_theme_for_name(name).is_some() => {
                    made.syntax = Some(name.to_owned());
                }
                Some(name) => notes.say(&format!(
                    "syntax {} is not a syntax theme zo has; ignored",
                    short(name)
                )),
                None => notes.say(&format!(
                    "\"syntax\" is a syntax theme's name, not {}; ignored",
                    shown(value)
                )),
            },
            other => notes.say(&format!(
                "unknown key {} (a theme file has base, ansi, colors and syntax); ignored",
                short(other)
            )),
        }
    }
    made
}

/// Where a colour slot name of `ansi` lands.
fn ansi_field<'t>(theme: &'t mut Theme, name: &str) -> Option<&'t mut Option<(u8, u8, u8)>> {
    let slot = ANSI_SLOT_NAMES.iter().position(|slot| *slot == name)?;
    Some(&mut theme.ansi[slot])
}

/// Where a role name of `colors` lands.
fn role_field<'t>(theme: &'t mut Theme, name: &str) -> Option<&'t mut Option<(u8, u8, u8)>> {
    Some(match name {
        "accent" => &mut theme.accent,
        "code" => &mut theme.code,
        "link" => &mut theme.link,
        "heading" => &mut theme.heading,
        "caret" => &mut theme.caret,
        "model" => &mut theme.model,
        "cwd" => &mut theme.cwd,
        "warning" => &mut theme.warning,
        _ => return None,
    })
}

/// The colours of one section (`ansi` or `colors`): each name `field` knows
/// whose value is a colour is kept; every other one is said.
fn read_colours(
    target: &mut Theme,
    key: &str,
    value: &Value,
    field: ColourField,
    kind: &str,
    notes: &mut Notes<'_>,
) {
    let Some(colours) = value.as_object() else {
        notes.say(&format!(
            "{key:?} is an object of colours, not {}; ignored",
            shown(value)
        ));
        return;
    };
    for (name, colour) in colours {
        let Some(slot) = field(target, name) else {
            notes.say(&format!(
                "unknown {kind} {} in {key:?}; ignored",
                short(name)
            ));
            continue;
        };
        match colour.as_str().and_then(theme::parse_hex_color) {
            Some(rgb) => *slot = Some(rgb),
            None => notes.say(&format!(
                "{key}.{name} = {} is not a colour (#rgb or #rrggbb); ignored",
                shown(colour)
            )),
        }
    }
}

/// `text` quoted, cut after 40 characters — a warning stays one short line
/// whatever a file holds.
fn short(text: &str) -> String {
    const MOST: usize = 40;
    if text.chars().count() <= MOST {
        return format!("{text:?}");
    }
    let head: String = text.chars().take(MOST).collect();
    format!("{head:?}…")
}

/// A JSON value as a warning shows it: a string quoted, a number or `true`
/// as written, a list or an object by its kind.
fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => short(text),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => "null".to_owned(),
        Value::Array(_) => "a list".to_owned(),
        Value::Object(_) => "an object".to_owned(),
    }
}

/// The person's themes as last read, and the warnings that read said.
#[derive(Debug, Default)]
pub(crate) struct Catalog {
    themes: Option<Vec<Theme>>,
    said: BTreeSet<String>,
}

impl Catalog {
    pub(crate) const fn new() -> Self {
        Self {
            themes: None,
            said: BTreeSet::new(),
        }
    }

    /// Reads `dir` again (`None`: no folder) and returns the warnings the
    /// read before did not say, so a picker opened again and again does not
    /// repeat the same line.
    pub(crate) fn read(&mut self, dir: Option<&Path>) -> Vec<String> {
        let (themes, warnings) = dir.map_or_else(|| (Vec::new(), Vec::new()), load_from);
        self.themes = Some(themes);
        let fresh = warnings
            .iter()
            .filter(|warning| !self.said.contains(*warning))
            .cloned()
            .collect();
        self.said = warnings.into_iter().collect();
        fresh
    }

    /// Reads `dir` unless it has been read; the warnings of that read.
    fn read_once(&mut self, dir: Option<&Path>) -> Vec<String> {
        if self.themes.is_some() {
            return Vec::new();
        }
        self.read(dir)
    }

    /// Every slug after reading `dir` again: `terminal`, the built-in themes,
    /// then the person's themes by name — one that replaces a built-in one
    /// keeps the built-in place. Returns the list and the new warnings.
    pub(crate) fn names(&mut self, dir: Option<&Path>) -> (Vec<String>, Vec<String>) {
        let warnings = self.read(dir);
        let mut list: Vec<String> = theme::builtin_names()
            .into_iter()
            .map(str::to_owned)
            .collect();
        for user in self.themes.iter().flatten() {
            if !list.contains(&user.name) {
                list.push(user.name.clone());
            }
        }
        (list, warnings)
    }

    /// `terminal`, else the person's theme of that name, else the built-in one.
    pub(crate) fn find(&self, slug: &str) -> Option<Theme> {
        if !theme::is_valid_slug(slug) {
            return None;
        }
        if slug == TERMINAL_THEME_NAME {
            return Some(Theme::terminal());
        }
        self.themes
            .iter()
            .flatten()
            .find(|user| user.name == slug)
            .cloned()
            .or_else(|| theme::find_builtin(slug))
    }
}

static CATALOG: Mutex<Catalog> = Mutex::new(Catalog::new());

/// `<zo home>/themes` — the one place the person's real home is chosen. A
/// test build reads no folder, so no test sees the person's themes.
fn user_themes_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(runtime::default_config_home().join(THEMES_FOLDER))
}

/// Runs `f` on the process catalog and queues the warnings it returns where
/// the screen says startup warnings (`/warnings`).
fn with_catalog<R>(f: impl FnOnce(&mut Catalog, Option<&Path>) -> (R, Vec<String>)) -> R {
    let dir = user_themes_dir();
    let (value, warnings) = {
        let mut catalog = CATALOG.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut catalog, dir.as_deref())
    };
    for warning in warnings {
        crate::runtime_support::push_startup_warning(warning);
    }
    value
}

/// Every theme slug, reading the person's themes folder again.
pub(crate) fn names() -> Vec<String> {
    with_catalog(Catalog::names)
}

/// A theme by slug; reads the person's themes folder if it was never read.
pub(crate) fn find(slug: &str) -> Option<Theme> {
    with_catalog(|catalog, dir| {
        let warnings = catalog.read_once(dir);
        (catalog.find(slug), warnings)
    })
}

/// Reads the person's themes folder at start, saying what is wrong in it.
pub(crate) fn read_at_start() {
    with_catalog(|catalog, dir| ((), catalog.read(dir)));
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use two_face::theme::EmbeddedThemeName;

    use super::{load_from, Catalog, MAX_THEME_FILE_BYTES};
    use crate::tui::ansi::Role;
    use crate::tui::theme::find_builtin;

    fn write_theme(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).expect("write theme fixture");
    }

    #[test]
    fn a_user_theme_layers_over_its_base() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_theme(
            dir.path(),
            "my-dracula.json",
            r##"{
                "base": "dracula",
                "ansi": { "green": "#69ff94" },
                "colors": { "accent": "#ff79c6" }
            }"##,
        );

        let (themes, warnings) = load_from(dir.path());
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert_eq!(themes.len(), 1, "expected 1 user theme: {themes:?}");

        let dracula = find_builtin("dracula").expect("dracula builtin");
        let custom = &themes[0];
        // Everything but the two colours written is dracula's.
        let mut expected = dracula.clone();
        expected.name = "my-dracula".to_owned();
        expected.label = "my-dracula".to_owned();
        expected.ansi[2] = Some((0x69, 0xff, 0x94));
        expected.accent = Some((0xff, 0x79, 0xc6));
        expected.syntax.clone_from(&custom.syntax);
        assert_eq!(custom, &expected);
        assert_eq!(custom.role_color(Role::Code), Some((0x69, 0xff, 0x94)));
        assert_eq!(custom.role_color(Role::Heading), Some((0xff, 0x79, 0xc6)));
        assert_eq!(custom.role_color(Role::Caret), dracula.role_color(Role::Caret));
        // … and so is its syntax theme, whatever the file is called.
        assert_eq!(custom.syntax_theme(false), EmbeddedThemeName::Dracula);
        assert_eq!(custom.syntax_theme(true), EmbeddedThemeName::Dracula);

        let mut catalog = Catalog::new();
        let (list, catalog_warnings) = catalog.names(Some(dir.path()));
        assert!(catalog_warnings.is_empty());
        assert_eq!(list.len(), 32);
        assert_eq!(list[31], "my-dracula");
        assert_eq!(catalog.find("my-dracula").as_ref(), Some(custom));
    }

    #[test]
    fn a_user_theme_without_a_base_leaves_unset_colours_to_the_terminal() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_theme(
            dir.path(),
            "sparse-user.json",
            r##"{
                "colors": { "code": "#AbC", "warning": "#ffAA00" }
            }"##,
        );

        let (themes, warnings) = load_from(dir.path());
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert_eq!(themes.len(), 1);

        let theme = &themes[0];
        let mut expected = crate::tui::theme::Theme::empty("sparse-user", "sparse-user");
        expected.code = Some((0xaa, 0xbb, 0xcc));
        expected.warning = Some((0xff, 0xaa, 0x00));
        assert_eq!(theme, &expected);
        assert_eq!(theme.role_color(Role::Code), Some((0xaa, 0xbb, 0xcc)));
        assert_eq!(theme.role_color(Role::Link), None);
        assert_eq!(theme.ansi_slot(1), None);
    }

    #[test]
    fn a_user_theme_file_with_mistakes_keeps_what_it_can_and_says_each_mistake() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_theme(
            dir.path(),
            "flawed.json",
            r##"{
                "base": "no-such-base",
                "unknownTop": true,
                "ansi": {
                    "green": "#69ff94",
                    "purple": "#112233",
                    "red": "not-a-hex",
                    "blue": "#+1+2+3",
                    "yellow": "#é1",
                    "cyan": 7
                },
                "colors": {
                    "accent": "#ff79c6",
                    "bogusRole": "#112233",
                    "code": "#12345"
                },
                "syntax": "no-such-syntax"
            }"##,
        );
        write_theme(
            dir.path(),
            "wrong-types.json",
            r##"{ "base": 3, "ansi": "x", "colors": ["#ffffff"], "syntax": false }"##,
        );

        let (themes, warnings) = load_from(dir.path());
        assert_eq!(themes.len(), 2, "readable files still make themes: {themes:?}");
        let theme = &themes[0];
        assert_eq!(theme.name, "flawed");
        let mut expected = crate::tui::theme::Theme::empty("flawed", "flawed");
        expected.ansi[2] = Some((0x69, 0xff, 0x94));
        expected.accent = Some((0xff, 0x79, 0xc6));
        assert_eq!(theme, &expected, "only the two good colours are kept");
        assert_eq!(
            themes[1],
            crate::tui::theme::Theme::empty("wrong-types", "wrong-types")
        );

        for warning in &warnings {
            assert!(!warning.contains('\n'), "a warning is one line: {warning:?}");
        }
        let flawed: Vec<&String> = warnings
            .iter()
            .filter(|msg| msg.contains("flawed.json"))
            .collect();
        let mistakes = [
            "no-such-base",
            "unknownTop",
            "purple",
            "not-a-hex",
            "#+1+2+3",
            "#é1",
            "cyan",
            "bogusRole",
            "#12345",
            "no-such-syntax",
        ];
        for expected in mistakes {
            assert!(
                flawed.iter().any(|msg| msg.contains(expected)),
                "missing warning for {expected:?} in {warnings:?}"
            );
        }
        assert_eq!(flawed.len(), mistakes.len(), "one line a mistake: {flawed:?}");

        let typed: Vec<&String> = warnings
            .iter()
            .filter(|msg| msg.contains("wrong-types.json"))
            .collect();
        for key in ["base", "ansi", "colors", "syntax"] {
            assert!(
                typed.iter().any(|msg| msg.contains(key)),
                "missing warning for the type of {key:?} in {warnings:?}"
            );
        }
        assert_eq!(typed.len(), 4, "one line a mistake: {typed:?}");
        assert_eq!(warnings.len(), mistakes.len() + 4, "{warnings:?}");
    }

    #[test]
    fn an_unreadable_user_theme_is_absent_and_named_in_a_warning() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_theme(dir.path(), "bad-json.json", "{ not valid json");
        let oversize = usize::try_from(MAX_THEME_FILE_BYTES).expect("fits usize") + 1;
        let big_body = format!(
            "{{\"colors\":{{\"accent\":\"#112233\"}},\"pad\":\"{}\"}}",
            "x".repeat(oversize)
        );
        write_theme(dir.path(), "too-big.json", &big_body);
        write_theme(dir.path(), "array.json", r#"["dracula"]"#);
        // Exactly 256 KiB is still read.
        let fits = usize::try_from(MAX_THEME_FILE_BYTES).expect("fits usize");
        write_theme(
            dir.path(),
            "just-fits.json",
            &format!("{{}}{}", " ".repeat(fits - 2)),
        );

        let (themes, warnings) = load_from(dir.path());
        let names: Vec<&str> = themes.iter().map(|theme| theme.name.as_str()).collect();
        assert_eq!(names, ["just-fits"], "unreadable themes must be absent");

        let mut catalog = Catalog::new();
        let (list, _) = catalog.names(Some(dir.path()));
        assert_eq!(list.len(), 32, "{list:?}");
        for slug in ["bad-json", "too-big", "array"] {
            assert!(
                catalog.find(slug).is_none(),
                "{slug} must be absent from catalog"
            );
            let file_name = format!("{slug}.json");
            assert!(
                warnings.iter().any(|msg| msg.contains(&file_name)),
                "missing warning naming {file_name} in {warnings:?}"
            );
        }
        assert!(
            warnings.iter().any(|msg| msg.contains("too-big.json") && msg.contains("256 KiB")),
            "the size warning says the limit: {warnings:?}"
        );
        assert_eq!(warnings.len(), 3, "{warnings:?}");
    }

    #[test]
    fn a_user_theme_named_like_a_builtin_replaces_it_and_terminal_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_theme(
            dir.path(),
            "dracula.json",
            r##"{ "base": "dracula", "colors": { "accent": "#123456" } }"##,
        );
        write_theme(
            dir.path(),
            "terminal.json",
            r##"{ "colors": { "accent": "#abcdef" } }"##,
        );

        let (themes, warnings) = load_from(dir.path());
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "dracula");
        assert!(
            warnings.iter().any(|msg| msg.contains("terminal.json")),
            "terminal.json override must warn: {warnings:?}"
        );

        let mut catalog = Catalog::new();
        let (list, _) = catalog.names(Some(dir.path()));
        assert_eq!(list.len(), 31, "overridden builtin appears once: {list:?}");
        assert_eq!(list.iter().filter(|name| *name == "dracula").count(), 1);

        let dracula = catalog.find("dracula").expect("user dracula");
        assert_eq!(dracula.accent, Some((0x12, 0x34, 0x56)));
        assert_eq!(dracula.ansi[1], Some((0xff, 0x55, 0x55)));

        let terminal = catalog.find("terminal").expect("terminal");
        assert!(terminal.is_terminal());
        assert_eq!(terminal.accent, None);
    }

    #[test]
    fn theme_file_names_must_be_slugs() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_theme(dir.path(), "Upper.json", "{}");
        write_theme(dir.path(), "with space.json", "{}");
        write_theme(dir.path(), "two..dots.json", "{}");
        write_theme(dir.path(), "good-slug.json", "{}");
        // Not theme files at all: nothing to say.
        write_theme(dir.path(), ".DS_Store", "\0\0");
        write_theme(dir.path(), "notes.txt", "my themes");

        let sub = dir.path().join("subdir");
        fs::create_dir_all(&sub).expect("create subdir");
        write_theme(&sub, "nested.json", "{}");
        fs::create_dir_all(dir.path().join("folder.json")).expect("create a folder named .json");

        #[cfg(unix)]
        let _outside_dir = {
            let outside = tempfile::tempdir().expect("outside tempdir");
            write_theme(outside.path(), "secret.json", "{}");
            std::os::unix::fs::symlink(
                dir.path().join("good-slug.json"),
                dir.path().join("inside-link.json"),
            )
            .expect("inside symlink");
            std::os::unix::fs::symlink(
                outside.path().join("secret.json"),
                dir.path().join("outside-link.json"),
            )
            .expect("outside symlink");
            outside
        };

        let (themes, warnings) = load_from(dir.path());
        let names: Vec<&str> = themes.iter().map(|theme| theme.name.as_str()).collect();
        assert!(names.contains(&"good-slug"), "missing good-slug: {names:?}");
        #[cfg(unix)]
        {
            assert_eq!(names, ["good-slug", "inside-link"], "the link inside is followed");
            assert!(
                warnings.iter().any(|msg| msg.contains("outside-link.json")),
                "a link outside the folder is skipped and said: {warnings:?}"
            );
        }
        #[cfg(not(unix))]
        assert_eq!(names, ["good-slug"]);
        for bad_file in ["Upper.json", "with space.json", "two..dots.json"] {
            assert!(
                warnings.iter().any(|msg| msg.contains(bad_file)),
                "missing slug warning for {bad_file} in {warnings:?}"
            );
        }
        for quiet in ["nested.json", "folder.json", ".DS_Store", "notes.txt"] {
            assert!(
                !warnings.iter().any(|msg| msg.contains(quiet)),
                "{quiet} is not a theme file and is not said: {warnings:?}"
            );
        }
        let expected_warnings = if cfg!(unix) { 4 } else { 3 };
        assert_eq!(warnings.len(), expected_warnings, "{warnings:?}");
    }

    #[test]
    fn the_themes_folder_is_read_again_when_names_are_asked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut catalog = Catalog::new();

        let (initial, _) = catalog.names(Some(dir.path()));
        assert_eq!(initial.len(), 31);
        assert!(catalog.find("beta").is_none());

        write_theme(dir.path(), "beta.json", r##"{"colors":{"accent":"#112233"}}"##);
        let (after_add, _) = catalog.names(Some(dir.path()));
        assert_eq!(after_add.len(), 32);
        assert_eq!(after_add[31], "beta");
        assert_eq!(
            catalog.find("beta").expect("beta").accent,
            Some((0x11, 0x22, 0x33))
        );

        write_theme(dir.path(), "alpha.json", r##"{"colors":{"accent":"#445566"}}"##);
        write_theme(dir.path(), "beta.json", r##"{"colors":{"accent":"#778899"}}"##);
        let (after_edit, _) = catalog.names(Some(dir.path()));
        assert_eq!(after_edit.len(), 33);
        assert_eq!(after_edit[31], "alpha");
        assert_eq!(after_edit[32], "beta");
        assert_eq!(
            catalog.find("beta").expect("updated beta").accent,
            Some((0x77, 0x88, 0x99))
        );

        fs::remove_file(dir.path().join("alpha.json")).expect("remove alpha");
        let (after_remove, _) = catalog.names(Some(dir.path()));
        assert_eq!(after_remove.len(), 32);
        assert_eq!(after_remove[31], "beta");
        assert!(catalog.find("alpha").is_none());

        // A theme's name sorts by the name, not by the file name.
        write_theme(dir.path(), "a.json", "{}");
        write_theme(dir.path(), "a-b.json", "{}");
        let (sorted, _) = catalog.names(Some(dir.path()));
        assert_eq!(&sorted[31..], ["a", "a-b", "beta"]);
    }

    #[test]
    fn a_mistake_is_said_once_until_the_file_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_theme(dir.path(), "oops.json", r#"{ "colours": {} }"#);

        let mut catalog = Catalog::new();
        let (_, first) = catalog.names(Some(dir.path()));
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(first[0].contains("oops.json") && first[0].contains("colours"));
        let (_, again) = catalog.names(Some(dir.path()));
        assert!(again.is_empty(), "the picker opened again says nothing new: {again:?}");

        write_theme(dir.path(), "oops.json", "{}");
        let (_, fixed) = catalog.names(Some(dir.path()));
        assert!(fixed.is_empty(), "{fixed:?}");
        write_theme(dir.path(), "oops.json", r#"{ "colours": {} }"#);
        let (_, broken_again) = catalog.names(Some(dir.path()));
        assert_eq!(broken_again.len(), 1, "a mistake made again is said again");

        // What the read at start said, the first picker does not say twice.
        let mut started = Catalog::new();
        assert_eq!(started.read(Some(dir.path())).len(), 1);
        assert!(started.names(Some(dir.path())).1.is_empty());
    }

    #[test]
    fn without_a_folder_there_are_only_the_built_in_themes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("no-themes-here");
        assert_eq!(load_from(&missing), (Vec::new(), Vec::new()));

        let mut catalog = Catalog::new();
        let (list, warnings) = catalog.names(None);
        assert_eq!(list, crate::tui::theme::builtin_names());
        assert!(warnings.is_empty());
        assert!(catalog.find("dracula").is_some_and(|theme| theme == find_builtin("dracula").expect("dracula")));
        assert!(catalog.find("Dracula").is_none());
        // The process catalog in a test build reads no folder at all.
        assert_eq!(crate::tui::theme::names(), crate::tui::theme::builtin_names());
    }
}
