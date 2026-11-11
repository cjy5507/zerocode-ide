//! One folder's index on disk, kept in step with the folder.
//!
//! Every search walks the folder with `stat` alone and reads only the files
//! whose stamp moved (`FileStamp`: size, times and, where the host tells,
//! which file it is). What it finds is written as two index files:
//!
//! - the **base**, built from every file once;
//! - the **overlay**, built from the files that changed since, with the list
//!   of base rows they replace. It is small, so writing it again on every
//!   change costs little. Past [`Table::rebuild_share`] of the base the
//!   folder is built whole again and the overlay goes.
//!
//! A manifest names the files that stand together. Each is written under a
//! new name and never changed, and the manifest is replaced in one rename, so
//! a reader sees one whole generation or the next. One writer works at a time
//! (a lock file); a reader that finds its generation swept away reads the
//! manifest again.
//!
//! Nothing here is the only copy of anything. A manifest that cannot be read,
//! a file cut short or a folder that cannot be written all end the same way:
//! the files are read whole and the answer comes from memory.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::passages;
use super::rank::{Corpus, Edge, Held};
use super::segment::{self, Damaged, DocInput, DocRecord, Place, Segment, WordHash};
use super::settings::{Kind, Source, Table};
use crate::atomic_file::write_atomically;
use crate::file_stamp::{FileStamp, stamp_of};
use crate::held_file_lock::HeldFileLock;
use crate::second_brain::WIKI_DIR;
use crate::second_brain_graph::{
    FoundPage, LinkLookup, MAX_PAGE_LINKS, WalkBounds, enters_vault_folder, parse_page, path_below,
    walk_files,
};
use crate::second_brain_related::{is_structural_id, name_text, tokenize};

pub(super) const MANIFEST_FILE: &str = "manifest.json";
pub(super) const LOCK_FILE: &str = "lock";
/// How often a reader reads the manifest again when the files it names are
/// gone: a writer swept them between the two reads.
const READ_TRIES: usize = 3;
/// A relation row in the relations file: two file numbers and a kind.
const EDGE_BYTES: usize = 9;
const EDGE_SUM_BYTES: usize = 8;

/// How a layer's index came to be what this search read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Nothing had changed.
    Fresh,
    /// The changed files were read and laid over the base.
    Updated,
    /// Every file was read and the index written whole.
    Built,
    /// Nothing could be written or read back: every file was read, and the
    /// answer came from memory.
    Scanned,
}

impl Freshness {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Updated => "updated",
            Self::Built => "built",
            Self::Scanned => "scanned",
        }
    }
}

/// An index file on disk, or one that never left memory.
#[derive(Debug)]
pub(super) enum Store {
    Disk(File),
    Memory(Cursor<Vec<u8>>),
}

impl Read for Store {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Disk(file) => file.read(into),
            Self::Memory(bytes) => bytes.read(into),
        }
    }
}

impl Seek for Store {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Disk(file) => file.seek(to),
            Self::Memory(bytes) => bytes.seek(to),
        }
    }
}

/// One file the walk found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Walked {
    /// Below the source's folder, `/`-separated.
    pub rel: String,
    pub path: PathBuf,
    pub stamp: FileStamp,
}

/// The files of a source that the table lets in, by path, and whether
/// [`Table::max_files`] ended the walk before the folder did.
///
/// The walk is the knowledge graph's (`walk_files`). The vault's own folders
/// are entered as the graph enters them, however deep, so the search and the
/// graph hold the same pages. Only a folder a person added keeps the table's
/// own bounds on folders: code a build wrote lies there, not in a vault.
pub(super) fn walk(source: &Source, table: &Table) -> (Vec<Walked>, bool) {
    let own = source.kind != Kind::Added;
    let bounds = WalkBounds {
        entries: usize::MAX,
        files: table.max_files,
        depth: if own { usize::MAX } else { table.max_depth },
    };
    let enters = |name: &str| {
        if own {
            enters_vault_folder(name)
        } else {
            !name.starts_with('.') && !table.skipped_folders.iter().any(|held| held == name)
        }
    };
    let mut files = Vec::new();
    let capped = walk_files(&source.root, bounds, enters, |folder, name, entry| {
        if !table.reads(name) {
            return false;
        }
        let rel = path_below(folder, name);
        if source.kind == Kind::Wiki && is_structural_id(&format!("{WIKI_DIR}/{rel}")) {
            return false;
        }
        let Ok(metadata) = entry.metadata() else {
            return false;
        };
        if metadata.len() > table.max_file_bytes {
            return false;
        }
        files.push(Walked {
            rel,
            path: entry.path(),
            stamp: stamp_of(&metadata),
        });
        true
    });
    files.sort_by(|left, right| left.rel.cmp(&right.rel));
    (files, capped)
}

