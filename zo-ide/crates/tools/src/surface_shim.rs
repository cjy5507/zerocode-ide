//! Exact-byte provenance checks for the window's installed surface shims.

use std::path::Path;
use zerocode_core::agent_shims::{SurfaceShim, installed_script};

fn matches_file(program: &Path, expected: &str) -> bool {
    let Ok(cap) = u64::try_from(expected.len()) else { return false; };
    runtime::secure_fs::read_regular_file_absolute_no_follow_bounded(program, cap)
        .ok().flatten().is_some_and(|contents| contents == expected)
}

pub(crate) fn is_installed(program: &Path, shim: SurfaceShim) -> bool {
    let Some(parent) = program.parent().and_then(|parent| std::fs::canonicalize(parent).ok()) else { return false; };
    let Some(name) = program.file_name() else { return false; };
    let program = parent.join(name);
    if cfg!(windows) {
        let powershell = format!("{}.impl.ps1", shim.name());
        matches_file(&program, &zerocode_core::agent_teams::shim_cmd(&powershell))
            && matches_file(&parent.join(powershell), &installed_script(shim, true))
    } else {
        matches_file(&program, &installed_script(shim, false))
    }
}
