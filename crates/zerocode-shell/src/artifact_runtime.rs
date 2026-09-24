//! The artifact store: catalog, bounded scan, previews, retention (t-2720 §2).
//!
//! Everything an agent makes ends up in one of four places today — an
//! automation's evidence folder, a worker's `/tmp/t-<id>-report.md`, a
//! Computer Use or browser screenshot, a workflow's output — and until this
//! module the window knew about the first only. This is the one catalog:
//! `<local data root>/artifacts/<run|automation|manual>/<id>/…` for the
//! files it copies, `index.jsonl` (append-only, one row per line, a tombstone
//! per removal) for what it knows, and one `id → Artifact` map in memory.
//!
//! What this file does NOT do is as deliberate as what it does:
//!
//! - It does not walk the disk on its own clock. A scan is bounded by the
//!   table ([`Limits`]) — depth, files, bytes — and incremental on
//!   modified-time and length, so a folder that has not changed costs one
//!   `stat` per file and no read. The file watcher's artifact lane
//!   (`file_watch`) says WHEN to look; nothing here spawns a thread.
//! - It does not decode an image until the drawer asks for it, and it hands
//!   the window a bounded `data:` URL through an LRU with a byte cap, never a
//!   decoded bitmap held for the life of the window.
//! - It does not know where evidence folders are. The automation runtime
//!   REGISTERS them ([`Store::adopt_source`]) with the origin it holds; this
//!   file only scans what it was handed. The orchestration runtime hands in
//!   worker reports the same way ([`Store::register_report`]).
//! - It does not delete without being told the person confirmed
//!   ([`Store::delete`] with `confirmed == false` is a no-op), and it never
//!   deletes a file outside the app's own data root.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use crate::{AppState, ShellStateExt as _};
use serde::{Deserialize, Serialize};
use zerocode_core::artifact::{
    self, Artifact, ArtifactKind, Limits, Origin, Preview, Source, artifact_id,
    artifact_id_for_url, body_tokens, kind_of, preview_of, query_matches, title_of,
};
use zerocode_core::artifact_transcript::{PageFact, RemoteFact};

/// The store's folder under the local data root.
pub(crate) const STORE_DIR_NAME: &str = "artifacts";
/// The append-only catalog inside it.
pub(crate) const INDEX_FILE: &str = "index.jsonl";
/// The file watcher lane this store's folders ride in.
pub(crate) const WATCH_LANE: &str = "artifacts";
/// The window event that says the catalog moved.
pub(crate) const CHANGED_EVENT: &str = "artifacts:changed";
/// Where a page's snapshots live: `<root>/versions/<id>/<n>/<name>` (t-3233 §5).
pub(crate) const VERSIONS_DIR_NAME: &str = "versions";
/// Where a card's rendered thumbnail lives: `<root>/thumbs/<id>.png` (t-3233 §3).
pub(crate) const THUMBS_DIR_NAME: &str = "thumbs";

/// One line of `index.jsonl`: a row, or the removal of one. Untagged so an
/// old build's rows (which know no tombstones) still read, and a tombstone —
/// which has no `id` field — can never parse as a row.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum IndexLine {
    Row(Box<Artifact>),
    Tombstone { tombstone: String },
}

/// What a file was when it was last indexed — the cheap question a scan asks
/// before it reads anything, exactly as `file_watch::Snap` asks it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then_some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

fn epoch_ms(time: Option<SystemTime>) -> i64 {
    time.and_then(|held| held.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_millis() as i64)
}

/// What a row is built from: the file, its identity, its provenance, and
/// the clock — one bundle so the builder reads as one question.
struct RowSeed<'a> {
    path: &'a Path,
    id: String,
    source: Source,
    origin: Origin,
    stamp: Stamp,
    now_ms: i64,
    created_ms: Option<i64>,
}

/// A row as held in memory: the artifact, its search tokens (sorted, bounded
/// by the table), and the stamp the incremental scan compares against.
#[derive(Clone)]
struct Row {
    artifact: Artifact,
    tokens: Vec<String>,
    stamp: Option<Stamp>,
}

/// A folder the scan walks, with the origin every file in it inherits. The
/// caller that knows the folder — the automation runtime for evidence, the
/// settings for an export folder — hands it in; this file never guesses one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanSource {
    pub(crate) dir: PathBuf,
    pub(crate) source: Source,
    pub(crate) origin: Origin,
}

/// What one scan pass did, and whether it hit a bound.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ScanReport {
    pub(crate) seen: usize,
    pub(crate) added: usize,
    pub(crate) updated: usize,
    pub(crate) removed: usize,
    /// A bound of the table was reached before every file was looked at.
    pub(crate) truncated: bool,
}

impl ScanReport {
    pub(crate) fn changed(&self) -> bool {
        self.added + self.updated + self.removed > 0
    }
}

/// What a retention sweep gave up.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Pruned {
    pub(crate) rows: usize,
    pub(crate) files: usize,
}

/// The window's filter, as the view sends it. Every field optional; an empty
/// filter lists everything the table allows.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Filter {
    pub(crate) query: String,
    pub(crate) kind: Option<String>,
    pub(crate) run: Option<String>,
    pub(crate) task: Option<String>,
    pub(crate) worker: Option<String>,
    pub(crate) worktree: Option<String>,
    pub(crate) automation: Option<String>,
    pub(crate) agent: Option<String>,
    /// `Some(true)` lists claude.ai artifacts only, `Some(false)` this
    /// machine's only — the gallery's 전체/이 기계/claude.ai segments (t-3233 §4).
    pub(crate) remote: Option<bool>,
    /// `Some(true)` lists publications only — the rows `zerocode-artifact`
    /// can read and export by version (t-3952), exactly what the headless
    /// catalog holds.
    pub(crate) published: Option<bool>,
}

/// One snapshot of a page or document (t-3233 §5), or one kept version of a
/// publication (t-3952): the number the picker shows, the immutable file,
/// when it was written, and its SHA-256 — the digest feedback cites so an
/// agent can tell the version it was shown from the source it will edit.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Version {
    pub(crate) n: u32,
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) modified_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sha256: Option<String>,
}

/// One listing answer: rows newest first, the count behind them, and whether
/// the table's row cap cut the answer.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Listing {
    pub(crate) rows: Vec<Artifact>,
    pub(crate) total: usize,
    pub(crate) truncated: bool,
}

/// Counts by origin, for the chips other surfaces wear — the board card, the
/// task row, the worktree row ask "how many for mine" and nothing else.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) total: usize,
    pub(crate) by_worker: BTreeMap<String, usize>,
    pub(crate) by_task: BTreeMap<String, usize>,
    pub(crate) by_run: BTreeMap<String, usize>,
    pub(crate) by_worktree: BTreeMap<String, usize>,
    pub(crate) by_automation: BTreeMap<String, usize>,
    /// The newest report per worker — the id 「보고서 보기」 opens from a
    /// ledger row (t-2720 §4), beside the count the chip wears.
    pub(crate) report_by_worker: BTreeMap<String, String>,
}

/// What the drawer draws: the file's text (bounded), or a `data:` URL for an
/// image (bounded), or nothing with a reason.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct PreviewPayload {
    /// `markdown` | `image` | `text` | `none`.
    pub(crate) kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data_url: Option<String>,
    pub(crate) bytes: u64,
    /// The file was larger than the table allows, and what came back is cut
    /// (text) or absent (image).
    pub(crate) truncated: bool,
}

impl PreviewPayload {
    fn cost(&self) -> u64 {
        self.text.as_ref().map_or(0, |held| held.len() as u64)
            + self.data_url.as_ref().map_or(0, |held| held.len() as u64)
    }
}

/// The preview LRU: bounded by bytes, not by count — one 4 MB screenshot and
/// a thousand 200-byte texts are not the same memory.
#[derive(Default)]
struct PreviewCache {
    order: VecDeque<String>,
    held: HashMap<String, Arc<PreviewPayload>>,
    bytes: u64,
}

impl PreviewCache {
    fn get(&mut self, id: &str) -> Option<Arc<PreviewPayload>> {
        let found = self.held.get(id)?.clone();
        // Touched: to the back of the line.
        if let Some(at) = self.order.iter().position(|held| held == id) {
            self.order.remove(at);
        }
        self.order.push_back(id.to_string());
        Some(found)
    }

    fn put(&mut self, id: &str, payload: Arc<PreviewPayload>, cap: u64) {
        let cost = payload.cost();
        if cost > cap {
            // Larger than the whole cache: served once, never kept.
            return;
        }
        self.evict(id);
        while self.bytes + cost > cap {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(gone) = self.held.remove(&oldest) {
                self.bytes -= gone.cost();
            }
        }
        self.order.push_back(id.to_string());
        self.held.insert(id.to_string(), payload);
        self.bytes += cost;
    }

    fn evict(&mut self, id: &str) {
        if let Some(gone) = self.held.remove(id) {
            self.bytes -= gone.cost();
        }
        if let Some(at) = self.order.iter().position(|held| held == id) {
            self.order.remove(at);
        }
    }

    fn clear(&mut self) {
        self.order.clear();
        self.held.clear();
        self.bytes = 0;
    }
}

struct Index {
    rows: BTreeMap<String, Row>,
    /// Resume bounded page scans after the last visited id, avoiding starvation.
    page_scan_after: Option<String>,
    /// Lines appended since the last compaction. Nothing reads it but the
    /// sweep, which rewrites the file whole when it has removed anything.
    appended: usize,
}

/// The store. One per window, installed at boot ([`install`]) and found by
/// every road through [`store`].
pub(crate) struct Store {
    root: PathBuf,
    local_data_root: PathBuf,
    limits: Mutex<Limits>,
    window: Mutex<Option<tauri::AppHandle>>,
    index: Mutex<Index>,
    previews: Mutex<PreviewCache>,
    sources: Mutex<Vec<ScanSource>>,
    /// Serialize cursor load/read/save across boot, refresh and hook readers.
    pub(crate) transcript_reads: Mutex<()>,
    /// The ledger's `swept_at_ms` this store last swept beside; the artifact
    /// sweep rides the ledger's own hour rather than keeping a second clock.
    swept_beside: Mutex<Option<i64>>,
}

fn store_cell() -> &'static Mutex<Option<Arc<Store>>> {
    static CELL: OnceLock<Mutex<Option<Arc<Store>>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// Put the booted store where every road finds it.
pub(crate) fn install(store: Arc<Store>, app: tauri::AppHandle) {
    *store.window.lock().unwrap_or_else(PoisonError::into_inner) = Some(app);
    *store_cell().lock().unwrap_or_else(PoisonError::into_inner) = Some(store);
}

