//! Crash-resumable relocation from the historical HOME directory.
//!
//! Source data is first copied into a private staging tree. Live platform
//! destinations are never touched until every artifact and the Jira native
//! credential preflight have succeeded. A failed attempt discards its staging
//! tree, so later legacy writes can never be hidden behind a stale partial
//! destination. Pre-existing platform files are preserved during promotion.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::app_paths::{
    ARTIFACTS, AppPaths, ArtifactLayout, ArtifactSpec, MigrationKind, SETTINGS_DOCUMENTS,
    legacy_settings_file,
};
use crate::durable_file::{self, Durability};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zerocode_lane::{
    PATH_MIGRATION_RECEIPT, PathClass, PathMigrationLock, PathMigrationReceipt, SERVE_TOKEN_PREFIX,
    is_canonical_serve_token_name, is_owned_staged_serve_token_name,
    read_complete_path_migration_receipt,
};

const STAGING_DIRECTORY: &str = ".platform-path-migration-v1.staging";
const PROMOTION_JOURNAL: &str = ".platform-path-migration-v1.promotions.json";
const PROMOTION_JOURNAL_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PromotionClass {
    Config,
    LocalData,
    Cache,
}

impl PromotionClass {
    fn path_class(self) -> PathClass {
        match self {
            Self::Config => PathClass::Config,
            Self::LocalData => PathClass::LocalData,
            Self::Cache => PathClass::Cache,
        }
    }
}

impl From<PathClass> for PromotionClass {
    fn from(value: PathClass) -> Self {
        match value {
            PathClass::Config => Self::Config,
            PathClass::LocalData => Self::LocalData,
            PathClass::Cache => Self::Cache,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromotionJournalFile {
    version: u8,
    entries: Vec<PromotionJournalEntry>,
    #[serde(default)]
    owned_directories: Vec<PromotionOwnedDirectory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromotionJournalEntry {
    class: PromotionClass,
    relative: PathBuf,
    accepted_sha256: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromotionOwnedDirectory {
    class: PromotionClass,
    relative: PathBuf,
}

#[derive(Default)]
struct PromotionJournal {
    entries: BTreeMap<(PromotionClass, PathBuf), Vec<String>>,
    owned_directories: BTreeSet<(PromotionClass, PathBuf)>,
}

/// Copy every known artifact and switch this process only after the source
/// receipt is visible. On any pre-receipt failure the caller retains the
/// original `AppPaths`, so every runtime write remains on legacy.
pub(crate) fn migrate_if_needed(paths: &AppPaths) -> io::Result<AppPaths> {
    migrate_with_jira_preflight(paths, |source| {
        crate::jira_store::JiraStore::new(source)
            .prepare_path_migration()
            .map_err(io::Error::other)
    })
}

fn migrate_with_jira_preflight(
    paths: &AppPaths,
    jira_preflight: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<AppPaths> {
    migrate_with_hooks(paths, jira_preflight, || Ok(()))
}

fn migrate_with_hooks(
    paths: &AppPaths,
    jira_preflight: impl FnOnce(&Path) -> io::Result<()>,
    mut after_visible_promotion: impl FnMut() -> io::Result<()>,
) -> io::Result<AppPaths> {
    // A durable Platform witness is irreversible authority. Never rediscover,
    // lock, clean, or otherwise interpret a legacy path after this boundary;
    // it may now belong to unrelated/restored data.
    if paths.platform_paths_active() {
        return Ok(paths.clone());
    }
    let source = paths.legacy_state().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "legacy state root is unavailable")
    })?;
    durable_file::ensure_private_directory(source)?;
    let lock = PathMigrationLock::acquire(source)?;
    let current = resolve_migration_paths(paths, source)?;
    if current.platform_paths_active() {
        return Ok(current);
    }

    // A failed attempt can leave private plaintext copies in the exact
    // migration staging tree. Retire them while the source authority is
    // exclusively locked, before Jira or any other fallible preflight can
    // keep a Legacy session alive. The durable helper also covers a deletion
    // that became visible immediately before a prior crash.
    let staging = source.join(STAGING_DIRECTORY);
    remove_private_staging(&staging)?;

    // Another app instance may have completed while this one waited.
    if complete_receipt(source).is_some() {
        let durable = receipt_parent_is_durable(source);
        return activate_after_receipt(&current, source, durable, &lock);
    }

    // Recover the old Jira transaction state and promote every plaintext
    // credential before copying metadata. Native credential keys do not embed
    // this path; token files and journals therefore never cross directories.
    jira_preflight(source)?;

    let staged = StagedPaths::new(&staging)?;

    let staged_result = (|| {
        for artifact in ARTIFACTS {
            migrate_artifact(source, &staged, &current, artifact)?;
        }
        promote_staged(&current, source, &staged, &mut after_visible_promotion)?;
        Ok(())
    })();
    if let Err(error) = staged_result {
        if let Err(cleanup) = remove_private_staging(&staging) {
            return Err(io::Error::new(
                cleanup.kind(),
                format!("migration staging cleanup failed after {error}: {cleanup}"),
            ));
        }
        return Err(error);
    }
    remove_private_staging(&staging)?;

    let completed_artifacts = crate::app_paths::artifact_manifest_entries();
    let receipt = PathMigrationReceipt::completed(completed_artifacts);
    if !receipt.is_complete() {
        return Err(io::Error::other(
            "path migration artifact manifest does not match the shared receipt contract",
        ));
    }
    let mut bytes = serde_json::to_vec_pretty(&receipt).map_err(io::Error::other)?;
    bytes.push(b'\n');
    let outcome = durable_file::replace_bytes(&source.join(PATH_MIGRATION_RECEIPT), &bytes)?;
    let durable_receipt = outcome.platform_durable();
    if let Durability::VisibleButSyncFailed(error) = &outcome.durability {
        eprintln!(
            "zerocode-shell: migration receipt is visible but directory sync failed: {error}"
        );
    }
    // Visibility has already changed the authority observed by a fresh CLI.
    // Cleanup can therefore never turn this successful switch back into a
    // Legacy result. Credentials are retired only after a durable receipt;
    // otherwise a power loss could erase the receipt and its only source.
    activate_after_receipt(&current, source, durable_receipt, &lock)
}

fn resolve_migration_paths(paths: &AppPaths, source: &Path) -> io::Result<AppPaths> {
    AppPaths::try_from_resolved(
        paths.target(PathClass::Config).to_path_buf(),
        paths.target(PathClass::LocalData).to_path_buf(),
        paths.target(PathClass::Cache).to_path_buf(),
        Some(source.to_path_buf()),
    )
}

fn activate_after_receipt(
    paths: &AppPaths,
    source: &Path,
    receipt_is_durable: bool,
    lock: &PathMigrationLock,
) -> io::Result<AppPaths> {
    if !receipt_is_durable {
        return Err(io::Error::other(
            "path migration is pending: the source receipt is visible but not platform-durable",
        ));
    }
    retire_legacy_secrets(source).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("path migration is pending until legacy secrets are durably retired: {error}"),
        )
    })?;
    let pending = resolve_migration_paths(paths, source)?;
    let activated = pending.publish_platform_activation(lock).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("path migration activation witness was not published: {error}"),
        )
    })?;
    finish_visible_migration(source);
    Ok(activated)
}

fn finish_visible_migration(source: &Path) {
    for path in [
        source.join(PROMOTION_JOURNAL),
        source.join(STAGING_DIRECTORY),
    ] {
        let result = match fs::symlink_metadata(&path) {
            Ok(metadata) if durable_file::is_plain_directory(&metadata) => {
                fs::remove_dir_all(&path)
            }
            Ok(_) => fs::remove_file(&path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            eprintln!(
                "zerocode-shell: migration cleanup will retry for {}: {error}",
                path.display()
            );
        }
    }
}

fn receipt_parent_is_durable(source: &Path) -> bool {
    match durable_file::sync_visible_parent(&source.join(PATH_MIGRATION_RECEIPT)) {
        Ok(outcome) if outcome.platform_durable() => true,
        Ok(_) => {
            eprintln!("zerocode-shell: migration receipt directory sync will retry");
            false
        }
        Err(error) => {
            eprintln!("zerocode-shell: migration receipt durability check failed: {error}");
            false
        }
    }
}

struct StagedPaths {
    config: PathBuf,
    local_data: PathBuf,
    cache: PathBuf,
}

impl StagedPaths {
    fn new(root: &Path) -> io::Result<Self> {
        let staged = Self {
            config: root.join("config"),
            local_data: root.join("local-data"),
            cache: root.join("cache"),
        };
        for directory in [&staged.config, &staged.local_data, &staged.cache] {
            durable_file::ensure_private_directory(directory)?;
        }
        Ok(staged)
    }

    fn root(&self, class: PathClass) -> &Path {
        match class {
            PathClass::Config => &self.config,
            PathClass::LocalData => &self.local_data,
            PathClass::Cache => &self.cache,
        }
    }
}

fn migrate_artifact(
    source: &Path,
    staged: &StagedPaths,
    paths: &AppPaths,
    artifact: &ArtifactSpec,
) -> io::Result<()> {
    let destination = staged.root(artifact.class);
    match artifact.layout {
        ArtifactLayout::File(relative) => {
            copy_optional_file(&source.join(relative), &destination.join(relative))
        }
        ArtifactLayout::Directory(relative) => {
            if artifact.migration == MigrationKind::Recreate {
                Ok(())
            } else {
                copy_optional_directory(&source.join(relative), &destination.join(relative))
            }
        }
        ArtifactLayout::Prefix(prefix) => copy_prefixed(source, destination, prefix),
        ArtifactLayout::SettingsRepository => migrate_settings_repository(source, destination),
        ArtifactLayout::JiraRepository => migrate_jira_repository(source, destination),
        ArtifactLayout::AccountRepositories => migrate_account_repositories(source, staged, paths),
    }
}

fn migrate_settings_repository(source: &Path, destination: &Path) -> io::Result<()> {
    for name in SETTINGS_DOCUMENTS
        .iter()
        .chain(legacy_settings_file::ALL.iter())
    {
        copy_optional_file(&source.join(name), &destination.join(name))?;
    }

    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let belongs = SETTINGS_DOCUMENTS
            .iter()
            .any(|document| crate::settings::is_repository_recovery_file(document, &name));
        if belongs {
            copy_optional_file(&entry.path(), &destination.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn migrate_jira_repository(source: &Path, destination: &Path) -> io::Result<()> {
    // Only canonical metadata crosses roots. Legacy token files and transaction
    // journals are consumed in-place by the Jira preflight above.
    copy_optional_file(
        &source.join(crate::jira_store::SITE_FILE_NAME),
        &destination.join(crate::jira_store::SITE_FILE_NAME),
    )
}

#[derive(Clone, Copy)]
enum ManagedAccountLayout {
    Claude,
    Codex,
}

impl ManagedAccountLayout {
    fn index_file(self) -> &'static str {
        match self {
            Self::Claude => crate::accounts::ACCOUNT_STORE_FILE,
            Self::Codex => crate::codex_accounts::ACCOUNT_STORE_FILE,
        }
    }

    fn path_field(self) -> &'static str {
        match self {
            Self::Claude => crate::app_paths::CLAUDE_ACCOUNT_PATH_FIELD,
            Self::Codex => crate::app_paths::CODEX_ACCOUNT_PATH_FIELD,
        }
    }

    fn managed_path(self, local_data_root: &Path, id: &str) -> Option<PathBuf> {
        match self {
            Self::Claude => crate::accounts::account_dir(local_data_root, id),
            Self::Codex => crate::codex_accounts::account_home(local_data_root, id),
        }
    }

    fn managed_root(self, local_data_root: &Path) -> PathBuf {
        match self {
            Self::Claude => local_data_root.join(crate::accounts::MANAGED_ACCOUNTS_DIR),
            Self::Codex => local_data_root.join(crate::codex_accounts::MANAGED_ACCOUNTS_DIR),
        }
    }

    fn owned_directory(self, managed_path: &Path) -> io::Result<&Path> {
        match self {
            Self::Claude => Ok(managed_path),
            Self::Codex => managed_path.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid managed Codex home: {}", managed_path.display()),
                )
            }),
        }
    }
}

