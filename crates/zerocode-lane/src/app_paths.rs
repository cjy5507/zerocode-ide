//! One desktop path contract shared by the window and the headless CLI.
//!
//! The platform directories are the destination contract. Writes deliberately
//! remain on the legacy root until every artifact has an explicit class and a
//! crash-safe migration; switching only the serve token or only preferences
//! would leave two processes using different authorities.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The Tauri bundle identifier, and therefore the stable directory name.
///
/// One spelling for the whole product: an agent running outside a pane finds
/// this same folder to follow the account the window is signed in to, so the
/// name belongs to the shared vocabulary crate.
pub use zerocode_core::app::IDENTIFIER as APP_IDENTIFIER;

/// The window's settings document, under the config root. Spelled here so the
/// shell that writes it and a CLI that only reads one key out of it
/// (`zerocode vault-lint`, for the saved second-brain vault) cannot drift.
pub const PREFERENCES_FILE: &str = "preferences.json";
pub const PATH_MIGRATION_VERSION: u64 = 1;
pub const PATH_MIGRATION_RECEIPT: &str = ".platform-path-migration-v1.json";
pub const PATH_MIGRATION_LOCK: &str = ".platform-path-migration.lock";
pub const PATH_ACTIVATION_WITNESS: &str = ".platform-path-activation-v1.json";
pub const PATH_ACTIVATION_VERSION: u64 = 1;
/// FNV-1a fingerprint of the ordered, release-owned artifact manifest.
///
/// The receipt is accepted only when its diagnostic artifact list reproduces
/// this value. This gives the shell and headless CLI one typed definition of a
/// complete authority switch instead of two permissive JSON interpretations.
pub const PATH_MIGRATION_MANIFEST_ID: &str = "cfba0f83ced9b9f9";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMigrationReceipt {
    version: u64,
    app_identifier: String,
    manifest_id: String,
    complete: bool,
    completed_artifacts: Vec<String>,
}

impl PathMigrationReceipt {
    #[must_use]
    pub fn completed(completed_artifacts: Vec<String>) -> Self {
        Self {
            version: PATH_MIGRATION_VERSION,
            app_identifier: APP_IDENTIFIER.to_string(),
            manifest_id: artifact_manifest_id(&completed_artifacts),
            complete: true,
            completed_artifacts,
        }
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.version == PATH_MIGRATION_VERSION
            && self.app_identifier == APP_IDENTIFIER
            && self.manifest_id == PATH_MIGRATION_MANIFEST_ID
            && artifact_manifest_id(&self.completed_artifacts) == PATH_MIGRATION_MANIFEST_ID
            && self.complete
    }