/// The files that stand together as one generation of a layer's index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Manifest {
    format: u32,
    /// What the index was built under ([`Table::fingerprint`]): another
    /// table or another folder is another index.
    settings: String,
    base: String,
    overlay: Option<String>,
    relations: Option<String>,
}

/// A layer's index, open: what a search ranks over.
#[derive(Debug)]
pub(super) struct Layer {
    base: Segment<Store>,
    overlay: Option<Segment<Store>>,
    /// Base rows, then overlay rows.
    records: Vec<DocRecord>,
    standing: Vec<bool>,
    edges: Vec<Edge>,
    files: u32,
    total_length: u64,
    pub freshness: Freshness,
    /// Files read to bring the index in step.
    pub read: usize,
    pub capped: bool,
}

impl Layer {
    /// The rows of the two index files as one list, with the base rows the
    /// overlay replaced marked as gone.
    fn of(
        mut base: Segment<Store>,
        mut overlay: Option<Segment<Store>>,
        edges: Vec<Edge>,
    ) -> Result<Self, Damaged> {
        let mut records = base.take_records();
        let mut standing = vec![true; records.len()];
        let mut total_length = base.total_length();
        if let Some(overlay) = &mut overlay {
            for row in overlay.replaced() {
                let at = *row as usize;
                // A row replaced twice, or one the base never had, is not a
                // list this overlay's writer wrote.
                if !standing.get(at).copied().unwrap_or(false) {
                    return Err(Damaged);
                }
                standing[at] = false;
                total_length = total_length.saturating_sub(u64::from(base.docs()[at].length));
            }
            records.extend(overlay.take_records());
            standing.resize(records.len(), true);
            total_length += overlay.total_length();
        }
        let rows = u32::try_from(records.len()).map_err(|_| Damaged)?;
        if edges
            .iter()
            .any(|edge| edge.from >= rows || edge.to >= rows)
        {
            return Err(Damaged);
        }
        let files =
            u32::try_from(standing.iter().filter(|held| **held).count()).map_err(|_| Damaged)?;
        Ok(Self {
            base,
            overlay,
            records,
            standing,
            edges,
            files,
            total_length,
            freshness: Freshness::Fresh,
            read: 0,
            capped: false,
        })
    }

    fn base_rows(&self) -> u32 {
        // `of` refused a layer whose rows do not fit.
        self.base.docs().len() as u32
    }

    /// The index file a row lies in, and its number there.
    fn part(&self, doc: u32) -> (&Segment<Store>, usize, u32) {
        let base_rows = self.base_rows();
        match &self.overlay {
            Some(overlay) if doc >= base_rows => (
                overlay,
                (doc - base_rows) as usize,
                self.base.passages().len() as u32,
            ),
            _ => (&self.base, doc as usize, 0),
        }
    }

    pub(super) fn record(&self, doc: u32) -> &DocRecord {
        &self.records[doc as usize]
    }

    /// How many passages its standing files hold, their names apart.
    pub(super) fn standing_passages(&self) -> usize {
        (0..self.docs())
            .filter(|doc| self.standing[*doc as usize])
            .map(|doc| self.body_of(doc).len())
            .sum()
    }

    /// Where a passage lies in its file.
    pub(super) fn place(&self, passage: u32) -> Place {
        let base_passages = self.base.passages().len() as u32;
        match &self.overlay {
            Some(overlay) if passage >= base_passages => {
                overlay.passages()[(passage - base_passages) as usize]
            }
            _ => self.base.passages()[passage as usize],
        }
    }
}

impl Corpus for Layer {
    fn docs(&self) -> u32 {
        self.records.len() as u32
    }

    fn passages(&self) -> u32 {
        let overlay = self
            .overlay
            .as_ref()
            .map_or(0, |held| held.passages().len());
        (self.base.passages().len() + overlay) as u32
    }

    fn standing(&self) -> u32 {
        self.files
    }

    fn total_length(&self) -> u64 {
        self.total_length
    }

