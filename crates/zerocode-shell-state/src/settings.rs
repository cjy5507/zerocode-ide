//! Durable, typed JSON settings persistence.
//!
//! This module deliberately owns only persistence mechanics. Callers own the
//! settings schema and domain validation, and credentials must use a separate
//! secret store. Each document is revisioned, written through a same-directory
//! temporary file, and protected by both a process mutex and an OS file lock.

use std::convert::Infallible;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

const DOCUMENT_FORMAT: u64 = 1;
pub const SETTINGS_BACKUP_LIMIT: usize = 5;
const BACKUP_MARKER: &str = ".backup-";
const CORRUPT_MARKER: &str = ".corrupt-";
const INTERRUPTED_REPLACEMENT_MARKER: &str = ".replace-";
pub const PATH_MIGRATION_SCHEMA: &str = "settings-transform-v1";
pub const PATH_MIGRATION_RECOVERY_MARKERS: &[&str] = &[
    BACKUP_MARKER,
    CORRUPT_MARKER,
    INTERRUPTED_REPLACEMENT_MARKER,
];

static PROCESS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static UNIQUE_FILE_ID: AtomicU64 = AtomicU64::new(1);

/// Whether `candidate` is durable recovery state belonging to `document`.
///
/// Path migration uses the same naming contract as repository recovery instead
/// of maintaining a second list of filename patterns. Temporary `.tmp-*`
/// writes are deliberately excluded: they were never committed recovery data.
pub fn is_repository_recovery_file(document: &str, candidate: &str) -> bool {
    [
        BACKUP_MARKER,
        CORRUPT_MARKER,
        INTERRUPTED_REPLACEMENT_MARKER,
    ]
    .into_iter()
    .any(|marker| candidate.starts_with(&format!(".{document}{marker}")))
}

/// A settings store rooted at a caller-selected directory.
///
/// Relative document names are resolved below `root`; absolute paths and
/// parent traversal are rejected. Repository clones intentionally share the
/// process-wide mutex and the root's cross-process lock file.
#[derive(Clone, Debug)]
pub struct SettingsRepository {
    root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsHealth {
    Missing,
    Healthy,
    Recovered {
        backup: PathBuf,
        quarantined_primary: PathBuf,
        invalid_backups: Vec<PathBuf>,
    },
}

#[derive(Debug)]
pub struct SettingsRead<T> {
    pub value: Option<T>,
    /// Zero means that no settings document exists yet.
    pub revision: u64,
    pub health: SettingsHealth,
}

#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct SettingsWrite<T> {
    /// The canonical value deserialized from the committed file.
    pub value: T,
    pub revision: u64,
    pub prior_health: SettingsHealth,
}

#[derive(Debug)]
pub struct SettingsMutation<T, R> {
    /// The canonical value deserialized from the committed file.
    pub value: T,
    pub result: R,
    pub revision: u64,
    pub prior_health: SettingsHealth,
}

