//! zo's desk guard (t-104888): a shell command of the model's that would
//! move the person's screen — an input-injection tool, an `osascript` that
//! drives System Events or changes an app's windows and tabs, a one-line
//! program that injects input — is refused, and the model is sent back to the
//! Computer tool.
//!
//! On 2026-11-11 a zo session, refused twice by Computer Use, went around it
//! through the shell: `osascript` switched and raised Chrome's windows and
//! tabs, and `cliclick` clicked the real pointer by coordinates. Outside
//! Computer Use none of its safeguards — the person's hand, the share band,
//! the confirmation card — applied.
//!
//! A railing that keeps the model on its road, not a security boundary: it
//! reads only the names a command spells out. A script file, another
//! language's way around, a new tool get past it. Every name it knows is in
//! one table, and the command is split by the same reading the kill guard
//! uses ([`zerocode_core::shell_rule::runs::programs_run`]).
//!
//! It stands where the kill guard stands — [`crate::PermissionPolicy`]'s
//! `authorize_with_context` before the mode and the rules, and
//! [`crate::permission_enforcer::PermissionEnforcer::check_bash`] — right
//! after the kill guard's judgment, and refuses in every permission mode: no
//! mode, allow rule or hook answers it. Only in a session that can reach the
//! Computer tool (inside a pane of the window), and only while the person's
//! user settings leave [`SETTING`] on.

use zerocode_core::shell_rule::runs::{self, ProgramRun};

use crate::kill_guard::Shell;

/// The settings key a person sets `false` to turn the guard off, in their
/// user settings file only: a repository's `.zo/settings.json`, a workspace's
/// `.zo/settings.local.json` and a `--settings` file cannot (`crate::config`).
pub const SETTING: &str = "deskGuard";

/// Every name the guard knows, in one place: nothing outside it is
/// compared with a command's words.
pub(crate) struct Table {
    /// Programs that inject pointer or key input, by the name in command
    /// position (`.exe` taken off).
    pub(crate) input_tools: &'static [&'static str],
    /// The program that runs an `AppleScript`, and the flag that hands it a
    /// line of one.
    pub(crate) script_runner: &'static str,
    pub(crate) script_flag: &'static str,
    /// The app a screen script is said to.
    pub(crate) screen_app: &'static [&'static str],
    /// What a screen script tells it, word by word.
    pub(crate) screen_verbs: &'static [&'static [&'static str]],
    /// What a script tells an app that changes its windows or tabs.
    pub(crate) window_verbs: &'static [&'static [&'static str]],
    /// A window verb that opens these words does something else
    /// (`close access` closes a file).
    pub(crate) not_window_verbs: &'static [&'static [&'static str]],
    /// `set`, then — past an optional `the` — a property of a window, tab or
    /// element: a change of it, where `set <variable> to …` only fills a
    /// variable.
    pub(crate) set_verb: &'static str,
    pub(crate) article: &'static str,
    pub(crate) set_properties: &'static [&'static [&'static str]],
    /// The programs that run a one-line program, by name, and by the start
    /// of a name (`python3.12`, `pypy3`).
    pub(crate) interpreters: &'static [&'static str],
    pub(crate) interpreter_families: &'static [&'static str],
    /// The flags an interpreter is handed a one-line program by (`python3 -c`,
    /// `node -e`, `powershell -Command`), any case (`perl -E`).
    pub(crate) code_flags: &'static [&'static str],
    /// Names in a one-line program that inject input, any case.
    pub(crate) code_names: &'static [&'static str],
}

/// The table: design §3 (`reports/desk-guard/DESIGN.md`), row for row.
pub(crate) const TABLE: Table = Table {
    input_tools: &[
        "cliclick",
        "xdotool",
        "ydotool",
        "dotool",
        "wtype",
        "xte",
        "nircmd",
        "autohotkey",
        "autohotkey64",
        "ahk",
    ],
    script_runner: "osascript",
    script_flag: "-e",
    screen_app: &["system", "events"],
    screen_verbs: &[
        &["click"],
        &["keystroke"],
        &["key", "code"],
        &["perform", "action"],
        &["set", "frontmost"],
    ],
    window_verbs: &[
        &["activate"],
        &["close"],
        &["make", "new", "window"],
        &["make", "new", "tab"],
        &["make", "new", "document"],
        &["execute"],
        &["do", "javascript"],
    ],
    not_window_verbs: &[&["close", "access"]],
    set_verb: "set",
    article: "the",
    set_properties: &[
        &["active", "tab", "index"],
        &["active", "tab"],
        &["index"],
        &["frontmost"],
        &["bounds"],
        &["position"],
        &["size"],
        &["visible"],
        &["miniaturized"],
        &["zoomed"],
        &["url"],
        &["value"],
        &["selected"],
        &["focused"],
    ],
    interpreters: &["node", "ruby", "perl", "swift", "osascript", "pwsh", "powershell"],
    interpreter_families: &["python", "pypy"],
    code_flags: &["-c", "-e", "-command"],
    code_names: &[
        "pyautogui",
        "pynput",
        "CGEventPost",
        "SendKeys",
        "SendInput",
        "mouse_event",
    ],
};

