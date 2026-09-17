//! One same-directory atomic file primitive for new persistence code.
//!
//! Settings and Jira keep ownership of their domain-specific recovery formats,
//! while this module alone owns the platform replace operation and its
//! durability outcome. In particular, failure to sync a parent *after* rename
//! is not reported as if the visible commit never happened.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Durability {
    /// File contents and the containing directory were synchronised.
    Durable,
    /// Windows exposes the atomic replacement but has no portable Rust
    /// equivalent of Unix directory fsync. The inherited per-user AppData ACL
    /// remains the security boundary; this is not a claim of power-loss proof.
    #[cfg(not(unix))]
    PlatformBestEffort,
    /// The replacement is already visible, but syncing its directory failed.
    VisibleButSyncFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub durability: Durability,
}

impl CommitOutcome {
    /// Whether this platform provided every durability guarantee it exposes.
    /// Windows has no portable directory-fsync primitive, so its explicit
    /// best-effort result is the truthful success baseline, not an unknown
    /// post-commit failure.
    #[must_use]
    pub fn platform_durable(&self) -> bool {
        match self.durability {
            Durability::Durable => true,
            #[cfg(not(unix))]
            Durability::PlatformBestEffort => true,
            Durability::VisibleButSyncFailed(_) => false,
        }
    }
}

/// Replace `target` with exact bytes, without a missing-file window.
pub fn replace_bytes(target: &Path, bytes: &[u8]) -> io::Result<CommitOutcome> {
    replace_bytes_with_directory_sync(target, bytes, sync_parent_directory)
}

fn replace_bytes_with_directory_sync(
    target: &Path,
    bytes: &[u8],
    sync_directory_parent: impl FnMut(&Path) -> Durability,
) -> io::Result<CommitOutcome> {
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "target has no parent directory",
        )
    })?;
    let directory_outcome = ensure_private_directory_durable_with(parent, sync_directory_parent)?;
    let (temporary, mut file) = private_temporary_file(target)?;
    let mut guard = TemporaryFile::new(temporary.clone());
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    let replacement_outcome = replace_staged(&temporary, target)?;
    guard.disarm();
    Ok(CommitOutcome {
        durability: weaker_durability(directory_outcome.durability, replacement_outcome.durability),
    })
}

/// Publish a fully written, same-directory staging file and report the
/// durability of the already-visible replacement separately from the rename.
///
/// Callers must never turn [`Durability::VisibleButSyncFailed`] into an error
/// that implies the old destination is still visible. Migration may require a
/// fully durable result before publishing its authority receipt; ordinary
/// settings commits instead acknowledge the visible value and can report the
/// weaker durability independently.
pub fn replace_staged(temporary: &Path, target: &Path) -> io::Result<CommitOutcome> {
    replace_staged_with(temporary, target, sync_parent_directory)
}

/// Remove a committed file and report the durability of the already-visible
/// deletion. The same post-visibility rule as [`replace_staged`] applies: a
/// parent sync failure cannot be reported as if the file were still present.
pub fn remove_file(target: &Path) -> io::Result<CommitOutcome> {
    remove_visible(target, |path| fs::remove_file(path))
}

/// Remove an empty committed directory with the same visibility/durability
/// distinction as a file replacement. This is used only for directories a
/// transaction journal proves it created.
pub fn remove_directory(target: &Path) -> io::Result<CommitOutcome> {
    remove_visible(target, |path| fs::remove_dir(path))
}

fn remove_visible(
    target: &Path,
    remove: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<CommitOutcome> {
    remove(target)?;
    sync_target_parent_with(target, sync_parent_directory)
}

fn replace_staged_with(
    temporary: &Path,
    target: &Path,
    sync_parent: impl FnOnce(&Path) -> Durability,
) -> io::Result<CommitOutcome> {
    atomic_replace(temporary, target)?;
    sync_target_parent_with(target, sync_parent)
}

/// Re-synchronise the parent of a target whose directory entry is already
/// visible. This is the retry primitive for a previous
/// [`Durability::VisibleButSyncFailed`] outcome; it never republishes or removes
/// the target itself.
pub fn sync_visible_parent(target: &Path) -> io::Result<CommitOutcome> {
    sync_target_parent_with(target, sync_parent_directory)
}

fn sync_target_parent_with(
    target: &Path,
    sync_parent: impl FnOnce(&Path) -> Durability,
) -> io::Result<CommitOutcome> {
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "target has no parent directory",
        )
    })?;
    Ok(CommitOutcome {
        durability: sync_parent(parent),
    })
}