#[derive(Debug)]
pub enum SettingsError {
    InvalidRelativePath(PathBuf),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    IncompatibleSchema {
        path: PathBuf,
        source: serde_json::Error,
    },
    InvalidDocument {
        path: PathBuf,
        reason: String,
    },
    UnsupportedFormat {
        path: PathBuf,
        format: u64,
    },
    RevisionOverflow(PathBuf),
    NoValidBackup {
        primary: PathBuf,
        quarantined_primary: PathBuf,
        invalid_backups: Vec<PathBuf>,
        primary_error: String,
    },
    #[cfg(test)]
    InterruptedBeforeReplace(PathBuf),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRelativePath(path) => write!(
                formatter,
                "settings path must be a non-empty relative path below the repository root: {}",
                path.display()
            ),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "could not {operation} settings path {}: {source}",
                path.display()
            ),
            Self::Json { path, source } => write!(
                formatter,
                "settings JSON at {} is invalid: {source}",
                path.display()
            ),
            Self::IncompatibleSchema { path, source } => write!(
                formatter,
                "settings data at {} cannot be decoded by this build: {source}",
                path.display()
            ),
            Self::InvalidDocument { path, reason } => write!(
                formatter,
                "settings document at {} is invalid: {reason}",
                path.display()
            ),
            Self::UnsupportedFormat { path, format } => write!(
                formatter,
                "settings document at {} uses unsupported format {format}",
                path.display()
            ),
            Self::RevisionOverflow(path) => write!(
                formatter,
                "settings revision at {} cannot be incremented",
                path.display()
            ),
            Self::NoValidBackup {
                primary,
                quarantined_primary,
                invalid_backups,
                primary_error,
            } => write!(
                formatter,
                "settings at {} were quarantined to {} after `{primary_error}`; no valid backup was found ({} invalid backup(s))",
                primary.display(),
                quarantined_primary.display(),
                invalid_backups.len()
            ),
            #[cfg(test)]
            Self::InterruptedBeforeReplace(path) => write!(
                formatter,
                "test interrupted the settings write before replacing {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::IncompatibleSchema { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl SettingsRepository {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The one directory this repository owns.
    ///
    /// Migration adapters use this boundary to read pre-repository files
    /// without consulting process-global home-directory state.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads and validates a typed document. A missing document is not an
    /// error. A corrupt primary is quarantined and the newest valid backup is
    /// restored before this method returns.
    pub fn read_json<T>(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<SettingsRead<T>, SettingsError>
    where
        T: DeserializeOwned,
    {
        let target = self.resolve(relative_path.as_ref())?;
        let _lock = self.lock()?;
        self.prepare_target(&target)?;

        match self.load_current::<T>(&target)? {
            Current::Missing => Ok(SettingsRead {
                value: None,
                revision: 0,
                health: SettingsHealth::Missing,
            }),
            Current::Found { document, health } => Ok(SettingsRead {
                value: Some(document.typed),
                revision: document.revision,
                health,
            }),
        }
    }

    /// Writes a typed value and returns the canonical readback. Object fields
    /// unknown to `T` are recursively retained from the previous document;
    /// known fields omitted by the new value are treated as deletions.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn write_json<T>(
        &self,
        relative_path: impl AsRef<Path>,
        value: &T,
    ) -> Result<SettingsWrite<T>, SettingsError>
    where
        T: Serialize + DeserializeOwned,
    {
        self.write_json_with_behavior(relative_path.as_ref(), value, CommitBehavior::Replace)
    }

    /// Serializes a read-modify-write transaction under one process and file
    /// lock. The default is evaluated only when the document does not exist.
    /// Unknown JSON object fields survive the typed mutation while removals
    /// made through `T` remain removed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn mutate_json<T, R, D, F>(
        &self,
        relative_path: impl AsRef<Path>,
        default: D,
        mutate: F,
    ) -> Result<SettingsMutation<T, R>, SettingsError>
    where
        T: Serialize + DeserializeOwned,
        D: FnOnce() -> T,
        F: FnOnce(&mut T) -> R,
    {
        match self.try_mutate_json(relative_path, default, |value| {
            Ok::<R, Infallible>(mutate(value))
        })? {
            Ok(committed) => Ok(committed),
            Err(never) => match never {},
        }
    }

    /// Fallible read-modify-write under the same lock. A domain validation
    /// error returns without creating a revision or touching the target.
    pub fn try_mutate_json<T, R, E, D, F>(
        &self,
        relative_path: impl AsRef<Path>,
        default: D,
        mutate: F,
    ) -> Result<Result<SettingsMutation<T, R>, E>, SettingsError>
    where
        T: Serialize + DeserializeOwned,
        D: FnOnce() -> T,
        F: FnOnce(&mut T) -> Result<R, E>,
    {
        let target = self.resolve(relative_path.as_ref())?;
        let _lock = self.lock()?;
        self.prepare_target(&target)?;
        let current = self.load_current::<T>(&target)?;

        let (mut value, prior_health) = match &current {
            Current::Missing => (default(), SettingsHealth::Missing),
            Current::Found { document, health } => (
                decode_typed(document.data.clone(), &target)?,
                health.clone(),
            ),
        };
        let result = match mutate(&mut value) {
            Ok(result) => result,
            Err(error) => return Ok(Err(error)),
        };
        let committed =
            self.commit(&target, current.document(), &value, CommitBehavior::Replace)?;

        Ok(Ok(SettingsMutation {
            value: committed.typed,
            result,
            revision: committed.revision,
            prior_health,
        }))
    }

    fn write_json_with_behavior<T>(
        &self,
        relative_path: &Path,
        value: &T,
        behavior: CommitBehavior,
    ) -> Result<SettingsWrite<T>, SettingsError>
    where
        T: Serialize + DeserializeOwned,
    {
        let target = self.resolve(relative_path)?;
        let _lock = self.lock()?;
        self.prepare_target(&target)?;
        let current = self.load_current::<T>(&target)?;
        let prior_health = current.health();
        let committed = self.commit(&target, current.document(), value, behavior)?;

        Ok(SettingsWrite {
            value: committed.typed,
            revision: committed.revision,
            prior_health,
        })
    }

    fn resolve(&self, relative_path: &Path) -> Result<PathBuf, SettingsError> {
        let valid = !relative_path.as_os_str().is_empty()
            && !relative_path.is_absolute()
            && relative_path
                .components()
                .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
        if !valid {
            return Err(SettingsError::InvalidRelativePath(
                relative_path.to_path_buf(),
            ));
        }
        Ok(self.root.join(relative_path))
    }

    fn lock(&self) -> Result<RepositoryLock, SettingsError> {
        crate::durable_file::ensure_private_directory(&self.root)
            .map_err(|source| io_error("create repository root", &self.root, source))?;

        let process = PROCESS_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let lock_path = self.root.join(".settings.lock");
        let file = crate::durable_file::private_lock_file(&lock_path)
            .map_err(|source| io_error("open repository lock", &lock_path, source))?;
        file.lock()
            .map_err(|source| io_error("lock repository", &lock_path, source))?;

        Ok(RepositoryLock {
            _process: process,
            _file: file,
        })
    }

    fn prepare_target(&self, target: &Path) -> Result<(), SettingsError> {
        let parent = target
            .parent()
            .ok_or_else(|| SettingsError::InvalidDocument {
                path: target.to_path_buf(),
                reason: "document has no parent directory".into(),
            })?;
        crate::durable_file::ensure_private_directory(parent)
            .map_err(|source| io_error("create settings directory", parent, source))?;
        cleanup_temporary_files(target)?;
        rotate_backups(target)?;
        Ok(())
    }

    fn load_current<T>(&self, target: &Path) -> Result<Current<T>, SettingsError>
    where
        T: DeserializeOwned,
    {
        if !target.exists() {
            return Ok(Current::Missing);
        }

        match read_document(target) {
            Ok(document) => Ok(Current::Found {
                document: Box::new(document),
                health: SettingsHealth::Healthy,
            }),
            Err(error) if error.is_corrupt_document() => self.recover(target, error),
            Err(error) => Err(error),
        }
    }

    fn recover<T>(
        &self,
        target: &Path,
        primary_error: SettingsError,
    ) -> Result<Current<T>, SettingsError>
    where
        T: DeserializeOwned,
    {
        let backups = backup_paths(target)?;
        // A recovery is itself a new persisted state. Advancing past the
        // greatest visible watermark prevents the frontend from rejecting the
        // recovered snapshot as stale. A structurally invalid envelope may
        // still contain a readable raw primary revision.
        let backup_revision = backups.first().map_or(0, |(revision, _)| *revision);
        let watermark = raw_positive_revision(target)
            .unwrap_or(0)
            .max(backup_revision);
        let recovery_revision = watermark
            .checked_add(1)
            .ok_or_else(|| SettingsError::RevisionOverflow(target.to_path_buf()))?;

        // All fallible watermark work happens before moving the primary. In
        // particular, an unadvanceable revision must leave the file in place.
        let quarantined_primary = quarantine(target)?;
        let mut invalid_backups = Vec::new();

        for (_, backup) in backups {
            match read_document::<T>(&backup) {
                Ok(_) => {
                    let document = restore_backup::<T>(&backup, target, recovery_revision)?;
                    return Ok(Current::Found {
                        document: Box::new(document),
                        health: SettingsHealth::Recovered {
                            backup,
                            quarantined_primary,
                            invalid_backups,
                        },
                    });
                }
                Err(error) if error.is_corrupt_document() => invalid_backups.push(backup),
                Err(error) => return Err(error),
            }
        }

        Err(SettingsError::NoValidBackup {
            primary: target.to_path_buf(),
            quarantined_primary,
            invalid_backups,
            primary_error: primary_error.to_string(),
        })
    }

    fn commit<T>(
        &self,
        target: &Path,
        previous: Option<&DecodedDocument<T>>,
        value: &T,
        behavior: CommitBehavior,
    ) -> Result<DecodedDocument<T>, SettingsError>
    where
        T: Serialize + DeserializeOwned,
    {
        // A failed recovery leaves the corrupt primary in quarantine. When a
        // caller subsequently seals a missing canonical document, advance
        // past that durable watermark so renderers that already observed it
        // do not reject the repaired snapshot as stale.
        let previous_revision = match previous {
            Some(document) => document.revision,
            None => missing_document_revision_floor(target)?,
        };
        let revision = previous_revision
            .checked_add(1)
            .ok_or_else(|| SettingsError::RevisionOverflow(target.to_path_buf()))?;
        let encoded = serde_json::to_value(value).map_err(|source| SettingsError::Json {
            path: target.to_path_buf(),
            source,
        })?;

        let data = match previous {
            Some(document) => {
                let known_before = serde_json::to_value(&document.typed).map_err(|source| {
                    SettingsError::Json {
                        path: target.to_path_buf(),
                        source,
                    }
                })?;
                merge_preserving_unknown(document.data.clone(), known_before, encoded)
            }
            None => encoded,
        };
        // Validate before touching disk, then validate the exact bytes again
        // from the temporary file and after the atomic replacement.
        let _: T = decode_typed(data.clone(), target)?;
        let root = document_value(previous.map(|document| &document.root), data, revision);
        let mut bytes = serde_json::to_vec_pretty(&root).map_err(|source| SettingsError::Json {
            path: target.to_path_buf(),
            source,
        })?;
        bytes.push(b'\n');

        let (temporary, mut file) = create_temporary_file(target, "primary")?;
        let mut temporary_guard = TemporaryGuard::new(temporary.clone());
        file.write_all(&bytes)
            .map_err(|source| io_error("write temporary settings", &temporary, source))?;
        file.sync_all()
            .map_err(|source| io_error("sync temporary settings", &temporary, source))?;
        drop(file);
        let staged = read_document::<T>(&temporary)?;
        if staged.revision != revision {
            return Err(SettingsError::InvalidDocument {
                path: temporary,
                reason: "temporary readback changed the revision".into(),
            });
        }

        if target.exists() {
            create_backup::<T>(target, previous_revision)?;
            rotate_backups(target)?;
        }

        #[cfg(test)]
        if matches!(behavior, CommitBehavior::InterruptBeforeReplace) {
            return Err(SettingsError::InterruptedBeforeReplace(
                target.to_path_buf(),
            ));
        }
        #[cfg(not(test))]
        let _ = behavior;

        let outcome = replace_file(&temporary, target)?;
        temporary_guard.disarm();
        report_visible_commit_durability(target, &outcome);

        let committed = read_document::<T>(target)?;
        if committed.revision != revision {
            return Err(SettingsError::InvalidDocument {
                path: target.to_path_buf(),
                reason: "committed readback changed the revision".into(),
            });
        }
        Ok(committed)
    }

    #[cfg(test)]
    fn write_json_interrupted_for_test<T>(
        &self,
        relative_path: impl AsRef<Path>,
        value: &T,
    ) -> Result<SettingsWrite<T>, SettingsError>
    where
        T: Serialize + DeserializeOwned,
    {
        self.write_json_with_behavior(
            relative_path.as_ref(),
            value,
            CommitBehavior::InterruptBeforeReplace,
        )
    }
}