/// The window the booted store answers to, for a caller outside this file
/// that published through the store and must say the catalog moved.
pub(crate) fn window_handle() -> Option<tauri::AppHandle> {
    store()?
        .window
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The window's store, or `None` before boot — every caller reads that as
/// "nothing to register", never as an error.
pub(crate) fn store() -> Option<Arc<Store>> {
    store_cell()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

impl Store {
    /// Open the store under the local data root and read its catalog. A
    /// missing folder is a fresh store; a catalog line that does not parse
    /// is skipped, because one bad line must not cost the rows around it.
    pub(crate) fn open(local_data_root: &Path, limits: Limits) -> Self {
        let root = local_data_root.join(STORE_DIR_NAME);
        let mut rows = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(root.join(INDEX_FILE)) {
            for line in text.lines() {
                match serde_json::from_str::<IndexLine>(line) {
                    Ok(IndexLine::Row(artifact)) => {
                        rows.insert(
                            artifact.id.clone(),
                            Row {
                                artifact: *artifact,
                                tokens: Vec::new(),
                                stamp: None,
                            },
                        );
                    }
                    Ok(IndexLine::Tombstone { tombstone }) => {
                        rows.remove(&tombstone);
                    }
                    Err(_) => {}
                }
            }
        }
        // The cap holds at boot too: the newest rows win, the oldest are
        // forgotten rather than held past the table.
        while rows.len() > limits.index_rows_max {
            let Some(oldest) = rows
                .values()
                .min_by_key(|row| row.artifact.modified_ms)
                .map(|row| row.artifact.id.clone())
            else {
                break;
            };
            rows.remove(&oldest);
        }
        // A publication catalogued before rows named their editable source
        // (t-3952) reads it from its own metadata — one bounded read per such
        // row, and only until the next compaction writes it down.
        for row in rows.values_mut() {
            if is_publication(&row.artifact) && row.artifact.source_path.is_none() {
                row.artifact.source_path =
                    zerocode_core::artifact_publish::read_meta(&root, &row.artifact.id)
                        .ok()
                        .map(|meta| meta.source_path);
            }
        }
        // Tokens are rebuilt from disk lazily by the first scan; the rows the
        // catalog named are re-stamped there too.
        Self {
            root,
            local_data_root: local_data_root.to_path_buf(),
            limits: Mutex::new(limits),
            window: Mutex::new(None),
            index: Mutex::new(Index {
                rows,
                appended: 0,
                page_scan_after: None,
            }),
            previews: Mutex::new(PreviewCache::default()),
            sources: Mutex::new(Vec::new()),
            transcript_reads: Mutex::new(()),
            swept_beside: Mutex::new(None),
        }
    }

    /// Publish into this catalog under its existing lock; no watcher or timer of its own.
    pub(crate) fn publish_page(
        &self,
        input: &zerocode_core::artifact_publish::PublishInput,
    ) -> Result<zerocode_core::artifact_publish::PageMeta, String> {
        let limits = self.limits();
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let _lock = zerocode_core::artifact_publish::lock_store(&self.root)?;
        let meta = zerocode_core::artifact_publish::publish(&self.root, input, &limits)?;
        let artifact = meta.artifact(Origin::default());
        let row = Row {
            tokens: Self::tokens_of_words(
                &[&meta.title, meta.description.as_deref().unwrap_or_default()],
                &limits,
            ),
            stamp: stamp_of(&artifact.path),
            artifact,
        };
        self.insert_row(&mut index, row, &limits)?;
        Ok(meta)
    }

    pub(crate) fn limits(&self) -> Limits {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The store's folder — where its sidecars (versions, thumbnails, the
    /// transcript stamps) live beside the catalog.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The app's data root the store lives under — where the window's event
    /// log is written.
    pub(crate) fn local_data_root(&self) -> &Path {
        &self.local_data_root
    }

    /// Replace the table — the settings overlay's road in. A smaller preview
    /// cap takes effect by dropping what the cache holds.
    pub(crate) fn set_limits(&self, limits: Limits) {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner) = limits;
        self.previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn append_line(&self, line: &IndexLine) -> Result<(), String> {
        use std::io::Write as _;
        std::fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join(INDEX_FILE))
            .map_err(|error| error.to_string())?;
        let mut text = serde_json::to_string(line).map_err(|error| error.to_string())?;
        text.push('\n');
        file.write_all(text.as_bytes())
            .map_err(|error| error.to_string())
    }

    /// Rewrite the catalog from the rows in memory: the compaction that
    /// follows a sweep. Written beside and renamed over, like every durable
    /// file this window keeps.
    fn compact(&self, index: &mut Index) -> Result<(), String> {
        std::fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;
        let mut text = String::new();
        for row in index.rows.values() {
            text.push_str(
                &serde_json::to_string(&IndexLine::Row(Box::new(row.artifact.clone())))
                    .map_err(|error| error.to_string())?,
            );
            text.push('\n');
        }
        let target = self.root.join(INDEX_FILE);
        let tmp = self.root.join(format!("{INDEX_FILE}.tmp"));
        std::fs::write(&tmp, text).map_err(|error| error.to_string())?;
        std::fs::rename(&tmp, &target).map_err(|error| error.to_string())?;
        index.appended = 0;
        Ok(())
    }

    /// Read the front of a file for its preview and its search tokens —
    /// bounded by the table, never the whole of a large file.
    fn read_front(path: &Path, limits: &Limits) -> Vec<u8> {
        use std::io::Read as _;
        let Ok(file) = std::fs::File::open(path) else {
            return Vec::new();
        };
        let mut held = Vec::new();
        let _ = file
            .take(limits.body_index_bytes_max)
            .read_to_end(&mut held);
        held
    }

    fn build_row(seed: RowSeed<'_>, limits: &Limits) -> Row {
        let RowSeed {
            path,
            id,
            source,
            origin,
            stamp,
            now_ms,
            created_ms,
        } = seed;
        let kind = kind_of(path, source);
        let front = Self::read_front(path, limits);
        let preview = preview_of(kind, &front, limits);
        let title =
            artifact::descriptive_title(path, &front, origin.work_summary.as_deref(), limits);
        let mut tokens = zerocode_core::second_brain_related::tokenize(&title)
            .into_iter()
            .collect::<Vec<_>>();
        if matches!(
            kind,
            ArtifactKind::Report
                | ArtifactKind::Evidence
                | ArtifactKind::Transcript
                | ArtifactKind::Other
        ) && let Ok(text) = std::str::from_utf8(&front)
        {
            tokens.extend(body_tokens(text, limits));
        }
        tokens.sort();
        tokens.dedup();
        let modified_ms = epoch_ms(stamp.modified).max(0);
        Row {
            artifact: Artifact {
                id,
                kind,
                title,
                path: path.to_path_buf(),
                bytes: stamp.len,
                created_ms: created_ms.unwrap_or(if modified_ms > 0 {
                    modified_ms
                } else {
                    now_ms
                }),
                modified_ms: if modified_ms > 0 { modified_ms } else { now_ms },
                url: None,
                favicon: None,
                description: None,
                version: None,
                source_path: None,
                origin,
                tags: Vec::new(),
                preview,
                source,
            },
            tokens,
            stamp: Some(stamp),
        }
    }

    /// The search tokens of a row that has no file: its title, its
    /// description and its url, sorted and deduplicated like every other row's.
    fn tokens_of_words(words: &[&str], limits: &Limits) -> Vec<String> {
        let mut tokens: Vec<String> = Vec::new();
        for word in words {
            tokens.extend(body_tokens(word, limits));
        }
        tokens.sort();
        tokens.dedup();
        tokens
    }

    fn insert_row(&self, index: &mut Index, row: Row, limits: &Limits) -> Result<(), String> {
        // The cap: the oldest row gives its seat to the new one.
        while index.rows.len() >= limits.index_rows_max
            && !index.rows.contains_key(&row.artifact.id)
        {
            let Some(oldest) = index
                .rows
                .values()
                .min_by_key(|held| held.artifact.modified_ms)
                .map(|held| held.artifact.id.clone())
            else {
                break;
            };
            self.remove_row(index, &oldest)?;
        }
        self.append_line(&IndexLine::Row(Box::new(row.artifact.clone())))?;
        index.appended += 1;
        self.previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .evict(&row.artifact.id);
        index.rows.insert(row.artifact.id.clone(), row);
        Ok(())
    }

    /// Copy a worker's report into the store and catalog it. Idempotent: the
    /// same file under the same origin is the same artifact, and a source that
    /// has not changed since the copy is not copied again.
    pub(crate) fn register_report(
        &self,
        source_path: &Path,
        origin: Origin,
        now_ms: i64,
    ) -> Result<Artifact, String> {
        self.register_copy(source_path, Source::WorkerReport, origin, now_ms)
    }

    /// Copy any file into the store under its source's bucket — the report
    /// road above, and the hand-registration road (`manual`).
    pub(crate) fn register_copy(
        &self,
        source_path: &Path,
        source: Source,
        origin: Origin,
        now_ms: i64,
    ) -> Result<Artifact, String> {
        let stamp = stamp_of(source_path)
            .ok_or_else(|| format!("report file is not there: {}", source_path.display()))?;
        let limits = self.limits();
        if stamp.len > limits.scan_bytes_max {
            return Err(format!(
                "report is larger than the table allows ({} bytes)",
                stamp.len
            ));
        }
        let id = artifact_id(&origin, source_path);
        let name = source_path
            .file_name()
            .map(|held| held.to_string_lossy().into_owned())
            .unwrap_or_else(|| "report.md".to_string());
        let seat = self.root.join(source.bucket()).join(&id);
        let target = seat.join(&name);
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = index.rows.get(&id)
            && held.artifact.path == target
            && held.artifact.bytes == stamp.len
            && held.artifact.modified_ms == epoch_ms(stamp.modified).max(0)
            && target.is_file()
        {
            return Ok(held.artifact.clone());
        }
        std::fs::create_dir_all(&seat).map_err(|error| error.to_string())?;
        // Beside and renamed over: a copy that fails halfway leaves no
        // half-report under the artifact's name.
        let tmp = seat.join(format!("{name}.tmp"));
        std::fs::copy(source_path, &tmp).map_err(|error| error.to_string())?;
        std::fs::rename(&tmp, &target).map_err(|error| error.to_string())?;
        // The copy keeps the source's stamp in the row so "unchanged" can be
        // answered against the source next time, not against the copy.
        let created = index.rows.get(&id).map(|held| held.artifact.created_ms);
        let mut row = Self::build_row(
            RowSeed {
                path: &target,
                id,
                source,
                origin,
                stamp,
                now_ms,
                created_ms: created,
            },
            &limits,
        );
        row.stamp = stamp_of(&target);
        let artifact = row.artifact.clone();
        self.insert_row(&mut index, row, &limits)?;
        Ok(artifact)
    }

    /// The in-place road under the caller's lock; answers whether the row is
    /// new, updated or unchanged so the scan can count.
    fn register_in_place_locked(
        &self,
        index: &mut Index,
        path: &Path,
        source: Source,
        origin: Origin,
        now_ms: i64,
        limits: &Limits,
    ) -> Result<(Artifact, Option<bool>), String> {
        let stamp =
            stamp_of(path).ok_or_else(|| format!("file is not there: {}", path.display()))?;
        let id = artifact_id(&origin, path);
        if let Some(held) = index.rows.get_mut(&id) {
            if held.stamp == Some(stamp) && held.artifact.path == path {
                return Ok((held.artifact.clone(), None));
            }
            // Read back from the catalog at boot: the file is what the row
            // says, so re-stamp without re-reading.
            if held.stamp.is_none()
                && held.artifact.path == path
                && held.artifact.bytes == stamp.len
                && held.artifact.modified_ms == epoch_ms(stamp.modified).max(0)
            {
                held.stamp = Some(stamp);
                // Tokens were not persisted; rebuild them from the front of
                // the file once, bounded like everything else.
                if held.tokens.is_empty() {
                    let front = Self::read_front(path, limits);
                    let mut tokens =
                        zerocode_core::second_brain_related::tokenize(&held.artifact.title)
                            .into_iter()
                            .collect::<Vec<_>>();
                    if let Ok(text) = std::str::from_utf8(&front) {
                        tokens.extend(body_tokens(text, limits));
                    }
                    tokens.sort();
                    tokens.dedup();
                    held.tokens = tokens;
                }
                return Ok((held.artifact.clone(), None));
            }
        }
        let existed = index.rows.contains_key(&id);
        let created = index.rows.get(&id).map(|held| held.artifact.created_ms);
        let row = Self::build_row(
            RowSeed {
                path,
                id,
                source,
                origin,
                stamp,
                now_ms,
                created_ms: created,
            },
            limits,
        );
        let artifact = row.artifact.clone();
        // A page or document's every version is kept (t-3233 §5): the first
        // registration is V1, and each stamp the scan sees move is the next.
        if source == Source::AgentPage
            && matches!(artifact.kind, ArtifactKind::Page | ArtifactKind::Document)
        {
            let _ = self.snapshot_version(&artifact, limits);
        }
        self.insert_row(index, row, limits)?;
        Ok((artifact, Some(!existed)))
    }

    /// Copy the file as it is now into `versions/<id>/<n>/<name>`, `n` one
    /// past the newest kept, and drop the oldest past the table's count.
    /// Beside and renamed over, like every durable file this window keeps.
    fn snapshot_version(&self, artifact: &Artifact, limits: &Limits) -> Result<u32, String> {
        let seat = self.root.join(VERSIONS_DIR_NAME).join(&artifact.id);
        std::fs::create_dir_all(&seat).map_err(|error| error.to_string())?;
        let mut held = Self::version_numbers(&seat);
        let next = held.last().map_or(1, |newest| newest + 1);
        let name = artifact
            .path
            .file_name()
            .map(|held| held.to_string_lossy().into_owned())
            .unwrap_or_else(|| "page".to_string());
        let folder = seat.join(next.to_string());
        std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
        let tmp = folder.join(format!("{name}.tmp"));
        // The file may grow after stat. Copy at most the observed bytes,
        // within the page cap; a changing file must not become a truncated
        // snapshot with a valid version number.
        let copy = (|| {
            use std::io::Read as _;
            let source = std::fs::File::open(&artifact.path)?;
            let mut target = std::fs::File::create(&tmp)?;
            let cap = artifact.bytes.min(limits.page_bytes_max);
            let copied = std::io::copy(&mut source.take(cap.saturating_add(1)), &mut target)?;
            if copied != artifact.bytes || copied > cap {
                return Err(std::io::Error::other(
                    "page changed while saving its version",
                ));
            }
            std::fs::rename(&tmp, folder.join(&name))
        })();
        if let Err(error) = copy {
            let _ = std::fs::remove_dir_all(&folder);
            return Err(error.to_string());
        }
        held.push(next);
        while held.len() > limits.versions_per_artifact_max.max(1) {
            let oldest = held.remove(0);
            let _ = std::fs::remove_dir_all(seat.join(oldest.to_string()));
        }
        Ok(next)
    }

    /// The version numbers under one seat, ascending.
    fn version_numbers(seat: &Path) -> Vec<u32> {
        let mut numbers: Vec<u32> = std::fs::read_dir(seat)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
                    .collect()
            })
            .unwrap_or_default();
        numbers.sort_unstable();
        numbers
    }

    /// Every immutable version kept for one row, oldest first — what the
    /// page's version picker lists (t-3233 §5). A publication's are the
    /// versions its own store keeps (`pages/<id>/v<n>`, t-3952), with the
    /// digest each was written with; a page or document the transcript road
    /// follows has the scan's snapshots, digested here. Empty for a row that
    /// has none.
    pub(crate) fn versions(&self, id: &str) -> Vec<Version> {
        let published = zerocode_core::artifact_publish::versions(&self.root, id);
        if !published.is_empty() {
            return published
                .into_iter()
                .filter_map(|kept| {
                    let stamp = stamp_of(&kept.path)?;
                    Some(Version {
                        n: kept.n,
                        path: kept.path,
                        bytes: stamp.len,
                        modified_ms: epoch_ms(stamp.modified),
                        sha256: kept.sha256,
                    })
                })
                .collect();
        }
        let cap = self.limits().page_bytes_max;
        self.snapshot_versions(id)
            .into_iter()
            .map(|mut version| {
                version.sha256 = file_sha256(&version.path, cap);
                version
            })
            .collect()
    }

    /// The scan's snapshots of one page or document, oldest first, without
    /// digests — what retention walks.
    fn snapshot_versions(&self, id: &str) -> Vec<Version> {
        let seat = self.root.join(VERSIONS_DIR_NAME).join(id);
        Self::version_numbers(&seat)
            .into_iter()
            .filter_map(|n| {
                let folder = seat.join(n.to_string());
                let file = std::fs::read_dir(&folder)
                    .ok()?
                    .flatten()
                    .find_map(|entry| {
                        let path = entry.path();
                        (path.is_file() && !path.to_string_lossy().ends_with(".tmp"))
                            .then_some(path)
                    })?;
                let stamp = stamp_of(&file)?;
                Some(Version {
                    n,
                    path: file,
                    bytes: stamp.len,
                    modified_ms: epoch_ms(stamp.modified),
                    sha256: None,
                })
            })
            .collect()
    }

    /// A claude.ai artifact a transcript saw published (t-3233 §2a): one row
    /// per url, refreshed with the newer title and time when the fact is
    /// newer than the row, left alone when it is older. Idempotent.
    pub(crate) fn register_remote(
        &self,
        fact: &RemoteFact,
        agent: &str,
        now_ms: i64,
    ) -> Result<Artifact, String> {
        let url = fact.url.trim();
        if !url.starts_with("https://") {
            return Err(format!("not an artifact url: {url}"));
        }
        let limits = self.limits();
        let id = artifact_id_for_url(url);
        let at_ms = fact.at_ms.unwrap_or(now_ms);
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = index.rows.get(&id)
            && held.artifact.modified_ms >= at_ms
        {
            return Ok(held.artifact.clone());
        }
        let title = fact
            .title
            .clone()
            .filter(|held| !held.trim().is_empty())
            .unwrap_or_else(|| {
                url.rsplit('/')
                    .next()
                    .filter(|tail| !tail.is_empty())
                    .unwrap_or(url)
                    .to_string()
            });
        let title = artifact::truncate_chars(&title, limits.title_chars);
        let description = fact.description.clone().unwrap_or_default();
        let created_ms = index
            .rows
            .get(&id)
            .map_or(at_ms, |held| held.artifact.created_ms.min(at_ms));
        let tokens = Self::tokens_of_words(&[&title, &description, url], &limits);
        let row = Row {
            artifact: Artifact {
                id: id.clone(),
                kind: ArtifactKind::Web,
                title,
                path: PathBuf::new(),
                bytes: 0,
                created_ms,
                modified_ms: at_ms,
                url: Some(url.to_string()),
                favicon: fact.favicon.clone(),
                description: None,
                version: None,
                source_path: None,
                origin: Origin {
                    agent: Some(agent.to_string()),
                    session: fact.session.clone(),
                    project: fact.project.clone(),
                    ..Origin::default()
                },
                tags: Vec::new(),
                preview: if description.is_empty() {
                    Preview::None
                } else {
                    Preview::Text {
                        text: artifact::truncate_chars(&description, limits.preview_chars),
                    }
                },
                source: Source::Remote,
            },
            tokens,
            stamp: None,
        };
        let artifact = row.artifact.clone();
        self.insert_row(&mut index, row, &limits)?;
        Ok(artifact)
    }

    /// A page an agent wrote into its project (t-3233 §2b): registered where
    /// it is, never copied. `Ok(None)` when the file is not there, is larger
    /// than the table allows, or is not a page at all — none of which is an
    /// error, a transcript names files that have since moved on. Bounded per
    /// session: past `pages_per_session` the session's oldest row gives way.
    pub(crate) fn register_page(
        &self,
        fact: &PageFact,
        agent: &str,
        now_ms: i64,
    ) -> Result<Option<Artifact>, String> {
        if !artifact::is_page_extension(&fact.path) || !fact.path.is_absolute() {
            return Ok(None);
        }
        // Writers follow symlinks before ..; normalize on disk before the
        // final containment check, then key aliases as one physical page.
        let Some((path, project)) = fact
            .project
            .as_deref()
            .and_then(|root| Some((fact.path.canonicalize().ok()?, root.canonicalize().ok()?)))
        else {
            return Ok(None);
        };
        if !path.starts_with(&project) {
            return Ok(None);
        }
        let Some(stamp) = stamp_of(&path) else {
            return Ok(None);
        };
        let limits = self.limits();
        if stamp.len > limits.page_bytes_max {
            return Ok(None);
        }
        let origin = Origin {
            agent: Some(agent.to_string()),
            session: fact.session.clone(),
            project: Some(project),
            ..Origin::default()
        };
        let id = artifact_id(&Origin::default(), &path);
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        if !index.rows.contains_key(&id)
            && let Some(session) = fact.session.as_deref()
        {
            let mut same: Vec<(i64, String)> = index
                .rows
                .values()
                .filter(|row| {
                    row.artifact.source == Source::AgentPage
                        && row.artifact.origin.session.as_deref() == Some(session)
                })
                .map(|row| (row.artifact.modified_ms, row.artifact.id.clone()))
                .collect();
            same.sort();
            while same.len() >= limits.pages_per_session.max(1) {
                let (_, oldest) = same.remove(0);
                self.remove_row(&mut index, &oldest)?;
            }
        }
        let (artifact, _) = self.register_in_place_locked(
            &mut index,
            &path,
            Source::AgentPage,
            origin,
            now_ms,
            &limits,
        )?;
        Ok(Some(artifact))
    }

    /// Every row the transcript road registered (t-3233 §2b) — what a change
    /// of that road's rule must judge again (t-3952).
    pub(crate) fn agent_pages(&self) -> Vec<Artifact> {
        self.index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .rows
            .values()
            .filter(|row| row.artifact.source == Source::AgentPage)
            .map(|row| row.artifact.clone())
            .collect()
    }

    /// Drop transcript-road rows from the catalog with their sidecars (their
    /// snapshots and thumbnails). The files they point at are the project's
    /// and are never touched; any other row is refused. Answers how many went.
    pub(crate) fn forget_agent_pages(&self, ids: &[String]) -> usize {
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut gone = 0;
        for id in ids {
            let agent_page = index
                .rows
                .get(id)
                .is_some_and(|row| row.artifact.source == Source::AgentPage);
            if agent_page && matches!(self.remove_row(&mut index, id), Ok(Some(_))) {
                gone += 1;
            }
        }
        gone
    }

    /// Register a folder the scan walks. The newest registrations win the
    /// table's seats; a folder registered twice keeps one seat.
    pub(crate) fn adopt_source(&self, source: ScanSource) {
        let limits = self.limits();
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        sources.retain(|held| held.dir != source.dir);
        sources.push(source);
        while sources.len() > limits.scan_sources_max {
            sources.remove(0);
        }
    }

    /// The folders the file watcher's artifact lane follows: the store's own
    /// root and the newest registered sources, bounded by the table.
    pub(crate) fn watch_targets(&self) -> Vec<(String, Option<PathBuf>)> {
        let limits = self.limits();
        let sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        // The root and its three buckets: a copied report lands as a new
        // folder under a bucket, and that is the folder whose stamp moves.
        let mut targets = vec![(format!("{WATCH_LANE}:root"), Some(self.root.clone()))];
        for bucket in [Source::WorkerReport, Source::Evidence, Source::Manual] {
            targets.push((
                format!("{WATCH_LANE}:{}", bucket.bucket()),
                Some(self.root.join(bucket.bucket())),
            ));
        }
        for source in sources
            .iter()
            .rev()
            .take(limits.watch_dirs_max.saturating_sub(targets.len()))
        {
            targets.push((
                format!("{WATCH_LANE}:{}", source.dir.display()),
                Some(source.dir.clone()),
            ));
        }
        // The pages registered one by one (t-3233 §2b): the newest keep the
        // seats the table has left, so a page an agent is rewriting now is
        // heard and re-versioned while an old one waits for the next scan.
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut pages: Vec<&Row> = index
            .rows
            .values()
            .filter(|row| row.artifact.source == Source::AgentPage)
            .collect();
        pages.sort_by_key(|row| std::cmp::Reverse(row.artifact.modified_ms));
        for row in pages
            .into_iter()
            .take(limits.watch_dirs_max.saturating_sub(targets.len()))
        {
            targets.push((
                format!("{WATCH_LANE}:{}", row.artifact.path.display()),
                Some(row.artifact.path.clone()),
            ));
        }
        targets
    }

    /// One bounded, incremental pass over every registered folder and the
    /// store's own files.
    ///
    /// Bounded: depth, files and bytes from the table, and `truncated` says
    /// which pass hit one. Incremental: a file whose stamp matches its row is
    /// skipped without a read. A row whose file is gone is removed — for the
    /// in-place sources only; a copied report is the store's own file and its
    /// absence is a bug to notice, not a row to drop silently.
    pub(crate) fn scan(&self, now_ms: i64) -> ScanReport {
        let limits = self.limits();
        let sources = self
            .sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut report = ScanReport::default();
        let mut budget = ScanBudget {
            files: limits.scan_files_max,
            bytes: limits.scan_bytes_max,
        };
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut seen_paths: Vec<PathBuf> = Vec::new();
        // Upgrade filename-only catalogs on their first bounded scan. Explicit
        // published titles stay authoritative; no extra watcher or UI reads.
        let legacy: Vec<String> = index
            .rows
            .iter()
            .filter(|(_, row)| {
                row.stamp.is_none()
                    && row.artifact.url.is_none()
                    && row.artifact.title == title_of(&row.artifact.path, &limits)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in legacy {
            let Some(row) = index.rows.get(&id) else {
                continue;
            };
            if !budget.visit_file()
                || !budget.read_bytes(row.artifact.bytes.min(limits.body_index_bytes_max))
            {
                report.truncated = true;
                break;
            }
            let front = Self::read_front(&row.artifact.path, &limits);
            let title = artifact::descriptive_title(
                &row.artifact.path,
                &front,
                row.artifact.origin.work_summary.as_deref(),
                &limits,
            );
            if title != row.artifact.title {
                let mut row = row.clone();
                row.artifact.title = title;
                row.tokens = Self::tokens_of_words(
                    &[
                        &row.artifact.title,
                        std::str::from_utf8(&front).unwrap_or_default(),
                    ],
                    &limits,
                );
                if self.insert_row(&mut index, row, &limits).is_ok() {
                    report.updated += 1;
                }
            }
        }
        for source in &sources {
            let (mut found, cut) = Self::walk(&source.dir, limits.scan_depth, &mut budget);
            report.truncated |= cut;
            found.sort();
            for path in found {
                report.seen += 1;
                match self.register_in_place_locked(
                    &mut index,
                    &path,
                    source.source,
                    source.origin.clone(),
                    now_ms,
                    &limits,
                ) {
                    Ok((_, Some(true))) => report.added += 1,
                    Ok((_, Some(false))) => report.updated += 1,
                    Ok((_, None)) | Err(_) => {}
                }
                seen_paths.push(path);
            }
        }
        // The store's own copies: re-stamped, never dropped.
        let own: Vec<(String, PathBuf)> = index
            .rows
            .values()
            .filter(|row| row.artifact.path.starts_with(&self.root))
            .map(|row| (row.artifact.id.clone(), row.artifact.path.clone()))
            .collect();
        for (id, path) in own {
            if let Some(row) = index.rows.get_mut(&id)
                && row.stamp.is_none()
            {
                row.stamp = stamp_of(&path);
                if row.tokens.is_empty() {
                    let front = Self::read_front(&path, &limits);
                    let mut tokens =
                        zerocode_core::second_brain_related::tokenize(&row.artifact.title)
                            .into_iter()
                            .collect::<Vec<_>>();
                    if let Ok(text) = std::str::from_utf8(&front) {
                        tokens.extend(body_tokens(text, &limits));
                    }
                    tokens.sort();
                    tokens.dedup();
                    row.tokens = tokens;
                }
            }
        }
        // Pages registered one by one (t-3233 §2b) have no source folder:
        // each is asked its stamp — one `stat` — and the row goes when the
        // file is gone, moves (and keeps a version) when the file did.
        let mut pages: Vec<(String, PathBuf, Option<Stamp>, Origin)> = index
            .rows
            .values()
            .filter(|row| row.artifact.source == Source::AgentPage)
            .map(|row| {
                (
                    row.artifact.id.clone(),
                    row.artifact.path.clone(),
                    row.stamp,
                    row.artifact.origin.clone(),
                )
            })
            .collect();
        if let Some(after) = &index.page_scan_after {
            let start = pages.partition_point(|(id, _, _, _)| id <= after);
            pages.rotate_left(start);
        }
        for (id, path, stamp, origin) in pages {
            if !budget.visit_file() {
                report.truncated = true;
                break;
            }
            index.page_scan_after = Some(id.clone());
            match stamp_of(&path) {
                None => {
                    if self.remove_row(&mut index, &id).is_ok() {
                        report.removed += 1;
                    }
                }
                Some(now) if stamp != Some(now) => {
                    report.seen += 1;
                    if now.len > limits.page_bytes_max || !budget.read_bytes(now.len) {
                        report.truncated = true;
                        continue;
                    }
                    if let Ok((_, Some(_))) = self.register_in_place_locked(
                        &mut index,
                        &path,
                        Source::AgentPage,
                        origin,
                        now_ms,
                        &limits,
                    ) {
                        report.updated += 1;
                    }
                }
                Some(_) => {}
            }
        }
        // Rows of in-place files that are no longer there — only under
        // folders this pass actually finished walking.
        if !report.truncated {
            let gone: Vec<String> = index
                .rows
                .values()
                .filter(|row| {
                    row.artifact.source != Source::WorkerReport
                        && !row.artifact.path.starts_with(&self.root)
                        && sources
                            .iter()
                            .any(|source| row.artifact.path.starts_with(&source.dir))
                        && !row.artifact.path.is_file()
                })
                .map(|row| row.artifact.id.clone())
                .collect();
            for id in gone {
                if self.remove_row(&mut index, &id).is_ok() {
                    report.removed += 1;
                }
            }
        }
        report
    }

    /// Walk one folder to the table's depth, spending the shared budget.
    fn walk(dir: &Path, depth: usize, budget: &mut ScanBudget) -> (Vec<PathBuf>, bool) {
        let mut found = Vec::new();
        let mut cut = false;
        let mut pending: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
        while let Some((folder, level)) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if level + 1 < depth {
                        pending.push((path, level + 1));
                    } else {
                        cut = true;
                    }
                    continue;
                }
                if !kind.is_file() {
                    continue;
                }
                // The catalog's own files are not artifacts.
                if path.file_name().is_some_and(|name| {
                    name == INDEX_FILE || name.to_string_lossy().ends_with(".tmp")
                }) {
                    continue;
                }
                if !budget.visit_file() {
                    cut = true;
                    return (found, cut);
                }
                let len = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                if !budget.read_bytes(len) {
                    cut = true;
                    return (found, cut);
                }
                found.push(path);
            }
        }
        (found, cut)
    }

    fn remove_row(&self, index: &mut Index, id: &str) -> Result<Option<Artifact>, String> {
        let Some(row) = index.rows.remove(id) else {
            return Ok(None);
        };
        self.append_line(&IndexLine::Tombstone {
            tombstone: id.to_string(),
        })?;
        index.appended += 1;
        self.previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .evict(id);
        self.forget_sidecars(id);
        Ok(Some(row.artifact))
    }

    /// Whether a file is the store's to delete: under the app's own data
    /// root. A person's export folder is theirs; a row can go, the file stays.
    fn owns(&self, path: &Path) -> bool {
        path.starts_with(&self.local_data_root)
    }

    /// Remove one artifact — only when the person confirmed. Without the
    /// confirmation this is a no-op that answers `false`, so a caller that
    /// forgot to ask cannot delete by accident. The copied file (or the
    /// in-place file, when it is under the app's data root) goes with the row.
    pub(crate) fn delete(&self, id: &str, confirmed: bool) -> Result<bool, String> {
        if !confirmed {
            return Ok(false);
        }
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(artifact) = self.remove_row(&mut index, id)? else {
            return Ok(false);
        };
        if self.owns(&artifact.path) {
            let seat = self.root.join(artifact.source.bucket()).join(&artifact.id);
            if artifact.path.starts_with(&seat) {
                let _ = std::fs::remove_dir_all(&seat);
            } else {
                let _ = std::fs::remove_file(&artifact.path);
            }
        }
        self.forget_sidecars(&artifact.id);
        Ok(true)
    }

    /// The store's own files beside a row — its versions and its rendered
    /// thumbnail — go with the row, whoever owned the file itself.
    fn forget_sidecars(&self, id: &str) {
        let _ = std::fs::remove_dir_all(
            self.root
                .join(zerocode_core::artifact_publish::PAGES_DIR)
                .join(id),
        );
        let _ = std::fs::remove_dir_all(self.root.join(VERSIONS_DIR_NAME).join(id));
        let _ = std::fs::remove_file(self.root.join(THUMBS_DIR_NAME).join(format!("{id}.png")));
        let _ = std::fs::remove_file(self.root.join(THUMBS_DIR_NAME).join(format!("{id}.key")));
    }

    /// Drop every row whose file lived under one of the given folders — the
    /// automation runtime's road when it sweeps evidence folders. Files are
    /// the caller's; only the catalog moves here.
    pub(crate) fn forget_under(&self, dirs: &[PathBuf]) -> usize {
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let gone: Vec<String> = index
            .rows
            .values()
            .filter(|row| dirs.iter().any(|dir| row.artifact.path.starts_with(dir)))
            .map(|row| row.artifact.id.clone())
            .collect();
        let mut count = 0;
        for id in gone {
            if self.remove_row(&mut index, &id).is_ok() {
                count += 1;
            }
        }
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        sources.retain(|source| !dirs.iter().any(|dir| source.dir.starts_with(dir)));
        count
    }

    /// The retention sweep: rows older than `days` go, and the store's own
    /// files with them; the catalog is compacted afterwards so the tombstones
    /// do not outlive what they buried.
    pub(crate) fn prune(&self, now_ms: i64, days: u32) -> Pruned {
        let horizon = now_ms - i64::from(days.max(1)) * 24 * 60 * 60 * 1000;
        let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let old: Vec<Artifact> = index
            .rows
            .values()
            .filter(|row| row.artifact.modified_ms < horizon)
            .map(|row| row.artifact.clone())
            .collect();
        let mut pruned = Pruned::default();
        // Active pages also shed old versions on the ledger's retention
        // beat. Keep the newest snapshot so version numbers never restart.
        // A publication's own versions are its store's (bounded by
        // `versions_per_artifact_max` at publish), never retention's.
        for row in index.rows.values() {
            let mut versions = self.snapshot_versions(&row.artifact.id);
            versions.pop();
            for version in versions {
                if version.modified_ms < horizon
                    && let Some(folder) = version.path.parent()
                    && std::fs::remove_dir_all(folder).is_ok()
                {
                    pruned.files += 1;
                }
            }
        }
        for artifact in old {
            index.rows.remove(&artifact.id);
            pruned.rows += 1;
            if self.owns(&artifact.path) {
                let seat = self.root.join(artifact.source.bucket()).join(&artifact.id);
                let removed = if artifact.path.starts_with(&seat) {
                    std::fs::remove_dir_all(&seat).is_ok()
                } else {
                    std::fs::remove_file(&artifact.path).is_ok()
                };
                if removed {
                    pruned.files += 1;
                }
            }
            self.forget_sidecars(&artifact.id);
            self.previews
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .evict(&artifact.id);
        }
        if pruned.rows > 0 || index.appended > 0 {
            let _ = self.compact(&mut index);
        }
        pruned
    }

    /// Sweep beside the ledger: once per ledger sweep, with the days the
    /// table (or its settings overlay) says. `None` when the ledger has not
    /// swept since the last look — the ordinary answer on every beat but one.
    pub(crate) fn sweep_beside_ledger(
        &self,
        ledger_swept_at_ms: Option<i64>,
        ledger_days: u32,
        now_ms: i64,
    ) -> Option<Pruned> {
        let mut beside = self
            .swept_beside
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *beside == ledger_swept_at_ms {
            return None;
        }
        *beside = ledger_swept_at_ms;
        drop(beside);
        let days = self.limits().effective_retention_days(ledger_days);
        let pruned = self.prune(now_ms, days);
        if pruned.rows > 0 {
            crate::system_runtime::note_window_event(
                &self.local_data_root,
                &format!(
                    "artifacts: retention swept {} rows and {} files older than {days} days",
                    pruned.rows, pruned.files
                ),
            );
        }
        Some(pruned)
    }

    /// List rows the filter admits, newest first, bounded by the table.
    pub(crate) fn list(&self, filter: &Filter) -> Listing {
        let limits = self.limits();
        let kind = filter.kind.as_deref().and_then(ArtifactKind::parse);
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut rows: Vec<&Row> = index
            .rows
            .values()
            .filter(|row| kind.is_none_or(|wanted| row.artifact.kind == wanted))
            .filter(|row| {
                filter
                    .remote
                    .is_none_or(|remote| (row.artifact.kind == ArtifactKind::Web) == remote)
            })
            .filter(|row| {
                filter
                    .published
                    .is_none_or(|published| is_publication(&row.artifact) == published)
            })
            .filter(|row| {
                let origin = &row.artifact.origin;
                let same = |asked: &Option<String>, held: Option<&str>| {
                    asked.as_deref().is_none_or(|wanted| held == Some(wanted))
                };
                same(&filter.run, origin.run.as_deref())
                    && same(&filter.task, origin.task.as_deref())
                    && same(&filter.worker, origin.worker.as_deref())
                    && same(&filter.automation, origin.automation.as_deref())
                    && same(&filter.agent, origin.agent.as_deref())
                    && filter.worktree.as_deref().is_none_or(|wanted| {
                        origin
                            .worktree
                            .as_deref()
                            .is_some_and(|held| held == Path::new(wanted))
                    })
            })
            .filter(|row| {
                filter.query.trim().is_empty() || query_matches(&filter.query, &row.tokens)
            })
            .collect();
        rows.sort_by(|a, b| {
            b.artifact
                .modified_ms
                .cmp(&a.artifact.modified_ms)
                .then_with(|| a.artifact.id.cmp(&b.artifact.id))
        });
        let total = rows.len();
        let truncated = total > limits.list_rows_max;
        Listing {
            rows: rows
                .into_iter()
                .take(limits.list_rows_max)
                .map(|row| row.artifact.clone())
                .collect(),
            total,
            truncated,
        }
    }

    /// Counts by origin, for the chips.
    pub(crate) fn counts(&self) -> Counts {
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut counts = Counts::default();
        for row in index.rows.values() {
            counts.total += 1;
            let origin = &row.artifact.origin;
            let bump = |map: &mut BTreeMap<String, usize>, key: Option<String>| {
                if let Some(key) = key {
                    *map.entry(key).or_insert(0) += 1;
                }
            };
            bump(&mut counts.by_worker, origin.worker.clone());
            bump(&mut counts.by_task, origin.task.clone());
            bump(&mut counts.by_run, origin.run.clone());
            bump(
                &mut counts.by_worktree,
                origin
                    .worktree
                    .as_ref()
                    .map(|held| held.display().to_string()),
            );
            bump(&mut counts.by_automation, origin.automation.clone());
            if row.artifact.kind == ArtifactKind::Report
                && let Some(worker) = origin.worker.clone()
            {
                let newer = counts
                    .report_by_worker
                    .get(&worker)
                    .and_then(|held| index.rows.get(held))
                    .is_none_or(|held| held.artifact.modified_ms < row.artifact.modified_ms);
                if newer {
                    counts
                        .report_by_worker
                        .insert(worker, row.artifact.id.clone());
                }
            }
        }
        counts
    }

    /// One row.
    pub(crate) fn get(&self, id: &str) -> Option<Artifact> {
        self.index
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .rows
            .get(id)
            .map(|row| row.artifact.clone())
    }

    /// The artifacts one worker left, newest first — what `worker-show` is
    /// garnished with.
    pub(crate) fn ids_for_worker(&self, worker: &str) -> Vec<(String, ArtifactKind)> {
        let index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
        let mut rows: Vec<&Row> = index
            .rows
            .values()
            .filter(|row| row.artifact.origin.worker.as_deref() == Some(worker))
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.artifact.modified_ms));
        rows.iter()
            .map(|row| (row.artifact.id.clone(), row.artifact.kind))
            .collect()
    }

    /// What the drawer draws for one row, through the byte-capped LRU.
    /// Images are read and encoded only here — on request — and never for a
    /// card that is merely listed.
    pub(crate) fn preview(&self, id: &str) -> Result<Arc<PreviewPayload>, String> {
        if let Some(held) = self
            .previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            return Ok(held);
        }
        let artifact = self
            .get(id)
            .ok_or_else(|| format!("unknown artifact: {id}"))?;
        let limits = self.limits();
        // A claude.ai artifact has no file: its description is the drawer's
        // text, and the browser door draws the rest (t-3233 §3).
        if artifact.kind == ArtifactKind::Web {
            let text = match &artifact.preview {
                Preview::Text { text } => Some(text.clone()),
                _ => None,
            };
            let payload = Arc::new(PreviewPayload {
                kind: if text.is_some() { "text" } else { "none" },
                text,
                data_url: None,
                bytes: 0,
                truncated: false,
            });
            return Ok(payload);
        }
        let meta = std::fs::metadata(&artifact.path).map_err(|error| error.to_string())?;
        let bytes = meta.len();
        let payload = match artifact.kind {
            ArtifactKind::Screenshot => {
                if bytes > limits.preview_image_bytes_max {
                    PreviewPayload {
                        kind: "image",
                        text: None,
                        data_url: None,
                        bytes,
                        truncated: true,
                    }
                } else {
                    use base64::Engine as _;
                    let data = std::fs::read(&artifact.path).map_err(|error| error.to_string())?;
                    let mime = image_mime(&artifact.path);
                    PreviewPayload {
                        kind: "image",
                        text: None,
                        data_url: Some(format!(
                            "data:{mime};base64,{}",
                            base64::engine::general_purpose::STANDARD.encode(data)
                        )),
                        bytes,
                        truncated: false,
                    }
                }
            }
            ArtifactKind::Export | ArtifactKind::Page | ArtifactKind::Web => PreviewPayload {
                kind: "none",
                text: None,
                data_url: None,
                bytes,
                truncated: false,
            },
            // A PDF document is not text; its bytes fall to `none` below.
            ArtifactKind::Document if !is_utf8_document(&artifact.path) => PreviewPayload {
                kind: "none",
                text: None,
                data_url: None,
                bytes,
                truncated: false,
            },
            ArtifactKind::Report
            | ArtifactKind::Document
            | ArtifactKind::Evidence
            | ArtifactKind::Transcript
            | ArtifactKind::Other => {
                use std::io::Read as _;
                let file =
                    std::fs::File::open(&artifact.path).map_err(|error| error.to_string())?;
                let mut held = Vec::new();
                file.take(limits.preview_text_bytes_max)
                    .read_to_end(&mut held)
                    .map_err(|error| error.to_string())?;
                let text = String::from_utf8_lossy(&held).into_owned();
                PreviewPayload {
                    kind: if matches!(artifact.kind, ArtifactKind::Report | ArtifactKind::Document)
                    {
                        "markdown"
                    } else {
                        "text"
                    },
                    text: Some(text),
                    data_url: None,
                    bytes,
                    truncated: bytes > limits.preview_text_bytes_max,
                }
            }
        };
        let payload = Arc::new(payload);
        self.previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .put(id, Arc::clone(&payload), limits.preview_cache_bytes);
        Ok(payload)
    }
}

