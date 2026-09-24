//! The live layer over the vault's graph (t-2931).
//!
//! The graph view used to be a photograph: a scan when the tab opened, a scan
//! when the button was pressed, and nothing in the picture said what had
//! changed or what an agent had just been shown. The X video's compiler
//! "doesn't wait for you to ask — it's already linking", and the half of that
//! we already had (the prompt-time recall block,
//! [`crate::second_brain_related`]) left no trace anywhere.
//!
//! This module is the four facts that make the picture live, computed once in
//! Rust so the window draws them and never re-derives them:
//!
//! - the **recall trace** — one line per injection in
//!   `<vault>/.zerocode/recall.jsonl`, bounded and rotated, read back as
//!   per-page activity ([`recall_activity`]);
//! - the **bus log** — what happened, newest first: pages created, updated and
//!   newly linked (the diff of two scans, [`diff_rows`]), pages recalled (the
//!   trace, [`recall_rows`]) and pages ingested (the tail of `wiki/log.md`,
//!   [`ingest_rows`]), merged and bounded by [`bus_log`];
//! - **merge candidates** — pairs of pages whose titles share most of their
//!   tokens or whose outbound links are the same set, with no link between them
//!   ([`merge_candidates`]) — the one function the vault lint recipe and the
//!   dedupe lens both call;
//! - the **watch targets** — which paths under `wiki/` the window's file
//!   watcher lane follows so a change re-reads the graph without a second
//!   scanner ([`watch_targets`]).
//!
//! Every number lives in [`Limits`]; the window reads the table off the wire.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::second_brain::{RAW_DIR, WIKI_DIR, WIKI_INDEX_FILE, WIKI_LOG_FILE};
use crate::second_brain_graph::{EdgeProvenance, GraphNode, NodeKind, VaultGraph};
use crate::second_brain_related::tokenize;

/// The vault's private folder, beside `raw/` and `wiki/` and never walked by
/// the graph scanner (which starts at `wiki/`).
pub const TRACE_DIR: &str = ".zerocode";
/// The recall trace inside it: one JSON object per line, append-only between
/// rotations.
pub const TRACE_FILE: &str = "recall.jsonl";

const MARKDOWN_SUFFIX: &str = ".md";
const MS_PER_MINUTE: i64 = 60_000;
const MS_PER_HOUR: i64 = 3_600_000;
const MS_PER_DAY: i64 = 86_400_000;

/// Every number the live layer has, in one place. Serialized with the graph
/// so the window's rings, lenses and rows are sized by this table and not by
/// a constant of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// Minutes a recall or a change keeps a page "alive": the activity ring on
    /// the canvas and the 「살아 있는 것만」 lens read the same window.
    pub activity_window_minutes: u32,
    /// Hours the health card's 「오늘 회상」 counts back.
    pub today_hours: u32,
    /// Lines the trace keeps after a rotation.
    pub trace_lines_max: usize,
    /// Bytes the trace may grow to before the next append rotates it.
    pub trace_rotate_bytes: u64,
    /// Bytes read from the trace's tail when the graph asks.
    pub trace_read_bytes_max: u64,
    /// Slugs one trace line is read with; the writer never exceeds
    /// [`crate::second_brain_related::MAX_RELATED_ENTRIES`], the reader
    /// refuses a hand-edited line that does.
    pub trace_pages_per_line_max: usize,
    /// Rows the bus log carries to the window.
    pub bus_rows_max: usize,
    /// Characters of one bus row's note.
    pub bus_note_chars: usize,
    /// Lines read from the tail of `wiki/log.md` for ingest rows.
    pub log_tail_lines: usize,
    /// Bytes read from the tail of `wiki/log.md`.
    pub log_tail_bytes_max: u64,
    /// Title-token overlap (Jaccard, in percent) at or above which two pages
    /// are merge candidates.
    pub merge_overlap_min_percent: u32,
    /// Tokens a title needs before its overlap means anything: 「설계」 and
    /// 「설계 노트」 are not the same page because of one word.
    pub merge_title_tokens_min: usize,
    /// A token shared by more pages than this is a stop word for pairing.
    pub merge_token_pages_max: usize,
    /// Pairs the answer carries.
    pub merge_pairs_max: usize,
    /// Folders under `wiki/` the watcher lane follows.
    pub watch_dirs_max: usize,
    /// Recently modified pages the watcher lane follows as files — a folder's
    /// stamp moves when a page is added or renamed, not when one is rewritten
    /// in place, and the page being rewritten is almost always a recent one.
    pub watch_files_max: usize,
    /// Pages one seat's answer names ([`seat_recalls`]) — the task board's
    /// 「참고한 지식」 is a short list, newest first, not the trace.
    pub seat_recalls_max: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            activity_window_minutes: 30,
            today_hours: 24,
            trace_lines_max: 2_000,
            trace_rotate_bytes: 1024 * 1024,
            trace_read_bytes_max: 512 * 1024,
            trace_pages_per_line_max: 8,
            bus_rows_max: 40,
            bus_note_chars: 120,
            log_tail_lines: 20,
            log_tail_bytes_max: 64 * 1024,
            merge_overlap_min_percent: 60,
            merge_title_tokens_min: 2,
            merge_token_pages_max: 64,
            merge_pairs_max: 64,
            watch_dirs_max: 64,
            watch_files_max: 64,
            seat_recalls_max: 12,
        }
    }
}

impl Limits {
    /// The activity window in milliseconds.
    #[must_use]
    pub fn activity_window_ms(&self) -> i64 {
        i64::from(self.activity_window_minutes) * MS_PER_MINUTE
    }

    /// 「오늘」 in milliseconds.
    #[must_use]
    pub fn today_ms(&self) -> i64 {
        i64::from(self.today_hours) * MS_PER_HOUR
    }
}

/* ---- the recall trace ---------------------------------------------------- */

/// One injection: when, which pages, for whom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecallLine {
    /// Milliseconds since the epoch.
    pub at: i64,
    /// The slugs the block named, in the order it named them.
    pub pages: Vec<String>,
    /// The agent session the block went to.
    pub session: String,
    /// The pane it was typed in (`term-<n>`), or empty for a hook whose pane
    /// the bridge did not know.
    pub pane: String,
}