/// Atomically install a same-directory staged file, replacing the destination.
/// This platform operation lives here so settings, Jira, and migration depend
/// on one persistence primitive rather than on one another's feature module.
#[cfg(not(windows))]
pub fn atomic_replace(temporary: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(windows)]
pub fn atomic_replace(temporary: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let temporary: Vec<u16> = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: both pointers refer to NUL-terminated UTF-16 buffers that live
    // through the call, and staged files are created beside their target.
    let moved = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn ensure_private_directory(path: &Path) -> io::Result<()> {
    ensure_private_directory_durable(path).map(|_| ())
}

/// Reject a link-like or non-directory application-owned directory when it
/// exists, without creating an otherwise unused directory.
pub fn require_plain_directory_if_present(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => require_plain_directory(path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Ensure a private directory exists and report whether each newly visible
/// directory entry reached the platform's durability boundary.
///
/// On Unix each missing component is created with mode 0700 from its first
/// visible instant. The final directory's parent is synchronised even when the
/// directory already existed, allowing a caller to retry an earlier sync
/// failure without recreating anything.
pub fn ensure_private_directory_durable(path: &Path) -> io::Result<CommitOutcome> {
    ensure_private_directory_durable_with(path, sync_parent_directory)
}

fn ensure_private_directory_durable_with(
    path: &Path,
    mut sync_parent: impl FnMut(&Path) -> Durability,
) -> io::Result<CommitOutcome> {
    let path = absolute_path(path)?;
    if path.parent().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory must not be a filesystem root",
        ));
    }
    let mut chain: Vec<_> = path
        .ancestors()
        .filter(|directory| !directory.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .collect();
    chain.reverse();

    let mut missing = Vec::new();
    for directory in chain {
        match fs::symlink_metadata(&directory) {
            Ok(metadata) => {
                require_plain_directory(&directory, &metadata)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(directory);
            }
            Err(error) => return Err(error),
        }
    }

    let mut durability = None;
    for directory in &missing {
        let parent = directory.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "private directory must have a parent",
            )
        })?;
        let parent_metadata = fs::symlink_metadata(parent)?;
        require_plain_directory(parent, &parent_metadata)?;
        match create_private_directory(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = fs::symlink_metadata(directory)?;
        require_plain_directory(directory, &metadata)?;
        set_private_directory_permissions(directory)?;
        merge_durability(
            &mut durability,
            sync_target_parent_with(directory, &mut sync_parent)?.durability,
        );
    }

    if missing.is_empty() {
        // Keep the compatibility contract of `ensure_private_directory`: an
        // existing final directory is tightened to owner-only on Unix. Its
        // parent sync is deliberately repeated so a previous failure can heal.
        set_private_directory_permissions(&path)?;
        merge_durability(
            &mut durability,
            sync_target_parent_with(&path, sync_parent)?.durability,
        );
    }

    durability
        .map(|durability| CommitOutcome { durability })
        .ok_or_else(|| io::Error::other("directory parent was not synchronised"))
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory path is empty",
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    normalize_platform_root_alias(absolute)
}

/// Root-level aliases such as macOS `/var -> private/var` are controlled by the
/// operating system, not by the managed state tree. Resolve that one trusted
/// namespace edge before checking every remaining component for symlinks.
#[cfg(unix)]
fn normalize_platform_root_alias(path: PathBuf) -> io::Result<PathBuf> {
    use std::path::Component;

    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Ok(path);
    }
    let Some(Component::Normal(first)) = components.next() else {
        return Ok(path);
    };
    let first = Path::new("/").join(first);
    let metadata = match fs::symlink_metadata(&first) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_symlink() {
        return Ok(path);
    }

    let mut normalized = fs::canonicalize(first)?;
    for component in components {
        normalized.push(component.as_os_str());
    }
    Ok(normalized)
}

#[cfg(not(unix))]
fn normalize_platform_root_alias(path: PathBuf) -> io::Result<PathBuf> {
    Ok(path)
}

/// Whether metadata obtained without following the final component describes a
/// regular directory rather than a link-like filesystem object.
///
/// Windows junctions and other reparse points are not always reported by
/// `FileType::is_symlink`, so callers must use this predicate instead of
/// open-coding `is_dir() && !is_symlink()`.
pub fn is_plain_directory(metadata: &fs::Metadata) -> bool {
    metadata.is_dir() && !has_link_semantics(metadata)
}

/// File counterpart of [`is_plain_directory`]. Cloud placeholders and other
/// file reparse points are rejected along with symbolic links.
pub fn is_plain_file(metadata: &fs::Metadata) -> bool {
    metadata.is_file() && !has_link_semantics(metadata)
}

