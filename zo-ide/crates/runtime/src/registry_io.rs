//! Shared atomic JSON persistence for the in-process registries
//! ([`super::task_registry`] and [`super::team_cron_registry`]). Each registry
//! stores a single serializable "inner" value file under `.zo/registries/`,
//! so the path resolution, load, and atomic save logic is shared here rather
//! than copied per registry.
//!
//! Multi-process safety. Every persisting call in this repo loads a registry
//! file once at startup, mutates an in-memory copy, then rewrites the whole
//! file. Two Zo processes editing the same registry would therefore lose
//! each other's inserts: whoever renames last wins. To prevent that lost
//! update, [`save_registry_inner`] serializes writers across processes with an
//! exclusive advisory lock on a sibling `<file>.lock`, and while holding it
//! re-reads the on-disk value and merges it into the value being written (see
//! [`MergeInto`]). The lock is *mandatory*: if it cannot be acquired within
//! [`LOCK_ACQUIRE_BUDGET`], the save fails with an error rather than writing
//! unlocked, so the lost-update guarantee (and any counter/revision invariant a
//! merge relies on) is never silently broken. A successful save therefore holds
//! the lock across the entire read → merge → write, so no peer can slip a
//! rewrite between our reload and our rename.
//!
//! Non-Unix platforms. The cross-process advisory lock and the no-symlink
//! secure-fs path are Unix-only. On other platforms there is no cross-process
//! serialization: [`save_registry_inner`] still merges the on-disk copy in and
//! writes atomically (safe for a single process), but two concurrent processes
//! can still lose each other's writes. This is a deliberately honest, narrower
//! guarantee, not a silent one.
//!
//! Deletion durability (tombstones). A plain union merge cannot distinguish
//! "an id I never held" from "an id I deleted": re-inserting every on-disk id
//! would resurrect removals off disk. Registries that support removal therefore
//! persist tombstones (id → the monotonic `revision` at which it was removed)
//! and stamp each live entry with the `revision` it was last written at, so
//! [`MergeInto`] can keep a deletion authoritative. Because the merge discards
//! any live entry an equal-or-newer tombstone covers — and vice versa — the
//! reconciliation is order-independent and idempotent. Tombstones are
//! unbounded and durable: they are never evicted. An eviction cap cannot be made
//! safe without cross-process coordination (a generation/GC handshake), because
//! any surviving tombstone might still be the only thing standing between a
//! deleted id and an arbitrarily-stale peer that has held that id on disk,
//! untouched, since before the tombstone was minted. Keeping every tombstone
//! makes "removal is permanent" hold against peers of any staleness. The cost is
//! that the `tombstones` map — and therefore the on-disk file — grows without
//! bound in a registry that churns through unique ids forever; this correctness-
//! for-growth tradeoff is deliberate. (A tombstone is still dropped the moment
//! the same id is re-created at a strictly newer revision, so a create/delete
//! cycle on a *reused* id does not accumulate.)
//!
//! Revisions vs. wall-clock seconds. Merge decisions use a monotonic per-
//! registry `revision` counter, not `updated_at` (which has only whole-second
//! resolution). Two updates to the same id within the same second would tie on
//! seconds and one would be silently dropped; because the mandatory lock
//! serializes committed writers and every mutation advances the shared,
//! max-merged `revision`, distinct writes get distinct revisions and the newer
//! one wins deterministically.
//!
//! Durability. The payload is written to a per-process unique temp file in the
//! same directory and atomically renamed over the target. A registry that asks
//! for [`Durability::Flushed`] also fsyncs the data before the rename and the
//! parent directory after it, so the write survives a power loss; one that asks
//! for [`Durability::Published`] stops at the rename — the new inode is visible
//! to every process at once, and only a power loss inside the OS flush window
//! can lose it. The task registry is published: a background spawn commits
//! several times and each flushed commit costs two `F_FULLFSYNC` on macOS,
//! while nothing in it must outlive a power loss. On Unix all of this goes
//! through [`crate::secure_fs`], which additionally refuses to follow symlinks
//! and keeps the file owner-only.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::de::DeserializeOwned;
use serde::Serialize;

pub use crate::secure_fs::Durability;

#[cfg(unix)]
use crate::secure_fs;