    #[must_use]
    pub fn completed_artifacts(&self) -> &[String] {
        &self.completed_artifacts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathClass {
    Config,
    LocalData,
    Cache,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathAuthority {
    Legacy,
    MigrationPending,
    Platform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathActivationWitness {
    version: u64,
    app_identifier: String,
    manifest_id: String,
    authority: PathAuthority,
}

impl PathActivationWitness {
    fn platform() -> Self {
        Self {
            version: PATH_ACTIVATION_VERSION,
            app_identifier: APP_IDENTIFIER.to_string(),
            manifest_id: PATH_MIGRATION_MANIFEST_ID.to_string(),
            authority: PathAuthority::Platform,
        }
    }

    fn is_platform(&self) -> bool {
        self.version == PATH_ACTIVATION_VERSION
            && self.app_identifier == APP_IDENTIFIER
            && !self.manifest_id.is_empty()
            && self.authority == PathAuthority::Platform
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    config: PathBuf,
    local_data: PathBuf,
    cache: PathBuf,
    legacy_state: Option<PathBuf>,
    authority: PathAuthority,
}

impl AppPaths {
    /// Resolve the same base directories Tauri 2 uses on desktop platforms.
    pub fn from_platform() -> io::Result<Self> {
        let legacy_state = legacy_state_root();
        let config = dirs::config_dir()
            .ok_or_else(unknown_path)?
            .join(APP_IDENTIFIER);
        let local_data = dirs::data_local_dir()
            .ok_or_else(unknown_path)?
            .join(APP_IDENTIFIER);
        let cache = dirs::cache_dir()
            .ok_or_else(unknown_path)?
            .join(APP_IDENTIFIER);
        Self::try_from_resolved(config, local_data, cache, legacy_state)
    }

    #[must_use]
    pub fn from_resolved(
        config: PathBuf,
        local_data: PathBuf,
        cache: PathBuf,
        legacy_state: Option<PathBuf>,
    ) -> Self {
        let authority = resolve_authority(&config, legacy_state.as_deref())
            .unwrap_or(PathAuthority::MigrationPending);
        Self {
            config,
            local_data,
            cache,
            legacy_state,
            authority,
        }
    }

    pub fn try_from_resolved(
        config: PathBuf,
        local_data: PathBuf,
        cache: PathBuf,
        legacy_state: Option<PathBuf>,
    ) -> io::Result<Self> {
        let authority = resolve_authority(&config, legacy_state.as_deref())?;
        Ok(Self {
            config,
            local_data,
            cache,
            legacy_state,
            authority,
        })
    }

    #[must_use]
    pub fn target(&self, class: PathClass) -> &Path {
        match class {
            PathClass::Config => &self.config,
            PathClass::LocalData => &self.local_data,
            PathClass::Cache => &self.cache,
        }
    }

    #[must_use]
    pub fn legacy_state(&self) -> Option<&Path> {
        self.legacy_state.as_deref()
    }

    /// Compatibility accessor for callers that cannot return a storage error.
    /// Migration-pending state deliberately panics instead of silently writing
    /// either authority; new I/O paths should use [`Self::writable_root`].
    #[must_use]
    pub fn active_root(&self, class: PathClass) -> &Path {
        self.writable_root(class)
            .expect("migration-pending paths have no writable authority")
    }

    pub fn writable_root(&self, class: PathClass) -> io::Result<&Path> {
        match (self.authority, self.legacy_state.as_deref()) {
            (PathAuthority::Legacy, Some(legacy)) => Ok(legacy),
            (PathAuthority::Platform, _) => Ok(self.target(class)),
            (PathAuthority::MigrationPending, _) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "path migration is pending; neither authority is writable",
            )),
            (PathAuthority::Legacy, None) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy authority has no legacy root",
            )),
        }
    }

    #[must_use]
    pub const fn authority(&self) -> PathAuthority {
        self.authority
    }

    #[must_use]
    pub const fn migration_pending(&self) -> bool {
        matches!(self.authority, PathAuthority::MigrationPending)
    }

    #[must_use]
    pub const fn platform_paths_active(&self) -> bool {
        matches!(self.authority, PathAuthority::Platform)
    }

    /// Materialise every active application root with the platform's private
    /// directory policy before a feature module creates children below it.
    ///
    /// In legacy mode all classes intentionally resolve to one directory; the
    /// repeated idempotent call is preferable to teaching startup a second
    /// copy of the authority rules.
    pub fn ensure_active_roots(&self) -> io::Result<()> {
        for class in [PathClass::Config, PathClass::LocalData, PathClass::Cache] {
            ensure_private_app_dir(self.writable_root(class)?)?;
        }
        Ok(())
    }

    /// Seal a first run only when no legacy inventory exists. This method owns
    /// the exclusive check-and-publish transaction so the shell and CLI cannot
    /// independently decide that an absent source means Platform.
    pub fn seal_platform_if_source_empty(&self) -> io::Result<Self> {
        if self.platform_paths_active() {
            return Ok(self.clone());
        }
        let Some(legacy) = self.legacy_state.as_deref() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "legacy state root is unavailable; an empty source cannot be proven",
            ));
        };

        let lock = PathMigrationLock::acquire(legacy)?;
        let current = self.resolve_again()?;
        if current.platform_paths_active() || !source_can_be_sealed(legacy)? {
            return Ok(current);
        }
        current.publish_platform_activation(&lock)
    }

    /// Publish Platform authority after migration cleanup is durable. The
    /// exclusive lock proves no legacy writer can overlap the activation.
    /// Target is published first so a source witness is never the only durable
    /// proof after this method returns successfully.
    pub fn publish_platform_activation(&self, lock: &PathMigrationLock) -> io::Result<Self> {
        let Some(legacy) = self.legacy_state.as_deref() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a migration activation requires a legacy root",
            ));
        };
        if lock.legacy_root() != legacy {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the migration lock does not own this legacy root",
            ));
        }
        let current = self.resolve_again()?;
        if !current.platform_paths_active() && !current.migration_pending() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "legacy inventory has not reached the activation boundary",
            ));
        }

        current.prepare_platform_targets()?;
        publish_activation_witness(&self.config)?;
        publish_activation_witness(legacy)?;
        self.resolve_again()
    }

    fn prepare_platform_targets(&self) -> io::Result<()> {
        for class in [PathClass::Config, PathClass::LocalData, PathClass::Cache] {
            ensure_private_app_dir(self.target(class))?;
        }
        Ok(())
    }

    fn resolve_again(&self) -> io::Result<Self> {
        Self::try_from_resolved(
            self.config.clone(),
            self.local_data.clone(),
            self.cache.clone(),
            self.legacy_state.clone(),
        )
    }
}