impl SettingsError {
    fn is_corrupt_document(&self) -> bool {
        matches!(self, Self::Json { .. } | Self::InvalidDocument { .. })
    }
}

struct RepositoryLock {
    _process: MutexGuard<'static, ()>,
    _file: File,
}

enum CommitBehavior {
    Replace,
    #[cfg(test)]
    InterruptBeforeReplace,
}

struct DecodedDocument<T> {
    root: Map<String, Value>,
    data: Value,
    typed: T,
    revision: u64,
}

enum Current<T> {
    Missing,
    Found {
        document: Box<DecodedDocument<T>>,
        health: SettingsHealth,
    },
}

impl<T> Current<T> {
    fn document(&self) -> Option<&DecodedDocument<T>> {
        match self {
            Self::Missing => None,
            Self::Found { document, .. } => Some(document),
        }
    }

    fn health(&self) -> SettingsHealth {
        match self {
            Self::Missing => SettingsHealth::Missing,
            Self::Found { health, .. } => health.clone(),
        }
    }
}

struct TemporaryGuard {
    path: Option<PathBuf>,
}

impl TemporaryGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TemporaryGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn document_value(previous_root: Option<&Map<String, Value>>, data: Value, revision: u64) -> Value {
    let mut root = previous_root.cloned().unwrap_or_default();
    let mut metadata = root
        .remove("_meta")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    metadata.insert("format".into(), Value::from(DOCUMENT_FORMAT));
    metadata.insert("revision".into(), Value::from(revision));
    root.insert("_meta".into(), Value::Object(metadata));
    root.insert("data".into(), data);
    Value::Object(root)
}

