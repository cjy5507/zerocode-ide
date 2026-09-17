//! Capability-style filesystem helpers for private Zo state.
//!
//! Callers choose a trusted root once. All attacker-controlled descendants are
//! then traversed relative to a retained directory handle without following
//! symlinks.

use std::fs;
#[cfg(any(unix, windows))]
use std::ffi::OsString;
use std::io;
#[cfg(unix)]
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
#[cfg(any(unix, windows))]
use std::sync::atomic::AtomicU64;
#[cfg(unix)]
use std::sync::atomic::Ordering;

#[cfg(any(unix, windows))]
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn invalid_path(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn validate_relative(path: &Path, allow_empty: bool) -> io::Result<()> {
    let mut saw_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_component = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(invalid_path("secure state path must stay below its trusted root"));
            }
        }
    }
    if !allow_empty && !saw_component {
        return Err(invalid_path("secure state path must name a file or directory"));
    }
    Ok(())
}

/// Create a private directory below `root` without following descendant
/// symlinks. Existing descendant directories are tightened to owner-only mode
/// on Unix.
pub fn ensure_private_dir(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    validate_relative(relative, true)?;
    ensure_private_dir_impl(root, relative)?;
    Ok(root.join(relative))
}

/// Create the private directory named by the absolute `path` without following
/// a symlink at ANY component — including intermediate ancestors — and without
/// canonicalizing user-controlled input.
///
/// `owned_suffix_len` is the number of trailing components that are Zo-owned
/// and may therefore be created here (for example `2` for the
/// `<slug>/state` suffix under a configured `projects/` base, or `1` for a
/// config home directly under an existing parent). Every ancestor above that
/// suffix must already exist: it is opened `O_DIRECTORY|O_NOFOLLOW` relative to
/// a retained descriptor starting from `/`, so a symlink ancestor is rejected
/// rather than followed, and a *missing* non-Zo ancestor is a hard error rather
/// than being fabricated. This bounds creation to the Zo-owned suffix so an
/// explicit `ZO_CONFIG_HOME=/existing/absent-parent/home` never creates the
/// caller's `absent-parent`.
///
/// Every existing or newly created directory inside the Zo-owned suffix is
/// descriptor-relatively tightened to `0o700` after current-user ownership is
/// verified. Ancestors above the suffix (`/`, `/Users`, and home parents) are
/// opened no-follow but never modified.
///
/// The only alias resolved is the fixed macOS `/var` -> `/private/var` mapping;
/// no other symlink is ever followed. On non-Unix targets there are no
/// no-follow directory handles, so this fails closed and creates nothing.
pub fn ensure_private_dir_absolute(path: &Path, owned_suffix_len: usize) -> io::Result<PathBuf> {
    #[cfg(unix)]
    {
        ensure_private_dir_absolute_impl(path, owned_suffix_len)?;
        Ok(path.to_path_buf())
    }
    #[cfg(windows)]
    {
        windows_impl::ensure_private_dir_absolute(path, owned_suffix_len)?;
        Ok(path.to_path_buf())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, owned_suffix_len);
        Err(unsupported_secure_mutation())
    }
}

/// Whether the absolute `path` currently resolves — component by component,
/// following no symlink at any user-controlled ancestor — to an existing
/// current-user-owned, non-symlink directory of mode exactly `0o700`.
///
/// Used by `doctor` to re-check the postcondition of an absolute repair without
/// a `canonicalize`-then-`stat` race. Returns `Ok(false)` when any component is
/// a symlink, missing, foreign-owned, or the leaf is broader than owner-only.
/// Fails closed on non-Unix.
pub fn is_owned_private_dir_absolute(path: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        is_owned_private_dir_absolute_impl(path)
    }
    #[cfg(windows)]
    {
        windows_impl::is_owned_private_dir_absolute(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(false)
    }
}

/// Whether *every* Zo-owned suffix component of the absolute `path` — the
/// trailing `owned_suffix_len` directories — currently resolves no-follow to a
/// current-user-owned, non-symlink directory of mode exactly `0o700`.
///
/// This is the full postcondition `doctor` verifies after an absolute create:
/// tightening the whole suffix (not just the leaf) is only claimed `FIXED` when
/// every owned suffix directory is confirmed private, so a concurrent broadening
/// of an intermediate suffix directory cannot be reported as fixed. Non-Zo
/// ancestors are opened no-follow but never inspected for mode. Fails closed on
/// non-Unix.
pub fn is_owned_private_suffix_absolute(path: &Path, owned_suffix_len: usize) -> io::Result<bool> {
    #[cfg(unix)]
    {
        is_owned_private_suffix_absolute_impl(path, owned_suffix_len)
    }
    #[cfg(windows)]
    {
        windows_impl::is_owned_private_suffix_absolute(path, owned_suffix_len)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, owned_suffix_len);
        Ok(false)
    }
}

/// No-follow classification of the final component of an absolute Zo-owned
/// directory path, used by `doctor` to decide between create, tighten, and
/// refuse without a `canonicalize`-then-`stat` race.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsoluteDirLeaf {
    /// Nothing exists at the leaf; the missing suffix can be created safely.
    Missing,
    /// An owner-owned, non-symlink directory whose mode is broader than `0o700`.
    OwnedDirTooBroad,
    /// A symlink, a non-directory, a foreign-owned entry, or an ancestor that is
    /// itself a symlink — never created-through, followed, or modified.
    Unsafe,
}

