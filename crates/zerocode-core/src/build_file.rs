//! A project's own builds and their order: `.zo/build.toml` under a
//! checkout's root. `zo build` runs them top to bottom, and the board shows
//! every build of the project in this order; a build some other script
//! reports (`source = "report"`) keeps its place here and runs nothing.
//!
//! ```toml
//! [[build]]
//! id = "app"
//! name = "앱 빌드"             # or one per language: { ko = "앱 빌드", en = "App build" }
//! tool = "gradle"              # whose output lines tell a step's progress
//! step = [
//!   { id = "compile", name = "컴파일", run = "./gradlew compileJava" },
//!   { id = "unit", name = "단위 시험", run = "./gradlew test", cap = "20m" },
//! ]
//!
//! [[build]]
//! id = "image"
//! after = "app"                # runs once `app` has passed
//! step = [
//!   { id = "build", run = "docker build -t shop:${sha8} ." },
//!   { id = "push", run = "docker push shop:${sha8}", when = "manual" },
//! ]
//! ```
//!
//! One reading for the window and for `zo build` ([`parse`]), one set of rules
//! for a file read and for one the order editor drew ([`check`]), and one
//! writing ([`to_toml`]). A file that could not be run exactly as written — an
//! unknown key, an id used twice, an `after` that names no build above it, a
//! placeholder no run fills — is refused in one sentence, never half read.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::atomic_file::write_atomically;
use crate::builds::{BuildName, Text, is_build_id, joined, read_bounded, revision_of};
use crate::file_stamp::stamp_plain;

/// The file, under a checkout's root.
pub const FILE: [&str; 2] = [".zo", "build.toml"];

/// What a run fills into a command: the commit's full id, its first eight
/// characters, and the version the run is labelled with.
pub const PLACEHOLDERS: [&str; 3] = ["sha", "sha8", "version"];

/// The longest a language key of a name may be (`ko`, `en`, `zh-Hant`).
const LANGUAGE_LONGEST: usize = 16;

/// The longest a cap may be: a day.
const CAP_SECS_MOST: u32 = 24 * 60 * 60;

/// The whole file: its builds, top to bottom.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildFile {
    #[serde(default, rename = "build", skip_serializing_if = "Vec::is_empty")]
    pub builds: Vec<BuildSpec>,
}

/// One build: its id (the folder its records stand in), what to call it, the
/// tool that builds it, the build it waits for, who runs it, and its steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpec {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// The folder under the checkout its steps run in (`backend`, `apps/web`):
    /// a project in a monorepo's subfolder. None runs them at the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(default, skip_serializing_if = "Source::runs_here")]
    pub source: Source,
    #[serde(default, rename = "step", skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepSpec>,
    /// A build another script reports may say how to run it again and how to
    /// stop it: the board's 「다시」 and 「멈추기」 run these through
    /// `zo build act`, after a person allowed them like any step's command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<String>,
}

impl BuildSpec {
    /// The command of `act` this build names (a reported build's
    /// [`retry`](Self::retry) or [`stop`](Self::stop)).
    #[must_use]
    pub fn act_command(&self, act: Act) -> Option<&str> {
        match act {
            Act::Retry => self.retry.as_deref(),
            Act::Stop => self.stop.as_deref(),
        }
        .filter(|command| !command.trim().is_empty())
    }
}

/// What a person asks of a build another script reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Act {
    Retry,
    Stop,
}

impl Act {
    /// The act a word names: `retry` or `stop`.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        match word {
            "retry" => Some(Self::Retry),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::Stop => "stop",
        }
    }
}

/// Who runs a build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// `zo build`, from the steps' commands.
    #[default]
    Run,
    /// Another script, through `zo build report`: the file holds its place.
    Report,
}

impl Source {
    #[must_use]
    pub const fn runs_here(&self) -> bool {
        matches!(self, Self::Run)
    }
}

/// One step: its id, what to call it, its command (and the one for Windows,
/// when that differs), the longest it may take, and whether it waits for a
/// person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepSpec {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Text>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
    /// `90s`, `20m`, `2h`: past it the step has failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<String>,
    #[serde(default, skip_serializing_if = "When::always")]
    pub when: When,
}