struct ScanBudget {
    files: usize,
    bytes: u64,
}

impl ScanBudget {
    fn visit_file(&mut self) -> bool {
        let Some(left) = self.files.checked_sub(1) else {
            return false;
        };
        self.files = left;
        true
    }

    fn read_bytes(&mut self, bytes: u64) -> bool {
        let Some(left) = self.bytes.checked_sub(bytes) else {
            return false;
        };
        self.bytes = left;
        true
    }
}

/// Whether a row is a publication (t-3952): the only rows that carry a
/// version number of their own, written by `artifact_publish::PageMeta`.
fn is_publication(artifact: &Artifact) -> bool {
    artifact.kind == ArtifactKind::Page && artifact.version.is_some()
}

/// Whether a document is Markdown rather than a PDF — by extension, the way
/// the kind table judged it.
fn is_utf8_document(path: &Path) -> bool {
    !path
        .extension()
        .is_some_and(|held| held.to_string_lossy().eq_ignore_ascii_case("pdf"))
}

/// The SHA-256 of bytes in hand, hex — the same digest `file_sha256` answers
/// for a file, for a picture already read.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// The SHA-256 of a whole file, hex — `None` when it cannot be read or is
/// larger than `cap`, because a digest of part of a file is not that file's.
pub(crate) fn file_sha256(path: &Path, cap: u64) -> Option<String> {
    use sha2::{Digest as _, Sha256};
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > cap {
        return None;
    }
    let mut reader = file.take(cap.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > cap {
            return None;
        }
        hasher.update(&buffer[..read]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

fn image_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|held| held.to_string_lossy().to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "image/jpeg",
    }
}

/// The origin a ledger row vouches for. Only what the row holds: the seat,
/// the agent and model it was launched as, the checkout it was placed in, the
/// task its dispatch carries. No commit — the ledger records none.
pub(crate) fn origin_of_worker(
    run: &zerocode_core::orchestration::Run,
    worker: &zerocode_core::orchestration::Worker,
) -> Origin {
    let task =
        worker_task_id(&run.dispatches, &worker.id, worker.dispatch.as_deref()).map(str::to_string);
    Origin {
        work_summary: task
            .as_deref()
            .and_then(|id| run.task(id))
            .map(|task| task.display_name().to_string()),
        run: Some(run.id.clone()),
        task,
        worker: Some(worker.id.clone()),
        pane: Some(format!("{}/{}", worker.team, worker.pane)),
        agent: Some(worker.agent.clone()),
        model: worker.model.clone(),
        commit: None,
        worktree: worker.checkout.as_deref().map(PathBuf::from),
        automation: None,
        session: None,
        project: None,
    }
}

// Completing a dispatch clears the worker's active pointer before its report
// is registered. The immutable dispatch history still names the work it did.
fn worker_task_id<'a>(
    dispatches: &'a [zerocode_core::orchestration::Dispatch],
    worker: &str,
    active: Option<&str>,
) -> Option<&'a str> {
    active
        .and_then(|id| dispatches.iter().find(|d| d.id == id && d.worker == worker))
        .or_else(|| {
            dispatches
                .iter()
                .filter(|d| d.worker == worker)
                .max_by_key(|d| d.started_ms)
        })
        .map(|dispatch| dispatch.task.as_str())
}

