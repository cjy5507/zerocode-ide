//! The vault's `wiki/` as a read-only recall corpus.
//!
//! There is deliberately no second retriever here. A page becomes an ordinary
//! [`MemoryEntry`] pointer plus the text it is indexed by, and the existing
//! lexical index in [`crate::memory::recall`] scores it beside the project's
//! own memories. Everything the recall section already guarantees — the
//! five-entry clamp, the per-field byte caps, the reserve the compaction
//! preflight sizes against — therefore holds unchanged, because the corpus adds
//! candidates rather than a second section.
//!
//! Two budgets keep a large vault from becoming a startup cost:
//!
//! * **Per page** — [`MAX_PAGE_INDEX_BYTES`] of each file is read. Nothing more
//!   is opened, and the bytes are dropped as soon as they are tokenized: the
//!   index holds tokens, never page bodies.
//! * **Per vault** — [`MAX_INDEXED_PAGES`] pages, in a deterministic walk
//!   order, so a runaway vault degrades to a prefix instead of a stall.
//!
//! A process-wide `PAGE_CACHE` keyed by path and stamped with each file's
//! `(mtime, len)` makes every scan after the first almost free — which matters
//! because one session rebuilds its runtime several times (`/model`, `/resume`)
//! and each rebuild would otherwise re-read and re-tokenize the whole vault.
//! Cached entries are shared with the live retrievers rather than copied.
//!
//! # The links between pages
//!
//! The same 8 KiB also yields the page's [`Relation`]s — the five typed
//! frontmatter keys and every body `[[link]]` — because a vault's own links are
//! the one relevance signal a lexical index cannot see. Recall expands them:
//! see [`crate::memory::recall::GRAPH_RELATION_DECAY`].
//!
//! Resolution is a whole-vault fact, so it happens after the walk and is
//! redone on every scan: a page cached from an earlier scan keeps its tokens
//! and is re-linked, never re-read. Outgoing relations live on the page;
//! incoming ones live on [`CorpusScan`], which is rebuilt each scan, because a
//! cached page is an `Arc` shared with every retriever built since and cannot
//! learn about a neighbour written after it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use core_types::MemoryEntry;

use super::{SecondBrain, WIKI_DIR};
use crate::memory::recall::{IndexedCorpusPage, CORPUS_SUMMARY_MAX_BYTES};

/// Bytes read from one page. The head of a page carries its frontmatter, its
/// title and the claim it exists to make; the tail is elaboration a recall hit
/// leads the model to read in full anyway.
pub const MAX_PAGE_INDEX_BYTES: usize = 8 * 1024;

/// Pages one vault contributes. Past this the scan stops and says so, rather
/// than spending an unbounded amount of a session's startup on knowledge that
/// five recall slots could never surface.
pub const MAX_INDEXED_PAGES: usize = 5_000;

/// Directories descended into. A vault is a shallow tree of topics, and this
/// stops a stray symlink loop or a `node_modules` that wandered in from turning
/// the walk into a crawl.
const MAX_WALK_DEPTH: usize = 8;

/// Markdown extension. Everything else in `wiki/` (attachments, canvases) is
/// not prose and is skipped.
const PAGE_EXTENSION: &str = "md";

/// Relations one page contributes, typed and body links together. A page that
/// links a few dozen others is a hub; one linking thousands is a generated
/// index whose every neighbour would be a tie-break, which is not a signal.
pub const MAX_PAGE_RELATIONS: usize = 64;

/// Relation suffixes one pointer line carries. Three names the page's closest
/// company without turning the recall section into an adjacency list.
const MAX_SUMMARY_RELATIONS: usize = 3;

/// Frontmatter key holding when a page entered the vault. The spelling is a
/// contract with [`super::promote`], which writes it, and with the vault's own
/// house rules, which ask a person to. Recall reads it for one purpose: when
/// two pages `contradicts` each other, saying which of the two is the later
/// word.
pub const INGESTED_AT_FIELD: &str = "ingested_at";