/// How long to spin trying to acquire the cross-process registry lock before
/// giving up. The lock is an `flock`, so it is released automatically when a
/// crashed holder's file descriptors close — there is no stale lock file to
/// reap and no risk of a dead process wedging writers forever. This bound only
/// guards against a *live* peer holding the lock for an unexpectedly long time;
/// exceeding it surfaces an error to the caller rather than writing unlocked,
/// because an unlocked write would drop a concurrent peer's update and could
/// commit a stale `revision`/counter — the very lost-update the lock exists to
/// prevent. A user-facing mutation reports the failure instead of silently
/// corrupting the merge invariant.
#[cfg(unix)]
const LOCK_ACQUIRE_BUDGET: std::time::Duration = std::time::Duration::from_millis(500);
#[cfg(unix)]
const LOCK_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

/// A registry "inner" value that knows how to fold a concurrently-persisted
/// on-disk copy of itself into `self` before a rewrite, so a peer process's
/// updates are not clobbered. Implementations union their collections and keep
/// whichever side is newer; a monotonic counter takes the max. `self` is the
/// value this process is about to write and wins ties.
pub(crate) trait MergeInto {
    fn merge_in(&mut self, on_disk: Self);
}

/// Where a registry file lives for a workspace: `<cwd>/.zo/registries/<file>`.
/// The one spelling — the session's default below and a process registering
/// on a workspace's behalf (`zo cron`) both come here.
pub(crate) fn registry_path_in(cwd: &Path, file_name: &str) -> PathBuf {
    cwd.join(".zo").join("registries").join(file_name)
}

/// Default on-disk location for a registry file, under `.zo/registries/` in
/// the current working directory.
pub(crate) fn default_registry_path(file_name: &str) -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .map(|cwd| registry_path_in(&cwd, file_name))
}

/// Load a registry's inner value, returning `None` when the file is absent or
/// cannot be parsed (callers fall back to an empty registry).
pub(crate) fn load_registry_inner<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let contents = read_registry_file(path)?;
    serde_json::from_str(&contents).ok()
}

/// Read a registry file's contents, refusing to follow symlinks on Unix (a
/// registry directory an attacker can influence must not redirect a load to an
/// arbitrary file). `None` on any absence, symlink rejection, or read error —
/// callers treat that as an empty registry.
fn read_registry_file(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        let (root, relative) = split_root_relative(path)?;
        secure_fs::read_to_string_no_symlink(&root, &relative).ok()
    }
    #[cfg(not(unix))]
    {
        std::fs::read_to_string(path).ok()
    }
}

/// Atomically persist a registry's inner value, serializing cross-process
/// writers and merging any concurrent on-disk update in first so it is not
/// lost. `inner` is mutated in place with the merge result.
pub(crate) fn save_registry_inner<T>(path: &Path, inner: &mut T) -> Result<(), String>
where
    T: Serialize + DeserializeOwned + MergeInto,
{
    // `save_registry_inner` has no mutation to rebase and no fail-closed latch
    // (it is the load-time sanitize/quarantine writer). A persistence failure
    // here is surfaced directly; the caller (warn-once) decides how to react.
    let never_latched = AtomicBool::new(false);
    commit_registry_mutation(path, &never_latched, inner, Durability::Flushed, |_| {}).1
}

