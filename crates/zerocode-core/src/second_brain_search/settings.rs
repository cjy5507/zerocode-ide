//! What the search reads and how, as one table a vault may override.
//!
//! The vault's own `wiki/` and `raw/` are always read. Any other folder is
//! read where it lies, and only because a person added it: the settings file
//! (`<vault>/.zerocode/search.json`) lists those folders and nothing in this
//! crate names one.
//!
//! ```json
//! { "sources": [{ "path": "/somewhere/reports", "layer": "reports" }],
//!   "records": true, "maxFileBytes": 262144, "passageBytes": 1200 }
//! ```
//!
//! Every key is optional. A key the file leaves out keeps the value
//! [`Table::default`] gives it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use super::index;
use super::passages::Bounds;
use super::segment;
use crate::atomic_file::write_atomically;
use crate::second_brain::{RAW_DIR, WIKI_DIR};
use crate::second_brain_graph::MAX_PAGE_BYTES;
use crate::second_brain_live::TRACE_DIR as STATE_DIR;

/// The settings file, inside the vault's private folder.
pub const SETTINGS_FILE: &str = "search.json";
/// The folder the index files live in, inside the vault's private folder.
pub const INDEX_DIR: &str = "search-index";
/// What a person is told when a folder is added, and wherever the added
/// folders are listed: the index is a part of the folder, kept in the vault.
pub const ADDED_FOLDER_NOTICE: &str = "an added folder's words, paths and titles are kept in the index under the vault's .zerocode/ — a vault that is synced carries them along";

/// Files read as prose, by extension. On unless the settings name their own.
pub const PROSE_EXTENSIONS: [&str; 7] = ["md", "markdown", "mdx", "txt", "rst", "org", "adoc"];
/// Structured and recorded formats: dumps, logs and data. Off until the
/// settings say `"records": true`, because in a vault's `raw/` most of them
/// are screen dumps and test output — noise beside the evidence.
pub const RECORD_EXTENSIONS: [&str; 9] = [
    "xml", "json", "jsonl", "log", "html", "csv", "tsv", "yaml", "yml",
];

/// The rules that cut a file into the words an index holds, by the release
/// that carries them: how a text becomes words (`second_brain_related`),
/// where a passage ends (`passages`) and what a page's name and links are
/// (`second_brain_graph`). None of the three knows this index exists, so no
/// number a person must remember to raise stands for them — the crate's own
/// version does, and every release builds a vault's index once.
pub(super) const RULES: &str = env!("CARGO_PKG_VERSION");

const KEY_SOURCES: &str = "sources";
const KEY_PATH: &str = "path";
const KEY_LAYER: &str = "layer";
const KEY_EXTENSIONS: &str = "extensions";
const KEY_RECORDS: &str = "records";
const KEY_MAX_FILE_BYTES: &str = "maxFileBytes";
const KEY_PASSAGE_BYTES: &str = "passageBytes";

/// What is read and how. One table: every bound the index is built under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// Lower-case extensions of the files read.
    pub extensions: BTreeSet<String>,
    /// A file longer than this is left out: past it a file is a dump, and a
    /// passage's place must fit four bytes. Unless the settings say another,
    /// it is what the knowledge graph reads of one page (`MAX_PAGE_BYTES`):
    /// the vault's two readers agree on how long a page is. They part past
    /// it — the graph reads a longer page up to the bound, the search leaves
    /// it out, because a passage names bytes of the whole file.
    pub max_file_bytes: u64,
    /// How long a passage may be and how short one may stand alone.
    pub(super) passage: Bounds,
    /// Folders never entered in a folder a person added, beside every folder
    /// there whose name starts with a dot: what a build wrote, not a person.
    /// The vault's own folders are entered as its knowledge graph enters them.
    pub skipped_folders: Vec<String>,
    /// Files one source contributes; past it the walk stops and says so.
    pub max_files: usize,
    /// Folders descended into in a folder a person added.
    pub max_depth: usize,
    /// Files changed since the base was built, as one in this many of it,
    /// past which the folder is built whole again — and the count below
    /// which it never is.
    pub rebuild_share: usize,
    pub rebuild_floor: usize,
    /// How long a search waits for another one writing the same index before
    /// it answers from memory, and how often it looks.
    pub lock_patience: Duration,
    pub lock_poll: Duration,
    /// Threads a build reads files on.
    pub build_threads: usize,
}

impl Default for Table {
    fn default() -> Self {
        let most = 1_200;
        Self {
            extensions: PROSE_EXTENSIONS
                .iter()
                .map(|held| (*held).to_string())
                .collect(),
            max_file_bytes: MAX_PAGE_BYTES as u64,
            passage: Bounds {
                most,
                // A line and a half of prose: less says too little to be an
                // answer on its own.
                least: most / 15,
            },
            skipped_folders: vec!["node_modules".to_string(), "target".to_string()],
            max_files: 50_000,
            max_depth: 12,
            rebuild_share: 10,
            rebuild_floor: 32,
            lock_patience: Duration::from_secs(30),
            lock_poll: Duration::from_millis(10),
            build_threads: std::thread::available_parallelism().map_or(1, |held| held.get().min(8)),
        }
    }
}

