//! Canonical resolution of zo's per-user state home (`~/.zo`).
//!
//! Single source of truth for the `ZO_CONFIG_HOME` → `ZO_HOME` →
//! `$HOME/.zo` chain, with a read-only legacy `$HOME/.forge` fallback
//! appended last. Before this module, the credential stores, the log path,
//! and the config tools each re-implemented the chain and drifted: all of
//! them silently ignored `ZO_HOME` (splitting user state away from the
//! configured home) and one read the variable UTF-8-only, dropping non-UTF-8
//! paths. `runtime::config` delegates here so every crate — including `api`,
//! which cannot depend on `runtime` — resolves the same directories.

use rand::Rng as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[cfg(windows)]
mod windows_owner_only {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Component, Path, PathBuf};

    use cap_fs_ext::{
        FollowSymlinks, MetadataExt as _, OpenOptionsFollowExt as _,
        OpenOptionsMaybeDirExt as _,
    };
    use cap_std::fs::{Dir, File, OpenOptions, OpenOptionsExt as _};
    use cap_std::ambient_authority;
    use windows_permissions::constants::{
        AccessRights, AceType, SeObjectType, SecurityInformation,
    };
    use windows_permissions::{LocalBox, SecurityDescriptor};
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        READ_CONTROL, WRITE_DAC,
    };

    const PRIVATE_DACL_PREFIX: &str = "D:P";

    fn invalid_path(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, message)
    }

    /// Split an absolute Windows path into its volume root and normal names.
    /// The volume root is the only ambient open; every caller-controlled name
    /// below it is opened relative to retained handles with no-follow.
    fn root_and_names(path: &Path) -> io::Result<(PathBuf, Vec<OsString>)> {
        let absolute = if path.is_absolute() {
            super::normalize_path_components(path)
        } else {
            super::normalize_path_components(&std::env::current_dir()?.join(path))
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
                        "private Windows path must be absolute and stay on one volume",
                    ));
                }
            }
        }
        if !saw_prefix || !saw_root {
            return Err(invalid_path(
                "private Windows path must include a drive or UNC root",
            ));
        }
        Ok((root, names))
    }

    fn open_parent_no_follow(path: &Path) -> io::Result<(Dir, OsString)> {
        use cap_fs_ext::DirExt as _;

        let (root, mut names) = root_and_names(path)?;
        let leaf = names
            .pop()
            .ok_or_else(|| invalid_path("private path must name an entry below its volume"))?;
        let mut dir = Dir::open_ambient_dir(root, ambient_authority())?;
        for name in names {
            // `open_dir_nofollow` maps Windows symlinks and junctions to a
            // refusal instead of letting a reparse point redirect the walk.
            dir = dir.open_dir_nofollow(name)?;
        }
        Ok((dir, leaf))
    }

    fn entry_open_options(write: bool) -> OpenOptions {
        let mut options = OpenOptions::new();
        let data_access = if write { GENERIC_WRITE } else { GENERIC_READ };
        options
            .access_mode(data_access | READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .follow(FollowSymlinks::No)
            .maybe_dir(true);
        options
    }

    pub(super) fn open_entry_no_follow(path: &Path, write: bool) -> io::Result<File> {
        let (parent, leaf) = open_parent_no_follow(path)?;
        let file = parent.open_with(leaf, &entry_open_options(write))?;
        let metadata = file.metadata()?;
        if metadata.is_symlink() {
            return Err(invalid_path(
                "private Windows path must not be a symlink or junction",
            ));
        }
        Ok(file)
    }

    pub(super) fn ensure_dir_no_follow(path: &Path) -> io::Result<()> {
        use cap_fs_ext::DirExt as _;

        let (root, names) = root_and_names(path)?;
        if names.is_empty() {
            return Err(invalid_path("refusing to modify a Windows volume root"));
        }
        let mut dir = Dir::open_ambient_dir(root, ambient_authority())?;
        for name in names {
            dir = match dir.open_dir_nofollow(&name) {
                Ok(child) => child,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    match dir.create_dir(&name) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    // A concurrent junction plant loses this no-follow open;
                    // the walk never continues through the attacker's target.
                    dir.open_dir_nofollow(&name)?
                }
                Err(error) => return Err(error),
            };
        }
        // Apply and verify the protected DACL through the retained leaf handle.
        let mut handle = dir.try_clone()?.into_std_file();
        restrict_handle(&mut handle)
    }

    fn owner_only_descriptor() -> io::Result<LocalBox<SecurityDescriptor>> {
        let owner = windows_permissions::utilities::current_process_sid()?;
        format!("D:P(A;;FA;;;{owner})").parse()
    }

    pub(super) fn restrict_handle<H: AsRawHandle>(handle: &mut H) -> io::Result<()> {
        if !is_handle_current_user_owned(handle)? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "refusing to change the DACL of a Windows entry owned by another SID",
            ));
        }
        let descriptor = owner_only_descriptor()?;
        let dacl = descriptor
            .dacl()
            .ok_or_else(|| invalid_path("owner-only Windows descriptor has no DACL"))?;
        windows_permissions::wrappers::SetSecurityInfo(
            handle,
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
            None,
            None,
            Some(dacl),
            None,
        )?;
        if is_handle_owner_only(handle)? {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Windows DACL did not remain owner-only after it was applied",
            ))
        }
    }

    pub(super) fn is_handle_owner_only<H: AsRawHandle>(handle: &H) -> io::Result<bool> {
        let current = windows_permissions::utilities::current_process_sid()?;
        let descriptor = windows_permissions::wrappers::GetSecurityInfo(
            handle,
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Owner | SecurityInformation::Dacl,
        )?;
        if descriptor.owner() != Some(current.as_ref()) {
            return Ok(false);
        }
        let Some(dacl) = descriptor.dacl() else {
            // A null DACL grants everyone access, the opposite of owner-only.
            return Ok(false);
        };
        if dacl.len() != 1 {
            return Ok(false);
        }
        let Some(ace) = dacl.get_ace(0) else {
            return Ok(false);
        };
        if ace.ace_type() != AceType::ACCESS_ALLOWED_ACE_TYPE
            || !ace.flags().is_empty()
            || ace.mask() != AccessRights::FileAllAccess
            || ace.sid() != Some(current.as_ref())
        {
            return Ok(false);
        }
        let sddl = windows_permissions::wrappers::ConvertSecurityDescriptorToStringSecurityDescriptor(
            &descriptor,
            SecurityInformation::Dacl,
        )?;
        Ok(sddl.to_string_lossy().starts_with(PRIVATE_DACL_PREFIX))
    }

    pub(super) fn is_handle_current_user_owned<H: AsRawHandle>(handle: &H) -> io::Result<bool> {
        let current = windows_permissions::utilities::current_process_sid()?;
        let descriptor = windows_permissions::wrappers::GetSecurityInfo(
            handle,
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Owner,
        )?;
        Ok(descriptor.owner() == Some(current.as_ref()))
    }

    pub(super) fn restrict_path(path: &Path) -> io::Result<()> {
        let mut entry = open_entry_no_follow(path, true)?;
        restrict_handle(&mut entry)
    }

    pub(super) fn is_path_owner_only(path: &Path) -> io::Result<bool> {
        let entry = match open_entry_no_follow(path, false) {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        let metadata = entry.metadata()?;
        if metadata.is_file() && metadata.nlink() != 1 {
            return Ok(false);
        }
        is_handle_owner_only(&entry)
    }

    pub(super) fn open_private_file(
        path: &Path,
        append: bool,
        truncate: bool,
    ) -> io::Result<File> {
        let (parent, leaf) = open_parent_no_follow(path)?;
        let mut options = entry_open_options(true);
        options
            .create(true)
            .append(append)
            .truncate(truncate && !append);
        let mut file = parent.open_with(leaf, &options)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.is_symlink() || metadata.nlink() != 1 {
            return Err(invalid_path(
                "private Windows target must be a singly linked regular file",
            ));
        }
        restrict_handle(&mut file)?;
        Ok(file)
    }

    pub(super) fn open_existing_private_file(path: &Path, append: bool) -> io::Result<File> {
        let (parent, leaf) = open_parent_no_follow(path)?;
        let mut options = entry_open_options(append);
        options.read(!append).append(append);
        let file = parent.open_with(leaf, &options)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.is_symlink()
            || metadata.nlink() != 1
            || !is_handle_owner_only(&file)?
        {
            return Err(invalid_path(
                "private Windows target is not an owner-only singly linked regular file",
            ));
        }
        Ok(file)
    }

    pub(super) fn open_existing_private_file_read_write(path: &Path) -> io::Result<File> {
        let (parent, leaf) = open_parent_no_follow(path)?;
        let mut options = entry_open_options(true);
        options
            .access_mode(
                GENERIC_READ | GENERIC_WRITE | READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
            )
            .read(true)
            .write(true);
        let file = parent.open_with(leaf, &options)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.is_symlink()
            || metadata.nlink() != 1
            || !is_handle_owner_only(&file)?
        {
            return Err(invalid_path(
                "private Windows target is not an owner-only singly linked regular file",
            ));
        }
        Ok(file)
    }

    pub(super) fn handle_matches_path(handle: &std::fs::File, path: &Path) -> io::Result<bool> {
        let named = open_entry_no_follow(path, false)?;
        let held_metadata = cap_std::fs::Metadata::from_file(handle)?;
        let named_metadata = named.metadata()?;
        Ok(
            named_metadata.is_file()
                && !named_metadata.is_symlink()
                && named_metadata.nlink() == 1
                && held_metadata.dev() == named_metadata.dev()
                && held_metadata.ino() == named_metadata.ino(),
        )
    }
}

