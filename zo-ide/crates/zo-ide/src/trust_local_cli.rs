//! `zo settings trust-local [--forget] [--cwd <dir>]` — the person records a
//! folder's `.zo/settings.local.json` and `.zo/trusted-mcp-servers.json` as
//! their own, from their own terminal (153).
//!
//! zo reads those files as the person's only when their bytes are what zo last
//! wrote there (`runtime::config::local_record`). A file the folder came with,
//! one an older zo wrote before it kept a record, or one the person edited by
//! hand reads as the repository's until the person records it here.
//!
//! A folder's instructions can ask a model to run this command, so it refuses
//! the plain ways a model's call reaches it:
//!
//! 1. standard input and standard error are both terminals — zo's shell tool
//!    gives its children an empty standard input;
//! 2. no `ZO_TOOL_CHILD` (every child of zo's shell tool carries it) and no
//!    `ZO_AGENT_ID` (a helper pane);
//! 3. no zo process above this one, as far as `ps` names it (Unix);
//! 4. the confirmation is the folder's name typed in full, not `y` — `yes |`
//!    answers nothing.
//!
//! These guard against a mistake and a plain instruction; they are not a
//! security boundary. A shell that can run commands of its choosing gets past
//! each of them (`env -u`, a faked terminal, a job that outlives its parent),
//! and it can write the record itself more simply still. The boundary is what
//! that shell is let do: zo's permission prompts and the sandbox.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
zo settings trust-local [--forget] [--cwd <dir>]

Record this folder's .zo/settings.local.json and .zo/trusted-mcp-servers.json,
as they are now, as yours: their hooks, env, \"always allow\" rules and MCP
servers then apply here without being stripped. Run it yourself in a terminal;
zo asks you to type the folder's name. --forget takes the folder's files out of
the record.";

/// What a run says where no record is kept: it is trusted by Unix ownership.
pub const NO_RECORD_ON_THIS_SYSTEM: &str = "zo keeps no record of local files on this system (it \
     trusts one by Unix ownership), so a folder's .zo/settings.local.json and \
     .zo/trusted-mcp-servers.json read as the repository's here.";

/// What a refused run says.
pub const NOT_THE_PERSONS_TERMINAL: &str = "zo records a folder's local settings as yours only \
     from your own terminal, not from inside a zo session or a tool zo started. Run `zo settings \
     trust-local` yourself in a terminal.";

/// One recorded-kind file found in the folder.
struct Found {
    real: PathBuf,
    shown: String,
    bytes: Vec<u8>,
}

/// # Errors
///
/// A usage error, a refusal ([`NOT_THE_PERSONS_TERMINAL`]), a folder with
/// neither file, a confirmation that did not name the folder, a file that
/// changed while the person was asked, or a record that could not be written.
pub fn run(args: &[String], cwd: &Path) -> Result<String, String> {
    let mut forget = false;
    let mut folder = cwd.to_path_buf();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--forget" => forget = true,
            "--cwd" => {
                folder = PathBuf::from(words.next().ok_or_else(|| format!("--cwd needs a folder\n\n{USAGE}"))?);
            }
            "--help" | "-h" | "help" => return Ok(USAGE.to_string()),
            other => return Err(format!("unexpected argument {other}\n\n{USAGE}")),
        }
    }
    if !cfg!(unix) {
        return Err(NO_RECORD_ON_THIS_SYSTEM.to_string());
    }
    refuse_unless_the_persons_terminal(&Surroundings::of_this_process())?;
    let folder = folder
        .canonicalize()
        .map_err(|error| format!("cannot read {}: {error}", folder.display()))?;
    let found = found_in(&folder)?;
    let home = runtime::default_config_home();
    if forget {
        let mut lines = Vec::new();
        for file in &found {
            let removed = runtime::forget_local_file(&home, &file.real).map_err(|error| error.to_string())?;
            lines.push(format!(
                "{} {}",
                if removed { "forgot" } else { "was not recorded:" },
                file.shown
            ));
        }
        return Ok(lines.join("\n"));
    }
    let name = folder_name(&folder);
    let mut terminal = std::io::stderr();
    let _ = writeln!(terminal, "{}", summary(&found));
    let _ = write!(
        terminal,
        "Type the folder's name `{name}` to record these files as yours (anything else leaves them as they are): "
    );
    let _ = terminal.flush();
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    if !names_the_folder(&answer, &name) {
        return Err("nothing was recorded: the answer did not name the folder".to_string());
    }
    let mut lines = Vec::new();
    for file in &found {
        // The bytes recorded are the bytes shown: a file that changed while
        // the person was asked is not recorded.
        let now = std::fs::read(&file.real).map_err(|error| error.to_string())?;
        if now != file.bytes {
            return Err(format!(
                "nothing more was recorded: {} changed while you were asked; run the command again",
                file.shown
            ));
        }
        runtime::remember_local_file(&home, &file.real, &file.bytes)
            .map_err(|error| error.to_string())?;
        lines.push(format!("recorded {} as yours", file.shown));
    }
    Ok(lines.join("\n"))
}

