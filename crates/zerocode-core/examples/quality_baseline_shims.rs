//! Materialize the production CLI transports for the hermetic quality suite.
use std::{fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("usage: quality_baseline_shims <directory>"))?;
    fs::create_dir_all(&directory)?;
    let shims = [
        (
            "zerocode-orc",
            zerocode_core::agent_teams::shim_script(
                "ZEROCODE_HOOK_PORT",
                "ZEROCODE_HOOK_TOKEN",
                "orchestration",
            ),
        ),
        (
            "zerocode-browser",
            zerocode_core::agent_browser::shim_script(
                "ZEROCODE_HOOK_PORT",
                "ZEROCODE_BROWSER_TOKEN",
                "ZEROCODE_HOOK_TOKEN",
            ),
        ),
    ];
    for (name, source) in shims {
        let path = directory.join(name);
        fs::write(&path, source)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}