/// Stage the two account indexes together with only the homes whose stored
/// paths prove they are app-owned. External/system homes are metadata, not
/// migration sources, and remain byte-for-byte unchanged in the JSON value.
fn migrate_account_repositories(
    source: &Path,
    staged: &StagedPaths,
    paths: &AppPaths,
) -> io::Result<()> {
    for layout in [ManagedAccountLayout::Claude, ManagedAccountLayout::Codex] {
        migrate_account_repository(source, staged, paths, layout)?;
    }
    Ok(())
}

fn migrate_account_repository(
    source: &Path,
    staged: &StagedPaths,
    paths: &AppPaths,
    layout: ManagedAccountLayout,
) -> io::Result<()> {
    let source_index = source.join(layout.index_file());
    let bytes = match read_plain_file(&source_index) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            validate_managed_account_inventory(&layout.managed_root(source), &BTreeSet::new())?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid account index {}: {error}", source_index.display()),
        )
    })?;
    let accounts = match document.get_mut("accounts") {
        None => &mut [],
        Some(serde_json::Value::Array(accounts)) => accounts.as_mut_slice(),
        Some(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "account index has a non-array accounts field: {}",
                    source_index.display()
                ),
            ));
        }
    };

    let mut referenced_owned = BTreeSet::new();
    for account in accounts {
        let object = account.as_object_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "account index row is not an object: {}",
                    source_index.display()
                ),
            )
        })?;
        let id = object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid_account_row(&source_index, "id"))?;
        let stored = object
            .get(layout.path_field())
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid_account_row(&source_index, layout.path_field()))?;
        let stored = Path::new(stored);
        if !stored.is_absolute() {
            return Err(invalid_account_row(&source_index, layout.path_field()));
        }
        let source_managed = layout.managed_path(source, id);
        if !source_managed
            .as_deref()
            .is_some_and(|expected| paths_name_the_same_location(stored, expected))
        {
            if path_is_within(stored, &layout.managed_root(source)) {
                return Err(invalid_account_row(&source_index, layout.path_field()));
            }
            continue;
        }
        let source_managed = source_managed.expect("the exact managed path was compared above");

        let target_managed = layout
            .managed_path(paths.target(PathClass::LocalData), id)
            .ok_or_else(|| invalid_account_row(&source_index, "id"))?;
        let staged_managed = layout
            .managed_path(staged.root(PathClass::LocalData), id)
            .ok_or_else(|| invalid_account_row(&source_index, "id"))?;
        let source_owned = layout.owned_directory(&source_managed)?;
        let staged_owned = layout.owned_directory(&staged_managed)?;
        referenced_owned.insert(source_owned.to_path_buf());
        copy_optional_directory(source_owned, staged_owned)?;
        object.insert(
            layout.path_field().to_string(),
            serde_json::Value::String(target_managed.to_string_lossy().into_owned()),
        );
    }
    validate_managed_account_inventory(&layout.managed_root(source), &referenced_owned)?;

    let mut transformed = serde_json::to_vec_pretty(&document).map_err(io::Error::other)?;
    transformed.push(b'\n');
    let staged_index = staged.root(PathClass::Config).join(layout.index_file());
    let outcome = durable_file::replace_bytes(&staged_index, &transformed)?;
    if !outcome.platform_durable() || fs::read(&staged_index)? != transformed {
        return Err(io::Error::other(format!(
            "account migration staging readback failed: {}",
            staged_index.display()
        )));
    }
    Ok(())
}

fn validate_managed_account_inventory(
    managed_root: &Path,
    referenced: &BTreeSet<PathBuf>,
) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(managed_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !durable_file::is_plain_directory(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "managed account root is not a plain directory: {}",
                managed_root.display()
            ),
        ));
    }
    for entry in fs::read_dir(managed_root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let known = referenced
            .iter()
            .any(|expected| paths_name_the_same_location(&path, expected));
        if !durable_file::is_plain_directory(&metadata) || !known {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unindexed or invalid managed account directory: {}",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn paths_name_the_same_location(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        let mut left = left.components();
        let mut right = right.components();
        loop {
            match (left.next(), right.next()) {
                (None, None) => return true,
                (Some(left), Some(right))
                    if left
                        .as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy()) => {}
                _ => return false,
            }
        }
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let mut path = path.components();
        root.components().all(|expected| {
            path.next().is_some_and(|actual| {
                actual
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
            })
        })
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

fn invalid_account_row(index: &Path, field: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("account index {} has an invalid {field}", index.display()),
    )
}

#[derive(Default)]
struct LegacySecretInventory {
    files: Vec<PathBuf>,
    directories: Vec<PathBuf>,
}

#[derive(Default)]
struct ServeTokenInventory {
    canonical: Vec<PathBuf>,
    staged: Vec<PathBuf>,
}

/// Durably retire every plaintext secret left in the legacy authority.
///
/// Discovery validates the complete inventory before the first unlink. The
/// deletion pass is bottom-up and idempotent; a retry also syncs the legacy
/// root, which makes an unlink from a previously interrupted attempt durable
/// even when the removed name can no longer be rediscovered.
fn retire_legacy_secrets(source: &Path) -> io::Result<()> {
    retire_legacy_secrets_with(source, |_| Ok(()))
}

fn retire_legacy_secrets_with(
    source: &Path,
    mut after_visible_unlink: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let inventory = discover_legacy_secrets(source)?;
    for file in &inventory.files {
        remove_plain_secret_file(file)?;
        after_visible_unlink(file)?;
    }
    for directory in &inventory.directories {
        remove_plain_secret_directory(directory)?;
        after_visible_unlink(directory)?;
    }
    require_platform_durability(
        source,
        durable_file::sync_visible_parent(&source.join(PATH_MIGRATION_RECEIPT))?,
        "legacy secret retirement",
    )
}

fn discover_legacy_secrets(source: &Path) -> io::Result<LegacySecretInventory> {
    let source_metadata = fs::symlink_metadata(source)?;
    if !durable_file::is_plain_directory(&source_metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "legacy secret root is not a plain directory: {}",
                source.display()
            ),
        ));
    }

    let tokens = discover_serve_token_inventory(source)?;
    let mut inventory = LegacySecretInventory::default();
    // Staging links must disappear before their canonical name. If a crash
    // interrupts retirement, every remaining staged bearer still has the
    // canonical counterpart needed for a safe retry.
    inventory.files.extend(tokens.staged);
    inventory.files.extend(tokens.canonical);

    for root in [
        source.join(crate::accounts::MANAGED_ACCOUNTS_DIR),
        source.join(crate::codex_accounts::MANAGED_ACCOUNTS_DIR),
    ] {
        collect_plain_secret_tree(&root, &mut inventory)?;
    }
    Ok(inventory)
}

fn discover_serve_token_inventory(source: &Path) -> io::Result<ServeTokenInventory> {
    let mut inventory = ServeTokenInventory::default();
    let mut canonical_names = BTreeSet::new();
    let mut staged = Vec::new();
    let mut entries = fs::read_dir(source)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        if is_canonical_serve_token_name(&name) {
            require_plain_secret_file(&entry.path())?;
            canonical_names.insert(name);
            inventory.canonical.push(entry.path());
        } else if is_owned_staged_serve_token_name(&name) {
            require_plain_secret_file(&entry.path())?;
            let canonical = canonical_name_for_owned_stage(&name).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "invalid owned staged serve token: {}",
                        entry.path().display()
                    ),
                )
            })?;
            staged.push((entry.path(), canonical));
        }
    }

    for (path, canonical) in staged {
        if !canonical_names.contains(&canonical) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "owned staged serve token has no canonical counterpart: {}",
                    path.display()
                ),
            ));
        }
        inventory.staged.push(path);
    }
    Ok(inventory)
}

fn canonical_name_for_owned_stage(name: &OsStr) -> Option<OsString> {
    if !is_owned_staged_serve_token_name(name) {
        return None;
    }
    let name = name.to_str()?.strip_prefix('.')?;
    let (canonical, _) = name.split_once('.')?;
    let canonical = OsString::from(canonical);
    is_canonical_serve_token_name(&canonical).then_some(canonical)
}

fn collect_plain_secret_tree(root: &Path, inventory: &mut LegacySecretInventory) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !durable_file::is_plain_directory(&metadata) {
        return Err(invalid_secret_entry(root));
    }
    let mut entries = fs::read_dir(root)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if durable_file::is_plain_file(&metadata) {
            inventory.files.push(path);
        } else if durable_file::is_plain_directory(&metadata) {
            collect_plain_secret_tree(&path, inventory)?;
        } else {
            return Err(invalid_secret_entry(&path));
        }
    }
    inventory.directories.push(root.to_path_buf());
    Ok(())
}

