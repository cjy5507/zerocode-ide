//! Versioned publishing of bundled plugin trees (t-76269).
//!
//! A publish never renames a directory a reader can reach. Windows refuses to
//! rename a directory while any file inside it is open (os error 5), so the old
//! "swing the installed tree aside, rename the new one in" publish failed
//! whenever another zo process or a running hook held a file of the plugin.
//!
//! Layout inside one install root:
//!
//! ```text
//! <root>/<plugin dir>/                      legacy tree (before t-76269); read while no pointer exists
//! <root>/.versions/.lock                    cross-process publish lock
//! <root>/.versions/retired.json             trees waiting to be removed, kept across restarts
//! <root>/.versions/<plugin dir>/current     pointer: the name of the live version folder
//! <root>/.versions/<plugin dir>/<name>/     one published tree, never changed once the pointer names it
//! <root>/.versions/<plugin dir>/<name>.lease  readers hold a shared lock while they may use <name>
//! <root>/.versions/<plugin dir>/legacy.lease  the same, for the legacy tree
//! ```
//!
//! The registry keeps recording `<root>/<plugin dir>` (the logical path);
//! [`installed_tree`] and [`hold_installed_tree`] are the one place a logical
//! path becomes the tree that is read.
//!
//! - Publishers take an exclusive [`PublishLock`] (`flock` / `LockFileEx`, freed
//!   by the kernel when its holder dies), copy into a folder no reader has ever
//!   been given, and switch the pointer with an atomic file rename.
//! - Readers take a shared lock on the version's lease file and hold it for the
//!   life of the process, since the hooks they loaded keep pointing into it.
//! - A folder no longer named by the pointer is removed only when its lease can
//!   be taken exclusively; one that cannot be removed stays in `retired.json`
//!   and every later sync tries again.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::install::{copy_dir_all, write_atomic};
use super::util::unix_time_ms;
use super::PluginError;

/// Hidden, manifest-less directory: `discover_plugin_dirs` never takes it for a
/// plugin.
pub(crate) const VERSIONS_DIR_NAME: &str = ".versions";
const POINTER_FILE_NAME: &str = "current";
const LOCK_FILE_NAME: &str = ".lock";
const RETIRED_FILE_NAME: &str = "retired.json";
const LEASE_EXTENSION: &str = "lease";
const LEGACY_LEASE_STEM: &str = "legacy";

/// How long a sync that has something to publish waits for another process's
/// publish. A publish copies a handful of small files, so ten seconds only
/// runs out when a peer is stuck; the sync then reads what is current.
pub(crate) const PUBLISH_LOCK_WAIT: Duration = Duration::from_secs(10);
const PUBLISH_LOCK_POLL: Duration = Duration::from_millis(10);

/// A retired legacy tree is removed no sooner than this after it was retired.
/// zo binaries from before t-76269 read `<root>/<plugin dir>/` without taking a
/// lease, so nothing tells when the last of them stopped using it; a day
/// outlives an ordinary session, and keeping a few kilobytes of plugin files a
/// day longer costs nothing. Version folders need no such wait: every reader
/// of one holds its lease.
pub(crate) const LEGACY_TREE_HOLD: Duration = Duration::from_secs(24 * 60 * 60);

/// Rounds a reader tries to pin the tree the pointer names while publishers
/// keep switching it; each round is one pointer read and one lease attempt.
const LEASE_ATTEMPTS: usize = 64;
const LEASE_RETRY_PAUSE: Duration = Duration::from_millis(1);

static VERSION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Leases this process holds, one per tree it has loaded. They are held until
/// the process exits: the hooks and tools a load resolved keep absolute paths
/// into the tree, and nothing tells when the last copy of them is dropped.
static HELD_LEASES: Mutex<BTreeMap<PathBuf, File>> = Mutex::new(BTreeMap::new());

fn versions_root(install_root: &Path) -> PathBuf {
    install_root.join(VERSIONS_DIR_NAME)
}

fn plugin_versions_dir(logical: &Path) -> Option<PathBuf> {
    let parent = logical.parent()?;
    let name = logical.file_name()?;
    Some(parent.join(VERSIONS_DIR_NAME).join(name))
}

