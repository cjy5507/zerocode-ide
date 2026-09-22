//! The index on disk: one `SQLite` file per workspace (t-5970).
//!
//! The v1 cache was one JSON document holding every reference with its file
//! path spelled out — 352 MB for this repository — so a session paid ~1 s and
//! ~800 MB resident to load it, and one save rewrote all of it. Here each fact
//! is a row keyed by the question asked of it: references and definitions are
//! found through the interned name, a file's rows through its id, and a save
//! replaces one file's rows in one transaction. Opening reads the file list
//! and nothing else.
//!
//! Every table, statement and pragma lives in this module; the rest of the
//! crate asks it questions in the model's own types.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::index::{CodeGraphError, FileContent, FileRecord, IndexedFile};
use crate::positions::{self, Occurrence};
use crate::model::{
    FileFingerprint, FileLinks, Import, IndexStatus, LinkedFile, Position, Reference, SkipReason,
    SkippedFile, SourceRange, Symbol, SymbolKind,
};

/// Written into `meta` beside the workspace root. An index that says anything
/// else — another schema, another root, nothing at all — is rebuilt, never
/// read.
const SCHEMA_TAG: &str = "zo-codegraph-v2";
const META_SCHEMA: &str = "schema";
const META_WORKSPACE_ROOT: &str = "workspace_root";
const META_FILE_LIMIT_REACHED: &str = "file_limit_reached";
const META_TRUE: &str = "1";
const META_FALSE: &str = "0";
/// How long a writer waits for another session's transaction on the same
/// index. Reads never wait (WAL); a first build of a large workspace is the
/// longest write there is.
const BUSY_TIMEOUT: Duration = Duration::from_secs(60);
/// The WAL is cut back to this after a checkpoint, so a first build's
/// transaction does not stay on disk twice.
const JOURNAL_SIZE_LIMIT_BYTES: i64 = 8 * 1024 * 1024;
/// Files `SQLite` keeps beside the index in WAL mode; thrown away with it.
const SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-shm"];
/// Stored paths are workspace-relative and `/`-separated on every platform,
/// so an index reads the same wherever it is opened.
const STORED_SEPARATOR: char = '/';

/// References are one row per name and file, keyed by the name a query asks
/// for, with every occurrence packed into `positions` (`crate::positions`).
/// An occurrence carries no end: an identifier is one token on one row, so
/// its end is its start plus the name's length in bytes — the arithmetic
/// tree-sitter's own columns use.
const CREATE_TABLES: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    modified_nanos INTEGER NOT NULL,
    size INTEGER NOT NULL,
    language TEXT,
    skip_reason TEXT
);
CREATE TABLE names (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
CREATE TABLE symbols (
    file_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    name_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    start_row INTEGER NOT NULL,
    start_column INTEGER NOT NULL,
    end_row INTEGER NOT NULL,
    end_column INTEGER NOT NULL,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    container TEXT,
    PRIMARY KEY (file_id, ordinal)
) WITHOUT ROWID;
CREATE TABLE refs (
    name_id INTEGER NOT NULL,
    file_id INTEGER NOT NULL,
    occurrences INTEGER NOT NULL,
    positions BLOB NOT NULL,
    PRIMARY KEY (name_id, file_id)
) WITHOUT ROWID;
CREATE TABLE imports (
    file_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    path TEXT NOT NULL,
    name TEXT,
    start_row INTEGER NOT NULL,
    start_column INTEGER NOT NULL,
    end_row INTEGER NOT NULL,
    end_column INTEGER NOT NULL,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    PRIMARY KEY (file_id, ordinal)
) WITHOUT ROWID;
";
/// Built after a rebuild's rows are in: one sort instead of a B-tree insert
/// per row.
const CREATE_LOOKUP_INDEXES: &str = "
CREATE INDEX symbols_by_name ON symbols(name_id);
CREATE INDEX refs_by_file ON refs(file_id);
";
const DROP_TABLES: &str = "
DROP TABLE IF EXISTS meta;
DROP TABLE IF EXISTS files;
DROP TABLE IF EXISTS names;
DROP TABLE IF EXISTS symbols;
DROP TABLE IF EXISTS refs;
DROP TABLE IF EXISTS imports;
";

const SELECT_HAS_META: &str =
    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'";
const SELECT_META: &str = "SELECT value FROM meta WHERE key = ?1";
const UPSERT_META: &str = "INSERT INTO meta (key, value) VALUES (?1, ?2)
    ON CONFLICT (key) DO UPDATE SET value = excluded.value";
const SELECT_FILES: &str =
    "SELECT path, id, modified_nanos, size, skip_reason IS NULL FROM files";
const SELECT_FILE_ID: &str = "SELECT id FROM files WHERE path = ?1";
const UPSERT_FILE: &str = "INSERT INTO files (path, modified_nanos, size, language, skip_reason)
    VALUES (?1, ?2, ?3, ?4, ?5)
    ON CONFLICT (path) DO UPDATE SET modified_nanos = excluded.modified_nanos,
        size = excluded.size, language = excluded.language, skip_reason = excluded.skip_reason
    RETURNING id";
const DELETE_FILE: &str = "DELETE FROM files WHERE id = ?1";
const DELETE_FILE_ROWS: [&str; 3] = [
    "DELETE FROM symbols WHERE file_id = ?1",
    "DELETE FROM refs WHERE file_id = ?1",
    "DELETE FROM imports WHERE file_id = ?1",
];
const SELECT_NAME_ID: &str = "SELECT id FROM names WHERE name = ?1";
const INSERT_NAME: &str = "INSERT INTO names (name) VALUES (?1)";
const INSERT_SYMBOL: &str = "INSERT INTO symbols (file_id, ordinal, name_id, kind, start_row,
    start_column, end_row, end_column, start_byte, end_byte, container)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";
