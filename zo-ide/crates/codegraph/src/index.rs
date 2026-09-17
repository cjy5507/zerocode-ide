use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::extract::extract;
use crate::language::spec_for_path;
use crate::model::{
    ExtractedFile, FileFingerprint, IndexStatus, Reference, SkipReason, SkippedFile, Symbol,
    SymbolKind,
};

pub const MAX_INDEXABLE_FILE_SIZE: u64 = 5 * 1024 * 1024;
pub const MAX_INDEXED_FILES: usize = 50_000;
pub const DEFAULT_CACHE_FILE_NAME: &str = "index-v1.json";
const CACHE_SCHEMA_TAG: &str = "zo-codegraph-v1";
static CACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum CodeGraphError {
    #[error("codegraph I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("tree-sitter language `{language}` could not be loaded: {message}")]
    Language {
        language: &'static str,
        message: String,
    },
    #[error("tree-sitter {kind} query for `{language}` is invalid: {message}")]
    Query {
        language: &'static str,
        kind: &'static str,
        message: String,
    },
    #[error("tree-sitter did not return a syntax tree for {0}")]
    Parse(PathBuf),
    #[error("codegraph path must stay within the workspace: {0}")]
    InvalidPath(PathBuf),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FileRecord {
    fingerprint: FileFingerprint,
    content: FileContent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum FileContent {
    Indexed(ExtractedFile),
    Skipped(SkipReason),
}

#[derive(Debug, Deserialize, Serialize)]
struct CacheEnvelope {
    schema: String,
    workspace_root: PathBuf,
    files: BTreeMap<PathBuf, FileRecord>,
    file_limit_reached: bool,
}

#[derive(Clone, Copy)]
struct ScanEntry {
    fingerprint: FileFingerprint,
    too_large: bool,
}

struct WorkspaceScan {
    files: BTreeMap<PathBuf, ScanEntry>,
    file_limit_reached: bool,
}

/// A session-owned workspace index.
///
/// Callers normally keep one instance per tool session. [`Self::refresh`] is
/// the future hookup point for the host freshness watcher: v1 invokes it lazily
/// at query time and uses a metadata-only walk before doing any parsing work.
#[derive(Debug)]
pub struct CodeGraph {
    workspace_root: PathBuf,
    cache_path: PathBuf,
    files: BTreeMap<PathBuf, FileRecord>,
    file_limit_reached: bool,
}

impl CodeGraph {
    pub fn load_or_build(
        workspace_root: impl AsRef<Path>,
        cache_path: impl Into<PathBuf>,
    ) -> Result<Self, CodeGraphError> {
        let workspace_root = canonicalize(workspace_root.as_ref())?;
        let cache_path = cache_path.into();
        if let Some(cache) = load_cache(&cache_path, &workspace_root) {
            return Ok(Self {
                workspace_root,
                cache_path,
                files: cache.files,
                file_limit_reached: cache.file_limit_reached,
            });
        }

        let mut graph = Self {
            workspace_root,
            cache_path,
            files: BTreeMap::new(),
            file_limit_reached: false,
        };
        let scan = graph.scan_workspace();
        graph.rebuild_from_scan(&scan)?;
        graph.persist_best_effort();
        Ok(graph)
    }

    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Refresh changed files, rebuilding the parallel snapshot only when the
    /// cheap metadata walk detects added or removed supported files.
    pub fn refresh(&mut self) -> Result<(), CodeGraphError> {
        let scan = self.scan_workspace();
        let structural_change = self.files.keys().ne(scan.files.keys())
            || self.file_limit_reached != scan.file_limit_reached;
        if structural_change {
            self.rebuild_from_scan(&scan)?;
            self.persist_best_effort();
            return Ok(());
        }

        let changed = scan
            .files
            .iter()
            .filter(|(path, entry)| {
                self.files
                    .get(*path)
                    .is_none_or(|record| record.fingerprint != entry.fingerprint)
            })
            .map(|(path, entry)| (path.clone(), *entry))
            .collect::<Vec<_>>();
        if changed.is_empty() {
            return Ok(());
        }
        let records = self.extract_entries(&changed)?;
        self.files.extend(records);
        self.persist_best_effort();
        Ok(())
    }

    pub fn find_symbols(
        &mut self,
        name: &str,
        kind: Option<SymbolKind>,
    ) -> Result<Vec<Symbol>, CodeGraphError> {
        self.refresh()?;
        Ok(self
            .files
            .values()
            .filter_map(|record| match &record.content {
                FileContent::Indexed(file) => Some(file),
                FileContent::Skipped(_) => None,
            })
            .flat_map(|file| file.symbols.iter())
            .filter(|symbol| symbol.name == name && kind.is_none_or(|kind| symbol.kind == kind))
            .cloned()
            .collect())
    }

    pub fn find_references(&mut self, name: &str) -> Result<Vec<Reference>, CodeGraphError> {
        self.refresh()?;
        Ok(self
            .files
            .values()
            .filter_map(|record| match &record.content {
                FileContent::Indexed(file) => Some(file),
                FileContent::Skipped(_) => None,
            })
            .flat_map(|file| file.references.iter())
            .filter(|reference| reference.name == name)
            .cloned()
            .collect())
    }

    pub fn file_outline(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<Option<Vec<Symbol>>, CodeGraphError> {
        self.refresh()?;
        let path = self.relative_path(path.as_ref())?;
        Ok(self.files.get(&path).and_then(|record| match &record.content {
            FileContent::Indexed(file) => Some(file.symbols.clone()),
            FileContent::Skipped(_) => None,
        }))
    }

    #[must_use]
    pub fn skipped_files(&self) -> Vec<SkippedFile> {
        self.files
            .iter()
            .filter_map(|(path, record)| match &record.content {
                FileContent::Skipped(reason) => Some(SkippedFile {
                    file: path.clone(),
                    reason: reason.clone(),
                }),
                FileContent::Indexed(_) => None,
            })
            .collect()
    }

    #[must_use]
    pub fn status(&self) -> IndexStatus {
        let indexed_files = self
            .files
            .values()
            .filter(|record| matches!(record.content, FileContent::Indexed(_)))
            .count();
        IndexStatus {
            indexed_files,
            skipped_files: self.files.len() - indexed_files,
            file_limit_reached: self.file_limit_reached,
        }
    }

    fn scan_workspace(&self) -> WorkspaceScan {
        let mut files = BTreeMap::new();
        let mut file_limit_reached = false;
        let mut builder = WalkBuilder::new(&self.workspace_root);
        builder.require_git(false);
        builder.sort_by_file_path(Path::cmp);
        let walker = builder.build();
        for result in walker {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!("[codegraph] workspace walk skipped an entry: {error}");
                    continue;
                }
            };
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() || spec_for_path(entry.path()).is_none() {
                continue;
            }
            if files.len() == MAX_INDEXED_FILES {
                file_limit_reached = true;
                eprintln!(
                    "[codegraph] reached MAX_INDEXED_FILES ({MAX_INDEXED_FILES}); remaining supported files are not indexed"
                );
                break;
            }
            let relative = match entry.path().strip_prefix(&self.workspace_root) {
                Ok(relative) => relative.to_path_buf(),
                Err(_) => continue,
            };
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    eprintln!(
                        "[codegraph] metadata read failed for {}: {error}",
                        entry.path().display()
                    );
                    continue;
                }
            };
            let fingerprint = fingerprint(&metadata);
            files.insert(
                relative,
                ScanEntry {
                    fingerprint,
                    too_large: fingerprint.size > MAX_INDEXABLE_FILE_SIZE,
                },
            );
        }
        WorkspaceScan {
            files,
            file_limit_reached,
        }
    }

    fn rebuild_from_scan(&mut self, scan: &WorkspaceScan) -> Result<(), CodeGraphError> {
        let entries = scan
            .files
            .iter()
            .map(|(path, entry)| (path.clone(), *entry))
            .collect::<Vec<_>>();
        self.files = self.extract_entries(&entries)?.into_iter().collect();
        self.file_limit_reached = scan.file_limit_reached;
        Ok(())
    }

    fn extract_entries(
        &self,
        entries: &[(PathBuf, ScanEntry)],
    ) -> Result<Vec<(PathBuf, FileRecord)>, CodeGraphError> {
        entries
            .par_iter()
            .map(|(relative, entry)| {
                let record = self.extract_entry(relative, *entry)?;
                Ok((relative.clone(), record))
            })
            .collect()
    }

    fn extract_entry(
        &self,
        relative: &Path,
        entry: ScanEntry,
    ) -> Result<FileRecord, CodeGraphError> {
        if entry.too_large {
            return Ok(FileRecord {
                fingerprint: entry.fingerprint,
                content: FileContent::Skipped(SkipReason::TooLarge {
                    size: entry.fingerprint.size,
                    limit: MAX_INDEXABLE_FILE_SIZE,
                }),
            });
        }
        let absolute = self.workspace_root.join(relative);
        let source = match fs::read(&absolute) {
            Ok(source) => source,
            Err(error) => {
                return Ok(FileRecord {
                    fingerprint: entry.fingerprint,
                    content: FileContent::Skipped(SkipReason::ReadError {
                        message: error.to_string(),
                    }),
                });
            }
        };
        if source.contains(&0) || std::str::from_utf8(&source).is_err() {
            return Ok(FileRecord {
                fingerprint: entry.fingerprint,
                content: FileContent::Skipped(SkipReason::Binary),
            });
        }
        let Some(spec) = spec_for_path(relative) else {
            return Ok(FileRecord {
                fingerprint: entry.fingerprint,
                content: FileContent::Skipped(SkipReason::ParseError {
                    message: "supported language disappeared during extraction".to_string(),
                }),
            });
        };
        let content = extract(relative, &source, spec).map(FileContent::Indexed)?;
        Ok(FileRecord {
            fingerprint: entry.fingerprint,
            content,
        })
    }

    fn relative_path(&self, path: &Path) -> Result<PathBuf, CodeGraphError> {
        if path.is_absolute() {
            return path
                .strip_prefix(&self.workspace_root)
                .map(Path::to_path_buf)
                .map_err(|_| CodeGraphError::InvalidPath(path.to_path_buf()));
        }
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(CodeGraphError::InvalidPath(path.to_path_buf()));
        }
        Ok(path.to_path_buf())
    }

    fn persist_best_effort(&self) {
        if let Err(error) = self.persist() {
            eprintln!("[codegraph] cache write skipped: {error}");
        }
    }

    fn persist(&self) -> Result<(), CodeGraphError> {
        let envelope = CacheEnvelope {
            schema: CACHE_SCHEMA_TAG.to_string(),
            workspace_root: self.workspace_root.clone(),
            files: self.files.clone(),
            file_limit_reached: self.file_limit_reached,
        };
        let bytes = serde_json::to_vec(&envelope).map_err(|error| CodeGraphError::Io {
            path: self.cache_path.clone(),
            source: std::io::Error::other(error),
        })?;
        let Some(parent) = self.cache_path.parent() else {
            return Err(CodeGraphError::InvalidPath(self.cache_path.clone()));
        };
        fs::create_dir_all(parent).map_err(|source| CodeGraphError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let temporary = self
            .cache_path
            .with_extension(format!(
                "tmp-{}-{}",
                std::process::id(),
                CACHE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
        fs::write(&temporary, bytes).map_err(|source| CodeGraphError::Io {
            path: temporary.clone(),
            source,
        })?;
        if let Err(source) = replace_cache_file(&temporary, &self.cache_path) {
            let _ = fs::remove_file(&temporary);
            return Err(CodeGraphError::Io {
                path: self.cache_path.clone(),
                source,
            });
        }
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_cache_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_cache_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    match fs::remove_file(destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(temporary, destination)
}

fn canonicalize(path: &Path) -> Result<PathBuf, CodeGraphError> {
    fs::canonicalize(path).map_err(|source| CodeGraphError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn fingerprint(metadata: &fs::Metadata) -> FileFingerprint {
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    FileFingerprint {
        modified_nanos,
        size: metadata.len(),
    }
}

fn load_cache(path: &Path, workspace_root: &Path) -> Option<CacheEnvelope> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!("[codegraph] cache read failed; rebuilding: {error}");
            return None;
        }
    };
    let cache: CacheEnvelope = match serde_json::from_slice(&bytes) {
        Ok(cache) => cache,
        Err(error) => {
            eprintln!("[codegraph] cache is corrupt; rebuilding: {error}");
            return None;
        }
    };
    if cache.schema != CACHE_SCHEMA_TAG || cache.workspace_root != workspace_root {
        eprintln!("[codegraph] cache schema or workspace is stale; rebuilding");
        return None;
    }
    Some(cache)
}