/// When a step runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    /// Every run, in its place.
    #[default]
    Always,
    /// Only when a person presses it.
    Manual,
}

impl When {
    #[must_use]
    pub const fn always(&self) -> bool {
        matches!(self, Self::Always)
    }
}

/// The file `text` holds, when it can be run exactly as written.
///
/// # Errors
///
/// One sentence: why the file does not read as TOML of this shape, or which
/// of [`check`]'s rules it breaks.
pub fn parse(text: &str) -> Result<BuildFile, String> {
    let file: BuildFile = toml::from_str(text)
        .map_err(|error| format!("the build file does not read: {}", error.message()))?;
    check(&file)?;
    Ok(file)
}

/// Whether `file` can be run exactly as written: every id a folder name and
/// used once, every `after` a build above it, every step of a build `zo build`
/// runs a command, every cap a length, every placeholder one a run fills, and
/// every name's language a short key.
///
/// # Errors
///
/// The first rule broken, in one sentence that names the build and step.
pub fn check(file: &BuildFile) -> Result<(), String> {
    let mut above: HashSet<&str> = HashSet::new();
    for build in &file.builds {
        let id = build.id.as_str();
        if !is_build_id(id) {
            return Err(format!(
                "`{id}` cannot name a build: letters, digits, `.`, `_` and `-`, led by a letter or a digit"
            ));
        }
        if above.contains(id) {
            return Err(format!("the build `{id}` is named twice"));
        }
        if let Some(after) = build.after.as_deref()
            && !above.contains(after)
        {
            return Err(format!(
                "the build `{id}` waits for `{after}`, which is not a build above it"
            ));
        }
        if let Some(tool) = build.tool.as_deref()
            && !is_build_id(tool)
        {
            return Err(format!(
                "the build `{id}` names its tool `{tool}`, which is not an id"
            ));
        }
        check_name(build.name.as_ref(), id)?;
        if let Some(dir) = build.dir.as_deref()
            && !is_folder_under(dir)
        {
            return Err(format!(
                "the build `{id}` runs in `{dir}`, which is not a folder under the checkout (a relative path with no `..`)"
            ));
        }
        if build.source.runs_here() && build.steps.is_empty() {
            return Err(format!("the build `{id}` has no step to run"));
        }
        for (key, command) in [
            ("retry", build.retry.as_deref()),
            ("stop", build.stop.as_deref()),
        ] {
            let Some(command) = command else { continue };
            if build.source.runs_here() {
                return Err(format!(
                    "the build `{id}` names `{key}`, which only a build another script reports does (`source = \"report\"`); `zo build` runs and stops its own"
                ));
            }
            if command.trim().is_empty() {
                return Err(format!("the build `{id}` has an empty `{key}` command"));
            }
            for name in placeholders(command)? {
                if !PLACEHOLDERS.contains(&name) {
                    return Err(format!(
                        "the `{key}` command of `{id}` uses `${{{name}}}`, which no run fills (only {})",
                        PLACEHOLDERS.map(|one| format!("`${{{one}}}`")).join(", ")
                    ));
                }
            }
        }
        let mut steps: HashSet<&str> = HashSet::new();
        for step in &build.steps {
            check_step(build, step)?;
            if !steps.insert(step.id.as_str()) {
                return Err(format!(
                    "the build `{id}` names its step `{}` twice",
                    step.id
                ));
            }
        }
        above.insert(id);
    }
    Ok(())
}