const INSERT_REFERENCES: &str =
    "INSERT INTO refs (name_id, file_id, occurrences, positions) VALUES (?1, ?2, ?3, ?4)";
const INSERT_IMPORT: &str = "INSERT INTO imports (file_id, ordinal, path, name, start_row,
    start_column, end_row, end_column, start_byte, end_byte)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";
const SELECT_REFERENCES_BY_NAME: &str =
    "SELECT file_id, occurrences, positions FROM refs WHERE name_id = ?1";
const SELECT_SYMBOLS_BY_NAME: &str = "SELECT file_id, ordinal, kind, start_row, start_column,
    end_row, end_column, start_byte, end_byte, container FROM symbols
    WHERE name_id = ?1 AND (?2 IS NULL OR kind = ?2)";
const SELECT_SYMBOLS_OF_FILE: &str = "SELECT names.name, symbols.kind, symbols.start_row,
    symbols.start_column, symbols.end_row, symbols.end_column, symbols.start_byte,
    symbols.end_byte, symbols.container
    FROM symbols JOIN names ON names.id = symbols.name_id
    WHERE symbols.file_id = ?1 ORDER BY symbols.ordinal";
const SELECT_IMPORTS_OF_FILE: &str = "SELECT path, name, start_row, start_column, end_row,
    end_column, start_byte, end_byte FROM imports WHERE file_id = ?1 ORDER BY ordinal";
/// Names this file spells that exactly one other file defines, with that
/// file. A name defined twice in the one file comes back twice; the caller
/// counts it once.
const SELECT_USES: &str = "SELECT DISTINCT names.name, refs.occurrences, symbols.file_id
    FROM refs JOIN symbols ON symbols.name_id = refs.name_id
    JOIN names ON names.id = refs.name_id
    WHERE refs.file_id = ?1 AND symbols.file_id != ?1
      AND NOT EXISTS (SELECT 1 FROM symbols AS other
          WHERE other.name_id = refs.name_id AND other.file_id != symbols.file_id)";
/// Files spelling names that only this file defines.
const SELECT_USED_BY: &str = "SELECT DISTINCT refs.file_id, names.name, refs.occurrences
    FROM symbols JOIN refs ON refs.name_id = symbols.name_id
    JOIN names ON names.id = symbols.name_id
    WHERE symbols.file_id = ?1 AND refs.file_id != ?1
      AND NOT EXISTS (SELECT 1 FROM symbols AS other
          WHERE other.name_id = symbols.name_id AND other.file_id != ?1)";
const SELECT_DEFINERS: &str = "SELECT DISTINCT file_id FROM symbols WHERE name_id = ?1";
const SELECT_SKIPPED: &str =
    "SELECT path, skip_reason FROM files WHERE skip_reason IS NOT NULL";

/// What the index remembers about one file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StoredFile {
    pub(crate) id: i64,
    pub(crate) fingerprint: FileFingerprint,
    pub(crate) indexed: bool,
}

/// The `files` table, held in memory: the freshness check compares against
/// it, and query rows are ordered and labelled through it.
#[derive(Debug, Default)]
struct FileTable {
    by_path: BTreeMap<PathBuf, StoredFile>,
    /// Row id → (position in path order, path). Rows sort by the position,
    /// never by comparing paths.
    by_id: HashMap<i64, (usize, PathBuf)>,
    file_limit_reached: bool,
}