    fn length(&self, doc: u32) -> u32 {
        let (part, row, _) = self.part(doc);
        part.docs()[row].length
    }

    fn body_of(&self, doc: u32) -> Range<u32> {
        let (part, row, first) = self.part(doc);
        let body = part.body_of(row);
        first + body.start..first + body.end
    }

    fn path(&self, doc: u32) -> &str {
        &self.records[doc as usize].path
    }

    fn edges(&self) -> &[Edge] {
        &self.edges
    }

    fn holders(&mut self, word: &str) -> Result<Vec<Held>, Damaged> {
        let mut held = Vec::new();
        let standing = &self.standing;
        gather(&mut self.base, word, 0, 0, |row| standing[row], &mut held)?;
        if let Some(overlay) = &mut self.overlay {
            let first_row = self.base.docs().len();
            let first_passage = self.base.passages().len() as u32;
            gather(overlay, word, first_row, first_passage, |_| true, &mut held)?;
        }
        Ok(held)
    }
}

/// One index file's passages for a word, as the layer numbers them.
fn gather(
    part: &mut Segment<Store>,
    word: &str,
    first_row: usize,
    first_passage: u32,
    stands: impl Fn(usize) -> bool,
    into: &mut Vec<Held>,
) -> Result<(), Damaged> {
    let passages = part.holders(word)?;
    let rows = part.docs();
    let mut row = 0;
    // Passages rise, and so do the rows that own them: one walk over both.
    for passage in passages {
        while rows
            .get(row + 1)
            .is_some_and(|next| next.first_passage <= passage)
        {
            row += 1;
        }
        if stands(row) {
            into.push(Held {
                doc: (first_row + row) as u32,
                passage: first_passage + passage,
                name: rows[row].first_passage == passage,
            });
        }
    }
    Ok(())
}

/// What a layer's index needs to be in step with its folder.
enum Plan<'a> {
    Fresh,
    /// Lay these files over the base, in place of these base rows.
    Lay {
        changed: Vec<&'a Walked>,
        replaced: Vec<u32>,
    },
    Build,
}

fn plan<'a>(layer: &Layer, walked: &'a [Walked], table: &Table) -> Plan<'a> {
    let base_rows = layer.base.docs().len();
    let (in_base, in_overlay) = layer.records.split_at(base_rows);
    let on_disk: HashMap<&str, &Walked> = walked
        .iter()
        .map(|file| (file.rel.as_str(), file))
        .collect();
    let replaced: Vec<u32> = (0_u32..)
        .zip(in_base)
        .filter(|(_, record)| {
            on_disk
                .get(record.path.as_str())
                .is_none_or(|file| file.stamp != record.stamp)
        })
        .map(|(row, _)| row)
        .collect();
    let based: HashMap<&str, FileStamp> = in_base
        .iter()
        .map(|record| (record.path.as_str(), record.stamp))
        .collect();
    let changed: Vec<&Walked> = walked
        .iter()
        .filter(|file| based.get(file.rel.as_str()) != Some(&file.stamp))
        .collect();
    let said: &[u32] = match &layer.overlay {
        Some(held) => held.replaced(),
        None => &[],
    };
    let says_so = said == replaced
        && in_overlay.len() == changed.len()
        && in_overlay
            .iter()
            .zip(&changed)
            .all(|(record, file)| record.path == file.rel && record.stamp == file.stamp);
    if says_so {
        return Plan::Fresh;
    }
    let moved = replaced.len() + changed.len();
    if moved
        > table
            .rebuild_floor
            .max(base_rows / table.rebuild_share.max(1))
    {
        return Plan::Build;
    }
    Plan::Lay { changed, replaced }
}

/// The layer's index, in step with its folder: read as it stands when
/// nothing moved, brought up to date when something did, and read whole into
/// memory when the folder's index cannot be written.
pub(super) fn open(
    dir: &Path,
    source: &Source,
    table: &Table,
    hash: WordHash,
) -> Result<Layer, Damaged> {
    let (walked, capped) = walk(source, table);
    let fingerprint = table.fingerprint();
    let mut layer = match in_step(dir, source, table, &walked, &fingerprint, hash) {
        Some(layer) => layer,
        None => scanned(source, table, &walked, hash)?,
    };
    layer.capped = capped;
    Ok(layer)
}

