//! Cross-process file write-lease registry (track 4-2).
//!
//! When two agents — including two separate `zo` *processes* sharing one
//! working tree — edit the same file, the later writer silently clobbers the
//! earlier one's uncommitted work. (The documented incidents were exactly this:
//! a concurrent `zo` session in the same tree reverting another's edits.)
//! Track 4-1 guards *whole-tree* commands; this guards *the same file*.
//!
//! A lease is a small JSON file under the workspace's Zo state directory
//! (`zo_project_state_dir(cwd)/locks/writes/<hash>.lease`), named by a hash
//! of the target's absolute path. Because the state dir is partitioned by a
//! stable workspace slug (not stored inside the worktree), two processes editing
//! the same tree resolve to the *same* lease directory and therefore see each
//! other's leases — which an in-process registry never could, since every agent
//! builds its own [`crate::ToolContext`].
//!
//! Acquisition publishes a completed, synced candidate atomically. On Unix,
//! replacement and release lock the current lease inode, so a stale claimant can
//! never overwrite or delete a lease acquired after it inspected the old record.
//! The lock is advisory and tied to the open file descriptor; it is not a second
//! persistent lock file that can itself become stale.
//!
//! The whole mechanism is **opt-in**: a tool context with no lease owner
//! (`owner == None`, the solo default) skips it entirely, so single-agent and
//! single-process workflows are completely unaffected.

use std::fs;
use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Subdirectory (under the project state dir) holding write leases.
const LEASE_SUBDIR: &str = "locks/writes";

/// How long a lease stays valid without renewal (60 minutes). A writer that
/// crashes or is killed without cleaning up its lease blocks the path for at
/// most this long before the next writer reclaims it. Each successful
/// `acquire`/renewal pushes the expiry forward, so a long-running agent that
/// keeps editing the same file never expires out from under itself; the window
/// is generous so an agent pausing on one file between edits is not preempted.
const LEASE_TTL_MS: u64 = 60 * 60 * 1000;

static CANDIDATE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// On-disk lease record. Identifies who holds the path, the OS process (so a
/// dead holder can be reclaimed), and when the lease was taken / expires.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct WriteLease {
    /// Logical owner: a foreground session id or a sub-agent id. Two writers
    /// with the same owner are the same actor and never conflict.
    pub owner: String,
    /// OS process id of the holder, so a stale lease left by a crashed process
    /// can be reclaimed even before its TTL elapses.
    pub pid: u32,
    /// Absolute path the lease covers, stored for diagnostics (the file name is
    /// only a hash).
    pub path: String,
    /// Monotonic-ish wall-clock acquisition time (ms since epoch).
    pub acquired_ms: u64,
    /// Wall-clock expiry (ms since epoch). Past this, any writer may reclaim.
    pub expiry_ms: u64,
}

/// Outcome of attempting to acquire a write lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LeaseOutcome {
    /// The caller holds the lease (freshly taken, renewed, or reclaimed).
    Acquired,
    /// Another live owner holds the lease; the write must not proceed.
    Conflict(WriteLease),
}

/// Current wall-clock time in milliseconds since the Unix epoch. A clock before
/// the epoch (impossible in practice) degrades to 0, which only makes leases
/// look already-expired — fail-open toward reclaim, never a false conflict.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Stable lease file name for an absolute path: the hex SHA-256 of the path
/// bytes plus a `.lease` suffix. Hashing keeps the name filesystem-safe and
/// fixed-length regardless of how deep or unusual the real path is.
fn lease_file_name(abs_path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(abs_path.to_string_lossy().as_bytes());
    format!("{:x}.lease", hasher.finalize())
}

/// Directory holding this workspace's write leases.
fn lease_dir(cwd: &Path) -> PathBuf {
    runtime::zo_project_state_dir(cwd).join(LEASE_SUBDIR)
}

/// Whether `pid` is a live process. Unix signal-0 reports `EPERM` for a live
/// process owned by another user, which must remain a conflict rather than being
/// mistaken for a dead holder. Other probe failures stay conservative (alive).
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    process_alive_from_probe(nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        None,
    ))
}

#[cfg(unix)]
fn process_alive_from_probe(result: Result<(), nix::errno::Errno>) -> bool {
    !matches!(result, Err(nix::errno::Errno::ESRCH))
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    true
}

/// Whether an existing lease may be taken over by `owner` now: the caller
/// already owns it, it has expired, or its holder process is gone. A live lease
/// held by a different owner is *not* reclaimable.
fn is_reclaimable(existing: &WriteLease, owner: &str, now: u64) -> bool {
    existing.owner == owner || now >= existing.expiry_ms || !process_alive(existing.pid)
}