/// Classify the final component of the absolute `path` no-follow, opening every
/// ancestor with `O_NOFOLLOW`. A symlink at any ancestor makes the parent walk
/// fail and classifies as [`AbsoluteDirLeaf::Unsafe`] — the fail-closed default.
/// The leaf itself is `lstat`'d with `SYMLINK_NOFOLLOW`, so a symlink leaf is
/// reported as `Unsafe` rather than dereferenced. Fails closed on non-Unix.
#[must_use]
pub fn owned_dir_leaf_state_absolute(path: &Path) -> AbsoluteDirLeaf {
    #[cfg(unix)]
    {
        owned_dir_leaf_state_absolute_impl(path)
    }
    #[cfg(windows)]
    {
        windows_impl::owned_dir_leaf_state_absolute(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        AbsoluteDirLeaf::Unsafe
    }
}

/// Tighten an existing owner-owned directory named by the absolute `path` to
/// owner-only `0o700`, opening every component (including the leaf) with
/// `O_NOFOLLOW` so no symlink at any user-controlled ancestor is followed and
/// the descriptor chmod'd is exactly the validated leaf. The leaf must already
/// exist, be a non-symlink directory, and be owned by the effective user.
/// Fails closed on non-Unix.
pub fn restrict_existing_owner_only_absolute(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        restrict_existing_owner_only_absolute_impl(path)
    }
    #[cfg(windows)]
    {
        windows_impl::restrict_existing_owner_only_absolute(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(unsupported_secure_mutation())
    }
}

/// No-follow snapshot of the regular file named by the absolute `path`, opening
/// EVERY component — including intermediate ancestors and the leaf itself —
/// with `O_NOFOLLOW` from a genuinely trusted `/` descriptor, never
/// canonicalizing user-controlled input.
///
/// Unlike [`read_to_string_no_symlink`] (which canonicalizes its caller-supplied
/// root through `open_root` and so would follow a symlink planted at an
/// intermediate ancestor), this treats the whole absolute path as
/// attacker-controlled: a symlink at any ancestor or at the leaf is rejected
/// rather than dereferenced, closing the intermediate-ancestor no-follow hole
/// for credential reads.
///
/// Returns `Ok(None)` when the leaf is missing (`ENOENT`). The opened leaf must
/// be a current-user-owned, non-symlink regular file; a foreign-owned or
/// non-regular leaf is an error, never silently accepted. Fails closed on
/// non-Unix.
pub fn read_regular_file_absolute_no_follow(path: &Path) -> io::Result<Option<String>> {
    #[cfg(unix)]
    {
        read_regular_file_absolute_no_follow_impl(path)
    }
    #[cfg(windows)]
    {
        windows_impl::read_regular_file_absolute_no_follow(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(unsupported_secure_mutation())
    }
}

/// Whether the regular file named by the absolute `path` currently exists as a
/// safe, current-user-owned, non-symlink regular file, established by opening
/// every component (including the leaf) `O_NOFOLLOW`. `Ok(true)` means a safe
/// regular file is present; `Ok(false)` means it is absent. A symlink at any
/// component, a non-regular leaf, or a foreign-owned leaf is an `Err` so callers
/// can distinguish "safely absent" from "present but unsafe". Fails closed on
/// non-Unix.
pub fn is_safe_regular_file_absolute_no_follow(path: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        is_safe_regular_file_absolute_no_follow_impl(path)
    }
    #[cfg(windows)]
    {
        windows_impl::is_safe_regular_file_absolute_no_follow(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(unsupported_secure_mutation())
    }
}

/// Whether the absolute `path` is an owner-owned, singly linked, non-symlink
/// regular file with no group/other permission bits. Every component is opened
/// descriptor-relatively with `O_NOFOLLOW`; `Ok(false)` also covers a safely
/// missing leaf. Fails closed on non-Unix.
pub fn is_owned_private_regular_file_absolute(path: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        is_owned_private_regular_file_absolute_impl(path)
    }
    #[cfg(windows)]
    {
        windows_impl::is_owned_private_regular_file_absolute(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(false)
    }
}

/// Tighten an existing owner-owned, singly linked regular file at the absolute
/// `path` to `0o600`. Every component, including the leaf, is opened with
/// `O_NOFOLLOW`; chmod is applied to the retained validated descriptor, never a
/// path lookup. Fails closed on non-Unix.
pub fn restrict_existing_owner_only_regular_file_absolute(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        restrict_existing_owner_only_regular_file_absolute_impl(path)
    }
    #[cfg(windows)]
    {
        windows_impl::restrict_existing_owner_only_regular_file_absolute(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(unsupported_secure_mutation())
    }
}

/// How far a secure atomic replace is pushed before it returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    /// The bytes and the parent directory are fsynced (`F_FULLFSYNC` on
    /// macOS, ~4 ms each), so the write survives a power loss.
    Flushed,
    /// The write stops at the rename: the new inode is visible to every
    /// process at once, and only a power loss inside the OS flush window can
    /// lose it. For state that is rebuilt on restart anyway.
    Published,
}

/// Atomically replace a private state file below `root` using owner-only
/// permissions and retained no-follow directory handles, flushed to disk.
pub fn write_atomic_owner_only(root: &Path, relative: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic_owner_only_with(root, relative, contents, Durability::Flushed)
}

/// [`write_atomic_owner_only`] with the caller's choice of [`Durability`].
pub fn write_atomic_owner_only_with(
    root: &Path,
    relative: &Path,
    contents: &[u8],
    durability: Durability,
) -> io::Result<()> {
    write_atomic_owner_only_observed(root, relative, contents, durability, &mut |_| {})
}

/// Seam for tests: `observe` receives, in order, `temp_written`, `file_synced`
/// (flushed only), `renamed`, and `parent_dir_synced` (flushed only) as each
/// step of the replace completes.
pub(crate) fn write_atomic_owner_only_observed(
    root: &Path,
    relative: &Path,
    contents: &[u8],
    durability: Durability,
    observe: &mut dyn FnMut(&'static str),
) -> io::Result<()> {
    validate_relative(relative, false)?;
    write_atomic_owner_only_impl(root, relative, contents, durability, observe)
}

#[cfg(unix)]
pub(crate) fn write_atomic_owner_only_retained(
    dir: &RetainedDir,
    relative: &Path,
    contents: &[u8],
) -> io::Result<()> {
    dir.write_atomic_owner_only_retained(relative, contents)
}

/// Append bytes to an owner-only regular state file below `root` without
/// following descendant symlinks.
pub fn append_owner_only(root: &Path, relative: &Path, contents: &[u8]) -> io::Result<()> {
    validate_relative(relative, false)?;
    append_owner_only_impl(root, relative, contents)
}

/// Read a regular state file below `root` without following descendant
/// symlinks.
pub fn read_to_string_no_symlink(root: &Path, relative: &Path) -> io::Result<String> {
    read_to_string_no_symlink_with_validation(root, relative, || Ok(()))
}

/// Open one no-follow descriptor, stabilize its byte snapshot before an
/// external provenance check, then revalidate pathname identity before
/// returning the retained pre-check snapshot.
pub(crate) fn read_to_string_no_symlink_with_validation<F>(
    root: &Path,
    relative: &Path,
    validate: F,
) -> io::Result<String>
where
    F: FnOnce() -> io::Result<()>,
{
    validate_relative(relative, false)?;
    read_to_string_no_symlink_impl(root, relative, validate)
}

/// Remove a regular state file below `root` without following descendant
/// symlinks. A missing file is treated as success.
pub fn remove_file_no_symlink(root: &Path, relative: &Path) -> io::Result<()> {
    validate_relative(relative, false)?;
    remove_file_no_symlink_impl(root, relative)
}

/// Flush the parent directory containing `relative` below `root`. Callers that
/// coordinate multiple durable files can use this as an explicit phase barrier
/// after a rename or unlink.
pub fn sync_parent_directory(root: &Path, relative: &Path) -> io::Result<()> {
    validate_relative(relative, false)?;
    sync_parent_directory_impl(root, relative)
}

/// A directory capability retained across related filesystem operations.
#[cfg(unix)]
pub(crate) struct RetainedDir {
    fd: std::os::fd::OwnedFd,
}

/// A no-follow regular file retained so a later rename can verify identity.
#[cfg(unix)]
pub(crate) struct RetainedRegularFile {
    file: fs::File,
    device: i128,
    inode: i128,
}

#[cfg(unix)]
impl RetainedDir {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        open_root(path).map(|fd| Self { fd })
    }

    pub(crate) fn entry_names(&self) -> io::Result<Vec<OsString>> {
        use std::os::unix::ffi::OsStringExt as _;

        let entries = rustix::fs::Dir::read_from(&self.fd).map_err(io::Error::from)?;
        entries
            .map(|entry| {
                entry
                    .map(|entry| OsString::from_vec(entry.file_name().to_bytes().to_vec()))
                    .map_err(io::Error::from)
            })
            .collect()
    }

    pub(crate) fn ensure_private_subdir(&self, relative: &Path) -> io::Result<Self> {
        use rustix::fs::{Mode, fchmod, mkdirat};

        validate_relative(relative, false)?;
        let mode = Mode::from_raw_mode(0o700);
        let mut dir = rustix::io::dup(&self.fd).map_err(io::Error::from)?;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                continue;
            };
            match mkdirat(&dir, name, mode) {
                Ok(()) => {}
                Err(error) if error == rustix::io::Errno::EXIST => {}
                Err(error) => return Err(io::Error::from(error)),
            }
            let child = open_child_dir(&dir, name)?;
            fchmod(&child, mode).map_err(io::Error::from)?;
            dir = child;
        }
        Ok(Self { fd: dir })
    }

    /// Atomically create one new private child directory. Existing entries are
    /// rejected rather than opened, so callers can retain ownership of a run
    /// directory they later clean up.
    pub(crate) fn create_private_subdir_new(&self, name: &Path) -> io::Result<()> {
        use rustix::fs::{AtFlags, Mode, fchmod, mkdirat, unlinkat};

        validate_relative(name, false)?;
        let mut components = name.components();
        let Some(Component::Normal(component)) = components.next() else {
            return Err(invalid_path("secure state directory must name one child"));
        };
        if components.next().is_some() {
            return Err(invalid_path("secure state directory must name one child"));
        }
        let mode = Mode::from_raw_mode(0o700);
        mkdirat(&self.fd, component, mode).map_err(io::Error::from)?;
        let cleanup = |error: io::Error| match unlinkat(&self.fd, component, AtFlags::REMOVEDIR) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(io::Error::other(format!(
                "{error}; secure new-directory cleanup also failed: {cleanup}"
            ))),
        };
        let child = match open_child_dir(&self.fd, component) {
            Ok(child) => child,
            Err(error) => return cleanup(error),
        };
        if let Err(error) = fchmod(&child, mode).map_err(io::Error::from) {
            drop(child);
            return cleanup(error);
        }
        Ok(())
    }

    pub(crate) fn open_regular_file(
        &self,
        relative: &Path,
    ) -> io::Result<RetainedRegularFile> {
        use rustix::fs::{Mode, OFlags, openat};

        validate_relative(relative, false)?;
        let (parent, file_name) = open_parent_and_name_from(&self.fd, relative)?;
        let fd = openat(
            &parent,
            &file_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        let stat = validate_opened_regular_file(&parent, &file_name, &fd, None)?;
        Ok(RetainedRegularFile {
            file: fs::File::from(fd),
            device: i128::from(stat.st_dev),
            inode: i128::from(stat.st_ino),
        })
    }

    pub(crate) fn append_owner_only(
        &self,
        relative: &Path,
        contents: &[u8],
    ) -> io::Result<()> {
        use rustix::fs::{Mode, OFlags, fchmod, openat};

        validate_relative(relative, false)?;
        let (parent, file_name) = open_parent_and_name_from(&self.fd, relative)?;
        let fd = openat(
            &parent,
            &file_name,
            OFlags::WRONLY
                | OFlags::APPEND
                | OFlags::CREATE
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io::Error::from)?;
        let owner = current_euid_from_new_file(&parent)?;
        validate_opened_regular_file(&parent, &file_name, &fd, Some(owner))?;
        fchmod(&fd, Mode::from_raw_mode(0o600)).map_err(io::Error::from)?;
        validate_opened_regular_file(&parent, &file_name, &fd, Some(owner))?;
        fs::File::from(fd).write_all(contents)
    }

    /// Append with durability guarantees: the appended data is fsynced before
    /// returning, and the parent directory is synced because the append may
    /// have created (or re-linked) the file entry.
    pub(crate) fn append_owner_only_durable(
        &self,
        relative: &Path,
        contents: &[u8],
    ) -> io::Result<()> {
        self.append_owner_only_durable_observed(relative, contents, &mut |_| {})
    }

    /// Seam for tests: `observe` receives, in order, `append_write`,
    /// `file_synced`, and `parent_dir_synced` as each durability step completes.
    pub(crate) fn append_owner_only_durable_observed(
        &self,
        relative: &Path,
        contents: &[u8],
        observe: &mut dyn FnMut(&'static str),
    ) -> io::Result<()> {
        use rustix::fs::{Mode, OFlags, fchmod, openat};

        validate_relative(relative, false)?;
        let (parent, file_name) = open_parent_and_name_from(&self.fd, relative)?;
        let fd = openat(
            &parent,
            &file_name,
            OFlags::WRONLY
                | OFlags::APPEND
                | OFlags::CREATE
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io::Error::from)?;
        let owner = current_euid_from_new_file(&parent)?;
        validate_opened_regular_file(&parent, &file_name, &fd, Some(owner))?;
        fchmod(&fd, Mode::from_raw_mode(0o600)).map_err(io::Error::from)?;
        validate_opened_regular_file(&parent, &file_name, &fd, Some(owner))?;
        let mut file = fs::File::from(fd);
        file.write_all(contents)?;
        observe("append_write");
        file.sync_all()?;
        observe("file_synced");
        sync_directory(&parent)?;
        observe("parent_dir_synced");
        Ok(())
    }

    pub(crate) fn try_lock_owner_only(
        &self,
        relative: &Path,
    ) -> io::Result<Option<ExclusiveFileLock>> {
        use rustix::fs::{Mode, OFlags, fchmod, openat};

        validate_relative(relative, false)?;
        let (parent, file_name) = open_parent_and_name_from(&self.fd, relative)?;
        let fd = openat(
            &parent,
            &file_name,
            OFlags::RDWR
                | OFlags::CREATE
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io::Error::from)?;
        let owner = current_euid_from_new_file(&parent)?;
        validate_opened_regular_file(&parent, &file_name, &fd, Some(owner))?;
        fchmod(&fd, Mode::from_raw_mode(0o600)).map_err(io::Error::from)?;
        validate_opened_regular_file(&parent, &file_name, &fd, Some(owner))?;
        let file = fs::File::from(fd);
        match file.try_lock() {
            // The flock is only proof for the file the path *currently* names.
            // A reclaimer (the retention sweep) may hold the flock and unlink
            // this very inode between our open and our lock: we would then be
            // the exclusive owner of a file nobody else can reach, while the
            // next writer creates a fresh lock file at the path and "owns" the
            // store too. Re-validating fd↔direntry identity under the lock
            // closes that window; a mismatch is contention, not corruption —
            // report "not acquired" so the caller's retry opens the live file.
            Ok(()) => match validate_opened_regular_file(&parent, &file_name, &file, Some(owner))
            {
                Ok(_) => Ok(Some(ExclusiveFileLock { file })),
                Err(_) => Ok(None),
            },
            Err(error) => {
                let error = io::Error::from(error);
                if error.kind() == io::ErrorKind::WouldBlock {
                    Ok(None)
                } else {
                    Err(error)
                }
            }
        }
    }

    pub(crate) fn remove_regular_file(&self, relative: &Path) -> io::Result<bool> {
        use rustix::fs::{AtFlags, Mode, OFlags, openat, unlinkat};

        validate_relative(relative, false)?;
        let (parent, file_name) = open_parent_and_name_from(&self.fd, relative)?;
        let fd = match openat(
            &parent,
            &file_name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(false),
            Err(error) => return Err(io::Error::from(error)),
        };
        validate_opened_regular_file(&parent, &file_name, &fd, None)?;
        unlinkat(&parent, &file_name, AtFlags::empty()).map_err(io::Error::from)?;
        sync_directory(&parent)?;
        Ok(true)
    }

    pub(crate) fn write_atomic_owner_only_retained(
        &self,
        relative: &Path,
        contents: &[u8],
    ) -> io::Result<()> {
        use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};

        validate_relative(relative, false)?;
        let (parent, file_name) = open_parent_and_name_from(&self.fd, relative)?;
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_name = format!(".zo-state-{}-{sequence}.tmp", std::process::id());
        let fd = openat(
            &parent,
            temp_name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io::Error::from)?;
        rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o600)).map_err(io::Error::from)?;
        let mut file = fs::File::from(fd);
        let result = (|| {
            file.write_all(contents)?;
            file.sync_all()?;
            drop(file);
            renameat(&parent, temp_name.as_str(), &parent, &file_name).map_err(io::Error::from)
        })();
        match result {
            Ok(()) => sync_directory(&parent),
            Err(error) => match unlinkat(&parent, temp_name.as_str(), AtFlags::empty()) {
                Ok(()) => Err(error),
                Err(cleanup) if cleanup == rustix::io::Errno::NOENT => Err(error),
                Err(cleanup) => Err(io::Error::other(format!(
                    "{error}; secure temporary file cleanup also failed: {cleanup}"
                ))),
            },
        }
    }

    pub(crate) fn rename_file_no_replace(
        &self,
        source: &Path,
        retained: &RetainedRegularFile,
        destination: &Self,
        destination_name: &Path,
    ) -> io::Result<bool> {
        use rustix::fs::{AtFlags, FileType, statat};

        validate_relative(source, false)?;
        validate_relative(destination_name, false)?;
        let (source_parent, source_name) = open_parent_and_name_from(&self.fd, source)?;
        let (destination_parent, destination_name) =
            open_parent_and_name_from(&destination.fd, destination_name)?;
        let source_stat = match statat(&source_parent, &source_name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(false),
            Err(error) => return Err(io::Error::from(error)),
        };
        if FileType::from_raw_mode(source_stat.st_mode) != FileType::RegularFile
            || source_stat.st_nlink != 1
            || !retained.matches(&source_stat)
        {
            return Ok(false);
        }
        let source_stat = statat(&source_parent, &source_name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(io::Error::from)?;
        if source_stat.st_nlink != 1 || !retained.matches(&source_stat) {
            return Ok(false);
        }
        match renameat_noreplace(
            &source_parent,
            &source_name,
            &destination_parent,
            &destination_name,
        ) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(rustix::io::Errno::EXIST.raw_os_error()) => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
        let destination_stat = statat(
            &destination_parent,
            &destination_name,
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(io::Error::from)?;
        if !retained.matches(&destination_stat) {
            return Err(io::Error::other(
                "secure state source changed while it was being renamed",
            ));
        }
        retained.make_owner_only()?;
        sync_directory(&source_parent)?;
        sync_directory(&destination_parent)?;
        Ok(true)
    }
}

#[cfg(unix)]
impl RetainedRegularFile {
    pub(crate) fn read_to_string(&mut self) -> io::Result<String> {
        let mut contents = String::new();
        self.file.read_to_string(&mut contents)?;
        Ok(contents)
    }

    pub(crate) fn modified(&self) -> io::Result<std::time::SystemTime> {
        self.file.metadata()?.modified()
    }

    fn matches(&self, stat: &rustix::fs::Stat) -> bool {
        self.device == i128::from(stat.st_dev) && self.inode == i128::from(stat.st_ino)
    }

    fn make_owner_only(&self) -> io::Result<()> {
        use rustix::fs::{Mode, fchmod, fstat};

        let stat = fstat(&self.file).map_err(io::Error::from)?;
        if stat.st_nlink != 1 || !self.matches(&stat) {
            return Err(invalid_path(
                "secure state target must remain a singly-linked regular file",
            ));
        }
        fchmod(&self.file, Mode::from_raw_mode(0o600)).map_err(io::Error::from)?;
        let stat = fstat(&self.file).map_err(io::Error::from)?;
        if stat.st_nlink != 1 || !self.matches(&stat) {
            return Err(invalid_path(
                "secure state target must remain a singly-linked regular file",
            ));
        }
        Ok(())
    }
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn renameat_noreplace(
    old_dir: &std::os::fd::OwnedFd,
    old_path: &std::ffi::OsStr,
    new_dir: &std::os::fd::OwnedFd,
    new_path: &std::ffi::OsStr,
) -> io::Result<()> {
    rustix::fs::renameat_with(
        old_dir,
        old_path,
        new_dir,
        new_path,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(io::Error::from)
}

#[cfg(all(
    unix,
    not(any(target_vendor = "apple", target_os = "linux", target_os = "android"))
))]
fn renameat_noreplace(
    old_dir: &std::os::fd::OwnedFd,
    old_path: &std::ffi::OsStr,
    new_dir: &std::os::fd::OwnedFd,
    new_path: &std::ffi::OsStr,
) -> io::Result<()> {
    use rustix::fs::{AtFlags, linkat, unlinkat};

    linkat(&old_dir, &old_path, &new_dir, &new_path, AtFlags::empty())
        .map_err(io::Error::from)?;
    if let Err(error) = unlinkat(&old_dir, &old_path, AtFlags::empty()) {
        let _ = unlinkat(&new_dir, &new_path, AtFlags::empty());
        return Err(io::Error::from(error));
    }
    Ok(())
}