/// Open an application-owned regular file without following a link-like final
/// component. The path is checked before and after the open against the
/// identity of the returned handle, so a concurrent replacement fails closed.
pub fn open_plain_file(path: &Path) -> io::Result<File> {
    open_existing_plain_file(path, false)
}

/// Read an application-owned regular file through [`open_plain_file`].
pub fn read_plain_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_plain_file(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Report whether a plain file exists while rejecting link-like or special
/// entries. This is for recovery decisions that must validate before rename
/// or removal but do not need to read file contents.
pub fn require_plain_file_if_present(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            require_plain_file(path, &metadata)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Open the stable sentinel used by a higher-level process/file lock.
///
/// A new file is private from its first visible instant. An existing file is
/// opened without following a symbolic link or Windows reparse point, and its
/// handle identity must match the path both before and after opening. This
/// function deliberately does not acquire a lock; settings and Jira retain
/// their existing lock implementations and process mutex ordering.
pub fn private_lock_file(path: &Path) -> io::Result<File> {
    let file = match open_new_private_file(path) {
        Ok(file) => {
            verify_open_handle(path, &file, None)?;
            file
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            open_existing_plain_file(path, true)?
        }
        Err(error) => return Err(error),
    };
    set_private_file_permissions(&file)?;
    verify_open_handle(path, &file, None)?;
    Ok(file)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    file: u64,
}

fn open_existing_plain_file(path: &Path, writable: bool) -> io::Result<File> {
    let before = plain_path_identity(path)?;
    let file = open_file_no_follow(path, writable)?;
    verify_open_handle(path, &file, Some(before))?;
    Ok(file)
}

fn open_new_private_file(path: &Path) -> io::Result<File> {
    let mut options = no_follow_options(true);
    options.create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

fn open_file_no_follow(path: &Path, writable: bool) -> io::Result<File> {
    no_follow_options(writable).open(path)
}

fn no_follow_options(writable: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(writable);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // O_NONBLOCK keeps a raced-in FIFO/device from blocking before its
        // opened metadata can be rejected. It has no effect on regular files.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}

fn verify_open_handle(
    path: &Path,
    file: &File,
    before: Option<FileIdentity>,
) -> io::Result<FileIdentity> {
    let opened = opened_plain_identity(path, file)?;
    if before.is_some_and(|before| before != opened) {
        return Err(changed_file_identity(path));
    }
    if plain_path_identity(path)? != opened {
        return Err(changed_file_identity(path));
    }
    Ok(opened)
}

fn opened_plain_identity(path: &Path, file: &File) -> io::Result<FileIdentity> {
    let metadata = file.metadata()?;
    require_plain_file(path, &metadata)?;
    file_identity(file, &metadata)
}

#[cfg(unix)]
fn plain_path_identity(path: &Path) -> io::Result<FileIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    require_plain_file(path, &metadata)?;
    file_identity_from_metadata(&metadata)
}

#[cfg(windows)]
fn plain_path_identity(path: &Path) -> io::Result<FileIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    require_plain_file(path, &metadata)?;
    let file = open_file_no_follow(path, false)?;
    opened_plain_identity(path, &file)
}

#[cfg(not(any(unix, windows)))]
fn plain_path_identity(_path: &Path) -> io::Result<FileIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "plain-file identity verification is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn file_identity(_file: &File, metadata: &fs::Metadata) -> io::Result<FileIdentity> {
    file_identity_from_metadata(metadata)
}

#[cfg(unix)]
fn file_identity_from_metadata(metadata: &fs::Metadata) -> io::Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt as _;

    Ok(FileIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
    })
}