/// A folder a build may run in: relative, each part a plain name — never the
/// root itself, a parent, or a path that leaves the checkout.
#[must_use]
pub fn is_folder_under(dir: &str) -> bool {
    let path = std::path::Path::new(dir);
    !dir.trim().is_empty()
        && !dir.contains('\\')
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

impl BuildSpec {
    /// The folder its steps run in: `checkout`, or the build's `dir` under it.
    #[must_use]
    pub fn folder(&self, checkout: &std::path::Path) -> std::path::PathBuf {
        match self.dir.as_deref().filter(|dir| is_folder_under(dir)) {
            Some(dir) => checkout.join(dir),
            None => checkout.to_path_buf(),
        }
    }
}

fn check_step(build: &BuildSpec, step: &StepSpec) -> Result<(), String> {
    let (id, step_id) = (build.id.as_str(), step.id.as_str());
    if !is_build_id(step_id) {
        return Err(format!(
            "the build `{id}` has a step `{step_id}`: letters, digits, `.`, `_` and `-`, led by a letter or a digit"
        ));
    }
    check_name(step.name.as_ref(), id)?;
    let commands = [step.run.as_deref(), step.windows.as_deref()];
    if !build.source.runs_here() {
        if commands.iter().any(Option::is_some) {
            return Err(format!(
                "the build `{id}` is reported by another script, so its step `{step_id}` runs nothing here"
            ));
        }
        return Ok(());
    }
    if step.run.as_deref().is_none_or(|run| run.trim().is_empty()) {
        return Err(format!(
            "the step `{step_id}` of `{id}` has no command (`run`)"
        ));
    }
    for command in commands.into_iter().flatten() {
        for name in placeholders(command)? {
            if !PLACEHOLDERS.contains(&name) {
                return Err(format!(
                    "the step `{step_id}` of `{id}` uses `${{{name}}}`, which no run fills (only {})",
                    PLACEHOLDERS.map(|one| format!("`${{{one}}}`")).join(", ")
                ));
            }
        }
    }
    if let Some(cap) = step.cap.as_deref()
        && cap_secs(cap).is_none()
    {
        return Err(format!(
            "the step `{step_id}` of `{id}` caps itself at `{cap}`: a number and `s`, `m` or `h`, up to a day"
        ));
    }
    Ok(())
}

fn check_name(name: Option<&Text>, build: &str) -> Result<(), String> {
    let Some(Text::ByLanguage(words)) = name else {
        return Ok(());
    };
    match words.keys().find(|key| !is_language_key(key)) {
        Some(key) => Err(format!(
            "the build `{build}` names a language `{key}`, which is not a short key such as `ko` or `en`"
        )),
        None => Ok(()),
    }
}

fn is_language_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= LANGUAGE_LONGEST
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The names `command` asks to be filled, in order, each `${name}` once per
/// use.
///
/// # Errors
///
/// When a `${` is never closed.
pub fn placeholders(command: &str) -> Result<Vec<&str>, String> {
    let mut names = Vec::new();
    let mut rest = command;
    while let Some(at) = rest.find("${") {
        let after = &rest[at + 2..];
        let Some(end) = after.find('}') else {
            return Err(format!("`{command}` opens a `${{` it never closes"));
        };
        names.push(&after[..end]);
        rest = &after[end + 1..];
    }
    Ok(names)
}

/// `command` with each `${name}` replaced by its value in `values`; a name
/// with no value is left as it was written, for the caller to refuse.
#[must_use]
pub fn fill(command: &str, values: &BTreeMap<&str, String>) -> String {
    let mut filled = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(at) = rest.find("${") {
        filled.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(end) = after.find('}') else {
            break;
        };
        let name = &after[..end];
        match values.get(name) {
            Some(value) => filled.push_str(value),
            None => {
                filled.push_str("${");
                filled.push_str(name);
                filled.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    filled.push_str(rest);
    filled
}

/// The seconds a cap such as `90s`, `20m` or `2h` allows, when it is one
/// number and one unit, above none and up to a day.
#[must_use]
pub fn cap_secs(cap: &str) -> Option<u32> {
    let cap = cap.trim();
    let unit = cap.chars().last()?;
    let per = match unit {
        's' => 1,
        'm' => 60,
        'h' => 60 * 60,
        _ => return None,
    };
    let number: u32 = cap[..cap.len() - 1].parse().ok()?;
    let secs = number.checked_mul(per)?;
    (secs > 0 && secs <= CAP_SECS_MOST).then_some(secs)
}

/// The text of the file at `path`: none when nothing stands there.
///
/// # Errors
///
/// When something other than a plain file stands there, or the file is too
/// large or not UTF-8 text.
pub fn read_text(path: &Path) -> Result<Option<String>, String> {
    if stamp_plain(path).is_none() {
        return match std::fs::symlink_metadata(path) {
            Ok(_) => Err(format!("{} is not a plain file", path.display())),
            Err(_) => Ok(None),
        };
    }
    let bytes = read_bounded(path).map_err(|error| error.to_string())?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| format!("{} is not UTF-8 text", path.display()))
}

/// The order file of `checkout`, as `zo build` runs it.
///
/// # Errors
///
/// When the checkout has none, or it cannot be read or run as written.
pub fn read(checkout: &Path) -> Result<BuildFile, String> {
    let name = FILE.join("/");
    match read_text(&joined(checkout, &FILE))? {
        Some(text) => parse(&text).map_err(|reason| format!("{name}: {reason}")),
        None => Err(format!("{} has no {name}", checkout.display())),
    }
}

/// The number a text is known by: what the order editor hands back so that a
/// write never lands on a file that changed since it was opened.
#[must_use]
pub fn version_of(text: &str) -> u64 {
    revision_of(&text)
}

/// Where the order editor starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftFrom {
    /// The checkout's own file.
    File,
    /// What the checkout's root suggests: it has no file yet.
    Suggestion,
    /// A file that cannot be run as written: the person fixes it by hand.
    Refused,
}

/// What the order editor opens on: the builds, where they come from, the
/// file's path to open and its name under the checkout to say, the version of
/// its text (none with no file), whether it holds comments a written order
/// would not keep, and why a refused file is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Draft {
    pub from: DraftFrom,
    pub file: BuildFile,
    pub path: String,
    pub name: String,
    pub version: Option<u64>,
    pub comments: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// The order editor's start for `checkout`: its own file when it reads, else
/// what `suggestion` drafts for a checkout with none. A build that keeps runs
/// (`recorded`) and is not in it joins at its end as one another script
/// reports, so that writing the order gives it its place.
pub fn draft(
    checkout: &Path,
    suggestion: impl FnOnce() -> BuildFile,
    recorded: &[BuildName],
) -> Draft {
    let path = joined(checkout, &FILE);
    let start = |from, file, version, comments, reason| Draft {
        from,
        file,
        path: path.display().to_string(),
        name: FILE.join("/"),
        version,
        comments,
        reason,
    };
    match read_text(&path) {
        Ok(None) => start(
            DraftFrom::Suggestion,
            with_recorded(suggestion(), recorded),
            None,
            false,
            String::new(),
        ),
        Ok(Some(text)) => {
            let (version, comments) = (Some(version_of(&text)), has_comments(&text));
            match parse(&text) {
                Ok(file) => start(
                    DraftFrom::File,
                    with_recorded(file, recorded),
                    version,
                    comments,
                    String::new(),
                ),
                Err(reason) => start(
                    DraftFrom::Refused,
                    BuildFile::default(),
                    version,
                    comments,
                    reason,
                ),
            }
        }
        Err(reason) => start(
            DraftFrom::Refused,
            BuildFile::default(),
            None,
            false,
            reason,
        ),
    }
}

/// `file` with each of `recorded` it does not name added at its end as a
/// build another script reports: with its name and tool where those can stand
/// in a file, by its id alone where they cannot.
fn with_recorded(mut file: BuildFile, recorded: &[BuildName]) -> BuildFile {
    for build in recorded {
        if file.builds.iter().any(|named| named.id == build.id) {
            continue;
        }
        let reported = |name: Option<Text>, tool: Option<String>| BuildSpec {
            id: build.id.clone(),
            name,
            tool,
            after: None,
            dir: None,
            source: Source::Report,
            steps: Vec::new(),
            retry: None,
            stop: None,
        };
        let stands = |spec: &BuildSpec| {
            check(&BuildFile {
                builds: vec![spec.clone()],
            })
            .is_ok()
        };
        let named = reported(build.name.clone(), build.tool.clone());
        let bare = reported(None, None);
        if stands(&named) {
            file.builds.push(named);
        } else if stands(&bare) {
            file.builds.push(bare);
        }
    }
    file
}

/// Writes `file` as `checkout`'s `.zo/build.toml` and answers the text
/// written — when it can be run as written, and the file still holds the text
/// `version` was taken of (none: there was no file).
///
/// # Errors
///
/// Which of [`check`]'s rules `file` breaks, that the file changed since, or
/// why it could not be written.
pub fn write(checkout: &Path, file: &BuildFile, version: Option<u64>) -> Result<String, String> {
    check(file)?;
    let path = joined(checkout, &FILE);
    if read_text(&path)?.as_deref().map(version_of) != version {
        return Err(format!(
            "{} changed since the order was opened: open it again",
            path.display()
        ));
    }
    let text = to_toml(file);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("{}: {error}", folder.display()))?;
    }
    write_atomically(&path, text.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(text)
}

/// Whether `text` holds a comment: a `#` outside a string.
fn has_comments(text: &str) -> bool {
    text.lines().any(|line| {
        let mut quote = None;
        let mut escaped = false;
        for c in line.chars() {
            match quote {
                Some('"') if escaped => escaped = false,
                Some('"') if c == '\\' => escaped = true,
                Some(open) if c == open => quote = None,
                Some(_) => {}
                None if c == '#' => return true,
                None if c == '"' || c == '\'' => quote = Some(c),
                None => {}
            }
        }
        false
    })
}

/// The file `file` is, as [`parse`] reads it back: each build a table, its
/// steps one inline table a line, in the order the file keeps.
#[must_use]
pub fn to_toml(file: &BuildFile) -> String {
    let mut text = String::new();
    for (at, build) in file.builds.iter().enumerate() {
        if at > 0 {
            text.push('\n');
        }
        text.push_str("[[build]]\n");
        let _ = writeln!(text, "id = {}", quoted(&build.id));
        if let Some(name) = &build.name {
            let _ = writeln!(text, "name = {}", words(name));
        }
        if let Some(tool) = &build.tool {
            let _ = writeln!(text, "tool = {}", quoted(tool));
        }
        if let Some(after) = &build.after {
            let _ = writeln!(text, "after = {}", quoted(after));
        }
        if let Some(dir) = &build.dir {
            let _ = writeln!(text, "dir = {}", quoted(dir));
        }
        if build.source == Source::Report {
            text.push_str("source = \"report\"\n");
        }
        if let Some(retry) = &build.retry {
            let _ = writeln!(text, "retry = {}", quoted(retry));
        }
        if let Some(stop) = &build.stop {
            let _ = writeln!(text, "stop = {}", quoted(stop));
        }
        if !build.steps.is_empty() {
            text.push_str("step = [\n");
            for step in &build.steps {
                let _ = writeln!(text, "  {},", step_table(step));
            }
            text.push_str("]\n");
        }
    }
    text
}

fn step_table(step: &StepSpec) -> String {
    let mut pairs = vec![format!("id = {}", quoted(&step.id))];
    if let Some(name) = &step.name {
        pairs.push(format!("name = {}", words(name)));
    }
    for (key, value) in [
        ("run", &step.run),
        ("windows", &step.windows),
        ("cap", &step.cap),
    ] {
        if let Some(value) = value {
            pairs.push(format!("{key} = {}", quoted(value)));
        }
    }
    if step.when == When::Manual {
        pairs.push("when = \"manual\"".to_owned());
    }
    format!("{{ {} }}", pairs.join(", "))
}

fn words(name: &Text) -> String {
    match name {
        Text::One(text) => quoted(text),
        Text::ByLanguage(by_language) => {
            let pairs: Vec<String> = by_language
                .iter()
                .map(|(language, text)| format!("{language} = {}", quoted(text)))
                .collect();
            format!("{{ {} }}", pairs.join(", "))
        }
    }
}

/// `text` as a TOML basic string.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &str = r#"
[[build]]
id = "app"
name = { ko = "앱 빌드", en = "App build" }
tool = "gradle"
step = [
  { id = "compile", name = "컴파일", run = "./gradlew compileJava" },
  { id = "unit", name = "단위 시험", run = "./gradlew test", cap = "20m" },
]

[[build]]
id = "image"
name = "도커 이미지"
tool = "docker"
after = "app"
step = [
  { id = "build", run = "docker build -t shop:${sha8} .", windows = "docker build -t shop:${sha8} ." },
  { id = "push", run = "docker push shop:${sha8}", when = "manual" },
]

[[build]]
id = "release"
after = "image"
source = "report"
"#;

    #[test]
    fn a_file_reads_as_its_builds_in_order_and_writes_back_as_itself() {
        let file = parse(SERVER).expect("the file reads");
        assert_eq!(
            file.builds
                .iter()
                .map(|build| build.id.as_str())
                .collect::<Vec<_>>(),
            ["app", "image", "release"]
        );
        let image = &file.builds[1];
        assert_eq!(image.after.as_deref(), Some("app"));
        assert_eq!(image.steps[1].when, When::Manual);
        assert_eq!(file.builds[2].source, Source::Report);
        assert!(file.builds[2].steps.is_empty());
        assert_eq!(
            file.builds[0].steps[1].cap.as_deref().and_then(cap_secs),
            Some(1200)
        );
        let written = to_toml(&file);
        assert_eq!(parse(&written), Ok(file.clone()), "{written}");
        assert_eq!(to_toml(&parse(&written).expect("again")), written);
    }

    #[test]
    fn words_a_person_wrote_come_back_exactly() {
        let tricky = "say \"hi\" \\ back\tthen\nnew line \u{7} bell — 한국어";
        let file = BuildFile {
            builds: vec![BuildSpec {
                id: "odd".into(),
                name: Some(Text::One(tricky.into())),
                tool: None,
                after: None,
                dir: None,
                source: Source::Run,
                steps: vec![StepSpec {
                    id: "one".into(),
                    name: None,
                    run: Some(format!("echo '{tricky}'")),
                    windows: None,
                    cap: None,
                    when: When::Always,
                }],
                retry: None,
                stop: None,
            }],
        };
        assert_eq!(parse(&to_toml(&file)), Ok(file));
    }

    #[test]
    fn a_file_that_could_not_run_as_written_is_refused_in_one_sentence() {
        let refused = |text: &str, says: &str| {
            let error = parse(text).expect_err(text);
            assert!(error.contains(says), "{error:?} does not say {says:?}");
        };
        let one = |steps: &str| format!("[[build]]\nid = \"app\"\nstep = [{steps}]\n");
        refused(
            "[[build]]\nid = \"app\"\nnmae = \"typo\"\nstep = [{ id = \"a\", run = \"x\" }]\n",
            "nmae",
        );
        refused(
            "[[build]]\nid = \"../up\"\nstep = [{ id = \"a\", run = \"x\" }]\n",
            "cannot name a build",
        );
        refused(
            &format!(
                "{}{}",
                one("{ id = \"a\", run = \"x\" }"),
                one("{ id = \"a\", run = \"x\" }")
            ),
            "named twice",
        );
        refused(
            "[[build]]\nid = \"image\"\nafter = \"app\"\nstep = [{ id = \"a\", run = \"x\" }]\n",
            "not a build above it",
        );
        refused("[[build]]\nid = \"app\"\n", "no step to run");
        refused(&one("{ id = \"a\" }"), "has no command");
        refused(
            &one("{ id = \"a\", run = \"x\" }, { id = \"a\", run = \"y\" }"),
            "twice",
        );
        refused(
            &one("{ id = \"a\", run = \"echo ${home}\" }"),
            "which no run fills",
        );
        refused(
            &one("{ id = \"a\", run = \"echo ${sha8\" }"),
            "never closes",
        );
        refused(
            &one("{ id = \"a\", run = \"x\", cap = \"soon\" }"),
            "caps itself",
        );
        refused(
            &one("{ id = \"a\", run = \"x\", when = \"sometimes\" }"),
            "the build file does not read",
        );
        refused(
            "[[build]]\nid = \"r\"\nsource = \"report\"\nstep = [{ id = \"a\", run = \"x\" }]\n",
            "runs nothing here",
        );
        refused(
            "[[build]]\nid = \"app\"\nname = { \"k o\" = \"x\" }\nstep = [{ id = \"a\", run = \"x\" }]\n",
            "not a short key",
        );
    }

    #[test]
    fn the_editor_opens_on_the_file_or_the_suggestion_and_never_writes_over_a_moved_file() {
        let checkout = tempfile::tempdir().expect("a checkout");
        let root = checkout.path();
        let suggestion = || parse(SERVER).expect("the suggestion");
        let first = draft(root, suggestion, &[]);
        assert_eq!((first.from, first.version), (DraftFrom::Suggestion, None));
        assert_eq!(first.file, suggestion());

        let written = write(root, &first.file, first.version).expect("the first write");
        let opened = draft(root, || unreachable!("the checkout has its own file"), &[]);
        assert_eq!(opened.from, DraftFrom::File);
        assert_eq!(opened.file, first.file);
        assert_eq!(opened.version, Some(version_of(&written)));
        assert!(!opened.comments);

        let mut reordered = opened.file.clone();
        reordered.builds.swap(0, 1);
        assert!(
            write(root, &reordered, opened.version)
                .expect_err("an `after` that is no longer above")
                .contains("not a build above it")
        );
        reordered.builds[0].after = None;
        reordered.builds[1].after = None;
        assert!(
            write(root, &reordered, None)
                .expect_err("a version taken of no file")
                .contains("changed since")
        );
        write(root, &reordered, opened.version).expect("the reordered write");

        // A file the person edited by hand, with a comment: the editor says the
        // comment would go, and an old version never lands on it.
        let path = root.join(".zo/build.toml");
        let hand = format!(
            "# the order\n{}",
            std::fs::read_to_string(&path).expect("the file")
        );
        std::fs::write(&path, &hand).expect("a hand edit");
        assert!(draft(root, || unreachable!(), &[]).comments);
        assert!(write(root, &reordered, opened.version).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("the file"), hand);

        std::fs::write(&path, "[[build]]\nid = \"app\"\n").expect("a broken file");
        let broken = draft(root, || unreachable!(), &[]);
        assert_eq!(broken.from, DraftFrom::Refused);
        assert!(
            broken.reason.contains("no step to run"),
            "{}",
            broken.reason
        );
    }

    /// zerocode's case: a build another script reports keeps runs, and the
    /// order file does not name it yet. The draft gives it a place at the end,
    /// with its name and tool — its id alone when its tool is no id — and a
    /// build the file already names is not added twice.
    /// 「다시」·「멈추기」 for a build another script reports (11-08): it may name
    /// its own retry and stop commands, with the placeholders any step fills; a
    /// build `zo build` runs may not, and the file writes them back as it read them.
    #[test]
    fn a_build_runs_in_a_folder_under_the_checkout_and_nowhere_else() {
        let file = |dir: &str| {
            parse(&format!(
                "[[build]]\nid = \"api\"\ndir = \"{dir}\"\nstep = [{{ id = \"test\", run = \"go test ./...\" }}]\n"
            ))
        };
        let written = file("services/api").expect("a build in a subfolder");
        assert_eq!(written.builds[0].dir.as_deref(), Some("services/api"));
        assert_eq!(
            written.builds[0].folder(std::path::Path::new("/repo")),
            std::path::Path::new("/repo/services/api")
        );
        assert_eq!(
            parse(&to_toml(&written)),
            Ok(written.clone()),
            "the writer keeps the folder"
        );
        for outside in ["../elsewhere", "/etc", "api/../..", ".", ""] {
            assert!(
                file(outside).is_err(),
                "`{outside}` is not a folder under the checkout, yet the file stood"
            );
        }
    }

    #[test]
    fn a_reported_build_may_name_its_retry_and_stop_and_a_run_build_may_not() {
        let text = "[[build]]\nid = \"release\"\nsource = \"report\"\nretry = \"tools/lane.sh ${sha}\"\nstop = \"tools/lane.sh stop\"\n";
        let file = parse(text).expect("a reported build with its acts");
        let release = &file.builds[0];
        assert_eq!(
            release.act_command(Act::Retry),
            Some("tools/lane.sh ${sha}")
        );
        assert_eq!(release.act_command(Act::Stop), Some("tools/lane.sh stop"));
        assert_eq!(parse(&to_toml(&file)).expect("written back"), file);
        assert_eq!(
            (Act::of("retry"), Act::of("stop"), Act::of("again")),
            (Some(Act::Retry), Some(Act::Stop), None)
        );

        let run = parse(
            "[[build]]\nid = \"app\"\nretry = \"make\"\nstep = [{ id = \"a\", run = \"make\" }]\n",
        )
        .expect_err("a build zo build runs names no retry");
        assert!(run.contains("only a build another script reports"), "{run}");
        let empty = parse("[[build]]\nid = \"r\"\nsource = \"report\"\nstop = \" \"\n")
            .expect_err("an empty stop");
        assert!(empty.contains("empty `stop`"), "{empty}");
        let unfilled =
            parse("[[build]]\nid = \"r\"\nsource = \"report\"\nretry = \"x ${branch}\"\n")
                .expect_err("a placeholder no run fills");
        assert!(unfilled.contains("`${branch}`"), "{unfilled}");
    }

    #[test]
    fn a_build_that_keeps_runs_joins_the_draft_as_one_another_script_reports() {
        let checkout = tempfile::tempdir().expect("a checkout");
        let root = checkout.path();
        let name = |id: &str, tool: &str| BuildName {
            id: id.into(),
            name: Some(Text::One(format!("{id} words"))),
            tool: Some(tool.to_owned()),
        };
        let recorded = [
            name("app", "gradle"),
            name("release", "lane"),
            name("docs", "mkdocs"),
            name("ci", "two words"),
        ];
        let drafted = draft(root, || parse(SERVER).expect("the suggestion"), &recorded);
        assert_eq!(
            drafted
                .file
                .builds
                .iter()
                .map(|build| build.id.as_str())
                .collect::<Vec<_>>(),
            ["app", "image", "release", "docs", "ci"]
        );
        let release = &drafted.file.builds[2];
        assert_eq!(
            (release.tool.as_deref(), release.after.as_deref()),
            (None, Some("image")),
            "a build the file names stays as the file says"
        );
        let docs = &drafted.file.builds[3];
        assert_eq!(docs.source, Source::Report);
        assert_eq!(docs.tool.as_deref(), Some("mkdocs"));
        assert_eq!(docs.name, Some(Text::One("docs words".into())));
        let ci = &drafted.file.builds[4];
        assert_eq!((ci.name.as_ref(), ci.tool.as_ref()), (None, None));
        assert_eq!(check(&drafted.file), Ok(()));
        assert!(drafted.path.ends_with("build.toml") && Path::new(&drafted.path).is_absolute());
        assert_eq!(drafted.name, FILE.join("/"));
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        assert!(!has_comments("run = \"echo '#1' \\\" # still\"\n"));
        assert!(has_comments("run = 'x' # why\n"));
        assert!(has_comments("  # the order\n"));
    }

    #[test]
    fn a_cap_is_one_number_and_one_unit_up_to_a_day() {
        assert_eq!(cap_secs("90s"), Some(90));
        assert_eq!(cap_secs("20m"), Some(1200));
        assert_eq!(cap_secs("2h"), Some(7200));
        assert_eq!(cap_secs("24h"), Some(CAP_SECS_MOST));
        for none in ["0m", "25h", "1h30m", "m", "", "-5m", "5 m", "5d"] {
            assert_eq!(cap_secs(none), None, "{none:?}");
        }
    }

    #[test]
    fn a_command_is_filled_with_the_runs_values_and_keeps_the_rest_as_written() {
        let values = BTreeMap::from([("sha8", "3e1c9a0d".to_owned())]);
        assert_eq!(
            fill(
                "docker build -t shop:${sha8} --label v=${version} .",
                &values
            ),
            "docker build -t shop:3e1c9a0d --label v=${version} ."
        );
        assert_eq!(
            placeholders("a ${sha} b ${sha8} ${sha}"),
            Ok(vec!["sha", "sha8", "sha"])
        );
    }
}