/// Environment variable naming the highest-priority zo home.
pub const ZO_CONFIG_HOME_ENV: &str = "ZO_CONFIG_HOME";
/// Secondary home override honored after [`ZO_CONFIG_HOME_ENV`].
pub const ZO_HOME_ENV: &str = "ZO_HOME";
/// Directory name of the conventional per-user home under `$HOME`, and of
/// the per-project state directory under a workspace root.
pub const ZO_DIR_NAME: &str = ".zo";

const LEGACY_FORGE_DIR_NAME: &str = ".forge";

/// All per-user global config homes, highest priority first: the canonical
/// `ZO_CONFIG_HOME` → `ZO_HOME` → `~/.zo` chain, de-duplicated. When `HOME` is
/// set and non-empty, the read-only legacy `~/.forge` fallback is appended
/// last; an unset `HOME` contributes neither conventional home.
#[must_use]
pub fn zo_global_config_roots() -> Vec<PathBuf> {
    canonical_config_roots_from(
        std::env::var_os(ZO_CONFIG_HOME_ENV).map(PathBuf::from),
        std::env::var_os(ZO_HOME_ENV).map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn canonical_config_roots_from(
    config_home: Option<PathBuf>,
    zo_home: Option<PathBuf>,
    user_home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let user_home = user_home.filter(|home| !home.as_os_str().is_empty());
    dedupe_paths(
        config_home
            .into_iter()
            .chain(zo_home)
            .chain(user_home.iter().map(|home| home.join(ZO_DIR_NAME)))
            .chain(
                user_home
                    .iter()
                    .map(|home| home.join(LEGACY_FORGE_DIR_NAME)),
            ),
    )
}

fn dedupe_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for path in paths {
        if !path.as_os_str().is_empty() && !roots.iter().any(|existing| existing == &path) {
            roots.push(path);
        }
    }
    roots
}

/// The single canonical write location for user state (sessions,
/// credentials, generated settings): the first entry of
/// [`zo_global_config_roots`]. When no user home can be resolved, use one
/// process-scoped, unpredictably named, owner-only temporary Zo home rather
/// than leaking global state into the current working directory.
#[must_use]
pub fn default_config_home() -> PathBuf {
    zo_global_config_roots()
        .into_iter()
        .next()
        .unwrap_or_else(secure_unresolved_config_home)
}

static UNRESOLVED_CONFIG_HOME: OnceLock<PathBuf> = OnceLock::new();

fn secure_unresolved_config_home() -> PathBuf {
    UNRESOLVED_CONFIG_HOME
        .get_or_init(|| {
            create_secure_unresolved_config_home(&std::env::temp_dir())
                .unwrap_or_else(|_| persistence_disabled_home())
        })
        .clone()
}

fn create_secure_unresolved_config_home(temp_dir: &Path) -> std::io::Result<PathBuf> {
    let base = if temp_dir.is_absolute() {
        temp_dir.to_path_buf()
    } else {
        std::env::current_dir()?.join(temp_dir)
    }
    .canonicalize()?;

    for _ in 0..128 {
        let token = rand::rng().random::<u128>();
        let private_root = base.join(format!("zo-unresolved-home-{token:032x}"));
        match create_owner_only_dir(&private_root) {
            Ok(()) => {
                let home = private_root.join(ZO_DIR_NAME);
                if let Err(error) = create_owner_only_dir(&home) {
                    let _ = std::fs::remove_dir(&private_root);
                    return Err(error);
                }
                return Ok(home);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a private temporary Zo home",
    ))
}

fn create_owner_only_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(not(unix))]
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)?;
    restrict_permissions_owner_only(path)
}