/// `ingested_at` as seconds since the Unix epoch, or nothing when it is not a
/// date this can be sure of.
///
/// Enough of RFC 3339 for a frontmatter stamp: `YYYY-MM-DD`, optionally
/// `Thh:mm[:ss]`, optionally `Z` or `±hh[:]mm`. The offset is applied rather
/// than ignored, because a vault written on two machines in two zones would
/// otherwise report the wrong page as the later one — the exact mistake this
/// exists to prevent. Anything else answers `None`, and a caller with no answer
/// says nothing about which page is newer rather than guessing.
#[must_use]
pub fn ingested_at_seconds(stamp: &str) -> Option<i64> {
    let stamp = stamp.trim();
    let (date, rest) = match stamp.split_once(['T', 't', ' ']) {
        Some((date, rest)) => (date, rest),
        None => (stamp, ""),
    };
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let (time, offset) = split_offset(rest);
    let mut clock = time.split(':');
    let hour: i64 = parse_field(clock.next())?;
    let minute: i64 = parse_field(clock.next())?;
    // Fractional seconds are dropped: two stamps a second apart are already
    // beyond what a frontmatter date claims to resolve.
    let second: i64 = parse_field(clock.next().map(|held| {
        held.split_once('.')
            .map_or(held, |(whole, _fraction)| whole)
    }))?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second - offset)
}

/// One clock field, or zero when the stamp stopped before it.
fn parse_field(field: Option<&str>) -> Option<i64> {
    match field {
        None | Some("") => Some(0),
        Some(value) => value.parse().ok(),
    }
}

/// The clock and the zone offset in seconds, from the part after the date.
fn split_offset(rest: &str) -> (&str, i64) {
    let rest = rest.trim();
    if let Some(time) = rest.strip_suffix(['Z', 'z']) {
        return (time, 0);
    }
    let Some(at) = rest.rfind(['+', '-']) else {
        return (rest, 0);
    };
    let (time, zone) = rest.split_at(at);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let digits = &zone[1..];
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if digits.len() == 4 => digits.split_at(2),
        None => (digits, "0"),
    };
    let offset = hours
        .parse::<i64>()
        .ok()
        .zip(minutes.parse::<i64>().ok())
        .map_or(0, |(hours, minutes)| sign * (hours * 3_600 + minutes * 60));
    (time, offset)
}

/// Days from 1970-01-01 to a proleptic-Gregorian date (Howard Hinnant's
/// `days_from_civil`), so ordering two stamps needs no calendar dependency.
fn days_from_civil(year: i64, month: u32, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// What one relation means. The variant order is the order relations are
/// stored and rendered in, so a scan reads the same twice.
///
/// Every name but [`RelationKind::Mentions`] is also the frontmatter key that
/// writes it — a page says `implements: [[wiki/adr/003]]`. `mentions` is what a
/// body `[[link]]` means and is never spelled as a key. The spellings are a
/// contract with the window's own graph view
/// (`zerocode_core::second_brain_graph::EdgeKind`), which reads the same vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationKind {
    Mentions,
    Related,
    Implements,
    DependsOn,
    Supersedes,
    Contradicts,
}

impl RelationKind {
    /// The spelling shared with the frontmatter key and the rendered suffix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mentions => "mentions",
            Self::Related => "related",
            Self::Implements => "implements",
            Self::DependsOn => "depends_on",
            Self::Supersedes => "supersedes",
            Self::Contradicts => "contradicts",
        }
    }

    /// Every kind a frontmatter key may declare, in the order they are read.
    const TYPED: [Self; 5] = [
        Self::Related,
        Self::Implements,
        Self::DependsOn,
        Self::Supersedes,
        Self::Contradicts,
    ];
}

/// One outgoing link of a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub kind: RelationKind,
    /// The slug this points at when a page in the vault answers to it, else the
    /// target exactly as written. An unresolved target is kept because "the
    /// page I keep linking to and never wrote" is worth showing on the pointer
    /// line; only ranking ignores it.
    pub target: String,
    pub resolved: bool,
}

/// The result of one walk of a vault's `wiki/`.
#[derive(Debug, Clone, Default)]
pub struct CorpusScan {
    /// Indexed pages, in walk order.
    pub pages: Vec<Arc<IndexedCorpusPage>>,
    /// Who points AT each slug, by target — the reverse of every page's
    /// resolved outgoing relations.
    ///
    /// It lives on the scan rather than the page because a page is an `Arc`
    /// reused across scans: the neighbours pointing at it change when other
    /// files change, and a cached page must not be rewritten to learn that.
    pub incoming: BTreeMap<String, Vec<(RelationKind, String)>>,
    /// Whether [`MAX_INDEXED_PAGES`] cut the walk short.
    pub capped: bool,
}

