//! One serve token per project, surviving restarts.
//!
//! The server is deliberately started **detached** so it outlives the window —
//! which makes the token the only thing standing between "reopen the window,
//! the lanes are still there" and "reopen the window, our own server rejects
//! us". A token minted fresh per process guarantees the second outcome: the
//! serve keeps the first token forever, and every later window presents a new
//! one. So the minted secret is written down, per project, and reused.
//!
//! This is IDE-owned local data, so it lives under the platform-resolved
//! [`AppPaths`] local-data root — never in the harness's home, which `zo` owns
//! (docs/architecture.md, 저장). Legacy installs keep using
//! `~/.zerocode/state/` only until the path-migration receipt is committed.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use crate::generate_token;
use crate::{AppPaths, LegacyAuthorityLease, PathClass, ensure_private_app_dir};

pub const SERVE_TOKEN_PREFIX: &str = "serve-token-";
const SERVE_TOKEN_HASH_LEN: usize = 16;
const STAGED_TOKEN_NONCE_LEN: usize = 32;

#[must_use]
pub fn is_canonical_serve_token_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    name.strip_prefix(SERVE_TOKEN_PREFIX)
        .is_some_and(|hash| is_lower_hex(hash, SERVE_TOKEN_HASH_LEN))
}

#[must_use]
pub fn is_owned_staged_serve_token_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str().and_then(|name| name.strip_prefix('.')) else {
        return false;
    };
    let Some((canonical, staged_suffix)) = name.split_once('.') else {
        return false;
    };
    let Some(nonce) = staged_suffix.strip_suffix(".tmp") else {
        return false;
    };
    is_canonical_serve_token_name(OsStr::new(canonical))
        && is_lower_hex(nonce, STAGED_TOKEN_NONCE_LEN)
}

fn is_lower_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformTokenError {
    #[error("path migration is pending; persistent token writes are paused")]
    MigrationPending,
    #[error("the operating system did not provide the application paths: {0}")]
    PathUnavailable(#[source] io::Error),
    #[error("persistent token storage failed: {0}")]
    Storage(#[source] io::Error),
}

/// Resolve and lock the shared desktop authority before touching a serve
/// token. Re-resolving after the lock is what makes a CLI waiting behind the
/// shell's migration observe the new source receipt rather than write stale
/// HOME state after the authority changed.
pub fn project_token_for_platform(root: &Path) -> Result<String, PlatformTokenError> {
    project_token_for_platform_with(root, || {
        AppPaths::from_platform().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                PlatformTokenError::PathUnavailable(error)
            } else {
                PlatformTokenError::Storage(error)
            }
        })
    })
}

fn project_token_for_platform_with(
    root: &Path,
    mut resolve_paths: impl FnMut() -> Result<AppPaths, PlatformTokenError>,
) -> Result<String, PlatformTokenError> {
    let mut paths = resolve_paths()?;
    if paths.migration_pending() {
        paths = paths.seal_platform_if_source_empty().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                PlatformTokenError::PathUnavailable(error)
            } else {
                PlatformTokenError::Storage(error)
            }
        })?;
    }
    if paths.platform_paths_active() {
        return write_platform_token(&paths, root);
    }
    let Some(legacy) = paths.legacy_state() else {
        return write_platform_token(&paths, root);
    };

    let lease = LegacyAuthorityLease::acquire(legacy).map_err(PlatformTokenError::Storage)?;
    let resolved = resolve_paths()?;
    let token = write_platform_token(&resolved, root);
    drop(lease);
    token
}

fn write_platform_token(paths: &AppPaths, root: &Path) -> Result<String, PlatformTokenError> {
    if paths.migration_pending() {
        return Err(PlatformTokenError::MigrationPending);
    }
    let state_dir = paths
        .writable_root(PathClass::LocalData)
        .map_err(PlatformTokenError::Storage)?;
    project_token(state_dir, root).map_err(PlatformTokenError::Storage)
}