fn persistence_disabled_home() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from(std::path::MAIN_SEPARATOR_STR))
        .join("zo-persistence-disabled")
        .join(ZO_DIR_NAME)
}

/// Environment variable that redirects all of zo's per-project `.zo/*`
/// operational state (todos, turn traces, …) out of the working directory. Set
/// it to a writable directory when the cwd is read-only or must stay clean
/// (a graded benchmark tree, a read-only mount). Unset → state lives under the
/// cwd as before. Distinct from the home chain above, which holds *global*
/// user state (credentials, sessions); this relocates *per-project* state.
pub const ZO_STATE_DIR_ENV: &str = "ZO_STATE_DIR";

/// Base directory under which a workspace's `.zo/` state is read and written:
/// the [`ZO_STATE_DIR_ENV`] override when set (and non-empty), else `cwd`.
/// Callers append `ZO_DIR_NAME`/… as before, so a single override relocates
/// every reader and writer consistently (no per-writer fallback drift).
#[must_use]
pub fn zo_state_base(cwd: &Path) -> PathBuf {
    state_base_from(std::env::var_os(ZO_STATE_DIR_ENV), cwd)
}

/// Env-free core of [`zo_state_base`], so the precedence is unit-testable
/// without mutating process state.
fn state_base_from(override_dir: Option<std::ffi::OsString>, cwd: &Path) -> PathBuf {
    match override_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => cwd.to_path_buf(),
    }
}