/// One page as it was last read: the stamp that says whether the file moved,
/// the indexed form shared with every retriever built since, and the two facts
/// re-linking needs so a resolution change never costs a second read.
struct CachedPage {
    stamp: (SystemTime, u64),
    page: Arc<IndexedCorpusPage>,
    /// The pointer summary before any relation suffix.
    base_summary: String,
    /// Targets as written. Resolution is a whole-vault fact and is redone every
    /// scan, so what survives a scan is the unresolved form.
    written: Vec<(RelationKind, String)>,
    /// The page's `ingested_at`, as written.
    ingested_at: Option<String>,
}

/// One page after the walk, before the vault-wide link resolution.
struct PendingPage {
    path: PathBuf,
    stamp: (SystemTime, u64),
    slug: String,
    /// The pointer summary before any relation suffix.
    base_summary: String,
    /// Targets as written, before resolution.
    written: Vec<(RelationKind, String)>,
    /// The page's `ingested_at`, as written.
    ingested_at: Option<String>,
    source: PendingSource,
}

/// Where a pending page's index comes from.
enum PendingSource {
    /// Read this scan. The body is still in hand, so the page is tokenized once
    /// — after resolution, with its relations already known.
    Fresh { entry: MemoryEntry, body: String },
    /// Reused from the cache. Its tokens stand; only its links may have moved.
    Cached(Arc<IndexedCorpusPage>),
}

/// Last reading of every page, keyed by absolute path.
///
/// Rebuilt (not merely updated) by each scan, so a deleted page's tokens are
/// dropped instead of accumulating for the life of the process.
static PAGE_CACHE: Mutex<Option<BTreeMap<PathBuf, CachedPage>>> = Mutex::new(None);

/// Walk `wiki/**/*.md` and return the indexed corpus.
///
/// Cheap to call repeatedly: an unchanged page costs one `stat` and an `Arc`
/// clone.
#[must_use]
pub fn scan(vault: &SecondBrain) -> CorpusScan {
    let wiki = vault.wiki_dir();
    if !wiki.is_dir() {
        return CorpusScan::default();
    }
    let mut previous = PAGE_CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or_default();

    let mut pending = Vec::new();
    let mut capped = false;
    walk(&wiki, &wiki, 0, &mut previous, &mut pending, &mut capped);

    let (scan, fresh) = resolve(pending, capped);
    *PAGE_CACHE.lock().unwrap_or_else(PoisonError::into_inner) = Some(fresh);
    scan
}

/// Depth-first walk in sorted order, so two scans of an unchanged vault return
/// the same prefix when the page cap bites.
fn walk(
    wiki_root: &Path,
    dir: &Path,
    depth: usize,
    previous: &mut BTreeMap<PathBuf, CachedPage>,
    pending: &mut Vec<PendingPage>,
    capped: &mut bool,
) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    children.sort();
    for child in children {
        if *capped {
            return;
        }
        // `symlink_metadata` rather than `metadata`: a symlink out of the vault
        // is not vault knowledge, and following one is how a walk leaves the
        // tree it was asked about.
        let Ok(metadata) = fs::symlink_metadata(&child) else {
            continue;
        };
        if metadata.is_dir() {
            walk(wiki_root, &child, depth + 1, previous, pending, capped);
            continue;
        }
        if !metadata.is_file() || !is_markdown(&child) {
            continue;
        }
        // Counted only against pages, and only once the file is known to be
        // one: a vault whose last file is an attachment is not a capped vault.
        if pending.len() >= MAX_INDEXED_PAGES {
            *capped = true;
            return;
        }
        if let Some(page) = index_page(wiki_root, &child, &metadata, previous) {
            pending.push(page);
        }
    }
}

fn stamp(metadata: &fs::Metadata) -> (SystemTime, u64) {
    (
        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        metadata.len(),
    )
}

/// Whether a file is a page rather than an attachment.
fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(PAGE_EXTENSION))
}