fn lease_path(versions_dir: &Path, stem: &str) -> PathBuf {
    versions_dir.join(format!("{stem}.{LEASE_EXTENSION}"))
}

/// One plain, visible path component — what every version folder name is.
fn is_version_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && matches!(
            Path::new(name).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
}

fn read_pointer(versions_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(versions_dir.join(POINTER_FILE_NAME)).ok()?;
    let name = text.trim();
    is_version_name(name).then(|| name.to_string())
}

/// Where the tree for one logical path is, and the lease that pins it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Located {
    tree: PathBuf,
    lease: Option<PathBuf>,
}

fn locate(logical: &Path, lease_legacy: bool) -> Located {
    let Some(versions_dir) = plugin_versions_dir(logical) else {
        return Located {
            tree: logical.to_path_buf(),
            lease: None,
        };
    };
    match read_pointer(&versions_dir) {
        Some(name) => Located {
            tree: versions_dir.join(&name),
            lease: Some(lease_path(&versions_dir, &name)),
        },
        None => Located {
            tree: logical.to_path_buf(),
            lease: lease_legacy.then(|| lease_path(&versions_dir, LEGACY_LEASE_STEM)),
        },
    }
}

/// The tree a logical install path reads from right now: the version folder
/// its pointer names, or the logical path itself for an install made before
/// pointers existed. For checks that do not hand the tree to anyone.
pub(crate) fn installed_tree(logical: &Path) -> PathBuf {
    locate(logical, false).tree
}

/// [`installed_tree`] for a reader that is about to load the tree: the tree is
/// pinned with a lease this process keeps, so no publish removes it while the
/// hooks loaded from it may still run.
///
/// `lease_legacy` pins a legacy tree too; it is set for bundled plugins, the
/// only ones whose legacy tree a publish retires.
///
/// The pointer is read again after the lease is taken: a publish may have
/// switched it in between, and a retired tree may already be gone. A tree that
/// cannot be leased (a read-only root) is still returned, unpinned.
pub(crate) fn hold_installed_tree(logical: &Path, lease_legacy: bool) -> PathBuf {
    let mut located = locate(logical, lease_legacy);
    for _ in 0..LEASE_ATTEMPTS {
        let Some(lease) = located.lease.clone() else {
            break;
        };
        if !located.tree.is_dir() {
            break;
        }
        match take_shared_lease(&lease) {
            Ok(SharedLease::AlreadyHeld) => return located.tree,
            Ok(SharedLease::Taken(file)) => {
                let again = locate(logical, lease_legacy);
                if again == located && located.tree.is_dir() {
                    held_leases().insert(lease, file);
                    return located.tree;
                }
                // Switched under us: let the stale tree go and pin the new one.
                drop(file);
                located = again;
            }
            Ok(SharedLease::Busy) => {
                // A sweep holds this lease exclusively and is removing the tree.
                thread::sleep(LEASE_RETRY_PAUSE);
                located = locate(logical, lease_legacy);
            }
            Err(_) => break,
        }
    }
    located.tree
}

fn held_leases() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, File>> {
    HELD_LEASES.lock().unwrap_or_else(PoisonError::into_inner)
}

enum SharedLease {
    AlreadyHeld,
    Taken(File),
    Busy,
}

fn open_lease(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn take_shared_lease(path: &Path) -> io::Result<SharedLease> {
    if held_leases().contains_key(path) {
        return Ok(SharedLease::AlreadyHeld);
    }
    let file = open_lease(path)?;
    match file.try_lock_shared() {
        Ok(()) => Ok(SharedLease::Taken(file)),
        Err(TryLockError::WouldBlock) => Ok(SharedLease::Busy),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// The exclusive, cross-process publish lock of one install root. Every
/// publish, pointer switch, `retired.json` write and removal happens while it
/// is held. The lock is on the open file, so two handles in one process
/// exclude each other just as two processes do.
#[derive(Debug)]
pub(crate) struct PublishLock {
    _held: File,
}

impl PublishLock {
    /// Take the lock, waiting up to `wait`. `Ok(None)` when a peer still
    /// held it at the deadline.
    pub(crate) fn acquire(install_root: &Path, wait: Duration) -> Result<Option<Self>, PluginError> {
        let path = lock_path(install_root);
        let file = open_lease(&path)?;
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(Self { _held: file })),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(PUBLISH_LOCK_POLL);
                }
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(error)) => return Err(PluginError::Io(error)),
            }
        }
    }
}