/// Three-way merge of a raw document around a typed mutation.
///
/// A key absent from both typed values is unknown to this producer and must be
/// retained. A key present in `known_before` but absent from `next` was removed
/// deliberately (for example `Some` becoming `None`, or a map entry being
/// deleted) and must stay removed.
fn merge_preserving_unknown(previous: Value, known_before: Value, next: Value) -> Value {
    match (previous, next) {
        (Value::Object(mut previous), Value::Object(next)) => {
            let mut known_before = match known_before {
                Value::Object(known_before) => known_before,
                _ => Map::new(),
            };
            for key in known_before.keys() {
                if !next.contains_key(key) {
                    previous.remove(key);
                }
            }
            for (key, next_value) in next {
                let merged = match previous.remove(&key) {
                    Some(previous_value) => merge_preserving_unknown(
                        previous_value,
                        known_before.remove(&key).unwrap_or(Value::Null),
                        next_value,
                    ),
                    None => next_value,
                };
                previous.insert(key, merged);
            }
            Value::Object(previous)
        }
        (_, next) => next,
    }
}

fn read_document<T>(path: &Path) -> Result<DecodedDocument<T>, SettingsError>
where
    T: DeserializeOwned,
{
    let bytes = crate::durable_file::read_plain_file(path)
        .map_err(|source| io_error("read settings", path, source))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|source| SettingsError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    decode_document(value, path)
}

/// Reads only the revision watermark, without interpreting the typed payload.
/// Recovery is best-effort here: malformed JSON has no usable watermark and
/// falls back to the backup revisions.
fn raw_positive_revision(path: &Path) -> Option<u64> {
    let bytes = crate::durable_file::read_plain_file(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("_meta")?
        .get("revision")?
        .as_u64()
        .filter(|revision| *revision > 0)
}

fn decode_document<T>(value: Value, path: &Path) -> Result<DecodedDocument<T>, SettingsError>
where
    T: DeserializeOwned,
{
    let root = value
        .as_object()
        .cloned()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: path.to_path_buf(),
            reason: "root must be a JSON object".into(),
        })?;
    let metadata = root
        .get("_meta")
        .and_then(Value::as_object)
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: path.to_path_buf(),
            reason: "missing object `_meta`".into(),
        })?;
    let format = metadata
        .get("format")
        .and_then(Value::as_u64)
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: path.to_path_buf(),
            reason: "missing numeric `_meta.format`".into(),
        })?;
    if format != DOCUMENT_FORMAT {
        return Err(SettingsError::UnsupportedFormat {
            path: path.to_path_buf(),
            format,
        });
    }
    let revision = metadata
        .get("revision")
        .and_then(Value::as_u64)
        .filter(|revision| *revision > 0)
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: path.to_path_buf(),
            reason: "`_meta.revision` must be a positive integer".into(),
        })?;
    let data = root
        .get("data")
        .cloned()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: path.to_path_buf(),
            reason: "missing `data`".into(),
        })?;
    let typed = decode_typed(data.clone(), path)?;

    Ok(DecodedDocument {
        root,
        data,
        typed,
        revision,
    })
}

fn decode_typed<T>(value: Value, path: &Path) -> Result<T, SettingsError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value).map_err(|source| SettingsError::IncompatibleSchema {
        path: path.to_path_buf(),
        source,
    })
}

fn restore_backup<T>(
    backup: &Path,
    target: &Path,
    recovery_revision: u64,
) -> Result<DecodedDocument<T>, SettingsError>
where
    T: DeserializeOwned,
{
    let recovered = read_document::<T>(backup)?;
    let value = document_value(Some(&recovered.root), recovered.data, recovery_revision);
    let mut bytes = serde_json::to_vec_pretty(&value).map_err(|source| SettingsError::Json {
        path: backup.to_path_buf(),
        source,
    })?;
    bytes.push(b'\n');

    let (temporary, mut file) = create_temporary_file(target, "recovery")?;
    let mut guard = TemporaryGuard::new(temporary.clone());
    file.write_all(&bytes)
        .map_err(|error| io_error("write recovered settings", &temporary, error))?;
    file.sync_all()
        .map_err(|error| io_error("sync recovered settings", &temporary, error))?;
    drop(file);
    let _: DecodedDocument<T> = read_document(&temporary)?;
    let outcome = replace_file(&temporary, target)?;
    guard.disarm();
    report_visible_commit_durability(target, &outcome);
    read_document(target)
}

fn create_backup<T>(target: &Path, revision: u64) -> Result<(), SettingsError>
where
    T: DeserializeOwned,
{
    if revision == 0 {
        return Ok(());
    }
    let backup = backup_path(target, revision)?;
    if backup.exists() {
        match read_document::<T>(&backup) {
            Ok(document) if document.revision == revision => return Ok(()),
            _ => {
                let _ = quarantine(&backup)?;
            }
        }
    }

    let bytes = crate::durable_file::read_plain_file(target)
        .map_err(|source| io_error("read settings for backup", target, source))?;

    let (temporary, mut file) = create_temporary_file(target, "backup")?;
    let mut guard = TemporaryGuard::new(temporary.clone());
    file.write_all(&bytes)
        .map_err(|source| io_error("write settings backup", &temporary, source))?;
    file.sync_all()
        .map_err(|source| io_error("sync settings backup", &temporary, source))?;
    drop(file);
    let staged = read_document::<T>(&temporary)?;
    if staged.revision != revision {
        return Err(SettingsError::InvalidDocument {
            path: temporary,
            reason: format!(
                "backup revision {} did not match expected revision {revision}",
                staged.revision
            ),
        });
    }
    fs::rename(&temporary, &backup)
        .map_err(|source| io_error("install settings backup", &backup, source))?;
    guard.disarm();
    sync_parent_directory(&backup)
}

fn create_temporary_file(target: &Path, role: &str) -> Result<(PathBuf, File), SettingsError> {
    let parent = target
        .parent()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document has no parent directory".into(),
        })?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document file name is not valid UTF-8".into(),
        })?;

    loop {
        let id = UNIQUE_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{file_name}.tmp-{role}-{}-{id}",
            std::process::id()
        ));
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
            Err(source) => return Err(io_error("create temporary settings", &path, source)),
        }
    }
}

fn replace_file(
    temporary: &Path,
    target: &Path,
) -> Result<crate::durable_file::CommitOutcome, SettingsError> {
    crate::durable_file::replace_staged(temporary, target)
        .map_err(|source| io_error("replace settings atomically", target, source))
}