/// The report path a `worker_done` names: the payload's `reportPath` first,
/// else the first absolute Markdown path the body mentions. `None` when the
/// message names nothing — most `worker_done`s do not.
pub(crate) fn report_path_in(payload: Option<&str>, body: &str) -> Option<PathBuf> {
    if let Some(payload) = payload
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(payload)
        && let Some(path) = value["reportPath"].as_str()
        && Path::new(path).is_absolute()
    {
        return Some(PathBuf::from(path));
    }
    body.split(|ch: char| ch.is_whitespace() || matches!(ch, '`' | '"' | '\'' | '(' | ')' | ','))
        .find(|word| word.starts_with('/') && word.ends_with(".md"))
        .map(PathBuf::from)
}

/// The window's road after the file watcher's lane reported movement, or
/// after boot: one bounded pass, then the watcher's lane is re-aimed at the
/// folders the store now knows, and the window hears about any change.
pub(crate) fn rescan_and_tell(app: &tauri::AppHandle, now_ms: i64) -> Option<ScanReport> {
    use tauri::{Emitter as _, Manager as _};
    let store = store()?;
    let report = store.scan(now_ms);
    app.state::<AppState>()
        .watched()
        .replace_lane(WATCH_LANE, store.watch_targets(), true);
    if report.changed() {
        let _ = app.emit(CHANGED_EVENT, ());
    }
    Some(report)
}