/// The facts the refusal is decided on, read from this process — apart so a
/// test can give each one.
// Five independent facts, each a yes or no; an enum would only rename them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Surroundings {
    pub stdin_is_terminal: bool,
    pub stderr_is_terminal: bool,
    pub tool_child: bool,
    pub helper_pane: bool,
    pub zo_above: bool,
}

impl Surroundings {
    #[must_use]
    pub fn of_this_process() -> Self {
        Self {
            stdin_is_terminal: std::io::stdin().is_terminal(),
            stderr_is_terminal: std::io::stderr().is_terminal(),
            tool_child: std::env::var_os(runtime::TOOL_CHILD_ENV).is_some(),
            helper_pane: std::env::var_os(runtime::subagent_panes::ZO_AGENT_ID_VAR).is_some(),
            zo_above: zo_process_above(),
        }
    }
}

/// # Errors
///
/// [`NOT_THE_PERSONS_TERMINAL`] unless every fact says a person is at a
/// terminal of their own.
pub fn refuse_unless_the_persons_terminal(here: &Surroundings) -> Result<(), String> {
    let persons = here.stdin_is_terminal
        && here.stderr_is_terminal
        && !here.tool_child
        && !here.helper_pane
        && !here.zo_above;
    if persons {
        Ok(())
    } else {
        Err(NOT_THE_PERSONS_TERMINAL.to_string())
    }
}

/// Whether `answer` is the folder's name, typed in full.
#[must_use]
pub fn names_the_folder(answer: &str, name: &str) -> bool {
    !name.is_empty() && answer.trim() == name
}

fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .map_or_else(|| folder.display().to_string(), |name| name.to_string_lossy().into_owned())
}

fn found_in(folder: &Path) -> Result<Vec<Found>, String> {
    let mut found = Vec::new();
    for name in runtime::RECORDED_LOCAL_FILES {
        let path = folder.join(".zo").join(name);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            return Err(format!("{} is not a regular file; zo does not record it", path.display()));
        }
        let real = path.canonicalize().map_err(|error| error.to_string())?;
        if real != folder.join(".zo").join(name) {
            return Err(format!("{} is reached through a link; zo does not record it", path.display()));
        }
        let bytes = std::fs::read(&real).map_err(|error| error.to_string())?;
        found.push(Found {
            shown: format!(".zo/{name}"),
            real,
            bytes,
        });
    }
    if found.is_empty() {
        return Err(format!(
            "{} has no .zo/settings.local.json or .zo/trusted-mcp-servers.json",
            folder.display()
        ));
    }
    Ok(found)
}