fn in_step(
    dir: &Path,
    source: &Source,
    table: &Table,
    walked: &[Walked],
    fingerprint: &str,
    hash: WordHash,
) -> Option<Layer> {
    if let Some((_, layer)) = read_layer(dir, fingerprint, hash)
        && matches!(plan(&layer, walked, table), Plan::Fresh)
    {
        return Some(layer);
    }
    fs::create_dir_all(dir).ok()?;
    let lock = lock(dir, table)?;
    // Read again under the lock: the writer before this one may have done it.
    let standing = read_layer(dir, fingerprint, hash);
    let whole: Vec<&Walked> = walked.iter().collect();
    let step = match &standing {
        Some((manifest, layer)) => match plan(layer, walked, table) {
            Plan::Fresh => None,
            Plan::Lay { changed, replaced } => {
                lay(
                    dir, source, table, manifest, layer, &changed, &replaced, hash,
                )
                .ok()?;
                Some((Freshness::Updated, changed.len()))
            }
            Plan::Build => {
                build(dir, source, table, &whole, fingerprint, hash).ok()?;
                Some((Freshness::Built, whole.len()))
            }
        },
        None => {
            build(dir, source, table, &whole, fingerprint, hash).ok()?;
            Some((Freshness::Built, whole.len()))
        }
    };
    let Some((freshness, read)) = step else {
        return standing.map(|(_, layer)| layer);
    };
    drop(standing);
    drop(lock);
    let (_, mut layer) = read_layer(dir, fingerprint, hash)?;
    layer.freshness = freshness;
    layer.read = read;
    Some(layer)
}

/// Throws the standing generation away: the next search builds another. For
/// an index that opened and then read wrong.
pub(super) fn discard(dir: &Path) {
    let _ = fs::remove_file(dir.join(MANIFEST_FILE));
}

/// Removes a layer's index for good: its folder left the settings. The
/// index is a copy of nothing, so this only frees its bytes — but the
/// folder's one writer is waited for first ([`Table::lock_patience`] at
/// most) and the lock held while the files go, so they do not go from under
/// a search that is writing them. A search that outlasts the wait, or was
/// waiting for the lock itself, finds no folder to write to and answers from
/// memory, as it does without an index.
pub(super) fn remove(dir: &Path, table: &Table) {
    if !dir.is_dir() {
        return;
    }
    let writer = lock(dir, table);
    if fs::remove_dir_all(dir).is_err() {
        // A host that will not remove a file held open: let go of the lock
        // file and remove what is left.
        drop(writer);
        let _ = fs::remove_dir_all(dir);
    }
}

/// Every file read, and the index kept in memory.
fn scanned(
    source: &Source,
    table: &Table,
    walked: &[Walked],
    hash: WordHash,
) -> Result<Layer, Damaged> {
    let whole: Vec<&Walked> = walked.iter().collect();
    let docs = read_docs(source, table, &whole);
    let records: Vec<DocRecord> = docs.iter().map(|doc| doc.record.clone()).collect();
    let edges = edges_of(source, &records, &vec![true; records.len()]);
    let bytes = segment::write(&docs, &[], hash);
    let base = Segment::open(Store::Memory(Cursor::new(bytes)), hash)?;
    let mut layer = Layer::of(base, None, edges)?;
    layer.freshness = Freshness::Scanned;
    layer.read = whole.len();
    Ok(layer)
}

/// The generation the manifest names, when every file of it opens and agrees
/// with itself.
fn read_layer(dir: &Path, fingerprint: &str, hash: WordHash) -> Option<(Manifest, Layer)> {
    for _ in 0..READ_TRIES {
        let manifest: Manifest =
            serde_json::from_slice(&fs::read(dir.join(MANIFEST_FILE)).ok()?).ok()?;
        if manifest.format != segment::FORMAT || manifest.settings != fingerprint {
            return None;
        }
        if let Some(layer) = read_generation(dir, &manifest, hash) {
            return Some((manifest, layer));
        }
    }
    None
}

fn read_generation(dir: &Path, manifest: &Manifest, hash: WordHash) -> Option<Layer> {
    // A manifest names files beside itself and nothing else.
    let beside =
        |name: &String| (Path::new(name).file_name()?.to_str()? == name).then(|| dir.join(name));
    let open =
        |name: &String| Segment::open(Store::Disk(File::open(beside(name)?).ok()?), hash).ok();
    let base = open(&manifest.base)?;
    let overlay = match &manifest.overlay {
        Some(name) => Some(open(name)?),
        None => None,
    };
    let edges = match &manifest.relations {
        Some(name) => read_edges(&fs::read(beside(name)?).ok()?)?,
        None => Vec::new(),
    };
    Layer::of(base, overlay, edges).ok()
}