/// Serialize a mutation against the latest on-disk state under the cross-process
/// lock: acquire the lock, merge the newest disk copy into `inner`, run
/// `mutate` (which may allocate ids/revisions rebased onto that merged state),
/// then atomically publish the result. Holding the lock across
/// merge→mutate→write is what serializes two processes that loaded the same base
/// revision: the second one rebases its id/revision allocation onto the first
/// one's committed write instead of colliding at `base + 1`.
///
/// Returns `(mutation_result, persistence_status)`. `mutate` runs **exactly
/// once** and its result is always returned so callers never panic. Persistence
/// is governed by a process-local **fail-closed latch** (`persist_disabled`)
/// that upholds two contracts at once — no false success and no peer clobber:
///
/// - Latch already set (a prior persist on this instance failed): the lock,
///   disk merge, and write are all skipped. `mutate` runs against the current
///   in-memory `inner` and the change is **kept** (so local reads stay truthful
///   and consistent), but nothing is written. Status is `Err`.
/// - Fresh persistence failure (path setup, lock acquire, serialize, or write):
///   the candidate mutation is **kept in memory** (no rollback — the returned
///   id/entry must exist for local reads), the latch is **set** so this instance
///   never writes to disk again, and status is `Err`. Because no partial or
///   later write can occur, a peer's committed on-disk state is never clobbered.
/// - Success: `inner` reflects the merged disk state plus `mutate`; status `Ok`.
///
/// A latched instance is intentionally memory-only until the process restarts
/// and reloads from disk; that reload is the recovery path. The lock is only
/// ever held around a real write, so the "no unlocked write" guarantee holds:
/// on any failure nothing is written at all.
pub(crate) fn commit_registry_mutation<T, R>(
    path: &Path,
    persist_disabled: &AtomicBool,
    inner: &mut T,
    durability: Durability,
    mutate: impl FnOnce(&mut T) -> R,
) -> (R, Result<(), String>)
where
    T: Serialize + DeserializeOwned + MergeInto,
{
    // Once this instance has failed to persist, it is fail-closed: never touch
    // disk again (no merge, no write, no lock). Apply the mutation in memory so
    // local reads remain consistent, and report the latch as an error.
    if persist_disabled.load(Ordering::Relaxed) {
        let out = mutate(inner);
        return (
            out,
            Err("registry persistence is disabled after a prior failure".to_string()),
        );
    }

    // Determine whether disk is reachable *before* running the single-shot
    // mutation. `prepare` covers path setup and (on Unix) cross-process lock
    // acquisition; its `Err` means we must fail closed without writing. Binding
    // the lock guard here keeps it held across the merge→mutate→write below.
    let prepare: Result<(), String> = path
        .parent()
        .ok_or_else(|| format!("registry path has no parent: {}", path.display()))
        .and_then(|parent| std::fs::create_dir_all(parent).map_err(|error| error.to_string()));

    #[cfg(unix)]
    let prepare = prepare.and_then(|()| acquire_registry_lock(path));
    // `_lock` is dropped (and the advisory lock released) at the end of this
    // function. On non-Unix there is no cross-process lock (see module docs);
    // the merge below still protects a single process.
    #[cfg(unix)]
    let (_lock, prepare) = match prepare {
        Ok(lock) => (Some(lock), Ok(())),
        Err(error) => (None, Err(error)),
    };

    if let Err(error) = prepare {
        // Disk is unreachable and `mutate` has not run yet. Keep the mutation in
        // memory (truthful local reads) and latch fail-closed so no write ever
        // happens for this instance.
        let out = mutate(inner);
        persist_disabled.store(true, Ordering::Relaxed);
        return (out, Err(error));
    }

    if let Some(on_disk) = load_registry_inner::<T>(path) {
        inner.merge_in(on_disk);
    }

    // `mutate` runs here, exactly once.
    let out = mutate(inner);

    match serde_json::to_string(inner)
        .map_err(|error| error.to_string())
        .and_then(|serialized| write_atomic(path, serialized.as_bytes(), durability))
    {
        Ok(()) => (out, Ok(())),
        Err(error) => {
            // The write did not land, but the merged+mutated state is kept in
            // memory so the returned result stays truthful. Latch fail-closed so
            // this instance never writes again — that is what prevents a later
            // commit from clobbering a peer with this un-persisted state.
            persist_disabled.store(true, Ordering::Relaxed);
            (out, Err(error))
        }
    }
}

/// Write `contents` to `path` atomically: unique same-directory temp file,
/// rename over the target. [`Durability::Flushed`] also fsyncs the data before
/// the rename and the parent directory after it. On Unix this refuses to
/// follow symlinks and keeps the file owner-only via [`crate::secure_fs`].
fn write_atomic(path: &Path, contents: &[u8], durability: Durability) -> Result<(), String> {
    #[cfg(unix)]
    {
        let (root, relative) =
            split_root_relative(path).ok_or_else(|| "registry path has no file name".to_string())?;
        secure_fs::write_atomic_owner_only_with(&root, &relative, contents, durability)
            .map_err(|error| error.to_string())?;
        if durability == Durability::Flushed {
            // The rename above published the new inode; fsync the directory so
            // the rename survives a crash. Best-effort: some filesystems reject
            // directory fsync, and the data is already durable on the file.
            let _ = secure_fs::sync_parent_directory(&root, &relative);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        use std::io::Write as _;
        let parent = path
            .parent()
            .ok_or_else(|| "registry path has no parent".to_string())?;
        let temp_path = unique_temp_path(path);
        let write = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&temp_path)?;
            file.write_all(contents)?;
            if durability == Durability::Flushed {
                file.sync_all()?;
            }
            Ok(())
        })();
        if let Err(error) = write {
            let _ = std::fs::remove_file(&temp_path);
            return Err(error.to_string());
        }
        std::fs::rename(&temp_path, path).map_err(|error| {
            let _ = std::fs::remove_file(&temp_path);
            error.to_string()
        })?;
        if durability == Durability::Flushed {
            let _ = std::fs::File::open(parent).and_then(|dir| dir.sync_all());
        }
        Ok(())
    }
}