/// Index one page, reusing the last reading when the file has not moved.
///
/// Both branches yield the same three facts, because the relations still have
/// to be resolved against this scan's vault: what the cache saves is the read
/// and the tokenization, not the linking.
fn index_page(
    wiki_root: &Path,
    path: &Path,
    metadata: &fs::Metadata,
    previous: &mut BTreeMap<PathBuf, CachedPage>,
) -> Option<PendingPage> {
    let slug = page_slug(wiki_root, path)?;
    let stamp = stamp(metadata);
    if let Some(cached) = previous.remove(path) {
        if cached.stamp == stamp {
            return Some(PendingPage {
                path: path.to_path_buf(),
                stamp,
                slug,
                base_summary: cached.base_summary,
                written: cached.written,
                ingested_at: cached.ingested_at,
                source: PendingSource::Cached(cached.page),
            });
        }
    }
    let text = read_capped(path)?;
    let (frontmatter, body) = split_frontmatter(&text);
    let title = frontmatter_field(frontmatter, "title")
        .map_or_else(|| page_title(&slug), ToOwned::to_owned);
    let tags = frontmatter_field(frontmatter, "tags").map(clean_tag_list);
    let base_summary = summary_for(&slug, &title, tags.as_deref());
    let written = written_relations(frontmatter, body);
    let ingested_at = frontmatter_field(frontmatter, INGESTED_AT_FIELD)
        .map(|value| unquote(value.trim()).trim().to_string())
        .filter(|value| !value.is_empty());
    let entry = MemoryEntry {
        summary: base_summary.clone(),
        path: path.display().to_string(),
        slug: slug.clone(),
        // `slug`/`path`/`summary` are the pointer the recall section renders;
        // the body is handed over separately and never stored.
    };
    Some(PendingPage {
        path: path.to_path_buf(),
        stamp,
        slug,
        base_summary,
        written,
        ingested_at,
        source: PendingSource::Fresh {
            entry,
            body: body.to_string(),
        },
    })
}

/// Every relation a page declares, targets as written: body `[[links]]` as
/// [`RelationKind::Mentions`] in written order, then the typed frontmatter keys
/// in [`RelationKind::TYPED`] order. [`MAX_PAGE_RELATIONS`] bounds the two
/// together, so a runaway body cannot be joined by a runaway frontmatter.
fn written_relations(frontmatter: &str, body: &str) -> Vec<(RelationKind, String)> {
    let mut relations: Vec<(RelationKind, String)> = body_links(body)
        .into_iter()
        .map(|target| (RelationKind::Mentions, target))
        .collect();
    for kind in RelationKind::TYPED {
        let Some(value) = frontmatter_list_field(frontmatter, kind.as_str()) else {
            continue;
        };
        for item in split_relation_list(&value) {
            if relations.len() >= MAX_PAGE_RELATIONS {
                break;
            }
            if let Some(target) = relation_target(&item) {
                relations.push((kind, target));
            }
        }
    }
    relations.truncate(MAX_PAGE_RELATIONS);
    relations
}

/// Resolve every written target against the pages this scan found, and build
/// the reverse index.
///
/// A target may name a page the walk had not reached yet, so this cannot run
/// during the walk. Resolution order — exact id, file stem, folded stem — and
/// the sorted walk together make "which page did `[[design]]` mean" the same
/// answer twice.
fn resolve(pending: Vec<PendingPage>, capped: bool) -> (CorpusScan, BTreeMap<PathBuf, CachedPage>) {
    let mut by_id: BTreeSet<&str> = BTreeSet::new();
    let mut by_stem: BTreeMap<&str, &str> = BTreeMap::new();
    let mut by_folded: BTreeMap<String, &str> = BTreeMap::new();
    for page in &pending {
        by_id.insert(page.slug.as_str());
        let stem = page.slug.rsplit('/').next().unwrap_or(&page.slug);
        // First by sorted walk order wins, so two pages sharing a file name in
        // different folders resolve to the same one every scan.
        by_stem.entry(stem).or_insert(&page.slug);
        by_folded.entry(stem.to_lowercase()).or_insert(&page.slug);
    }

    let mut resolved: Vec<Vec<Relation>> = Vec::with_capacity(pending.len());
    let mut incoming: BTreeMap<String, Vec<(RelationKind, String)>> = BTreeMap::new();
    for page in &pending {
        let mut relations = Vec::with_capacity(page.written.len());
        for (kind, target) in &page.written {
            let hit = resolve_target(target, &by_id, &by_stem, &by_folded);
            // A page linking to itself is not a neighbour of itself.
            let hit = hit.filter(|slug| *slug != page.slug.as_str());
            match hit {
                Some(slug) => {
                    incoming
                        .entry(slug.to_string())
                        .or_default()
                        .push((*kind, page.slug.clone()));
                    relations.push(Relation {
                        kind: *kind,
                        target: slug.to_string(),
                        resolved: true,
                    });
                }
                None => relations.push(Relation {
                    kind: *kind,
                    target: target.clone(),
                    resolved: false,
                }),
            }
        }
        resolved.push(relations);
    }

    let mut pages = Vec::with_capacity(pending.len());
    let mut fresh = BTreeMap::new();
    for (page, relations) in pending.into_iter().zip(resolved) {
        let summary = summary_with_relations(&page.base_summary, &relations);
        let indexed = match page.source {
            PendingSource::Fresh { mut entry, body } => {
                entry.summary = summary;
                Arc::new(
                    IndexedCorpusPage::new(entry, &body, relations)
                        .ingested(page.ingested_at.clone()),
                )
            }
            // The `Arc` is shared with every retriever built from an earlier
            // scan, so a page whose links moved is re-wrapped rather than
            // mutated — and left alone, tokens and all, when they did not.
            PendingSource::Cached(cached) => {
                if cached.relations() == relations && cached.entry().summary == summary {
                    cached
                } else {
                    Arc::new(cached.relinked(summary, relations))
                }
            }
        };
        fresh.insert(
            page.path,
            CachedPage {
                stamp: page.stamp,
                page: Arc::clone(&indexed),
                base_summary: page.base_summary,
                written: page.written,
                ingested_at: page.ingested_at,
            },
        );
        pages.push(indexed);
    }
    (
        CorpusScan {
            pages,
            incoming,
            capped,
        },
        fresh,
    )
}