/// Restrict a path to owner-only access (`0o700` for directories, `0o600` for
/// files) so other local users cannot read zo's prompts, transcripts, or
/// credentials.
///
/// This is the single source of truth for the permission policy that the
/// credential store, session persistence, and turn-trace writers all share.
/// Windows applies a protected DACL containing exactly one full-control ACE
/// for the current process SID. The handle is opened without following any
/// symlink or junction component and the DACL is read back before success is
/// reported. Platforms where neither property can be established fail closed.
///
/// # Errors
/// Returns the underlying I/O error if the path exists but its permissions
/// cannot be changed.
pub fn restrict_permissions_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};

        let mut options = std::fs::OpenOptions::new();
        options
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if metadata.uid() != nix::unistd::geteuid().as_raw()
            || (!metadata.is_file() && !metadata.is_dir())
            || (metadata.is_file() && metadata.nlink() != 1)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "owner-only target is not a current-user-owned regular file or directory",
            ));
        }
        let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        let after = file.metadata()?;
        if after.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "owner-only Unix mode did not remain restricted after chmod",
            ));
        }
    }
    #[cfg(windows)]
    {
        windows_owner_only::restrict_path(path)?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "owner-only filesystem permissions are unavailable on this platform",
        ));
    }
    Ok(())
}

