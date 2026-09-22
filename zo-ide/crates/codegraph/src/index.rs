use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;

use rayon::prelude::*;
use thiserror::Error;

use crate::extract::extract;
use crate::language::spec_for_path;
use crate::model::{
    FileFingerprint, FileLinks, Impact, Import, IndexStatus, LinkedFile, Reference, Resolved,
    SkipReason, SkippedFile, Symbol, SymbolKind,
};
use crate::positions::{pack_by_name, PackedName};
use crate::scan::{fingerprint, scan_workspace, ScanEntry};
use crate::store::Store;

pub const MAX_INDEXABLE_FILE_SIZE: u64 = 5 * 1024 * 1024;
pub const MAX_INDEXED_FILES: usize = 50_000;
pub const DEFAULT_CACHE_FILE_NAME: &str = "index-v2.sqlite";
/// What earlier versions of this crate wrote where [`DEFAULT_CACHE_FILE_NAME`]
/// now lives. Nothing reads them any more; the v1 JSON was 352 MB on this
/// repository, so it is removed rather than left to sit.
pub(crate) const LEGACY_CACHE_FILE_NAMES: [&str; 1] = ["index-v1.json"];
/// Between a Rust path's segments in a symbol mention (`Type::name`).
const MENTION_PATH_SEPARATOR: &str = "::";

/// Files a first build extracts at a time: parallel within a chunk, and the
/// next chunk is parsed while this one is written, so at most three chunks'
/// facts are held at once.
const BUILD_CHUNK_FILES: usize = 128;

#[derive(Debug, Error)]
pub enum CodeGraphError {
    #[error("codegraph I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("codegraph index error at {path}: {source}")]
    Index {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("codegraph index at {path} holds unreadable {what}")]
    Corrupt { path: PathBuf, what: &'static str },
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

/// One file as extraction leaves it, before the index stores it.
#[derive(Clone, Debug)]
pub(crate) struct FileRecord {
    pub(crate) fingerprint: FileFingerprint,
    pub(crate) content: FileContent,
}

#[derive(Clone, Debug)]
pub(crate) enum FileContent {
    Indexed(IndexedFile),
    Skipped(SkipReason),
}

/// A parsed file as the index stores it: definitions and imports as
/// extracted, references already grouped by name and packed.
#[derive(Clone, Debug)]
pub(crate) struct IndexedFile {
    pub(crate) language: String,
    pub(crate) symbols: Vec<Symbol>,
    pub(crate) imports: Vec<Import>,
    pub(crate) references: Vec<PackedName>,
}

/// What one [`CodeGraph::refresh`] changed: the number the incremental promise
/// is measured by. Zero and zero on an unchanged workspace.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RefreshSummary {
    /// Files parsed again and stored — new ones and ones whose fingerprint
    /// moved.
    pub written: usize,
    /// Files the index forgot because the workspace no longer has them.
    pub removed: usize,
}

/// A session-owned workspace index, stored in one `SQLite` file.
///
/// Every query first runs [`Self::refresh`]: a metadata-only walk whose
/// fingerprints are compared with the stored ones, so only a file that was
/// added, changed or removed is parsed or written. Several sessions may hold
/// the same index; each one's writes are the others' next reads.
#[derive(Debug)]
pub struct CodeGraph {
    workspace_root: PathBuf,
    store: Store,
}

impl CodeGraph {
    pub fn load_or_build(
        workspace_root: impl AsRef<Path>,
        cache_path: impl Into<PathBuf>,
    ) -> Result<Self, CodeGraphError> {
        let workspace_root = canonicalize(workspace_root.as_ref())?;
        let cache_path = cache_path.into();
        remove_legacy_caches(&cache_path);
        let mut store = Store::open(&cache_path)?;
        if store.matches(&workspace_root)? {
            store.sync()?;
        } else {
            store.rebuild(&workspace_root, |writer| {
                let scan = scan_workspace(&workspace_root);
                let entries = scan
                    .files
                    .iter()
                    .map(|(path, entry)| (path.clone(), *entry))
                    .collect::<Vec<_>>();
                // Parse the next chunk while this one is written: the writer
                // is one thread holding the transaction, the parser a pool.
                std::thread::scope(|scope| {
                    let (parsed, chunks) = mpsc::sync_channel(1);
                    let root = workspace_root.as_path();
                    scope.spawn(move || {
                        for chunk in entries.chunks(BUILD_CHUNK_FILES) {
                            let records = extract_entries(root, chunk);
                            let failed = records.is_err();
                            if parsed.send(records).is_err() || failed {
                                return;
                            }
                        }
                    });
                    for records in chunks {
                        for (path, record) in records? {
                            writer.put(&path, record)?;
                        }
                    }
                    Ok(scan.file_limit_reached)
                })
            })?;
        }
        Ok(Self {
            workspace_root,
            store,
        })
    }