pub(crate) fn lock_path(install_root: &Path) -> PathBuf {
    versions_root(install_root).join(LOCK_FILE_NAME)
}

/// Copy `source_root` into a fresh version folder for `logical` and point the
/// logical path at it. Nothing a reader was handed is renamed or written: the
/// folder is new, and the pointer is replaced by renaming a complete file over
/// it. The tree the pointer named before stays until [`sweep_retired`] finds
/// it unleased.
///
/// A copy cut short leaves a folder no pointer names, which the next sweep
/// collects.
pub(crate) fn publish_version(
    _lock: &PublishLock,
    source_root: &Path,
    logical: &Path,
) -> Result<PathBuf, PluginError> {
    let versions_dir = plugin_versions_dir(logical).ok_or_else(|| {
        PluginError::InvalidManifest(format!(
            "plugin install path `{}` has no parent to publish beside",
            logical.display()
        ))
    })?;
    fs::create_dir_all(&versions_dir)?;
    let name = fresh_version_name();
    let tree = versions_dir.join(&name);
    if let Err(error) = copy_dir_all(source_root, &tree) {
        let _ = fs::remove_dir_all(&tree);
        return Err(error);
    }
    write_atomic(&versions_dir.join(POINTER_FILE_NAME), name.as_bytes())?;
    Ok(tree)
}

/// Publish time, process and a per-process counter: a name nobody has read.
fn fresh_version_name() -> String {
    let sequence = VERSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}.{}.{sequence}", unix_time_ms(), std::process::id())
}

/// Stop serving `logical` (its bundled plugin left the bundle): drop the
/// pointer so every version folder becomes collectable, and list the legacy
/// tree for removal at once. The removal itself is [`sweep_retired`]'s.
pub(crate) fn retire_removed_plugin(_lock: &PublishLock, install_root: &Path, logical: &Path) {
    if let Some(versions_dir) = plugin_versions_dir(logical) {
        if let Err(error) = fs::remove_file(versions_dir.join(POINTER_FILE_NAME)) {
            if error.kind() != io::ErrorKind::NotFound {
                eprintln!(
                    "[zo] plugins: could not unpublish {}: {error}",
                    logical.display()
                );
            }
        }
    }
    if logical.is_dir() {
        if let Some(entry) = legacy_entry(install_root, logical, 0) {
            let mut list = read_retired(install_root).unwrap_or_default();
            list.insert(entry);
            if let Err(error) = store_retired(install_root, &list) {
                eprintln!(
                    "[zo] plugins: could not record {} for removal: {error}",
                    logical.display()
                );
            }
        }
    }
}

/// One tree waiting for removal. Paths are relative to the install root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetiredTree {
    tree: PathBuf,
    lease: PathBuf,
    /// Never removed before this time (unix ms); 0 for version folders.
    not_before_ms: u128,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RetiredList {
    #[serde(default)]
    trees: Vec<RetiredTree>,
}

impl RetiredList {
    /// Add an entry unless its tree is already listed; a listed tree keeps
    /// its earlier time, so a rescan never pushes a removal back.
    fn insert(&mut self, entry: RetiredTree) -> bool {
        if self.trees.iter().any(|listed| listed.tree == entry.tree) {
            return false;
        }
        self.trees.push(entry);
        true
    }
}

fn retired_path(install_root: &Path) -> PathBuf {
    versions_root(install_root).join(RETIRED_FILE_NAME)
}