/// Discover the legacy state root from the user's home independently of the
/// platform config, local-data, and cache destinations.
#[must_use]
pub fn legacy_state_root() -> Option<PathBuf> {
    legacy_state_root_from_home(dirs::home_dir())
}

fn legacy_state_root_from_home(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|home| home.join(".zerocode").join("state"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WitnessStatus {
    Missing,
    Invalid,
    Platform,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiptStatus {
    Missing,
    Present,
}

fn resolve_authority(config: &Path, legacy: Option<&Path>) -> io::Result<PathAuthority> {
    let target_witness = activation_witness_status(config)?;
    if target_witness == WitnessStatus::Platform {
        return Ok(PathAuthority::Platform);
    }

    let Some(legacy) = legacy else {
        return Ok(PathAuthority::MigrationPending);
    };
    let source_witness = activation_witness_status(legacy)?;
    if source_witness == WitnessStatus::Platform {
        return Ok(PathAuthority::Platform);
    }
    if target_witness == WitnessStatus::Invalid || source_witness == WitnessStatus::Invalid {
        return Ok(PathAuthority::MigrationPending);
    }
    if receipt_status(legacy)? == ReceiptStatus::Present {
        return Ok(PathAuthority::MigrationPending);
    }
    if source_inventory_empty(legacy)? {
        Ok(PathAuthority::MigrationPending)
    } else {
        Ok(PathAuthority::Legacy)
    }
}

fn activation_witness_status(root: &Path) -> io::Result<WitnessStatus> {
    activation_witness_status_with(root, sync_activation_parent)
}

fn activation_witness_status_with(
    root: &Path,
    sync_parent: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<WitnessStatus> {
    if !plain_app_dir_exists(root)? {
        return Ok(WitnessStatus::Missing);
    }
    let Some(bytes) = read_plain_file(&root.join(PATH_ACTIVATION_WITNESS))? else {
        return Ok(WitnessStatus::Missing);
    };
    match serde_json::from_slice::<PathActivationWitness>(&bytes) {
        Ok(witness) if witness.is_platform() => {
            sync_parent(root)?;
            Ok(WitnessStatus::Platform)
        }
        Ok(_) | Err(_) => Ok(WitnessStatus::Invalid),
    }
}

fn receipt_status(legacy: &Path) -> io::Result<ReceiptStatus> {
    if !plain_app_dir_exists(legacy)? {
        return Ok(ReceiptStatus::Missing);
    }
    read_plain_file(&legacy.join(PATH_MIGRATION_RECEIPT)).map(|receipt| {
        if receipt.is_some() {
            ReceiptStatus::Present
        } else {
            ReceiptStatus::Missing
        }
    })
}

fn source_inventory_empty(legacy: &Path) -> io::Result<bool> {
    if !plain_app_dir_exists(legacy)? {
        return Ok(true);
    }
    match fs::symlink_metadata(legacy) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy state root is not a plain directory",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    }

    for entry in fs::read_dir(legacy)? {
        let entry = entry?;
        if entry.file_name() == PATH_MIGRATION_LOCK {
            require_plain_regular_file(&entry.path(), &fs::symlink_metadata(entry.path())?)?;
        } else {
            return Ok(false);
        }
    }
    Ok(true)
}

fn source_can_be_sealed(legacy: &Path) -> io::Result<bool> {
    Ok(activation_witness_status(legacy)? == WitnessStatus::Missing
        && receipt_status(legacy)? == ReceiptStatus::Missing
        && source_inventory_empty(legacy)?)
}

fn publish_activation_witness(root: &Path) -> io::Result<()> {
    match activation_witness_status(root)? {
        WitnessStatus::Platform => return Ok(()),
        WitnessStatus::Invalid => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "an invalid path activation witness already exists",
            ));
        }
        WitnessStatus::Missing => {}
    }

    ensure_private_app_dir(root)?;
    let path = root.join(PATH_ACTIVATION_WITNESS);
    let bytes = serde_json::to_vec(&PathActivationWitness::platform())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let staged = write_staged_activation_witness(root, &bytes)?;
    let published = match fs::hard_link(&staged, &path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
    };
    let _ = fs::remove_file(&staged);
    if !published && activation_witness_status(root)? != WitnessStatus::Platform {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a competing path activation witness is invalid",
        ));
    }
    sync_activation_parent(root)
}