/// An exclusive owner-only advisory lock. Its lock file is intentionally
/// persistent, avoiding an unlink/recreate race or crash-stale existence lock.
#[derive(Debug)]
pub struct ExclusiveFileLock {
    file: fs::File,
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        // On Linux, `flock` follows the open file description. Explicitly
        // unlock before close so a descriptor briefly duplicated by subprocess
        // creation cannot prolong the lock after this capability is dropped.
        let _ = self.file.unlock();
    }
}

/// Acquire an exclusive owner-only advisory lock below `root`. `Ok(None)`
/// means another holder already owns the lock.
pub fn try_lock_owner_only(
    root: &Path,
    relative: &Path,
) -> io::Result<Option<ExclusiveFileLock>> {
    validate_relative(relative, false)?;
    #[cfg(unix)]
    {
        let root = RetainedDir::open(root)?;
        root.try_lock_owner_only(relative)
    }
    #[cfg(windows)]
    {
        windows_impl::try_lock_owner_only(root, relative)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (root, relative);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "secure advisory locking requires Unix no-follow directory handles",
        ))
    }
}

#[cfg(unix)]
fn open_root(root: &Path) -> io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags, open};

    // Resolve platform-owned aliases once (for example macOS `/var` ->
    // `/private/var`), then reopen every resulting component relative to a
    // retained directory descriptor. A component replaced with a symlink after
    // canonicalization is rejected by `open_child_dir` instead of followed.
    let canonical = fs::canonicalize(root)?;
    let mut dir = open(
        Path::new("/"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    for component in canonical.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => dir = open_child_dir(&dir, name)?,
            Component::ParentDir | Component::Prefix(_) => {
                return Err(invalid_path("trusted state root is not an absolute directory"));
            }
        }
    }
    Ok(dir)
}

#[cfg(unix)]
fn open_child_dir(
    parent: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
) -> io::Result<std::os::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags, openat};

    openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)
}

/// The normalized `Normal` components of an absolute path, with only the fixed
/// macOS `/var` -> `/private/var` system alias applied. No other alias is
/// resolved and no symlink is followed: this is a lexical rewrite of a single
/// bounded platform mapping so tests using `std::env::temp_dir()` (commonly
/// under the `/var` alias) traverse the real `/private/var` tree that the
/// kernel would reach, while every remaining component stays user-controlled
/// and is opened `O_NOFOLLOW` by the walkers below.
#[cfg(unix)]
fn absolute_normal_components(path: &Path) -> io::Result<Vec<OsString>> {
    let mut names: Vec<OsString> = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => names.push(name.to_os_string()),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(invalid_path(
                    "absolute secure path must be a rooted, non-relative directory",
                ));
            }
        }
    }
    if path.is_relative() {
        return Err(invalid_path("secure absolute path must be rooted at `/`"));
    }
    // macOS exposes `/var`, `/tmp`, and `/etc` as fixed system symlinks into
    // `/private`. Rewrite only the exact `/var` prefix (the one temp paths use)
    // to its `/private/var` target so the walk lands on the real directory
    // instead of tripping `O_NOFOLLOW` on the top-level system alias. This is a
    // fixed, bounded mapping — never a general alias follow.
    #[cfg(target_vendor = "apple")]
    if names.first().is_some_and(|first| first == "var") {
        names.splice(0..1, [OsString::from("private"), OsString::from("var")]);
    }
    Ok(names)
}