/// The serve token for `root`, minting and persisting one on first use.
///
/// The file is keyed by the same hash that derives the project's port, so the
/// token follows the server it authenticates. Two calls — two processes, two
/// days apart — return the same secret, which is the entire point.
///
/// # Errors
///
/// Any filesystem failure creating the state directory or reading/writing the
/// token file.
pub fn project_token(state_dir: &Path, root: &Path) -> io::Result<String> {
    project_token_before_publish(state_dir, root, || {})
}

fn project_token_before_publish(
    state_dir: &Path,
    root: &Path,
    before_publish: impl FnOnce(),
) -> io::Result<String> {
    project_token_with_sync(state_dir, root, before_publish, &sync_parent)
}

fn project_token_with_sync(
    state_dir: &Path,
    root: &Path,
    before_publish: impl FnOnce(),
    sync_parent: &impl Fn(&Path) -> io::Result<()>,
) -> io::Result<String> {
    project_token_with_sync_and_observer(state_dir, root, before_publish, sync_parent, &|| {})
}

fn project_token_with_sync_and_observer(
    state_dir: &Path,
    root: &Path,
    before_publish: impl FnOnce(),
    sync_parent: &impl Fn(&Path) -> io::Result<()>,
    before_staged_lock: &impl Fn(),
) -> io::Result<String> {
    ensure_private_app_dir(state_dir)?;
    let path = state_dir.join(serve_token_name(root));
    if let Some(existing) = read_secret(&path)? {
        secure_existing_secret(&path)?;
        retire_owned_staged_siblings(&path, sync_parent)?;
        return Ok(existing);
    }
    if wait_for_staged_writer(&path, before_staged_lock)? {
        let existing = read_secret(&path)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "a staged token writer completed without a canonical token",
            )
        })?;
        secure_existing_secret(&path)?;
        retire_owned_staged_siblings(&path, sync_parent)?;
        return Ok(existing);
    }

    before_publish();

    let minted = generate_token();
    match publish_secret(&path, &minted, sync_parent)? {
        Publish::Won => Ok(minted),
        Publish::Lost => match read_secret(&path)? {
            Some(existing) => {
                secure_existing_secret(&path)?;
                retire_owned_staged_siblings(&path, sync_parent)?;
                Ok(existing)
            }
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "a competing serve token disappeared before readback",
            )),
        },
    }
}

fn serve_token_name(root: &Path) -> String {
    format!("{SERVE_TOKEN_PREFIX}{:016x}", root_hash(root))
}

/// FNV-1a of the project root — the same function the port derivation uses,
/// kept bit-for-bit so token file and port always name the same project.
fn root_hash(root: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in root.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Publish {
    Won,
    Lost,
}

fn read_secret(path: &Path) -> io::Result<Option<String>> {
    let Some(mut file) = open_plain_secret(path)? else {
        return Ok(None);
    };
    let mut existing = String::new();
    file.read_to_string(&mut existing)?;
    let existing = existing.trim();
    if existing.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "existing serve token is empty",
        ))
    } else {
        Ok(Some(existing.to_string()))
    }
}

fn open_plain_secret(path: &Path) -> io::Result<Option<File>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    require_plain_secret(path, &before)?;
    let file = File::open(path)?;
    let handle = file.metadata()?;
    require_plain_secret(path, &handle)?;
    let after = fs::symlink_metadata(path)?;
    require_plain_secret(path, &after)?;
    if !same_file(&before, &handle) || !same_file(&handle, &after) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "serve token changed while it was opened",
        ));
    }
    Ok(Some(file))
}

fn plain_secret_file_exists(path: &Path) -> io::Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    require_plain_secret(path, &metadata)?;
    Ok(true)
}

fn require_plain_secret(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    let is_plain = metadata.is_file() && !metadata.file_type().is_symlink();
    #[cfg(windows)]
    let is_plain = {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        is_plain && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
    };
    if is_plain {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("serve token path is not a plain file: {}", path.display()),
        ))
    }
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(_left: &fs::Metadata, _right: &fs::Metadata) -> bool {
    true
}

/// Publish a fully-written file without ever opening the final path for
/// truncation. A hard link is an atomic create-if-absent operation: exactly one
/// concurrent caller wins, and every loser can immediately reread that token.
fn publish_secret(
    path: &Path,
    token: &str,
    sync_parent: &impl Fn(&Path) -> io::Result<()>,
) -> io::Result<Publish> {
    publish_secret_after_link(path, token, sync_parent, |_| {})
}