/// Where a vault keeps its trace.
#[must_use]
pub fn trace_path(root: &Path) -> PathBuf {
    root.join(TRACE_DIR).join(TRACE_FILE)
}

/// Append one line. Past [`Limits::trace_rotate_bytes`] the file is rewritten
/// with its newest [`Limits::trace_lines_max`] lines — the trace is a window
/// onto recent activity, not an archive.
pub fn append_recall(root: &Path, line: &RecallLine, limits: &Limits) -> io::Result<()> {
    let path = trace_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string(line).map_err(io::Error::other)?;
    text.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(text.as_bytes())?;
    drop(file);
    if fs::metadata(&path)?.len() > limits.trace_rotate_bytes {
        keep_tail_lines(&path, limits.trace_lines_max)?;
    }
    Ok(())
}

/// The trace, oldest first, read from its tail within
/// [`Limits::trace_read_bytes_max`]. A line that does not parse is skipped:
/// the trace is advisory, and one bad line must not blind the picture.
#[must_use]
pub fn read_recalls(root: &Path, limits: &Limits) -> Vec<RecallLine> {
    let Some(text) = read_tail(&trace_path(root), limits.trace_read_bytes_max) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<RecallLine>(line).ok())
        .map(|mut line| {
            line.pages.truncate(limits.trace_pages_per_line_max);
            line
        })
        .collect()
}

/// Drop every line older than `days` — the retention sweep, on the same beat
/// and with the same days as the artifact store. Answers how many lines went.
pub fn prune_recalls(root: &Path, now_ms: i64, days: u32) -> io::Result<usize> {
    let path = trace_path(root);
    if !path.is_file() {
        return Ok(0);
    }
    let cutoff = now_ms.saturating_sub(i64::from(days).saturating_mul(MS_PER_DAY));
    let text = fs::read_to_string(&path)?;
    let mut kept = String::with_capacity(text.len());
    let mut dropped = 0;
    for line in text.lines() {
        match serde_json::from_str::<RecallLine>(line) {
            Ok(held) if held.at < cutoff => dropped += 1,
            // A line this reader cannot parse is not evidence of age; it is
            // left for the size rotation to take.
            _ => {
                kept.push_str(line);
                kept.push('\n');
            }
        }
    }
    if dropped > 0 {
        write_atomically(&path, kept.as_bytes())?;
    }
    Ok(dropped)
}

fn keep_tail_lines(path: &Path, keep: usize) -> io::Result<()> {
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(keep);
    let mut kept = String::with_capacity(text.len());
    for line in &lines[start..] {
        kept.push_str(line);
        kept.push('\n');
    }
    write_atomically(path, kept.as_bytes())
}

/// Temp beside, then rename: a reader never sees half a trace, and a crash
/// mid-write leaves the old file whole.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // An existing temporary path may be a link to a page or raw source.
    // Each writer owns a fresh sibling; create_new refuses even a dangling
    // link and keeps simultaneous snapshot writers from sharing a buffer.
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let written = file.write_all(bytes);
    drop(file);
    let result = written.and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// The last `budget` bytes of a file as text, starting at the first whole line
/// when the read began mid-line. `None` when there is no file.
fn read_tail(path: &Path, budget: u64) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(budget);
    let mut bytes = Vec::with_capacity(usize::try_from(len - start).unwrap_or(0));
    if start > 0 {
        file.seek(SeekFrom::Start(start)).ok()?;
    }
    file.read_to_end(&mut bytes).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if start > 0 {
        // The first line is a fragment whose head was cut off.
        match text.find('\n') {
            Some(at) => text.drain(..=at),
            None => text.drain(..),
        };
    }
    Some(text)
}

/* ---- activity ------------------------------------------------------------ */

/// One page's recall history, as the canvas wears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageRecall {
    /// The node id (`wiki/….md`).
    pub id: String,
    /// The newest recall.
    pub at_ms: i64,
    /// Recalls within the trace read.
    pub count: u32,
}

/// The trace folded onto the graph's pages: newest recall and count per page,
/// sorted by id. A slug the graph does not hold (a page since deleted) is
/// dropped — the picture cannot ring a point it does not draw.
#[must_use]
pub fn recall_activity(graph: &VaultGraph, lines: &[RecallLine]) -> Vec<PageRecall> {
    let index = page_index(graph);
    let mut held: BTreeMap<&str, (i64, u32)> = BTreeMap::new();
    for line in lines {
        for slug in &line.pages {
            let Some(id) = resolve_slug(graph, &index, slug) else {
                continue;
            };
            let entry = held.entry(id).or_insert((line.at, 0));
            entry.0 = entry.0.max(line.at);
            entry.1 = entry.1.saturating_add(1);
        }
    }
    held.into_iter()
        .map(|(id, (at_ms, count))| PageRecall {
            id: id.to_string(),
            at_ms,
            count,
        })
        .collect()
}

/// `id → index` for the graph's pages.
fn page_index(graph: &VaultGraph) -> HashMap<&str, usize> {
    graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind == NodeKind::Page)
        .map(|(at, node)| (node.id.as_str(), at))
        .collect()
}

/// The page a `[[slug]]` names: `wiki/x` → `wiki/x.md`, a bare `x` → the one
/// page whose stem is `x` (the vault's own log writes bare names), first in
/// sorted order when several do.
fn resolve_slug<'a>(
    graph: &'a VaultGraph,
    index: &HashMap<&'a str, usize>,
    slug: &str,
) -> Option<&'a str> {
    let slug = slug.trim();
    let slug = slug.split_once('|').map_or(slug, |(target, _)| target);
    let slug = slug.split('#').next().unwrap_or(slug).trim();
    if slug.is_empty() {
        return None;
    }
    let exact = format!("{slug}{MARKDOWN_SUFFIX}");
    if let Some((id, _)) = index.get_key_value(exact.as_str()) {
        return Some(id);
    }
    let below = format!("{WIKI_DIR}/{slug}{MARKDOWN_SUFFIX}");
    if let Some((id, _)) = index.get_key_value(below.as_str()) {
        return Some(id);
    }
    let tail = format!("/{slug}{MARKDOWN_SUFFIX}");
    graph
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Page && node.id.ends_with(&tail))
        .map(|node| node.id.as_str())
}