/// Build the lease record for `owner` taking `abs_path` at `now`.
fn new_lease(abs_path: &Path, owner: &str, now: u64) -> WriteLease {
    WriteLease {
        owner: owner.to_string(),
        pid: std::process::id(),
        path: abs_path.to_string_lossy().into_owned(),
        acquired_ms: now,
        expiry_ms: now.saturating_add(LEASE_TTL_MS),
    }
}

/// Write and sync a unique candidate in the lease directory before it is linked
/// or renamed into the public lease name. Candidates left by a crashed process
/// are never consulted, so they cannot block or be mistaken for ownership.
fn write_completed_candidate(lease_path: &Path, lease: &WriteLease) -> std::io::Result<PathBuf> {
    let body = serde_json::to_vec_pretty(lease).map_err(std::io::Error::other)?;
    for _ in 0..64 {
        let nonce = CANDIDATE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = lease_path.with_extension(format!(
            "candidate.{}.{}",
            std::process::id(),
            nonce
        ));
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        #[cfg(windows)]
        if let Err(error) = core_types::paths::restrict_permissions_owner_only(&candidate) {
            drop(file);
            let _ = fs::remove_file(&candidate);
            return Err(error);
        }
        if let Err(error) = file.write_all(&body).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&candidate);
            return Err(error);
        }
        return Ok(candidate);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "unable to allocate a unique lease candidate",
    ))
}

fn remove_candidate(candidate: &Path) {
    let _ = fs::remove_file(candidate);
}

fn read_lease(file: &mut fs::File) -> std::io::Result<WriteLease> {
    file.rewind()?;
    let mut body = String::new();
    file.read_to_string(&mut body)?;
    serde_json::from_str(&body).map_err(std::io::Error::other)
}

#[cfg(unix)]
fn lock_lease(file: &fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;

    #[allow(deprecated)]
    nix::fcntl::flock(file.as_raw_fd(), nix::fcntl::FlockArg::LockExclusive)
        .map_err(std::io::Error::from)
}

#[cfg(windows)]
fn lock_lease(file: &fs::File) -> std::io::Result<()> {
    file.lock()
}