fn write_staged_activation_witness(root: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    loop {
        let staged = root.join(format!(
            ".{PATH_ACTIVATION_WITNESS}.{}.tmp",
            crate::generate_token()
        ));
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
        if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        return Ok(staged);
    }
}

fn sync_activation_parent(_root: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(_root)?.sync_all()?;
        if let Some(parent) = _root.parent() {
            File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum AuthorityLockMode {
    Shared,
    Exclusive,
}

struct AuthorityLock(File);

impl AuthorityLock {
    fn acquire(legacy_root: &Path, mode: AuthorityLockMode) -> io::Result<Self> {
        let file = open_authority_lock(legacy_root)?;
        match mode {
            AuthorityLockMode::Shared => file.lock_shared()?,
            AuthorityLockMode::Exclusive => file.lock()?,
        }
        Ok(Self(file))
    }

    #[cfg(test)]
    fn try_acquire(legacy_root: &Path, mode: AuthorityLockMode) -> io::Result<Option<Self>> {
        let file = open_authority_lock(legacy_root)?;
        let result = match mode {
            AuthorityLockMode::Shared => file.try_lock_shared(),
            AuthorityLockMode::Exclusive => file.try_lock(),
        };
        match result {
            Ok(()) => Ok(Some(Self(file))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }
}

impl Drop for AuthorityLock {
    fn drop(&mut self) {
        let _ = File::unlock(&self.0);
    }
}

fn open_authority_lock(legacy_root: &Path) -> io::Result<File> {
    ensure_private_app_dir(legacy_root)?;
    let path = legacy_root.join(PATH_MIGRATION_LOCK);
    let mut create = OpenOptions::new();
    create.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        create.mode(0o600);
    }
    let file = match create.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let before = fs::symlink_metadata(&path)?;
            require_plain_regular_file(&path, &before)?;
            let file = OpenOptions::new().read(true).write(true).open(&path)?;
            let handle = file.metadata()?;
            require_plain_regular_file(&path, &handle)?;
            let after = fs::symlink_metadata(&path)?;
            require_plain_regular_file(&path, &after)?;
            if !same_file(&before, &handle) || !same_file(&handle, &after) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "authority lock changed while it was opened",
                ));
            }
            file
        }
        Err(error) => return Err(error),
    };
    let handle = file.metadata()?;
    require_plain_regular_file(&path, &handle)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.sync_all()?;
    let path_metadata = fs::symlink_metadata(&path)?;
    require_plain_regular_file(&path, &path_metadata)?;
    if !same_file(&file.metadata()?, &path_metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "authority lock path no longer names the locked file",
        ));
    }
    sync_activation_parent(legacy_root)?;
    Ok(file)
}

fn require_plain_regular_file(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
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
            format!(
                "authority protocol path is not a plain file: {}",
                path.display()
            ),
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

/// Hold the legacy authority stable for the complete duration of a write.
/// Multiple legacy writers may coexist; the exclusive migration switch waits
/// until every lease has been released.
#[must_use = "hold the lease for the full legacy write"]
pub struct LegacyAuthorityLease {
    _lock: AuthorityLock,
}

impl LegacyAuthorityLease {
    pub fn acquire(legacy_root: &Path) -> io::Result<Self> {
        AuthorityLock::acquire(legacy_root, AuthorityLockMode::Shared)
            .map(|lock| Self { _lock: lock })
    }

    #[cfg(test)]
    fn try_acquire(legacy_root: &Path) -> io::Result<Option<Self>> {
        AuthorityLock::try_acquire(legacy_root, AuthorityLockMode::Shared)
            .map(|lock| lock.map(|lock| Self { _lock: lock }))
    }
}

/// Serialize the authority switch with legacy CLI writes. Callers must resolve
/// [`AppPaths`] again after acquiring this exclusive lock: a receipt may have
/// appeared while they waited.
#[must_use = "hold the lock until the authority switch is complete"]
pub struct PathMigrationLock {
    _lock: AuthorityLock,
    legacy_root: PathBuf,
}

impl PathMigrationLock {
    pub fn acquire(legacy_root: &Path) -> io::Result<Self> {
        AuthorityLock::acquire(legacy_root, AuthorityLockMode::Exclusive).map(|lock| Self {
            _lock: lock,
            legacy_root: legacy_root.to_path_buf(),
        })
    }

    #[cfg(test)]
    fn try_acquire(legacy_root: &Path) -> io::Result<Option<Self>> {
        AuthorityLock::try_acquire(legacy_root, AuthorityLockMode::Exclusive).map(|lock| {
            lock.map(|lock| Self {
                _lock: lock,
                legacy_root: legacy_root.to_path_buf(),
            })
        })
    }

    fn legacy_root(&self) -> &Path {
        &self.legacy_root
    }
}

/// Private-by-construction on Unix; inherited per-user AppData ACL on Windows.
pub fn ensure_private_app_dir(path: &Path) -> io::Result<()> {
    let path = normalized_app_path(path)?;
    walk_plain_app_dir(&path, true)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        File::open(&path)?.sync_all()?;
        File::open(path.parent().expect("guarded application directory parent"))?.sync_all()?;
    }
    Ok(())
}