fn is_structural(node: &GraphNode) -> bool {
    node.id == WIKI_INDEX_FILE || node.id == WIKI_LOG_FILE
}

/* ---- the bus log --------------------------------------------------------- */

/// What a bus row says happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusKind {
    Created,
    Updated,
    Linked,
    Recalled,
    Ingested,
}

/// One line of the inspector's BUS LOG strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusRow {
    pub at_ms: i64,
    pub kind: BusKind,
    /// The node id the row is about, when the graph holds it.
    pub page: Option<String>,
    /// What to print: a title, a target, a pane, or the log line itself.
    pub note: String,
}

/// Pages created, rewritten and newly linked between two scans of the same
/// vault: the watcher's diff. A page's own modified time dates each row —
/// that is when the change happened, not when the window happened to look.
#[must_use]
pub fn diff_rows(before: &VaultGraph, after: &VaultGraph, limits: &Limits) -> Vec<BusRow> {
    let was: HashMap<&str, (i64, BTreeSet<&str>)> = before
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind == NodeKind::Page)
        .map(|(at, node)| {
            (
                node.id.as_str(),
                (node.modified_ms, out_targets(before, at)),
            )
        })
        .collect();
    let mut rows = Vec::new();
    for (at, node) in after.nodes.iter().enumerate() {
        if node.kind != NodeKind::Page {
            continue;
        }
        let now_targets = out_targets(after, at);
        match was.get(node.id.as_str()) {
            None => rows.push(BusRow {
                at_ms: node.modified_ms,
                kind: BusKind::Created,
                page: Some(node.id.clone()),
                note: clip_chars(&node.title, limits.bus_note_chars),
            }),
            Some((modified, targets)) => {
                if *modified != node.modified_ms {
                    rows.push(BusRow {
                        at_ms: node.modified_ms,
                        kind: BusKind::Updated,
                        page: Some(node.id.clone()),
                        note: clip_chars(&node.title, limits.bus_note_chars),
                    });
                }
                for target in now_targets.difference(targets) {
                    rows.push(BusRow {
                        at_ms: node.modified_ms,
                        kind: BusKind::Linked,
                        page: Some(node.id.clone()),
                        note: clip_chars(
                            &format!("{} → {}", node.title, title_of(after, target)),
                            limits.bus_note_chars,
                        ),
                    });
                }
            }
        }
    }
    bus_log(rows, limits)
}

/// The ids a page links to (ghosts included: a new link to a page not yet
/// written is still a link the page grew). Sources are not links — they are
/// the page's own `source:` line, and a scan with 「원본도」 on must not read
/// as every page having grown one.
fn out_targets(graph: &VaultGraph, at: usize) -> BTreeSet<&str> {
    let from = u32::try_from(at).unwrap_or(u32::MAX);
    graph
        .edges
        .iter()
        .filter(|edge| edge.from == from)
        .filter_map(|edge| graph.nodes.get(edge.to as usize))
        .filter(|node| node.kind != NodeKind::Source)
        .map(|node| node.id.as_str())
        .collect()
}

fn title_of<'a>(graph: &'a VaultGraph, id: &str) -> &'a str {
    graph
        .nodes
        .iter()
        .find(|node| node.id == id)
        .map_or("", |node| node.title.as_str())
}

/// The trace as rows: one per page named, so the strip reads "«page» was
/// shown to term-3" rather than a list a reader has to unpack.
#[must_use]
pub fn recall_rows(graph: &VaultGraph, lines: &[RecallLine], limits: &Limits) -> Vec<BusRow> {
    let index = page_index(graph);
    let mut rows = Vec::new();
    for line in lines {
        for slug in &line.pages {
            let page = resolve_slug(graph, &index, slug).map(str::to_string);
            let who = if line.pane.is_empty() {
                line.session.clone()
            } else {
                line.pane.clone()
            };
            rows.push(BusRow {
                at_ms: line.at,
                kind: BusKind::Recalled,
                page,
                note: clip_chars(&who, limits.bus_note_chars),
            });
        }
    }
    bus_log(rows, limits)
}

/// One page a seat was shown, with the newest time and how often.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatRecall {
    /// The page id the graph would use (`wiki/x.md`), derived from the slug
    /// the trace wrote — the window selects that node by this id.
    pub page: String,
    /// The page's frontmatter title, else its stem; the slug when the page is
    /// gone from the vault.
    pub title: String,
    /// The newest injection that named it for this seat.
    pub at_ms: i64,
    /// How many injections named it for this seat.
    pub count: u32,
}

/// The page id a trace slug names, without a graph: `wiki/x` → `wiki/x.md`, a
/// slug outside the two roots (`x`, `sub/x`) → `wiki/…x.md` — the second
/// step of [`resolve_slug`]; `|alias` and `#anchor` fall away as there.
/// `None` for an empty slug.
fn slug_page_id(slug: &str) -> Option<String> {
    let slug = slug.trim();
    let slug = slug.split_once('|').map_or(slug, |(target, _)| target);
    let slug = slug.split('#').next().unwrap_or(slug).trim();
    if slug.is_empty() {
        return None;
    }
    let rooted =
        slug.starts_with(&format!("{WIKI_DIR}/")) || slug.starts_with(&format!("{RAW_DIR}/"));
    let mut id = if rooted {
        slug.to_string()
    } else {
        format!("{WIKI_DIR}/{slug}")
    };
    if !id.ends_with(MARKDOWN_SUFFIX) {
        id.push_str(MARKDOWN_SUFFIX);
    }
    Some(id)
}