impl Table {
    /// Whether a file of this name is read.
    pub(super) fn reads(&self, name: &str) -> bool {
        name.rsplit_once('.').is_some_and(|(stem, extension)| {
            !stem.is_empty() && self.extensions.contains(&extension.to_ascii_lowercase())
        })
    }

    /// Everything an index's content depends on, as one line: an index built
    /// under another line is built again. The folder is not part of it — an
    /// index lies in a folder of its own (`Source::key`), and paths inside it
    /// are spelled from the folder down, so a vault reached by another
    /// spelling of its path, or moved, keeps its index.
    pub(super) fn fingerprint(&self) -> String {
        format!(
            "{}|{RULES}|{}|{}|{}|{}|{}|{}|{}",
            segment::FORMAT,
            self.extensions
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
            self.max_file_bytes,
            self.passage.most,
            self.passage.least,
            self.skipped_folders.join(","),
            self.max_files,
            self.max_depth,
        )
    }
}

/// What a source is to the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The vault's pages: curated, linked, and shown first.
    Wiki,
    /// The vault's originals.
    Raw,
    /// A folder outside the vault a person added.
    Added,
}

/// One folder the search reads, and the layer its answers are shown under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub layer: String,
    pub root: PathBuf,
    pub kind: Kind,
    /// The name of its index folder.
    pub(super) key: String,
}

/// The settings as a search reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub table: Table,
    pub sources: Vec<Source>,
    /// Said when the settings file could not be read as written and the
    /// defaults stood in for it.
    pub notice: Option<String>,
}

fn settings_path(vault: &Path) -> PathBuf {
    vault.join(STATE_DIR).join(SETTINGS_FILE)
}

/// Where the index files of `vault` live.
pub(super) fn index_dir(vault: &Path) -> PathBuf {
    vault.join(STATE_DIR).join(INDEX_DIR)
}

/// The settings of `vault`: its own two folders, the folders its file adds,
/// and the table with whatever the file overrides. A file that is not there
/// is the defaults; one that cannot be read is the defaults and a notice.
#[must_use]
pub fn read(vault: &Path) -> Settings {
    let (written, notice) = match std::fs::read(settings_path(vault)) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(written)) => (written, None),
            _ => (
                Map::new(),
                Some(format!(
                    "{} is not a JSON object — the defaults are used",
                    settings_path(vault).display()
                )),
            ),
        },
        Err(_) => (Map::new(), None),
    };
    let mut table = Table::default();
    if let Some(extensions) = written.get(KEY_EXTENSIONS).and_then(Value::as_array) {
        table.extensions = extensions
            .iter()
            .filter_map(Value::as_str)
            .map(|held| held.trim_start_matches('.').to_ascii_lowercase())
            .filter(|held| !held.is_empty())
            .collect();
    }
    if written.get(KEY_RECORDS).and_then(Value::as_bool) == Some(true) {
        table
            .extensions
            .extend(RECORD_EXTENSIONS.iter().map(|held| (*held).to_string()));
    }
    if let Some(bytes) = written.get(KEY_MAX_FILE_BYTES).and_then(Value::as_u64) {
        // A passage's place is four bytes wide.
        table.max_file_bytes = bytes.clamp(1, u64::from(u32::MAX));
    }
    if let Some(most) = written.get(KEY_PASSAGE_BYTES).and_then(Value::as_u64) {
        let share = Table::default().passage;
        // No passage is longer than the longest file read (the key above, so
        // it is final here): a number past that says nothing more, and cut
        // to it the arithmetic on a passage's length stays in range.
        let most = usize::try_from(most.clamp(1, table.max_file_bytes)).unwrap_or(usize::MAX);
        table.passage = Bounds {
            most,
            least: most.saturating_mul(share.least) / share.most,
        };
    }

    let mut sources = vec![Source {
        layer: WIKI_DIR.to_string(),
        root: vault.join(WIKI_DIR),
        kind: Kind::Wiki,
        key: WIKI_DIR.to_string(),
    }];
    if vault.join(RAW_DIR).is_dir() {
        sources.push(Source {
            layer: RAW_DIR.to_string(),
            root: vault.join(RAW_DIR),
            kind: Kind::Raw,
            key: RAW_DIR.to_string(),
        });
    }
    sources.extend(added(&written));
    Settings {
        table,
        sources,
        notice,
    }
}