/// The kind of screen-moving command the guard caught: one per row of
/// design §3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A program that injects pointer or key input.
    InputTool,
    /// An `osascript` that drives System Events.
    ScreenScript,
    /// An `osascript` that changes an app's windows or tabs.
    AppWindows,
    /// A one-line program that injects input.
    OneLiner,
}

impl Kind {
    /// How a refusal names it.
    const fn named(self) -> &'static str {
        match self {
            Self::InputTool => "an input-injection tool",
            Self::ScreenScript => "an AppleScript that drives System Events",
            Self::AppWindows => "an AppleScript that changes an app's windows or tabs",
            Self::OneLiner => "a one-line program that injects input",
        }
    }
}

/// What the guard caught in a command: the kind, and the words that showed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caught {
    pub kind: Kind,
    pub words: String,
}

/// What in `command`, as `shell` runs it, moves the person's screen; `None`
/// when nothing does. Reads the command only when it names one of the
/// table's programs or one-line names; nothing else is asked of the machine.
#[must_use]
pub fn caught(shell: Shell, command: &str) -> Option<Caught> {
    if shell == Shell::PowerShell {
        // The `PowerShell` tool's command is itself the one-line program.
        if let Some(name) = code_name_in(command) {
            return Some(Caught {
                kind: Kind::OneLiner,
                words: name.to_string(),
            });
        }
    }
    runs::programs_run(command, worth_reading)
        .iter()
        .find_map(caught_in_run)
}

/// Whether `text` names anything the table could catch.
fn worth_reading(text: &str) -> bool {
    mentions(text, TABLE.script_runner)
        || TABLE
            .input_tools
            .iter()
            .chain(TABLE.code_names)
            .any(|name| mentions(text, name))
}

/// What one program the command runs is caught as.
fn caught_in_run(run: &ProgramRun) -> Option<Caught> {
    let program = run.program.strip_suffix(".exe").unwrap_or(&run.program);
    if let Some(tool) = TABLE.input_tools.iter().find(|tool| **tool == program) {
        return Some(Caught {
            kind: Kind::InputTool,
            words: (*tool).to_string(),
        });
    }
    if program == TABLE.script_runner {
        if let Some(caught) = caught_in_script(&script_of(run)) {
            return Some(caught);
        }
    }
    if !is_interpreter(program) {
        return None;
    }
    run.args
        .iter()
        .zip(run.args.iter().skip(1))
        .filter(|(flag, _)| TABLE.code_flags.iter().any(|code| flag.eq_ignore_ascii_case(code)))
        .map(|(_, code)| code)
        .chain(&run.input)
        .find_map(|code| code_name_in(code))
        .map(|name| Caught {
            kind: Kind::OneLiner,
            words: name.to_string(),
        })
}

/// Whether `program` runs a one-line program it is handed: the program
/// that reads a `-c` · `-e` as code, where another reads a pattern, a folder
/// or a package there (`grep -E`, `git -C`, `pip install -e`).
fn is_interpreter(program: &str) -> bool {
    TABLE.interpreters.contains(&program)
        || TABLE
            .interpreter_families
            .iter()
            .any(|family| program.starts_with(family))
}

/// The `AppleScript` an `osascript` run is handed: its `-e` lines and what it
/// reads on its input. A script file it is named is not read.
fn script_of(run: &ProgramRun) -> String {
    let mut lines: Vec<&str> = run
        .args
        .iter()
        .zip(run.args.iter().skip(1))
        .filter(|(flag, _)| flag.as_str() == TABLE.script_flag)
        .map(|(_, line)| line.as_str())
        .collect();
    lines.extend(run.input.iter().map(String::as_str));
    lines.join("\n")
}

/// What an `AppleScript` is caught as. Its quoted strings are letters, not
/// verbs (`display notification "activate"`), save the app a `tell` names.
fn caught_in_script(script: &str) -> Option<Caught> {
    let said = words_of(script);
    let verbs = words_of(&without_strings(script));
    let screen_verb = || TABLE.screen_verbs.iter().find(|verb| has_phrase(&verbs, verb));
    if let Some(verb) = has_phrase(&said, TABLE.screen_app).then(screen_verb).flatten() {
        return Some(Caught {
            kind: Kind::ScreenScript,
            words: verb.join(" "),
        });
    }
    if let Some(verb) = window_verb(&verbs) {
        return Some(Caught {
            kind: Kind::AppWindows,
            words: verb.join(" "),
        });
    }
    set_property(&verbs).map(|property| Caught {
        kind: Kind::AppWindows,
        words: format!("{} {}", TABLE.set_verb, property.join(" ")),
    })
}