fn publish_secret_after_link(
    path: &Path,
    token: &str,
    sync_parent: &impl Fn(&Path) -> io::Result<()>,
    after_link: impl FnOnce(&Path),
) -> io::Result<Publish> {
    let staged = write_staged_secret(path, token)?;
    let result = match fs::hard_link(&staged.path, path) {
        Ok(()) => Publish::Won,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Publish::Lost,
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && plain_secret_file_exists(path)? =>
        {
            Publish::Lost
        }
        Err(error) => {
            let _ = fs::remove_file(&staged.path);
            return Err(error);
        }
    };
    after_link(&staged.path);
    // The final name is already visible and contains the synced bytes. Cleanup
    // or directory-sync failure must not make a caller fall back to a different
    // ephemeral identity.
    match fs::remove_file(&staged.path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    sync_parent(path)?;
    Ok(result)
}

fn wait_for_staged_writer(
    canonical_path: &Path,
    before_staged_lock: &impl Fn(),
) -> io::Result<bool> {
    for staged in owned_staged_siblings(canonical_path)? {
        let Some(file) = open_plain_secret(&staged)? else {
            continue;
        };
        before_staged_lock();
        file.lock()?;
        if plain_secret_file_exists(canonical_path)? {
            return Ok(true);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "orphaned staged serve token requires recovery: {}",
                staged.display()
            ),
        ));
    }
    Ok(false)
}

fn retire_owned_staged_siblings(
    canonical_path: &Path,
    sync_parent: &impl Fn(&Path) -> io::Result<()>,
) -> io::Result<()> {
    for staged in owned_staged_siblings(canonical_path)? {
        let Some(file) = open_plain_secret(&staged)? else {
            continue;
        };
        file.lock()?;
        match fs::remove_file(&staged) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    sync_parent(canonical_path)
}

fn owned_staged_siblings(canonical_path: &Path) -> io::Result<Vec<PathBuf>> {
    let parent = canonical_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "token path has no parent directory",
        )
    })?;
    let canonical_name = canonical_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid token file name"))?;
    let sibling_prefix = format!(".{canonical_name}.");

    let mut owned = Vec::new();
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_text) = name.to_str() else {
            continue;
        };
        if !name_text.starts_with(&sibling_prefix) || !is_owned_staged_serve_token_name(&name) {
            continue;
        }
        owned.push(entry.path());
    }
    Ok(owned)
}

/// Stage the token privately and durably in the destination directory. The
/// random suffix is a file identifier, not the serve token itself.
struct StagedSecret {
    path: PathBuf,
    _lock: File,
}

fn write_staged_secret(path: &Path, token: &str) -> io::Result<StagedSecret> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "token path has no parent"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "token path has no file name")
        })?;

    loop {
        let staged = parent.join(format!(".{name}.{}.tmp", generate_token()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = match options.open(&staged) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        file.lock()?;
        if let Err(error) = file
            .write_all(token.as_bytes())
            .and_then(|()| file.sync_all())
        {
            drop(file);
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        return Ok(StagedSecret {
            path: staged,
            _lock: file,
        });
    }
}

fn secure_existing_secret(path: &Path) -> io::Result<()> {
    let Some(file) = open_plain_secret(path)? else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "serve token disappeared before permission repair",
        ));
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.sync_all()?;
    Ok(())
}