/// One `refs` row: a name's occurrences in one file. A rebuild sorts these
/// by key before inserting, so the B-tree is appended to rather than written
/// all over.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ReferenceRow {
    name_id: i64,
    file_id: i64,
    occurrences: i64,
    positions: Vec<u8>,
}

#[derive(Debug)]
pub(crate) struct Store {
    path: PathBuf,
    connection: Connection,
    files: FileTable,
    /// `PRAGMA data_version` when `files` was last read. Another session's
    /// commit moves it; this connection's own never does.
    seen_version: Option<i64>,
}

impl Store {
    /// Open the index at `path`, creating it when missing. A file `SQLite`
    /// cannot read is thrown away with its sidecars and replaced by an empty
    /// one: it is a cache, and the next build refills it.
    pub(crate) fn open(path: &Path) -> Result<Self, CodeGraphError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| CodeGraphError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        match Self::open_once(path) {
            Ok(store) => Ok(store),
            Err(error) => {
                eprintln!(
                    "[codegraph] index at {} is unreadable; rebuilding: {error}",
                    path.display()
                );
                remove_index_files(path);
                Self::open_once(path)
            }
        }
    }

    fn open_once(path: &Path) -> Result<Self, CodeGraphError> {
        let fail = |source| index_error(path, source);
        let connection = Connection::open(path).map_err(fail)?;
        connection.busy_timeout(BUSY_TIMEOUT).map_err(fail)?;
        // The first statement that reads the header: a file that is not a
        // database fails here rather than at the first query.
        let _mode: String = connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(fail)?;
        connection
            .execute_batch(&format!(
                "PRAGMA synchronous = NORMAL; PRAGMA journal_size_limit = {JOURNAL_SIZE_LIMIT_BYTES};"
            ))
            .map_err(fail)?;
        connection
            .query_row(SELECT_HAS_META, [], |row| row.get::<_, i64>(0))
            .map_err(fail)?;
        Ok(Self {
            path: path.to_path_buf(),
            connection,
            files: FileTable::default(),
            seen_version: None,
        })
    }

    /// Whether this index was built by this schema for `workspace_root`.
    pub(crate) fn matches(&self, workspace_root: &Path) -> Result<bool, CodeGraphError> {
        matches_workspace(&self.connection, workspace_root).map_err(|source| self.error(source))
    }

    /// Write the whole index again for `workspace_root`: `fill` puts every
    /// file and answers whether the scan reached the file cap. When another
    /// session finished the same rebuild while this one waited for the write
    /// lock, nothing is written and that session's index is read instead.
    pub(crate) fn rebuild(
        &mut self,
        workspace_root: &Path,
        fill: impl FnOnce(&mut Writer<'_>) -> Result<bool, CodeGraphError>,
    ) -> Result<(), CodeGraphError> {
        let path = self.path.as_path();
        let fail = |source| index_error(path, source);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(fail)?;
        if matches_workspace(&transaction, workspace_root).map_err(fail)? {
            transaction.commit().map_err(fail)?;
        } else {
            transaction.execute_batch(DROP_TABLES).map_err(fail)?;
            transaction.execute_batch(CREATE_TABLES).map_err(fail)?;
            set_meta(&transaction, META_SCHEMA, SCHEMA_TAG).map_err(fail)?;
            set_meta(
                &transaction,
                META_WORKSPACE_ROOT,
                &workspace_root.to_string_lossy(),
            )
            .map_err(fail)?;
            let mut writer = Writer::new(transaction, path, true);
            let file_limit_reached = fill(&mut writer)?;
            writer.finish(file_limit_reached)?;
            // Fold the build's WAL into the file and cut it back; a reader
            // still holding an old snapshot only makes this partial.
            self.connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                .map_err(|source| index_error(&self.path, source))?;
        }
        self.reload_files()
    }

    /// Change some files in one transaction; `fill` puts and removes them.
    pub(crate) fn update(
        &mut self,
        file_limit_reached: bool,
        fill: impl FnOnce(&mut Writer<'_>) -> Result<(), CodeGraphError>,
    ) -> Result<(), CodeGraphError> {
        let path = self.path.as_path();
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| index_error(path, source))?;
        let mut writer = Writer::new(transaction, path, false);
        fill(&mut writer)?;
        writer.finish(file_limit_reached)?;
        self.reload_files()
    }

    /// Read the file table again if another session committed since the last
    /// read. This session's own writes reload it as they commit.
    pub(crate) fn sync(&mut self) -> Result<(), CodeGraphError> {
        let version = data_version(&self.connection).map_err(|source| self.error(source))?;
        if self.seen_version != Some(version) {
            self.reload_files()?;
        }
        Ok(())
    }

    fn reload_files(&mut self) -> Result<(), CodeGraphError> {
        let fail = |source| index_error(&self.path, source);
        // Read the version first: a commit landing between the two reads is
        // then read twice, never missed.
        let version = data_version(&self.connection).map_err(fail)?;
        let mut by_path = BTreeMap::new();
        let mut statement = self.connection.prepare_cached(SELECT_FILES).map_err(fail)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    StoredFile {
                        id: row.get(1)?,
                        fingerprint: FileFingerprint {
                            modified_nanos: u128::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                            size: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                        },
                        indexed: row.get(4)?,
                    },
                ))
            })
            .map_err(fail)?;
        for row in rows {
            let (path, file) = row.map_err(fail)?;
            by_path.insert(loaded_path(&path), file);
        }
        drop(statement);
        let by_id = by_path
            .iter()
            .enumerate()
            .map(|(rank, (path, file))| (file.id, (rank, path.clone())))
            .collect();
        let file_limit_reached =
            meta_value(&self.connection, META_FILE_LIMIT_REACHED).map_err(fail)?.as_deref()
                == Some(META_TRUE);
        self.files = FileTable {
            by_path,
            by_id,
            file_limit_reached,
        };
        self.seen_version = Some(version);
        Ok(())
    }

    pub(crate) fn stored(&self, path: &Path) -> Option<StoredFile> {
        self.files.by_path.get(path).copied()
    }

    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.files.by_path.keys().map(PathBuf::as_path)
    }

    pub(crate) fn indexed_paths(&self) -> Vec<PathBuf> {
        self.files
            .by_path
            .iter()
            .filter(|(_, file)| file.indexed)
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub(crate) fn file_limit_reached(&self) -> bool {
        self.files.file_limit_reached
    }

    pub(crate) fn status(&self) -> IndexStatus {
        let indexed_files = self
            .files
            .by_path
            .values()
            .filter(|file| file.indexed)
            .count();
        IndexStatus {
            indexed_files,
            skipped_files: self.files.by_path.len() - indexed_files,
            file_limit_reached: self.files.file_limit_reached,
        }
    }

    /// Every reference spelled `name`, in path order and then source order.
    pub(crate) fn references(&self, name: &str) -> Result<Vec<Reference>, CodeGraphError> {
        let fail = |source| index_error(&self.path, source);
        let Some(name_id) = self.name_id(name)? else {
            return Ok(Vec::new());
        };
        let mut statement = self
            .connection
            .prepare_cached(SELECT_REFERENCES_BY_NAME)
            .map_err(fail)?;
        let rows = statement
            .query_map([name_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(fail)?;
        let mut located = Vec::new();
        let mut total = 0;
        for row in rows {
            let (file_id, occurrences, packed) = row.map_err(fail)?;
            // A row of a file another session added after this one's last
            // sync has no path here yet; the next query's sync brings it.
            if let Some((rank, path)) = self.files.by_id.get(&file_id) {
                total += loaded_count(occurrences);
                located.push((*rank, path, packed));
            }
        }
        located.sort_unstable_by_key(|(rank, ..)| *rank);
        let mut references = Vec::with_capacity(total);
        for (_, path, packed) in located {
            positions::decode_each(&packed, |occurrence| {
                references.push(Reference {
                    name: name.to_string(),
                    file: path.clone(),
                    range: identifier_range(name, occurrence),
                });
            })
            .ok_or_else(|| CodeGraphError::Corrupt {
                path: self.path.clone(),
                what: "reference positions",
            })?;
        }
        Ok(references)
    }

    /// The occurrences of `name` meant for its definition in `path`: those in
    /// that file, those in files whose imports spell the name, and — when no
    /// other file defines it — all of them. `None` when `path` defines no
    /// `name`.
    pub(crate) fn references_to(
        &self,
        path: &Path,
        name: &str,
    ) -> Result<Option<Vec<Reference>>, CodeGraphError> {
        let fail = |source| index_error(&self.path, source);
        let (Some(file), Some(name_id)) = (self.stored(path), self.name_id(name)?) else {
            return Ok(None);
        };
        let mut statement = self.connection.prepare_cached(SELECT_DEFINERS).map_err(fail)?;
        let definers = statement
            .query_map([name_id], |row| row.get::<_, i64>(0))
            .map_err(fail)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(fail)?;
        if !definers.contains(&file.id) {
            return Ok(None);
        }
        let references = self.references(name)?;
        if definers.len() == 1 {
            return Ok(Some(references));
        }
        let mut imports = HashMap::new();
        let mut kept = Vec::with_capacity(references.len());
        for reference in references {
            let vouched = reference.file == path
                || match self.stored(&reference.file) {
                    Some(stored) => self.imports_spell(&mut imports, stored.id, name)?,
                    None => false,
                };
            if vouched {
                kept.push(reference);
            }
        }
        Ok(Some(kept))
    }

    /// Every definition spelled `name` (of `kind`, when given), in path
    /// order and then the order the file defines them.
    pub(crate) fn symbols_named(
        &self,
        name: &str,
        kind: Option<SymbolKind>,
    ) -> Result<Vec<Symbol>, CodeGraphError> {
        let fail = |source| index_error(&self.path, source);
        let Some(name_id) = self.name_id(name)? else {
            return Ok(Vec::new());
        };
        let mut statement = self
            .connection
            .prepare_cached(SELECT_SYMBOLS_BY_NAME)
            .map_err(fail)?;
        let rows = statement
            .query_map(params![name_id, kind.map(SymbolKind::as_str)], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    stored_range(row, 3)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            })
            .map_err(fail)?;
        let mut located = Vec::new();
        for row in rows {
            let (file_id, ordinal, kind, range, container) = row.map_err(fail)?;
            let (Some((rank, path)), Some(kind)) =
                (self.files.by_id.get(&file_id), SymbolKind::from_label(&kind))
            else {
                continue;
            };
            located.push((
                (*rank, ordinal),
                Symbol {
                    name: name.to_string(),
                    kind,
                    file: path.clone(),
                    range,
                    container,
                },
            ));
        }
        located.sort_unstable_by_key(|(order, _)| *order);
        Ok(located.into_iter().map(|(_, symbol)| symbol).collect())
    }

    /// The definitions of one file in the order it defines them, or `None`
    /// when the index holds no parsed file at `path`.
    pub(crate) fn outline(&self, path: &Path) -> Result<Option<Vec<Symbol>>, CodeGraphError> {
        let fail = |source| index_error(&self.path, source);
        let Some(file) = self.stored(path).filter(|file| file.indexed) else {
            return Ok(None);
        };
        let mut statement = self
            .connection
            .prepare_cached(SELECT_SYMBOLS_OF_FILE)
            .map_err(fail)?;
        let rows = statement
            .query_map([file.id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    stored_range(row, 2)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .map_err(fail)?;
        let mut symbols = Vec::new();
        for row in rows {
            let (name, kind, range, container) = row.map_err(fail)?;
            if let Some(kind) = SymbolKind::from_label(&kind) {
                symbols.push(Symbol {
                    name,
                    kind,
                    file: path.to_path_buf(),
                    range,
                    container,
                });
            }
        }
        Ok(Some(symbols))
    }

    /// The imports of one file as its source spells them, or `None` when the
    /// index holds no parsed file at `path`.
    pub(crate) fn imports(&self, path: &Path) -> Result<Option<Vec<Import>>, CodeGraphError> {
        let Some(file) = self.stored(path).filter(|file| file.indexed) else {
            return Ok(None);
        };
        self.imports_of(file.id, path).map(Some)
    }

    fn imports_of(&self, file_id: i64, path: &Path) -> Result<Vec<Import>, CodeGraphError> {
        let fail = |source| index_error(&self.path, source);
        let mut statement = self
            .connection
            .prepare_cached(SELECT_IMPORTS_OF_FILE)
            .map_err(fail)?;
        let rows = statement
            .query_map([file_id], |row| {
                Ok(Import {
                    path: row.get(0)?,
                    name: row.get(1)?,
                    file: path.to_path_buf(),
                    range: stored_range(row, 2)?,
                })
            })
            .map_err(fail)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(fail)
    }

    /// Whether `file_id`'s imports spell `name`, reading each file's imports
    /// once per question.
    fn imports_spell(
        &self,
        seen: &mut HashMap<i64, Vec<Import>>,
        file_id: i64,
        name: &str,
    ) -> Result<bool, CodeGraphError> {
        let imports = match seen.entry(file_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(match self.files.by_id.get(&file_id) {
                Some((_, path)) => self.imports_of(file_id, path)?,
                None => Vec::new(),
            }),
        };
        Ok(imports.iter().any(|import| import.spells(name)))
    }

    /// The files `file_id` is linked to ([`FileLinks`]): by a name exactly
    /// one file defines, spelled by the using file's imports. Each list the
    /// most referenced first and cut at `limit`.
    pub(crate) fn links(&self, file_id: i64, limit: usize) -> Result<FileLinks, CodeGraphError> {
        let fail = |source| index_error(&self.path, source);
        let mut imports = HashMap::new();
        let mut uses = HashMap::<String, (i64, i64)>::new();
        let mut statement = self.connection.prepare_cached(SELECT_USES).map_err(fail)?;
        let rows = statement
            .query_map([file_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get(2)?))
            })
            .map_err(fail)?;
        for row in rows {
            let (name, occurrences, definer) = row.map_err(fail)?;
            if self.imports_spell(&mut imports, file_id, &name)? {
                uses.insert(name, (definer, occurrences));
            }
        }
        let mut used_by = Vec::new();
        let mut statement = self.connection.prepare_cached(SELECT_USED_BY).map_err(fail)?;
        let rows = statement
            .query_map([file_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get(2)?))
            })
            .map_err(fail)?;
        for row in rows {
            let (reader, name, occurrences) = row.map_err(fail)?;
            if self.imports_spell(&mut imports, reader, &name)? {
                used_by.push((reader, occurrences));
            }
        }
        Ok(FileLinks {
            uses: self.linked_files(uses.into_values(), limit),
            used_by: self.linked_files(used_by, limit),
        })
    }

    /// `(file id, occurrences)` rows as links, most first, cut at `limit`.
    fn linked_files(
        &self,
        rows: impl IntoIterator<Item = (i64, i64)>,
        limit: usize,
    ) -> Vec<LinkedFile> {
        let pairs = rows.into_iter().filter_map(|(file_id, occurrences)| {
            self.files
                .by_id
                .get(&file_id)
                .map(|(_, path)| (path.as_path(), loaded_count(occurrences)))
        });
        let mut linked = LinkedFile::tally(pairs);
        linked.truncate(limit);
        linked
    }

    pub(crate) fn skipped(&self) -> Result<Vec<SkippedFile>, CodeGraphError> {
        let fail = |source| index_error(&self.path, source);
        let mut statement = self.connection.prepare_cached(SELECT_SKIPPED).map_err(fail)?;
        let rows = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(fail)?;
        let mut skipped = Vec::new();
        for row in rows {
            let (path, reason) = row.map_err(fail)?;
            skipped.push(SkippedFile {
                file: loaded_path(&path),
                reason: serde_json::from_str(&reason).unwrap_or(SkipReason::ParseError {
                    message: format!("unreadable skip reason: {reason}"),
                }),
            });
        }
        skipped.sort_by(|left, right| left.file.cmp(&right.file));
        Ok(skipped)
    }

    fn name_id(&self, name: &str) -> Result<Option<i64>, CodeGraphError> {
        self.connection
            .prepare_cached(SELECT_NAME_ID)
            .and_then(|mut statement| statement.query_row([name], |row| row.get(0)).optional())
            .map_err(|source| self.error(source))
    }

    fn error(&self, source: rusqlite::Error) -> CodeGraphError {
        index_error(&self.path, source)
    }
}