/// Every path in the list must stay inside the install root.
fn is_contained(relative: &Path) -> bool {
    relative.components().next().is_some()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// `None` when the list is missing or unreadable; the caller then rebuilds it
/// from the folders on disk.
fn read_retired(install_root: &Path) -> Option<RetiredList> {
    let text = fs::read_to_string(retired_path(install_root)).ok()?;
    let mut list: RetiredList = serde_json::from_str(&text).ok()?;
    list.trees
        .retain(|entry| is_contained(&entry.tree) && is_contained(&entry.lease));
    Some(list)
}

fn store_retired(install_root: &Path, list: &RetiredList) -> Result<(), PluginError> {
    write_atomic(
        &retired_path(install_root),
        serde_json::to_string_pretty(list)?.as_bytes(),
    )
}

/// The trees listed for removal, as absolute paths. A tree listed here and not
/// named by any pointer is never loaded (a plugin that left the bundle while
/// some process still held it).
pub(crate) fn retired_trees(install_root: &Path) -> BTreeSet<PathBuf> {
    read_retired(install_root)
        .unwrap_or_default()
        .trees
        .into_iter()
        .map(|entry| install_root.join(entry.tree))
        .collect()
}

fn relative_to(install_root: &Path, path: &Path) -> Option<PathBuf> {
    path.strip_prefix(install_root)
        .ok()
        .filter(|relative| is_contained(relative))
        .map(Path::to_path_buf)
}

fn legacy_entry(install_root: &Path, logical: &Path, not_before_ms: u128) -> Option<RetiredTree> {
    let versions_dir = plugin_versions_dir(logical)?;
    Some(RetiredTree {
        tree: relative_to(install_root, logical)?,
        lease: relative_to(install_root, &lease_path(&versions_dir, LEGACY_LEASE_STEM))?,
        not_before_ms,
    })
}

/// Every tree on disk that no pointer names: version folders beside a
/// pointer (or under a plugin that has none), lease files whose folder is
/// gone, and a legacy tree whose plugin now reads through a pointer. This is
/// what makes a lost or damaged `retired.json` safe — the disk is rescanned
/// and nothing is forgotten; a legacy tree only starts its hold again.
fn scan_retirable(install_root: &Path, now_ms: u128) -> Vec<RetiredTree> {
    let mut found = Vec::new();
    let Ok(plugins) = fs::read_dir(versions_root(install_root)) else {
        return found;
    };
    for plugin in plugins.flatten() {
        let versions_dir = plugin.path();
        if !versions_dir.is_dir() {
            continue;
        }
        let current = read_pointer(&versions_dir);
        let Ok(children) = fs::read_dir(&versions_dir) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let stem = if path.is_dir() {
                Some(name.to_string())
            } else {
                name.strip_suffix(&format!(".{LEASE_EXTENSION}"))
                    .filter(|stem| *stem != LEGACY_LEASE_STEM && !versions_dir.join(stem).exists())
                    .map(str::to_string)
            };
            let Some(stem) = stem.filter(|stem| is_version_name(stem)) else {
                continue;
            };
            if current.as_deref() == Some(stem.as_str()) {
                continue;
            }
            if let (Some(tree), Some(lease)) = (
                relative_to(install_root, &versions_dir.join(&stem)),
                relative_to(install_root, &lease_path(&versions_dir, &stem)),
            ) {
                found.push(RetiredTree {
                    tree,
                    lease,
                    not_before_ms: 0,
                });
            }
        }
        let legacy = install_root.join(plugin.file_name());
        if current.is_some() && legacy.is_dir() {
            let hold = LEGACY_TREE_HOLD.as_millis();
            found.extend(legacy_entry(install_root, &legacy, now_ms + hold));
        }
    }
    found
}

/// Whether a sync has removals to try: something listed, something on disk no
/// pointer names, or a list that is missing or unreadable.
pub(crate) fn retired_work_pending(install_root: &Path) -> bool {
    if !versions_root(install_root).is_dir() {
        return false;
    }
    match read_retired(install_root) {
        Some(list) => !list.trees.is_empty() || !scan_retirable(install_root, 0).is_empty(),
        None => true,
    }
}

/// Remove every retired tree no reader holds. A tree that is leased, still
/// inside its hold, or that the filesystem refuses (Windows: a file inside is
/// open) stays listed for the next sync — across restarts, since the list is
/// on disk. A tree the pointer names is never removed.
pub(crate) fn sweep_retired(lock: &PublishLock, install_root: &Path) {
    sweep_retired_at(lock, install_root, unix_time_ms());
}