/// Which pages one seat was shown — the proven edge between a task on the
/// board and the knowledge graph (the hook wrote the line when it put the
/// block in front of that agent). The seat is the agent session when the
/// window knows it (`<agent>:session:<id>` is what the hook writes; the id
/// survives a window restart that renumbers panes), else the pane
/// (`term-<n>`). One row per page, newest first, at most
/// [`Limits::seat_recalls_max`]. Titles are read from the page heads; a page
/// the vault no longer holds keeps its slug as its title.
#[must_use]
pub fn seat_recalls(
    root: &Path,
    session_id: Option<&str>,
    pane: Option<&str>,
    limits: &Limits,
) -> Vec<SeatRecall> {
    let session_id = session_id.map(str::trim).filter(|held| !held.is_empty());
    let pane = pane.map(str::trim).filter(|held| !held.is_empty());
    if session_id.is_none() && pane.is_none() {
        return Vec::new();
    }
    let mine = |line: &RecallLine| match session_id {
        Some(id) => line
            .session
            .rsplit_once(":session:")
            .is_some_and(|(_, held)| held == id),
        None => pane.is_some_and(|held| line.pane == held),
    };
    let mut held: BTreeMap<String, (i64, u32)> = BTreeMap::new();
    for line in read_recalls(root, limits).iter().filter(|line| mine(line)) {
        for slug in &line.pages {
            let Some(id) = slug_page_id(slug) else {
                continue;
            };
            let entry = held.entry(id).or_insert((line.at, 0));
            entry.0 = entry.0.max(line.at);
            entry.1 = entry.1.saturating_add(1);
        }
    }
    let mut rows: Vec<(String, i64, u32)> = held
        .into_iter()
        .map(|(page, (at_ms, count))| (page, at_ms, count))
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rows.truncate(limits.seat_recalls_max);
    rows.into_iter()
        .map(|(page, at_ms, count)| {
            let title = crate::second_brain_graph::page_title(root, &page).unwrap_or_else(|| {
                page.trim_end_matches(MARKDOWN_SUFFIX)
                    .rsplit('/')
                    .next()
                    .unwrap_or(&page)
                    .to_string()
            });
            SeatRecall {
                page,
                title,
                at_ms,
                count,
            }
        })
        .collect()
}

/// The tail of `wiki/log.md` as rows. The skill writes
/// `2026-09-07T00:24 — <what> → [[page]] …`; a line without a leading stamp is
/// not an entry. `utc_offset_minutes` is the window's own
/// `Date.getTimezoneOffset()` — the stamps are local wall-clock time and this
/// crate keeps no zone table.
#[must_use]
pub fn ingest_rows(
    root: &Path,
    graph: &VaultGraph,
    limits: &Limits,
    utc_offset_minutes: i32,
) -> Vec<BusRow> {
    let Some(text) = read_tail(&root.join(WIKI_LOG_FILE), limits.log_tail_bytes_max) else {
        return Vec::new();
    };
    let index = page_index(graph);
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(limits.log_tail_lines);
    let mut rows = Vec::new();
    for line in &lines[start..] {
        let Some((at_ms, rest)) = stamped_line(line, utc_offset_minutes) else {
            continue;
        };
        let page = first_wikilink(rest)
            .and_then(|slug| resolve_slug(graph, &index, slug).map(str::to_string));
        rows.push(BusRow {
            at_ms,
            kind: BusKind::Ingested,
            page,
            note: clip_chars(
                rest.trim_start_matches([' ', '—', '-']).trim(),
                limits.bus_note_chars,
            ),
        });
    }
    bus_log(rows, limits)
}

/// Newest first, ties by kind then page, bounded by [`Limits::bus_rows_max`].
#[must_use]
pub fn bus_log(mut rows: Vec<BusRow>, limits: &Limits) -> Vec<BusRow> {
    rows.sort_by(|left, right| {
        right
            .at_ms
            .cmp(&left.at_ms)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.page.cmp(&right.page))
            .then_with(|| left.note.cmp(&right.note))
    });
    rows.dedup();
    rows.truncate(limits.bus_rows_max);
    rows
}

/// `YYYY-MM-DDTHH:MM[:SS]` at the head of a line → (epoch ms, the rest).
fn stamped_line(line: &str, utc_offset_minutes: i32) -> Option<(i64, &str)> {
    let bytes = line.as_bytes();
    let digits = |from: usize, len: usize| -> Option<i64> {
        let slice = bytes.get(from..from + len)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    };
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
    {
        return None;
    }
    let year = digits(0, 4)?;
    let month = digits(5, 2)?;
    let day = digits(8, 2)?;
    let hour = digits(11, 2)?;
    let minute = digits(14, 2)?;
    let mut end = 16;
    let mut second = 0;
    if bytes.get(16) == Some(&b':') {
        second = digits(17, 2)?;
        end = 19;
    }
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let local_ms = days * MS_PER_DAY + hour * MS_PER_HOUR + minute * MS_PER_MINUTE + second * 1_000;
    let at_ms = local_ms + i64::from(utc_offset_minutes) * MS_PER_MINUTE;
    Some((at_ms, line.get(end..).unwrap_or("")))
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn first_wikilink(text: &str) -> Option<&str> {
    let start = text.find("[[")? + 2;
    let end = text[start..].find("]]")? + start;
    Some(&text[start..end])
}

fn clip_chars(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/* ---- merge candidates ---------------------------------------------------- */

/// Why two pages look like one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeReason {
    /// Their titles share at least [`Limits::merge_overlap_min_percent`] of
    /// their tokens.
    TitleOverlap,
    /// They link to exactly the same pages, and to at least one.
    SameLinks,
}

impl MergeReason {
    /// The reason as the lint table spells it — the `serde` name, so the
    /// recipe's JSON and the card's row say one word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TitleOverlap => "title_overlap",
            Self::SameLinks => "same_links",
        }
    }
}