/// Walk to the parent directory of the final component of an absolute path,
/// opening every intermediate component with `O_NOFOLLOW`. Returns the retained
/// parent descriptor and the final component name. Fails when the path has no
/// final component (is `/` itself).
#[cfg(unix)]
fn open_absolute_parent(path: &Path) -> io::Result<(std::os::fd::OwnedFd, OsString)> {
    use rustix::fs::{Mode, OFlags, open};

    let mut names = absolute_normal_components(path)?;
    let leaf = names
        .pop()
        .ok_or_else(|| invalid_path("secure absolute path has no final component"))?;
    let mut dir = open(
        Path::new("/"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    for name in &names {
        dir = open_child_dir(&dir, name)?;
    }
    Ok((dir, leaf))
}

#[cfg(unix)]
fn ensure_private_dir_absolute_impl(path: &Path, owned_suffix_len: usize) -> io::Result<()> {
    use rustix::fs::{Mode, OFlags, fchmod, fstat, mkdirat, open};

    let mode = Mode::from_raw_mode(0o700);
    let names = absolute_normal_components(path)?;
    if names.is_empty() {
        return Err(invalid_path("refusing to create the filesystem root"));
    }
    if owned_suffix_len == 0 || owned_suffix_len > names.len() {
        return Err(invalid_path(
            "secure absolute create requires a Zo-owned suffix within the target path",
        ));
    }
    // Only the trailing `owned_suffix_len` components are Zo-owned and may be
    // created or tightened; every ancestor above them must already exist.
    // `mkdirat` on an absent ancestor returns `ENOENT`, which surfaces as an
    // error rather than fabricating a non-Zo parent.
    let first_owned = names.len() - owned_suffix_len;
    let mut dir = open(
        Path::new("/"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    for (index, name) in names.iter().enumerate() {
        if index < first_owned {
            // A non-Zo ancestor: open it no-follow, never create or chmod it. A
            // missing ancestor (`ENOENT`) or a symlink ancestor is rejected here.
            dir = open_child_dir(&dir, name)?;
            continue;
        }
        // A Zo-owned suffix component: create it if missing, else open the
        // pre-existing entry. Either way, tighten it to owner-only `0o700` —
        // but only after proving it is a current-user-owned, non-symlink
        // directory (the no-follow open already rejects a symlink). A
        // foreign-owned suffix directory fails closed rather than being chmod'd.
        match mkdirat(&dir, name.as_os_str(), mode) {
            Ok(()) => {}
            Err(error) if error == rustix::io::Errno::EXIST => {}
            Err(error) => return Err(io::Error::from(error)),
        }
        let child = open_child_dir(&dir, name)?;
        let stat = fstat(&child).map_err(io::Error::from)?;
        if !stat_is_owned_kind(&stat, true) {
            return Err(invalid_path(
                "secure state suffix entry is not a current-user-owned directory",
            ));
        }
        fchmod(&child, mode).map_err(io::Error::from)?;
        dir = child;
    }
    Ok(())
}

#[cfg(unix)]
fn is_owned_private_dir_absolute_impl(path: &Path) -> io::Result<bool> {
    use rustix::fs::{Mode, OFlags, fstat, openat};

    // A symlink or missing intermediate ancestor makes the target provably not
    // an owner-private directory, so fail closed to `Ok(false)` rather than
    // surfacing the raw walk error to callers.
    let Ok((parent, leaf)) = open_absolute_parent(path) else {
        return Ok(false);
    };
    let fd = match openat(
        &parent,
        leaf.as_os_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        // Missing, a symlink leaf, or a non-directory: not an owner-private dir.
        Err(error)
            if error == rustix::io::Errno::NOENT
                || error == rustix::io::Errno::LOOP
                || error == rustix::io::Errno::NOTDIR =>
        {
            return Ok(false);
        }
        Err(error) => return Err(io::Error::from(error)),
    };
    let stat = fstat(&fd).map_err(io::Error::from)?;
    if !stat_is_owned_kind(&stat, true) {
        return Ok(false);
    }
    Ok(stat.st_mode & 0o777 == 0o700)
}

#[cfg(unix)]
fn is_owned_private_suffix_absolute_impl(
    path: &Path,
    owned_suffix_len: usize,
) -> io::Result<bool> {
    use rustix::fs::{Mode, OFlags, fstat, open};

    let names = absolute_normal_components(path)?;
    if owned_suffix_len == 0 || owned_suffix_len > names.len() {
        return Ok(false);
    }
    let first_owned = names.len() - owned_suffix_len;
    // Walk from `/` opening every component no-follow. A symlink or missing
    // component makes the target provably not an owner-private suffix, so fail
    // closed to `Ok(false)`. For owned-suffix components, additionally require
    // owner-owned mode exactly `0o700`.
    let Ok(mut dir) = open(
        Path::new("/"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return Ok(false);
    };
    for (index, name) in names.iter().enumerate() {
        let Ok(child) = open_child_dir(&dir, name) else {
            return Ok(false);
        };
        if index >= first_owned {
            let stat = fstat(&child).map_err(io::Error::from)?;
            if !stat_is_owned_kind(&stat, true) || stat.st_mode & 0o777 != 0o700 {
                return Ok(false);
            }
        }
        dir = child;
    }
    Ok(true)
}

#[cfg(unix)]
fn owned_dir_leaf_state_absolute_impl(path: &Path) -> AbsoluteDirLeaf {
    use rustix::fs::{AtFlags, Mode, OFlags, open, statat};

    let Ok(mut names) = absolute_normal_components(path) else {
        return AbsoluteDirLeaf::Unsafe;
    };
    let Some(leaf) = names.pop() else {
        return AbsoluteDirLeaf::Unsafe;
    };
    let Ok(mut dir) = open(
        Path::new("/"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return AbsoluteDirLeaf::Unsafe;
    };
    // Walk every intermediate ancestor no-follow. A *missing* ancestor is the
    // ordinary first-run case (the suffix can be created safely) and classifies
    // as `Missing`; a *symlink* ancestor (`ELOOP`/`ENOTDIR`) or any other error
    // classifies as `Unsafe` — never traversed.
    for name in &names {
        match open_child_dir(&dir, name) {
            Ok(child) => dir = child,
            Err(error)
                if error.raw_os_error() == Some(rustix::io::Errno::NOENT.raw_os_error()) =>
            {
                return AbsoluteDirLeaf::Missing;
            }
            Err(_) => return AbsoluteDirLeaf::Unsafe,
        }
    }
    match statat(&dir, leaf.as_os_str(), AtFlags::SYMLINK_NOFOLLOW) {
        // Any owner-owned, non-symlink directory enters the tightening path.
        // The leaf itself may already be 0o700 while another Zo-owned suffix
        // component is broad; the caller validates the complete suffix before
        // reaching this classifier.
        Ok(stat) if stat_is_owned_kind(&stat, true) => AbsoluteDirLeaf::OwnedDirTooBroad,
        // A missing leaf is the ordinary first-run case; the suffix can be
        // created safely.
        Err(error) if error == rustix::io::Errno::NOENT => AbsoluteDirLeaf::Missing,
        // Present but not an owner-owned directory (symlink, non-dir, foreign),
        // or any other stat error: fail closed.
        Ok(_) | Err(_) => AbsoluteDirLeaf::Unsafe,
    }
}

#[cfg(unix)]
fn restrict_existing_owner_only_absolute_impl(path: &Path) -> io::Result<()> {
    use rustix::fs::{Mode, OFlags, fchmod, fstat, openat};

    let (parent, leaf) = open_absolute_parent(path)?;
    let fd = openat(
        &parent,
        leaf.as_os_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let stat = fstat(&fd).map_err(io::Error::from)?;
    if !stat_is_owned_kind(&stat, true) {
        return Err(invalid_path(
            "secure state entry is not an owner-owned directory",
        ));
    }
    fchmod(&fd, Mode::from_raw_mode(0o700)).map_err(io::Error::from)
}

/// Open the regular-file leaf of an absolute path no-follow from a trusted
/// parent walk, validating it is a current-user-owned, non-symlink regular
/// file. `Ok(None)` for a missing leaf; `Err` for a symlink component, a
/// non-regular leaf, or a foreign-owned leaf.
#[cfg(unix)]
fn open_regular_file_absolute_no_follow(path: &Path) -> io::Result<Option<fs::File>> {
    use rustix::fs::{Mode, OFlags, fstat, openat};

    // A *missing* intermediate ancestor means the file is simply absent (a
    // config root that does not exist yet), so map the parent walk's `ENOENT`
    // to `Ok(None)`. A symlink ancestor surfaces as `ELOOP`/`ENOTDIR` and stays
    // an error — never followed.
    let (parent, leaf) = match open_absolute_parent(path) {
        Ok(pair) => pair,
        Err(error) if error.raw_os_error() == Some(rustix::io::Errno::NOENT.raw_os_error()) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let fd = match openat(
        &parent,
        leaf.as_os_str(),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(error) => return Err(io::Error::from(error)),
    };
    let stat = fstat(&fd).map_err(io::Error::from)?;
    if !stat_is_owned_kind(&stat, false) || stat.st_nlink != 1 {
        return Err(invalid_path(
            "secure absolute file is not a singly linked owner-owned regular file",
        ));
    }
    Ok(Some(fs::File::from(fd)))
}

#[cfg(unix)]
fn read_regular_file_absolute_no_follow_impl(path: &Path) -> io::Result<Option<String>> {
    let Some(mut file) = open_regular_file_absolute_no_follow(path)? else {
        return Ok(None);
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(Some(contents))
}

#[cfg(unix)]
fn is_safe_regular_file_absolute_no_follow_impl(path: &Path) -> io::Result<bool> {
    Ok(open_regular_file_absolute_no_follow(path)?.is_some())
}

#[cfg(unix)]
fn is_owned_private_regular_file_absolute_impl(path: &Path) -> io::Result<bool> {
    use rustix::fs::fstat;

    let Some(file) = open_regular_file_absolute_no_follow(path)? else {
        return Ok(false);
    };
    let stat = fstat(&file).map_err(io::Error::from)?;
    Ok(
        stat_is_owned_kind(&stat, false)
            && stat.st_nlink == 1
            && stat.st_mode.trailing_zeros() >= 6,
    )
}

#[cfg(unix)]
fn restrict_existing_owner_only_regular_file_absolute_impl(path: &Path) -> io::Result<()> {
    use rustix::fs::{Mode, fchmod, fstat};

    let file = open_regular_file_absolute_no_follow(path)?.ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "secure absolute file is missing")
    })?;
    let before = fstat(&file).map_err(io::Error::from)?;
    if !stat_is_owned_kind(&before, false) || before.st_nlink != 1 {
        return Err(invalid_path(
            "secure absolute file is not a singly linked owner-owned regular file",
        ));
    }
    fchmod(&file, Mode::from_raw_mode(0o600)).map_err(io::Error::from)?;
    let after = fstat(&file).map_err(io::Error::from)?;
    if !same_file_identity(&before, &after)
        || !stat_is_owned_kind(&after, false)
        || after.st_nlink != 1
        || after.st_mode & 0o077 != 0
    {
        return Err(invalid_path(
            "secure absolute file changed while permissions were tightened",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_private_dir_impl(root: &Path, relative: &Path) -> io::Result<()> {
    use rustix::fs::{Mode, fchmod, mkdirat};

    let mode = Mode::from_raw_mode(0o700);
    let mut dir = open_root(root)?;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        match mkdirat(&dir, name, mode) {
            Ok(()) => {}
            Err(error) if error == rustix::io::Errno::EXIST => {}
            Err(error) => return Err(io::Error::from(error)),
        }
        let child = open_child_dir(&dir, name)?;
        fchmod(&child, mode).map_err(io::Error::from)?;
        dir = child;
    }
    Ok(())
}

#[cfg(unix)]
fn open_parent_and_name(
    root: &Path,
    relative: &Path,
) -> io::Result<(std::os::fd::OwnedFd, std::ffi::OsString)> {
    let root = open_root(root)?;
    open_parent_and_name_from(&root, relative)
}

#[cfg(unix)]
fn sync_directory(directory: &std::os::fd::OwnedFd) -> io::Result<()> {
    fs::File::from(rustix::io::dup(directory).map_err(io::Error::from)?).sync_all()
}

#[cfg(unix)]
fn sync_parent_directory_impl(root: &Path, relative: &Path) -> io::Result<()> {
    let (parent, _) = open_parent_and_name(root, relative)?;
    sync_directory(&parent)
}

#[cfg(unix)]
fn open_parent_and_name_from(
    root: &std::os::fd::OwnedFd,
    relative: &Path,
) -> io::Result<(std::os::fd::OwnedFd, std::ffi::OsString)> {
    let mut components = relative.components().peekable();
    let mut dir = rustix::io::dup(root).map_err(io::Error::from)?;
    let mut file_name = None;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            continue;
        };
        if components.peek().is_some() {
            dir = open_child_dir(&dir, name)?;
        } else {
            file_name = Some(name.to_os_string());
        }
    }
    let file_name = file_name.ok_or_else(|| invalid_path("secure state path has no file name"))?;
    Ok((dir, file_name))
}

#[cfg(unix)]
fn same_file_identity(left: &rustix::fs::Stat, right: &rustix::fs::Stat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino
}

#[cfg(unix)]
fn current_euid_from_new_file(parent: &std::os::fd::OwnedFd) -> io::Result<u32> {
    use rustix::fs::{AtFlags, Mode, OFlags, fstat, openat, unlinkat};

    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let probe_name = format!(".zo-owner-probe-{}-{sequence}.tmp", std::process::id());
    let fd = openat(
        parent,
        probe_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(io::Error::from)?;
    let owner = fstat(&fd)
        .map(|stat| stat.st_uid)
        .map_err(io::Error::from);
    let cleanup = unlinkat(parent, probe_name.as_str(), AtFlags::empty()).map_err(io::Error::from);
    match (owner, cleanup) {
        (Ok(owner), Ok(())) => Ok(owner),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(io::Error::other(format!(
            "{error}; secure owner-probe cleanup also failed: {cleanup}"
        ))),
    }
}

#[cfg(unix)]
fn validate_opened_regular_file(
    parent: &std::os::fd::OwnedFd,
    file_name: &std::ffi::OsStr,
    fd: &impl std::os::fd::AsFd,
    expected_owner: Option<u32>,
) -> io::Result<rustix::fs::Stat> {
    use rustix::fs::{AtFlags, FileType, fstat, statat};

    let validate = |stat: &rustix::fs::Stat| {
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || expected_owner.is_some_and(|owner| stat.st_uid != owner)
            || stat.st_nlink != 1
        {
            Err(invalid_path(
                "secure state target must be a singly-linked regular file owned by the current user",
            ))
        } else {
            Ok(())
        }
    };
    let before = fstat(fd).map_err(io::Error::from)?;
    validate(&before)?;
    let linked = statat(parent, file_name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
    validate(&linked)?;
    let after = fstat(fd).map_err(io::Error::from)?;
    validate(&after)?;
    if !same_file_identity(&before, &linked) || !same_file_identity(&before, &after) {
        return Err(invalid_path(
            "secure state target changed while its identity was being validated",
        ));
    }
    Ok(after)
}

#[cfg(unix)]
fn reject_existing_non_file(
    parent: &std::os::fd::OwnedFd,
    file_name: &std::ffi::OsStr,
) -> io::Result<()> {
    use rustix::fs::{AtFlags, FileType, statat};

    match statat(parent, file_name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => Ok(()),
        Ok(_) => Err(invalid_path("secure state target is not a regular file")),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(()),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(unix)]
fn append_owner_only_impl(root: &Path, relative: &Path, contents: &[u8]) -> io::Result<()> {
    RetainedDir::open(root)?.append_owner_only(relative, contents)
}

#[cfg(unix)]
fn write_atomic_owner_only_impl(
    root: &Path,
    relative: &Path,
    contents: &[u8],
    durability: Durability,
    observe: &mut dyn FnMut(&'static str),
) -> io::Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};

    let (parent, file_name) = open_parent_and_name(root, relative)?;
    reject_existing_non_file(&parent, &file_name)?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_name = format!(".zo-state-{}-{sequence}.tmp", std::process::id());
    let fd = openat(
        &parent,
        temp_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(io::Error::from)?;
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o600)).map_err(io::Error::from)?;
    let mut file = fs::File::from(fd);
    let result = (|| {
        file.write_all(contents)?;
        observe("temp_written");
        if durability == Durability::Flushed {
            file.sync_all()?;
            observe("file_synced");
        }
        drop(file);
        renameat(&parent, temp_name.as_str(), &parent, &file_name).map_err(io::Error::from)
    })();
    match result {
        Ok(()) => {
            observe("renamed");
            if durability == Durability::Flushed {
                sync_directory(&parent)?;
                observe("parent_dir_synced");
            }
            Ok(())
        }
        Err(error) => match unlinkat(&parent, temp_name.as_str(), AtFlags::empty()) {
            Ok(()) => Err(error),
            Err(cleanup) if cleanup == rustix::io::Errno::NOENT => Err(error),
            Err(cleanup) => Err(io::Error::other(format!(
                "{error}; secure temporary file cleanup also failed: {cleanup}"
            ))),
        },
    }
}

#[cfg(unix)]
fn read_to_string_no_symlink_impl<F>(
    root: &Path,
    relative: &Path,
    validate: F,
) -> io::Result<String>
where
    F: FnOnce() -> io::Result<()>,
{
    use rustix::fs::{Mode, OFlags, openat};
    use std::io::{Seek as _, SeekFrom};

    let (parent, file_name) = open_parent_and_name(root, relative)?;
    let fd = openat(
        &parent,
        &file_name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    validate_opened_regular_file(&parent, &file_name, &fd, None)?;
    let mut file = fs::File::from(fd);
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    validate_opened_regular_file(&parent, &file_name, &file, None)?;

    // Establish a stable byte snapshot before the external provenance check.
    // Reading twice from the retained descriptor rejects concurrent in-place
    // mutation during the snapshot; later mutation cannot alter `contents`.
    file.seek(SeekFrom::Start(0))?;
    let mut confirmation = String::new();
    file.read_to_string(&mut confirmation)?;
    validate_opened_regular_file(&parent, &file_name, &file, None)?;
    if contents != confirmation {
        return Err(invalid_path(
            "secure state target changed while its bytes were being snapshotted",
        ));
    }

    validate()?;
    validate_opened_regular_file(&parent, &file_name, &file, None)?;
    Ok(contents)
}

#[cfg(unix)]
fn remove_file_no_symlink_impl(root: &Path, relative: &Path) -> io::Result<()> {
    use rustix::fs::{AtFlags, unlinkat};

    let (parent, file_name) = open_parent_and_name(root, relative)?;
    reject_existing_non_file(&parent, &file_name)?;
    match unlinkat(&parent, &file_name, AtFlags::empty()) {
        Ok(()) => sync_directory(&parent),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(()),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::{
        AbsoluteDirLeaf, Component, ExclusiveFileLock, OsString, Path, PathBuf, TEMP_SEQUENCE,
        invalid_path,
    };
    use cap_fs_ext::{
        FollowSymlinks, MetadataExt as _, OpenOptionsFollowExt as _,
        OpenOptionsMaybeDirExt as _,
    };
    use cap_std::fs::{Dir, File, OpenOptions, OpenOptionsExt as _};
    use cap_std::ambient_authority;
    use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        READ_CONTROL, WRITE_DAC,
    };

    fn root_and_names(path: &Path) -> io::Result<(PathBuf, Vec<OsString>)> {
        let absolute = if path.is_absolute() {
            core_types::paths::normalize_path_components(path)
        } else {
            core_types::paths::normalize_path_components(&std::env::current_dir()?.join(path))
        };
        let mut root = PathBuf::new();
        let mut names = Vec::new();
        let mut saw_prefix = false;
        let mut saw_root = false;
        for component in absolute.components() {
            match component {
                Component::Prefix(prefix) if !saw_prefix && names.is_empty() => {
                    root.push(prefix.as_os_str());
                    saw_prefix = true;
                }
                Component::RootDir if saw_prefix && names.is_empty() => {
                    root.push(component.as_os_str());
                    saw_root = true;
                }
                Component::Normal(name) if saw_root => names.push(name.to_os_string()),
                Component::CurDir => {}
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
                | Component::Normal(_) => {
                    return Err(invalid_path(
                        "secure Windows path must be absolute and stay on one volume",
                    ));
                }
            }
        }
        if !saw_prefix || !saw_root {
            return Err(invalid_path(
                "secure Windows path must include a drive or UNC root",
            ));
        }
        Ok((root, names))
    }

    fn entry_options(write: bool, directory: bool, write_dacl: bool) -> OpenOptions {
        let mut options = OpenOptions::new();
        let data_access = if write { GENERIC_WRITE } else { GENERIC_READ };
        let dacl_access = if write_dacl { WRITE_DAC } else { 0 };
        options
            .access_mode(data_access | READ_CONTROL | dacl_access | FILE_READ_ATTRIBUTES)
            .custom_flags(
                FILE_FLAG_OPEN_REPARSE_POINT
                    | if directory {
                        FILE_FLAG_BACKUP_SEMANTICS
                    } else {
                        0
                    },
            )
            .follow(FollowSymlinks::No)
            .maybe_dir(directory);
        options
    }

    fn open_child_dir(parent: &Dir, name: &std::ffi::OsStr, writable_acl: bool) -> io::Result<Dir> {
        let file = parent.open_with(
            name,
            &entry_options(writable_acl, true, writable_acl),
        )?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.is_symlink() {
            return Err(invalid_path(
                "secure Windows directory component is a symlink, junction, or non-directory",
            ));
        }
        Ok(Dir::from_std_file(file.into_std()))
    }

    fn open_volume_root(root: &Path) -> io::Result<Dir> {
        Dir::open_ambient_dir(root, ambient_authority())
    }

    fn open_absolute_parent(path: &Path) -> io::Result<(Dir, OsString)> {
        let (root, mut names) = root_and_names(path)?;
        let leaf = names
            .pop()
            .ok_or_else(|| invalid_path("secure path must name an entry below its volume"))?;
        let mut dir = open_volume_root(&root)?;
        for name in names {
            dir = open_child_dir(&dir, &name, false)?;
        }
        Ok((dir, leaf))
    }

    fn open_trusted_root(root: &Path) -> io::Result<Dir> {
        // Matching the Unix contract, the caller-selected root itself is the
        // capability boundary and may resolve platform aliases once. Every
        // attacker-controlled descendant is still walked handle-relatively.
        let canonical = std::fs::canonicalize(root)?;
        Dir::open_ambient_dir(canonical, ambient_authority())
    }

    fn open_parent(root: &Path, relative: &Path) -> io::Result<(Dir, OsString)> {
        let mut components = relative.components().peekable();
        let mut dir = open_trusted_root(root)?;
        let mut leaf = None;
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                continue;
            };
            if components.peek().is_some() {
                dir = open_child_dir(&dir, name, false)?;
            } else {
                leaf = Some(name.to_os_string());
            }
        }
        Ok((
            dir,
            leaf.ok_or_else(|| invalid_path("secure state path has no file name"))?,
        ))
    }

    fn validate_file(parent: &Dir, name: &std::ffi::OsStr, file: &File) -> io::Result<()> {
        let before = file.metadata()?;
        if !before.is_file()
            || before.is_symlink()
            || before.nlink() != 1
            || !core_types::paths::windows_handle_is_current_user_owned(file)?
        {
            return Err(invalid_path(
                "secure Windows target must be a singly linked regular file owned by the current user",
            ));
        }
        let linked = parent.symlink_metadata(name)?;
        let after = file.metadata()?;
        if !linked.is_file()
            || linked.is_symlink()
            || linked.nlink() != 1
            || before.dev() != linked.dev()
            || before.ino() != linked.ino()
            || before.dev() != after.dev()
            || before.ino() != after.ino()
        {
            return Err(invalid_path(
                "secure Windows target changed while its identity was validated",
            ));
        }
        Ok(())
    }

    // These flags mirror `OpenOptions` and keep each security-sensitive call
    // site explicit about create-new versus append semantics.
    #[allow(clippy::fn_params_excessive_bools)]
    fn open_regular(
        parent: &Dir,
        name: &std::ffi::OsStr,
        write: bool,
        create: bool,
        create_new: bool,
        append: bool,
    ) -> io::Result<File> {
        let mut options = entry_options(write, false, write);
        options
            .read(!write)
            .write(write && !append)
            .append(append)
            .create(create)
            .create_new(create_new);
        let file = parent.open_with(name, &options)?;
        validate_file(parent, name, &file)?;
        Ok(file)
    }

    fn ensure_child(parent: &Dir, name: &std::ffi::OsStr) -> io::Result<Dir> {
        match parent.create_dir(name) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let mut child = open_child_dir(parent, name, true)?;
        core_types::paths::restrict_windows_handle_owner_only(&mut child)?;
        Ok(child)
    }

    pub(super) fn ensure_private_dir(root: &Path, relative: &Path) -> io::Result<()> {
        let mut dir = open_trusted_root(root)?;
        for component in relative.components() {
            if let Component::Normal(name) = component {
                dir = ensure_child(&dir, name)?;
            }
        }
        Ok(())
    }

    pub(super) fn ensure_private_dir_absolute(
        path: &Path,
        owned_suffix_len: usize,
    ) -> io::Result<()> {
        let (root, names) = root_and_names(path)?;
        if names.is_empty() || owned_suffix_len == 0 || owned_suffix_len > names.len() {
            return Err(invalid_path(
                "secure absolute create requires an owned suffix below the volume root",
            ));
        }
        let first_owned = names.len() - owned_suffix_len;
        let mut dir = open_volume_root(&root)?;
        for (index, name) in names.iter().enumerate() {
            dir = if index < first_owned {
                open_child_dir(&dir, name, false)?
            } else {
                ensure_child(&dir, name)?
            };
        }
        Ok(())
    }

    pub(super) fn is_owned_private_dir_absolute(path: &Path) -> io::Result<bool> {
        let Ok((parent, leaf)) = open_absolute_parent(path) else {
            return Ok(false);
        };
        let Ok(dir) = open_child_dir(&parent, &leaf, false) else {
            return Ok(false);
        };
        core_types::paths::windows_handle_is_owner_only(&dir)
    }

    pub(super) fn is_owned_private_suffix_absolute(
        path: &Path,
        owned_suffix_len: usize,
    ) -> io::Result<bool> {
        let (root, names) = root_and_names(path)?;
        if owned_suffix_len == 0 || owned_suffix_len > names.len() {
            return Ok(false);
        }
        let first_owned = names.len() - owned_suffix_len;
        let mut dir = open_volume_root(&root)?;
        for (index, name) in names.iter().enumerate() {
            dir = match open_child_dir(&dir, name, false) {
                Ok(dir) => dir,
                Err(_) => return Ok(false),
            };
            if index >= first_owned
                && !core_types::paths::windows_handle_is_owner_only(&dir)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn owned_dir_leaf_state_absolute(path: &Path) -> AbsoluteDirLeaf {
        let Ok((parent, leaf)) = open_absolute_parent(path) else {
            return AbsoluteDirLeaf::Unsafe;
        };
        match open_child_dir(&parent, &leaf, false) {
            Ok(dir)
                if core_types::paths::windows_handle_is_current_user_owned(&dir)
                    .unwrap_or(false) =>
            {
                AbsoluteDirLeaf::OwnedDirTooBroad
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => AbsoluteDirLeaf::Missing,
            Ok(_) | Err(_) => AbsoluteDirLeaf::Unsafe,
        }
    }

    pub(super) fn restrict_existing_owner_only_absolute(path: &Path) -> io::Result<()> {
        let (parent, leaf) = open_absolute_parent(path)?;
        let mut dir = open_child_dir(&parent, &leaf, true)?;
        if !core_types::paths::windows_handle_is_current_user_owned(&dir)? {
            return Err(invalid_path(
                "secure Windows directory is not owned by the current user",
            ));
        }
        core_types::paths::restrict_windows_handle_owner_only(&mut dir)
    }

    fn open_absolute_regular(path: &Path) -> io::Result<Option<(Dir, OsString, File)>> {
        let (parent, leaf) = match open_absolute_parent(path) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        match open_regular(&parent, &leaf, false, false, false, false) {
            Ok(file) => Ok(Some((parent, leaf, file))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(super) fn read_regular_file_absolute_no_follow(
        path: &Path,
    ) -> io::Result<Option<String>> {
        let Some((parent, leaf, mut file)) = open_absolute_regular(path)? else {
            return Ok(None);
        };
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        validate_file(&parent, &leaf, &file)?;
        Ok(Some(contents))
    }

    pub(super) fn is_safe_regular_file_absolute_no_follow(path: &Path) -> io::Result<bool> {
        Ok(open_absolute_regular(path)?.is_some())
    }

    pub(super) fn is_owned_private_regular_file_absolute(path: &Path) -> io::Result<bool> {
        let Some((_, _, file)) = open_absolute_regular(path)? else {
            return Ok(false);
        };
        core_types::paths::windows_handle_is_owner_only(&file)
    }

    pub(super) fn restrict_existing_owner_only_regular_file_absolute(
        path: &Path,
    ) -> io::Result<()> {
        let (parent, leaf) = open_absolute_parent(path)?;
        let mut file = open_regular(&parent, &leaf, true, false, false, false)?;
        core_types::paths::restrict_windows_handle_owner_only(&mut file)?;
        validate_file(&parent, &leaf, &file)
    }

    pub(super) fn write_atomic_owner_only(
        root: &Path,
        relative: &Path,
        contents: &[u8],
        durability: super::Durability,
        observe: &mut dyn FnMut(&'static str),
    ) -> io::Result<()> {
        let (parent, leaf) = open_parent(root, relative)?;
        match parent.symlink_metadata(&leaf) {
            Ok(metadata)
                if metadata.is_file() && !metadata.is_symlink() && metadata.nlink() == 1 => {}
            Ok(_) => return Err(invalid_path("secure Windows target is not a regular file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = OsString::from(format!(
            ".zo-state-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut file = open_regular(&parent, &temp, true, false, true, false)?;
        let result = (|| {
            core_types::paths::restrict_windows_handle_owner_only(&mut file)?;
            file.write_all(contents)?;
            observe("temp_written");
            if durability == super::Durability::Flushed {
                file.sync_all()?;
                observe("file_synced");
            }
            drop(file);
            parent.rename(&temp, &parent, &leaf)
        })();
        if let Err(error) = result {
            let _ = parent.remove_file(&temp);
            return Err(error);
        }
        observe("renamed");
        Ok(())
    }

    pub(super) fn append_owner_only(
        root: &Path,
        relative: &Path,
        contents: &[u8],
    ) -> io::Result<()> {
        let (parent, leaf) = open_parent(root, relative)?;
        let mut file = open_regular(&parent, &leaf, true, true, false, true)?;
        core_types::paths::restrict_windows_handle_owner_only(&mut file)?;
        file.write_all(contents)
    }

    pub(super) fn read_to_string_no_symlink<F>(
        root: &Path,
        relative: &Path,
        validate: F,
    ) -> io::Result<String>
    where
        F: FnOnce() -> io::Result<()>,
    {
        let (parent, leaf) = open_parent(root, relative)?;
        let mut file = open_regular(&parent, &leaf, false, false, false, false)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        validate_file(&parent, &leaf, &file)?;
        file.seek(SeekFrom::Start(0))?;
        let mut confirmation = String::new();
        file.read_to_string(&mut confirmation)?;
        if contents != confirmation {
            return Err(invalid_path(
                "secure Windows target changed while its bytes were snapshotted",
            ));
        }
        validate()?;
        validate_file(&parent, &leaf, &file)?;
        Ok(contents)
    }

    pub(super) fn remove_file_no_symlink(root: &Path, relative: &Path) -> io::Result<()> {
        let (parent, leaf) = open_parent(root, relative)?;
        match parent.symlink_metadata(&leaf) {
            Ok(metadata)
                if metadata.is_file() && !metadata.is_symlink() && metadata.nlink() == 1 => {}
            Ok(_) => return Err(invalid_path("secure Windows target is not a regular file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
        parent.remove_file(leaf)
    }

    pub(super) fn try_lock_owner_only(
        root: &Path,
        relative: &Path,
    ) -> io::Result<Option<ExclusiveFileLock>> {
        let (parent, leaf) = open_parent(root, relative)?;
        let mut file = open_regular(&parent, &leaf, true, true, false, false)?;
        core_types::paths::restrict_windows_handle_owner_only(&mut file)?;
        let file = file.into_std();
        match file.try_lock() {
            Ok(()) => Ok(Some(ExclusiveFileLock { file })),
            Err(error) => {
                let error = io::Error::from(error);
                if error.kind() == io::ErrorKind::WouldBlock {
                    Ok(None)
                } else {
                    Err(error)
                }
            }
        }
    }

    pub(super) fn sync_parent_directory(_root: &Path, _relative: &Path) {
        // Windows does not expose a portable directory-fsync equivalent.
        // The preceding file FlushFileBuffers/atomic rename remains the data
        // durability barrier; treating this as success keeps persistence
        // usable without weakening ACL or no-follow validation.
    }
}

/// Whether a `lstat` result is a non-symlink entry of the expected kind owned by
/// the effective user.
#[cfg(unix)]
fn stat_is_owned_kind(stat: &rustix::fs::Stat, expect_dir: bool) -> bool {
    use rustix::fs::FileType;

    let kind = FileType::from_raw_mode(stat.st_mode as rustix::fs::RawMode);
    let kind_ok = if expect_dir {
        kind == FileType::Directory
    } else {
        kind == FileType::RegularFile
    };
    kind_ok && u64::from(stat.st_uid) == u64::from(rustix::process::geteuid().as_raw())
}

#[cfg(not(any(unix, windows)))]
fn unsupported_secure_mutation() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "secure state mutation requires Unix no-follow directory handles",
    )
}

#[cfg(windows)]
fn ensure_private_dir_impl(root: &Path, relative: &Path) -> io::Result<()> {
    windows_impl::ensure_private_dir(root, relative)
}

#[cfg(not(any(unix, windows)))]
fn ensure_private_dir_impl(_root: &Path, _relative: &Path) -> io::Result<()> {
    Err(unsupported_secure_mutation())
}

#[cfg(windows)]
fn write_atomic_owner_only_impl(
    root: &Path,
    relative: &Path,
    contents: &[u8],
    durability: Durability,
    observe: &mut dyn FnMut(&'static str),
) -> io::Result<()> {
    windows_impl::write_atomic_owner_only(root, relative, contents, durability, observe)
}

#[cfg(not(any(unix, windows)))]
fn write_atomic_owner_only_impl(
    _root: &Path,
    _relative: &Path,
    _contents: &[u8],
    _durability: Durability,
    _observe: &mut dyn FnMut(&'static str),
) -> io::Result<()> {
    Err(unsupported_secure_mutation())
}

#[cfg(windows)]
fn append_owner_only_impl(root: &Path, relative: &Path, contents: &[u8]) -> io::Result<()> {
    windows_impl::append_owner_only(root, relative, contents)
}

#[cfg(not(any(unix, windows)))]
fn append_owner_only_impl(_root: &Path, _relative: &Path, _contents: &[u8]) -> io::Result<()> {
    Err(unsupported_secure_mutation())
}

#[cfg(windows)]
fn read_to_string_no_symlink_impl<F>(
    root: &Path,
    relative: &Path,
    validate: F,
) -> io::Result<String>
where
    F: FnOnce() -> io::Result<()>,
{
    windows_impl::read_to_string_no_symlink(root, relative, validate)
}

#[cfg(not(any(unix, windows)))]
fn read_to_string_no_symlink_impl<F>(
    root: &Path,
    relative: &Path,
    _validate: F,
) -> io::Result<String>
where
    F: FnOnce() -> io::Result<()>,
{
    let path = root.join(relative);
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(error),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "secure state reads require Unix no-follow directory handles",
        )),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn remove_file_no_symlink_impl(root: &Path, relative: &Path) -> io::Result<()> {
    windows_impl::remove_file_no_symlink(root, relative)
}

#[cfg(not(any(unix, windows)))]
fn remove_file_no_symlink_impl(_root: &Path, _relative: &Path) -> io::Result<()> {
    Err(unsupported_secure_mutation())
}

#[cfg(windows)]
#[allow(clippy::unnecessary_wraps)]
fn sync_parent_directory_impl(root: &Path, relative: &Path) -> io::Result<()> {
    windows_impl::sync_parent_directory(root, relative);
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_parent_directory_impl(_root: &Path, _relative: &Path) -> io::Result<()> {
    Err(unsupported_secure_mutation())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "zo-secure-fs-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    /// A published replace stops at the rename; a flushed one syncs the bytes
    /// before it and the directory after it. The task registry commits on the
    /// published road: on macOS each sync is an `F_FULLFSYNC` (~4 ms), and a
    /// background spawn used to pay six of them (t-2941 axis F).
    #[cfg(unix)]
    #[test]
    fn a_published_replace_stops_at_the_rename() {
        use std::os::unix::fs::MetadataExt as _;

        let root = TestRoot::new();
        let state = Path::new("tasks.json");
        let mut steps = Vec::new();
        write_atomic_owner_only_observed(root.path(), state, b"{}", Durability::Published, &mut |step| {
            steps.push(step);
        })
        .unwrap();
        assert_eq!(steps, ["temp_written", "renamed"]);
        assert_eq!(fs::read_to_string(root.path().join(state)).unwrap(), "{}");
        assert_eq!(
            fs::metadata(root.path().join(state)).unwrap().mode() & 0o777,
            0o600
        );

        let mut steps = Vec::new();
        write_atomic_owner_only_observed(root.path(), state, b"{\"rev\":1}", Durability::Flushed, &mut |step| {
            steps.push(step);
        })
        .unwrap();
        assert_eq!(
            steps,
            ["temp_written", "file_synced", "renamed", "parent_dir_synced"]
        );
        assert_eq!(fs::read_to_string(root.path().join(state)).unwrap(), "{\"rev\":1}");
    }

    #[cfg(unix)]
    #[test]
    fn absolute_create_makes_missing_suffix_and_reports_owner_private() {
        use std::os::unix::fs::PermissionsExt;
        // `TestRoot` lives under `std::env::temp_dir()`, which on macOS is the
        // `/var` alias — so this also exercises the bounded `/var` normalization.
        let root = TestRoot::new();
        let target = root.path().join("projects").join("slug").join("state");
        assert_eq!(
            owned_dir_leaf_state_absolute(&target),
            AbsoluteDirLeaf::Missing
        );
        ensure_private_dir_absolute(&target, 3).unwrap();
        assert!(target.is_dir(), "missing suffix must be created");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(is_owned_private_dir_absolute(&target).unwrap());
        assert!(is_owned_private_suffix_absolute(&target, 3).unwrap());
        // Every created owned-suffix component is chmod'd to owner-only.
        assert_eq!(
            fs::metadata(root.path().join("projects"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700,
            "created intermediate suffix is owner-only"
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_create_tightens_preexisting_broad_owned_suffix_dirs() {
        use std::os::unix::fs::PermissionsExt;
        // The blocking-review scenario: a pre-existing broad `$ZO_STATE_DIR`
        // base and `projects/` at 0o777, with `slug/state` missing below. Every
        // Zo-owned suffix directory (base + projects + slug + state) must be
        // tightened to 0o700, while the non-Zo *parent* of the base is untouched.
        let root = TestRoot::new();
        let non_zo_parent = root.path().join("parent");
        fs::create_dir(&non_zo_parent).unwrap();
        fs::set_permissions(&non_zo_parent, fs::Permissions::from_mode(0o755)).unwrap();
        // The explicit state base and its `projects/` already exist and are broad.
        let base = non_zo_parent.join("state-base");
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o777)).unwrap();
        let projects = base.join("projects");
        fs::create_dir(&projects).unwrap();
        fs::set_permissions(&projects, fs::Permissions::from_mode(0o777)).unwrap();
        // `slug/state` is missing below `projects/`.
        let target = projects.join("slug").join("state");
        assert_eq!(owned_dir_leaf_state_absolute(&target), AbsoluteDirLeaf::Missing);

        // Owned suffix is base + projects + slug + state = 4.
        ensure_private_dir_absolute(&target, 4).unwrap();

        for dir in [&base, &projects, &projects.join("slug"), &target] {
            assert_eq!(
                fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700,
                "Zo-owned suffix dir {dir:?} must be tightened to 0o700"
            );
        }
        // The non-Zo parent is never chmod'd.
        assert_eq!(
            fs::metadata(&non_zo_parent).unwrap().permissions().mode() & 0o777,
            0o755,
            "non-Zo parent must remain unchanged"
        );
        assert!(
            is_owned_private_suffix_absolute(&target, 4).unwrap(),
            "full owned suffix must satisfy the private postcondition"
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_walker_refuses_intermediate_symlink_ancestor() {
        use std::os::unix::fs::PermissionsExt;
        let root = TestRoot::new();
        // A real directory that already contains `projects/` (a pre-existing
        // descendant), reached only through a symlinked intermediate ancestor.
        let real = root.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::create_dir(real.join("projects")).unwrap();
        fs::set_permissions(real.join("projects"), fs::Permissions::from_mode(0o777)).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let target = link.join("projects").join("slug").join("state");
        // Classification must not treat the symlinked-through leaf as safe.
        assert_eq!(owned_dir_leaf_state_absolute(&target), AbsoluteDirLeaf::Unsafe);
        // Creation must fail rather than traverse the symlink.
        assert!(ensure_private_dir_absolute(&target, 3).is_err());
        assert!(!is_owned_private_dir_absolute(&target).unwrap());
        assert_eq!(
            fs::read_dir(real.join("projects")).unwrap().count(),
            0,
            "nothing created through the symlinked ancestor"
        );
        assert_eq!(
            fs::metadata(real.join("projects")).unwrap().permissions().mode() & 0o777,
            0o777,
            "the symlink target's permissions must be untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_tighten_restores_owner_only_without_following_leaf_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let root = TestRoot::new();
        // A broad but owner-owned directory is tightened.
        let dir = root.path().join("broad");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            owned_dir_leaf_state_absolute(&dir),
            AbsoluteDirLeaf::OwnedDirTooBroad
        );
        restrict_existing_owner_only_absolute(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );

        // A symlinked leaf is classified Unsafe and refused, never followed.
        let real = root.path().join("real-target");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o777)).unwrap();
        let leaf_link = root.path().join("leaf-link");
        std::os::unix::fs::symlink(&real, &leaf_link).unwrap();
        assert_eq!(
            owned_dir_leaf_state_absolute(&leaf_link),
            AbsoluteDirLeaf::Unsafe
        );
        assert!(restrict_existing_owner_only_absolute(&leaf_link).is_err());
        assert_eq!(
            fs::metadata(&real).unwrap().permissions().mode() & 0o777,
            0o777,
            "symlink target must be untouched"
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_directories_and_files_are_owner_only() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let root = TestRoot::new();
        ensure_private_dir(root.path(), Path::new("private/nested")).unwrap();
        fs::set_permissions(
            root.path().join("private"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        ensure_private_dir(root.path(), Path::new("private/nested")).unwrap();
        write_atomic_owner_only(root.path(), Path::new("private/nested/state"), b"state").unwrap();
        append_owner_only(root.path(), Path::new("private/nested/events.jsonl"), b"event\n")
            .unwrap();

        assert_eq!(
            fs::metadata(root.path().join("private")).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join("private/nested"))
                .unwrap()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join("private/nested/state"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(root.path().join("private/nested/events.jsonl"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn advisory_lock_is_reacquired_without_removing_its_file() {
        use std::os::unix::fs::MetadataExt;

        let root = TestRoot::new();
        ensure_private_dir(root.path(), Path::new("private")).unwrap();
        let relative = Path::new("private/state.lock");
        let lock_path = root.path().join(relative);

        let first = try_lock_owner_only(root.path(), relative).unwrap().unwrap();
        assert!(lock_path.exists());
        assert_eq!(fs::metadata(&lock_path).unwrap().mode() & 0o777, 0o600);
        assert!(try_lock_owner_only(root.path(), relative).unwrap().is_none());

        let duplicated_descriptor = first.file.try_clone().unwrap();
        drop(first);
        assert!(lock_path.exists());
        assert!(
            try_lock_owner_only(root.path(), relative).unwrap().is_some(),
            "dropping the capability must explicitly unlock even while a duplicated descriptor is open"
        );
        drop(duplicated_descriptor);
    }

    #[cfg(unix)]
    #[test]
    fn append_and_lock_reject_hardlinks_without_chmod_or_write_side_effects() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let root = TestRoot::new();
        ensure_private_dir(root.path(), Path::new("private")).unwrap();
        let victim = root.path().join("victim");
        fs::write(&victim, "sentinel").unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap();
        let append_link = root.path().join("private/events.jsonl");
        let lock_link = root.path().join("private/state.lock");
        fs::hard_link(&victim, &append_link).unwrap();
        fs::hard_link(&victim, &lock_link).unwrap();

        assert!(append_owner_only(root.path(), Path::new("private/events.jsonl"), b"changed").is_err());
        assert!(try_lock_owner_only(root.path(), Path::new("private/state.lock")).is_err());
        assert!(read_to_string_no_symlink(root.path(), Path::new("private/events.jsonl")).is_err());

        assert_eq!(fs::read_to_string(&victim).unwrap(), "sentinel");
        assert_eq!(fs::metadata(&victim).unwrap().mode() & 0o777, 0o644);
        assert_eq!(fs::metadata(&victim).unwrap().nlink(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn secure_append_and_lock_remove_owner_probe_files() {
        let root = TestRoot::new();
        ensure_private_dir(root.path(), Path::new("private")).unwrap();

        append_owner_only(root.path(), Path::new("private/events.jsonl"), b"event\n").unwrap();
        let lock = try_lock_owner_only(root.path(), Path::new("private/state.lock"))
            .unwrap()
            .unwrap();
        drop(lock);

        let names = fs::read_dir(root.path().join("private"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|name| !name.to_string_lossy().contains("owner-probe")));
    }

    #[cfg(unix)]
    #[test]
    fn retained_directory_operations_ignore_path_replacement() {
        use std::os::unix::fs::MetadataExt;

        let root = TestRoot::new();
        let state = root.path().join("state");
        let original = root.path().join("original-state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("entry.md"), "entry").unwrap();
        let retained = RetainedDir::open(&state).unwrap();

        fs::rename(&state, &original).unwrap();
        fs::create_dir(&state).unwrap();
        fs::write(state.join("MEMORY.md"), "replacement").unwrap();

        let mut entry = retained
            .open_regular_file(Path::new("entry.md"))
            .unwrap();
        assert_eq!(entry.read_to_string().unwrap(), "entry");
        let archive = retained
            .ensure_private_subdir(Path::new("archive"))
            .unwrap();
        assert!(retained
            .rename_file_no_replace(
                Path::new("entry.md"),
                &entry,
                &archive,
                Path::new("entry.md"),
            )
            .unwrap());
        retained
            .write_atomic_owner_only_retained(Path::new("MEMORY.md"), b"retained")
            .unwrap();

        assert_eq!(
            fs::read_to_string(original.join("archive/entry.md")).unwrap(),
            "entry"
        );
        assert_eq!(
            fs::metadata(original.join("archive")).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(original.join("archive/entry.md"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(original.join("MEMORY.md")).unwrap().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(state.join("MEMORY.md")).unwrap(), "replacement");
    }

    #[cfg(unix)]
    #[test]
    fn durable_append_orders_write_file_and_parent_directory_sync() {
        let root = TestRoot::new();
        let private = ensure_private_dir(root.path(), Path::new("private")).unwrap();
        let retained = RetainedDir::open(&private).unwrap();
        let mut events = Vec::new();

        let mut observe = |event| events.push(event);
        retained
            .append_owner_only_durable_observed(
                Path::new("events.jsonl"),
                b"event\n",
                &mut observe,
            )
            .unwrap();

        assert_eq!(
            events,
            vec!["append_write", "file_synced", "parent_dir_synced"]
        );
        assert_eq!(
            fs::read_to_string(private.join("events.jsonl")).unwrap(),
            "event\n"
        );
    }

    #[cfg(not(any(unix, windows)))]
    #[test]
    fn non_unix_mutations_fail_closed_and_reads_do_not_create_directories() {
        let root = TestRoot::new();
        let relative = Path::new("private/state");

        assert_eq!(
            ensure_private_dir(root.path(), Path::new("private"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            write_atomic_owner_only(root.path(), relative, b"state")
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            remove_file_no_symlink(root.path(), relative)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            try_lock_owner_only(root.path(), relative).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            read_to_string_no_symlink(root.path(), relative)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert!(!root.path().join("private").exists());

        fs::write(root.path().join("state"), "state").unwrap();
        assert_eq!(
            read_to_string_no_symlink(root.path(), Path::new("state"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_state_round_trip_and_symlink_refusal() {
        let root = TestRoot::new();
        ensure_private_dir(root.path(), Path::new("private/nested")).unwrap();
        let private = root.path().join("private/nested");
        assert!(is_owned_private_dir_absolute(&private).unwrap());

        let state = Path::new("private/nested/state.json");
        write_atomic_owner_only(root.path(), state, b"one").unwrap();
        append_owner_only(root.path(), state, b" two").unwrap();
        assert_eq!(
            read_to_string_no_symlink(root.path(), state).unwrap(),
            "one two"
        );
        assert!(is_owned_private_regular_file_absolute(&root.path().join(state)).unwrap());

        let lock = try_lock_owner_only(root.path(), Path::new("private/nested/state.lock"))
            .unwrap()
            .unwrap();
        assert!(
            try_lock_owner_only(root.path(), Path::new("private/nested/state.lock"))
                .unwrap()
                .is_none()
        );
        drop(lock);

        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("state"), "outside").unwrap();
        let junction = root.path().join("private/link");
        // Windows may require Developer Mode or a privilege to create a
        // directory symlink. When the OS permits creation, the actual read
        // gate must refuse it rather than traversing to `outside`.
        if std::os::windows::fs::symlink_dir(&outside, &junction).is_ok() {
            assert!(
                read_to_string_no_symlink(root.path(), Path::new("private/link/state")).is_err()
            );
        }
    }
}
