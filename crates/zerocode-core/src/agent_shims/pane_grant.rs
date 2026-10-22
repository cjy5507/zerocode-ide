//! Read the window's per-pane grant as data. Never execute it or read its token.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::agent_teams::{TEAM_ID_VAR, TEAM_PANE_VAR};

const TEAM_TOKEN_FILE_VAR: &str = "ZEROCODE_AGENT_TEAM_TOKEN_FILE";
// The grant holds a handful of capability paths, not session or tool content.
const MAX_GRANT_BYTES: u64 = 64 * 1024;

/// A team and pane already granted by the window, with only a token-file path.
/// The token's bytes are left to the installed command that authenticates them.
#[derive(Clone)]
pub struct PaneTeamGrant {
    team: String,
    pane: String,
    token_file: PathBuf,
}

impl PaneTeamGrant {
    /// The complete binding to pass to the ledger command as one unit.
    #[must_use]
    pub fn environment(&self) -> [(&'static str, &OsStr); 3] {
        [
            (TEAM_ID_VAR, OsStr::new(&self.team)),
            (TEAM_PANE_VAR, OsStr::new(&self.pane)),
            (TEAM_TOKEN_FILE_VAR, self.token_file.as_os_str()),
        ]
    }
}

/// The installed ledger command beside this window's endpoint directory.
/// This does not search another window, a repository, or a user's home.
#[must_use]
pub fn orchestration_program(endpoint: &Path, windows: bool) -> Option<PathBuf> {
    if !endpoint.is_absolute() {
        return None;
    }
    let directory = endpoint.parent()?.parent()?;
    let name = if windows {
        format!("{}.ps1", super::ORCHESTRATION_COMMAND)
    } else {
        super::ORCHESTRATION_COMMAND.to_string()
    };
    Some(directory.join(super::ORCHESTRATION_DIR_NAME).join(name))
}

/// Read only the grant selected by this process's endpoint and pane key.
/// Absence (including a browser-only grant) grants no ledger capability.
/// Malformed, incomplete, non-regular, and oversized files are errors. Shell
/// expansions are never evaluated and no token-file contents are read.
///
/// # Errors
/// Returns an I/O error when a present grant cannot be read, is malformed, or
/// names a token file that is absent or not a regular non-empty file.
pub fn read_pane_team_grant(endpoint: &Path, pane_key: &str) -> io::Result<Option<PaneTeamGrant>> {
    let Some(number) = pane_key
        .strip_prefix("term-")
        .and_then(|s| s.parse::<u32>().ok())
    else {
        return Err(invalid("invalid pane key"));
    };
    if pane_key != format!("term-{number}") || !endpoint.is_absolute() {
        return Err(invalid("invalid pane grant location"));
    }
    let Some(directory) = endpoint.parent() else {
        return Err(invalid("missing window endpoint directory"));
    };
    let path = directory
        .join(super::PANE_GRANTS_DIR_NAME)
        .join(format!("{pane_key}.env"));
    let Some(body) = read_regular(&path)? else {
        return Ok(None);
    };
    let mut values = BTreeMap::new();
    for line in body.lines() {
        let Some((name, quoted)) = line.split_once('=') else {
            continue;
        };
        if ![TEAM_ID_VAR, TEAM_PANE_VAR, TEAM_TOKEN_FILE_VAR].contains(&name) {
            continue;
        }
        let value = unquote(quoted.trim())?;
        if value.trim().is_empty() || value.contains(['\0', '\r', '\n']) {
            return Err(invalid("empty or invalid team grant field"));
        }
        if values.insert(name, value).is_some() {
            return Err(invalid("duplicate team grant field"));
        }
    }
    if values.is_empty() {
        return Ok(None);
    }
    let mut take = |name| {
        values
            .remove(name)
            .ok_or_else(|| invalid("incomplete team grant"))
    };
    let team = take(TEAM_ID_VAR)?;
    let pane = take(TEAM_PANE_VAR)?;
    let token_file = PathBuf::from(take(TEAM_TOKEN_FILE_VAR)?);
    if !token_file.is_absolute() {
        return Err(invalid("team token file must have an absolute path"));
    }
    let token = fs::symlink_metadata(&token_file)?;
    if !token.is_file() || token.len() == 0 {
        return Err(invalid("team token file is unavailable"));
    }
    Ok(Some(PaneTeamGrant {
        team,
        pane,
        token_file,
    }))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn read_regular(path: &Path) -> io::Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        Ok(metadata) if !metadata.is_file() => {
            return Err(invalid("pane grant is not a regular file"));
        }
        Ok(_) => {}
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("pane grant is not a regular file"));
    }
    let mut body = String::new();
    file.take(MAX_GRANT_BYTES + 1).read_to_string(&mut body)?;
    if u64::try_from(body.len()).unwrap_or(u64::MAX) > MAX_GRANT_BYTES {
        return Err(invalid("pane grant is too large"));
    }
    Ok(Some(body))
}

// write_pane_grant writes single-quoted values, escaping an apostrophe as
// '\''. Decode exactly that format; an expansion is ordinary quoted text.
fn unquote(mut input: &str) -> io::Result<String> {
    let mut value = String::new();
    loop {
        let Some(inside) = input.strip_prefix('\'') else {
            return Err(invalid("invalid pane grant quoting"));
        };
        let Some(end) = inside.find('\'') else {
            return Err(invalid("unfinished pane grant value"));
        };
        value.push_str(&inside[..end]);
        let rest = &inside[end + 1..];
        if rest.is_empty() {
            return Ok(value);
        }
        let Some(after_quote) = rest.strip_prefix("\\'") else {
            return Err(invalid("invalid pane grant value suffix"));
        };
        value.push('\'');
        input = after_quote;
    }
}

#[cfg(test)]
mod tests;