fn require_plain_secret_file(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !durable_file::is_plain_file(&metadata) {
        Err(invalid_secret_entry(path))
    } else {
        Ok(())
    }
}

fn remove_plain_secret_file(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if durable_file::is_plain_file(&metadata) => require_platform_durability(
            path,
            durable_file::remove_file(path)?,
            "legacy secret unlink",
        ),
        Ok(_) => Err(invalid_secret_entry(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_plain_secret_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if durable_file::is_plain_directory(&metadata) => require_platform_durability(
            path,
            durable_file::remove_directory(path)?,
            "legacy secret directory unlink",
        ),
        Ok(_) => Err(invalid_secret_entry(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn invalid_secret_entry(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "legacy secret entry is not a plain file or directory: {}",
            path.display()
        ),
    )
}

fn copy_prefixed(source: &Path, destination: &Path, prefix: &str) -> io::Result<()> {
    if prefix != SERVE_TOKEN_PREFIX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported credential prefix in migration manifest: {prefix}"),
        ));
    }
    let inventory = match discover_serve_token_inventory(source) {
        Ok(inventory) => inventory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for canonical in inventory.canonical {
        let name = canonical.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "canonical token has no file name",
            )
        })?;
        copy_required_plain_file(&canonical, &destination.join(name))?;
    }
    // Owned staged names are deliberately omitted. Their canonical presence
    // was proven above; post-receipt retirement removes them from source.
    Ok(())
}

fn copy_required_plain_file(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if !durable_file::is_plain_file(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("migration source is not a plain file: {}", source.display()),
        ));
    }
    copy_plain_file(source, destination, &metadata)
}

fn copy_optional_file(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !durable_file::is_plain_file(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("migration source is not a plain file: {}", source.display()),
        ));
    }
    copy_plain_file(source, destination, &metadata)
}

fn copy_optional_directory(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !durable_file::is_plain_directory(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "migration source is not a plain directory: {}",
                source.display()
            ),
        ));
    }
    durable_file::ensure_private_directory(destination)?;
    let mut entries = fs::read_dir(source)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)?;
        if durable_file::is_plain_directory(&metadata) {
            copy_optional_directory(&path, &target)?;
        } else if durable_file::is_plain_file(&metadata) {
            copy_plain_file(&path, &target, &metadata)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported migration source: {}", path.display()),
            ));
        }
    }
    Ok(())
}

fn read_plain_file(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !durable_file::is_plain_file(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("migration source is not a plain file: {}", path.display()),
        ));
    }
    fs::read(path)
}

fn copy_plain_file(source: &Path, destination: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    let bytes = fs::read(source)?;
    let outcome = durable_file::replace_bytes(destination, &bytes)?;
    if !outcome.platform_durable() {
        return Err(io::Error::other(format!(
            "migration target durability is unknown: {}",
            destination.display()
        )));
    }
    if fs::read(destination)? != bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("migration readback differed: {}", destination.display()),
        ));
    }
    #[cfg(unix)]
    preserve_private_executable_bit(metadata, destination)?;
    Ok(())
}

fn promote_staged(
    paths: &AppPaths,
    source: &Path,
    staged: &StagedPaths,
    after_visible: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let mut journal = load_promotion_journal(source)?;
    let mut staged_files = BTreeSet::new();
    let mut staged_directories = BTreeSet::new();
    for class in [PathClass::Config, PathClass::LocalData, PathClass::Cache] {
        collect_staged_entries(
            staged.root(class),
            Path::new(""),
            class,
            &mut staged_files,
            &mut staged_directories,
        )?;
    }
    for class in [PathClass::Config, PathClass::LocalData, PathClass::Cache] {
        ensure_plain_target_root(paths.target(class))?;
    }
    remove_stale_owned_entries(
        paths,
        source,
        &mut journal,
        &staged_files,
        &staged_directories,
    )?;
    for class in [PathClass::Config, PathClass::LocalData, PathClass::Cache] {
        let target = paths.target(class);
        promote_directory(
            staged.root(class),
            target,
            Path::new(""),
            class,
            source,
            &mut journal,
            after_visible,
        )?;
    }
    Ok(())
}

fn promote_directory(
    staged: &Path,
    target: &Path,
    relative: &Path,
    class: PathClass,
    source: &Path,
    journal: &mut PromotionJournal,
    after_visible: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let mut entries = fs::read_dir(staged)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let staged_entry = entry.path();
        let target_entry = target.join(entry.file_name());
        let relative_entry = relative.join(entry.file_name());
        validate_relative_path(&relative_entry)?;
        let metadata = fs::symlink_metadata(&staged_entry)?;
        if durable_file::is_plain_directory(&metadata) {
            let directory_key = (PromotionClass::from(class), relative_entry.clone());
            match fs::symlink_metadata(&target_entry) {
                Ok(target_metadata) if !durable_file::is_plain_directory(&target_metadata) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "migration destination is not a plain directory: {}",
                            target_entry.display()
                        ),
                    ));
                }
                Ok(_) => {
                    let owned = journal.owned_directories.contains(&directory_key);
                    if !owned
                        && is_account_target(class, &relative_entry)
                        && fs::read_dir(&target_entry)?.next().transpose()?.is_some()
                    {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!(
                                "conflicting account migration directory: {}",
                                target_entry.display()
                            ),
                        ));
                    }
                    require_platform_durability(
                        &target_entry,
                        durable_file::ensure_private_directory_durable(&target_entry)?,
                        "migration directory",
                    )?;
                    if !owned && is_account_target(class, &relative_entry) {
                        journal.owned_directories.insert(directory_key);
                        write_promotion_journal(source, journal)?;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    require_platform_durability(
                        &target_entry,
                        durable_file::ensure_private_directory_durable(&target_entry)?,
                        "migration directory",
                    )?;
                    journal.owned_directories.insert(directory_key);
                    write_promotion_journal(source, journal)?;
                    after_visible()?;
                }
                Err(error) => return Err(error),
            }
            promote_directory(
                &staged_entry,
                &target_entry,
                &relative_entry,
                class,
                source,
                journal,
                after_visible,
            )?;
        } else if durable_file::is_plain_file(&metadata) {
            promote_file(
                &staged_entry,
                &target_entry,
                class,
                &relative_entry,
                source,
                journal,
            )?;
            after_visible()?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "staged migration entry is not a plain file or directory",
            ));
        }
    }
    Ok(())
}

fn promote_file(
    staged: &Path,
    target: &Path,
    class: PathClass,
    relative: &Path,
    source: &Path,
    journal: &mut PromotionJournal,
) -> io::Result<()> {
    let bytes = fs::read(staged)?;
    let digest = content_digest(&bytes);
    let key = (PromotionClass::from(class), relative.to_path_buf());
    let directory_key = key.clone();
    let existing = match fs::symlink_metadata(target) {
        Ok(metadata) if durable_file::is_plain_directory(&metadata) => {
            if !journal.owned_directories.contains(&directory_key) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "migration destination is an unowned directory: {}",
                        target.display()
                    ),
                ));
            }
            let outcome = durable_file::remove_directory(target)?;
            if !outcome.platform_durable() {
                return Err(io::Error::other(format!(
                    "migration directory deletion durability is unknown: {}",
                    target.display()
                )));
            }
            journal.owned_directories.remove(&directory_key);
            write_promotion_journal(source, journal)?;
            None
        }
        Ok(metadata) if !durable_file::is_plain_file(&metadata) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "migration destination is not a plain file: {}",
                    target.display()
                ),
            ));
        }
        Ok(_) => Some(fs::read(target)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let visible_owned_digest = if let Some(existing) = existing.as_deref() {
        let existing_digest = content_digest(existing);
        let owned = journal
            .entries
            .get(&key)
            .is_some_and(|accepted| accepted.iter().any(|one| one == &existing_digest));
        if existing == bytes {
            if is_account_target(class, relative) || is_serve_token_target(class, relative) {
                require_platform_durability(
                    target,
                    durable_file::replace_bytes(target, &bytes)?,
                    "credential migration target",
                )?;
            } else if owned {
                require_platform_durability(
                    target,
                    durable_file::sync_visible_parent(target)?,
                    "migration target",
                )?;
            }
            return Ok(());
        }
        if !owned {
            if is_account_target(class, relative) || is_serve_token_target(class, relative) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "conflicting credential migration target: {}",
                        target.display()
                    ),
                ));
            }
            // A genuine platform file predates this transaction and is the
            // authority. Only a digest recorded before our own visibility can
            // authorize replacement on a retry.
            return Ok(());
        }
        Some(existing_digest)
    } else {
        None
    };

    let accepted = next_accepted_digests(visible_owned_digest, digest);
    journal.entries.insert(key, accepted);
    write_promotion_journal(source, journal)?;
    let outcome = durable_file::replace_bytes(target, &bytes)?;
    if !outcome.platform_durable() {
        return Err(io::Error::other(format!(
            "migration target durability is unknown: {}",
            target.display()
        )));
    }
    #[cfg(unix)]
    preserve_private_executable_bit(&fs::metadata(staged)?, target)?;
    Ok(())
}

fn next_accepted_digests(visible_owned: Option<String>, desired: String) -> Vec<String> {
    let mut accepted = visible_owned.into_iter().collect::<Vec<_>>();
    if accepted.last() != Some(&desired) {
        accepted.push(desired);
    }
    accepted
}

fn ensure_plain_target_root(target: &Path) -> io::Result<()> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if !durable_file::is_plain_directory(&metadata) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "migration target root is not a plain directory: {}",
                target.display()
            ),
        )),
        Ok(_) => require_platform_durability(
            target,
            durable_file::ensure_private_directory_durable(target)?,
            "migration target root",
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => require_platform_durability(
            target,
            durable_file::ensure_private_directory_durable(target)?,
            "migration target root",
        ),
        Err(error) => Err(error),
    }
}

fn require_platform_durability(
    path: &Path,
    outcome: durable_file::CommitOutcome,
    operation: &str,
) -> io::Result<()> {
    if outcome.platform_durable() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{operation} durability is unknown: {}",
            path.display()
        )))
    }
}