/// Waits for the folder's one writer, [`Table::lock_patience`] at most.
fn lock(dir: &Path, table: &Table) -> Option<HeldFileLock> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_FILE))
        .ok()?;
    let began = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Some(HeldFileLock::locked(file)),
            Err(fs::TryLockError::WouldBlock) if began.elapsed() < table.lock_patience => {
                std::thread::sleep(table.lock_poll);
            }
            Err(_) => return None,
        }
    }
}

/// Writes the whole folder as a new base.
fn build(
    dir: &Path,
    source: &Source,
    table: &Table,
    files: &[&Walked],
    fingerprint: &str,
    hash: WordHash,
) -> io::Result<()> {
    let docs = read_docs(source, table, files);
    let records: Vec<DocRecord> = docs.iter().map(|doc| doc.record.clone()).collect();
    let edges = edges_of(source, &records, &vec![true; records.len()]);
    let base = fresh_name("base");
    write_atomically(&dir.join(&base), &segment::write(&docs, &[], hash))?;
    publish(
        dir,
        Manifest {
            format: segment::FORMAT,
            settings: fingerprint.to_string(),
            base,
            overlay: None,
            relations: None,
        },
        &edges,
    )
}

/// Writes the changed files as a new overlay on the standing base.
#[allow(clippy::too_many_arguments)]
fn lay(
    dir: &Path,
    source: &Source,
    table: &Table,
    manifest: &Manifest,
    layer: &Layer,
    changed: &[&Walked],
    replaced: &[u32],
    hash: WordHash,
) -> io::Result<()> {
    let docs = read_docs(source, table, changed);
    let base_rows = layer.base.docs().len();
    let mut records = layer.records[..base_rows].to_vec();
    let mut standing = vec![true; base_rows];
    for row in replaced {
        standing[*row as usize] = false;
    }
    records.extend(docs.iter().map(|doc| doc.record.clone()));
    standing.resize(records.len(), true);
    let edges = edges_of(source, &records, &standing);
    // Nothing differs from the base any more: the overlay goes.
    let overlay = if changed.is_empty() && replaced.is_empty() {
        None
    } else {
        let name = fresh_name("overlay");
        write_atomically(&dir.join(&name), &segment::write(&docs, replaced, hash))?;
        Some(name)
    };
    publish(
        dir,
        Manifest {
            overlay,
            relations: None,
            ..manifest.clone()
        },
        &edges,
    )
}

/// Makes a generation the standing one: its relations, then the manifest in
/// one rename, then every file no manifest names any more.
fn publish(dir: &Path, mut manifest: Manifest, edges: &[Edge]) -> io::Result<()> {
    if !edges.is_empty() {
        let name = fresh_name("relations");
        write_atomically(&dir.join(&name), &write_edges(edges))?;
        manifest.relations = Some(name);
    }
    let bytes = serde_json::to_vec(&manifest).map_err(io::Error::other)?;
    write_atomically(&dir.join(MANIFEST_FILE), &bytes)?;
    let kept: HashSet<&str> = [MANIFEST_FILE, LOCK_FILE, manifest.base.as_str()]
        .into_iter()
        .chain(manifest.overlay.as_deref())
        .chain(manifest.relations.as_deref())
        .collect();
    for entry in fs::read_dir(dir)?.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_none_or(|name| !kept.contains(name))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn fresh_name(what: &str) -> String {
    format!("{what}-{}", uuid::Uuid::new_v4().simple())
}

/// The files as they go into an index, in the order given. Reading and
/// cutting a file into words is most of what a build costs, and no file
/// needs another, so the list is shared out over [`Table::build_threads`].
fn read_docs(source: &Source, table: &Table, files: &[&Walked]) -> Vec<DocInput> {
    let threads = table.build_threads.clamp(1, files.len().max(1));
    let share = files.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = files
            .chunks(share)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|file| read_doc(source, table, file))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap_or_default())
            .collect()
    })
}