/// A pair the dedupe lens draws as a dashed `merge?` edge and the lint recipe
/// lists. Indices into [`VaultGraph::nodes`], `left < right`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeCandidate {
    pub left: u32,
    pub right: u32,
    pub reason: MergeReason,
    /// Title overlap in percent; a same-links pair scores 100.
    pub score: u32,
    /// The road that wrote the pair (t-5966): always
    /// [`EdgeProvenance::Measured`], carried on the wire so the dedupe lens
    /// dresses its `merge?` line from the answer and spells no road itself.
    pub provenance: EdgeProvenance,
}

/// Pages that may be one page: high title-token overlap
/// ([`crate::second_brain_related::tokenize`]) or identical outbound link
/// sets, and no link between them in either direction. Highest score first,
/// then by index, bounded by [`Limits::merge_pairs_max`].
///
/// The vault's home and journal are never candidates, and neither is a ghost
/// or a source: a candidate is a page somebody could merge into another.
#[must_use]
pub fn merge_candidates(graph: &VaultGraph, limits: &Limits) -> Vec<MergeCandidate> {
    let pages: Vec<usize> = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind == NodeKind::Page && !is_structural(node))
        .map(|(at, _)| at)
        .collect();
    let linked: HashSet<(u32, u32)> = graph
        .edges
        .iter()
        .map(|edge| (edge.from.min(edge.to), edge.from.max(edge.to)))
        .collect();
    let mut found: BTreeMap<(u32, u32), (MergeReason, u32)> = BTreeMap::new();
    let pair_of = |left: usize, right: usize| -> Option<(u32, u32)> {
        if left == right {
            return None;
        }
        let key = (
            u32::try_from(left.min(right)).ok()?,
            u32::try_from(left.max(right)).ok()?,
        );
        (!linked.contains(&key)).then_some(key)
    };

    // Title overlap through an inverted index: only pairs sharing a token are
    // ever compared, and a token half the vault shares compares nobody.
    let tokens: HashMap<usize, BTreeSet<String>> = pages
        .iter()
        .map(|&at| (at, tokenize(&graph.nodes[at].title)))
        .filter(|(_, held)| held.len() >= limits.merge_title_tokens_min)
        .collect();
    let mut postings: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for &at in &pages {
        if let Some(held) = tokens.get(&at) {
            for token in held {
                postings.entry(token.as_str()).or_default().push(at);
            }
        }
    }
    for holders in postings.values() {
        if holders.len() > limits.merge_token_pages_max {
            continue;
        }
        for (rank, &left) in holders.iter().enumerate() {
            for &right in &holders[rank + 1..] {
                let Some(key) = pair_of(left, right) else {
                    continue;
                };
                if found.contains_key(&key) {
                    continue;
                }
                let (a, b) = (&tokens[&left], &tokens[&right]);
                let shared = a.intersection(b).count();
                let union = a.union(b).count();
                if union == 0 {
                    continue;
                }
                let percent = u32::try_from(shared * 100 / union).unwrap_or(0);
                if percent >= limits.merge_overlap_min_percent {
                    found.insert(key, (MergeReason::TitleOverlap, percent));
                }
            }
        }
    }

    // Identical outbound link sets, grouped by the set itself.
    let mut by_links: BTreeMap<Vec<u32>, Vec<usize>> = BTreeMap::new();
    for &at in &pages {
        let from = u32::try_from(at).unwrap_or(u32::MAX);
        let targets: BTreeSet<u32> = graph
            .edges
            .iter()
            .filter(|edge| edge.from == from)
            .filter(|edge| {
                graph
                    .nodes
                    .get(edge.to as usize)
                    .is_some_and(|node| !is_structural(node) && node.kind != NodeKind::Source)
            })
            .map(|edge| edge.to)
            .collect();
        if targets.is_empty() {
            continue;
        }
        by_links
            .entry(targets.into_iter().collect())
            .or_default()
            .push(at);
    }
    for group in by_links.values() {
        if group.len() > limits.merge_token_pages_max {
            continue;
        }
        for (rank, &left) in group.iter().enumerate() {
            for &right in &group[rank + 1..] {
                let Some(key) = pair_of(left, right) else {
                    continue;
                };
                let entry = found.entry(key).or_insert((MergeReason::SameLinks, 100));
                if entry.0 == MergeReason::TitleOverlap {
                    // Both reasons: the title's own words are the one a person
                    // can check at a glance; the score says how sure.
                    entry.1 = 100;
                }
            }
        }
    }

    let mut candidates: Vec<MergeCandidate> = found
        .into_iter()
        .map(|((left, right), (reason, score))| MergeCandidate {
            left,
            right,
            reason,
            score,
            // The measured road: nobody wrote this pair in the vault.
            provenance: EdgeProvenance::Measured,
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.left.cmp(&b.left))
            .then_with(|| a.right.cmp(&b.right))
    });
    candidates.truncate(limits.merge_pairs_max);
    candidates
}

/* ---- the watcher lane ---------------------------------------------------- */

