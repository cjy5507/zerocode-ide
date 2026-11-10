//! Folders a process names after its own pid under the temporary folder, taken
//! back once that process has ended.
//!
//! A test process sets up one folder for its whole life — a home, a trace
//! root, an artifact store — named `<prefix>…-<pid>`, and nothing can remove it
//! when the process ends: libtest has no hook after the last test, and a
//! `static` is never dropped. One Mac's temporary folder held 87 thousand
//! entries on 2026-11-11, and in one day zo's tests had added 209
//! `zo-ide-test-home-*`, 127 `zo-runtime-itest-traces-*` and 112 `zo-lease-*`
//! of them. The next process to set one up removes those of processes that have
//! ended.

use std::path::Path;

/// Remove each folder directly under `dir` whose name starts with `prefix` and
/// ends with `-<pid>` for a pid that is neither this process's nor a running
/// process's; how many went. A pid that cannot be asked about counts as running
/// (on Windows nothing is asked, so nothing goes), a name without a pid at its
/// end stays, and so does anything that is not a plain folder — a link is not
/// followed.
#[must_use]
pub fn remove_folders_of_ended_processes(dir: &Path, prefix: &str) -> usize {
    remove_folders_of_ended_processes_with(dir, prefix, std::process::id(), process_is_running)
}

fn remove_folders_of_ended_processes_with(
    dir: &Path,
    prefix: &str,
    own: u32,
    running: impl Fn(u32) -> bool,
) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|name| name.starts_with(prefix))
            .and_then(|name| name.rsplit_once('-'))
            .map(|(_, tail)| tail)
            .filter(|tail| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|tail| tail.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own || running(pid) || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    !matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

#[cfg(not(unix))]
fn process_is_running(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::remove_folders_of_ended_processes_with;

    #[test]
    fn an_ended_processs_folders_go_and_nothing_else() {
        let temp = std::env::temp_dir().join(format!("zo-ended-process-folders-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let at = |name: &str| temp.join(name);
        let gone = ["zo-ide-test-home-41", "zo-ide-test-home-x-2-41", "zo-ide-test-home-42"];
        for name in gone {
            std::fs::create_dir_all(at(name).join("inside")).expect("an ended process's folder");
            std::fs::write(at(name).join("inside/settings.json"), "{}").expect("a file in it");
        }
        let stays = [
            "zo-ide-test-home-7",   // a running process's
            "zo-ide-test-home-9",   // this process's
            "zo-ide-test-home-",    // no pid
            "zo-ide-test-home-4x1", // not a pid
            "zo-ide-test-home",     // the prefix alone
            "other-41",             // another prefix
        ];
        for name in stays {
            std::fs::create_dir_all(at(name)).expect("a folder that stays");
        }
        std::fs::write(at("zo-ide-test-home-43"), "").expect("a file under the name");
        #[cfg(unix)]
        std::os::unix::fs::symlink(at("other-41"), at("zo-ide-test-home-44")).expect("a link under the name");

        let removed = remove_folders_of_ended_processes_with(&temp, "zo-ide-test-home-", 9, |pid| pid == 7);

        assert_eq!(removed, gone.len());
        for name in gone {
            assert!(!at(name).exists(), "{name} stayed");
        }
        for name in stays.into_iter().chain(["zo-ide-test-home-43"]) {
            assert!(at(name).exists(), "{name} went");
        }
        #[cfg(unix)]
        assert!(at("zo-ide-test-home-44").symlink_metadata().is_ok() && at("other-41").is_dir());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn a_missing_folder_is_nothing_to_remove() {
        let missing = std::env::temp_dir().join(format!("zo-ended-process-folders-none-{}", std::process::id()));
        assert_eq!(remove_folders_of_ended_processes_with(&missing, "zo-", 1, |_| false), 0);
    }
}