fn is_account_target(class: PathClass, relative: &Path) -> bool {
    match class {
        PathClass::Config => {
            relative == Path::new(crate::accounts::ACCOUNT_STORE_FILE)
                || relative == Path::new(crate::codex_accounts::ACCOUNT_STORE_FILE)
        }
        PathClass::LocalData => relative.components().next().is_some_and(|component| {
            component.as_os_str() == crate::accounts::MANAGED_ACCOUNTS_DIR
                || component.as_os_str() == crate::codex_accounts::MANAGED_ACCOUNTS_DIR
        }),
        PathClass::Cache => false,
    }
}

fn is_serve_token_target(class: PathClass, relative: &Path) -> bool {
    if class != PathClass::LocalData {
        return false;
    }
    let mut components = relative.components();
    let Some(Component::Normal(name)) = components.next() else {
        return false;
    };
    components.next().is_none() && is_canonical_serve_token_name(name)
}

fn collect_staged_entries(
    directory: &Path,
    relative: &Path,
    class: PathClass,
    files: &mut BTreeSet<(PromotionClass, PathBuf)>,
    directories: &mut BTreeSet<(PromotionClass, PathBuf)>,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let relative = relative.join(entry.file_name());
        validate_relative_path(&relative)?;
        let metadata = fs::symlink_metadata(&path)?;
        if durable_file::is_plain_directory(&metadata) {
            directories.insert((PromotionClass::from(class), relative.clone()));
            collect_staged_entries(&path, &relative, class, files, directories)?;
        } else if durable_file::is_plain_file(&metadata) {
            files.insert((PromotionClass::from(class), relative));
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid staged migration entry: {}", path.display()),
            ));
        }
    }
    Ok(())
}

fn remove_stale_owned_entries(
    paths: &AppPaths,
    source: &Path,
    journal: &mut PromotionJournal,
    staged_files: &BTreeSet<(PromotionClass, PathBuf)>,
    staged_directories: &BTreeSet<(PromotionClass, PathBuf)>,
) -> io::Result<()> {
    let stale: Vec<_> = journal
        .entries
        .keys()
        .filter(|key| !staged_files.contains(*key))
        .cloned()
        .collect();
    for key in stale {
        let (class, relative) = (&key.0, &key.1);
        validate_relative_path(relative)?;
        let target = resolve_target_without_symlinks(paths.target(class.path_class()), relative)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) if !durable_file::is_plain_file(&metadata) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "owned migration target is not a plain file: {}",
                        target.display()
                    ),
                ));
            }
            Ok(_) => {
                let digest = content_digest(&fs::read(&target)?);
                let owned = journal
                    .entries
                    .get(&key)
                    .is_some_and(|accepted| accepted.iter().any(|one| one == &digest));
                if owned {
                    let outcome = durable_file::remove_file(&target)?;
                    if !outcome.platform_durable() {
                        return Err(io::Error::other(format!(
                            "migration target deletion durability is unknown: {}",
                            target.display()
                        )));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                require_platform_durability(
                    &target,
                    durable_file::sync_visible_parent(&target)?,
                    "migration target deletion",
                )?;
            }
            Err(error) => return Err(error),
        }
        journal.entries.remove(&key);
        write_promotion_journal(source, journal)?;
    }

    let mut stale_directories: Vec<_> = journal
        .owned_directories
        .iter()
        .filter(|key| !staged_directories.contains(*key))
        .cloned()
        .collect();
    stale_directories.sort_by_key(|(_, relative)| std::cmp::Reverse(relative.components().count()));
    for key in stale_directories {
        let (class, relative) = (&key.0, &key.1);
        let target = resolve_target_without_symlinks(paths.target(class.path_class()), relative)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) if durable_file::is_plain_directory(&metadata) => {
                match durable_file::remove_directory(&target) {
                    Ok(outcome) if outcome.platform_durable() => {}
                    Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => continue,
                    Ok(_) => {
                        return Err(io::Error::other(format!(
                            "migration directory deletion durability is unknown: {}",
                            target.display()
                        )));
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                require_platform_durability(
                    &target,
                    durable_file::sync_visible_parent(&target)?,
                    "migration directory deletion",
                )?;
            }
            Ok(_) => continue,
            Err(error) => return Err(error),
        }
        journal.owned_directories.remove(&key);
        write_promotion_journal(source, journal)?;
    }
    Ok(())
}

fn resolve_target_without_symlinks(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    validate_relative_path(relative)?;
    match fs::symlink_metadata(root) {
        Ok(metadata) if durable_file::is_plain_directory(&metadata) => {}
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "migration target root is not a plain directory: {}",
                    root.display()
                ),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(root.join(relative));
        }
        Err(error) => return Err(error),
    }
    let mut resolved = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(Component::Normal(component)) = components.next() {
        resolved.push(component);
        if components.peek().is_some() {
            match fs::symlink_metadata(&resolved) {
                Ok(metadata) if durable_file::is_plain_directory(&metadata) => {}
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "migration target parent is not a plain directory: {}",
                            resolved.display()
                        ),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => return Err(error),
            }
        }
    }
    Ok(root.join(relative))
}

fn load_promotion_journal(source: &Path) -> io::Result<PromotionJournal> {
    let path = source.join(PROMOTION_JOURNAL);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !durable_file::is_plain_file(&metadata) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "migration promotion journal is not a plain file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(PromotionJournal::default());
        }
        Err(error) => return Err(error),
    }
    let bytes = fs::read(&path)?;
    let stored: PromotionJournalFile = serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid migration promotion journal: {error}"),
        )
    })?;
    if stored.version != PROMOTION_JOURNAL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported migration promotion journal version",
        ));
    }
    let mut journal = PromotionJournal::default();
    for entry in stored.entries {
        validate_relative_path(&entry.relative)?;
        if entry.accepted_sha256.is_empty()
            || entry.accepted_sha256.len() > 2
            || entry.accepted_sha256.iter().any(|digest| {
                digest.len() != 64
                    || !digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid migration promotion digest",
            ));
        }
        if journal
            .entries
            .insert((entry.class, entry.relative), entry.accepted_sha256)
            .is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate migration promotion journal entry",
            ));
        }
    }
    for directory in stored.owned_directories {
        validate_relative_path(&directory.relative)?;
        if !journal
            .owned_directories
            .insert((directory.class, directory.relative))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate migration promotion directory",
            ));
        }
    }
    Ok(journal)
}

fn write_promotion_journal(source: &Path, journal: &PromotionJournal) -> io::Result<()> {
    let stored = PromotionJournalFile {
        version: PROMOTION_JOURNAL_VERSION,
        entries: journal
            .entries
            .iter()
            .map(
                |((class, relative), accepted_sha256)| PromotionJournalEntry {
                    class: *class,
                    relative: relative.clone(),
                    accepted_sha256: accepted_sha256.clone(),
                },
            )
            .collect(),
        owned_directories: journal
            .owned_directories
            .iter()
            .map(|(class, relative)| PromotionOwnedDirectory {
                class: *class,
                relative: relative.clone(),
            })
            .collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&stored).map_err(io::Error::other)?;
    bytes.push(b'\n');
    let outcome = durable_file::replace_bytes(&source.join(PROMOTION_JOURNAL), &bytes)?;
    if outcome.platform_durable() {
        Ok(())
    } else {
        Err(io::Error::other(
            "migration promotion journal durability is unknown",
        ))
    }
}

fn validate_relative_path(relative: &Path) -> io::Result<()> {
    let valid = !relative.as_os_str().is_empty()
        && !relative.is_absolute()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid migration journal path",
        ))
    }
}

fn content_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn remove_private_staging(staging: &Path) -> io::Result<()> {
    let mut inventory = LegacySecretInventory::default();
    collect_plain_secret_tree(staging, &mut inventory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "migration staging is not an exact plain private tree: {}: {error}",
                staging.display()
            ),
        )
    })?;
    for file in &inventory.files {
        remove_plain_secret_file(file)?;
    }
    for directory in &inventory.directories {
        remove_plain_secret_directory(directory)?;
    }
    require_platform_durability(
        staging,
        durable_file::sync_visible_parent(staging)?,
        "migration staging retirement",
    )
}

#[cfg(unix)]
fn preserve_private_executable_bit(metadata: &fs::Metadata, destination: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let source_mode = metadata.permissions().mode();
    let mode = if source_mode & 0o100 != 0 {
        0o700
    } else {
        0o600
    };
    fs::set_permissions(destination, fs::Permissions::from_mode(mode))
}