/// The paths the window's file watcher follows for this vault, named for the
/// lane: `wiki/` itself, the home and the journal, every first-level folder
/// the graph saw (bounded), and the newest pages as files (bounded). A folder
/// hears a page added, removed or renamed; a watched file hears a rewrite.
#[must_use]
pub fn watch_targets(root: &Path, graph: &VaultGraph, limits: &Limits) -> Vec<(String, PathBuf)> {
    let wiki = root.join(WIKI_DIR);
    let mut targets = vec![
        (format!("{WIKI_DIR}/"), wiki.clone()),
        (WIKI_INDEX_FILE.to_string(), root.join(WIKI_INDEX_FILE)),
        (WIKI_LOG_FILE.to_string(), root.join(WIKI_LOG_FILE)),
    ];
    let folders: BTreeSet<&str> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Page && !node.folder.is_empty())
        .map(|node| node.folder.as_str())
        .collect();
    for folder in folders.into_iter().take(limits.watch_dirs_max) {
        targets.push((format!("{WIKI_DIR}/{folder}/"), wiki.join(folder)));
    }
    let mut pages: Vec<&GraphNode> = graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Page && !is_structural(node))
        .collect();
    pages.sort_by(|left, right| {
        right
            .modified_ms
            .cmp(&left.modified_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    for node in pages.into_iter().take(limits.watch_files_max) {
        let Some(path) = crate::second_brain_graph::page_path(root, &node.id) else {
            continue;
        };
        targets.push((node.id.clone(), path));
    }
    targets
}

/* ---- the layer as the window reads it ------------------------------------ */

/// Everything live about one vault, beside its graph on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveLayer {
    /// The clock the window measures the windows against — the backend's, so
    /// a machine whose webview clock disagrees still rings the right points.
    pub now_ms: i64,
    pub recalled: Vec<PageRecall>,
    pub bus: Vec<BusRow>,
    pub merge: Vec<MergeCandidate>,
    pub limits: Limits,
}

/// The layer for `graph`, read from the vault at `root`. `changes` are the
/// rows the caller kept from earlier diffs (the watcher's memory between
/// scans); this function reads the trace and the journal and folds everything
/// into one bounded bus.
#[must_use]
pub fn live_layer(
    root: &Path,
    graph: &VaultGraph,
    changes: Vec<BusRow>,
    now_ms: i64,
    utc_offset_minutes: i32,
    limits: &Limits,
) -> LiveLayer {
    let lines = read_recalls(root, limits);
    let mut rows = changes;
    rows.extend(recall_rows(graph, &lines, limits));
    rows.extend(ingest_rows(root, graph, limits, utc_offset_minutes));
    LiveLayer {
        now_ms,
        recalled: recall_activity(graph, &lines),
        bus: bus_log(rows, limits),
        merge: merge_candidates(graph, limits),
        limits: *limits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::second_brain_graph::{EdgeKind, EdgeProvenance, GraphEdge};

    fn page(id: &str, title: &str, modified_ms: i64) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            title: title.to_string(),
            tags: Vec::new(),
            kind: NodeKind::Page,
            modified_ms,
            out_links: 0,
            in_links: 0,
            source: None,
            excerpt: String::new(),
            folder: id
                .strip_prefix("wiki/")
                .and_then(|rest| rest.rsplit_once('/'))
                .map(|(folder, _)| folder.to_string())
                .unwrap_or_default(),
        }
    }

    fn graph(nodes: Vec<GraphNode>, edges: &[(u32, u32)]) -> VaultGraph {
        VaultGraph {
            pages: nodes.iter().filter(|h| h.kind == NodeKind::Page).count(),
            nodes,
            edges: edges
                .iter()
                .map(|(from, to)| GraphEdge {
                    from: *from,
                    to: *to,
                    kind: EdgeKind::Mentions,
                    provenance: EdgeProvenance::Inferred,
                })
                .collect(),
            ..VaultGraph::default()
        }
    }

    fn line(at: i64, pages: &[&str], pane: &str) -> RecallLine {
        RecallLine {
            at,
            pages: pages.iter().map(|h| (*h).to_string()).collect(),
            session: "claude:s".to_string(),
            pane: pane.to_string(),
        }
    }

    #[test]
    fn a_recall_is_appended_read_back_oldest_first_and_rotated_by_size() {
        let vault = tempfile::tempdir().expect("vault");
        let limits = Limits {
            trace_rotate_bytes: 400,
            trace_lines_max: 3,
            ..Limits::default()
        };
        for at in 1..=3 {
            append_recall(vault.path(), &line(at, &["wiki/a"], "term-1"), &limits).expect("append");
        }
        let held = read_recalls(vault.path(), &limits);
        assert_eq!(held.iter().map(|l| l.at).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(trace_path(vault.path()).is_file());
        // Past the byte bound the file keeps its newest lines only.
        for at in 4..=12 {
            append_recall(vault.path(), &line(at, &["wiki/a"], "term-1"), &limits).expect("append");
        }
        let held = read_recalls(vault.path(), &limits);
        assert!(
            held.len() <= limits.trace_lines_max + 1,
            "{} lines survived",
            held.len()
        );
        assert_eq!(held.last().map(|l| l.at), Some(12));
        assert!(held.first().map(|l| l.at).unwrap_or(0) >= 9, "{:?}", held);
        // A hand-edited line that does not parse is skipped, not fatal.
        let path = trace_path(vault.path());
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("not json\n");
        std::fs::write(&path, text).unwrap();
        let again = read_recalls(vault.path(), &limits);
        assert_eq!(again.len(), held.len());
    }

    #[test]
    fn the_tail_read_starts_on_a_whole_line_and_caps_pages_per_line() {
        let vault = tempfile::tempdir().expect("vault");
        let limits = Limits {
            trace_read_bytes_max: 120,
            trace_pages_per_line_max: 2,
            ..Limits::default()
        };
        for at in 1..=6 {
            append_recall(
                vault.path(),
                &line(at, &["wiki/a", "wiki/b", "wiki/c"], "term-1"),
                &Limits::default(),
            )
            .expect("append");
        }
        let held = read_recalls(vault.path(), &limits);
        assert!(!held.is_empty() && held.len() < 6, "{held:?}");
        assert!(held.iter().all(|l| l.pages.len() == 2), "{held:?}");
        assert_eq!(held.last().map(|l| l.at), Some(6));
    }

    #[test]
    fn pruning_drops_lines_older_than_the_retention_days() {
        let vault = tempfile::tempdir().expect("vault");
        let now = 100 * MS_PER_DAY;
        for at in [now - 40 * MS_PER_DAY, now - 10 * MS_PER_DAY, now] {
            append_recall(vault.path(), &line(at, &["wiki/a"], ""), &Limits::default()).unwrap();
        }
        assert_eq!(prune_recalls(vault.path(), now, 30).unwrap(), 1);
        let held = read_recalls(vault.path(), &Limits::default());
        assert_eq!(held.len(), 2);
        assert!(held.iter().all(|l| l.at >= now - 30 * MS_PER_DAY));
        assert_eq!(prune_recalls(vault.path(), now, 30).unwrap(), 0);
        let empty = tempfile::tempdir().expect("empty");
        assert_eq!(prune_recalls(empty.path(), now, 30).unwrap(), 0);
    }

    #[test]
    fn seat_recalls_follow_the_session_key_else_the_pane_and_read_titles_from_page_heads() {
        let vault = tempfile::tempdir().expect("vault");
        let wiki = vault.path().join(WIKI_DIR);
        std::fs::create_dir_all(wiki.join("sub")).unwrap();
        std::fs::write(wiki.join("a.md"), "---\ntitle: 알파\n---\nbody\n").unwrap();
        std::fs::write(wiki.join("sub").join("b.md"), "no frontmatter\n").unwrap();
        let limits = Limits {
            seat_recalls_max: 2,
            ..Limits::default()
        };
        let write = |at: i64, pages: &[&str], session: &str, pane: &str| {
            let line = RecallLine {
                at,
                pages: pages.iter().map(|h| (*h).to_string()).collect(),
                session: session.to_string(),
                pane: pane.to_string(),
            };
            append_recall(vault.path(), &line, &limits).expect("append");
        };
        write(10, &["wiki/a", "sub/b"], "claude:session:one", "term-1");
        write(20, &["wiki/a"], "claude:session:one", "term-1");
        // The same pane number after a restart belongs to another session.
        write(30, &["gone", "wiki/a"], "codex:session:two", "term-1");
        // The same session in a renumbered pane is still this seat.
        write(40, &["wiki/sub/b"], "claude:session:one", "term-9");
        let shape = |rows: &[SeatRecall]| {
            rows.iter()
                .map(|r| (r.page.clone(), r.title.clone(), r.at_ms, r.count))
                .collect::<Vec<_>>()
        };
        let by_session = seat_recalls(vault.path(), Some("one"), Some("term-1"), &limits);
        assert_eq!(
            shape(&by_session),
            vec![
                ("wiki/sub/b.md".to_string(), "b".to_string(), 40, 2),
                ("wiki/a.md".to_string(), "알파".to_string(), 20, 2),
            ]
        );
        // Without a session the pane is the seat; the bound keeps the newest
        // pages, and a page the vault lost keeps its slug as its title.
        let wide = Limits {
            seat_recalls_max: 3,
            ..Limits::default()
        };
        let by_pane = seat_recalls(vault.path(), None, Some("term-1"), &wide);
        assert_eq!(
            shape(&by_pane),
            vec![
                ("wiki/a.md".to_string(), "알파".to_string(), 30, 3),
                ("wiki/gone.md".to_string(), "gone".to_string(), 30, 1),
                ("wiki/sub/b.md".to_string(), "b".to_string(), 10, 1),
            ]
        );
        assert_eq!(
            seat_recalls(vault.path(), None, Some("term-1"), &limits).len(),
            2
        );
        assert!(seat_recalls(vault.path(), None, None, &limits).is_empty());
        assert!(seat_recalls(vault.path(), Some(""), Some(" "), &limits).is_empty());
        assert!(seat_recalls(vault.path(), Some("nobody"), Some("term-1"), &limits).is_empty());
    }

    #[test]
    fn recall_activity_maps_slugs_to_pages_and_keeps_the_newest() {
        let held = graph(
            vec![
                page("wiki/a.md", "A", 0),
                page("wiki/sub/b.md", "B", 0),
                page("wiki/index.md", "index", 0),
            ],
            &[],
        );
        let lines = [
            line(10, &["wiki/a", "b"], "term-1"),
            line(20, &["wiki/a", "wiki/gone"], "term-2"),
            line(15, &["sub/b|alias"], ""),
        ];
        let activity = recall_activity(&held, &lines);
        assert_eq!(
            activity,
            vec![
                PageRecall {
                    id: "wiki/a.md".into(),
                    at_ms: 20,
                    count: 2
                },
                PageRecall {
                    id: "wiki/sub/b.md".into(),
                    at_ms: 15,
                    count: 2
                },
            ]
        );
        let rows = recall_rows(&held, &lines, &Limits::default());
        assert_eq!(rows[0].at_ms, 20);
        assert_eq!(rows[0].kind, BusKind::Recalled);
        assert_eq!(rows[0].note, "term-2");
        // A slug the graph does not hold keeps its row, pageless.
        assert!(rows.iter().any(|r| r.page.is_none() && r.at_ms == 20));
        // No pane: the session names the reader.
        assert!(rows.iter().any(|r| r.note == "claude:s"));
    }

    #[test]
    fn a_diff_says_created_updated_and_linked_dated_by_the_page() {
        let before = graph(
            vec![page("wiki/a.md", "A", 100), page("wiki/b.md", "B", 100)],
            &[(0, 1)],
        );
        let after = graph(
            vec![
                page("wiki/a.md", "A", 100),
                page("wiki/b.md", "B", 300),
                page("wiki/c.md", "C", 200),
            ],
            &[(0, 1), (1, 2), (2, 0)],
        );
        let rows = diff_rows(&before, &after, &Limits::default());
        let said: Vec<(BusKind, &str, i64)> = rows
            .iter()
            .map(|r| (r.kind, r.page.as_deref().unwrap_or(""), r.at_ms))
            .collect();
        assert_eq!(
            said,
            vec![
                (BusKind::Updated, "wiki/b.md", 300),
                (BusKind::Linked, "wiki/b.md", 300),
                (BusKind::Created, "wiki/c.md", 200),
            ],
            "{rows:?}"
        );
        assert_eq!(rows[1].note, "B → C");
        // The same vault twice says nothing.
        assert!(diff_rows(&after, &after, &Limits::default()).is_empty());
    }

    #[test]
    fn the_journals_tail_becomes_ingest_rows_in_the_windows_zone() {
        let vault = tempfile::tempdir().expect("vault");
        let wiki = vault.path().join("wiki");
        std::fs::create_dir_all(&wiki).unwrap();
        std::fs::write(
            wiki.join("log.md"),
            "# 취합 일지\n\n2026-09-07T00:24 — 감사 → [[audit]] 신설\n\nnot an entry\n\n2026-09-07T01:09 — 사건 → [[wiki/late]] 신설(가드 셋).\n",
        )
        .unwrap();
        let held = graph(
            vec![
                page("wiki/audit.md", "Audit", 0),
                page("wiki/late.md", "Late", 0),
            ],
            &[],
        );
        // Korea: getTimezoneOffset() is -540.
        let rows = ingest_rows(vault.path(), &held, &Limits::default(), -540);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].page.as_deref(), Some("wiki/late.md"));
        assert_eq!(rows[0].kind, BusKind::Ingested);
        assert!(
            rows[0].note.starts_with("사건 → [[wiki/late]]"),
            "{}",
            rows[0].note
        );
        // 2026-09-07T00:24 KST = 2026-09-06T15:24Z.
        let (at, _) = stamped_line("2026-09-07T00:24 — x", -540).unwrap();
        assert_eq!(
            at,
            days_from_civil(2026, 9, 6) * MS_PER_DAY + 15 * MS_PER_HOUR + 24 * MS_PER_MINUTE
        );
        assert_eq!(rows[1].at_ms, at);
        assert!(stamped_line("2026-13-07T00:24 — x", 0).is_none());
        assert!(stamped_line("no stamp", 0).is_none());
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        // A tail bound in lines keeps the newest.
        let few = Limits {
            log_tail_lines: 1,
            ..Limits::default()
        };
        assert_eq!(ingest_rows(vault.path(), &held, &few, -540).len(), 1);
    }

    #[test]
    fn the_bus_is_newest_first_deduplicated_and_bounded() {
        let row = |at, kind, note: &str| BusRow {
            at_ms: at,
            kind,
            page: None,
            note: note.into(),
        };
        let limits = Limits {
            bus_rows_max: 3,
            ..Limits::default()
        };
        let rows = bus_log(
            vec![
                row(1, BusKind::Created, "a"),
                row(5, BusKind::Recalled, "b"),
                row(5, BusKind::Recalled, "b"),
                row(3, BusKind::Ingested, "c"),
                row(4, BusKind::Updated, "d"),
            ],
            &limits,
        );
        assert_eq!(
            rows.iter().map(|r| r.at_ms).collect::<Vec<_>>(),
            vec![5, 4, 3]
        );
    }

    #[test]
    fn merge_candidates_come_from_titles_or_links_and_never_from_a_linked_pair() {
        let held = graph(
            vec![
                page("wiki/a.md", "zo model catalog discovery", 0),
                page("wiki/b.md", "zo model catalog facts", 0),
                page("wiki/c.md", "hook bridge design", 0),
                page("wiki/d.md", "hook bridge design notes", 0),
                page("wiki/e.md", "unrelated", 0),
                page("wiki/f.md", "other", 0),
                page("wiki/g.md", "third", 0),
                page("wiki/index.md", "index", 0),
            ],
            // c and d are linked: never a pair. e and f share every link.
            &[(2, 3), (4, 6), (5, 6), (7, 0), (7, 1)],
        );
        let found = merge_candidates(&held, &Limits::default());
        assert_eq!(
            found,
            vec![
                MergeCandidate {
                    left: 4,
                    right: 5,
                    reason: MergeReason::SameLinks,
                    score: 100,
                    provenance: EdgeProvenance::Measured,
                },
                MergeCandidate {
                    left: 0,
                    right: 1,
                    reason: MergeReason::TitleOverlap,
                    score: 60,
                    provenance: EdgeProvenance::Measured,
                },
            ],
            "{found:?}"
        );
        // One-token titles never pair, and the bound holds.
        let tiny = Limits {
            merge_pairs_max: 1,
            ..Limits::default()
        };
        assert_eq!(merge_candidates(&held, &tiny).len(), 1);
        let stop = Limits {
            merge_token_pages_max: 1,
            ..Limits::default()
        };
        assert!(
            merge_candidates(&held, &stop)
                .iter()
                .all(|c| c.reason == MergeReason::SameLinks)
        );
    }

    #[test]
    fn watch_targets_are_the_wiki_its_folders_and_the_newest_pages_bounded() {
        let vault = tempfile::tempdir().expect("vault");
        let held = graph(
            vec![
                page("wiki/a.md", "A", 10),
                page("wiki/sub/b.md", "B", 30),
                page("wiki/sub/c.md", "C", 20),
                page("wiki/index.md", "index", 99),
            ],
            &[],
        );
        let limits = Limits {
            watch_files_max: 2,
            ..Limits::default()
        };
        let targets = watch_targets(vault.path(), &held, &limits);
        let names: Vec<&str> = targets.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "wiki/",
                "wiki/index.md",
                "wiki/log.md",
                "wiki/sub/",
                "wiki/sub/b.md",
                "wiki/sub/c.md"
            ]
        );
        assert_eq!(targets[3].1, vault.path().join("wiki/sub"));
        assert_eq!(targets[4].1, vault.path().join("wiki/sub/b.md"));
    }

    #[test]
    fn the_layer_folds_trace_journal_and_kept_changes_into_one_bus() {
        let vault = tempfile::tempdir().expect("vault");
        std::fs::create_dir_all(vault.path().join("wiki")).unwrap();
        std::fs::write(
            vault.path().join("wiki/log.md"),
            "2026-01-01T00:00 — 취합 → [[a]]\n",
        )
        .unwrap();
        append_recall(
            vault.path(),
            &line(50, &["wiki/a"], "term-1"),
            &Limits::default(),
        )
        .unwrap();
        let held = graph(vec![page("wiki/a.md", "A", 0)], &[]);
        let kept = vec![BusRow {
            at_ms: 70,
            kind: BusKind::Created,
            page: Some("wiki/a.md".into()),
            note: "A".into(),
        }];
        let layer = live_layer(vault.path(), &held, kept, 1_000, 0, &Limits::default());
        assert_eq!(layer.now_ms, 1_000);
        assert_eq!(layer.recalled.len(), 1);
        assert_eq!(
            layer.bus.iter().map(|r| r.kind).collect::<Vec<_>>(),
            vec![BusKind::Ingested, BusKind::Created, BusKind::Recalled]
        );
        assert_eq!(layer.limits, Limits::default());
        assert!(layer.merge.is_empty());
    }
}
