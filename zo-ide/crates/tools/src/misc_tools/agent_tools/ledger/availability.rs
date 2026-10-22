//! One reading of the current pane's ledger command and existing grant.

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use zerocode_core::agent_shims::{PaneTeamGrant, orchestration_program, read_pane_team_grant};

pub(super) struct Door {
    pub(super) program: PathBuf,
    grant: Option<PaneTeamGrant>,
}

impl Door {
    pub(super) fn apply(&self, command: &mut Command) {
        if let Some(grant) = &self.grant {
            // An incomplete inherited binding must not contribute a token
            // from another seat to the complete binding read from this pane.
            command.env_remove(zerocode_core::agent_teams::TEAM_TOKEN_VAR);
            command.envs(grant.environment());
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Unavailable {
    NoPane,
    NoGrant,
    UnreadableGrant(io::ErrorKind),
    NoProgram,
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPane => f.write_str("this process has no team binding or window pane grant pointer"),
            Self::NoGrant => f.write_str("this pane has no ledger grant; open the agent through ZeroCode"),
            Self::UnreadableGrant(kind) => write!(f, "this pane's ledger grant cannot be read ({kind:?})"),
            Self::NoProgram => f.write_str("no usable zerocode-orc was found on PATH or beside this window's endpoint"),
        }
    }
}

pub(super) fn current() -> Result<Door, Unavailable> {
    resolve(|key| std::env::var_os(key).filter(|value| !value.is_empty()), cfg!(windows))
}

pub(super) fn resolve(
    lookup: impl Fn(&str) -> Option<OsString>,
    windows: bool,
) -> Result<Door, Unavailable> {
    let endpoint = lookup("ZEROCODE_HOOK_ENDPOINT").filter(|v| !v.is_empty()).map(PathBuf::from);
    let grant = if super::has_grant(|key| lookup(key).and_then(|v| v.into_string().ok())) {
        None
    } else {
        let pane = lookup(zerocode_core::hook::PANE_KEY_ENV)
            .and_then(|v| v.into_string().ok()).filter(|v| !v.is_empty())
            .ok_or(Unavailable::NoPane)?;
        let endpoint = endpoint.as_deref().ok_or(Unavailable::NoPane)?;
        Some(read_pane_team_grant(endpoint, &pane)
            .map_err(|error| Unavailable::UnreadableGrant(error.kind()))?
            .ok_or(Unavailable::NoGrant)?)
    };
    let name = format!("{}{}", zerocode_core::agent_shims::ORCHESTRATION_COMMAND, if windows { ".ps1" } else { "" });
    let from_path = lookup("PATH").and_then(|path| {
        std::env::split_paths(&path).map(|directory| directory.join(&name)).find(|path| command_file(path, windows))
    });
    let program = from_path.or_else(|| endpoint.as_deref()
        .and_then(|path| orchestration_program(path, windows)).filter(|path| command_file(path, windows)))
        .ok_or(Unavailable::NoProgram)?;
    Ok(Door { program, grant })
}

fn command_file(path: &Path, windows: bool) -> bool {
    let Ok(metadata) = path.metadata() else { return false };
    if !metadata.is_file() { return false; }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        windows || metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = windows;
        true
    }
}

/// Check again at each command: a revoked file grant must stop an in-flight
/// worker watcher or steering request, just as the command's shim does.
#[cfg(not(test))]
pub(super) fn apply_current(command: &mut Command) -> Result<(), Unavailable> {
    current()?.apply(command);
    Ok(())
}

#[cfg(test)]
#[path = "availability_tests.rs"]
mod tests;
