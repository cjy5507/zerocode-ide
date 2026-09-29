//! `zerocode-find` (t-14869): the file pick seat, called by the agent in a
//! window pane itself — a shell command on the pane's PATH beside the other
//! doors (`zerocode-browser`, `zerocode-artifact`), asked through the same
//! bridge with the same token. It hands the window what the agent is about
//! to change or debug, and prints the files most likely involved, each with
//! its first comment line: the window's own search and the session's recent
//! edits, ranked by the seat's judgment where it acts for the pane and in
//! today's order where it only records.
//!
//! What it prints is a fact the agent may use or ignore, in fixed words; the
//! row the window writes of it names the agent that asked and the moment
//! (`asked`), and carries no path or word.

use crate::jev::file_pick::{FILE_PICK_NOTE_PREFIX, FilePickCandidate};

/// The command on a pane's PATH.
pub const SHIM: &str = "zerocode-find";

/// The bridge's road for it.
pub const ROUTE: &str = "/find";

/// What `--help` prints.
pub const USAGE: &str = "zerocode-find <what you are about to change or debug>\nLists the files of this project most likely involved, each with its first comment line: suggestions to verify, from the window's search and its file pick judgment.";

/// How long the shim waits for the window: the search's own deadline and the
/// seat's wall, with a second to spare — past it the command says it could not
/// reach the window.
pub const FIND_SHIM_TIMEOUT_SECS: u64 = 5;

/// The moment a row asked this way is written at.
pub const ASKED: &str = "asked";

/// The flag the shim adds with the folder the agent's shell stands in.
pub const CWD_FLAG: &str = "--cwd";

/// The flag the shim adds with the pane that asked.
pub const PANE_FLAG: &str = "--pane";

/// The words under the list: what it is, never what to do.
pub const LISTING_NOTE: &str =
    "(suggestions from the window's search and file pick judgment; verify or ignore)";

/// The line said when no file names the words.
pub const NOTHING_FOUND: &str = "No file in this project names these words.";

/// One `zerocode-find` as the window reads it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FindAsk {
    /// What the agent is about to change or debug, in its words.
    pub request: String,
    /// The folder its shell stood in.
    pub cwd: Option<String>,
    /// The pane that asked (`term-<n>`).
    pub pane: Option<String>,
}

/// Read one `zerocode-find` off its argv: the words, and the folder and pane
/// the shim added.
///
/// # Errors
/// The usage, when no word was given.
pub fn ask_from_argv(argv: &[String]) -> Result<FindAsk, &'static str> {
    let mut words = argv;
    let (mut cwd, mut pane) = (None, None);
    while let [rest @ .., flag, value] = words {
        let held = match flag.as_str() {
            CWD_FLAG => &mut cwd,
            PANE_FLAG => &mut pane,
            _ => break,
        };
        if held.is_some() {
            break;
        }
        *held = Some(value.clone());
        words = rest;
    }
    let request = words.join(" ").trim().to_string();
    if request.is_empty() {
        return Err(USAGE);
    }
    Ok(FindAsk { request, cwd, pane })
}

/// What the command prints for `files`, in their order: one line per file —
/// its path, and its first comment line where it has one — and the note
/// that says what the list is. [`NOTHING_FOUND`] for none.
#[must_use]
pub fn listing(files: &[FilePickCandidate]) -> String {
    if files.is_empty() {
        return format!("{NOTHING_FOUND}\n");
    }
    let mut said = String::new();
    for file in files {
        let path: String = file.path.chars().filter(|c| !c.is_control()).collect();
        said.push_str(FILE_PICK_NOTE_PREFIX);
        said.push(' ');
        said.push_str(&path);
        if !file.about.is_empty() {
            said.push_str("  ");
            said.push_str(&file.about);
        }
        said.push('\n');
    }
    said.push_str(LISTING_NOTE);
    said.push('\n');
    said
}

/// The command's script: the window's bridge door, carrying the folder the
/// shell stands in and the pane that asked
/// (`computer_use::PowerShellBridgeShim`).
#[must_use]
pub fn shim_script(port_var: &str, hook_token_var: &str) -> String {
    door(port_var, hook_token_var).render_posix()
}

/// The same door for a shell that cannot read a shebang.
#[must_use]
pub fn shim_script_powershell(port_var: &str, hook_token_var: &str) -> String {
    door(port_var, hook_token_var).render()
}

fn door<'a>(
    port_var: &'a str,
    hook_token_var: &'a str,
) -> crate::computer_use::PowerShellBridgeShim<'a> {
    crate::computer_use::PowerShellBridgeShim {
        command: SHIM,
        manual: USAGE,
        prefix: "",
        route: ROUTE,
        port_var,
        capability_var: hook_token_var,
        capability_header: "x-zerocode-find-token",
        capability_missing: "this shell is not inside a ZeroCode window",
        hook_token_var,
        deadline_seconds: FIND_SHIM_TIMEOUT_SECS,
        stdin_flags: false,
        pane_header: None,
        cwd_verbs: &[],
        cwd_flag: Some(CWD_FLAG),
        pane_flag: Some(PANE_FLAG),
    }
}

#[cfg(test)]
mod tests;