fn report_visible_commit_durability(target: &Path, outcome: &crate::durable_file::CommitOutcome) {
    if let crate::durable_file::Durability::VisibleButSyncFailed(error) = &outcome.durability {
        eprintln!(
            "zerocode-shell: settings replacement at {} is visible but directory sync failed: {error}",
            target.display()
        );
    }
}

#[cfg(test)]
fn replacement_backup_path(target: &Path) -> Result<PathBuf, SettingsError> {
    let parent = target
        .parent()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document has no parent directory".into(),
        })?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document file name is not valid UTF-8".into(),
        })?;
    loop {
        let id = UNIQUE_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{name}{INTERRUPTED_REPLACEMENT_MARKER}{}-{id}",
            std::process::id()
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
}

fn restore_interrupted_replacement(target: &Path) -> Result<(), SettingsError> {
    let parent = target
        .parent()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document has no parent directory".into(),
        })?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document file name is not valid UTF-8".into(),
        })?;
    let prefix = format!(".{name}{INTERRUPTED_REPLACEMENT_MARKER}");
    let mut displaced: Vec<PathBuf> = fs::read_dir(parent)
        .map_err(|source| io_error("scan interrupted settings replacements", parent, source))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect();
    displaced.sort();
    if !target.exists()
        && let Some(latest) = displaced.pop()
    {
        fs::rename(&latest, target).map_err(|source| {
            io_error("restore interrupted settings replacement", target, source)
        })?;
        sync_parent_directory(target)?;
    }
    for stale in displaced {
        let _ = fs::remove_file(stale);
    }
    Ok(())
}

fn cleanup_temporary_files(target: &Path) -> Result<(), SettingsError> {
    restore_interrupted_replacement(target)?;
    let parent = target
        .parent()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document has no parent directory".into(),
        })?;
    let prefix = temporary_prefix(target)?;
    let entries = fs::read_dir(parent)
        .map_err(|source| io_error("scan settings directory", parent, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| io_error("scan settings directory", parent, source))?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let path = entry.path();
            fs::remove_file(&path)
                .map_err(|source| io_error("remove stale settings temporary", &path, source))?;
        }
    }
    Ok(())
}

fn quarantine(path: &Path) -> Result<PathBuf, SettingsError> {
    let parent = path
        .parent()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: path.to_path_buf(),
            reason: "document has no parent directory".into(),
        })?;
    let prefix = quarantine_prefix(path)?;
    loop {
        let id = UNIQUE_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let quarantined = parent.join(format!("{prefix}{}-{id}", std::process::id()));
        match fs::rename(path, &quarantined) {
            Ok(()) => {
                sync_parent_directory(&quarantined)?;
                return Ok(quarantined);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(io_error("quarantine corrupt settings", path, source)),
        }
    }
}

fn missing_document_revision_floor(target: &Path) -> Result<u64, SettingsError> {
    let backup = backup_paths(target)?
        .first()
        .map_or(0, |(revision, _)| *revision);
    let quarantined = quarantined_document_paths(target)?
        .into_iter()
        .filter_map(|path| raw_positive_revision(&path))
        .max()
        .unwrap_or(0);
    Ok(backup.max(quarantined))
}

fn quarantined_document_paths(target: &Path) -> Result<Vec<PathBuf>, SettingsError> {
    let parent = target
        .parent()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document has no parent directory".into(),
        })?;
    let prefix = quarantine_prefix(target)?;
    let entries = fs::read_dir(parent)
        .map_err(|source| io_error("scan settings directory", parent, source))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| io_error("scan settings directory", parent, source))?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

fn quarantine_prefix(target: &Path) -> Result<String, SettingsError> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document file name is not valid UTF-8".into(),
        })?;
    Ok(format!(".{file_name}{CORRUPT_MARKER}"))
}

fn rotate_backups(target: &Path) -> Result<(), SettingsError> {
    for (_, path) in backup_paths(target)?
        .into_iter()
        .skip(SETTINGS_BACKUP_LIMIT)
    {
        fs::remove_file(&path)
            .map_err(|source| io_error("remove old settings backup", &path, source))?;
    }
    Ok(())
}

fn backup_paths(target: &Path) -> Result<Vec<(u64, PathBuf)>, SettingsError> {
    let parent = target
        .parent()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document has no parent directory".into(),
        })?;
    let prefix = backup_prefix(target)?;
    let mut paths = Vec::new();
    let entries =
        fs::read_dir(parent).map_err(|source| io_error("scan settings backups", parent, source))?;
    for entry in entries {
        let entry = entry.map_err(|source| io_error("scan settings backups", parent, source))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(revision) = name
            .strip_prefix(&prefix)
            .and_then(|tail| tail.strip_suffix(".json"))
            .and_then(|digits| digits.parse::<u64>().ok())
        else {
            continue;
        };
        paths.push((revision, entry.path()));
    }
    paths.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(paths)
}

fn backup_path(target: &Path, revision: u64) -> Result<PathBuf, SettingsError> {
    let parent = target
        .parent()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document has no parent directory".into(),
        })?;
    Ok(parent.join(format!("{}{revision:020}.json", backup_prefix(target)?)))
}

fn backup_prefix(target: &Path) -> Result<String, SettingsError> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document file name is not valid UTF-8".into(),
        })?;
    Ok(format!(".{file_name}{BACKUP_MARKER}"))
}