/// The table's window verb that stands in `words`, where it is not one of
/// the verbs that only start like it (`close access`).
fn window_verb(words: &[String]) -> Option<&'static [&'static str]> {
    TABLE.window_verbs.iter().copied().find(|verb| {
        (0..words.len()).any(|at| {
            let rest = &words[at..];
            starts_with(rest, verb) && !TABLE.not_window_verbs.iter().any(|not| starts_with(rest, not))
        })
    })
}

/// The property a `set` changes, when it is one of the table's.
fn set_property(words: &[String]) -> Option<&'static [&'static str]> {
    words.iter().enumerate().find_map(|(at, word)| {
        if word != TABLE.set_verb {
            return None;
        }
        let mut next = at + 1;
        if words.get(next).is_some_and(|word| word == TABLE.article) {
            next += 1;
        }
        let rest = words.get(next..)?;
        TABLE
            .set_properties
            .iter()
            .find(|property| starts_with(rest, property))
            .copied()
    })
}

/// The first of the table's one-line names in `code`, any case.
fn code_name_in(code: &str) -> Option<&'static str> {
    TABLE.code_names.iter().find(|name| mentions(code, name)).copied()
}

/// Whether `text` holds `name`'s letters, ASCII case folded.
fn mentions(text: &str, name: &str) -> bool {
    text.as_bytes()
        .windows(name.len())
        .any(|window| window.eq_ignore_ascii_case(name.as_bytes()))
}

/// `text`'s words, ASCII case folded: runs of letters, digits and `_`.
fn words_of(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// `script` with its double-quoted strings taken out.
fn without_strings(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut in_string = false;
    let mut escaped = false;
    for c in script.chars() {
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => {
                    in_string = false;
                    out.push(' ');
                }
                _ => {}
            }
        } else if c == '"' {
            in_string = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether `phrase` stands in `words` as consecutive words.
fn has_phrase(words: &[String], phrase: &[&str]) -> bool {
    (0..words.len()).any(|at| starts_with(&words[at..], phrase))
}

/// Whether `words` open with `phrase`.
fn starts_with(words: &[String], phrase: &[&str]) -> bool {
    words.len() >= phrase.len() && words.iter().zip(phrase).all(|(word, part)| word == part)
}

/// The refusal for what was caught: what the guard saw, what to do instead,
/// and — once, for the person who reads it — the setting's name.
#[must_use]
pub fn refusal(caught: &Caught) -> String {
    format!(
        "refused to run: this command moves the person's screen ({kind}: `{words}`). \
         Use the Computer tool (`{cli}`) instead — it acts in the background, where the person \
         can watch and stop it. If Computer Use refused that step, tell the person what it said, \
         word for word, and stop; do not go around it with another tool. \
         The desk guard decides this in every permission mode (the person's setting `{SETTING}`).",
        kind = caught.kind.named(),
        words = caught.words,
        cli = zerocode_core::computer_use::COMPUTER_CLI,
    )
}

/// The refusal for `command` as `shell` runs it in this session, when the
/// guard stands here and catches it. Asked by every layer that judges a
/// model's command, right after the kill guard.
#[must_use]
pub fn refused(shell: Shell, command: &str) -> Option<String> {
    let caught = caught(shell, command)?;
    stands_in_this_session().then(|| refusal(&caught))
}

/// Whether the guard stands in this session: it can reach the Computer tool
/// — the window's `zerocode-computer` is on its `PATH`, so there is a road
/// to send the model back to — and the person's settings leave [`SETTING`]
/// on. Read once per process, and only after a command is first caught, so
/// an ordinary command costs neither the path search nor the settings read
/// (design §6: turned off, it stands down from the next session).
#[cfg(not(test))]
fn stands_in_this_session() -> bool {
    static STANDS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *STANDS.get_or_init(|| computer_tool_reachable() && setting_on())
}

/// In a test build the guard stands only where a test says so
/// ([`with_session`]): no test reads the machine's `PATH` or the person's
/// settings.
#[cfg(test)]
fn stands_in_this_session() -> bool {
    STANDS_FOR_TEST.with(std::cell::Cell::get)
}

/// Whether this session can reach the Computer tool.
#[cfg(not(test))]
fn computer_tool_reachable() -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    zerocode_core::computer_use::computer_cli_on_path(&path, cfg!(windows)).is_some()
}

/// Whether the person's settings leave the guard on. Settings that do not
/// load leave it on.
#[cfg(not(test))]
fn setting_on() -> bool {
    let cwd = std::env::current_dir().unwrap_or_default();
    match crate::ConfigLoader::default_for(cwd).load() {
        Ok(config) => config.desk_guard(),
        Err(_) => true,
    }
}

#[cfg(test)]
thread_local! {
    /// What [`stands_in_this_session`] answers on this test thread.
    static STANDS_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with the guard standing (`true`) or not on this test thread.
#[cfg(test)]
pub(crate) fn with_session<T>(stands: bool, f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            STANDS_FOR_TEST.with(|slot| slot.set(self.0));
        }
    }
    let _restore = Restore(STANDS_FOR_TEST.with(|slot| slot.replace(stands)));
    f()
}

#[cfg(test)]
mod tests;