fn plain_app_dir_exists(path: &Path) -> io::Result<bool> {
    walk_plain_app_dir(&normalized_app_path(path)?, false)
}

fn walk_plain_app_dir(path: &Path, create_missing: bool) -> io::Result<bool> {
    let mut current = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        if matches!(component, Component::ParentDir) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "application directory contains a parent traversal",
            ));
        }
        current.push(component.as_os_str());
        if matches!(
            component,
            Component::CurDir | Component::Prefix(_) | Component::RootDir
        ) {
            continue;
        }
        if !ensure_plain_directory_component(&current, create_missing)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn ensure_plain_directory_component(path: &Path, create_missing: bool) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_plain_directory(path, &metadata).map(|()| true),
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create_missing => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
                    if let Some(parent) = path
                        .parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                    {
                        File::open(parent)?.sync_all()?;
                    }
                }
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(path)?;
                validate_plain_directory(path, &metadata).map(|()| true)
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    }
}

fn normalized_app_path(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "application directory path is empty",
        ));
    }
    let (absolute, is_current_directory) = if path.is_absolute() {
        (path.to_path_buf(), false)
    } else {
        let current = std::env::current_dir()?;
        let absolute = current.join(path);
        (absolute.clone(), absolute == current)
    };
    let normalized = normalize_platform_root_alias(absolute)?;
    if normalized.parent().is_none() || is_current_directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "application directory must be a scoped child path",
        ));
    }
    Ok(normalized)
}

/// Resolve one root-owned namespace alias (notably macOS `/var` and `/tmp`)
/// before enforcing no-follow semantics on the managed path components.
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

fn validate_plain_directory(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    let is_plain = metadata.is_dir() && !metadata.file_type().is_symlink();
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
            format!("application directory is not plain: {}", path.display()),
        ))
    }
}

fn unknown_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "the operating system did not provide an application directory",
    )
}

/// Read the authority-switch receipt using the one contract shared by every
/// binary. Invalid, incomplete, and old-manifest documents all mean legacy.
#[must_use]
pub fn read_complete_path_migration_receipt(legacy_root: &Path) -> Option<PathMigrationReceipt> {
    if !plain_app_dir_exists(legacy_root).ok()? {
        return None;
    }
    let path = legacy_root.join(PATH_MIGRATION_RECEIPT);
    let bytes = read_plain_file(&path).ok()??;
    let receipt = serde_json::from_slice::<PathMigrationReceipt>(&bytes).ok()?;
    receipt.is_complete().then_some(receipt)
}

fn read_plain_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    require_plain_regular_file(path, &metadata)?;
    fs::read(path).map(Some)
}