/// One target's slug: exact `wiki/...` id, else file stem, else folded stem.
fn resolve_target<'a>(
    target: &str,
    by_id: &BTreeSet<&'a str>,
    by_stem: &BTreeMap<&'a str, &'a str>,
    by_folded: &BTreeMap<String, &'a str>,
) -> Option<&'a str> {
    let trimmed = target.trim().trim_start_matches("./");
    let cleaned = trimmed
        .strip_suffix(PAGE_EXTENSION)
        .and_then(|held| held.strip_suffix('.'))
        .unwrap_or(trimmed);
    if let Some(id) = by_id.get(cleaned) {
        return Some(id);
    }
    by_stem
        .get(cleaned)
        .or_else(|| by_folded.get(&cleaned.to_lowercase()))
        .copied()
}

/// `wiki/<relative path without extension>` — the wikilink target a person
/// would type, and a slug unique within the vault.
///
/// Refuses a relative path whose characters would break the one-line shape the
/// recall section renders and re-parses (`- [slug](path) — summary`), rather
/// than emitting a line the dedup pass would misread.
fn page_slug(wiki_root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(wiki_root).ok()?;
    let without_extension = relative.with_extension("");
    let mut slug = String::from(WIKI_DIR);
    for component in without_extension.components() {
        let std::path::Component::Normal(name) = component else {
            return None;
        };
        let name = name.to_str()?;
        if name.is_empty() || name.contains(['[', ']', '(', ')', '\n', '\r']) {
            return None;
        }
        slug.push('/');
        slug.push_str(name);
    }
    (slug.len() > WIKI_DIR.len()).then_some(slug)
}

/// Title of last resort: the file name, with separators read as spaces.
fn page_title(slug: &str) -> String {
    slug.rsplit('/')
        .next()
        .unwrap_or(slug)
        .replace(['-', '_'], " ")
}

/// The rendered pointer summary. It opens with the page's own wikilink so the
/// model can see, in the recall section itself, that this line came from the
/// vault and how to cite it.
fn summary_for(slug: &str, title: &str, tags: Option<&str>) -> String {
    let mut summary = format!("[[{slug}]] — {title}");
    if let Some(tags) = tags.filter(|tags| !tags.is_empty()) {
        summary.push_str(" · tags: ");
        summary.push_str(tags);
    }
    summary
}