#[cfg(unix)]
fn lease_path_still_names(file: &fs::File, lease_path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt as _;

    let opened = file.metadata()?;
    match fs::metadata(lease_path) {
        Ok(current) => Ok(opened.dev() == current.dev() && opened.ino() == current.ino()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn lease_path_still_names(file: &fs::File, lease_path: &Path) -> std::io::Result<bool> {
    core_types::paths::windows_file_handle_matches_path(file, lease_path)
}

#[cfg(not(any(unix, windows)))]
#[allow(clippy::unnecessary_wraps)]
fn lease_path_still_names(_file: &fs::File, lease_path: &Path) -> std::io::Result<bool> {
    Ok(lease_path.exists())
}

/// Install `candidate` over the inspected lease. Unix `rename` replaces the
/// destination atomically while its inode lock excludes other protocol users.
/// Other targets retain fail-open advisory behavior if replacement is rejected.
fn replace_lease(candidate: &Path, lease_path: &Path) -> std::io::Result<()> {
    fs::rename(candidate, lease_path)
}

/// Try to acquire (or renew/reclaim) the write lease for `abs_path` on behalf of
/// `owner`. Returns [`LeaseOutcome::Acquired`] when the caller may write, or
/// [`LeaseOutcome::Conflict`] naming the live holder when it may not.
///
/// Best-effort by design: filesystem and advisory-lock errors fail *open*
/// (return `Acquired`) so the lease layer can never turn a transient I/O hiccup
/// into a hard write failure. A valid active lease, however, is never replaced
/// or removed without first holding its inode lock and rechecking that the path
/// still names that inode.
pub(crate) fn acquire(abs_path: &Path, owner: &str, cwd: &Path) -> LeaseOutcome {
    let dir = lease_dir(cwd);
    #[cfg(windows)]
    if core_types::paths::ensure_windows_owner_only_dir_no_follow(&dir).is_err() {
        return LeaseOutcome::Acquired;
    }
    #[cfg(not(windows))]
    if fs::create_dir_all(&dir).is_err() {
        return LeaseOutcome::Acquired;
    }
    let lease_path = dir.join(lease_file_name(abs_path));
    let lease = new_lease(abs_path, owner, now_ms());
    let Ok(candidate) = write_completed_candidate(&lease_path, &lease) else {
        return LeaseOutcome::Acquired;
    };

    loop {
        // Hard-linking a completed candidate is an atomic create-if-absent, so
        // no observer can ever read a newly-created empty or partial JSON file.
        match fs::hard_link(&candidate, &lease_path) {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Ok(()) | Err(_) => {
                remove_candidate(&candidate);
                return LeaseOutcome::Acquired;
            }
        }

        #[cfg(not(windows))]
        let existing_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lease_path);
        #[cfg(windows)]
        let existing_file =
            core_types::paths::open_windows_owner_only_regular_file_read_write(&lease_path);
        let Ok(mut existing_file) = existing_file else {
            // The lease disappeared between hard-link and open. Retry the same
            // completed candidate; other I/O errors remain advisory fail-open.
            if lease_path.exists() {
                remove_candidate(&candidate);
                return LeaseOutcome::Acquired;
            }
            continue;
        };

        #[cfg(any(unix, windows))]
        if lock_lease(&existing_file).is_err() {
            remove_candidate(&candidate);
            return LeaseOutcome::Acquired;
        }

        match lease_path_still_names(&existing_file, &lease_path) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => {
                remove_candidate(&candidate);
                return LeaseOutcome::Acquired;
            }
        }

        let existing = read_lease(&mut existing_file).ok();
        if let Some(holder) = existing.as_ref().filter(|holder| !is_reclaimable(holder, owner, now_ms())) {
            remove_candidate(&candidate);
            return LeaseOutcome::Conflict(holder.clone());
        }

        // A corrupt legacy file has no completed owner record. Holding the
        // inode lock and checking its identity makes replacing it safe; a racing
        // stale reclaimer wakes, sees our new inode, and reports its live owner.
        if replace_lease(&candidate, &lease_path).is_ok() {
            return LeaseOutcome::Acquired;
        }
        remove_candidate(&candidate);
        return LeaseOutcome::Acquired;
    }
}

#[cfg(unix)]
fn release_lease_if_owned(lease_path: &Path, owner: &str) {
    let Ok(mut file) = fs::OpenOptions::new().read(true).write(true).open(lease_path) else {
        return;
    };
    if lock_lease(&file).is_err()
        || !matches!(lease_path_still_names(&file, lease_path), Ok(true))
    {
        return;
    }
    if read_lease(&mut file).is_ok_and(|lease| lease.owner == owner) {
        // A waiting reclaimer holds only the old inode. It must recheck the
        // pathname after this unlink, so it cannot delete a lease it did not
        // inspect; a later fresh acquirer has nothing left for this release to
        // remove.
        let _ = fs::remove_file(lease_path);
    }
}

#[cfg(windows)]
fn release_lease_if_owned(lease_path: &Path, owner: &str) {
    let Ok(mut file) =
        core_types::paths::open_windows_owner_only_regular_file_read_write(lease_path)
    else {
        return;
    };
    if lock_lease(&file).is_ok()
        && lease_path_still_names(&file, lease_path).unwrap_or(false)
        && read_lease(&mut file).is_ok_and(|lease| lease.owner == owner)
    {
        let _ = fs::remove_file(lease_path);
    }
}

#[cfg(not(any(unix, windows)))]
fn release_lease_if_owned(lease_path: &Path, owner: &str) {
    let Ok(mut file) = fs::OpenOptions::new().read(true).open(lease_path) else {
        return;
    };
    if read_lease(&mut file).is_ok_and(|lease| lease.owner == owner) {
        let _ = fs::remove_file(lease_path);
    }
}

/// Release every lease currently held by `owner` in this workspace. Called when
/// an agent's run ends so the paths it edited are freed immediately for the next
/// (sequential) agent, instead of staying blocked until their TTL elapses.
///
/// Best-effort: scans the workspace lease directory and releases each matching
/// `.lease` file. A foreign or unparseable lease is left untouched, and any I/O
/// error skips that entry (the TTL / dead-pid reclaim remains the backstop). A
/// no-op when the directory does not exist.
pub(crate) fn release_all_for_owner(owner: &str, cwd: &Path) {
    let Ok(entries) = fs::read_dir(lease_dir(cwd)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "lease") {
            continue;
        }
        release_lease_if_owned(&path, owner);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        acquire, is_reclaimable, lease_dir, lease_file_name, new_lease, release_all_for_owner,
        LeaseOutcome, WriteLease, LEASE_TTL_MS,
    };
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::{SystemTime, UNIX_EPOCH};

    static UNIQ: AtomicU64 = AtomicU64::new(0);

    /// A unique workspace cwd plus a `ZO_STATE_DIR` redirect, so each test's
    /// leases land in their own throwaway directory (the lease dir derives from
    /// `zo_project_state_dir`, which honors `ZO_STATE_DIR`). The env var is
    /// process-global, so callers hold the shared tools env lock first.
    fn isolated_cwd() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let n = UNIQ.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("zo-lease-{nanos}-{n}-{}", std::process::id()));
        let cwd = base.join("ws");
        std::fs::create_dir_all(&cwd).expect("cwd");
        std::env::set_var("ZO_STATE_DIR", base.join("state"));
        cwd
    }

    fn lease_with(owner: &str, pid: u32, expiry_ms: u64) -> WriteLease {
        WriteLease {
            owner: owner.to_string(),
            pid,
            path: "/ws/src/lib.rs".to_string(),
            acquired_ms: 0,
            expiry_ms,
        }
    }

    fn with_isolated_cwd(test: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        test(isolated_cwd());
    }

    #[test]
    fn first_writer_acquires_then_same_owner_renews() {
        with_isolated_cwd(|cwd| {
            let path = Path::new("/ws/src/lib.rs");
            assert_eq!(acquire(path, "agent-a", &cwd), LeaseOutcome::Acquired);
            assert_eq!(acquire(path, "agent-a", &cwd), LeaseOutcome::Acquired);
        });
    }

    #[test]
    fn second_live_owner_conflicts() {
        with_isolated_cwd(|cwd| {
            let path = Path::new("/ws/src/lib.rs");
            assert_eq!(acquire(path, "agent-a", &cwd), LeaseOutcome::Acquired);
            match acquire(path, "agent-b", &cwd) {
                LeaseOutcome::Conflict(holder) => assert_eq!(holder.owner, "agent-a"),
                LeaseOutcome::Acquired => panic!("a live foreign lease must conflict"),
            }
        });
    }

    #[test]
    fn concurrent_initial_acquire_has_exactly_one_owner() {
        with_isolated_cwd(|cwd| {
            let path = Arc::new(std::path::PathBuf::from("/ws/src/lib.rs"));
            let barrier = Arc::new(Barrier::new(2));
            let outcomes = ["agent-a", "agent-b"]
                .into_iter()
                .map(|owner| {
                    let cwd = cwd.clone();
                    let path = Arc::clone(&path);
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        (owner, acquire(&path, owner, &cwd))
                    })
                })
                .collect::<Vec<_>>();
            let outcomes = outcomes
                .into_iter()
                .map(|thread| thread.join().expect("thread"))
                .collect::<Vec<_>>();
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|(_, outcome)| *outcome == LeaseOutcome::Acquired)
                    .count(),
                1
            );
            let winner = outcomes
                .iter()
                .find_map(|(owner, outcome)| (*outcome == LeaseOutcome::Acquired).then_some(*owner))
                .expect("one owner");
            assert!(outcomes.iter().any(|(_, outcome)| matches!(
                outcome,
                LeaseOutcome::Conflict(holder) if holder.owner == winner
            )));
        });
    }

    #[test]
    fn simultaneous_stale_reclaim_has_exactly_one_owner() {
        with_isolated_cwd(|cwd| {
            let path = Arc::new(std::path::PathBuf::from("/ws/src/lib.rs"));
            let lease_path = lease_dir(&cwd).join(lease_file_name(&path));
            std::fs::create_dir_all(lease_path.parent().expect("parent")).expect("dir");
            std::fs::write(
                &lease_path,
                serde_json::to_vec(&lease_with("dead", std::process::id(), 0)).expect("json"),
            )
            .expect("stale lease");

            let barrier = Arc::new(Barrier::new(2));
            let outcomes = ["agent-b", "agent-c"]
                .into_iter()
                .map(|owner| {
                    let cwd = cwd.clone();
                    let path = Arc::clone(&path);
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        (owner, acquire(&path, owner, &cwd))
                    })
                })
                .collect::<Vec<_>>();
            let outcomes = outcomes
                .into_iter()
                .map(|thread| thread.join().expect("thread"))
                .collect::<Vec<_>>();
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|(_, outcome)| *outcome == LeaseOutcome::Acquired)
                    .count(),
                1
            );
            let final_lease: WriteLease = serde_json::from_slice(
                &std::fs::read(&lease_path).expect("completed lease record"),
            )
            .expect("completed JSON");
            assert!(outcomes.iter().any(|(owner, outcome)| {
                *outcome == LeaseOutcome::Acquired && final_lease.owner == *owner
            }));
        });
    }

    #[test]
    fn corrupt_legacy_lease_is_replaced_only_by_a_completed_candidate() {
        with_isolated_cwd(|cwd| {
            let path = Path::new("/ws/src/lib.rs");
            let lease_path = lease_dir(&cwd).join(lease_file_name(path));
            std::fs::create_dir_all(lease_path.parent().expect("parent")).expect("dir");
            std::fs::write(&lease_path, b"{\"owner\":").expect("corrupt lease");

            assert_eq!(acquire(path, "agent-a", &cwd), LeaseOutcome::Acquired);
            let body = std::fs::read(&lease_path).expect("published lease");
            let lease: WriteLease = serde_json::from_slice(&body).expect("complete JSON");
            assert_eq!(lease.owner, "agent-a");
            assert!(!body.is_empty());
        });
    }

    #[test]
    fn release_frees_the_path_for_another_owner() {
        with_isolated_cwd(|cwd| {
            let path = Path::new("/ws/src/lib.rs");
            assert_eq!(acquire(path, "agent-a", &cwd), LeaseOutcome::Acquired);
            release_all_for_owner("agent-a", &cwd);
            assert_eq!(acquire(path, "agent-b", &cwd), LeaseOutcome::Acquired);
        });
    }

    #[test]
    fn release_then_reacquire_leaves_the_new_owner_intact() {
        with_isolated_cwd(|cwd| {
            let path = Path::new("/ws/src/lib.rs");
            assert_eq!(acquire(path, "agent-a", &cwd), LeaseOutcome::Acquired);
            release_all_for_owner("agent-a", &cwd);
            assert_eq!(acquire(path, "agent-b", &cwd), LeaseOutcome::Acquired);
            match acquire(path, "agent-c", &cwd) {
                LeaseOutcome::Conflict(holder) => assert_eq!(holder.owner, "agent-b"),
                LeaseOutcome::Acquired => panic!("release must not remove a reacquired lease"),
            }
        });
    }

    #[test]
    fn release_by_non_owner_is_ignored() {
        with_isolated_cwd(|cwd| {
            let path = Path::new("/ws/src/lib.rs");
            assert_eq!(acquire(path, "agent-a", &cwd), LeaseOutcome::Acquired);
            release_all_for_owner("agent-b", &cwd);
            match acquire(path, "agent-b", &cwd) {
                LeaseOutcome::Conflict(holder) => assert_eq!(holder.owner, "agent-a"),
                LeaseOutcome::Acquired => panic!("non-owner release must not free the lease"),
            }
        });
    }

    #[test]
    fn expired_lease_is_reclaimable_but_live_is_not() {
        let expired = lease_with("agent-a", std::process::id(), 10);
        assert!(is_reclaimable(&expired, "agent-b", 1_000));
        let live = lease_with("agent-a", std::process::id(), 10_000);
        assert!(!is_reclaimable(&live, "agent-b", 1_000));
        assert!(is_reclaimable(&live, "agent-a", 1_000));
    }

    #[test]
    fn dead_pid_lease_is_reclaimable_even_before_ttl() {
        let dead = lease_with("agent-a", 999_999, u64::MAX);
        assert!(
            is_reclaimable(&dead, "agent-b", 1_000),
            "a lease held by a dead process must be reclaimable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn eperm_liveness_probe_is_treated_as_alive() {
        assert!(super::process_alive_from_probe(Err(nix::errno::Errno::EPERM)));
        assert!(!super::process_alive_from_probe(Err(nix::errno::Errno::ESRCH)));
    }

    #[test]
    fn distinct_paths_get_distinct_lease_files() {
        assert_ne!(
            lease_file_name(Path::new("/ws/a.rs")),
            lease_file_name(Path::new("/ws/b.rs")),
        );
        assert_eq!(
            lease_file_name(Path::new("/ws/a.rs")),
            lease_file_name(Path::new("/ws/a.rs")),
        );
        let name = lease_file_name(Path::new("/ws/a.rs"));
        assert_eq!(
            Path::new(&name).extension().and_then(|e| e.to_str()),
            Some("lease"),
        );
    }

    #[test]
    fn new_lease_sets_ttl_window() {
        let lease = new_lease(Path::new("/ws/x.rs"), "agent-a", 1_000);
        assert_eq!(lease.acquired_ms, 1_000);
        assert_eq!(lease.expiry_ms, 1_000 + LEASE_TTL_MS);
        assert_eq!(lease.pid, std::process::id());
    }
}