fn sync_parent(_path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        if let Some(parent) = _path.parent() {
            File::open(parent)?.sync_all()?;
            if let Some(grandparent) = parent.parent() {
                File::open(grandparent)?.sync_all()?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::{Arc, Barrier};
    use std::thread;

    /// The reason this module exists: the second process — tomorrow's window —
    /// must present the token the server was started with.
    #[test]
    fn the_same_project_gets_the_same_token_across_calls() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/Users/dev/2026/zerocode");
        let first = project_token(dir.path(), root).expect("first");
        let second = project_token(dir.path(), root).expect("second");
        assert_eq!(first, second);
        assert!(!first.is_empty());
    }

    #[test]
    fn different_projects_get_different_tokens() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = project_token(dir.path(), Path::new("/p/a")).expect("a");
        let b = project_token(dir.path(), Path::new("/p/b")).expect("b");
        assert_ne!(a, b);
    }

    #[test]
    fn concurrent_first_calls_publish_one_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_dir = dir.path().to_path_buf();
        let root = PathBuf::from("/p/concurrent");
        let before_publish = Arc::new(Barrier::new(2));

        let spawn = |barrier: Arc<Barrier>| {
            let state_dir = state_dir.clone();
            let root = root.clone();
            thread::spawn(move || {
                project_token_before_publish(&state_dir, &root, || {
                    barrier.wait();
                })
                .expect("token")
            })
        };
        let left = spawn(Arc::clone(&before_publish));
        let right = spawn(before_publish);
        let left = left.join().expect("left thread");
        let right = right.join().expect("right thread");

        assert_eq!(left, right);
        assert_eq!(
            fs::read_to_string(state_dir.join(format!("serve-token-{:016x}", root_hash(&root))))
                .expect("persisted token"),
            left
        );
    }

    #[test]
    fn receipt_only_pending_state_refuses_to_write_a_token() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        fs::create_dir(&legacy).expect("legacy root");
        fs::write(
            legacy.join(crate::PATH_MIGRATION_RECEIPT),
            b"visible receipt",
        )
        .expect("receipt-only state");
        let config = root.path().join("config");
        let local_data = root.path().join("local-data");
        let cache = root.path().join("cache");
        let project = Path::new("project-during-pending");

        let error = project_token_for_platform_with(project, || {
            AppPaths::try_from_resolved(
                config.clone(),
                local_data.clone(),
                cache.clone(),
                Some(legacy.clone()),
            )
            .map_err(PlatformTokenError::Storage)
        })
        .expect_err("pending migration must pause token writes");

        let token_name = format!("serve-token-{:016x}", root_hash(project));
        assert!(matches!(error, PlatformTokenError::MigrationPending));
        assert!(!legacy.join(&token_name).exists());
        assert!(!local_data.join(token_name).exists());
    }

    #[test]
    fn active_platform_never_touches_a_broken_legacy_path() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        fs::create_dir(&legacy).expect("legacy root");
        fs::write(
            legacy.join(crate::PATH_MIGRATION_RECEIPT),
            b"visible receipt",
        )
        .expect("receipt-only state");
        let config = root.path().join("config");
        let local_data = root.path().join("local-data");
        let cache = root.path().join("cache");
        let pending = AppPaths::try_from_resolved(
            config.clone(),
            local_data.clone(),
            cache.clone(),
            Some(legacy.clone()),
        )
        .expect("pending paths");
        let migration = crate::PathMigrationLock::acquire(&legacy).expect("migration lock");
        pending
            .publish_platform_activation(&migration)
            .expect("activation after cleanup");
        drop(migration);
        fs::remove_dir_all(&legacy).expect("remove retired legacy root");
        fs::write(&legacy, b"irrelevant blocker").expect("broken legacy path");
        let resolutions = Cell::new(0);
        let project = Path::new("project-after-activation");

        let token = project_token_for_platform_with(project, || {
            resolutions.set(resolutions.get() + 1);
            AppPaths::try_from_resolved(
                config.clone(),
                local_data.clone(),
                cache.clone(),
                Some(legacy.clone()),
            )
            .map_err(PlatformTokenError::Storage)
        })
        .expect("platform token after activation");

        let token_name = format!("serve-token-{:016x}", root_hash(project));
        assert_eq!(resolutions.get(), 1);
        assert!(!legacy.join(&token_name).exists());
        assert_eq!(
            fs::read_to_string(local_data.join(token_name)).expect("platform token"),
            token
        );
    }

    #[test]
    fn an_existing_valid_token_is_never_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/existing");
        let path = dir
            .path()
            .join(format!("serve-token-{:016x}", root_hash(root)));
        fs::write(&path, "already-established").expect("existing token");

        assert_eq!(
            project_token(dir.path(), root).expect("reuse"),
            "already-established"
        );
        assert_eq!(
            fs::read_to_string(path).expect("persisted"),
            "already-established"
        );
    }

    /// The token authorizes command execution; it must not be group- or
    /// world-readable.
    #[cfg(unix)]
    #[test]
    fn the_token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/secret");
        project_token(dir.path(), root).expect("mint");
        let entry = fs::read_dir(dir.path())
            .expect("read dir")
            .next()
            .expect("one file")
            .expect("entry");
        let mode = entry.metadata().expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {mode:o}");
    }

    #[test]
    fn an_empty_token_file_fails_closed_without_replacing_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/empty");
        let path = dir
            .path()
            .join(format!("serve-token-{:016x}", root_hash(root)));
        fs::write(&path, " \n").expect("empty file");

        let error = project_token(dir.path(), root).expect_err("corrupt identity fails closed");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&path).expect("unchanged bytes"), b" \n");
    }

    #[test]
    fn serve_token_name_matchers_accept_only_owned_exact_grammar() {
        assert!(is_canonical_serve_token_name(OsStr::new(
            "serve-token-0123456789abcdef"
        )));
        assert!(!is_canonical_serve_token_name(OsStr::new(
            "serve-token-0123456789abcdeF"
        )));
        assert!(!is_canonical_serve_token_name(OsStr::new(
            "serve-token-0123456789abcdef0"
        )));
        assert!(is_owned_staged_serve_token_name(OsStr::new(
            ".serve-token-0123456789abcdef.0123456789abcdef0123456789abcdef.tmp"
        )));
        assert!(!is_owned_staged_serve_token_name(OsStr::new(
            ".serve-token-0123456789abcdef.notes.tmp"
        )));
        assert!(!is_owned_staged_serve_token_name(OsStr::new(
            ".unrelated.0123456789abcdef0123456789abcdef.tmp"
        )));
    }

    #[test]
    fn unavailable_home_returns_a_typed_error_without_writing() {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config");
        let local = root.path().join("local");
        let cache = root.path().join("cache");
        let project = Path::new("no-home-project");

        let error = project_token_for_platform_with(project, || {
            AppPaths::try_from_resolved(config.clone(), local.clone(), cache.clone(), None)
                .map_err(PlatformTokenError::Storage)
        })
        .expect_err("missing HOME cannot seal platform authority");

        assert!(matches!(error, PlatformTokenError::PathUnavailable(_)));
        assert!(!config.join(crate::PATH_ACTIVATION_WITNESS).exists());
        assert!(!local.join(serve_token_name(project)).exists());
    }

    #[test]
    fn an_existing_token_retires_only_exact_owned_staged_siblings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/staged-cleanup");
        let canonical = dir.path().join(serve_token_name(root));
        fs::write(&canonical, "established").expect("canonical");
        let staged = dir.path().join(format!(
            ".{}.{}.tmp",
            serve_token_name(root),
            "0123456789abcdef0123456789abcdef"
        ));
        fs::hard_link(&canonical, &staged).expect("crashed staged hardlink");
        let unrelated = dir.path().join(".notes.tmp");
        fs::write(&unrelated, b"keep").expect("unrelated dotfile");

        assert_eq!(
            project_token(dir.path(), root).expect("reuse"),
            "established"
        );
        assert!(!staged.exists());
        assert!(unrelated.is_file());
    }

    #[test]
    fn an_orphaned_staged_token_fails_before_a_new_identity_is_minted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/orphaned-stage");
        let canonical = dir.path().join(serve_token_name(root));
        let staged = dir.path().join(format!(
            ".{}.{}.tmp",
            serve_token_name(root),
            "0123456789abcdef0123456789abcdef"
        ));
        fs::write(&staged, b"ambiguous-identity").expect("orphaned staged token");

        let error = project_token(dir.path(), root).expect_err("orphan fails closed");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!canonical.exists());
        assert_eq!(
            fs::read(&staged).expect("orphan unchanged"),
            b"ambiguous-identity"
        );
    }

    #[test]
    fn a_staged_directory_fails_closed_even_when_the_canonical_token_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/staged-directory");
        fs::write(dir.path().join(serve_token_name(root)), b"established").expect("canonical");
        let staged = dir.path().join(format!(
            ".{}.{}.tmp",
            serve_token_name(root),
            "0123456789abcdef0123456789abcdef"
        ));
        fs::create_dir(&staged).expect("malformed staged directory");

        let error = project_token(dir.path(), root).expect_err("special staged path fails closed");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(staged.is_dir());
    }

    #[test]
    fn a_visible_token_heals_parent_sync_without_minting_a_second_secret() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/sync-retry");
        let sync_calls = Cell::new(0_u8);
        let injected_sync = |_: &Path| {
            if sync_calls.replace(sync_calls.get() + 1) == 0 {
                Err(io::Error::other("injected directory sync failure"))
            } else {
                Ok(())
            }
        };

        project_token_with_sync(dir.path(), root, || {}, &injected_sync)
            .expect_err("visible-but-unsynced publish is not success");
        let persisted = fs::read_to_string(dir.path().join(serve_token_name(root)))
            .expect("visible canonical token");
        let retried = project_token_with_sync(dir.path(), root, || {}, &injected_sync)
            .expect("retry heals parent durability");

        assert_eq!(retried, persisted);
        assert_eq!(sync_calls.get(), 2);
    }

    #[test]
    fn staged_writer_lock_makes_a_concurrent_reader_reuse_the_winner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_dir = dir.path().to_path_buf();
        let root = PathBuf::from("/p/staged-writer-race");
        let canonical = state_dir.join(serve_token_name(&root));
        let staged = write_staged_secret(&canonical, "winner-identity").expect("locked stage");
        let before_lock = Arc::new(Barrier::new(2));
        let reader_barrier = Arc::clone(&before_lock);
        let reader_state = state_dir.clone();
        let reader_root = root.clone();
        let reader = thread::spawn(move || {
            project_token_with_sync_and_observer(
                &reader_state,
                &reader_root,
                || {},
                &sync_parent,
                &|| {
                    reader_barrier.wait();
                },
            )
            .expect("concurrent reader")
        });

        before_lock.wait();
        fs::hard_link(&staged.path, &canonical).expect("publish winner");
        fs::remove_file(&staged.path).expect("retire winner stage");
        drop(staged);

        assert_eq!(reader.join().expect("reader thread"), "winner-identity");
        assert_eq!(
            fs::read_to_string(canonical).expect("canonical"),
            "winner-identity"
        );
    }

    #[test]
    fn publish_tolerates_idempotent_staged_cleanup_after_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("serve-token-0123456789abcdef");

        let outcome = publish_secret_after_link(&path, "winner", &sync_parent, |staged| {
            fs::remove_file(staged).expect("concurrent cleanup");
        })
        .expect("already-retired stage is idempotent");

        assert_eq!(outcome, Publish::Won);
        assert_eq!(fs::read_to_string(path).expect("canonical"), "winner");
    }

    #[test]
    fn a_directory_at_the_canonical_token_path_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/token-directory");
        let path = dir.path().join(serve_token_name(root));
        fs::create_dir(&path).expect("blocking directory");

        let error = project_token(dir.path(), root).expect_err("non-plain canonical path");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(path.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_token_never_reads_or_chmods_its_target() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/token-symlink");
        let outside = dir.path().join("outside-token");
        fs::write(&outside, b"outside-secret").expect("outside token");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).expect("outside mode");
        let path = dir.path().join(serve_token_name(root));
        std::os::unix::fs::symlink(&outside, &path).expect("token symlink");

        let error = project_token(dir.path(), root).expect_err("linked token fails closed");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(&outside).expect("outside bytes"),
            b"outside-secret"
        );
        assert_eq!(
            fs::metadata(&outside)
                .expect("outside metadata")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_state_directory_is_tightened_before_token_reuse() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let root = Path::new("/p/permissive-state-root");
        fs::write(dir.path().join(serve_token_name(root)), b"established").expect("token");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).expect("loose mode");

        assert_eq!(
            project_token(dir.path(), root).expect("reuse"),
            "established"
        );
        assert_eq!(
            fs::metadata(dir.path())
                .expect("state metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