/// Return whether `path` is a non-symlink entry owned by the current process
/// identity and inaccessible to other users. Regular files must also have a
/// single hard link. Any platform-specific property that cannot be verified is
/// treated as not private rather than silently trusted.
pub fn permissions_are_owner_only(path: &Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if metadata.file_type().is_symlink()
            || metadata.uid() != nix::unistd::geteuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
            || (metadata.is_file() && metadata.nlink() != 1)
        {
            return Ok(false);
        }
        Ok(metadata.is_file() || metadata.is_dir())
    }
    #[cfg(windows)]
    {
        windows_owner_only::is_path_owner_only(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(false)
    }
}

/// Apply the Windows owner-only DACL policy directly to an already retained
/// file or directory handle. This is used by capability-style callers so the
/// ACL update cannot be redirected by replacing a pathname after validation.
#[cfg(windows)]
pub fn restrict_windows_handle_owner_only<H: std::os::windows::io::AsRawHandle>(
    handle: &mut H,
) -> std::io::Result<()> {
    windows_owner_only::restrict_handle(handle)
}

/// Verify the Windows owner-only DACL policy on an already retained handle.
/// `Ok(false)` is the fail-closed result for an owner mismatch, inherited or
/// additional ACE, unprotected DACL, or non-full-control owner ACE.
#[cfg(windows)]
pub fn windows_handle_is_owner_only<H: std::os::windows::io::AsRawHandle>(
    handle: &H,
) -> std::io::Result<bool> {
    windows_owner_only::is_handle_owner_only(handle)
}

/// Verify that an already retained Windows file or directory handle is owned
/// by the current process SID. This is separate from DACL privacy so callers
/// can classify an owner-owned but overly broad entry before tightening it.
#[cfg(windows)]
pub fn windows_handle_is_current_user_owned<H: std::os::windows::io::AsRawHandle>(
    handle: &H,
) -> std::io::Result<bool> {
    windows_owner_only::is_handle_current_user_owned(handle)
}

/// Open an existing owner-only Windows regular file without following any
/// symlink or junction component. `append` selects append-only versus read-only
/// data access; both paths validate the protected current-SID DACL and one-link
/// identity on the retained handle.
#[cfg(windows)]
pub fn open_windows_owner_only_regular_file(
    path: &Path,
    append: bool,
) -> std::io::Result<std::fs::File> {
    windows_owner_only::open_existing_private_file(path, append)
        .map(cap_std::fs::File::into_std)
}

/// Open an existing owner-only Windows regular file for read/write access
/// without following reparse points. Used by lease records that are locked,
/// inspected, and sometimes atomically replaced under the same handle proof.
#[cfg(windows)]
pub fn open_windows_owner_only_regular_file_read_write(
    path: &Path,
) -> std::io::Result<std::fs::File> {
    windows_owner_only::open_existing_private_file_read_write(path)
        .map(cap_std::fs::File::into_std)
}

/// Create a Windows directory chain without following symlinks or junctions
/// and apply the protected current-SID DACL to the leaf directory.
#[cfg(windows)]
pub fn ensure_windows_owner_only_dir_no_follow(path: &Path) -> std::io::Result<()> {
    windows_owner_only::ensure_dir_no_follow(path)
}