/// The hook road (t-3233 §2): a Claude or zo pane's `Stop`/`PostToolUse`
/// names its transcript, and the tail past the remembered stamp is read for
/// the artifacts it published and the pages it wrote. The window hears
/// `artifacts:changed` only when a row landed. Cheap when the envelope is
/// not one of those — a string look, no disk.
pub(crate) fn note_hook(app: &tauri::AppHandle, envelope: &zerocode_core::HookEnvelope) {
    use tauri::Emitter as _;
    if !envelope.payload.contains("transcript_path") && !envelope.payload.contains("cwd") {
        return;
    }
    let Some(store) = store() else {
        return;
    };
    if crate::artifact_transcripts::note_hook(&store, envelope, crate::now_epoch_ms()) {
        let _ = app.emit(CHANGED_EVENT, ());
    }
}

/// The boot road runs bounded passes on the existing reconcile thread until
/// legacy history is judged. Refresh uses the same road; no manual refresh
/// loop or additional thread is needed to finish a long migration.
pub(crate) fn backfill_transcripts(
    app: &tauri::AppHandle,
) -> Option<crate::artifact_transcripts::BackfillReport> {
    use tauri::{Emitter as _, Manager as _};
    let store = store()?;
    let home = dirs::home_dir()?;
    let window_home = app.state::<AppState>().config_root().to_path_buf();
    let roots = crate::artifact_transcripts::roots(&home, &window_home);
    let mut total = crate::artifact_transcripts::BackfillReport::default();
    loop {
        let report = crate::artifact_transcripts::backfill(&store, &roots, crate::now_epoch_ms());
        if report.changed() {
            app.state::<AppState>()
                .watched()
                .replace_lane(WATCH_LANE, store.watch_targets(), true);
            let _ = app.emit(CHANGED_EVENT, ());
        }
        total.files += report.files;
        total.bytes += report.bytes;
        total.remote += report.remote;
        total.pages += report.pages;
        total.forgotten += report.forgotten;
        total.truncated |= report.truncated;
        total.rejudging = report.rejudging;
        if report.rejudging == 0 {
            return Some(total);
        }
        // Each pass releases the transcript lock, allowing hooks to add
        // durable creation evidence before the next cleanup decision.
        std::thread::yield_now();
    }
}

/// The automation runtime's registration road: one run's evidence folder,
/// with the origin the run row vouches for. The folder is walked by the next
/// scan; nothing is read here.
pub(crate) fn adopt_evidence(dir: &str, automation_id: &str, worktree: Option<&str>) {
    let Some(store) = store() else {
        return;
    };
    store.adopt_source(ScanSource {
        dir: PathBuf::from(dir),
        source: Source::Evidence,
        origin: Origin {
            automation: Some(automation_id.to_string()),
            worktree: worktree.map(PathBuf::from),
            ..Origin::default()
        },
    });
}

/// The automation runtime's other registration road: these evidence folders
/// were swept, so their rows go too. Only the catalog moves; the files were
/// the caller's to remove.
pub(crate) fn forget_evidence(dirs: &[PathBuf]) {
    if let Some(store) = store()
        && !dirs.is_empty()
    {
        store.forget_under(dirs);
    }
}

/// The table's cap on retention days — the one number the settings field
/// shows as its `max`, read from here so settings never spell it.
pub(crate) fn retention_days_max() -> u32 {
    Limits::default().retention_days_max
}