/// One write transaction. Dropped without [`Writer::finish`], it rolls back.
pub(crate) struct Writer<'store> {
    transaction: Transaction<'store>,
    path: &'store Path,
    rebuilding: bool,
    names: HashMap<String, i64>,
    /// A rebuild's reference rows, inserted in key order by `finish`.
    pending_references: Vec<ReferenceRow>,
}

impl<'store> Writer<'store> {
    fn new(transaction: Transaction<'store>, path: &'store Path, rebuilding: bool) -> Self {
        Self {
            transaction,
            path,
            rebuilding,
            names: HashMap::new(),
            pending_references: Vec::new(),
        }
    }

    /// Replace everything the index holds for `path` with `record`.
    pub(crate) fn put(&mut self, path: &Path, record: FileRecord) -> Result<(), CodeGraphError> {
        let stored = stored_path(path)?;
        let (language, skip_reason) = match &record.content {
            FileContent::Indexed(file) => (Some(file.language.as_str()), None),
            FileContent::Skipped(reason) => (
                None,
                Some(serde_json::to_string(reason).map_err(|error| CodeGraphError::Io {
                    path: path.to_path_buf(),
                    source: std::io::Error::other(error),
                })?),
            ),
        };
        let file_id: i64 = self
            .transaction
            .prepare_cached(UPSERT_FILE)
            .and_then(|mut statement| {
                statement.query_row(
                    params![
                        stored,
                        stored_nanos(record.fingerprint.modified_nanos),
                        stored_count(record.fingerprint.size),
                        language,
                        skip_reason
                    ],
                    |row| row.get(0),
                )
            })
            .map_err(|source| self.error(source))?;
        if !self.rebuilding {
            self.clear(file_id)?;
        }
        if let FileContent::Indexed(file) = record.content {
            self.insert_contents(file_id, file)?;
        }
        Ok(())
    }