/// Compare a retained Windows file handle with a pathname opened no-follow.
/// This closes the unlink/recreate race for advisory lock files.
#[cfg(windows)]
pub fn windows_file_handle_matches_path(
    file: &std::fs::File,
    path: &Path,
) -> std::io::Result<bool> {
    windows_owner_only::handle_matches_path(file, path)
}

/// Symlink-safe, owner-only (`0o600`) write of secret bytes (OAuth refresh
/// tokens, client secrets, ADC credentials) to `path`.
///
/// This writes in place: the permission bits are set at file creation, but the
/// write itself is a plain truncating overwrite — there is no atomic
/// temp-file-plus-rename and no `fsync` durability barrier, so a crash mid-write
/// can leave a truncated file. That is acceptable for these credentials (they
/// are re-fetched on the next run); do not describe it as atomic.
///
/// This is the single source of truth for the "write a private credential file"
/// policy so callers do not each re-derive the symlink/permission handling and
/// drift:
/// - the parent directory is created if missing and restricted to owner-only;
/// - a pre-existing target must be a regular file (never a symlink or special
///   file), so a planted symlink cannot redirect the write to an attacker's
///   target;
/// - on Unix the file is created with mode `0o600` *at creation* and opened
///   with `O_NOFOLLOW | O_NONBLOCK`, closing the TOCTOU window between the
///   pre-check and the open (a symlink is refused, and a swapped FIFO/device
///   cannot block the open); the opened fd is then re-checked to be a regular
///   file, and permissions are re-restricted for the case where the file
///   already existed.
///
/// On non-Unix platforms the symlink/regular-file pre-check still applies and
/// the bytes are written; POSIX permission bits do not exist there.
///
/// # Errors
/// Returns an I/O error if the parent cannot be prepared, the existing target
/// is not a regular file, a symlink is encountered on open, or the write fails.
pub fn write_secret_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    write_private_file(path, contents, &ParentDirPolicy::CreateAndRestrict)
}

/// How [`write_private_file`] treats `path`'s parent directory before writing.
pub enum ParentDirPolicy {
    /// Create the parent (and its ancestors) if missing and restrict it to
    /// owner-only. Correct for a credential file whose whole directory chain the
    /// caller owns (OAuth/ADC under the config home).
    CreateAndRestrict,
    /// Leave the parent directory entirely alone: the caller has already created
    /// and permissioned the leaf directory, and its ancestors may be shared,
    /// pre-existing directories this process does not own (chmod-ing those would
    /// `EPERM`). Used by the prompt cache, whose `ensure_private_dir` restricts
    /// the leaf dirs separately.
    LeaveParent,
}

/// The shared symlink-safe, owner-only (`0o600`) file write behind
/// [`write_secret_file`], parameterized by how the parent directory is handled
/// so the credential writers and the prompt cache reuse one implementation of
/// the symlink-rejection + `O_NOFOLLOW`/`0o600` policy rather than duplicating
/// it. Writes in place (no atomic rename / `fsync`), exactly as
/// [`write_secret_file`] documents.
///
/// # Errors
/// Returns an I/O error if the parent cannot be prepared (when requested), the
/// existing target is not a regular file, a symlink is encountered on open, or
/// the write fails.
pub fn write_private_file(
    path: &Path,
    contents: &[u8],
    parent_policy: &ParentDirPolicy,
) -> std::io::Result<()> {
    if matches!(parent_policy, ParentDirPolicy::CreateAndRestrict) {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            #[cfg(not(windows))]
            std::fs::create_dir_all(parent)?;
            #[cfg(windows)]
            windows_owner_only::ensure_dir_no_follow(parent)?;
            restrict_permissions_owner_only(parent)?;
        }
    }

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            restrict_permissions_owner_only(path)?;
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("private file path is not a regular file: {}", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    #[cfg(windows)]
    {
        let mut file = windows_owner_only::open_private_file(path, false, true)?;
        std::io::Write::write_all(&mut file, contents)
    }

    #[cfg(not(windows))]
    let mut options = std::fs::OpenOptions::new();
    #[cfg(not(windows))]
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // `O_NOFOLLOW` rejects a symlink at `path`; `O_NONBLOCK` ensures the
        // open of a swapped FIFO/device returns instead of blocking on it.
        options.mode(0o600);
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    #[cfg(not(windows))]
    let mut file = options.open(path)?;
    // Re-check the *opened* fd, not the earlier `symlink_metadata`: a FIFO or
    // device swapped in between the pre-check and the open would pass the
    // `is_file` check yet not be a regular file, so reject anything the open
    // actually landed on that is not one.
    #[cfg(not(windows))]
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("private file path is not a regular file: {}", path.display()),
        ));
    }
    #[cfg(not(windows))]
    {
        restrict_permissions_owner_only(path)?;
        std::io::Write::write_all(&mut file, contents)
    }
}