fn complete_receipt(source: &Path) -> Option<PathMigrationReceipt> {
    let receipt = read_complete_path_migration_receipt(source)?;
    let manifest_matches =
        receipt.completed_artifacts() == crate::app_paths::artifact_manifest_entries();
    manifest_matches.then_some(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> AppPaths {
        let legacy = root.join("legacy");
        fs::create_dir_all(&legacy).expect("legacy root");
        AppPaths::from_resolved(
            root.join("config"),
            root.join("local"),
            root.join("cache"),
            Some(legacy),
        )
    }

    fn write_pending_receipt(source: &Path) {
        let receipt =
            PathMigrationReceipt::completed(crate::app_paths::artifact_manifest_entries());
        assert!(
            receipt.is_complete(),
            "test receipt must match the live manifest"
        );
        let bytes = serde_json::to_vec_pretty(&receipt).expect("receipt JSON");
        let outcome = durable_file::replace_bytes(&source.join(PATH_MIGRATION_RECEIPT), &bytes)
            .expect("pending receipt");
        assert!(outcome.platform_durable());
    }

    fn resolve_again(paths: &AppPaths) -> AppPaths {
        AppPaths::try_from_resolved(
            paths.target(PathClass::Config).to_path_buf(),
            paths.target(PathClass::LocalData).to_path_buf(),
            paths.target(PathClass::Cache).to_path_buf(),
            paths.legacy_state().map(Path::to_path_buf),
        )
        .expect("resolve paths")
    }

    fn serve_token_name(identity: u64) -> String {
        let name = format!("{SERVE_TOKEN_PREFIX}{identity:016x}");
        assert!(is_canonical_serve_token_name(std::ffi::OsStr::new(&name)));
        name
    }

    fn staged_serve_token_name(canonical: &str, nonce: u128) -> String {
        let name = format!(".{canonical}.{nonce:032x}.tmp");
        assert!(is_owned_staged_serve_token_name(std::ffi::OsStr::new(
            &name
        )));
        name
    }

    #[test]
    fn all_classes_move_before_the_source_receipt_activates_them() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        fs::write(legacy.join("preferences.json"), b"preferences").expect("config");
        fs::write(legacy.join("automation-runs.json"), b"runs").expect("local data");
        fs::create_dir_all(legacy.join("agent-icons")).expect("cache source");
        fs::write(legacy.join("agent-icons/old.png"), b"old cache").expect("cache");

        let activated = migrate_if_needed(&paths).expect("migration");
        assert!(activated.platform_paths_active());
        assert_eq!(
            fs::read(activated.target(PathClass::Config).join("preferences.json")).unwrap(),
            b"preferences"
        );
        assert_eq!(
            fs::read(
                activated
                    .target(PathClass::LocalData)
                    .join("automation-runs.json")
            )
            .unwrap(),
            b"runs"
        );
        assert!(
            !activated
                .target(PathClass::Cache)
                .join("agent-icons/old.png")
                .exists()
        );
        assert!(complete_receipt(legacy).is_some());
    }

    #[test]
    fn preexisting_platform_data_wins_over_legacy() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        fs::write(legacy.join("recent-projects.json"), b"stale source").expect("source");
        fs::create_dir_all(paths.target(PathClass::Config)).expect("destination root");
        fs::write(
            paths.target(PathClass::Config).join("recent-projects.json"),
            b"already committed",
        )
        .expect("destination");

        let activated = migrate_if_needed(&paths).expect("migration");
        assert_eq!(
            fs::read(
                activated
                    .target(PathClass::Config)
                    .join("recent-projects.json")
            )
            .unwrap(),
            b"already committed"
        );
    }

    #[test]
    fn a_conflicting_platform_serve_token_never_retires_or_activates_legacy() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let name = serve_token_name(1);
        let source = legacy.join(&name);
        let target = paths.target(PathClass::LocalData).join(&name);
        fs::write(&source, b"legacy-A").expect("legacy token");
        durable_file::ensure_private_directory(paths.target(PathClass::LocalData))
            .expect("target local data");
        fs::write(&target, b"platform-B").expect("platform token");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("different credentials must never be merged implicitly");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&source).expect("legacy token remains"),
            b"legacy-A"
        );
        assert_eq!(
            fs::read(&target).expect("platform token remains"),
            b"platform-B"
        );
        assert!(complete_receipt(legacy).is_none());
        assert!(!resolve_again(&paths).platform_paths_active());
    }

    #[test]
    fn an_identical_platform_serve_token_is_an_idempotent_migration_target() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let name = serve_token_name(1);
        let source = legacy.join(&name);
        let target = paths.target(PathClass::LocalData).join(&name);
        fs::write(&source, b"same-token").expect("legacy token");
        durable_file::ensure_private_directory(paths.target(PathClass::LocalData))
            .expect("target local data");
        fs::write(&target, b"same-token").expect("platform token");

        let activated =
            migrate_with_jira_preflight(&paths, |_| Ok(())).expect("idempotent migration");
        assert!(activated.platform_paths_active());
        assert!(!source.exists());
        assert_eq!(fs::read(target).expect("platform token"), b"same-token");
        assert!(complete_receipt(legacy).is_some());
    }

    #[test]
    fn canonical_token_migrates_but_its_hardlinked_staging_bearer_is_only_retired() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let canonical_name = serve_token_name(2);
        let staged_name = staged_serve_token_name(&canonical_name, 3);
        let canonical = legacy.join(&canonical_name);
        let staged = legacy.join(&staged_name);
        fs::write(&canonical, b"one bearer identity").expect("canonical token");
        fs::hard_link(&canonical, &staged).expect("hardlinked token staging");
        #[cfg(unix)]
        let source_bearer = fs::File::open(&canonical).expect("open source bearer inode");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(source_bearer.metadata().expect("metadata").nlink(), 2);
        }

        let activated = migrate_with_jira_preflight(&paths, |_| Ok(())).expect("token migration");

        assert!(activated.platform_paths_active());
        assert_eq!(
            fs::read(activated.target(PathClass::LocalData).join(&canonical_name))
                .expect("target canonical token"),
            b"one bearer identity"
        );
        assert!(
            !activated
                .target(PathClass::LocalData)
                .join(&staged_name)
                .exists(),
            "transaction-private token names never migrate"
        );
        assert!(!canonical.exists());
        assert!(!staged.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(
                source_bearer.metadata().expect("retired metadata").nlink(),
                0,
                "the retired source inode must have no remaining bearer links"
            );
        }
        assert!(complete_receipt(legacy).is_some());
    }

    #[test]
    fn one_staged_only_token_blocks_migration_without_deleting_the_candidate_identity() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let canonical_name = serve_token_name(4);
        let staged = legacy.join(staged_serve_token_name(&canonical_name, 5));
        fs::write(&staged, b"ambiguous prepublish identity").expect("staged-only token");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("staged-only identity must fail closed");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("no canonical counterpart"));
        assert_eq!(
            fs::read(&staged).expect("staged candidate remains"),
            b"ambiguous prepublish identity"
        );
        assert!(
            !paths
                .target(PathClass::LocalData)
                .join(&canonical_name)
                .exists()
        );
        assert!(complete_receipt(legacy).is_none());
        assert!(!legacy.join(zerocode_lane::PATH_ACTIVATION_WITNESS).exists());
        assert!(
            !paths
                .target(PathClass::Config)
                .join(zerocode_lane::PATH_ACTIVATION_WITNESS)
                .exists()
        );
    }

    #[test]
    fn multiple_staged_only_tokens_all_remain_when_their_canonical_identity_is_ambiguous() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let canonical_name = serve_token_name(6);
        let first = legacy.join(staged_serve_token_name(&canonical_name, 7));
        let second = legacy.join(staged_serve_token_name(&canonical_name, 8));
        fs::write(&first, b"first candidate").expect("first staged token");
        fs::write(&second, b"second candidate").expect("second staged token");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("multiple staged-only identities must fail closed");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(&first).expect("first candidate remains"),
            b"first candidate"
        );
        assert_eq!(
            fs::read(&second).expect("second candidate remains"),
            b"second candidate"
        );
        assert!(
            !paths
                .target(PathClass::LocalData)
                .join(&canonical_name)
                .exists()
        );
        assert!(complete_receipt(legacy).is_none());
        assert!(!legacy.join(zerocode_lane::PATH_ACTIVATION_WITNESS).exists());
        assert!(
            !paths
                .target(PathClass::Config)
                .join(zerocode_lane::PATH_ACTIVATION_WITNESS)
                .exists()
        );
    }

    #[test]
    fn failed_staging_never_hides_a_newer_legacy_write_on_retry() {
        #[cfg(unix)]
        {
            let root = tempfile::tempdir().expect("root");
            let paths = fixture(root.path());
            let legacy = paths.legacy_state().expect("legacy");
            fs::write(legacy.join("onboarding.json"), b"first").expect("source");
            let outside = root.path().join("outside");
            fs::write(&outside, b"outside").expect("outside");
            std::os::unix::fs::symlink(&outside, legacy.join("quick-commands.json"))
                .expect("late failure");

            assert!(migrate_if_needed(&paths).is_err());
            assert!(
                !paths
                    .target(PathClass::Config)
                    .join("onboarding.json")
                    .exists()
            );
            assert!(complete_receipt(legacy).is_none());

            fs::remove_file(legacy.join("quick-commands.json")).expect("remove fault");
            fs::write(legacy.join("onboarding.json"), b"newer legacy write").expect("update");
            let activated = migrate_if_needed(&paths).expect("retry");
            assert_eq!(
                fs::read(activated.target(PathClass::Config).join("onboarding.json")).unwrap(),
                b"newer legacy write"
            );
        }
    }

    #[test]
    fn stale_plaintext_staging_is_durably_retired_before_jira_preflight() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let staging = legacy.join(STAGING_DIRECTORY);
        let staged_token = staging.join("local-data").join(serve_token_name(21));
        let staged_account = staging
            .join("local-data")
            .join(crate::accounts::MANAGED_ACCOUNTS_DIR)
            .join("account")
            .join(".credentials.json");
        fs::create_dir_all(staged_token.parent().expect("token parent"))
            .expect("staged token parent");
        fs::create_dir_all(staged_account.parent().expect("account parent"))
            .expect("staged account parent");
        fs::write(&staged_token, b"stale bearer").expect("staged token");
        fs::write(&staged_account, b"stale account credential").expect("staged account");

        let mut preflight_ran = false;
        let error = migrate_with_jira_preflight(&paths, |_| {
            preflight_ran = true;
            assert!(
                !staging.exists(),
                "plaintext staging must be gone before a denied preflight"
            );
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected Jira denial",
            ))
        })
        .expect_err("preflight denial");

        assert!(preflight_ran);
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!staging.exists());
        assert!(complete_receipt(legacy).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn stale_plaintext_staging_cleanup_never_follows_or_partially_unlinks_a_symlink() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let staging = legacy.join(STAGING_DIRECTORY);
        let staged_secret = staging.join("a-staged-secret");
        let outside = root.path().join("outside-secret");
        fs::create_dir_all(&staging).expect("staging");
        fs::write(&staged_secret, b"must not be partially unlinked").expect("staged secret");
        fs::write(&outside, b"outside remains").expect("outside secret");
        std::os::unix::fs::symlink(&outside, staging.join("z-linked-secret"))
            .expect("staging symlink");
        let mut preflight_ran = false;

        let error = migrate_with_jira_preflight(&paths, |_| {
            preflight_ran = true;
            Ok(())
        })
        .expect_err("a linked staging entry must block startup");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!preflight_ran);
        assert_eq!(
            fs::read(&staged_secret).expect("staged secret remains"),
            b"must not be partially unlinked"
        );
        assert_eq!(
            fs::read(&outside).expect("outside secret remains"),
            b"outside remains"
        );
        assert!(complete_receipt(legacy).is_none());
    }

    #[test]
    fn a_mid_promotion_crash_replaces_only_its_owned_stale_file_on_retry() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        fs::write(legacy.join("onboarding.json"), b"first").expect("source");

        let mut visible = false;
        let error = migrate_with_hooks(
            &paths,
            |_| Ok(()),
            || {
                if visible {
                    Ok(())
                } else {
                    visible = true;
                    Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "injected after-visible crash",
                    ))
                }
            },
        )
        .expect_err("promotion interruption");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        let target = paths.target(PathClass::Config).join("onboarding.json");
        assert_eq!(fs::read(&target).expect("visible first value"), b"first");
        assert!(legacy.join(PROMOTION_JOURNAL).is_file());
        assert!(complete_receipt(legacy).is_none());

        fs::write(legacy.join("onboarding.json"), b"newer legacy write").expect("update");
        let activated = migrate_with_jira_preflight(&paths, |_| Ok(())).expect("retry");
        assert_eq!(
            fs::read(activated.target(PathClass::Config).join("onboarding.json")).unwrap(),
            b"newer legacy write"
        );
        assert!(!legacy.join(PROMOTION_JOURNAL).exists());
    }

    #[test]
    fn retry_removes_an_owned_file_only_when_the_new_source_removed_it() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        fs::write(legacy.join("onboarding.json"), b"first").expect("source");
        let mut interrupted = false;
        migrate_with_hooks(
            &paths,
            |_| Ok(()),
            || {
                if interrupted {
                    Ok(())
                } else {
                    interrupted = true;
                    Err(io::Error::new(io::ErrorKind::Interrupted, "fault"))
                }
            },
        )
        .expect_err("interruption");
        let target = paths.target(PathClass::Config).join("onboarding.json");
        fs::remove_file(legacy.join("onboarding.json")).expect("source removal");

        migrate_with_jira_preflight(&paths, |_| Ok(())).expect("retry");
        assert!(!target.exists());
    }

    #[test]
    fn an_owned_directory_survives_the_journal_and_can_become_a_file() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let accounts = seed_account_repositories(&paths, root.path());
        let source = accounts.source_claude.join(".credentials.json");
        fs::remove_file(&source).expect("replace credential file");
        fs::create_dir(&source).expect("directory-shaped source");
        fs::write(source.join("held"), b"first").expect("nested source");
        let target = accounts.target_claude.join(".credentials.json");

        let mut interrupted = false;
        migrate_with_hooks(
            &paths,
            |_| Ok(()),
            || {
                if interrupted || !target.is_dir() {
                    Ok(())
                } else {
                    interrupted = true;
                    Err(io::Error::new(io::ErrorKind::Interrupted, "fault"))
                }
            },
        )
        .expect_err("directory publication interruption");
        let journal = load_promotion_journal(legacy).expect("persisted journal");
        assert!(journal.owned_directories.contains(&(
            PromotionClass::LocalData,
            PathBuf::from("claude-accounts/a1/.credentials.json")
        )));

        fs::remove_dir_all(&source).expect("replace source directory");
        fs::write(&source, b"second").expect("file-shaped source");
        migrate_with_jira_preflight(&paths, |_| Ok(())).expect("retry");
        assert!(fs::metadata(&target).expect("target metadata").is_file());
        assert_eq!(fs::read(target).expect("target value"), b"second");
    }

    #[test]
    fn three_intents_never_evict_the_digest_that_is_still_visible() {
        let visible = content_digest(b"A");
        let second = next_accepted_digests(Some(visible.clone()), content_digest(b"B"));
        assert_eq!(second.first(), Some(&visible));

        // B was journaled, but its replacement never became visible. A retry
        // for C must authorize the A that is still on disk, not retain B.
        let third = next_accepted_digests(Some(visible.clone()), content_digest(b"C"));
        assert_eq!(third.len(), 2);
        assert_eq!(third.first(), Some(&visible));
        assert_eq!(third.last(), Some(&content_digest(b"C")));
    }

    #[cfg(unix)]
    #[test]
    fn stale_cleanup_never_follows_a_target_root_symlink() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let outside = root.path().join("outside-root");
        fs::create_dir_all(&outside).expect("outside");
        let victim = outside.join("victim.json");
        fs::write(&victim, b"held").expect("outside victim");
        std::os::unix::fs::symlink(&outside, paths.target(PathClass::Config))
            .expect("target root symlink");
        let mut journal = PromotionJournal::default();
        journal.entries.insert(
            (PromotionClass::Config, PathBuf::from("victim.json")),
            vec![content_digest(b"held")],
        );

        let error = remove_stale_owned_entries(
            &paths,
            legacy,
            &mut journal,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .expect_err("target symlink must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(victim).expect("outside value"), b"held");
    }

    #[cfg(unix)]
    #[test]
    fn stale_cleanup_never_follows_an_intermediate_target_symlink() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        durable_file::ensure_private_directory(paths.target(PathClass::Config))
            .expect("target root");
        let outside = root.path().join("outside-parent");
        fs::create_dir_all(&outside).expect("outside");
        let victim = outside.join("victim.json");
        fs::write(&victim, b"held").expect("outside victim");
        std::os::unix::fs::symlink(&outside, paths.target(PathClass::Config).join("owned"))
            .expect("intermediate symlink");
        let mut journal = PromotionJournal::default();
        journal.entries.insert(
            (PromotionClass::Config, PathBuf::from("owned/victim.json")),
            vec![content_digest(b"held")],
        );

        assert!(
            remove_stale_owned_entries(
                &paths,
                legacy,
                &mut journal,
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .is_err()
        );
        assert_eq!(fs::read(victim).expect("outside value"), b"held");
    }

    #[test]
    fn settings_displaced_primary_moves_with_its_repository() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let displaced = legacy.join(".preferences.json.replace-1-1");
        let document = serde_json::json!({
            "_meta": {"format": 1, "revision": 7},
            "data": {"theme": "light"}
        });
        fs::write(
            &displaced,
            serde_json::to_vec_pretty(&document).expect("document"),
        )
        .expect("displaced primary");

        let activated = migrate_with_jira_preflight(&paths, |_| Ok(())).expect("migration");
        let repository =
            crate::settings::SettingsRepository::new(activated.target(PathClass::Config));
        let restored = repository
            .read_json::<serde_json::Value>("preferences.json")
            .expect("repository recovery");
        assert_eq!(restored.revision, 7);
        assert_eq!(restored.value.expect("value")["theme"], "light");
    }

    #[test]
    fn a_file_artifact_with_directory_shape_fails_before_the_receipt() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        fs::create_dir_all(legacy.join("onboarding.json")).expect("wrong shape");
        fs::write(legacy.join("onboarding.json/child"), b"held").expect("child");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("typed manifest must reject a directory-shaped file");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(complete_receipt(legacy).is_none());
    }

    #[test]
    fn deleting_a_destination_after_completion_never_reimports_home() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy").to_path_buf();
        let source_file = legacy.join("onboarding.json");
        fs::write(&source_file, b"old onboarding").expect("source");
        let activated = migrate_if_needed(&paths).expect("first migration");
        let target = activated.target(PathClass::Config).join("onboarding.json");
        fs::remove_file(&target).expect("intentional reset");

        let restarted = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy),
        );
        assert!(restarted.platform_paths_active());
        migrate_if_needed(&restarted).expect("restart");
        assert!(!target.exists(), "completed HOME data was imported again");
        assert_eq!(fs::read(source_file).unwrap(), b"old onboarding");
    }

    #[test]
    fn a_source_symlink_fails_closed_without_an_authority_switch() {
        #[cfg(unix)]
        {
            let root = tempfile::tempdir().expect("root");
            let paths = fixture(root.path());
            let legacy = paths.legacy_state().expect("legacy");
            let outside = root.path().join("outside");
            fs::write(&outside, b"outside").expect("outside");
            std::os::unix::fs::symlink(&outside, legacy.join("quick-commands.json"))
                .expect("symlink");
            assert!(migrate_if_needed(&paths).is_err());
            assert!(!paths.platform_paths_active());
            assert!(complete_receipt(legacy).is_none());
        }
    }

    #[test]
    fn a_blocked_native_credential_migration_keeps_pending_roots_non_writable() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy").to_path_buf();
        fs::write(legacy.join("preferences.json"), b"legacy settings").expect("source");

        let error = migrate_with_jira_preflight(&paths, |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "native vault denied",
            ))
        })
        .expect_err("migration must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(paths.migration_pending());
        for class in [PathClass::Config, PathClass::LocalData, PathClass::Cache] {
            assert_eq!(
                paths
                    .writable_root(class)
                    .expect_err("pending root must not be writable")
                    .kind(),
                io::ErrorKind::WouldBlock
            );
        }
        assert!(complete_receipt(&legacy).is_none());
        assert!(
            !paths
                .target(PathClass::Config)
                .join("preferences.json")
                .exists()
        );
    }

    #[test]
    fn receipt_and_a_fresh_cli_resolution_select_the_same_authority() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy").to_path_buf();
        let active = migrate_if_needed(&paths).expect("migration");
        let fresh = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy),
        );
        assert!(active.platform_paths_active());
        assert!(fresh.platform_paths_active());
        for class in [PathClass::Config, PathClass::LocalData, PathClass::Cache] {
            assert_eq!(active.active_root(class), fresh.active_root(class));
        }
    }

    struct AccountMigrationFixture {
        source_claude: PathBuf,
        source_codex_home: PathBuf,
        target_claude: PathBuf,
        target_codex_home: PathBuf,
        external_claude: PathBuf,
        external_codex: PathBuf,
    }

    fn seed_account_repositories(paths: &AppPaths, root: &Path) -> AccountMigrationFixture {
        let source = paths.legacy_state().expect("legacy");
        let source_claude = crate::accounts::account_dir(source, "a1").expect("Claude path");
        let source_codex_home =
            crate::codex_accounts::account_home(source, "c1").expect("Codex path");
        let target_claude = crate::accounts::account_dir(paths.target(PathClass::LocalData), "a1")
            .expect("target Claude path");
        let target_codex_home =
            crate::codex_accounts::account_home(paths.target(PathClass::LocalData), "c1")
                .expect("target Codex path");
        let external_claude = root.join("external-claude");
        let external_codex = root.join("external-codex");

        for directory in [
            &source_claude,
            &source_codex_home,
            &external_claude,
            &external_codex,
        ] {
            fs::create_dir_all(directory).expect("account home");
        }
        fs::write(
            source_claude.join(".credentials.json"),
            r#"{"claudeAiOauth":{"emailAddress":"managed@example.com"}}"#,
        )
        .expect("Claude credential");
        fs::write(
            source_codex_home.join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-test"}"#,
        )
        .expect("Codex credential");

        let claude_index = serde_json::json!({
            "accounts": [
                {
                    "id": "a1",
                    "email": "managed@example.com",
                    "config_dir": source_claude.to_string_lossy(),
                    "added_at": 1,
                    "future_account_field": {"keep": true}
                },
                {
                    "id": "a-external",
                    "email": "external@example.com",
                    "config_dir": external_claude.to_string_lossy(),
                    "added_at": 2
                }
            ],
            "active": "a1",
            "future_root_field": [1, 2, 3]
        });
        let codex_index = serde_json::json!({
            "accounts": [
                {
                    "id": "c1",
                    "home_dir": source_codex_home.to_string_lossy(),
                    "added_at": 1,
                    "future_account_field": "keep"
                },
                {
                    "id": "c-external",
                    "home_dir": external_codex.to_string_lossy(),
                    "added_at": 2
                }
            ],
            "active": "c1",
            "future_root_field": {"keep": true}
        });
        fs::write(
            source.join(crate::accounts::ACCOUNT_STORE_FILE),
            serde_json::to_vec_pretty(&claude_index).expect("Claude JSON"),
        )
        .expect("Claude index");
        fs::write(
            source.join(crate::codex_accounts::ACCOUNT_STORE_FILE),
            serde_json::to_vec_pretty(&codex_index).expect("Codex JSON"),
        )
        .expect("Codex index");

        AccountMigrationFixture {
            source_claude,
            source_codex_home,
            target_claude,
            target_codex_home,
            external_claude,
            external_codex,
        }
    }

    #[test]
    fn account_transform_rewrites_only_owned_paths_and_preserves_unknown_json() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let accounts = seed_account_repositories(&paths, root.path());

        let activated = migrate_with_jira_preflight(&paths, |_| Ok(())).expect("migration");
        let config = activated.target(PathClass::Config);
        let claude: serde_json::Value = serde_json::from_slice(
            &fs::read(config.join(crate::accounts::ACCOUNT_STORE_FILE)).expect("Claude index"),
        )
        .expect("Claude JSON");
        let codex: serde_json::Value = serde_json::from_slice(
            &fs::read(config.join(crate::codex_accounts::ACCOUNT_STORE_FILE)).expect("Codex index"),
        )
        .expect("Codex JSON");

        assert_eq!(
            claude["accounts"][0]["config_dir"],
            accounts.target_claude.to_string_lossy().as_ref()
        );
        assert_eq!(
            codex["accounts"][0]["home_dir"],
            accounts.target_codex_home.to_string_lossy().as_ref()
        );
        assert_eq!(
            claude["accounts"][1]["config_dir"],
            accounts.external_claude.to_string_lossy().as_ref()
        );
        assert_eq!(
            codex["accounts"][1]["home_dir"],
            accounts.external_codex.to_string_lossy().as_ref()
        );
        assert_eq!(claude["accounts"][0]["future_account_field"]["keep"], true);
        assert_eq!(claude["future_root_field"], serde_json::json!([1, 2, 3]));
        assert_eq!(codex["accounts"][0]["future_account_field"], "keep");
        assert_eq!(codex["future_root_field"]["keep"], true);
        assert!(accounts.target_claude.join(".credentials.json").is_file());
        assert!(accounts.target_codex_home.join("auth.json").is_file());
        assert!(accounts.external_claude.is_dir());
        assert!(accounts.external_codex.is_dir());
    }

    #[test]
    fn restart_launches_from_target_and_remove_leaves_no_managed_home() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy").to_path_buf();
        let accounts = seed_account_repositories(&paths, root.path());

        migrate_with_jira_preflight(&paths, |_| Ok(())).expect("migration");
        let restarted = AppPaths::from_resolved(
            root.path().join("config"),
            root.path().join("local"),
            root.path().join("cache"),
            Some(legacy.clone()),
        );
        assert!(restarted.platform_paths_active());
        let config = restarted.active_root(PathClass::Config);
        let local_data = restarted.active_root(PathClass::LocalData);
        // The Claude account survived the move — asked of the store, which is
        // where an account lives now. The launch environment names the one
        // app-owned runtime, never this account's credential directory: that is
        // what keeps every Zerocode account's conversations together without
        // changing a separate terminal's login.
        assert_eq!(
            crate::accounts::read_store(config)
                .accounts
                .iter()
                .find(|one| one.id == "a1")
                .map(|one| one.config_dir.as_str()),
            Some(accounts.target_claude.to_string_lossy().as_ref())
        );
        assert_eq!(
            crate::accounts::launch_env_for(config, "claude")
                .expect("the migrated Claude account launches")
                .iter()
                .find(|(key, _)| key == zerocode_core::CONFIG_DIR_VAR)
                .map(|(_, value)| value.as_str()),
            crate::accounts::runtime_home(config).to_str(),
            "a launch did not use the shared app-owned Claude runtime"
        );
        assert_eq!(
            crate::codex_accounts::launch_env(config)[0],
            (
                zerocode_core::codex_account::HOME_VAR.to_string(),
                crate::codex_accounts::runtime_home(local_data)
                    .to_string_lossy()
                    .into_owned()
            )
        );

        crate::accounts::remove_account(config, local_data, "a1").expect("remove Claude");
        crate::codex_accounts::remove_account(config, local_data, "c1").expect("remove Codex");
        assert!(!accounts.target_claude.exists());
        assert!(
            !accounts
                .target_codex_home
                .parent()
                .expect("Codex account directory")
                .exists()
        );
        assert!(!legacy.join(crate::accounts::MANAGED_ACCOUNTS_DIR).exists());
        assert!(
            !legacy
                .join(crate::codex_accounts::MANAGED_ACCOUNTS_DIR)
                .exists()
        );
        assert!(!accounts.source_claude.exists());
        assert!(!accounts.source_codex_home.exists());
    }

    #[test]
    fn conflicting_platform_account_store_fails_without_retiring_legacy_credentials() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy").to_path_buf();
        let accounts = seed_account_repositories(&paths, root.path());
        durable_file::ensure_private_directory(paths.target(PathClass::Config))
            .expect("target config");
        fs::write(
            paths
                .target(PathClass::Config)
                .join(crate::accounts::ACCOUNT_STORE_FILE),
            br#"{"accounts":[],"platform_only":true}"#,
        )
        .expect("conflicting index");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("platform account store conflict must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(complete_receipt(&legacy).is_none());
        assert!(accounts.source_claude.join(".credentials.json").is_file());
        assert!(accounts.source_codex_home.join("auth.json").is_file());
        assert!(!accounts.target_claude.exists());
        assert!(!accounts.target_codex_home.exists());
    }

    #[test]
    fn an_empty_account_directory_left_before_journaling_is_adopted_on_retry() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let accounts = seed_account_repositories(&paths, root.path());
        let residue = paths
            .target(PathClass::LocalData)
            .join(crate::accounts::MANAGED_ACCOUNTS_DIR);
        durable_file::ensure_private_directory_durable(&residue).expect("crash residue");

        migrate_with_jira_preflight(&paths, |_| Ok(())).expect("retry");
        assert!(accounts.target_claude.join(".credentials.json").is_file());
    }

    #[test]
    fn a_nonempty_unowned_account_directory_is_never_merged() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let accounts = seed_account_repositories(&paths, root.path());
        let conflict = paths
            .target(PathClass::LocalData)
            .join(crate::accounts::MANAGED_ACCOUNTS_DIR);
        durable_file::ensure_private_directory(&conflict).expect("platform account root");
        fs::write(conflict.join("foreign"), b"held").expect("foreign data");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("foreign account data must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(conflict.join("foreign").is_file());
        assert!(accounts.source_claude.join(".credentials.json").is_file());
        assert!(complete_receipt(legacy).is_none());
    }

    #[test]
    fn an_unindexed_managed_credential_home_fails_closed() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let orphan = legacy
            .join(crate::accounts::MANAGED_ACCOUNTS_DIR)
            .join("orphan");
        fs::create_dir_all(&orphan).expect("orphan home");
        fs::write(orphan.join(".credentials.json"), b"secret").expect("credential");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("unindexed credentials must not be deleted");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(orphan.join(".credentials.json").is_file());
        assert!(complete_receipt(legacy).is_none());
    }

    #[test]
    fn an_index_path_inside_the_managed_root_must_match_its_id() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let accounts = seed_account_repositories(&paths, root.path());
        let index = legacy.join(crate::accounts::ACCOUNT_STORE_FILE);
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&index).expect("index")).expect("JSON");
        document["accounts"][0]["config_dir"] = serde_json::Value::String(
            legacy
                .join(crate::accounts::MANAGED_ACCOUNTS_DIR)
                .join("different-id")
                .to_string_lossy()
                .into_owned(),
        );
        fs::write(&index, serde_json::to_vec_pretty(&document).expect("JSON"))
            .expect("modified index");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("mismatched managed path must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(accounts.source_claude.join(".credentials.json").is_file());
        assert!(complete_receipt(legacy).is_none());
    }

    #[test]
    fn legacy_secret_retirement_is_exact_and_idempotent() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let claude = legacy.join(crate::accounts::MANAGED_ACCOUNTS_DIR);
        let codex = legacy.join(crate::codex_accounts::MANAGED_ACCOUNTS_DIR);
        let unrelated = legacy.join("claude-accounts-backup");
        for directory in [&claude, &codex, &unrelated] {
            fs::create_dir_all(directory).expect("directory");
            fs::write(directory.join("secret"), b"held").expect("file");
        }
        let serve_token = legacy.join(serve_token_name(1));
        let similar_name = legacy.join("serve-tokenized");
        fs::write(&serve_token, b"token").expect("serve token");
        fs::write(&similar_name, b"not a token").expect("similar file");

        retire_legacy_secrets(legacy).expect("first cleanup");
        retire_legacy_secrets(legacy).expect("retry cleanup");
        assert!(!claude.exists());
        assert!(!codex.exists());
        assert!(!serve_token.exists());
        assert!(unrelated.join("secret").is_file());
        assert!(similar_name.is_file());
    }

    #[test]
    fn interrupted_secret_retirement_converges_on_restart() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let canonical_name = serve_token_name(1);
        let serve_token = legacy.join(&canonical_name);
        let staged_token = legacy.join(staged_serve_token_name(&canonical_name, 2));
        let claude = legacy.join(crate::accounts::MANAGED_ACCOUNTS_DIR);
        let codex = legacy.join(crate::codex_accounts::MANAGED_ACCOUNTS_DIR);
        fs::write(&serve_token, b"token").expect("serve token");
        fs::hard_link(&serve_token, &staged_token).expect("staged token link");
        for directory in [&claude, &codex] {
            fs::create_dir_all(directory).expect("managed root");
            fs::write(directory.join("secret"), b"held").expect("credential");
        }

        let mut unlinks = 0;
        let error = retire_legacy_secrets_with(legacy, |_| {
            unlinks += 1;
            if unlinks == 1 {
                Err(io::Error::new(io::ErrorKind::Interrupted, "crash"))
            } else {
                Ok(())
            }
        })
        .expect_err("injected crash");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(!staged_token.exists());
        assert!(
            serve_token.is_file(),
            "canonical must remain until every staged bearer is retired"
        );

        retire_legacy_secrets(legacy).expect("restart cleanup");
        assert!(!serve_token.exists());
        assert!(!staged_token.exists());
        assert!(!claude.exists());
        assert!(!codex.exists());
    }

    #[test]
    fn an_owned_staged_token_directory_blocks_migration_without_mutating_source_bearers() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let canonical = serve_token_name(10);
        let serve_token = legacy.join(&canonical);
        let invalid = legacy.join(staged_serve_token_name(&canonical, 11));
        fs::write(&serve_token, b"token").expect("serve token");
        fs::create_dir_all(&invalid).expect("invalid token directory");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("staged token directory must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&serve_token).expect("source token"), b"token");
        assert!(serve_token.is_file());
        assert!(invalid.is_dir());
        assert!(!paths.target(PathClass::LocalData).join(&canonical).exists());
        assert!(complete_receipt(legacy).is_none());
        assert!(!legacy.join(zerocode_lane::PATH_ACTIVATION_WITNESS).exists());
        assert!(
            !paths
                .target(PathClass::Config)
                .join(zerocode_lane::PATH_ACTIVATION_WITNESS)
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_owned_staged_token_symlink_blocks_migration_without_following_it() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let canonical_name = serve_token_name(12);
        let canonical = legacy.join(&canonical_name);
        let staged = legacy.join(staged_serve_token_name(&canonical_name, 13));
        let outside = root.path().join("outside-bearer");
        fs::write(&canonical, b"canonical").expect("canonical token");
        fs::write(&outside, b"outside").expect("outside token");
        std::os::unix::fs::symlink(&outside, &staged).expect("staged symlink");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("staged token symlink must fail closed");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(&canonical).expect("canonical source"),
            b"canonical"
        );
        assert_eq!(fs::read(&outside).expect("outside source"), b"outside");
        assert!(fs::symlink_metadata(&staged).is_ok());
        assert!(complete_receipt(legacy).is_none());
        assert!(!legacy.join(zerocode_lane::PATH_ACTIVATION_WITNESS).exists());
        assert!(
            !paths
                .target(PathClass::Config)
                .join(zerocode_lane::PATH_ACTIVATION_WITNESS)
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_owned_staged_token_special_file_blocks_migration_before_receipt() {
        use std::os::unix::net::UnixListener;

        // Unix-domain socket paths have a small fixed kernel limit, while the
        // strict staged-token name is intentionally long.
        let root = tempfile::Builder::new()
            .prefix("zc")
            .tempdir_in("/tmp")
            .expect("short root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let canonical_name = serve_token_name(14);
        let canonical = legacy.join(&canonical_name);
        let staged = legacy.join(staged_serve_token_name(&canonical_name, 15));
        fs::write(&canonical, b"canonical").expect("canonical token");
        let _socket = UnixListener::bind(&staged).expect("staged socket");

        let error = migrate_with_jira_preflight(&paths, |_| Ok(()))
            .expect_err("staged token socket must fail closed");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(&canonical).expect("canonical source"),
            b"canonical"
        );
        assert!(fs::symlink_metadata(&staged).is_ok());
        assert!(complete_receipt(legacy).is_none());
        assert!(!legacy.join(zerocode_lane::PATH_ACTIVATION_WITNESS).exists());
        assert!(
            !paths
                .target(PathClass::Config)
                .join(zerocode_lane::PATH_ACTIVATION_WITNESS)
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_account_symlink_blocks_retirement_without_following_or_partial_unlink() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let serve_token = legacy.join(serve_token_name(10));
        let claude = legacy.join(crate::accounts::MANAGED_ACCOUNTS_DIR);
        let outside = root.path().join("outside-secret");
        fs::write(&serve_token, b"token").expect("serve token");
        fs::create_dir_all(&claude).expect("managed root");
        fs::write(&outside, b"outside").expect("outside secret");
        std::os::unix::fs::symlink(&outside, claude.join("linked-secret")).expect("secret symlink");

        let error = retire_legacy_secrets(legacy).expect_err("symlink must fail closed");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(serve_token.is_file());
        assert_eq!(fs::read(outside).expect("outside value"), b"outside");
    }

    #[test]
    fn a_visible_but_unsynced_receipt_never_retires_source_credentials() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let claude = legacy.join(crate::accounts::MANAGED_ACCOUNTS_DIR);
        let serve_token = legacy.join(serve_token_name(10));
        fs::create_dir_all(&claude).expect("legacy credentials");
        fs::write(claude.join("secret"), b"held").expect("credential");
        fs::write(&serve_token, b"token").expect("serve token");
        write_pending_receipt(legacy);
        let pending = resolve_again(&paths);
        assert!(pending.migration_pending());
        let lock = PathMigrationLock::acquire(legacy).expect("migration lock");

        let error = activate_after_receipt(&pending, legacy, false, &lock)
            .expect_err("an unsynced receipt cannot activate writable platform state");
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(claude.join("secret").is_file());
        assert!(serve_token.is_file());
        assert!(resolve_again(&paths).migration_pending());

        let activated =
            activate_after_receipt(&pending, legacy, true, &lock).expect("durable retry");
        assert!(activated.platform_paths_active());
        assert!(!claude.exists());
        assert!(!serve_token.exists());
    }

    #[test]
    fn secret_cleanup_failure_keeps_both_authorities_non_writable() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy");
        let canonical = serve_token_name(10);
        let invalid = legacy.join(staged_serve_token_name(&canonical, 11));
        fs::create_dir_all(&invalid).expect("invalid token directory");
        write_pending_receipt(legacy);
        let pending = resolve_again(&paths);
        assert!(pending.migration_pending());
        let lock = PathMigrationLock::acquire(legacy).expect("migration lock");

        let error = activate_after_receipt(&pending, legacy, true, &lock)
            .expect_err("invalid secret inventory must block activation");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("migration is pending"));
        let restarted = resolve_again(&paths);
        assert!(restarted.migration_pending());
        assert_eq!(
            restarted
                .writable_root(PathClass::Config)
                .expect_err("pending config must not be writable")
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(invalid.is_dir());
    }

    #[test]
    fn platform_witness_never_reopens_or_cleans_a_restored_legacy_staging_tree() {
        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy").to_path_buf();
        let lock = PathMigrationLock::acquire(&legacy).expect("migration lock");
        let activated = paths
            .publish_platform_activation(&lock)
            .expect("platform witness");
        drop(lock);
        let restored = legacy.join(STAGING_DIRECTORY).join("restored-secret");
        fs::create_dir_all(restored.parent().expect("staging parent")).expect("restored staging");
        fs::write(&restored, b"unrelated restored bytes").expect("restored contents");

        let unchanged = migrate_if_needed(&activated).expect("platform remains authoritative");

        assert!(unchanged.platform_paths_active());
        assert_eq!(
            fs::read(&restored).expect("restored contents remain"),
            b"unrelated restored bytes"
        );

        let without_home = AppPaths::try_from_resolved(
            activated.target(PathClass::Config).to_path_buf(),
            activated.target(PathClass::LocalData).to_path_buf(),
            activated.target(PathClass::Cache).to_path_buf(),
            None,
        )
        .expect("target witness without HOME");
        assert!(
            migrate_if_needed(&without_home)
                .expect("a durable target witness needs no legacy discovery")
                .platform_paths_active()
        );
    }

    #[cfg(unix)]
    #[test]
    fn platform_witness_does_not_inspect_restored_legacy_file_symlink_or_permissions() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let root = tempfile::tempdir().expect("root");
        let paths = fixture(root.path());
        let legacy = paths.legacy_state().expect("legacy").to_path_buf();
        let lock = PathMigrationLock::acquire(&legacy).expect("migration lock");
        let activated = paths
            .publish_platform_activation(&lock)
            .expect("platform witness");
        drop(lock);

        fs::remove_dir_all(&legacy).expect("replace old legacy root");
        fs::write(&legacy, b"restored file").expect("restored file");
        let restored_file = resolve_again(&activated);
        migrate_if_needed(&restored_file).expect("ignore restored file");
        assert_eq!(
            fs::read(&legacy).expect("restored file remains"),
            b"restored file"
        );

        fs::remove_file(&legacy).expect("replace restored file");
        let outside = root.path().join("outside-restored");
        fs::write(&outside, b"outside remains").expect("outside contents");
        std::os::unix::fs::symlink(&outside, &legacy).expect("restored symlink");
        let restored_symlink = resolve_again(&activated);
        migrate_if_needed(&restored_symlink).expect("ignore restored symlink");
        assert!(
            fs::symlink_metadata(&legacy)
                .expect("symlink metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(&outside).expect("outside remains"),
            b"outside remains"
        );

        fs::remove_file(&legacy).expect("replace restored symlink");
        fs::create_dir(&legacy).expect("restored directory");
        let restored_contents = legacy.join("contents");
        fs::write(&restored_contents, b"permission protected").expect("protected contents");
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o000))
            .expect("protect restored directory");
        let protected_mode = fs::symlink_metadata(&legacy)
            .expect("protected metadata")
            .mode()
            & 0o777;
        let protected = resolve_again(&activated);
        migrate_if_needed(&protected).expect("ignore protected restored directory");
        assert_eq!(
            fs::symlink_metadata(&legacy)
                .expect("protected metadata remains")
                .mode()
                & 0o777,
            protected_mode
        );
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o700))
            .expect("restore cleanup permission");
        assert_eq!(
            fs::read(&restored_contents).expect("protected contents remain"),
            b"permission protected"
        );
    }
}