    /// Forget `path` and everything the index held for it.
    pub(crate) fn remove(&mut self, path: &Path) -> Result<(), CodeGraphError> {
        let stored = stored_path(path)?;
        let file_id: Option<i64> = self
            .transaction
            .prepare_cached(SELECT_FILE_ID)
            .and_then(|mut statement| statement.query_row([stored], |row| row.get(0)).optional())
            .map_err(|source| self.error(source))?;
        let Some(file_id) = file_id else {
            return Ok(());
        };
        self.clear(file_id)?;
        self.transaction
            .prepare_cached(DELETE_FILE)
            .and_then(|mut statement| statement.execute([file_id]))
            .map_err(|source| self.error(source))?;
        Ok(())
    }

    fn clear(&mut self, file_id: i64) -> Result<(), CodeGraphError> {
        for statement in DELETE_FILE_ROWS {
            self.transaction
                .prepare_cached(statement)
                .and_then(|mut statement| statement.execute([file_id]))
                .map_err(|source| self.error(source))?;
        }
        Ok(())
    }

    fn insert_contents(&mut self, file_id: i64, file: IndexedFile) -> Result<(), CodeGraphError> {
        for (ordinal, symbol) in file.symbols.iter().enumerate() {
            let name_id = self.name_id(&symbol.name)?;
            let range = symbol.range;
            self.transaction
                .prepare_cached(INSERT_SYMBOL)
                .and_then(|mut statement| {
                    statement.execute(params![
                        file_id,
                        stored_count(ordinal),
                        name_id,
                        symbol.kind.as_str(),
                        stored_count(range.start.row),
                        stored_count(range.start.column),
                        stored_count(range.end.row),
                        stored_count(range.end.column),
                        stored_count(range.start_byte),
                        stored_count(range.end_byte),
                        symbol.container,
                    ])
                })
                .map_err(|source| self.error(source))?;
        }
        for (ordinal, import) in file.imports.iter().enumerate() {
            let range = import.range;
            self.transaction
                .prepare_cached(INSERT_IMPORT)
                .and_then(|mut statement| {
                    statement.execute(params![
                        file_id,
                        stored_count(ordinal),
                        import.path,
                        import.name,
                        stored_count(range.start.row),
                        stored_count(range.start.column),
                        stored_count(range.end.row),
                        stored_count(range.end.column),
                        stored_count(range.start_byte),
                        stored_count(range.end_byte),
                    ])
                })
                .map_err(|source| self.error(source))?;
        }
        for packed in file.references {
            let row = ReferenceRow {
                name_id: self.name_id(&packed.name)?,
                file_id,
                occurrences: stored_count(packed.occurrences),
                positions: packed.positions,
            };
            if self.rebuilding {
                self.pending_references.push(row);
            } else {
                self.insert_references(&row)?;
            }
        }
        Ok(())
    }