/// Append `contents` to a private file under the same symlink-rejection +
/// `O_NOFOLLOW`/`0o600` policy as [`write_private_file`], creating the file
/// owner-only when absent. Appends in place (`O_APPEND`), no fsync — for
/// best-effort ledger/journal lines whose loss on a crash is acceptable but
/// whose write must never follow a planted symlink, land on a swapped
/// FIFO/device, or leave umask-widened permissions behind.
///
/// # Errors
/// Returns an I/O error if the existing target is not a regular file, a
/// symlink is encountered on open, or the write fails.
pub fn append_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            restrict_permissions_owner_only(path)?;
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("private file path is not a regular file: {}", path.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    #[cfg(windows)]
    {
        let mut file = windows_owner_only::open_private_file(path, true, false)?;
        std::io::Write::write_all(&mut file, contents)
    }

    #[cfg(not(windows))]
    let mut options = std::fs::OpenOptions::new();
    #[cfg(not(windows))]
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // Same rationale as `write_private_file`: `O_NOFOLLOW` rejects a
        // symlink at `path` (closing the pre-check/open TOCTOU window) and
        // `O_NONBLOCK` keeps a swapped FIFO/device from blocking the open;
        // `mode(0o600)` makes the CREATED file owner-only at birth instead of
        // umask-wide-then-chmod.
        options.mode(0o600);
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    #[cfg(not(windows))]
    let mut file = options.open(path)?;
    #[cfg(not(windows))]
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("private file path is not a regular file: {}", path.display()),
        ));
    }
    #[cfg(not(windows))]
    std::io::Write::write_all(&mut file, contents)
}

