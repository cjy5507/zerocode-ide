//! One settings lock protocol for the window and CLI writers.
//!
//! The sibling path and PID record stay compatible with existing zo writers.
//! Read the document only after acquiring the guard and retain it until commit.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Cross-process advisory lock over one JSON settings document.
///
/// `settings.json` has more than one writer — the model catalog overlay and the
/// `/connect` / `/providers` registry both read-modify-write the same file — so
/// every writer must take this lock or a concurrent save silently drops the
/// other's section. The lock is a `create_new` sibling (`settings.json.lock`)
/// holding the owning pid, polled for up to ~2 s and released on drop.
///
/// A lock whose owner died holding it is taken back (`reclaim_from_a_dead_owner`):
/// a killed process never runs its drop, and before this every later writer
/// of that file waited out its poll and gave up, for good (t-6263 — the
/// challenger arm's day book and ledger take this lock too).
#[derive(Debug)]
#[must_use = "the settings lock must remain held through the read and write"]
pub struct SettingsFileLock(PathBuf);

impl SettingsFileLock {
    /// Take the lock guarding `settings_path`, creating its parent if needed.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::WouldBlock`] when another writer holds the lock
    /// for longer than the poll budget, or the underlying filesystem error.
    pub fn acquire(settings_path: &Path) -> io::Result<Self> {
        use io::Write as _;

        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let path = settings_path.with_extension("json.lock");
        for _ in 0..200 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let held = Self(path);
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(held);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if !reclaim_from_a_dead_owner(&path) {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("timed out waiting for {}", path.display()),
        ))
    }
}

impl Drop for SettingsFileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Remove the lock file `lock` when the process it names has died holding it
/// — the one way a lock outlives its owner, since a live owner removes its
/// own on drop. Answers whether it removed one.
///
/// Reclaimers take turns on the lock file itself — an advisory lock on the
/// very file they found, which the kernel drops with whoever holds it — and
/// each checks that the path still names that file before reading whose it
/// is. So one that waited behind another finds the file it came for already
/// gone, and never removes the fresh lock a live owner took in its place.
/// An owner still writing its pid (an empty file) is alive; so is a pid this
/// cannot read, and every pid [`process_alive`] cannot see die.
#[cfg(unix)]
fn reclaim_from_a_dead_owner(lock: &Path) -> bool {
    use io::Read as _;
    use std::os::unix::fs::MetadataExt as _;

    let Ok(mut file) = fs::File::open(lock) else {
        return false;
    };
    if file.try_lock().is_err() {
        return false;
    }
    let still_this_file = match (file.metadata(), fs::metadata(lock)) {
        (Ok(held), Ok(named)) => held.dev() == named.dev() && held.ino() == named.ino(),
        _ => false,
    };
    let mut owner = String::new();
    if !still_this_file || file.read_to_string(&mut owner).is_err() {
        return false;
    }
    owner
        .trim()
        .parse::<u32>()
        .is_ok_and(|owner| !process_alive(owner))
        && fs::remove_file(lock).is_ok()
}

/// Nowhere else is a dead owner seen dying ([`process_alive`]): a lock is
/// never taken from one.
#[cfg(not(unix))]
fn reclaim_from_a_dead_owner(_lock: &Path) -> bool {
    false
}

/// Whether `pid` is a live process. Unix signal 0 reports `EPERM` for a live
/// process owned by another user, which must stay alive rather than be
/// mistaken for a dead holder; every other answer but "no such process"
/// stays alive too. Where liveness cannot be asked, every pid is alive: a
/// lock or a lease is never taken from an owner this cannot see die.
///
/// One reader for every holder a crash can strand — a write lease
/// (`tools::file_write_lease`) and a settings lock ([`SettingsFileLock`]).
#[cfg(unix)]
#[must_use]
pub fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    process_alive_from_probe(nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        None,
    ))
}

/// [`process_alive`]'s reading of one signal-0 probe.
#[cfg(unix)]
#[must_use]
pub fn process_alive_from_probe(result: Result<(), nix::errno::Errno>) -> bool {
    !matches!(result, Err(nix::errno::Errno::ESRCH))
}

/// See the Unix twin: an owner this cannot see die is alive.
#[cfg(not(unix))]
#[must_use]
pub fn process_alive(_pid: u32) -> bool {
    true
}