pub(crate) fn sweep_retired_at(_lock: &PublishLock, install_root: &Path, now_ms: u128) {
    let stored = read_retired(install_root);
    let mut list = stored.clone().unwrap_or_default();
    for entry in scan_retirable(install_root, now_ms) {
        list.insert(entry);
    }

    let mut kept = RetiredList::default();
    for entry in list.trees {
        let tree = install_root.join(&entry.tree);
        if is_current_version(&tree) {
            continue;
        }
        if now_ms < entry.not_before_ms {
            kept.trees.push(entry);
            continue;
        }
        match remove_unleased(&tree, &install_root.join(&entry.lease)) {
            Ok(true) => {}
            Ok(false) => kept.trees.push(entry),
            Err(error) => {
                eprintln!(
                    "[zo] plugins: kept {} for a later sync, it could not be removed: {error}",
                    tree.display()
                );
                kept.trees.push(entry);
            }
        }
    }

    if stored.as_ref() != Some(&kept) {
        if let Err(error) = store_retired(install_root, &kept) {
            eprintln!("[zo] plugins: could not record trees waiting for removal: {error}");
        }
    }
}

/// Whether `tree` is the version folder its plugin's pointer names.
fn is_current_version(tree: &Path) -> bool {
    let (Some(versions_dir), Some(name)) = (tree.parent(), tree.file_name()) else {
        return false;
    };
    versions_dir
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|parent| parent == VERSIONS_DIR_NAME)
        && read_pointer(versions_dir).as_deref() == name.to_str()
}