/// Stable manifest identity used by both the receipt producer and consumer.
#[must_use]
pub fn artifact_manifest_id(completed_artifacts: &[String]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for artifact in completed_artifacts {
        for byte in artifact.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_state_root_depends_only_on_the_injected_home() {
        let home = PathBuf::from("injected-home");

        assert_eq!(
            legacy_state_root_from_home(Some(home.clone())),
            Some(home.join(".zerocode").join("state"))
        );
        assert_eq!(legacy_state_root_from_home(None), None);
    }

    #[test]
    fn shared_legacy_authority_leases_coexist() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        let _first = LegacyAuthorityLease::acquire(&legacy).expect("first shared lease");

        let second = LegacyAuthorityLease::try_acquire(&legacy).expect("try second shared lease");

        assert!(second.is_some());
        let lock_path = legacy.join(PATH_MIGRATION_LOCK);
        assert!(lock_path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(lock_path)
                    .expect("lock metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn exclusive_migration_waits_while_a_shared_legacy_lease_is_held() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        let lease = LegacyAuthorityLease::acquire(&legacy).expect("shared lease");

        assert!(
            PathMigrationLock::try_acquire(&legacy)
                .expect("try exclusive migration lock")
                .is_none()
        );

        drop(lease);
        assert!(
            PathMigrationLock::try_acquire(&legacy)
                .expect("exclusive migration lock after lease")
                .is_some()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_receipt_never_switches_authority() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        fs::create_dir(&legacy).expect("legacy");
        let outside = root.path().join("receipt.json");
        fs::write(&outside, b"{}").expect("outside");
        std::os::unix::fs::symlink(&outside, legacy.join(PATH_MIGRATION_RECEIPT))
            .expect("receipt symlink");

        assert!(read_plain_file(&legacy.join(PATH_MIGRATION_RECEIPT)).is_err());
        assert!(read_complete_path_migration_receipt(&legacy).is_none());
        assert!(
            AppPaths::try_from_resolved(
                root.path().join("config"),
                root.path().join("local"),
                root.path().join("cache"),
                Some(legacy.clone()),
            )
            .is_err()
        );
        let paths = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy),
        );
        assert!(!paths.platform_paths_active());
        assert!(paths.migration_pending());
    }

    #[test]
    fn a_visible_witness_is_not_trusted_until_its_parent_sync_succeeds() {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config");
        ensure_private_app_dir(&config).expect("config root");
        fs::write(
            config.join(PATH_ACTIVATION_WITNESS),
            serde_json::to_vec(&PathActivationWitness::platform()).expect("witness JSON"),
        )
        .expect("visible witness");

        let error = activation_witness_status_with(&config, |_| {
            Err(io::Error::other("injected parent sync failure"))
        })
        .expect_err("visible but unsynced witness must not activate Platform");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(
            activation_witness_status(&config).expect("restart promotes witness durability"),
            WitnessStatus::Platform
        );
    }

    #[test]
    fn an_old_manifest_activation_witness_remains_irreversibly_platform() {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config");
        ensure_private_app_dir(&config).expect("config root");
        let witness = PathActivationWitness {
            version: PATH_ACTIVATION_VERSION,
            app_identifier: APP_IDENTIFIER.to_string(),
            manifest_id: "prior-release-manifest".to_string(),
            authority: PathAuthority::Platform,
        };
        fs::write(
            config.join(PATH_ACTIVATION_WITNESS),
            serde_json::to_vec(&witness).expect("old witness JSON"),
        )
        .expect("old witness");
        sync_activation_parent(&config).expect("durable old witness");
        let legacy = root.path().join("restored-legacy");
        fs::create_dir(&legacy).expect("legacy");
        fs::write(legacy.join("preferences.json"), b"old").expect("old state");

        let paths = AppPaths::try_from_resolved(
            config,
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy),
        )
        .expect("old activation epoch");

        assert!(paths.platform_paths_active());
    }

    #[test]
    fn target_roots_keep_config_local_data_and_cache_separate() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        std::fs::create_dir(&legacy).expect("legacy");
        let paths = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy),
        );
        assert_eq!(paths.target(PathClass::Config), root.path().join("config"));
        assert_eq!(
            paths.target(PathClass::LocalData),
            root.path().join("local")
        );
        assert_eq!(paths.target(PathClass::Cache), root.path().join("cache"));
    }

    #[test]
    fn a_partial_migration_cannot_change_the_active_authority() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        std::fs::create_dir(&legacy).expect("legacy");
        fs::write(legacy.join("preferences.json"), b"legacy").expect("legacy inventory");
        let paths = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy.clone()),
        );
        assert!(!paths.platform_paths_active());
        for class in [PathClass::Config, PathClass::LocalData, PathClass::Cache] {
            assert_eq!(paths.active_root(class), legacy);
        }
    }

    #[test]
    fn no_home_cannot_be_mistaken_for_an_empty_legacy_source() {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config");
        let local = root.path().join("local");
        let cache = root.path().join("cache");
        let paths = AppPaths::try_from_resolved(config.clone(), local.clone(), cache.clone(), None)
            .expect("pending paths");
        assert!(paths.migration_pending());
        assert_eq!(
            paths
                .writable_root(PathClass::LocalData)
                .expect_err("pending has no writable authority")
                .kind(),
            io::ErrorKind::WouldBlock
        );

        let error = paths
            .seal_platform_if_source_empty()
            .expect_err("no HOME cannot prove an empty source");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!config.join(PATH_ACTIVATION_WITNESS).exists());

        let restored_legacy = root.path().join("late-home-state");
        fs::create_dir(&restored_legacy).expect("late legacy root");
        fs::write(restored_legacy.join("preferences.json"), b"old").expect("old backup");
        let restarted = AppPaths::try_from_resolved(config, local, cache, Some(restored_legacy))
            .expect("restart with home");
        assert_eq!(restarted.authority(), PathAuthority::Legacy);
    }

    #[test]
    fn a_new_install_materialises_every_platform_root() {
        let root = tempfile::tempdir().expect("root");
        let paths = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(root.path().join("missing-legacy")),
        );
        let paths = paths
            .seal_platform_if_source_empty()
            .expect("seal new install");
        paths.ensure_active_roots().expect("private roots");
        for class in [PathClass::Config, PathClass::LocalData, PathClass::Cache] {
            assert!(paths.target(class).is_dir());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                assert_eq!(
                    std::fs::metadata(paths.target(class))
                        .expect("root metadata")
                        .permissions()
                        .mode()
                        & 0o777,
                    0o700
                );
            }
        }
    }

    #[test]
    fn a_loose_complete_bit_cannot_move_the_cli_to_platform_paths() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        std::fs::create_dir(&legacy).expect("legacy");
        std::fs::write(
            legacy.join(PATH_MIGRATION_RECEIPT),
            format!(
                r#"{{"version":{PATH_MIGRATION_VERSION},"app_identifier":"{APP_IDENTIFIER}","complete":true}}"#
            ),
        )
        .expect("receipt");
        let paths = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy),
        );
        assert!(!paths.platform_paths_active());
        assert!(paths.migration_pending());
        assert_eq!(
            paths
                .writable_root(PathClass::LocalData)
                .expect_err("receipt-only state pauses writes")
                .kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn a_self_consistent_but_incomplete_artifact_list_is_rejected() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        std::fs::create_dir(&legacy).expect("legacy");
        let receipt = PathMigrationReceipt::completed(vec!["settings-repository".to_string()]);
        std::fs::write(
            legacy.join(PATH_MIGRATION_RECEIPT),
            serde_json::to_vec(&receipt).expect("receipt JSON"),
        )
        .expect("receipt");
        let paths = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy.clone()),
        );
        assert!(!paths.platform_paths_active());
        assert!(paths.migration_pending());
    }

    #[test]
    fn migration_activation_survives_source_deletion_and_old_backup_restore() {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config");
        let local = root.path().join("local");
        let cache = root.path().join("cache");
        let legacy = root.path().join("legacy");
        fs::create_dir(&legacy).expect("legacy");
        fs::write(legacy.join(PATH_MIGRATION_RECEIPT), b"visible receipt")
            .expect("receipt-only pending state");
        let pending = AppPaths::try_from_resolved(
            config.clone(),
            local.clone(),
            cache.clone(),
            Some(legacy.clone()),
        )
        .expect("pending paths");
        assert!(pending.migration_pending());

        let migration = PathMigrationLock::acquire(&legacy).expect("exclusive migration lock");
        let active = pending
            .publish_platform_activation(&migration)
            .expect("publish activation after cleanup");
        assert!(active.platform_paths_active());
        drop(migration);
        assert!(config.join(PATH_ACTIVATION_WITNESS).is_file());
        assert!(legacy.join(PATH_ACTIVATION_WITNESS).is_file());

        fs::remove_dir_all(&legacy).expect("remove completed source");
        fs::create_dir(&legacy).expect("restore legacy root");
        fs::write(legacy.join("preferences.json"), b"old backup").expect("restore old backup");
        let restarted = AppPaths::try_from_resolved(config, local, cache, Some(legacy))
            .expect("restart after restore");
        assert!(restarted.platform_paths_active());
    }

    #[test]
    fn a_source_witness_recovers_a_missing_platform_witness() {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config");
        let legacy = root.path().join("legacy");
        let pending = AppPaths::try_from_resolved(
            config.clone(),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy.clone()),
        )
        .expect("empty source pending");
        let active = pending
            .seal_platform_if_source_empty()
            .expect("seal empty source");
        fs::remove_file(config.join(PATH_ACTIVATION_WITNESS)).expect("remove target witness");

        let recovered = active.resolve_again().expect("source witness recovery");
        assert!(recovered.platform_paths_active());
        fs::write(
            config.join(PATH_ACTIVATION_WITNESS),
            b"corrupt target witness",
        )
        .expect("corrupt target witness");
        assert!(
            active
                .resolve_again()
                .expect("valid source outranks corrupt target")
                .platform_paths_active()
        );
    }

    #[test]
    fn retry_repairs_a_crash_between_platform_and_source_witnesses() {
        let root = tempfile::tempdir().expect("root");
        let config = root.path().join("config");
        let legacy = root.path().join("legacy");
        fs::create_dir(&legacy).expect("legacy");
        fs::write(legacy.join(PATH_MIGRATION_RECEIPT), b"pending receipt").expect("receipt");
        let pending = AppPaths::try_from_resolved(
            config.clone(),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy.clone()),
        )
        .expect("pending paths");
        let migration = PathMigrationLock::acquire(&legacy).expect("migration lock");
        publish_activation_witness(&config).expect("platform-first witness");
        let platform_after_crash = pending.resolve_again().expect("platform witness authority");

        let repaired = platform_after_crash
            .publish_platform_activation(&migration)
            .expect("repair source witness");

        assert!(repaired.platform_paths_active());
        assert!(legacy.join(PATH_ACTIVATION_WITNESS).is_file());
    }

    #[test]
    fn metadata_errors_are_not_treated_as_an_absent_legacy_source() {
        let root = tempfile::tempdir().expect("root");
        let not_a_directory = root.path().join("not-a-directory");
        fs::write(&not_a_directory, b"file").expect("blocking file");
        let legacy = not_a_directory.join("state");

        let error = AppPaths::try_from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy.clone()),
        )
        .expect_err("NotADirectory must propagate");
        assert_ne!(error.kind(), io::ErrorKind::NotFound);

        let fail_closed = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy),
        );
        assert!(fail_closed.migration_pending());
    }

    #[test]
    fn nonempty_legacy_inventory_cannot_be_sealed_as_a_new_install() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        fs::create_dir(&legacy).expect("legacy");
        fs::write(legacy.join("unknown-user-data"), b"keep").expect("legacy data");
        let paths = AppPaths::try_from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy),
        )
        .expect("legacy paths");

        let after_seal_attempt = paths
            .seal_platform_if_source_empty()
            .expect("nonempty source remains legacy");
        assert_eq!(after_seal_attempt.authority(), PathAuthority::Legacy);
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_root_is_never_treated_as_an_application_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let before = fs::metadata("/")
            .expect("root metadata")
            .permissions()
            .mode()
            & 0o777;
        let error = ensure_private_app_dir(Path::new("/"))
            .expect_err("filesystem root is not application-owned");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            fs::metadata("/")
                .expect("root metadata after guard")
                .permissions()
                .mode()
                & 0o777,
            before
        );
    }

    #[test]
    fn trusted_platform_root_aliases_allow_private_children() {
        let root = tempfile::tempdir().expect("root");
        let child = root.path().join("private-child");

        ensure_private_app_dir(&child).expect("trusted root alias is normalized");

        assert!(child.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_root_cannot_forge_platform_authority() {
        let root = tempfile::tempdir().expect("root");
        let outside = root.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        fs::write(
            outside.join(PATH_ACTIVATION_WITNESS),
            serde_json::to_vec(&PathActivationWitness::platform()).expect("witness JSON"),
        )
        .expect("outside witness");
        let config = root.path().join("config-link");
        std::os::unix::fs::symlink(&outside, &config).expect("config symlink");

        let error = AppPaths::try_from_resolved(
            config,
            root.path().join("local"),
            root.path().join("cache"),
            Some(root.path().join("legacy")),
        )
        .expect_err("linked config roots fail closed");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(outside.join(PATH_ACTIVATION_WITNESS).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_authority_lock_cannot_be_opened_or_sealed() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("legacy");
        ensure_private_app_dir(&legacy).expect("legacy");
        let outside = root.path().join("outside-lock-target");
        fs::write(&outside, b"unchanged").expect("outside target");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).expect("outside mode");
        std::os::unix::fs::symlink(&outside, legacy.join(PATH_MIGRATION_LOCK))
            .expect("lock symlink");
        let config = root.path().join("config");
        let paths = AppPaths::from_resolved(
            config.clone(),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy.clone()),
        );

        assert!(PathMigrationLock::acquire(&legacy).is_err());
        assert!(paths.seal_platform_if_source_empty().is_err());
        assert_eq!(fs::read(&outside).expect("outside bytes"), b"unchanged");
        assert_eq!(
            fs::metadata(&outside)
                .expect("outside metadata")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        assert!(!config.join(PATH_ACTIVATION_WITNESS).exists());
    }

    #[test]
    fn target_preflight_failure_publishes_no_activation_witness() {
        let root = tempfile::tempdir().expect("root");
        let legacy = root.path().join("missing-legacy");
        let config = root.path().join("config");
        let cache = root.path().join("cache-blocker");
        fs::write(&cache, b"not a directory").expect("cache blocker");
        let pending = AppPaths::try_from_resolved(
            config.clone(),
            root.path().join("local"),
            cache,
            Some(legacy.clone()),
        )
        .expect("empty source is pending");

        pending
            .seal_platform_if_source_empty()
            .expect_err("all targets must pass before activation");

        assert!(!config.join(PATH_ACTIVATION_WITNESS).exists());
        assert!(!legacy.join(PATH_ACTIVATION_WITNESS).exists());
    }
}