fn temporary_prefix(target: &Path) -> Result<String, SettingsError> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: target.to_path_buf(),
            reason: "document file name is not valid UTF-8".into(),
        })?;
    Ok(format!(".{file_name}.tmp-"))
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), SettingsError> {
    let parent = path
        .parent()
        .ok_or_else(|| SettingsError::InvalidDocument {
            path: path.to_path_buf(),
            reason: "document has no parent directory".into(),
        })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error("sync settings directory", parent, source))
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<(), SettingsError> {
    Ok(())
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> SettingsError {
    SettingsError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    const SETTINGS: &str = "nested/settings.json";

    #[derive(Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
    struct Pair {
        left: u64,
        right: u64,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "kebab-case")]
    enum SearchEngine {
        Google,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
    struct BrowserSettings {
        search_engine: SearchEngine,
        enabled: bool,
    }

    #[derive(Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
    struct ProjectRow {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        local_setup: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        local_archive: Option<String>,
    }

    #[test]
    fn restart_roundtrip_keeps_revision_and_unknown_fields() {
        let directory = tempdir().unwrap();
        let first = SettingsRepository::new(directory.path());
        let written = first
            .write_json(SETTINGS, &Pair { left: 3, right: 5 })
            .unwrap();
        assert_eq!(written.value, Pair { left: 3, right: 5 });
        assert_eq!(written.revision, 1);
        assert_eq!(written.prior_health, SettingsHealth::Missing);
        drop(first);

        // Simulate a newer producer adding fields this version does not know.
        let target = directory.path().join(SETTINGS);
        let mut raw: Value = serde_json::from_reader(File::open(&target).unwrap()).unwrap();
        raw["data"]["future"] = json!({ "nested": true });
        raw["future_envelope"] = json!("kept");
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&target)
            .unwrap();
        serde_json::to_writer_pretty(&mut file, &raw).unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();

        let restarted = SettingsRepository::new(directory.path());
        let read = restarted.read_json::<Pair>(SETTINGS).unwrap();
        assert_eq!(read.value, Some(Pair { left: 3, right: 5 }));
        assert_eq!(read.revision, 1);
        assert_eq!(read.health, SettingsHealth::Healthy);

        restarted
            .mutate_json(SETTINGS, Pair::default, |settings| settings.left = 8)
            .unwrap();
        let persisted: Value = serde_json::from_reader(File::open(target).unwrap()).unwrap();
        assert_eq!(persisted["data"]["future"]["nested"], true);
        assert_eq!(persisted["future_envelope"], "kept");
    }

    #[test]
    fn fallible_mutation_rejection_is_byte_identical_and_the_delegate_keeps_metadata() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        let first = repository
            .mutate_json(SETTINGS, Pair::default, |pair| {
                pair.left = 3;
                "committed"
            })
            .unwrap();
        assert_eq!(first.result, "committed");
        assert_eq!(first.revision, 1);
        assert_eq!(first.prior_health, SettingsHealth::Missing);

        let target = directory.path().join(SETTINGS);
        let before = fs::read(&target).unwrap();
        let refused = repository
            .try_mutate_json(SETTINGS, Pair::default, |pair| {
                pair.right = 99;
                Err::<(), _>("domain refusal")
            })
            .unwrap();
        assert!(matches!(refused, Err("domain refusal")));
        assert_eq!(fs::read(&target).unwrap(), before);

        let second = repository
            .mutate_json(SETTINGS, Pair::default, |pair| {
                pair.right = 5;
                pair.left + pair.right
            })
            .unwrap();
        assert_eq!(second.value, Pair { left: 3, right: 5 });
        assert_eq!(second.result, 8);
        assert_eq!(second.revision, 2);
        assert_eq!(second.prior_health, SettingsHealth::Healthy);
    }

    #[test]
    fn clearing_an_optional_project_field_does_not_resurrect_it() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        let project = "/project/a".to_string();
        repository
            .write_json(
                SETTINGS,
                &BTreeMap::from([(
                    project.clone(),
                    ProjectRow {
                        local_setup: Some("setup-a".into()),
                        local_archive: Some("archive-a".into()),
                    },
                )]),
            )
            .unwrap();

        let target = directory.path().join(SETTINGS);
        let mut raw: Value = serde_json::from_reader(File::open(&target).unwrap()).unwrap();
        raw["data"][&project]["future"] = json!({ "nested": true });
        raw["future_envelope"] = json!("kept");
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&target)
            .unwrap();
        serde_json::to_writer_pretty(&mut file, &raw).unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();

        repository
            .mutate_json(SETTINGS, BTreeMap::<String, ProjectRow>::new, |projects| {
                projects.get_mut(&project).unwrap().local_setup = None;
            })
            .unwrap();

        let persisted: Value = serde_json::from_reader(File::open(target).unwrap()).unwrap();
        assert!(persisted["data"][&project].get("local_setup").is_none());
        assert_eq!(persisted["data"][&project]["local_archive"], "archive-a");
        assert_eq!(persisted["data"][&project]["future"]["nested"], true);
        assert_eq!(persisted["future_envelope"], "kept");
    }

    #[test]
    fn removing_a_project_entry_does_not_resurrect_it() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        let removed = "/project/removed".to_string();
        let kept = "/project/kept".to_string();
        repository
            .write_json(
                SETTINGS,
                &BTreeMap::from([
                    (
                        removed.clone(),
                        ProjectRow {
                            local_setup: Some("remove-me".into()),
                            ..ProjectRow::default()
                        },
                    ),
                    (
                        kept.clone(),
                        ProjectRow {
                            local_archive: Some("keep-me".into()),
                            ..ProjectRow::default()
                        },
                    ),
                ]),
            )
            .unwrap();

        let target = directory.path().join(SETTINGS);
        let mut raw: Value = serde_json::from_reader(File::open(&target).unwrap()).unwrap();
        raw["data"][&kept]["future"] = json!({ "nested": true });
        raw["future_envelope"] = json!("kept");
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&target)
            .unwrap();
        serde_json::to_writer_pretty(&mut file, &raw).unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();

        repository
            .mutate_json(SETTINGS, BTreeMap::<String, ProjectRow>::new, |projects| {
                projects.remove(&removed);
            })
            .unwrap();

        let persisted: Value = serde_json::from_reader(File::open(target).unwrap()).unwrap();
        assert!(persisted["data"].get(&removed).is_none());
        assert_eq!(persisted["data"][&kept]["local_archive"], "keep-me");
        assert_eq!(persisted["data"][&kept]["future"]["nested"], true);
        assert_eq!(persisted["future_envelope"], "kept");
    }

    #[test]
    fn interruption_before_replace_keeps_previous_document() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        repository
            .write_json(SETTINGS, &Pair { left: 1, right: 2 })
            .unwrap();

        let error = repository
            .write_json_interrupted_for_test(SETTINGS, &Pair { left: 9, right: 9 })
            .unwrap_err();
        assert!(matches!(error, SettingsError::InterruptedBeforeReplace(_)));
        let read = repository.read_json::<Pair>(SETTINGS).unwrap();
        assert_eq!(read.value, Some(Pair { left: 1, right: 2 }));
        assert_eq!(read.revision, 1);
        assert!(temporary_paths(&repository, SETTINGS).is_empty());
    }

    #[test]
    fn corrupt_primary_is_quarantined_and_latest_valid_backup_is_restored() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        repository
            .write_json(SETTINGS, &Pair { left: 1, right: 0 })
            .unwrap();
        repository
            .write_json(SETTINGS, &Pair { left: 2, right: 0 })
            .unwrap();

        let primary = directory.path().join(SETTINGS);
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&primary)
            .unwrap();
        file.write_all(b"{ definitely not JSON").unwrap();
        file.sync_all().unwrap();

        let recovered = repository.read_json::<Pair>(SETTINGS).unwrap();
        assert_eq!(recovered.value, Some(Pair { left: 1, right: 0 }));
        assert_eq!(recovered.revision, 2);
        let SettingsHealth::Recovered {
            backup,
            quarantined_primary,
            invalid_backups,
        } = recovered.health
        else {
            panic!("expected a recovery health report");
        };
        assert!(backup.exists());
        assert!(quarantined_primary.exists());
        assert!(invalid_backups.is_empty());
        assert_eq!(
            repository.read_json::<Pair>(SETTINGS).unwrap().health,
            SettingsHealth::Healthy
        );
    }

    #[test]
    fn recovery_advances_past_a_structurally_invalid_primary_revision() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        repository
            .write_json(SETTINGS, &Pair { left: 1, right: 2 })
            .unwrap();
        let primary = directory.path().join(SETTINGS);
        write_document_for_test(
            &backup_path(&primary, 5).unwrap(),
            document_value(None, json!({ "left": 5, "right": 8 }), 5),
        );
        let mut invalid = document_value(None, json!({ "left": 100, "right": 0 }), 100);
        invalid.as_object_mut().unwrap().remove("data");
        write_document_for_test(&primary, invalid);

        let recovered = repository.read_json::<Pair>(SETTINGS).unwrap();

        assert_eq!(recovered.value, Some(Pair { left: 5, right: 8 }));
        assert_eq!(recovered.revision, 101);
        assert!(matches!(recovered.health, SettingsHealth::Recovered { .. }));
    }

    #[test]
    fn a_future_format_is_left_byte_identical_for_a_newer_producer() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        repository
            .write_json(SETTINGS, &Pair { left: 3, right: 4 })
            .unwrap();
        let primary = directory.path().join(SETTINGS);
        let future = document_value(None, json!({ "left": 3, "right": 4 }), 7);
        let mut future = future.as_object().unwrap().clone();
        future["_meta"]["format"] = json!(DOCUMENT_FORMAT + 1);
        write_document_for_test(&primary, Value::Object(future));
        let before = fs::read(&primary).unwrap();

        let error = repository.read_json::<Pair>(SETTINGS).unwrap_err();

        assert!(matches!(
            error,
            SettingsError::UnsupportedFormat { format, .. }
                if format == DOCUMENT_FORMAT + 1
        ));
        assert_eq!(fs::read(&primary).unwrap(), before);
        assert!(quarantined_paths(&primary).is_empty());
    }

    #[test]
    fn a_future_typed_value_is_left_byte_identical_for_a_newer_producer() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        repository
            .write_json(
                SETTINGS,
                &BrowserSettings {
                    search_engine: SearchEngine::Google,
                    enabled: false,
                },
            )
            .unwrap();
        let primary = directory.path().join(SETTINGS);
        write_document_for_test(
            &primary,
            document_value(
                None,
                json!({ "search_engine": "future-engine", "enabled": true }),
                7,
            ),
        );
        let before = fs::read(&primary).unwrap();

        let read_error = repository
            .read_json::<BrowserSettings>(SETTINGS)
            .unwrap_err();
        assert!(matches!(
            read_error,
            SettingsError::IncompatibleSchema { .. }
        ));
        let mutate_error = repository
            .mutate_json(
                SETTINGS,
                || BrowserSettings {
                    search_engine: SearchEngine::Google,
                    enabled: false,
                },
                |settings| settings.enabled = false,
            )
            .unwrap_err();

        assert!(matches!(
            mutate_error,
            SettingsError::IncompatibleSchema { .. }
        ));
        assert_eq!(fs::read(&primary).unwrap(), before);
        assert!(quarantined_paths(&primary).is_empty());
    }

    #[test]
    fn sealing_after_failed_recovery_advances_past_the_quarantined_revision() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        let primary = directory.path().join(SETTINGS);
        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        let mut invalid = document_value(None, json!({ "left": 1, "right": 2 }), 100);
        invalid.as_object_mut().unwrap().remove("data");
        write_document_for_test(&primary, invalid);

        let error = repository.read_json::<Pair>(SETTINGS).unwrap_err();
        assert!(matches!(error, SettingsError::NoValidBackup { .. }));

        let sealed = repository
            .mutate_json(SETTINGS, Pair::default, |_| {})
            .unwrap();
        assert_eq!(sealed.revision, 101);
        assert_eq!(sealed.value, Pair::default());
    }

    #[test]
    fn an_unadvanceable_primary_revision_is_not_quarantined() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        repository
            .write_json(SETTINGS, &Pair { left: 1, right: 2 })
            .unwrap();
        let primary = directory.path().join(SETTINGS);
        write_document_for_test(
            &backup_path(&primary, 5).unwrap(),
            document_value(None, json!({ "left": 5, "right": 8 }), 5),
        );
        let mut invalid = document_value(None, json!({ "left": 1, "right": 0 }), u64::MAX);
        invalid.as_object_mut().unwrap().remove("data");
        write_document_for_test(&primary, invalid);
        let before = fs::read(&primary).unwrap();

        let error = repository.read_json::<Pair>(SETTINGS).unwrap_err();

        assert!(matches!(error, SettingsError::RevisionOverflow(path) if path == primary));
        assert_eq!(fs::read(&primary).unwrap(), before);
        assert!(quarantined_paths(&primary).is_empty());
    }

    #[test]
    fn concurrent_disjoint_mutations_do_not_lose_updates() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        repository.write_json(SETTINGS, &Pair::default()).unwrap();

        let barrier = Arc::new(Barrier::new(3));
        let left_repository = repository.clone();
        let left_barrier = Arc::clone(&barrier);
        let left = thread::spawn(move || {
            left_barrier.wait();
            left_repository
                .mutate_json(SETTINGS, Pair::default, |settings| settings.left += 1)
                .unwrap();
        });
        let right_repository = repository.clone();
        let right_barrier = Arc::clone(&barrier);
        let right = thread::spawn(move || {
            right_barrier.wait();
            right_repository
                .mutate_json(SETTINGS, Pair::default, |settings| settings.right += 1)
                .unwrap();
        });
        barrier.wait();
        left.join().unwrap();
        right.join().unwrap();

        let read = repository.read_json::<Pair>(SETTINGS).unwrap();
        assert_eq!(read.value, Some(Pair { left: 1, right: 1 }));
        assert_eq!(read.revision, 3);
    }

    #[test]
    fn stale_temporaries_are_cleaned_and_backups_are_bounded() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        let target = directory.path().join(SETTINGS);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let stale = target
            .parent()
            .unwrap()
            .join(format!("{}stale", temporary_prefix(&target).unwrap()));
        File::create(&stale).unwrap();

        for value in 0..10 {
            repository
                .write_json(
                    SETTINGS,
                    &Pair {
                        left: value,
                        right: value,
                    },
                )
                .unwrap();
        }

        assert!(!stale.exists());
        assert!(temporary_paths(&repository, SETTINGS).is_empty());
        assert_eq!(backup_paths(&target).unwrap().len(), SETTINGS_BACKUP_LIMIT);
    }

    #[test]
    fn an_interrupted_windows_style_replace_restores_the_displaced_primary() {
        let directory = tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path());
        repository
            .write_json(SETTINGS, &Pair { left: 7, right: 9 })
            .unwrap();
        let target = directory.path().join(SETTINGS);
        let displaced = replacement_backup_path(&target).unwrap();
        fs::rename(&target, &displaced).unwrap();

        let read = repository.read_json::<Pair>(SETTINGS).unwrap();
        assert_eq!(read.value, Some(Pair { left: 7, right: 9 }));
        assert!(target.exists());
        assert!(!displaced.exists());
    }

    #[test]
    fn repository_recovery_filename_contract_has_one_owner() {
        for name in [
            ".preferences.json.backup-00000000000000000001.json",
            ".preferences.json.corrupt-1-1",
            ".preferences.json.replace-1-1",
        ] {
            assert!(is_repository_recovery_file("preferences.json", name));
        }
        assert!(!is_repository_recovery_file(
            "preferences.json",
            ".preferences.json.tmp-primary-1-1"
        ));
        assert!(!is_repository_recovery_file(
            "project-settings.json",
            ".preferences.json.replace-1-1"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn repository_tree_and_every_persisted_file_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempdir().unwrap();
        let root = directory.path().join("private-settings");
        let repository = SettingsRepository::new(&root);
        repository
            .write_json(SETTINGS, &Pair { left: 1, right: 2 })
            .unwrap();
        repository
            .write_json(SETTINGS, &Pair { left: 3, right: 4 })
            .unwrap();

        let target = root.join(SETTINGS);
        let backup = backup_paths(&target).unwrap().pop().expect("backup").1;
        for directory in [&root, target.parent().unwrap()] {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for file in [root.join(".settings.lock"), target, backup] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn repository_lock_symlink_never_touches_the_external_file() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let directory = tempdir().unwrap();
        let root = directory.path().join("settings");
        fs::create_dir(&root).unwrap();
        let external = directory.path().join("external-lock");
        fs::write(&external, b"external lock").unwrap();
        fs::set_permissions(&external, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&external, root.join(".settings.lock")).unwrap();

        let error = SettingsRepository::new(&root)
            .read_json::<Pair>(SETTINGS)
            .expect_err("the lock leaf must fail closed");

        match error {
            SettingsError::Io { source, .. } => {
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(fs::read(&external).unwrap(), b"external lock");
        assert_eq!(
            fs::metadata(&external).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn canonical_settings_symlink_is_not_read_or_quarantined() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let directory = tempdir().unwrap();
        let root = directory.path().join("settings");
        let target = root.join(SETTINGS);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let external = directory.path().join("external-settings");
        fs::write(&external, b"external settings").unwrap();
        fs::set_permissions(&external, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&external, &target).unwrap();

        let error = SettingsRepository::new(&root)
            .read_json::<Pair>(SETTINGS)
            .expect_err("the canonical leaf must fail closed");

        match error {
            SettingsError::Io { source, .. } => {
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert!(
            fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&external).unwrap(), b"external settings");
        assert_eq!(
            fs::metadata(&external).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    fn temporary_paths(repository: &SettingsRepository, relative: &str) -> Vec<PathBuf> {
        let target = repository.resolve(Path::new(relative)).unwrap();
        let parent = target.parent().unwrap();
        let prefix = temporary_prefix(&target).unwrap();
        fs::read_dir(parent)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect()
    }

    fn write_document_for_test(path: &Path, value: Value) {
        let mut bytes = serde_json::to_vec_pretty(&value).unwrap();
        bytes.push(b'\n');
        fs::write(path, bytes).unwrap();
        File::open(path).unwrap().sync_all().unwrap();
    }

    fn quarantined_paths(target: &Path) -> Vec<PathBuf> {
        let prefix = quarantine_prefix(target).expect("quarantine prefix");
        fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect()
    }
}
