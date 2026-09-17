//! Where OpenCode keeps its data on this machine.
//!
//! The token ledger uses these paths to find the SQLite file OpenCode records
//! its turns in.
//!
//! Orca resolves only `XDG_DATA_HOME` or `~/.local/share`
//! (`opencode-data-directory.ts:4-11`). The other two entries here are the
//! real placements on the platforms this window runs on, and were already in
//! the first reader before this module existed.

use std::path::{Path, PathBuf};

/// Every directory OpenCode might keep its data in, most specific first.
///
/// The arguments are the environment, passed in rather than read, so the order
/// this returns can be tested without a machine that has any of them.
#[must_use]
pub fn data_dirs_in(home: &Path, appdata: Option<&Path>, xdg_data: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(appdata) = appdata {
        dirs.push(appdata.join("opencode"));
    }
    if let Some(xdg) = xdg_data {
        dirs.push(xdg.join("opencode"));
    }
    dirs.push(home.join(".local").join("share").join("opencode"));
    dirs.push(
        home.join("Library")
            .join("Application Support")
            .join("opencode"),
    );
    dirs
}

/// The same list, read from the real environment.
#[must_use]
pub fn data_dirs() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let xdg_data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    data_dirs_in(&home, appdata.as_deref(), xdg_data.as_deref())
}

/// Whether a file name is one of OpenCode's databases.
///
/// `opencode.db` is the live one; `opencode-backup.db` and friends are copies
/// beside it, which hold the same sessions and must not be counted twice.
/// Orca's own pattern (`opencode-database-discovery.ts:47`).
#[must_use]
pub fn is_database(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".db") else {
        return false;
    };
    match stem.strip_prefix("opencode") {
        Some("") => true,
        Some(rest) => {
            let Some(rest) = rest.strip_prefix('-') else {
                return false;
            };
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        }
        None => false,
    }
}

/// Every OpenCode database on this machine, in the order they must claim
/// sessions.
///
/// The live `opencode.db` comes first in each directory: a backup beside it
/// repeats sessions, and whichever file is read first is the one that counts
/// them. Path order breaks the remaining ties so two runs agree.
///
/// `OPENCODE_DB` overrides the search the way OpenCode itself reads it —
/// absolute, or relative to a data directory. `:memory:` is a database with no
/// file, so there is nothing to read.
#[must_use]
pub fn databases() -> Vec<PathBuf> {
    let dirs = data_dirs();
    if let Some(set) = std::env::var("OPENCODE_DB")
        .ok()
        .map(|raw| raw.trim().to_string())
        && !set.is_empty()
    {
        if set == ":memory:" {
            return Vec::new();
        }
        let named = PathBuf::from(&set);
        if named.is_absolute() {
            return named.is_file().then_some(vec![named]).unwrap_or_default();
        }
        return dirs
            .iter()
            .map(|dir| dir.join(&set))
            .filter(|path| path.is_file())
            .collect();
    }
    let mut found = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut here: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter(|entry| entry.file_name().to_str().is_some_and(is_database))
            .map(|entry| entry.path())
            .collect();
        in_claim_order(&mut here);
        found.append(&mut here);
    }
    found
}

/// Puts the live database ahead of the copies beside it, then orders by path.
///
/// Two stable passes rather than one comparison over a built key: the key
/// would be a path allocated afresh for every comparison, and the answer is
/// the same.
fn in_claim_order(paths: &mut [PathBuf]) {
    paths.sort();
    paths.sort_by_key(|path| u8::from(!is_live(path)));
}

/// Whether this is the database OpenCode is writing to, rather than a copy.
fn is_live(path: &Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("opencode.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The directories walk most-specific first, and the two home layouts are
    /// always there.
    #[test]
    fn the_directories_walk_in_order() {
        let home = Path::new("/home/someone");
        let plain = data_dirs_in(home, None, None);
        assert_eq!(plain.len(), 2, "the two home layouts");
        assert!(plain[0].ends_with(".local/share/opencode"));
        assert!(plain[1].ends_with("Library/Application Support/opencode"));

        let set = data_dirs_in(
            home,
            Some(Path::new("/c/AppData")),
            Some(Path::new("/xdg/data")),
        );
        assert_eq!(set[0], PathBuf::from("/c/AppData/opencode"));
        assert_eq!(set[1], PathBuf::from("/xdg/data/opencode"));
        assert_eq!(set.len(), 4);
    }

    /// A database is `opencode.db` or `opencode-<something>.db`, and nothing
    /// else in that folder is read as one.
    #[test]
    fn only_opencodes_own_databases_are_databases() {
        assert!(is_database("opencode.db"));
        assert!(is_database("opencode-backup.db"));
        assert!(is_database("opencode-2026.08.20.db"));
        assert!(!is_database("opencode-.db"));
        assert!(!is_database("opencodex.db"));
        assert!(!is_database("opencode.db-wal"));
        assert!(!is_database("opencode.sqlite"));
        assert!(!is_database("auth.json"));
        assert!(
            !is_database("opencode-../../etc/passwd.db"),
            "a name with a path separator is not a file name"
        );
    }

    /// The live database is read before the copies beside it, so a session in
    /// both is counted by the live one.
    #[test]
    fn the_live_database_claims_before_its_copies() {
        let mut paths = [
            PathBuf::from("/d/opencode-backup.db"),
            PathBuf::from("/d/opencode-2.db"),
            PathBuf::from("/d/opencode.db"),
        ];
        in_claim_order(&mut paths);
        assert_eq!(paths[0], PathBuf::from("/d/opencode.db"));
        assert_eq!(
            paths[1],
            PathBuf::from("/d/opencode-2.db"),
            "the copies lost their path order"
        );
    }
}