    fn insert_references(&self, row: &ReferenceRow) -> Result<(), CodeGraphError> {
        self.transaction
            .prepare_cached(INSERT_REFERENCES)
            .and_then(|mut statement| {
                statement.execute(params![
                    row.name_id,
                    row.file_id,
                    row.occurrences,
                    row.positions
                ])
            })
            .map(|_| ())
            .map_err(|source| self.error(source))
    }

    fn name_id(&mut self, name: &str) -> Result<i64, CodeGraphError> {
        if let Some(id) = self.names.get(name) {
            return Ok(*id);
        }
        // A rebuild starts from empty tables: every name it meets is new.
        let existing = if self.rebuilding {
            None
        } else {
            self.transaction
                .prepare_cached(SELECT_NAME_ID)
                .and_then(|mut statement| statement.query_row([name], |row| row.get(0)).optional())
                .map_err(|source| self.error(source))?
        };
        let id = if let Some(id) = existing {
            id
        } else {
            self.transaction
                .prepare_cached(INSERT_NAME)
                .and_then(|mut statement| statement.execute([name]))
                .map_err(|source| self.error(source))?;
            self.transaction.last_insert_rowid()
        };
        self.names.insert(name.to_owned(), id);
        Ok(id)
    }

    fn finish(mut self, file_limit_reached: bool) -> Result<(), CodeGraphError> {
        if self.rebuilding {
            let mut pending = std::mem::take(&mut self.pending_references);
            pending.sort_unstable();
            for row in &pending {
                self.insert_references(row)?;
            }
            self.transaction
                .execute_batch(CREATE_LOOKUP_INDEXES)
                .map_err(|source| self.error(source))?;
        }
        set_meta(
            &self.transaction,
            META_FILE_LIMIT_REACHED,
            if file_limit_reached { META_TRUE } else { META_FALSE },
        )
        .map_err(|source| self.error(source))?;
        let path = self.path;
        self.transaction
            .commit()
            .map_err(|source| index_error(path, source))
    }