/// What the person is asked to record: each file and what it says that the
/// repository's rules would otherwise strip.
fn summary(found: &[Found]) -> String {
    let mut lines = vec!["zo will read these files as yours, as they are now:".to_string()];
    for file in found {
        let said = serde_json::from_slice::<serde_json::Value>(&file.bytes).ok().map_or_else(
            || "not valid JSON".to_string(),
            |value| match value {
                serde_json::Value::Object(map) => {
                    let keys: Vec<&str> = map.keys().map(String::as_str).collect();
                    format!("settings: {}", keys.join(", "))
                }
                serde_json::Value::Array(names) => format!("{} trusted MCP server name(s)", names.len()),
                _ => "not a JSON object or list".to_string(),
            },
        );
        lines.push(format!("  {} — {said}", file.shown));
    }
    lines.join("\n")
}

/// Whether a zo process stands above this one. Reads each parent with `ps`
/// (Unix); a parent that cannot be read ends the walk.
#[cfg(unix)]
fn zo_process_above() -> bool {
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..64 {
        if pid <= 1 {
            return false;
        }
        let Ok(output) = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-o", "comm=", "-p", &pid.to_string()])
            .output()
        else {
            return false;
        };
        let line = String::from_utf8_lossy(&output.stdout);
        let mut fields = line.split_whitespace();
        let (Some(parent), Some(command)) = (fields.next(), fields.next()) else {
            return false;
        };
        if is_zo_command(command) {
            return true;
        }
        let Ok(parent) = parent.parse::<u32>() else {
            return false;
        };
        pid = parent;
    }
    false
}

#[cfg(not(unix))]
fn zo_process_above() -> bool {
    false
}

/// Whether `command` (as `ps -o comm=` prints it) is the zo binary.
#[must_use]
pub fn is_zo_command(command: &str) -> bool {
    Path::new(command)
        .file_name()
        .is_some_and(|name| name == "zo")
}

#[cfg(test)]
mod tests {
    use super::{
        is_zo_command, names_the_folder, refuse_unless_the_persons_terminal, Surroundings,
        NOT_THE_PERSONS_TERMINAL,
    };

    fn persons() -> Surroundings {
        Surroundings {
            stdin_is_terminal: true,
            stderr_is_terminal: true,
            ..Surroundings::default()
        }
    }

    /// Only a person at a terminal of their own records: an empty standard
    /// input (zo's shell tool), a tool child, a helper pane or a zo above
    /// each refuse, with the sentence that sends the person to their terminal.
    #[test]
    fn trust_local_records_only_from_the_persons_own_terminal() {
        assert_eq!(refuse_unless_the_persons_terminal(&persons()), Ok(()));
        for (what, here) in [
            ("no terminal input", Surroundings { stdin_is_terminal: false, ..persons() }),
            ("no terminal for the question", Surroundings { stderr_is_terminal: false, ..persons() }),
            ("a child of zo's shell tool", Surroundings { tool_child: true, ..persons() }),
            ("a helper pane", Surroundings { helper_pane: true, ..persons() }),
            ("a zo above", Surroundings { zo_above: true, ..persons() }),
        ] {
            assert_eq!(
                refuse_unless_the_persons_terminal(&here),
                Err(NOT_THE_PERSONS_TERMINAL.to_string()),
                "{what}"
            );
        }
        assert!(NOT_THE_PERSONS_TERMINAL.contains("yourself in a terminal"));
    }

    /// The confirmation is the folder's name in full: `y`, an empty line or
    /// another name records nothing.
    #[test]
    fn only_the_folders_name_confirms() {
        assert!(names_the_folder("acme-app\n", "acme-app"));
        for answer in ["y\n", "yes\n", "\n", "acme\n", "acme-app-2\n"] {
            assert!(!names_the_folder(answer, "acme-app"), "{answer:?}");
        }
        assert!(!names_the_folder("\n", ""));
    }

    #[test]
    fn a_zo_parent_is_told_by_its_binary_name() {
        assert!(is_zo_command("/Users/dev/.local/bin/zo"));
        assert!(is_zo_command("zo"));
        assert!(!is_zo_command("/bin/zsh"));
        assert!(!is_zo_command("zoxide"));
    }
}