/// Remove `tree` if no reader holds `lease`, then the lease file with it.
/// `Ok(false)` while a reader holds it.
fn remove_unleased(tree: &Path, lease: &Path) -> io::Result<bool> {
    let file = open_lease(lease)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(false),
        Err(TryLockError::Error(error)) => return Err(error),
    }
    if let Err(error) = fs::remove_dir_all(tree) {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    // Removed while still locked, so a reader that opens the path afterwards
    // gets a new file and finds, on its second pointer read, that the tree
    // is gone.
    let removed = fs::remove_file(lease);
    drop(file);
    if let Err(error) = removed {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    if let Some(versions_dir) = lease.parent() {
        // Succeeds only once a plugin that left the bundle has nothing left
        // there; a live plugin's folder always holds its pointer.
        let _ = fs::remove_dir(versions_dir);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let sequence = VERSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "plugins-published-{label}-{}-{}-{sequence}",
            unix_time_ms(),
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("scratch root should be creatable");
        root
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent should be creatable");
        }
        fs::write(path, contents).expect("file should be writable");
    }

    fn write_source(root: &Path, label: &str) {
        write_file(
            &root.join("plugin.json"),
            &format!("{{\"name\":\"demo\",\"version\":\"{label}\",\"description\":\"d\"}}"),
        );
        write_file(&root.join("hooks").join("pre.sh"), label);
    }

    fn lock(install_root: &Path) -> PublishLock {
        PublishLock::acquire(install_root, PUBLISH_LOCK_WAIT)
            .expect("lock file should open")
            .expect("nobody else holds the lock")
    }

    fn publish(install_root: &Path, source: &Path, logical: &Path) -> PathBuf {
        let held = lock(install_root);
        publish_version(&held, source, logical).expect("publish should work")
    }

    /// A reader in another process: its own handle, a shared lock.
    fn foreign_reader(lease: &Path) -> File {
        let file = open_lease(lease).expect("lease should open");
        file.lock_shared().expect("shared lease should be taken");
        file
    }

    fn lease_of(tree: &Path) -> PathBuf {
        let name = tree.file_name().and_then(|name| name.to_str()).expect("version name");
        lease_path(tree.parent().expect("versions dir"), name)
    }

    /// How long a test waits for a lock or lease it has just let go. A child
    /// that another test spawns at that moment starts with a copy of every
    /// open file and closes the copies only when it execs, and `flock` frees
    /// a lock only when its last copy is closed: a lock let go is free a
    /// moment later, not always on the very next try.
    const LET_GO_WAIT: Duration = PUBLISH_LOCK_WAIT;

    /// Sweep until `tree` is gone or [`LET_GO_WAIT`] has passed, for a tree
    /// whose last reader has just let go.
    fn sweep_until_gone(install_root: &Path, now_ms: u128, tree: &Path) {
        let deadline = Instant::now() + LET_GO_WAIT;
        loop {
            sweep_retired_at(&lock(install_root), install_root, now_ms);
            if !tree.exists() || Instant::now() >= deadline {
                return;
            }
            thread::sleep(PUBLISH_LOCK_POLL);
        }
    }

    #[test]
    fn a_logical_path_without_a_pointer_reads_its_own_tree() {
        let install_root = scratch("legacy-read");
        let logical = install_root.join("demo-bundled");
        write_source(&logical, "0.1.0");

        assert_eq!(installed_tree(&logical), logical);
        assert_eq!(hold_installed_tree(&logical, false), logical);
        assert!(
            !versions_root(&install_root).exists(),
            "reading an external tree must not create anything beside it",
        );

        let _ = fs::remove_dir_all(install_root);
    }

    #[test]
    fn a_publish_points_the_logical_path_at_a_fresh_complete_tree() {
        let install_root = scratch("publish-fresh");
        let source = scratch("publish-fresh-source");
        write_source(&source, "1.0.0");
        let logical = install_root.join("demo-bundled");

        let first = publish(&install_root, &source, &logical);
        assert_eq!(installed_tree(&logical), first);
        assert!(first.join("hooks").join("pre.sh").exists());
        let second = publish(&install_root, &source, &logical);
        assert_ne!(first, second, "every publish lands in a folder of its own");
        assert_eq!(installed_tree(&logical), second);
        assert!(!logical.exists(), "the logical path is never created as a tree");

        let _ = fs::remove_dir_all(install_root);
        let _ = fs::remove_dir_all(source);
    }

    /// Two handles of the lock file exclude each other even inside one
    /// process (flock binds the open file, `LockFileEx` the handle), so this
    /// is the cross-process case: publishers taking turns never overlap, and
    /// the tree the pointer ends on is one whole source, not a mix.
    #[test]
    fn two_publishers_take_turns_and_never_mix_a_tree() {
        const ROUNDS: usize = 12;
        let install_root = scratch("publish-turns");
        let logical = install_root.join("demo-bundled");
        let inside = Arc::new(AtomicBool::new(false));
        let overlaps = Arc::new(AtomicU64::new(0));

        let publishers: Vec<_> = ["left", "right"]
            .into_iter()
            .map(|label| {
                let source = scratch(&format!("publish-turns-{label}"));
                write_source(&source, label);
                for index in 0..40 {
                    write_file(&source.join("files").join(format!("{index}")), label);
                }
                let install_root = install_root.clone();
                let logical = logical.clone();
                let inside = Arc::clone(&inside);
                let overlaps = Arc::clone(&overlaps);
                thread::spawn(move || {
                    for _ in 0..ROUNDS {
                        let held = lock(&install_root);
                        if inside.swap(true, Ordering::SeqCst) {
                            overlaps.fetch_add(1, Ordering::SeqCst);
                        }
                        publish_version(&held, &source, &logical).expect("publish should work");
                        sweep_retired(&held, &install_root);
                        inside.store(false, Ordering::SeqCst);
                    }
                    source
                })
            })
            .collect();
        let sources: Vec<PathBuf> = publishers
            .into_iter()
            .map(|publisher| publisher.join().expect("publisher should not panic"))
            .collect();

        assert_eq!(
            overlaps.load(Ordering::SeqCst),
            0,
            "two publishers were inside the publish lock at once",
        );
        let tree = installed_tree(&logical);
        let owner = fs::read_to_string(tree.join("hooks").join("pre.sh")).expect("hook");
        let files: Vec<String> = fs::read_dir(tree.join("files"))
            .expect("files dir")
            .flatten()
            .map(|entry| fs::read_to_string(entry.path()).expect("file"))
            .collect();
        assert_eq!(files.len(), 40, "the live tree is complete");
        assert!(
            files.iter().all(|file| *file == owner),
            "the live tree mixes two publishes",
        );
        let versions = fs::read_dir(tree.parent().expect("versions dir"))
            .expect("versions dir")
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .count();
        assert_eq!(versions, 1, "unleased earlier versions are swept");

        let _ = fs::remove_dir_all(install_root);
        for source in sources {
            let _ = fs::remove_dir_all(source);
        }
    }

    #[test]
    fn a_busy_lock_is_waited_for_and_then_given_up() {
        let install_root = scratch("lock-busy");
        let held = lock(&install_root);
        let started = Instant::now();
        let second = PublishLock::acquire(&install_root, Duration::from_millis(60))
            .expect("lock file should open");
        assert!(second.is_none(), "a held lock must not be taken twice");
        assert!(started.elapsed() >= Duration::from_millis(60), "the wait is honoured");
        drop(held);
        assert!(
            PublishLock::acquire(&install_root, LET_GO_WAIT)
                .expect("lock file should open")
                .is_some(),
            "a released lock is free again",
        );

        let _ = fs::remove_dir_all(install_root);
    }

    /// The previous version stays while any reader holds it — whatever time
    /// it is — and goes, with its lease file, once the reader lets go. The
    /// current version is never removed.
    #[test]
    fn a_held_version_and_the_current_one_outlive_any_sweep() {
        let install_root = scratch("held-version");
        let source = scratch("held-version-source");
        write_source(&source, "1.0.0");
        let logical = install_root.join("demo-bundled");

        let first = publish(&install_root, &source, &logical);
        let reader = foreign_reader(&lease_of(&first));
        let second = publish(&install_root, &source, &logical);
        let far_future = unix_time_ms() + 10 * LEGACY_TREE_HOLD.as_millis();
        sweep_retired_at(&lock(&install_root), &install_root, far_future);
        assert!(first.join("hooks").join("pre.sh").exists(), "a held version must stay");
        assert!(second.join("hooks").join("pre.sh").exists(), "the current version must stay");
        assert!(
            retired_trees(&install_root).contains(&first),
            "the held version waits on the list",
        );

        drop(reader);
        sweep_until_gone(&install_root, far_future, &first);
        assert!(!first.exists(), "a version nobody holds is removed");
        assert!(!lease_of(&first).exists(), "its lease file goes with it");
        assert!(second.exists(), "the current version is never removed");
        assert!(retired_trees(&install_root).is_empty());

        let _ = fs::remove_dir_all(install_root);
        let _ = fs::remove_dir_all(source);
    }

    /// A reader pins what it was handed: a publish that switches the pointer
    /// after the reader took its lease leaves that tree in place.
    #[test]
    fn a_tree_held_by_this_process_survives_later_publishes() {
        let install_root = scratch("held-here");
        let source = scratch("held-here-source");
        write_source(&source, "1.0.0");
        let logical = install_root.join("demo-bundled");

        let first = publish(&install_root, &source, &logical);
        assert_eq!(hold_installed_tree(&logical, true), first);
        publish(&install_root, &source, &logical);
        sweep_retired(&lock(&install_root), &install_root);
        assert!(first.join("hooks").join("pre.sh").exists());

        let _ = fs::remove_dir_all(install_root);
        let _ = fs::remove_dir_all(source);
    }

    /// The legacy tree is not renamed, is no longer read once a pointer
    /// exists, and is removed only after its hold even when nobody leases it.
    #[test]
    fn a_legacy_tree_is_kept_for_its_hold_and_then_removed() {
        let install_root = scratch("legacy-hold");
        let source = scratch("legacy-hold-source");
        write_source(&source, "2.0.0");
        let logical = install_root.join("demo-bundled");
        write_source(&logical, "1.0.0");

        let now = unix_time_ms();
        let tree = publish(&install_root, &source, &logical);
        assert_eq!(installed_tree(&logical), tree, "the pointer wins over the legacy tree");
        sweep_retired_at(&lock(&install_root), &install_root, now);
        assert!(
            logical.join("hooks").join("pre.sh").exists(),
            "binaries from before pointers may still read the legacy tree",
        );
        assert!(retired_trees(&install_root).contains(&logical));

        let after_hold = now + LEGACY_TREE_HOLD.as_millis() + 1_000;
        sweep_retired_at(&lock(&install_root), &install_root, after_hold);
        assert!(!logical.exists(), "the legacy tree goes once its hold is over");
        assert!(tree.exists());
        assert!(retired_trees(&install_root).is_empty());

        let _ = fs::remove_dir_all(install_root);
        let _ = fs::remove_dir_all(source);
    }

    /// `retired.json` lost or damaged is read as empty and rebuilt from the
    /// disk: an unpinned old version is still removed, the current one is not,
    /// and a legacy tree only starts its hold again.
    #[test]
    fn a_damaged_or_missing_retired_list_is_rebuilt_from_disk() {
        for damage in [Some("{ not json"), None] {
            let install_root = scratch("retired-rebuild");
            let source = scratch("retired-rebuild-source");
            write_source(&source, "2.0.0");
            let logical = install_root.join("demo-bundled");
            write_source(&logical, "1.0.0");
            let first = publish(&install_root, &source, &logical);
            let second = publish(&install_root, &source, &logical);
            match damage {
                Some(text) => write_file(&retired_path(&install_root), text),
                None => {
                    let _ = fs::remove_file(retired_path(&install_root));
                }
            }
            assert!(retired_work_pending(&install_root), "a lost list is work");

            let now = unix_time_ms();
            sweep_retired_at(&lock(&install_root), &install_root, now);
            assert!(!first.exists(), "the rescan finds the unpinned old version");
            assert!(second.exists(), "the current version is never removed");
            assert!(logical.exists(), "the legacy tree starts its hold again");
            let list = read_retired(&install_root).expect("the list is written back whole");
            assert_eq!(list.trees.len(), 1);
            assert!(list.trees[0].not_before_ms >= now + LEGACY_TREE_HOLD.as_millis());

            let _ = fs::remove_dir_all(install_root);
            let _ = fs::remove_dir_all(source);
        }
    }

    /// A tree the filesystem refuses to remove (on Windows a file inside is
    /// open; here its folder is read-only) stays listed on disk, a later run
    /// that knows nothing of this one finds it there, and removes it once the
    /// filesystem lets it.
    #[cfg(unix)]
    #[test]
    fn a_tree_that_cannot_be_removed_waits_on_disk_for_a_later_run() {
        use std::os::unix::fs::PermissionsExt as _;

        let install_root = scratch("stuck-tree");
        let source = scratch("stuck-tree-source");
        write_source(&source, "1.0.0");
        let logical = install_root.join("demo-bundled");
        let first = publish(&install_root, &source, &logical);
        publish(&install_root, &source, &logical);

        let hooks = first.join("hooks");
        fs::set_permissions(&hooks, fs::Permissions::from_mode(0o555)).expect("chmod");
        sweep_retired(&lock(&install_root), &install_root);
        assert!(hooks.join("pre.sh").exists(), "the refused tree is still there");
        let listed = fs::read_to_string(retired_path(&install_root)).expect("list on disk");
        assert!(
            listed.contains(first.file_name().and_then(|name| name.to_str()).expect("name")),
            "the refused tree is recorded on disk: {listed}",
        );

        fs::set_permissions(&hooks, fs::Permissions::from_mode(0o755)).expect("chmod back");
        assert!(retired_work_pending(&install_root), "a later run sees the work");
        sweep_retired(&lock(&install_root), &install_root);
        assert!(!first.exists(), "a later run removes it");
        assert!(retired_trees(&install_root).is_empty());

        let _ = fs::remove_dir_all(install_root);
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn a_removed_plugin_loses_its_pointer_and_every_tree() {
        let install_root = scratch("removed-plugin");
        let source = scratch("removed-plugin-source");
        write_source(&source, "1.0.0");
        let logical = install_root.join("demo-bundled");
        write_source(&logical, "0.9.0");
        let tree = publish(&install_root, &source, &logical);

        let held = lock(&install_root);
        retire_removed_plugin(&held, &install_root, &logical);
        sweep_retired(&held, &install_root);
        assert!(!tree.exists());
        assert!(!logical.exists(), "a plugin that left the bundle has no hold");
        assert!(!install_root.join(VERSIONS_DIR_NAME).join("demo-bundled").exists());
        assert!(retired_trees(&install_root).is_empty());

        let _ = fs::remove_dir_all(install_root);
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn a_pointer_naming_anything_but_a_plain_folder_is_ignored() {
        for bad in ["", "..", "../escape", ".hidden", "a/b"] {
            assert!(!is_version_name(bad), "{bad:?} must not be a version name");
        }
        assert!(is_version_name("1793773647659.42.0"));
    }
}