    fn error(&self, source: rusqlite::Error) -> CodeGraphError {
        index_error(self.path, source)
    }
}

fn matches_workspace(connection: &Connection, workspace_root: &Path) -> rusqlite::Result<bool> {
    if connection.query_row(SELECT_HAS_META, [], |row| row.get::<_, i64>(0))? == 0 {
        return Ok(false);
    }
    Ok(
        meta_value(connection, META_SCHEMA)?.as_deref() == Some(SCHEMA_TAG)
            && meta_value(connection, META_WORKSPACE_ROOT)?.as_deref()
                == Some(workspace_root.to_string_lossy().as_ref()),
    )
}

fn meta_value(connection: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    connection
        .prepare_cached(SELECT_META)?
        .query_row([key], |row| row.get(0))
        .optional()
}

fn set_meta(connection: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    connection
        .prepare_cached(UPSERT_META)?
        .execute([key, value])
        .map(|_| ())
}

fn data_version(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row("PRAGMA data_version", [], |row| row.get(0))
}

/// Columns `first..first + 6` as a range: start row and column, end row and
/// column, start and end byte.
fn stored_range(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<SourceRange> {
    let at = |offset: usize| row.get::<_, i64>(first + offset).map(loaded_count);
    Ok(SourceRange {
        start: Position {
            row: at(0)?,
            column: at(1)?,
        },
        end: Position {
            row: at(2)?,
            column: at(3)?,
        },
        start_byte: at(4)?,
        end_byte: at(5)?,
    })
}

fn identifier_range(name: &str, occurrence: Occurrence) -> SourceRange {
    let Occurrence {
        start_byte,
        row,
        column,
    } = occurrence;
    SourceRange {
        start: Position { row, column },
        end: Position {
            row,
            column: column + name.len(),
        },
        start_byte,
        end_byte: start_byte + name.len(),
    }
}

/// Counts in the index are rows, columns, byte offsets and sizes — all
/// bounded by `MAX_INDEXABLE_FILE_SIZE` or the file system — so neither
/// direction of these conversions can saturate in practice.
fn stored_count<T: TryInto<i64>>(value: T) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

fn loaded_count(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

/// The scan clamps modification times to this range already
/// (`scan::fingerprint`), so a stored time reads back equal.
fn stored_nanos(nanos: u128) -> i64 {
    i64::try_from(nanos).unwrap_or(i64::MAX)
}

/// A workspace-relative path as the index spells it. Only plain relative
/// UTF-8 paths are stored; the scan never offers anything else.
fn stored_path(path: &Path) -> Result<String, CodeGraphError> {
    let mut text = String::new();
    for component in path.components() {
        let part = match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        }
        .ok_or_else(|| CodeGraphError::InvalidPath(path.to_path_buf()))?;
        if !text.is_empty() {
            text.push(STORED_SEPARATOR);
        }
        text.push_str(part);
    }
    Ok(text)
}

fn loaded_path(text: &str) -> PathBuf {
    text.split(STORED_SEPARATOR).collect()
}

fn index_error(path: &Path, source: rusqlite::Error) -> CodeGraphError {
    CodeGraphError::Index {
        path: path.to_path_buf(),
        source,
    }
}

fn remove_index_files(path: &Path) {
    let mut doomed = vec![path.to_path_buf()];
    doomed.extend(SIDECAR_SUFFIXES.iter().map(|suffix| {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }));
    for file in doomed {
        match fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!("[codegraph] could not remove {}: {error}", file.display()),
        }
    }
}