#[cfg(windows)]
fn file_identity(file: &File, _metadata: &fs::Metadata) -> io::Result<FileIdentity> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` owns a valid handle and `information` is writable for the
    // complete duration of this synchronous call.
    let result = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileIdentity {
        volume: u64::from(information.dwVolumeSerialNumber),
        file: (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    })
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_file: &File, _metadata: &fs::Metadata) -> io::Result<FileIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "plain-file identity verification is unsupported on this platform",
    ))
}

fn require_plain_file(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    if !is_plain_file(metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "file path is not a plain file or is a link/reparse point: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn changed_file_identity(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("file identity changed while opening: {}", path.display()),
    )
}

#[cfg(unix)]
fn set_private_file_permissions(file: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_file: &File) -> io::Result<()> {
    // Windows files inherit the per-user AppData DACL; preserve enterprise
    // ACLs instead of replacing that descriptor with a hand-built one.
    Ok(())
}

#[cfg(windows)]
fn has_link_semantics(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn has_link_semantics(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn require_plain_directory(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    if has_link_semantics(metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "directory path is a symbolic link or reparse point: {}",
                path.display()
            ),
        ));
    }
    if !is_plain_directory(metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("directory path is not a directory: {}", path.display()),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().create(path)
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> io::Result<()> {
    // Windows directories inherit their DACL from the per-user AppData parent.
    // Replacing that inherited descriptor here would erase enterprise ACLs and
    // is less correct than preserving the platform's own user boundary.
    Ok(())
}

fn merge_durability(current: &mut Option<Durability>, next: Durability) {
    *current = Some(match current.take() {
        Some(previous) => weaker_durability(previous, next),
        None => next,
    });
}

fn weaker_durability(first: Durability, second: Durability) -> Durability {
    match (first, second) {
        (failure @ Durability::VisibleButSyncFailed(_), _) => failure,
        (_, failure @ Durability::VisibleButSyncFailed(_)) => failure,
        #[cfg(not(unix))]
        (Durability::PlatformBestEffort, _) | (_, Durability::PlatformBestEffort) => {
            Durability::PlatformBestEffort
        }
        (Durability::Durable, Durability::Durable) => Durability::Durable,
    }
}

fn private_temporary_file(target: &Path) -> io::Result<(PathBuf, File)> {
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "target has no parent directory",
        )
    })?;
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no file name"))?
        .to_string_lossy();
    loop {
        let id = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{name}.tmp-{}-{id}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Durability {
    match File::open(parent).and_then(|directory| directory.sync_all()) {
        Ok(()) => Durability::Durable,
        Err(error) => Durability::VisibleButSyncFailed(error.to_string()),
    }
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Durability {
    Durability::PlatformBestEffort
}

struct TemporaryFile(Option<PathBuf>);

impl TemporaryFile {
    fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_is_visible_as_one_complete_file() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("state.json");
        fs::write(&target, b"old").expect("old value");
        let outcome = replace_bytes(&target, b"new complete value").expect("replacement");
        assert!(outcome.platform_durable());
        assert_eq!(fs::read(&target).expect("new value"), b"new complete value");
        assert!(
            fs::read_dir(directory.path())
                .expect("directory")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp-"))
        );
    }

    #[test]
    fn directory_sync_failure_is_an_already_visible_outcome() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("state.json");
        let temporary = directory.path().join(".state.json.staged");
        fs::write(&target, b"old").expect("old value");
        fs::write(&temporary, b"new").expect("staged value");

        let outcome = replace_staged_with(&temporary, &target, |_| {
            Durability::VisibleButSyncFailed("injected sync failure".to_string())
        })
        .expect("the replacement itself succeeded");

        assert_eq!(fs::read(&target).expect("visible value"), b"new");
        assert!(!temporary.exists());
        assert!(!outcome.platform_durable());
        assert_eq!(
            outcome.durability,
            Durability::VisibleButSyncFailed("injected sync failure".to_string())
        );
    }

    #[test]
    fn mkdir_parent_sync_failure_survives_a_durable_file_publish() {
        let root = tempfile::tempdir().expect("temp directory");
        let target = root.path().join("private").join("state.json");

        let outcome = replace_bytes_with_directory_sync(&target, b"visible", |_| {
            Durability::VisibleButSyncFailed("injected mkdir sync failure".to_string())
        })
        .expect("the file publish remains visible");

        assert_eq!(fs::read(&target).expect("visible target"), b"visible");
        assert!(!outcome.platform_durable());
        assert_eq!(
            outcome.durability,
            Durability::VisibleButSyncFailed("injected mkdir sync failure".to_string())
        );
    }

    #[test]
    fn an_already_visible_parent_sync_failure_does_not_hide_the_target() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("receipt.json");
        fs::write(&target, b"visible").expect("visible target");

        let outcome = sync_target_parent_with(&target, |parent| {
            assert_eq!(parent, directory.path());
            Durability::VisibleButSyncFailed("injected retry failure".to_string())
        })
        .expect("the already-visible target is not republished");

        assert_eq!(
            fs::read(&target).expect("target remains visible"),
            b"visible"
        );
        assert!(!outcome.platform_durable());
        assert_eq!(
            outcome.durability,
            Durability::VisibleButSyncFailed("injected retry failure".to_string())
        );
    }

    #[test]
    fn fresh_nested_private_directories_reach_the_platform_boundary() {
        let root = tempfile::tempdir().expect("temp directory");
        let first = root.path().join("private");
        let second = first.join("nested");
        let target = second.join("receipts");

        let outcome =
            ensure_private_directory_durable(&target).expect("durable private directories");
        let retry = ensure_private_directory_durable(&target)
            .expect("an existing directory parent can be synchronised again");

        assert!(outcome.platform_durable());
        assert!(retry.platform_durable());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            assert_eq!(outcome.durability, Durability::Durable);
            for directory in [&first, &second, &target] {
                assert_eq!(
                    fs::symlink_metadata(directory)
                        .expect("directory metadata")
                        .permissions()
                        .mode()
                        & 0o777,
                    0o700
                );
            }
        }
    }

    #[test]
    fn private_directory_refuses_a_non_directory_parent() {
        let root = tempfile::tempdir().expect("temp directory");
        let file = root.path().join("not-a-directory");
        fs::write(&file, b"file").expect("blocking file");

        let error = ensure_private_directory_durable(&file.join("child"))
            .expect_err("a file cannot become a directory parent");

        assert!(matches!(
            error.kind(),
            io::ErrorKind::InvalidInput | io::ErrorKind::NotADirectory
        ));
        assert!(!file.join("child").exists());
    }

    #[cfg(unix)]
    #[test]
    fn private_directory_refuses_a_symbolic_link_parent() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp directory");
        let outside = tempfile::tempdir().expect("outside directory");
        let link = root.path().join("linked");
        symlink(outside.path(), &link).expect("directory symlink");

        let error = ensure_private_directory_durable(&link.join("child"))
            .expect_err("a symlink cannot become a trusted directory parent");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!outside.path().join("child").exists());
    }

    #[cfg(unix)]
    #[test]
    fn private_directory_checks_symlinks_before_existing_descendants() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp directory");
        let outside = tempfile::tempdir().expect("outside directory");
        fs::create_dir(outside.path().join("existing")).expect("existing outside descendant");
        let link = root.path().join("linked");
        symlink(outside.path(), &link).expect("directory symlink");

        let escaped_child = outside.path().join("existing").join("child");
        let error = ensure_private_directory_durable(&link.join("existing").join("child"))
            .expect_err("every existing component must be a plain directory");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(!escaped_child.exists());
    }

    #[cfg(unix)]
    #[test]
    fn plain_open_and_private_lock_refuse_a_symbolic_link_leaf() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let root = tempfile::tempdir().expect("temp directory");
        let outside = root.path().join("outside");
        let leaf = root.path().join("lock");
        fs::write(&outside, b"external sentinel").expect("outside value");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o640)).expect("outside mode");
        symlink(&outside, &leaf).expect("file symlink");

        let read_error = read_plain_file(&leaf).expect_err("a plain read cannot follow a link");
        let lock_error = private_lock_file(&leaf).expect_err("a private lock cannot follow a link");

        assert_eq!(read_error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(lock_error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(&outside).expect("outside remains"),
            b"external sentinel"
        );
        assert_eq!(
            fs::metadata(&outside)
                .expect("outside metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_junction_is_not_a_plain_directory() {
        use std::os::windows::fs::MetadataExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        let root = tempfile::tempdir().expect("temp directory");
        let outside = tempfile::tempdir().expect("outside directory");
        fs::create_dir(outside.path().join("existing")).expect("existing outside descendant");
        let sentinel = outside.path().join("sentinel");
        fs::write(&sentinel, b"outside").expect("outside sentinel");
        let junction = root.path().join("junction");
        let command = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
        let output = std::process::Command::new(command)
            .arg("/D")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(outside.path())
            .output()
            .expect("run mklink");
        assert!(
            output.status.success(),
            "mklink failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let metadata = fs::symlink_metadata(&junction).expect("junction metadata");
        assert_ne!(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT, 0);
        assert!(!is_plain_directory(&metadata));

        let escaped_child = outside.path().join("existing").join("child");
        ensure_private_directory_durable(&junction.join("existing").join("child"))
            .expect_err("a junction cannot become a trusted directory parent");
        let lock_error = private_lock_file(&junction).expect_err("a lock cannot follow a junction");
        let read_error = read_plain_file(&junction).expect_err("a read cannot follow a junction");
        assert_eq!(lock_error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(read_error.kind(), io::ErrorKind::InvalidData);
        assert!(!escaped_child.exists());
        assert_eq!(fs::read(sentinel).expect("outside remains"), b"outside");
    }

    #[cfg(unix)]
    #[test]
    fn directories_and_replacements_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("temp directory");
        let private = root.path().join("private");
        let target = private.join("receipt.json");
        replace_bytes(&target, b"{}\n").expect("replacement");
        assert_eq!(
            fs::metadata(&private)
                .expect("directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&target)
                .expect("file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