    /// The index at `cache_path` when one was already built for this
    /// workspace; `None` otherwise. Never builds and never walks — for a
    /// caller that must not pay for a first index, like a file read.
    pub fn open_existing(
        workspace_root: impl AsRef<Path>,
        cache_path: impl AsRef<Path>,
    ) -> Result<Option<Self>, CodeGraphError> {
        let workspace_root = canonicalize(workspace_root.as_ref())?;
        let cache_path = cache_path.as_ref();
        if !cache_path.is_file() {
            return Ok(None);
        }
        let mut store = Store::open(cache_path)?;
        if !store.matches(&workspace_root)? {
            return Ok(None);
        }
        store.sync()?;
        Ok(Some(Self {
            workspace_root,
            store,
        }))
    }

    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Bring the index up to date with the workspace: parse and store the
    /// files whose fingerprint moved or that are new, forget the ones that are
    /// gone. Parsing happens before the write lock is taken, so another
    /// session waits only for the rows to be written.
    pub fn refresh(&mut self) -> Result<RefreshSummary, CodeGraphError> {
        self.store.sync()?;
        let scan = scan_workspace(&self.workspace_root);
        let changed = scan
            .files
            .iter()
            .filter(|(path, entry)| {
                self.store
                    .stored(path)
                    .is_none_or(|stored| stored.fingerprint != entry.fingerprint)
            })
            .map(|(path, entry)| (path.clone(), *entry))
            .collect::<Vec<_>>();
        let removed = self
            .store
            .paths()
            .filter(|path| !scan.files.contains_key(*path))
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        if changed.is_empty()
            && removed.is_empty()
            && self.store.file_limit_reached() == scan.file_limit_reached
        {
            return Ok(RefreshSummary::default());
        }
        let records = extract_entries(&self.workspace_root, &changed)?;
        let summary = RefreshSummary {
            written: records.len(),
            removed: removed.len(),
        };
        self.store.update(scan.file_limit_reached, |writer| {
            for path in &removed {
                writer.remove(path)?;
            }
            for (path, record) in records {
                writer.put(&path, record)?;
            }
            Ok(())
        })?;
        Ok(summary)
    }

    pub fn find_symbols(
        &mut self,
        name: &str,
        kind: Option<SymbolKind>,
    ) -> Result<Vec<Symbol>, CodeGraphError> {
        self.refresh()?;
        self.store.symbols_named(name, kind)
    }

    pub fn find_references(&mut self, name: &str) -> Result<Vec<Reference>, CodeGraphError> {
        self.refresh()?;
        self.store.references(name)
    }

    /// The references of one definition — `name` as `file` defines it —
    /// rather than of a spelling: the occurrences in `file`, those in files
    /// whose imports spell the name ([`Import::spells`]), and every one when
    /// no other file defines the name. `None` when `file` defines no `name`.
    ///
    /// Against rust-analyzer on 267 definitions drawn from this repository's
    /// Rust (three seeds of 100; t-5970), the spelling alone kept 38,364
    /// occurrences and 3.6% of them were right — 75.9% for a name one file
    /// defines, 0.2% for a name ten or more files define (`new`, `finish`).
    /// This answer keeps 2,323, 57.1% right, and 96.4% of the real
    /// references. A name one file defines is answered as spelled. For a
    /// shared name the defining file's own occurrences are kept whole, which
    /// is what still misleads for the most shared names (7.5% right at ten or
    /// more definers); what it misses is a method called in another file
    /// without an import naming it.
    pub fn references_to(
        &mut self,
        file: impl AsRef<Path>,
        name: &str,
    ) -> Result<Option<Vec<Reference>>, CodeGraphError> {
        self.refresh()?;
        let path = self.relative_path(file.as_ref())?;
        self.store.references_to(&path, name)
    }