/// One file's record, the words of its name and the words of each passage.
/// A file that cannot be read stands as an empty one under the stamp the walk
/// saw, so the index does not ask for it again until it moves.
pub(super) fn read_doc(source: &Source, table: &Table, file: &Walked) -> DocInput {
    let stem = Path::new(&file.rel)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&file.rel);
    let mut doc = DocInput {
        record: DocRecord {
            path: file.rel.clone(),
            stamp: file.stamp,
            title: stem.to_string(),
            ..DocRecord::default()
        },
        ..DocInput::default()
    };
    let Some(text) = read_text(&file.path, table.max_file_bytes) else {
        return doc;
    };
    let facts = parse_page(&text, stem, MAX_PAGE_LINKS);
    doc.name = tokenize(&name_text(&facts.title, &facts.tags, stem));
    doc.passages = passages::split(&text, &table.passage)
        .into_iter()
        .map(|place| {
            let start = place.start as usize;
            (place, tokenize(&text[start..start + place.len as usize]))
        })
        .collect();
    doc.record.title = facts.title;
    doc.record.tags = facts.tags;
    // Only a vault's pages write links this index can follow.
    if source.kind == Kind::Wiki {
        doc.record.links = facts
            .links
            .into_iter()
            .map(|link| (link.kind, link.target))
            .collect();
    }
    doc
}

/// A file's text as the index reads it, or nothing when it cannot be read or
/// has grown past the bound since the walk. Bytes that are not UTF-8 read as
/// the replacement character, the same way every time, so a passage's place
/// names the same bytes when the file is read again to show it.
pub(super) fn read_text(path: &Path, most: u64) -> Option<String> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(most.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > most {
        return None;
    }
    Some(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    })
}

/// The relations between the standing pages of a vault's `wiki/`, each link
/// resolved the way the knowledge graph resolves it, in path order and then
/// the order the page wrote them.
pub(super) fn edges_of(source: &Source, records: &[DocRecord], standing: &[bool]) -> Vec<Edge> {
    if source.kind != Kind::Wiki {
        return Vec::new();
    }
    let mut pages: Vec<(u32, FoundPage)> = (0_u32..)
        .zip(records)
        .filter(|(row, _)| standing[*row as usize])
        .map(|(row, record)| {
            let stem = Path::new(&record.path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(&record.path);
            (
                row,
                FoundPage {
                    id: format!("{WIKI_DIR}/{}", record.path),
                    path: PathBuf::new(),
                    stem: stem.to_string(),
                    modified_ms: 0,
                    len: 0,
                },
            )
        })
        .collect();
    pages.sort_by(|left, right| left.1.id.cmp(&right.1.id));
    let found: Vec<FoundPage> = pages.iter().map(|(_, page)| page.clone()).collect();
    let lookup = LinkLookup::of(&found);
    let rows: HashMap<&str, u32> = pages
        .iter()
        .map(|(row, page)| (page.id.as_str(), *row))
        .collect();
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for (from, _) in &pages {
        for (kind, target) in &records[*from as usize].links {
            let Some(to) = lookup.resolve(target, &rows) else {
                continue;
            };
            let edge = Edge {
                from: *from,
                to,
                kind: *kind,
            };
            // A page linking to itself is not a neighbour of itself.
            if to != *from && seen.insert(edge) {
                edges.push(edge);
            }
        }
    }
    edges
}

/// The relations file: the sum of its rows, then the rows.
fn write_edges(edges: &[Edge]) -> Vec<u8> {
    let mut rows = Vec::with_capacity(edges.len() * EDGE_BYTES);
    for edge in edges {
        rows.extend_from_slice(&edge.from.to_le_bytes());
        rows.extend_from_slice(&edge.to.to_le_bytes());
        rows.push(segment::kind_byte(edge.kind));
    }
    let mut bytes = crate::launch_budget::fnv1a(rows.iter().copied())
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&rows);
    bytes
}

fn read_edges(bytes: &[u8]) -> Option<Vec<Edge>> {
    let (sum, rows) = bytes.split_at_checked(EDGE_SUM_BYTES)?;
    if rows.len() % EDGE_BYTES != 0
        || crate::launch_budget::fnv1a(rows.iter().copied()).to_le_bytes() != sum
    {
        return None;
    }
    rows.chunks_exact(EDGE_BYTES)
        .map(|row| {
            Some(Edge {
                from: u32::from_le_bytes(row[..4].try_into().ok()?),
                to: u32::from_le_bytes(row[4..8].try_into().ok()?),
                kind: segment::kind_of(row[8])?,
            })
        })
        .collect()
}