/// A collision-free temp path in the target's own directory. Encodes the
/// process id and a monotonic counter so two Zo processes (or two threads)
/// never race on a shared `<file>.tmp` name — the fixed name was the original
/// bug this module carried.
#[cfg(not(unix))]
fn unique_temp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::AtomicU64;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("registry");
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ))
}

/// Split an absolute registry path into a trusted root (its parent directory)
/// and the relative file name that [`crate::secure_fs`] traverses no-follow.
#[cfg(unix)]
fn split_root_relative(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let parent = path.parent()?;
    let file_name = path.file_name()?;
    Some((parent.to_path_buf(), PathBuf::from(file_name)))
}

/// Acquire the exclusive cross-process registry lock, spinning up to
/// [`LOCK_ACQUIRE_BUDGET`]. Returns the held lock, or an error when the budget
/// is exhausted (a live peer is holding it) or the lock cannot be created. The
/// caller must *not* write on error: an unlocked write would defeat the
/// lost-update and revision-monotonicity guarantees the lock exists to keep.
#[cfg(unix)]
fn acquire_registry_lock(path: &Path) -> Result<secure_fs::ExclusiveFileLock, String> {
    let (root, relative) =
        split_root_relative(path).ok_or_else(|| "registry path has no file name".to_string())?;
    let lock_name = {
        let file_name = relative.to_string_lossy();
        PathBuf::from(format!("{file_name}.lock"))
    };
    let deadline = std::time::Instant::now() + LOCK_ACQUIRE_BUDGET;
    loop {
        match secure_fs::try_lock_owner_only(&root, &lock_name) {
            Ok(Some(lock)) => return Ok(lock),
            // Held by another process: retry until the budget runs out.
            Ok(None) => {}
            // The lock file itself could not be created or validated (for
            // example a hostile non-regular file at the lock path). Surface the
            // error; the registry mutation is not committed unlocked.
            Err(error) => {
                return Err(format!(
                    "could not acquire registry lock {}: {error}",
                    lock_name.display()
                ));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "timed out after {}ms acquiring registry lock {} (held by another process)",
                LOCK_ACQUIRE_BUDGET.as_millis(),
                lock_name.display()
            ));
        }
        std::thread::sleep(LOCK_RETRY_INTERVAL);
    }
}

/// Persist a registry's inner value, warning on stderr at most once per
/// failure episode. The first failure logs path+cause; repeats stay silent
/// (a hot `append_output` loop against an unwritable path would otherwise
/// emit one warning per chunk); a successful save re-arms the latch so the
/// next distinct failure episode warns again.
pub(crate) fn save_registry_inner_warn_once<T>(
    label: &str,
    path: &Path,
    inner: &mut T,
    warned: &AtomicBool,
) where
    T: Serialize + DeserializeOwned + MergeInto,
{
    match save_registry_inner(path, inner) {
        Ok(()) => warned.store(false, Ordering::Relaxed),
        Err(error) => {
            if !warned.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "[zo] warning: failed to persist {label} at {}: {} (suppressing repeats until a save succeeds)",
                    path.display(),
                    error
                );
            }
        }
    }
}