/// The pointer summary plus up to [`MAX_SUMMARY_RELATIONS`] of the page's
/// company — typed relations first, then mentions, so the strongest claim a
/// page makes about a neighbour is the one that gets the room.
///
/// A suffix is only appended while the whole summary stays inside
/// [`CORPUS_SUMMARY_MAX_BYTES`], which sits far enough below the renderer's own
/// cap that its truncation can never land inside a `[[…]]` and leave the model
/// a wikilink it cannot follow.
fn summary_with_relations(base: &str, relations: &[Relation]) -> String {
    let mut summary = String::from(base);
    let mut shown = 0;
    let typed = relations
        .iter()
        .filter(|relation| relation.kind != RelationKind::Mentions);
    let mentions = relations
        .iter()
        .filter(|relation| relation.kind == RelationKind::Mentions);
    let mut seen: BTreeSet<(RelationKind, &str)> = BTreeSet::new();
    for relation in typed.chain(mentions) {
        if shown >= MAX_SUMMARY_RELATIONS {
            break;
        }
        if !seen.insert((relation.kind, relation.target.as_str())) {
            continue;
        }
        let suffix = format!(" · {}: [[{}]]", relation.kind.as_str(), relation.target);
        if summary.len() + suffix.len() > CORPUS_SUMMARY_MAX_BYTES {
            break;
        }
        summary.push_str(&suffix);
        shown += 1;
    }
    summary
}

/// Read at most [`MAX_PAGE_INDEX_BYTES`], normalizing line endings so a
/// CRLF vault tokenizes identically to an LF one.
fn read_capped(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(MAX_PAGE_INDEX_BYTES.min(4096));
    file.by_ref()
        .take(MAX_PAGE_INDEX_BYTES as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).replace("\r\n", "\n"))
}

/// Split a leading `---` YAML frontmatter block from the body. An unterminated
/// block is treated as body, so a page that merely opens with a horizontal rule
/// is still indexed.
fn split_frontmatter(text: &str) -> (&str, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return ("", text);
    };
    match rest.find("\n---") {
        Some(end) => {
            let body = rest[end + "\n---".len()..].trim_start_matches(['\r', '\n']);
            (&rest[..end], body)
        }
        None => ("", text),
    }
}

/// One scalar frontmatter field, by key. Only the flat `key: value` form is
/// read — a vault page's `title` and `tags` are written that way, and a YAML
/// parser for two fields would be a dependency this module does not need.
fn frontmatter_field<'a>(frontmatter: &'a str, key: &str) -> Option<&'a str> {
    frontmatter.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| value.trim())
    })
}

/// One relation field, by key, flattened to the comma-separated form
/// [`split_relation_list`] reads.
///
/// Beyond [`frontmatter_field`]'s flat `key: value` this also reads the block
/// list — `key:` followed by indented `- item` lines — because that is the
/// shape a person writes several relations in, and a relation key that only
/// worked on one line would silently drop the other half of the vault's links.
fn frontmatter_list_field(frontmatter: &str, key: &str) -> Option<String> {
    let mut lines = frontmatter.lines();
    let value = loop {
        let line = lines.next()?;
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == key {
            break value.trim();
        }
    };
    if !value.is_empty() {
        return Some(value.to_string());
    }
    let mut items = Vec::new();
    for line in lines {
        if !line.starts_with([' ', '\t']) {
            break;
        }
        let Some(item) = line.trim_start().strip_prefix("- ") else {
            break;
        };
        items.push(item.trim().to_string());
    }
    (!items.is_empty()).then(|| items.join(","))
}

/// One relation value's items, before normalisation.
///
/// A `[[link]]` is never split down the middle and its own brackets are never
/// eaten by the flow-list strip: `implements: [[wiki/adr/003]]` is one link
/// rather than a list holding a list, while `depends_on: [ [[wiki/a]], b ]` is
/// two items. The two are told apart by what stripping one bracket pair would
/// leave — a flow list of links still opens with `[[`, a lone link does not.
fn split_relation_list(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let inner = match trimmed
        .strip_prefix('[')
        .and_then(|held| held.strip_suffix(']'))
    {
        Some(held) if !trimmed.starts_with("[[") || held.trim_start().starts_with("[[") => held,
        _ => trimmed,
    };
    let chars: Vec<char> = inner.chars().collect();
    let mut items = Vec::new();
    let mut current = String::new();
    let mut at = 0;
    while at < chars.len() {
        if let Some(end) = link_span(&chars, at) {
            current.extend(&chars[at..=end + 1]);
            at = end + 2;
            continue;
        }
        if chars[at] == '[' && at + 1 < chars.len() && chars[at + 1] == '[' {
            // An unclosed `[[` is the rest of the value, as a renderer reads it.
            current.extend(&chars[at..]);
            break;
        }
        if chars[at] == ',' {
            items.push(std::mem::take(&mut current));
        } else {
            current.push(chars[at]);
        }
        at += 1;
    }
    items.push(current);
    items
}