/// The folders a settings file adds, as it lists them.
fn added(written: &Map<String, Value>) -> Vec<Source> {
    written
        .get(KEY_SOURCES)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|held| {
            let path = held.get(KEY_PATH)?.as_str()?;
            let layer = held.get(KEY_LAYER)?.as_str()?;
            valid_layer(layer).then(|| Source {
                layer: layer.to_string(),
                root: PathBuf::from(path),
                kind: Kind::Added,
                // The layer says what the folder is to a person; the hash
                // keeps two folders of one layer apart.
                key: format!("{layer}-{:016x}", crate::launch_budget::fnv1a(path.bytes())),
            })
        })
        .collect()
}

/// A layer's name is typed after `--layer` and names a folder on disk: small
/// letters, digits, `-` and `_`, and not one of the vault's own two.
fn valid_layer(layer: &str) -> bool {
    !layer.is_empty()
        && layer != WIKI_DIR
        && layer != RAW_DIR
        && layer.chars().all(|held| {
            held.is_ascii_lowercase() || held.is_ascii_digit() || matches!(held, '-' | '_')
        })
}

fn written_object(vault: &Path) -> Result<Map<String, Value>, String> {
    let path = settings_path(vault);
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(written)) => Ok(written),
            _ => Err(format!(
                "{} is not a JSON object — fix or remove it first",
                path.display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn write_object(vault: &Path, written: &Map<String, Value>) -> Result<(), String> {
    let path = settings_path(vault);
    let saved = |error: std::io::Error| format!("{}: {error}", path.display());
    std::fs::create_dir_all(vault.join(STATE_DIR)).map_err(saved)?;
    let mut bytes = serde_json::to_vec_pretty(written).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    write_atomically(&path, &bytes).map_err(saved)
}

/// Adds `folder` to what the search of `vault` reads, under `layer` or, when
/// none is given, the folder's own name.
///
/// # Errors
///
/// The folder is not one, lies inside the vault, is listed already, or the
/// layer's name cannot be one; or the settings file could not be read or
/// written.
pub fn add_source(vault: &Path, folder: &Path, layer: Option<&str>) -> Result<Source, String> {
    let root = folder
        .canonicalize()
        .ok()
        .filter(|held| held.is_dir())
        .ok_or_else(|| format!("{} is not a folder", folder.display()))?;
    if vault
        .canonicalize()
        .is_ok_and(|held| root.starts_with(held))
    {
        return Err(format!(
            "{} is inside the vault — its {WIKI_DIR}/ and {RAW_DIR}/ are read already",
            root.display()
        ));
    }
    let layer = match layer {
        Some(layer) => layer.to_string(),
        None => root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase(),
    };
    if !valid_layer(&layer) {
        return Err(format!(
            "`{layer}` cannot name a layer — small letters, digits, - and _, and not {WIKI_DIR} or {RAW_DIR}; pass --layer <name>"
        ));
    }
    let mut written = written_object(vault)?;
    let path = root.display().to_string();
    if let Some(held) = added(&written).iter().find(|held| held.root == root) {
        return Err(format!("{path} is read already, as layer {}", held.layer));
    }
    let listed = written
        .entry(KEY_SOURCES)
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(listed) = listed else {
        return Err(format!(
            "`{KEY_SOURCES}` in {} is not a list",
            settings_path(vault).display()
        ));
    };
    listed.push(Value::Object(Map::from_iter([
        (KEY_PATH.to_string(), Value::String(path)),
        (KEY_LAYER.to_string(), Value::String(layer)),
    ])));
    let source = added(&written)
        .into_iter()
        .find(|held| held.root == root)
        .ok_or("the folder could not be listed")?;
    write_object(vault, &written)?;
    Ok(source)
}

/// Takes a folder out of what the search of `vault` reads, named by its path
/// or by its layer, and removes its index. The folder itself is not touched.
/// A search that is writing that index is waited for first (`index::remove`).
///
/// # Errors
///
/// Nothing listed answers to `named`, or the settings file could not be read
/// or written.
pub fn remove_source(vault: &Path, named: &str) -> Result<Vec<Source>, String> {
    let mut written = written_object(vault)?;
    let by_path = Path::new(named).canonicalize().ok();
    let answers = |held: &Source| {
        held.layer == named
            || held.root == Path::new(named)
            || by_path.as_ref().is_some_and(|path| *path == held.root)
    };
    let gone: Vec<Source> = added(&written)
        .into_iter()
        .filter(|held| answers(held))
        .collect();
    if gone.is_empty() {
        return Err(format!(
            "no added folder answers to `{named}` — see `zo vault source list`"
        ));
    }
    if let Some(Value::Array(listed)) = written.get_mut(KEY_SOURCES) {
        listed.retain(|held| {
            let path = held.get(KEY_PATH).and_then(Value::as_str);
            !gone
                .iter()
                .any(|source| path.is_some_and(|path| Path::new(path) == source.root))
        });
    }
    write_object(vault, &written)?;
    let table = read(vault).table;
    for source in &gone {
        index::remove(&index_dir(vault).join(&source.key), &table);
    }
    Ok(gone)
}