/// The settings overlay's road: the person changed the retention days.
/// Answers the days as the table admits them (clamped), which is what the
/// settings document stores.
pub(crate) fn note_retention_days(days: u32) -> u32 {
    let days = days.min(retention_days_max());
    let Some(store) = store() else {
        return days;
    };
    let overlay: BTreeMap<String, u64> = [(
        format!("{}retention_days", artifact::OVERLAY_PREFIX),
        u64::from(days),
    )]
    .into_iter()
    .collect();
    store.set_limits(Limits::default().overlaid(&overlay));
    days
}

/// The authenticated hook route uses the same store that the gallery reads.
pub(crate) struct ArtifactDoor;
impl zerocode_hookd::ArtifactCommands for ArtifactDoor {
    fn execute(&self, request: serde_json::Value) -> Result<serde_json::Value, String> {
        let store = store().ok_or("artifact store is not ready")?;
        artifact_request(&store, request)
    }
}

fn artifact_request(
    store: &Store,
    mut request: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let action = request
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    match action.as_str() {
        "publish" => {
            request
                .as_object_mut()
                .ok_or("expected object")?
                .remove("action");
            let input = serde_json::from_value(request).map_err(|e| e.to_string())?;
            let meta = store.publish_page(&input)?;
            if let Some(app) = store
                .window
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
            {
                use tauri::Emitter as _;
                let _ = app.emit(CHANGED_EVENT, ());
            }
            serde_json::to_value(meta).map_err(|e| e.to_string())
        }
        // Publications only, as the headless catalog answers: a page an agent
        // wrote into its project is the gallery's, not this door's to read
        // or export by version (t-3952).
        "list" => serde_json::to_value(store.list(&Filter {
            kind: Some("page".into()),
            published: Some(true),
            ..Filter::default()
        }))
        .map_err(|e| e.to_string()),
        "read" => {
            let id = request
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or("read needs id")?;
            let row = store.get(id).ok_or("artifact not found")?;
            let html = zerocode_core::artifact_publish::read_page(store.root(), &row)?;
            Ok(serde_json::json!({ "artifact": row, "html": html }))
        }
        "export" => {
            request
                .as_object_mut()
                .ok_or("expected object")?
                .remove("action");
            let input: zerocode_core::artifact_publish::ExportInput =
                serde_json::from_value(request).map_err(|e| e.to_string())?;
            store.get(&input.id).ok_or("artifact not found")?;
            serde_json::to_value(zerocode_core::artifact_publish::export(
                store.root(),
                &input,
            )?)
            .map_err(|e| e.to_string())
        }
        _ => Err("Artifact action must be publish, list, read or export".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only doors onto the store, beside the tests so the shipped half
    /// of this file ends where they begin.
    impl Store {
        /// Catalog a file where it is — an evidence frame, an export — without
        /// copying it. The row is refreshed when the file changed, kept when not.
        /// The scan is the shipped road onto `register_in_place_locked`; this
        /// door is the tests' way of placing one file without a source folder.
        pub(crate) fn register_in_place(
            &self,
            path: &Path,
            source: Source,
            origin: Origin,
            now_ms: i64,
        ) -> Result<Artifact, String> {
            let limits = self.limits();
            let mut index = self.index.lock().unwrap_or_else(PoisonError::into_inner);
            self.register_in_place_locked(&mut index, path, source, origin, now_ms, &limits)
                .map(|(artifact, _)| artifact)
        }

        /// Bytes the preview cache holds right now — the number the tests pin.
        pub(crate) fn preview_cache_bytes(&self) -> u64 {
            self.previews
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .bytes
        }

        /// Rows held — the other number the tests pin.
        pub(crate) fn len(&self) -> usize {
            self.index
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .rows
                .len()
        }
    }
    use std::time::Duration;
    use zerocode_core::artifact::Preview;

    fn touch(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    fn origin(worker: &str) -> Origin {
        Origin {
            run: Some("run-1".into()),
            task: Some("t-1".into()),
            worker: Some(worker.into()),
            worktree: Some(PathBuf::from("/wt/a")),
            ..Origin::default()
        }
    }

    /// Two scans of an unchanged folder: the second reads nothing and changes
    /// nothing. A rewritten file is the one row that moves.
    #[test]
    fn completed_workers_keep_the_task_their_report_describes() {
        let dispatch =
            |id: &str, worker: &str, task: &str, at| zerocode_core::orchestration::Dispatch {
                id: id.to_string(),
                worker: worker.to_string(),
                task: task.to_string(),
                started_ms: at,
                ended_ms: Some(at + 1),
                succeeded: Some(true),
                retry_of: None,
                remote: None,
                source: None,
            };
        let history = vec![
            dispatch("d1", "w1", "old", 1),
            dispatch("d2", "w1", "current", 2),
            dispatch("d3", "w2", "someone-else", 3),
        ];
        assert_eq!(worker_task_id(&history, "w1", None), Some("current"));
        assert_eq!(worker_task_id(&history, "w1", Some("d1")), Some("old"));
        assert_eq!(worker_task_id(&history, "unknown", None), None);
    }

    #[test]
    fn reopening_a_filename_catalog_upgrades_titles_from_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = dir.path().join("t-42-report.md");
        touch(&report, "# SFTP home mapping and transfer queue");
        let data = dir.path().join("data");
        let store = Store::open(&data, Limits::default());
        let mut artifact = store
            .register_report(&report, origin("worker"), 1000)
            .expect("report");
        artifact.title = artifact
            .path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        store
            .append_line(&IndexLine::Row(Box::new(artifact)))
            .expect("legacy row");
        drop(store);
        let reopened = Store::open(&data, Limits::default());
        reopened.scan(2000);
        assert_eq!(
            reopened.list(&Filter::default()).rows[0].title,
            "SFTP home mapping and transfer queue"
        );
    }

    #[test]
    fn artifact_window_export_uses_the_requested_snapshot_without_changing_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store"), Limits::default());
        let source = dir.path().join("source.html");
        let out = dir.path().join("share.html");
        touch(&source, "<main>first</main>");
        let request = serde_json::json!({"action":"publish", "file_path":source});
        let first = artifact_request(&store, request.clone()).unwrap();
        touch(&source, "<main>second</main>");
        artifact_request(&store, request).unwrap();
        std::fs::remove_file(source).unwrap();
        let answer = artifact_request(
            &store,
            serde_json::json!({"action":"export", "id":first["id"], "version":1, "out":out}),
        )
        .unwrap();
        assert_eq!(answer["version"], 1);
        let snapshot = store
            .root()
            .join("pages")
            .join(first["id"].as_str().unwrap())
            .join("v1/index.html");
        assert_eq!(
            std::fs::read(out).unwrap(),
            std::fs::read(snapshot).unwrap()
        );
        assert_eq!(store.list(&Filter::default()).total, 1);
        assert_eq!(
            store.get(first["id"].as_str().unwrap()).unwrap().version,
            Some(2)
        );
    }

    #[test]
    fn artifact_publication_is_one_gallery_row_after_reopen_and_delete_cleans_versions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("report.html");
        touch(&source, "<main>original</main>");
        let store = Store::open(dir.path(), Limits::default());
        let input = zerocode_core::artifact_publish::PublishInput {
            file_path: source.clone(),
            title: Some("Orbit Review".into()),
            description: Some("A finished report".into()),
            favicon: Some("🌿".into()),
            ..Default::default()
        };
        let one = store.publish_page(&input).unwrap();
        touch(&source, "<main>revised</main>");
        let two = store.publish_page(&input).unwrap();
        assert_eq!(one.url, two.url);
        assert_eq!(two.version, 2);
        let store = Store::open(dir.path(), Limits::default());
        assert_eq!(store.list(&Filter::default()).total, 1);
        let row = store.get(&one.id).unwrap();
        assert_eq!(row.kind, ArtifactKind::Page);
        assert_eq!(row.title, "Orbit Review");
        assert_eq!(row.description.as_deref(), Some("A finished report"));
        assert_eq!(row.favicon.as_deref(), Some("🌿"));
        assert_eq!(row.version, Some(2));
        assert!(store.delete(&row.id, true).unwrap());
        assert!(!store.root().join("pages").join(&row.id).exists());
        assert!(source.exists());
    }

    /// The version picker of a publication lists its store's own immutable
    /// versions with their digests; the publication door lists publications
    /// only (an agent's project page stays the gallery's); the snapshot road
    /// digests what it kept; and a publication catalogued before rows named
    /// their editable source reads it back at open (t-3952).
    #[test]
    fn a_publication_lists_its_kept_versions_and_the_door_lists_only_publications() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let store = Store::open(&data, Limits::default());
        let source = dir.path().join("deck.html");
        touch(&source, "<main>first</main>");
        let request = serde_json::json!({"action":"publish", "file_path":source});
        let first = artifact_request(&store, request.clone()).unwrap();
        touch(&source, "<main>second</main>");
        let second = artifact_request(&store, request).unwrap();
        let id = first["id"].as_str().unwrap().to_string();
        let versions = store.versions(&id);
        assert_eq!(
            versions.iter().map(|one| one.n).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(versions[0].path.ends_with("v1/index.html"));
        assert_eq!(versions[1].sha256.as_deref(), second["sha256"].as_str());
        assert_ne!(versions[0].sha256, versions[1].sha256);

        let project = dir.path().join("project");
        touch(&project.join("site.html"), "<title>Site</title>");
        store
            .register_page(
                &PageFact {
                    path: project.join("site.html"),
                    at_ms: Some(1),
                    session: Some("s-1".into()),
                    project: Some(project.clone()),
                },
                "claude",
                1,
            )
            .unwrap()
            .expect("the created page is a row");
        let pages = store.list(&Filter {
            kind: Some("page".into()),
            ..Filter::default()
        });
        assert_eq!(pages.total, 2);
        let listed = artifact_request(&store, serde_json::json!({"action":"list"})).unwrap();
        assert_eq!(listed["total"], 1, "{listed}");
        assert_eq!(listed["rows"][0]["id"], id.as_str());
        let page = store.agent_pages().remove(0);
        let snapshots = store.versions(&page.id);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            snapshots[0].sha256,
            file_sha256(&snapshots[0].path, u64::MAX),
            "{snapshots:?}"
        );
        assert!(snapshots[0].sha256.is_some());

        let mut legacy = store.get(&id).unwrap();
        assert_eq!(legacy.source_path, Some(source.canonicalize().unwrap()));
        legacy.source_path = None;
        store
            .append_line(&IndexLine::Row(Box::new(legacy)))
            .unwrap();
        drop(store);
        let reopened = Store::open(&data, Limits::default());
        assert_eq!(
            reopened.get(&id).unwrap().source_path,
            Some(source.canonicalize().unwrap())
        );
    }

    #[test]
    fn a_scan_is_incremental_on_modified_time_and_length() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let evidence = dir.path().join("evidence");
        touch(&evidence.join("steps.jsonl"), "{\"n\":1}\n");
        touch(&evidence.join("shot-1.png"), "not really a png");
        let store = Store::open(&data, Limits::default());
        store.adopt_source(ScanSource {
            dir: evidence.clone(),
            source: Source::Evidence,
            origin: Origin {
                automation: Some("auto-1".into()),
                ..Origin::default()
            },
        });
        let first = store.scan(1_000);
        assert_eq!(
            (first.seen, first.added, first.updated),
            (2, 2, 0),
            "{first:?}"
        );
        let second = store.scan(2_000);
        assert_eq!(
            (second.seen, second.added, second.updated),
            (2, 0, 0),
            "{second:?}"
        );
        assert!(!second.changed());

        std::thread::sleep(Duration::from_millis(20));
        touch(&evidence.join("steps.jsonl"), "{\"n\":1}\n{\"n\":2}\n");
        let third = store.scan(3_000);
        assert_eq!((third.added, third.updated), (0, 1), "{third:?}");
        let rows = store.list(&Filter::default());
        assert_eq!(rows.rows.len(), 2);
        let steps = rows
            .rows
            .iter()
            .find(|row| row.title == "steps.jsonl")
            .expect("the step log is a row");
        assert_eq!(steps.kind, ArtifactKind::Evidence);
        assert_eq!(steps.origin.automation.as_deref(), Some("auto-1"));
        assert!(matches!(&steps.preview, Preview::Text { text } if text.contains("\"n\":2")));

        std::fs::remove_file(evidence.join("shot-1.png")).expect("remove");
        let fourth = store.scan(4_000);
        assert_eq!(fourth.removed, 1, "{fourth:?}");
        assert_eq!(store.len(), 1);
    }

    /// The bounds are the table's, and reaching one is reported rather than
    /// silently walked past.
    #[test]
    fn a_scan_past_the_table_reports_truncation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().join("exports");
        for n in 0..5 {
            touch(&folder.join(format!("row-{n}.csv")), "a,b\n");
        }
        touch(&folder.join("deep/deeper/deepest/leaf.csv"), "a,b\n");
        let limits = Limits {
            scan_files_max: 3,
            ..Limits::default()
        };
        let store = Store::open(&dir.path().join("data"), limits);
        store.adopt_source(ScanSource {
            dir: folder.clone(),
            source: Source::Export,
            origin: Origin::default(),
        });
        let report = store.scan(1_000);
        assert!(report.truncated, "{report:?}");
        assert_eq!(report.added, 3);

        let shallow = Store::open(
            &dir.path().join("data2"),
            Limits {
                scan_depth: 2,
                ..Limits::default()
            },
        );
        shallow.adopt_source(ScanSource {
            dir: folder,
            source: Source::Export,
            origin: Origin::default(),
        });
        let report = shallow.scan(1_000);
        assert!(report.truncated, "depth was not reported: {report:?}");
        assert_eq!(report.added, 5, "the shallow rows still landed");
    }

    /// Registering the same report twice under the same origin is one
    /// artifact and one copy; a changed report is copied again under the
    /// same id; another worker's identical path is another artifact.
    #[test]
    fn a_report_copy_is_idempotent_and_survives_reopening() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let report = dir.path().join("tmp/t-1-report.md");
        touch(&report, "# report\n\nlanded 아티팩트 view\n");
        let store = Store::open(&data, Limits::default());
        let first = store
            .register_report(&report, origin("w-1"), 1_000)
            .expect("registers");
        let again = store
            .register_report(&report, origin("w-1"), 2_000)
            .expect("registers again");
        assert_eq!(first, again, "the same report became two artifacts");
        assert!(
            first
                .path
                .starts_with(data.join(STORE_DIR_NAME).join("run"))
        );
        assert_eq!(first.kind, ArtifactKind::Report);
        assert!(
            matches!(&first.preview, Preview::Markdown { text } if text.starts_with("# report"))
        );
        let lines = std::fs::read_to_string(data.join(STORE_DIR_NAME).join(INDEX_FILE))
            .expect("index")
            .lines()
            .count();
        assert_eq!(lines, 1, "an unchanged report appended a second line");

        std::thread::sleep(Duration::from_millis(20));
        touch(&report, "# report v2\n");
        let changed = store
            .register_report(&report, origin("w-1"), 3_000)
            .expect("re-registers");
        assert_eq!(changed.id, first.id);
        assert_eq!(
            std::fs::read_to_string(&changed.path).expect("copy"),
            "# report v2\n"
        );
        let other = store
            .register_report(&report, origin("w-2"), 3_000)
            .expect("another worker");
        assert_ne!(other.id, first.id);
        assert_eq!(store.counts().by_worker.get("w-1"), Some(&1));
        assert_eq!(store.counts().by_task.get("t-1"), Some(&2));
        assert_eq!(
            store.counts().report_by_worker.get("w-1"),
            Some(&changed.id)
        );
        assert_eq!(store.counts().report_by_worker.get("w-2"), Some(&other.id));

        // Reopened: the rows are still there, and search finds the body.
        let reopened = Store::open(&data, Limits::default());
        assert_eq!(reopened.len(), 2);
        reopened.scan(4_000);
        let found = reopened.list(&Filter {
            query: "v2 report".into(),
            ..Filter::default()
        });
        assert_eq!(found.rows.len(), 2, "{found:?}");
        let none = reopened.list(&Filter {
            query: "아티팩트".into(),
            ..Filter::default()
        });
        assert_eq!(none.rows.len(), 0, "the old body still matched: {none:?}");
        assert_eq!(reopened.ids_for_worker("w-2").len(), 1);
    }

    /// Pruning takes the row AND the store's copy, compacts the catalog, and
    /// leaves a file outside the app's data root alone.
    #[test]
    fn pruning_takes_the_row_and_the_file_together() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let report = dir.path().join("tmp/t-1-report.md");
        touch(&report, "# old\n");
        let export = dir.path().join("exports/rows.csv");
        touch(&export, "a,b\n");
        let store = Store::open(&data, Limits::default());
        let copied = store
            .register_report(&report, origin("w-1"), 1_000)
            .expect("registers");
        let outside = store
            .register_in_place(&export, Source::Export, Origin::default(), 1_000)
            .expect("registers in place");
        let day = 24 * 60 * 60 * 1000;
        let now = copied.modified_ms + 40 * day;
        let pruned = store.prune(now, 30);
        assert_eq!(pruned, Pruned { rows: 2, files: 1 }, "{pruned:?}");
        assert!(!copied.path.exists(), "the store's copy survived the sweep");
        assert!(export.exists(), "a file outside the data root was deleted");
        assert_eq!(store.len(), 0);
        let index =
            std::fs::read_to_string(data.join(STORE_DIR_NAME).join(INDEX_FILE)).expect("index");
        assert_eq!(
            index, "",
            "the catalog kept tombstones after compaction: {index}"
        );
        let _ = outside;

        // The ledger's clock: no sweep until the ledger swept, one per sweep.
        touch(&report, "# new\n");
        store
            .register_report(&report, origin("w-1"), 1_000)
            .expect("registers");
        assert_eq!(store.sweep_beside_ledger(None, 30, now), None);
        assert!(store.sweep_beside_ledger(Some(5), 30, now).is_some());
        assert_eq!(store.sweep_beside_ledger(Some(5), 30, now), None);
    }

    /// Without the person's confirmation nothing moves; with it, the row and
    /// the copy go.
    #[test]
    fn deleting_without_confirmation_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let report = dir.path().join("tmp/t-1-report.md");
        touch(&report, "# report\n");
        let store = Store::open(&data, Limits::default());
        let copied = store
            .register_report(&report, origin("w-1"), 1_000)
            .expect("registers");
        assert_eq!(store.delete(&copied.id, false), Ok(false));
        assert!(copied.path.exists());
        assert_eq!(store.len(), 1);
        assert_eq!(store.delete(&copied.id, true), Ok(true));
        assert!(!copied.path.exists());
        assert_eq!(store.len(), 0);
        assert_eq!(
            store.delete(&copied.id, true),
            Ok(false),
            "a second delete finds nothing"
        );
    }

    /// The preview cache is bounded by bytes, evicts the least recently used
    /// entry first, and forgets a row that was deleted.
    #[test]
    fn the_preview_cache_is_bounded_by_bytes_and_least_recently_used() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let limits = Limits {
            preview_cache_bytes: 40,
            ..Limits::default()
        };
        let store = Store::open(&data, limits);
        let mut ids = Vec::new();
        for n in 0..3 {
            let report = dir.path().join(format!("tmp/t-{n}-report.md"));
            touch(&report, "0123456789012345");
            ids.push(
                store
                    .register_report(&report, origin(&format!("w-{n}")), 1_000)
                    .expect("registers")
                    .id,
            );
        }
        for id in &ids[..2] {
            assert_eq!(store.preview(id).expect("preview").kind, "markdown");
        }
        assert_eq!(store.preview_cache_bytes(), 32);
        // Touch the first so the second is the one to go.
        store.preview(&ids[0]).expect("preview");
        store.preview(&ids[2]).expect("preview");
        assert_eq!(
            store.preview_cache_bytes(),
            32,
            "the cache grew past its cap"
        );
        let held = store.previews.lock().unwrap();
        assert!(
            held.held.contains_key(&ids[0]),
            "the recently used entry was evicted"
        );
        assert!(
            !held.held.contains_key(&ids[1]),
            "the least recently used entry stayed"
        );
        drop(held);
        store.delete(&ids[0], true).expect("deletes");
        assert_eq!(store.preview_cache_bytes(), 16);
    }

    /// Listing filters by kind and by each origin field, newest first, and
    /// says when the table cut the answer.
    #[test]
    fn listing_filters_by_kind_and_origin_and_is_bounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        let limits = Limits {
            list_rows_max: 2,
            ..Limits::default()
        };
        let store = Store::open(&data, limits);
        for n in 0..3 {
            let report = dir.path().join(format!("tmp/t-{n}-report.md"));
            touch(&report, "# r\n");
            store
                .register_report(&report, origin(&format!("w-{n}")), 1_000 + n)
                .expect("registers");
        }
        let shot = dir.path().join("evidence/shot.png");
        touch(&shot, "png?");
        store
            .register_in_place(
                &shot,
                Source::Evidence,
                Origin {
                    automation: Some("auto-1".into()),
                    worktree: Some(PathBuf::from("/wt/b")),
                    ..Origin::default()
                },
                1_000,
            )
            .expect("registers");
        let all = store.list(&Filter::default());
        assert!(all.truncated);
        assert_eq!(all.total, 4);
        assert_eq!(all.rows.len(), 2);
        let shots = store.list(&Filter {
            kind: Some("screenshot".into()),
            ..Filter::default()
        });
        assert_eq!(shots.rows.len(), 1);
        assert_eq!(shots.rows[0].title, "shot.png");
        let worker = store.list(&Filter {
            worker: Some("w-1".into()),
            ..Filter::default()
        });
        assert_eq!(worker.rows.len(), 1);
        let worktree = store.list(&Filter {
            worktree: Some("/wt/b".into()),
            ..Filter::default()
        });
        assert_eq!(worktree.rows.len(), 1);
        let unknown_kind = store.list(&Filter {
            kind: Some("picture".into()),
            ..Filter::default()
        });
        assert_eq!(
            unknown_kind.total, 4,
            "an unknown kind word filtered rather than being ignored"
        );
        assert_eq!(store.counts().by_automation.get("auto-1"), Some(&1));
        assert_eq!(store.counts().by_worktree.get("/wt/a"), Some(&3));

        let forgotten = store.forget_under(&[dir.path().join("evidence")]);
        assert_eq!(forgotten, 1);
        assert_eq!(store.len(), 3);
    }

    /// A claude.ai artifact is one row per url (t-3233 §2a): the same url
    /// twice is one row, a newer fact refreshes the title and time, an older
    /// one is ignored, and the row lists as `web` with its url and no file.
    #[test]
    fn a_remote_row_is_one_per_url_and_takes_the_newer_title() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let fact = |title: &str, at_ms: i64| RemoteFact {
            url: "https://claude.ai/code/artifact/abc".into(),
            title: Some(title.into()),
            description: Some("한 장짜리 결정 문서 landed".into()),
            favicon: Some("🧱".into()),
            source_path: Some(PathBuf::from("/tmp/x.html")),
            at_ms: Some(at_ms),
            session: Some("s-1".into()),
            project: Some(PathBuf::from("/p")),
        };
        let first = store
            .register_remote(&fact("첫 제목", 1_000), "claude", 5_000)
            .expect("registers");
        assert_eq!(first.kind, ArtifactKind::Web);
        assert_eq!(
            first.url.as_deref(),
            Some("https://claude.ai/code/artifact/abc")
        );
        assert_eq!(first.path, PathBuf::new());
        assert_eq!(first.modified_ms, 1_000);
        assert_eq!(first.favicon.as_deref(), Some("🧱"));
        assert_eq!(first.source, Source::Remote);
        let again = store
            .register_remote(&fact("첫 제목", 1_000), "claude", 6_000)
            .expect("registers again");
        assert_eq!(again, first, "the same fact made a second row");
        assert_eq!(store.len(), 1);
        let older = store
            .register_remote(&fact("옛 제목", 500), "claude", 6_000)
            .expect("registers");
        assert_eq!(older.title, "첫 제목", "an older fact overwrote the row");
        let newer = store
            .register_remote(&fact("새 제목", 2_000), "claude", 6_000)
            .expect("registers");
        assert_eq!(newer.title, "새 제목");
        assert_eq!(newer.id, first.id);
        assert_eq!(
            newer.created_ms, 1_000,
            "the first sighting stays the birth"
        );
        assert_eq!(store.len(), 1);
        let lines = std::fs::read_to_string(
            dir.path()
                .join("data")
                .join(STORE_DIR_NAME)
                .join(INDEX_FILE),
        )
        .expect("index")
        .lines()
        .count();
        assert_eq!(lines, 2, "one line per change, none for the repeats");
        // Search reaches the title, the description and the url.
        for query in ["새 제목", "결정 문서", "artifact"] {
            let found = store.list(&Filter {
                query: query.into(),
                ..Filter::default()
            });
            assert_eq!(found.rows.len(), 1, "{query}: {found:?}");
        }
        assert_eq!(store.preview(&first.id).expect("preview").kind, "text");
        assert!(
            store
                .register_remote(
                    &RemoteFact {
                        url: "javascript:alert(1)".into(),
                        ..fact("x", 1)
                    },
                    "claude",
                    1
                )
                .is_err(),
            "only an https url is an artifact"
        );
        // Reopened, the url is still there.
        let reopened = Store::open(&dir.path().join("data"), Limits::default());
        assert_eq!(reopened.get(&first.id).and_then(|row| row.url), first.url);
    }

    /// A page is pointed at, never copied (t-3233 §2b): the same path twice
    /// is one row, a session past the table's count loses its oldest page,
    /// a file past the byte cap or not there is `None`, and the scan drops
    /// the row when the file goes.
    #[test]
    fn a_page_is_registered_in_place_bounded_per_session_and_gone_with_its_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        let store = Store::open(
            &dir.path().join("data"),
            Limits {
                pages_per_session: 2,
                page_bytes_max: 64,
                ..Limits::default()
            },
        );
        let fact = |name: &str| PageFact {
            path: project.join(name),
            at_ms: Some(1_000),
            session: Some("s-1".into()),
            project: Some(project.clone()),
        };
        touch(
            &project.join("index.html"),
            "<!doctype html><title>a</title>",
        );
        let page = store
            .register_page(&fact("index.html"), "claude", 1_000)
            .expect("registers")
            .expect("a row");
        assert_eq!(page.kind, ArtifactKind::Page);
        assert_eq!(page.source, Source::AgentPage);
        assert_eq!(
            page.path,
            project.join("index.html").canonicalize().unwrap()
        );
        assert_eq!(page.origin.agent.as_deref(), Some("claude"));
        assert_eq!(page.origin.session.as_deref(), Some("s-1"));
        assert_eq!(page.origin.project, Some(project.canonicalize().unwrap()));
        assert!(
            !page.path.starts_with(dir.path().join("data")),
            "the page was copied into the store"
        );
        let again = store
            .register_page(&fact("index.html"), "claude", 2_000)
            .expect("registers")
            .expect("a row");
        assert_eq!(again.id, page.id);
        assert_eq!(store.len(), 1);
        assert_eq!(
            store.register_page(&fact("missing.html"), "claude", 1_000),
            Ok(None),
            "a file that is not there is not an error"
        );
        touch(&project.join("big.html"), &"x".repeat(100));
        assert_eq!(
            store.register_page(&fact("big.html"), "claude", 1_000),
            Ok(None)
        );
        touch(&project.join("main.rs"), "fn main() {}");
        assert_eq!(
            store.register_page(&fact("main.rs"), "claude", 1_000),
            Ok(None)
        );

        touch(
            &project.join("notes.md"),
            "# notes
",
        );
        touch(&project.join("paper.pdf"), "%PDF-1.4");
        let notes = store
            .register_page(&fact("notes.md"), "claude", 1_100)
            .expect("registers")
            .expect("a row");
        assert_eq!(notes.kind, ArtifactKind::Document);
        assert!(
            matches!(&notes.preview, Preview::Markdown { text } if text.starts_with("# notes"))
        );
        assert_eq!(store.preview(&notes.id).expect("preview").kind, "markdown");
        let pdf = store
            .register_page(&fact("paper.pdf"), "claude", 1_200)
            .expect("registers")
            .expect("a row");
        assert_eq!(pdf.kind, ArtifactKind::Document);
        assert_eq!(store.preview(&pdf.id).expect("preview").kind, "none");
        assert_eq!(store.len(), 2, "the session's oldest page gave its seat");
        assert!(
            store.get(&page.id).is_none(),
            "the oldest page kept its row"
        );

        std::fs::remove_file(project.join("notes.md")).expect("remove");
        let report = store.scan(3_000);
        assert_eq!(report.removed, 1, "{report:?}");
        assert_eq!(store.len(), 1);
        assert!(
            store.watch_targets().iter().any(|(_, path)| path.as_deref()
                == Some(project.join("paper.pdf").canonicalize().unwrap().as_path())),
            "the page's file does not ride the watcher's lane"
        );
    }

    #[cfg(unix)]
    #[test]
    fn page_registration_resolves_aliases_and_rejects_symlink_traversal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        touch(&project.join("docs/page.html"), "inside");
        touch(&dir.path().join("outside/page.html"), "outside");
        std::os::unix::fs::symlink(project.join("docs"), project.join("alias")).expect("alias");
        std::os::unix::fs::symlink(dir.path().join("outside"), project.join("escape"))
            .expect("escape");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let fact = |path: &str| PageFact {
            path: project.join(path),
            at_ms: None,
            session: None,
            project: Some(project.clone()),
        };
        let original = store
            .register_page(&fact("docs/page.html"), "zo", 1)
            .unwrap()
            .unwrap();
        let alias = store
            .register_page(&fact("alias/page.html"), "zo", 2)
            .unwrap()
            .unwrap();
        assert_eq!(alias.id, original.id, "one physical page is one row");
        assert_eq!(
            store.register_page(&fact("escape/page.html"), "zo", 3),
            Ok(None)
        );
        // Lexically this is project/outside/page.html, but resolving the
        // symlink before .. reaches the sibling outside/page.html.
        assert_eq!(
            store.register_page(&fact("escape/../outside/page.html"), "zo", 4),
            Ok(None)
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn rescanning_pages_obeys_the_file_byte_and_page_limits() {
        for limits in [
            Limits {
                page_bytes_max: 4,
                ..Limits::default()
            },
            Limits {
                scan_bytes_max: 4,
                ..Limits::default()
            },
            Limits {
                scan_files_max: 1,
                ..Limits::default()
            },
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let project = dir.path().join("project");
            let store = Store::open(&dir.path().join("data"), limits);
            let mut ids = Vec::new();
            for name in ["a.html", "b.md"] {
                let path = project.join(name);
                touch(&path, "v1");
                let row = store
                    .register_page(
                        &PageFact {
                            path: path.clone(),
                            project: Some(project.clone()),
                            at_ms: None,
                            session: None,
                        },
                        "zo",
                        1,
                    )
                    .unwrap()
                    .unwrap();
                ids.push(row.id);
                touch(&path, "v2222");
            }
            let report = store.scan(2);
            assert!(report.truncated, "{limits:?}: {report:?}");
            let new_versions: usize = ids.iter().map(|id| store.versions(id).len() - 1).sum();
            assert_eq!(
                new_versions,
                usize::from(limits.scan_files_max == 1),
                "{limits:?}: {report:?}"
            );
            if limits.scan_files_max == 1 {
                store.scan(3);
                assert!(
                    ids.iter().all(|id| store.versions(id).len() == 2),
                    "a bounded scan must reach the deferred page"
                );
            }
        }
    }

    /// Every change to a page keeps a version (t-3233 §5): V1 at registration,
    /// V2 when the scan sees the stamp move, bounded by the table, and the
    /// versions go with the row.
    #[test]
    fn a_page_keeps_its_versions_bounded_and_loses_them_with_the_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        let store = Store::open(
            &dir.path().join("data"),
            Limits {
                versions_per_artifact_max: 2,
                ..Limits::default()
            },
        );
        let fact = PageFact {
            path: project.join("index.html"),
            at_ms: None,
            session: Some("s-1".into()),
            project: Some(project.clone()),
        };
        touch(&project.join("index.html"), "v1");
        let page = store
            .register_page(&fact, "zo", 1_000)
            .expect("registers")
            .expect("a row");
        let versions = store.versions(&page.id);
        assert_eq!(versions.len(), 1, "{versions:?}");
        assert_eq!(versions[0].n, 1);
        assert_eq!(
            std::fs::read_to_string(&versions[0].path).expect("v1"),
            "v1"
        );
        assert!(
            versions[0]
                .path
                .starts_with(store.root().join(VERSIONS_DIR_NAME))
        );
        std::thread::sleep(Duration::from_millis(20));
        touch(&project.join("index.html"), "v2 longer");
        let report = store.scan(2_000);
        assert_eq!(report.updated, 1, "{report:?}");
        let versions = store.versions(&page.id);
        assert_eq!(versions.iter().map(|v| v.n).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(
            std::fs::read_to_string(&versions[1].path).expect("v2"),
            "v2 longer"
        );
        std::thread::sleep(Duration::from_millis(20));
        touch(&project.join("index.html"), "v3");
        store.scan(3_000);
        let versions = store.versions(&page.id);
        assert_eq!(
            versions.iter().map(|v| v.n).collect::<Vec<_>>(),
            vec![2, 3],
            "the oldest went first"
        );
        assert_eq!(
            store.scan(4_000).updated,
            0,
            "an unchanged page was re-versioned"
        );
        assert_eq!(store.versions(&page.id).len(), 2);
        assert!(store.delete(&page.id, true).expect("deletes"));
        assert!(store.versions(&page.id).is_empty());
        assert!(!store.root().join(VERSIONS_DIR_NAME).join(&page.id).exists());
        assert!(
            project.join("index.html").exists(),
            "the project's file was deleted"
        );
        assert!(store.versions("nobody").is_empty());
    }

    #[test]
    fn retention_removes_old_snapshots_of_a_still_active_page() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("project");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let fact = PageFact {
            path: project.join("index.html"),
            at_ms: None,
            session: Some("s-1".into()),
            project: Some(project),
        };
        touch(&fact.path, "v1");
        let page = store
            .register_page(&fact, "zo", 1)
            .expect("register")
            .expect("row");
        let first = store.versions(&page.id).remove(0);
        std::fs::File::options()
            .write(true)
            .open(&first.path)
            .expect("snapshot")
            .set_times(std::fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
            .expect("age snapshot");
        touch(&fact.path, "v2 longer");
        store.scan(2);
        store.prune(epoch_ms(Some(SystemTime::now())), 1);
        assert!(store.get(&page.id).is_some());
        assert!(!first.path.exists());
        assert_eq!(
            store
                .versions(&page.id)
                .iter()
                .map(|v| v.n)
                .collect::<Vec<_>>(),
            vec![2]
        );
        touch(&fact.path, "v3");
        store.scan(3);
        assert_eq!(
            store
                .versions(&page.id)
                .iter()
                .map(|v| v.n)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn page_removal_cleans_snapshots_and_thumbnails_on_every_catalog_exit() {
        for reason in ["missing", "session_cap", "index_cap"] {
            let dir = tempfile::tempdir().expect("tempdir");
            let project = dir.path().join("project");
            let store = Store::open(
                &dir.path().join("data"),
                Limits {
                    pages_per_session: if reason == "session_cap" { 1 } else { 10 },
                    index_rows_max: if reason == "index_cap" { 1 } else { 10 },
                    ..Limits::default()
                },
            );
            let mut fact = PageFact {
                path: project.join("first.html"),
                at_ms: None,
                session: Some("s-1".into()),
                project: Some(project.clone()),
            };
            touch(&fact.path, "first");
            let page = store
                .register_page(&fact, "zo", 1)
                .expect("register")
                .expect("row");
            let snapshot = store.versions(&page.id).remove(0).path;
            let thumbnail = store
                .root()
                .join(THUMBS_DIR_NAME)
                .join(format!("{}.png", page.id));
            touch(&thumbnail, "picture");
            if reason == "missing" {
                std::fs::remove_file(&fact.path).expect("remove source");
                store.scan(2);
            } else {
                fact.path = project.join("second.html");
                touch(&fact.path, "second");
                store.register_page(&fact, "zo", 2).expect("register");
            }
            assert!(store.get(&page.id).is_none(), "{reason}");
            assert!(!snapshot.exists(), "{reason}: orphan snapshot");
            assert!(!thumbnail.exists(), "{reason}: orphan thumbnail");
        }
    }

    /// The report path a `worker_done` names, from the payload or the body.
    #[test]
    fn a_worker_done_names_its_report_in_the_payload_or_the_body() {
        assert_eq!(
            report_path_in(
                Some(r#"{"reportPath":"/tmp/t-9-report.md","lifetime":"ephemeral"}"#),
                "done"
            ),
            Some(PathBuf::from("/tmp/t-9-report.md"))
        );
        assert_eq!(
            report_path_in(None, "landed; report at `/tmp/t-9-report.md` (ephemeral)"),
            Some(PathBuf::from("/tmp/t-9-report.md"))
        );
        assert_eq!(report_path_in(None, "done, nothing to read"), None);
        assert_eq!(
            report_path_in(Some(r#"{"reportPath":"relative.md"}"#), "done"),
            None,
            "a relative path is not a report"
        );
    }
}
