//! Which file a pane's conversation is read from (herdr 4, t-26597).
//!
//! A pane is a process, and its conversation is a file the CLI writes. The
//! window has to say which file, and it answers in this order:
//!
//! 1. The file the agent reported, while it still exists. The agent named its
//!    own transcript together with its own session id.
//! 2. A session id is known. The file is the one that CARRIES that id — named
//!    by it, or naming it on its first line — and no other. The open files and
//!    the screen are not asked: an open file can belong to a neighbour, and a
//!    screen can share words with anyone's transcript.
//! 3. No session id yet. The file is the one session file the pane's process
//!    holds open. Two open session files name neither.
//! 4. Still nothing. The file is the one session file whose text holds a line
//!    of the screen. Two files holding such a line name neither.
//!
//! The newest file in a folder is never an answer. A folder holds the files of
//! every pane that has run in it, so the newest one belongs to a neighbour as
//! often as to the pane asking. Every answer is one file or a named reason for
//! none; a stand-in file is never returned.
//!
//! The rule reads nothing itself. The window supplies the facts through
//! [`Facts`], so the same rule answers the window and the tests alike. A CLI
//! joins by its row of [`crate::vault::AGENT_SOURCES`] and by no branch here.

use std::path::{Path, PathBuf};

use crate::vault::{self, AgentSource, IdPlacement};

/// Shorter screen lines are too common to name a file: a prompt's chrome or a
/// one-word answer appears in many transcripts at once.
pub const SCREEN_LINE_CHARS: usize = 24;

/// The most session files a screen match reads. A longer listing is not read
/// line by line on every poll, and no file is named from it.
pub const SCREEN_CANDIDATES_MAX: usize = 200;

/// How a file was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The agent reported this file itself, with the session id.
    Reported,
    /// The file carries the pane's session id, and no other file does.
    SessionId,
    /// No session id is known, and the file is the one session file the pane's
    /// process holds open.
    OpenFile,
    /// No session id is known and nothing is open, and the file is the one
    /// session file that holds a line of the screen.
    ScreenMatch,
}

/// Why no file is named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Absent {
    /// No row of the adapter table names this CLI's store, so this window does
    /// not know where it keeps its conversations.
    NoAdapter,
    /// The store has a row, but this window cannot read its format yet.
    UnreadFormat,
    /// A session id is known and no file carries it. A listing cut short also
    /// lands here, because a file missing from it cannot be ruled out.
    NoSessionFile,
    /// A session id is known and more than one file carries it.
    TwoSessionFiles,
    /// No session id is known and the pane's process holds no session file open.
    NoOpenFile,
    /// No session id is known and the pane's process holds two or more session
    /// files open.
    TwoOpenFiles,
    /// No session file holds a line of the screen. A listing too long to read
    /// is the same answer.
    NoScreenMatch,
    /// Two or more session files hold a line of the screen.
    TwoScreenMatches,
}

/// What the window knows about the disk and the pane. The rule asks for these
/// facts and reads nothing else.
pub trait Facts {
    /// Whether the file is there.
    fn exists(&self, path: &Path) -> bool;
    /// The directories one source's session files live under.
    fn roots(&self, source: &AgentSource) -> Vec<PathBuf>;
    /// Every session file of one source under its roots, or `None` when the
    /// listing was cut short.
    fn session_files(&self, source: &AgentSource) -> Option<Vec<PathBuf>>;
    /// The session id a file names on its first line, when its store puts the
    /// id there.
    fn first_line_id(&self, path: &Path) -> Option<String>;
    /// The files the pane's process holds open now.
    fn open_files(&self) -> Vec<PathBuf>;
    /// The lines the pane shows now.
    fn screen_lines(&self) -> Vec<String>;
    /// The text of one file, bounded, or `None` when it cannot be read.
    fn text_of(&self, path: &Path) -> Option<String>;
}

/// The file one pane's conversation is read from, and how it was found.
///
/// `slug` is the agent's catalog slug, `reported` the transcript the agent
/// named, and `session_id` the id the agent reported for this conversation.
pub fn bind(
    slug: &str,
    reported: Option<&Path>,
    session_id: Option<&str>,
    facts: &dyn Facts,
) -> Result<(PathBuf, Via), Absent> {
    let rows: Vec<&AgentSource> = vault::AGENT_SOURCES
        .iter()
        .filter(|row| row.slug == slug)
        .collect();
    if rows.is_empty() {
        return Err(Absent::NoAdapter);
    }
    if let Some(path) = reported.filter(|path| facts.exists(path)) {
        return Ok((path.to_path_buf(), Via::Reported));
    }
    let readable: Vec<&AgentSource> = rows
        .into_iter()
        .filter(|row| row.format.readable())
        .collect();
    if readable.is_empty() {
        return Err(Absent::UnreadFormat);
    }
    // One agent can keep its store in more than one place (Antigravity moved
    // its brain folder between versions). Each place is asked, and the first
    // one that names a file answers.
    let mut last = Absent::NoSessionFile;
    for row in readable {
        let found = match session_id {
            Some(id) => by_session_id(row, id, facts).map(|path| (path, Via::SessionId)),
            None => match by_open_file(row, facts) {
                Ok(path) => Ok((path, Via::OpenFile)),
                Err(Absent::NoOpenFile) => {
                    by_screen(row, facts).map(|path| (path, Via::ScreenMatch))
                }
                Err(reason) => Err(reason),
            },
        };
        match found {
            Ok(hit) => return Ok(hit),
            Err(reason) => last = reason,
        }
    }
    Err(last)
}

/// Whether a file carries the session id, by the way its store writes the id.
fn carries_id(source: &AgentSource, id: &str, path: &Path, facts: &dyn Facts) -> bool {
    match source.format.id_placement() {
        IdPlacement::FileStem => path.file_stem().and_then(|stem| stem.to_str()) == Some(id),
        IdPlacement::FirstLine => facts.first_line_id(path).as_deref() == Some(id),
        IdPlacement::Folder => path
            .ancestors()
            .skip(1)
            .any(|dir| dir.file_name().and_then(|name| name.to_str()) == Some(id)),
        IdPlacement::Unstated => false,
    }
}

/// The one session file that carries `id`.
fn by_session_id(_source: &AgentSource, _id: &str, _facts: &dyn Facts) -> Result<PathBuf, Absent> {
    // Red stub: the id rule is not written yet.
    Err(Absent::NoSessionFile)
}

/// The one session file the pane's process holds open.
fn by_open_file(_source: &AgentSource, _facts: &dyn Facts) -> Result<PathBuf, Absent> {
    // Red stub: the open-file rule is not written yet.
    Err(Absent::NoOpenFile)
}

/// The one session file whose text holds a line of the screen.
fn by_screen(_source: &AgentSource, _facts: &dyn Facts) -> Result<PathBuf, Absent> {
    // Red stub: the screen rule is not written yet.
    Err(Absent::NoScreenMatch)
}

#[cfg(test)]
mod tests;