/// Read an owner-only, singly linked regular file without following the leaf
/// symlink (or any Windows junction/symlink component). This is the read-side
/// counterpart of [`write_private_file`] for credentials and trust stores.
/// Files whose ownership or private access cannot be proven are rejected.
pub fn read_private_file(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;

    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt as _;

        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
            .open(path)?
    };
    #[cfg(windows)]
    let mut file = open_windows_owner_only_regular_file(path, false)?;
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "private file reads are unavailable on this platform",
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != nix::unistd::geteuid().as_raw()
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "private file is not an owner-only singly linked regular file",
            ));
        }
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Lexically normalize a path: drop `.` (current-dir) components and resolve
/// each `..` (parent-dir) by popping the previous component. Performs no
/// filesystem access, so it is purely syntactic.
///
/// This is the single source of truth for the `..`-traversal collapsing that
/// both the config path resolver and the workspace-trust gate rely on; two
/// independent copies of this logic are exactly the divergence risk that
/// matters for trust and path decisions.
#[must_use]
pub fn normalize_path_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn normalize_collapses_dot_and_parent_components() {
        assert_eq!(
            normalize_path_components(Path::new("/a/./b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(
            normalize_path_components(Path::new("a/b/../../d")),
            PathBuf::from("d")
        );
    }

    #[test]
    fn canonical_roots_skip_empty_home_before_appending_zo() {
        assert_eq!(
            canonical_config_roots_from(
                Some(PathBuf::from("/config")),
                Some(PathBuf::new()),
                Some(PathBuf::new()),
            ),
            vec![PathBuf::from("/config")]
        );
        assert!(canonical_config_roots_from(
            Some(PathBuf::new()),
            Some(PathBuf::new()),
            Some(PathBuf::new()),
        )
        .is_empty());
    }

    #[test]
    fn global_roots_append_legacy_forge_last_without_changing_primary() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior = [
            (ZO_CONFIG_HOME_ENV, std::env::var_os(ZO_CONFIG_HOME_ENV)),
            (ZO_HOME_ENV, std::env::var_os(ZO_HOME_ENV)),
            ("HOME", std::env::var_os("HOME")),
        ];
        std::env::remove_var(ZO_CONFIG_HOME_ENV);
        std::env::remove_var(ZO_HOME_ENV);
        let home = std::env::temp_dir().join(format!(
            "zo-global-roots-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::env::set_var("HOME", &home);

        let roots = zo_global_config_roots();
        assert_eq!(
            roots,
            vec![home.join(ZO_DIR_NAME), home.join(LEGACY_FORGE_DIR_NAME)]
        );
        assert_eq!(roots.first(), Some(&home.join(ZO_DIR_NAME)));
        assert_eq!(default_config_home(), home.join(ZO_DIR_NAME));

        std::env::remove_var("HOME");
        assert!(zo_global_config_roots().is_empty());

        for (key, value) in prior {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn no_home_fallback_is_private_absolute_and_process_stable() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let prior = [
            (ZO_CONFIG_HOME_ENV, std::env::var_os(ZO_CONFIG_HOME_ENV)),
            (ZO_HOME_ENV, std::env::var_os(ZO_HOME_ENV)),
            ("HOME", std::env::var_os("HOME")),
        ];
        for (key, _) in &prior {
            std::env::remove_var(key);
        }

        let first = default_config_home();
        let second = default_config_home();

        for (key, value) in prior {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }

        assert!(first.is_absolute());
        assert_eq!(first, second);
        assert_eq!(first.file_name(), Some(std::ffi::OsStr::new(ZO_DIR_NAME)));
        assert!(first.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&first)
                    .expect("temporary Zo home metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(first.parent().expect("private parent"))
                    .expect("private parent metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn state_base_prefers_non_empty_override_else_cwd() {
        let cwd = Path::new("/work/proj");
        assert_eq!(
            state_base_from(Some("/state".into()), cwd),
            PathBuf::from("/state")
        );
        assert_eq!(state_base_from(None, cwd), PathBuf::from("/work/proj"));
        // An empty override must not silently send state to the filesystem root.
        assert_eq!(
            state_base_from(Some("".into()), cwd),
            PathBuf::from("/work/proj")
        );
    }

    // A FIFO swapped in at the target passes the `symlink_metadata` pre-check
    // yet is not a regular file; the post-open re-check must reject it (and
    // `O_NONBLOCK` keeps the open from blocking on the FIFO).
    #[cfg(unix)]
    #[test]
    fn write_private_file_rejects_a_fifo_target() {
        let dir = std::env::temp_dir().join(format!(
            "zo-fifo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("target");
        nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRWXU).expect("mkfifo");

        // A pre-existing FIFO is caught by the `symlink_metadata` pre-check
        // (`AlreadyExists`, "not a regular file"). The post-open `is_file`
        // re-check plus `O_NONBLOCK` are the defense for the harder case — a FIFO
        // swapped in *after* the pre-check — which cannot be raced
        // deterministically here; either layer rejects, and the call never
        // blocks or writes through the FIFO.
        let error = write_private_file(&path, b"secret", &ParentDirPolicy::LeaveParent)
            .expect_err("a FIFO target must be rejected");
        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::InvalidInput
            ) || error.raw_os_error().is_some(),
            "unexpected error kind for FIFO rejection: {error:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