/// Warn-once wrapper over [`commit_registry_mutation`] that also returns the
/// persistence status so a caller whose public API is `Result<_, String>` can
/// surface a persistence failure as an error instead of a silent success. `mutate`
/// runs exactly once; on failure the candidate mutation is kept in memory and the
/// instance is latched fail-closed (see [`commit_registry_mutation`]).
pub(crate) fn commit_registry_mutation_warn_once_status<T, R>(
    label: &str,
    path: &Path,
    persist_disabled: &AtomicBool,
    inner: &mut T,
    warned: &AtomicBool,
    durability: Durability,
    mutate: impl FnOnce(&mut T) -> R,
) -> (R, Result<(), String>)
where
    T: Serialize + DeserializeOwned + MergeInto,
{
    let (out, status) =
        commit_registry_mutation(path, persist_disabled, inner, durability, mutate);
    match &status {
        Ok(()) => warned.store(false, Ordering::Relaxed),
        Err(error) => {
            if !warned.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "[zo] warning: failed to persist {label} at {}: {} (suppressing repeats until a save succeeds)",
                    path.display(),
                    error
                );
            }
        }
    }
    (out, status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("zo-registry-io-{tag}-{nanos}-{sequence}"))
    }

    #[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct Inner {
        items: HashMap<String, u64>,
        counter: u64,
    }

    impl MergeInto for Inner {
        fn merge_in(&mut self, on_disk: Self) {
            for (key, disk_updated) in on_disk.items {
                match self.items.get(&key) {
                    // Keep whichever side is newer; ties keep ours.
                    Some(mine) if *mine >= disk_updated => {}
                    _ => {
                        self.items.insert(key, disk_updated);
                    }
                }
            }
            self.counter = self.counter.max(on_disk.counter);
        }
    }

    fn save(path: &Path, inner: &mut Inner) {
        save_registry_inner(path, inner).expect("save should succeed");
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = unique_dir("round-trip");
        let path = dir.join("regs").join("tasks.json");
        let mut inner = Inner {
            items: HashMap::from([("a".to_string(), 1)]),
            counter: 3,
        };
        save(&path, &mut inner);
        let loaded: Inner = load_registry_inner(&path).expect("load");
        assert_eq!(loaded, inner);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_fixed_tmp_file_is_left_behind() {
        // The original module wrote a fixed `<file>.json.tmp` sibling that two
        // processes would collide on. After a save, no such fixed temp exists.
        let dir = unique_dir("no-fixed-tmp");
        let path = dir.join("tasks.json");
        let mut inner = Inner::default();
        save(&path, &mut inner);
        let leftover = path.with_extension("json.tmp");
        assert!(
            !leftover.exists(),
            "fixed json.tmp temp must not survive a save"
        );
        // The parent should contain only the published file (plus possibly the
        // persistent lock file), never a stray `.tmp-` remnant.
        let strays: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .expect("read dir")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(strays.is_empty(), "no temp remnants: {strays:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_on_disk_insert_is_merged_not_lost() {
        // Simulate a peer process: this process loaded an empty registry and is
        // about to write its own entry, but the file on disk already gained a
        // different entry from a peer. The peer's entry must survive our write.
        let dir = unique_dir("merge");
        let path = dir.join("tasks.json");

        let mut peer = Inner {
            items: HashMap::from([("peer".to_string(), 10)]),
            counter: 5,
        };
        save(&path, &mut peer);

        // Our in-memory state never saw the peer's entry.
        let mut mine = Inner {
            items: HashMap::from([("mine".to_string(), 20)]),
            counter: 2,
        };
        save(&path, &mut mine);

        let merged: Inner = load_registry_inner(&path).expect("load merged");
        assert_eq!(merged.items.get("peer"), Some(&10), "peer entry preserved");
        assert_eq!(merged.items.get("mine"), Some(&20), "our entry present");
        assert_eq!(merged.counter, 5, "counter takes the max across processes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_local_entry_wins_over_stale_disk_copy() {
        let dir = unique_dir("recency");
        let path = dir.join("tasks.json");
        let mut old = Inner {
            items: HashMap::from([("x".to_string(), 1)]),
            counter: 1,
        };
        save(&path, &mut old);
        let mut newer = Inner {
            items: HashMap::from([("x".to_string(), 9)]),
            counter: 1,
        };
        save(&path, &mut newer);
        let merged: Inner = load_registry_inner(&path).expect("load");
        assert_eq!(merged.items.get("x"), Some(&9), "newer update wins");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn persisted_registry_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = unique_dir("perms");
        let path = dir.join("tasks.json");
        let mut inner = Inner::default();
        save(&path, &mut inner);
        let mode = std::fs::metadata(&path).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "registry file must be owner-only");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_registry_path_is_rejected_on_load() {
        // A registry file that is actually a symlink must not be followed: the
        // no-follow load returns None (empty registry) rather than reading
        // through the link to an arbitrary target.
        let dir = unique_dir("symlink");
        std::fs::create_dir_all(&dir).expect("dir");
        let target = dir.join("secret.json");
        std::fs::write(&target, br#"{"items":{"leaked":1},"counter":9}"#).expect("target");
        let link = dir.join("tasks.json");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(
            load_registry_inner::<Inner>(&link).is_none(),
            "no-follow load must not read through a symlink"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_failure_after_mutation_returns_result_without_panicking() {
        // A serialize/write failure that happens *after* the mutation closure
        // already ran must not panic and must not discard the in-memory result:
        // `commit_registry_mutation` runs the closure exactly once and returns
        // its value paired with an `Err` persistence status. We force the write
        // to fail by making the registry path itself a directory, so lock
        // acquisition and parent-dir setup succeed but the final atomic rename
        // over the target cannot.
        let dir = unique_dir("write-fail");
        let path = dir.join("tasks.json");
        std::fs::create_dir_all(&path).expect("create dir at the registry path");

        let mut inner = Inner {
            items: HashMap::from([("k".to_string(), 1)]),
            counter: 1,
        };
        let persist_disabled = AtomicBool::new(false);
        let runs = std::cell::Cell::new(0u32);
        let (out, status) =
            commit_registry_mutation(&path, &persist_disabled, &mut inner, Durability::Flushed, |inner| {
                runs.set(runs.get() + 1);
                inner.counter += 1;
                inner.counter
            });

        assert_eq!(runs.get(), 1, "mutation closure must run exactly once");
        assert_eq!(out, 2, "the in-memory mutation result must be returned");
        assert_eq!(
            inner.counter, 2,
            "the mutation is kept in memory (fail-closed keeps local reads truthful)"
        );
        assert!(
            status.is_err(),
            "a failed write must be reported as an Err persistence status"
        );
        assert!(
            persist_disabled.load(Ordering::Relaxed),
            "a failed write must latch the instance fail-closed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn warn_once_survives_write_failure_and_returns_mutation_result() {
        // The warn-once wrapper must not panic when persistence fails after the
        // mutation ran: it returns the closure's value and only flips the
        // warn-once flag. This is the exact path the public registry mutation
        // methods take.
        let dir = unique_dir("warn-once-write-fail");
        let path = dir.join("tasks.json");
        std::fs::create_dir_all(&path).expect("create dir at the registry path");

        let mut inner = Inner::default();
        let persist_disabled = AtomicBool::new(false);
        let warned = AtomicBool::new(false);
        let runs = std::cell::Cell::new(0u32);
        let (out, status) = commit_registry_mutation_warn_once_status(
            "test registry",
            &path,
            &persist_disabled,
            &mut inner,
            &warned,
            Durability::Flushed,
            |inner| {
                runs.set(runs.get() + 1);
                inner.counter += 7;
                inner.counter
            },
        );

        assert_eq!(runs.get(), 1, "mutation closure must run exactly once");
        assert_eq!(out, 7, "warn-once wrapper must return the mutation result");
        assert_eq!(
            inner.counter, 7,
            "the mutation is kept in memory even when the write fails"
        );
        assert!(
            status.is_err(),
            "a persistence failure must be returned as a status error"
        );
        assert!(
            warned.load(Ordering::Relaxed),
            "a persistence failure must set the warn-once flag"
        );
        assert!(
            persist_disabled.load(Ordering::Relaxed),
            "a persistence failure must latch the instance fail-closed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn held_lock_fails_the_save_within_the_budget_without_writing_unlocked() {
        // The cross-process lock is mandatory: if a peer holds it for the whole
        // budget, the save must *fail* (returning the timeout error) rather than
        // fall back to an unlocked write, which would drop the peer's update and
        // could commit a stale revision/counter. It must also give up bounded by
        // the budget, not hang a user-facing registry mutation.
        let dir = unique_dir("lock-budget");
        let path = dir.join("tasks.json");
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");
        let (root, relative) = split_root_relative(&path).expect("split");
        let lock_name = PathBuf::from(format!("{}.lock", relative.to_string_lossy()));
        let held = secure_fs::try_lock_owner_only(&root, &lock_name)
            .expect("lock call")
            .expect("lock acquired");
        let start = std::time::Instant::now();
        let mut inner = Inner {
            items: HashMap::from([("k".to_string(), 1)]),
            counter: 1,
        };
        let result = save_registry_inner(&path, &mut inner);
        let elapsed = start.elapsed();
        let error = result.expect_err("save must fail while the lock is held");
        assert!(
            error.contains("acquiring registry lock"),
            "save must fail with the lock-timeout error, got: {error}"
        );
        assert!(
            elapsed >= LOCK_ACQUIRE_BUDGET,
            "save must spend the full lock budget before giving up"
        );
        assert!(
            elapsed < LOCK_ACQUIRE_BUDGET * 4,
            "save must give up bounded by the lock budget, not hang"
        );
        // No unlocked write happened: the registry file was never published.
        assert!(
            !path.exists(),
            "a lock-timeout must not write the registry file unlocked"
        );
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