    /// What changing `name` as `file` defines it reaches: its references
    /// narrowed to it ([`Self::references_to`]), the files they sit in, and
    /// which of those are tests ([`Impact`]). `None` when `file` defines no
    /// `name`.
    pub fn impact(
        &mut self,
        file: impl AsRef<Path>,
        name: &str,
    ) -> Result<Option<Impact>, CodeGraphError> {
        let file = file.as_ref();
        let Some(references) = self.references_to(file, name)? else {
            return Ok(None);
        };
        let path = self.relative_path(file)?;
        let definitions = self
            .store
            .outline(&path)?
            .unwrap_or_default()
            .into_iter()
            .filter(|symbol| symbol.name == name)
            .collect();
        let files = LinkedFile::tally(
            references
                .iter()
                .map(|reference| (reference.file.as_path(), 1)),
        );
        Ok(Some(Impact {
            file: path,
            definitions,
            references: references.len(),
            files,
        }))
    }

    pub fn file_outline(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<Option<Vec<Symbol>>, CodeGraphError> {
        self.refresh()?;
        let path = self.relative_path(path.as_ref())?;
        self.store.outline(&path)
    }

    /// The imports one file's source spells, in source order, or `None` when
    /// the index holds no parsed file there. Paths are as written, not
    /// resolved.
    pub fn file_imports(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<Option<Vec<Import>>, CodeGraphError> {
        self.refresh()?;
        let path = self.relative_path(path.as_ref())?;
        self.store.imports(&path)
    }

    /// The files one file is linked to by names exactly one file defines
    /// ([`FileLinks`]), each list cut at `limit`. Answered from the index as
    /// the last refresh left it — no walk — and `None` unless the index holds
    /// this very version of the file (same modification time and length), so
    /// a file saved since then is never described by its old contents.
    pub fn file_links(
        &mut self,
        path: impl AsRef<Path>,
        limit: usize,
    ) -> Result<Option<FileLinks>, CodeGraphError> {
        self.store.sync()?;
        let path = self.relative_path(path.as_ref())?;
        let Some(stored) = self.store.stored(&path).filter(|stored| stored.indexed) else {
            return Ok(None);
        };
        let Ok(metadata) = fs::metadata(self.workspace_root.join(&path)) else {
            return Ok(None);
        };
        if fingerprint(&metadata) != stored.fingerprint {
            return Ok(None);
        }
        self.store.links(stored.id, limit).map(Some)
    }

    /// What each of `mentions` names, after one refresh for the whole batch:
    /// a path that is an indexed file — exactly, or as the one indexed path
    /// ending in it (`scan.rs`, `codegraph/src/scan.rs`) — or a symbol whose
    /// definitions all sit in one file, `Type::name` narrowed to that
    /// container or module. A word, a folder, or a name several files define
    /// is `None`: a picture draws what the index can place, not what it could
    /// guess.
    pub fn resolve_mentions(
        &mut self,
        mentions: &[String],
    ) -> Result<Vec<Option<Resolved>>, CodeGraphError> {
        self.refresh()?;
        let mut by_name = HashMap::<OsString, Vec<PathBuf>>::new();
        for path in self.store.indexed_paths() {
            if let Some(name) = path.file_name().map(OsStr::to_os_string) {
                by_name.entry(name).or_default().push(path);
            }
        }
        mentions
            .iter()
            .map(|mention| {
                if let Some(file) = self.resolve_path(mention, &by_name) {
                    return Ok(Some(Resolved::File(file)));
                }
                self.resolve_symbol(mention)
                    .map(|symbol| symbol.map(Resolved::Symbol))
            })
            .collect()
    }

    fn resolve_path(
        &self,
        mention: &str,
        by_name: &HashMap<OsString, Vec<PathBuf>>,
    ) -> Option<PathBuf> {
        let wanted = mention.split('/').collect::<PathBuf>();
        if self.store.stored(&wanted).is_some_and(|file| file.indexed) {
            return Some(wanted);
        }
        let mut ending = by_name
            .get(wanted.file_name()?)?
            .iter()
            .filter(|path| path.ends_with(&wanted));
        let only = ending.next()?;
        ending.next().is_none().then(|| only.clone())
    }

    fn resolve_symbol(&self, mention: &str) -> Result<Option<Symbol>, CodeGraphError> {
        let segments = mention.split(MENTION_PATH_SEPARATOR).collect::<Vec<_>>();
        let Some((name, qualifiers)) = segments.split_last() else {
            return Ok(None);
        };
        let mut found = self.store.symbols_named(name, None)?;
        if let Some(qualifier) = qualifiers.last() {
            // `Type::name` is a member of `Type`; `module::name` lives in a
            // file named after the module.
            found.retain(|symbol| {
                symbol.container.as_deref() == Some(*qualifier)
                    || symbol.file.file_stem() == Some(OsStr::new(qualifier))
            });
        }
        let Some(first) = found.first() else {
            return Ok(None);
        };
        Ok(found
            .iter()
            .all(|symbol| symbol.file == first.file)
            .then(|| first.clone()))
    }

    /// Every file the index holds parsed, in path order, as the last refresh
    /// left them.
    #[must_use]
    pub fn indexed_files(&self) -> Vec<PathBuf> {
        self.store.indexed_paths()
    }

    pub fn skipped_files(&self) -> Result<Vec<SkippedFile>, CodeGraphError> {
        self.store.skipped()
    }

    #[must_use]
    pub fn status(&self) -> IndexStatus {
        self.store.status()
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
}

fn extract_entries(
    workspace_root: &Path,
    entries: &[(PathBuf, ScanEntry)],
) -> Result<Vec<(PathBuf, FileRecord)>, CodeGraphError> {
    entries
        .par_iter()
        .map(|(relative, entry)| {
            let record = extract_entry(workspace_root, relative, *entry)?;
            Ok((relative.clone(), record))
        })
        .collect()
}

fn extract_entry(
    workspace_root: &Path,
    relative: &Path,
    entry: ScanEntry,
) -> Result<FileRecord, CodeGraphError> {
    let skipped = |reason| FileRecord {
        fingerprint: entry.fingerprint,
        content: FileContent::Skipped(reason),
    };
    if entry.too_large {
        return Ok(skipped(SkipReason::TooLarge {
            size: entry.fingerprint.size,
            limit: MAX_INDEXABLE_FILE_SIZE,
        }));
    }
    let source = match fs::read(workspace_root.join(relative)) {
        Ok(source) => source,
        Err(error) => {
            return Ok(skipped(SkipReason::ReadError {
                message: error.to_string(),
            }));
        }
    };
    if source.contains(&0) || std::str::from_utf8(&source).is_err() {
        return Ok(skipped(SkipReason::Binary));
    }
    let Some(spec) = spec_for_path(relative) else {
        return Ok(skipped(SkipReason::ParseError {
            message: "supported language disappeared during extraction".to_string(),
        }));
    };
    let extracted = extract(relative, &source, spec)?;
    Ok(FileRecord {
        fingerprint: entry.fingerprint,
        content: FileContent::Indexed(IndexedFile {
            references: pack_by_name(&extracted.references),
            language: extracted.language,
            symbols: extracted.symbols,
            imports: extracted.imports,
        }),
    })
}

/// Best effort: a cache nobody reads is only disk, never a reason to fail.
fn remove_legacy_caches(cache_path: &Path) {
    for legacy in LEGACY_CACHE_FILE_NAMES {
        let legacy = cache_path.with_file_name(legacy);
        match fs::remove_file(&legacy) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "[codegraph] could not remove the old cache {}: {error}",
                legacy.display()
            ),
        }
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf, CodeGraphError> {
    fs::canonicalize(path).map_err(|source| CodeGraphError::Io {
        path: path.to_path_buf(),
        source,
    })
}