/// One declared relation's target: quotes off, one `[[…]]` pair off, then the
/// same `Target|alias` and `Target#heading` rule a body link answers to.
fn relation_target(item: &str) -> Option<String> {
    let held = unquote(item.trim()).trim();
    let bare = held
        .strip_prefix("[[")
        .and_then(|inner| inner.strip_suffix("]]"))
        .unwrap_or(held);
    link_target(bare)
}

fn unquote(value: &str) -> &str {
    for mark in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(mark)
            .and_then(|held| held.strip_suffix(mark))
        {
            return inner;
        }
    }
    value
}

/// Every `[[target]]` in the body, in the order it was written.
///
/// Fenced blocks and inline code spans are skipped, the same rule the window's
/// graph scanner applies: a page documenting this very syntax must not be read
/// as linking to its own example. `![[embed]]` counts — an embed is a relation
/// a person can see on the page.
fn body_links(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(mark) = fence_mark(trimmed) {
            match fence {
                // A fence closes on its own character at its own width or more.
                Some((open, width)) if open == mark.0 && mark.1 >= width => fence = None,
                Some(_) => {}
                None => fence = Some(mark),
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        scan_line_links(line, &mut links);
        if links.len() >= MAX_PAGE_RELATIONS {
            links.truncate(MAX_PAGE_RELATIONS);
            break;
        }
    }
    links
}

/// A backtick or tilde fence (three or more of one kind) and its width, or nothing.
fn fence_mark(trimmed: &str) -> Option<(char, usize)> {
    for mark in ['`', '~'] {
        let width = trimmed.chars().take_while(|held| *held == mark).count();
        if width >= 3 {
            return Some((mark, width));
        }
    }
    None
}

fn scan_line_links(line: &str, into: &mut Vec<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        if chars[at] == '`' {
            // An inline span closes on a backtick run of the same width; an
            // unclosed one swallows the rest of the line, which is what a
            // renderer does with it too.
            let width = chars[at..].iter().take_while(|held| **held == '`').count();
            at += width;
            let mut run = 0;
            while at < chars.len() {
                run = if chars[at] == '`' { run + 1 } else { 0 };
                at += 1;
                if run == width {
                    break;
                }
            }
            continue;
        }
        if let Some(end) = link_span(&chars, at) {
            let inner: String = chars[at + 2..end].iter().collect();
            if let Some(target) = link_target(&inner) {
                into.push(target);
                if into.len() >= MAX_PAGE_RELATIONS {
                    return;
                }
            }
            at = end + 2;
            continue;
        }
        at += 1;
    }
}

/// Index of the `]]` closing a `[[` opened at `at`, or nothing when `at` opens
/// no link or the link is never closed.
fn link_span(chars: &[char], at: usize) -> Option<usize> {
    if chars[at] != '[' || chars.get(at + 1) != Some(&'[') {
        return None;
    }
    let mut end = at + 2;
    while end + 1 < chars.len() && !(chars[end] == ']' && chars[end + 1] == ']') {
        end += 1;
    }
    (end + 1 < chars.len()).then_some(end)
}

/// `Target`, `Target|alias`, `Target#heading` and `Target#heading|alias`.
///
/// A bare `[[#heading]]` answers nothing: it points inside the page it is
/// written on, and a page is not its own neighbour.
fn link_target(inner: &str) -> Option<String> {
    let before_alias = inner.split('|').next().unwrap_or(inner);
    let target = before_alias
        .split('#')
        .next()
        .unwrap_or(before_alias)
        .trim();
    (!target.is_empty()).then(|| target.to_string())
}

/// `[a, b]` and `a, b` both read as `a, b`.
fn clean_tag_list(raw: &str) -> String {
    raw.trim_matches(['[', ']'])
        .split(',')
        .map(|tag| tag.trim().trim_matches(['"', '\'']))
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `candidate` — already canonicalized — is a page of the configured
/// vault, and may therefore be opened to render a recall snippet.
///
/// Resolved from the environment on every call, like the memory stores' own
/// authorization: it runs at most twice per turn (the section renders at most
/// two snippets), and a memoized answer would let a vault change silently
/// outlive the session that made it.
#[must_use]
pub fn authorizes(candidate: &Path) -> bool {
    let Some(vault) = SecondBrain::from_env() else {
        return false;
    };
    let Ok(wiki) = fs::canonicalize(vault.wiki_dir()) else {
        return false;
    };
    candidate.starts_with(&wiki)
}
