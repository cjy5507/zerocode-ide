//! `@` file search — Codex 0.155.1's `codex-rs/file-search/src/lib.rs`,
//! carried over whole, with zo's own ranking laid on top of it.
//!
//! The engine is the same pair of workers Codex runs: an `ignore` walker that
//! honours `.gitignore`, git's global and per-repo excludes and `.ignore` files
//! (`require_git(true)`, so a parent `~/.gitignore` cannot swallow a tree that
//! is not a repository), feeding paths into a [`nucleo`] matcher whose pattern
//! is updated **incrementally** on every keystroke rather than re-walking. A
//! session is one walk; `update_query` is cheap; dropping the session stops
//! both threads. Every constant keeps Codex's name and value and says so.
//!
//! What zo adds, and Codex has not:
//!
//! * **More than one root.** The repository is the first [`SearchRoot`]; the
//!   second brain's `wiki/` is the second, shown and inserted as `wiki/<page>`
//!   ([`SearchRoot::pages`]). Codex's `search_directories: Vec<PathBuf>` already
//!   allowed several roots — the prefix is the one addition.
//! * **A rerank.** [`RankBoost`] adds to the matcher's score before the top-N
//!   is cut, so a file this session read or edited, or one changed a moment
//!   ago, stands above an equally good match nobody has touched
//!   ([`SessionBoost`]). With no boost the order is nucleo's — Codex's.
//!
//! nucleo 0.5.0 (crates.io) does not expose `Snapshot::matches()` with scores
//! the way Codex's git pin does; the score of a matched item is taken from
//! `Pattern::indices`, which is the same number the worker ranked it by and
//! which Codex calls anyway for highlighting.

use std::collections::HashSet;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Injector, Matcher, Nucleo, Utf32String};

/// Codex `FileSearchOptions::default().limit` — how many matches a snapshot
/// carries.
pub const DEFAULT_LIMIT: NonZero<usize> = NonZero::new(20).unwrap();
/// Codex `FileSearchOptions::default().threads` — walker and matcher threads.
pub const DEFAULT_THREADS: NonZero<usize> = NonZero::new(2).unwrap();
/// Codex `matcher_worker::TICK_TIMEOUT_MS` — how long one nucleo tick waits
/// for the worker, and the debounce between two notifications.
pub const TICK_TIMEOUT_MS: u64 = 10;
/// Codex `walker_worker::CHECK_INTERVAL` — entries between two looks at the
/// cancel flag.
pub const CHECK_INTERVAL: usize = 1024;
/// Codex `matcher_worker`'s `default(Duration::from_millis(100))` select arm —
/// how often an idle matcher looks at the cancel flag.
pub const IDLE_POLL: Duration = Duration::from_millis(100);
/// zo: how many times `limit` the rerank looks at before it cuts the top-N.
/// A boosted file the matcher ranked 100th would be invisible with a window
/// of exactly `limit`; eight pages of it is 160 items, and every one is
/// scored once per snapshot — microseconds.
pub const RERANK_WINDOW: usize = 8;
/// zo: the walker never enters the repository's own object store. `ignore`
/// walks hidden entries when asked to (`hidden(false)`, as Codex asks), and
/// nothing else keeps `.git/objects/..` out of a hash-shaped query.
pub const ALWAYS_EXCLUDED: [&str; 1] = [".git"];

/// A single match result returned from the search (Codex `FileMatch`).
///
/// * `score` – relevance score returned by `nucleo`, plus a [`RankBoost`]
///   when one is configured.
/// * `path` – what the popup shows and inserts: the root's prefix followed by
///   the path relative to that root.
/// * `full_path` – where the entry is on disk.
/// * `indices` – character indices of `path` that matched, sorted and unique,
///   filled only with `compute_indices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatch {
    pub score: u32,
    pub path: PathBuf,
    pub match_type: MatchType,
    pub root: PathBuf,
    pub full_path: PathBuf,
    pub indices: Option<Vec<u32>>,
}

impl FileMatch {
    #[must_use]
    pub fn full_path(&self) -> &Path {
        &self.full_path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchType {
    File,
    Directory,
}

/// One directory the walk starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRoot {
    pub dir: PathBuf,
    /// Put before every relative path from this root — what the person types,
    /// sees and inserts. Empty for the repository; `wiki/` for the vault.
    pub prefix: String,
    /// Keep only pages ([`crate::second_brain::PAGE_SUFFIX`]) — a vault root.
    pub pages_only: bool,
}

impl SearchRoot {
    /// The repository: relative paths as they are, every file.
    #[must_use]
    pub fn repo(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            prefix: String::new(),
            pages_only: false,
        }
    }

    /// A page directory shown under `prefix` — the second brain's `wiki/`.
    #[must_use]
    pub fn pages(dir: impl Into<PathBuf>, prefix: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            prefix: prefix.into(),
            pages_only: true,
        }
    }
}

/// Carries what the walker saw so matched paths are not stat'ed again.
struct IndexedEntry {
    full_path: PathBuf,
    display: Arc<str>,
    root: usize,
    match_type: MatchType,
}

#[derive(Debug)]
pub struct FileSearchResults {
    pub matches: Vec<FileMatch>,
    pub total_match_count: usize,
}

/// Codex `FileSearchSnapshot`, plus zo's `settled`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileSearchSnapshot {
    pub query: String,
    pub matches: Vec<FileMatch>,
    pub total_match_count: usize,
    pub scanned_file_count: usize,
    pub walk_complete: bool,
    /// zo: the walk is over and the matcher idle — this top-N is the final
    /// answer for `query`. Before that, an empty `matches` means "nothing
    /// found yet", which is not the same as nothing.
    pub settled: bool,
}

/// Something that lifts a candidate above its matcher score.
pub trait RankBoost: Send + Sync {
    /// Added to the matcher's score for the entry at `full_path`.
    fn boost(&self, full_path: &Path) -> u32;
}

/// Codex `FileSearchOptions`, plus zo's `boost`.
#[derive(Clone)]
pub struct FileSearchOptions {
    pub limit: NonZero<usize>,
    pub exclude: Vec<String>,
    pub threads: NonZero<usize>,
    pub compute_indices: bool,
    /// Toggle ignore-file processing in the walker. When enabled, `.gitignore`
    /// files are scoped by `WalkBuilder::require_git(true)`, so they are
    /// honoured only inside a git repository. When disabled, the walker turns
    /// off `.gitignore`, git-global/exclude rules, `.ignore`, and parent
    /// scanning.
    pub respect_gitignore: bool,
    /// zo: the rerank. `None` keeps nucleo's order — what Codex shows.
    pub boost: Option<Arc<dyn RankBoost>>,
}

impl std::fmt::Debug for FileSearchOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileSearchOptions")
            .field("limit", &self.limit)
            .field("exclude", &self.exclude)
            .field("threads", &self.threads)
            .field("compute_indices", &self.compute_indices)
            .field("respect_gitignore", &self.respect_gitignore)
            .field("boost", &self.boost.is_some())
            .finish()
    }
}

impl Default for FileSearchOptions {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            exclude: Vec::new(),
            threads: DEFAULT_THREADS,
            compute_indices: false,
            respect_gitignore: true,
            boost: None,
        }
    }
}

/// Codex `SessionReporter`.
pub trait SessionReporter: Send + Sync + 'static {
    /// Called when the debounced top-N changes.
    fn on_update(&self, snapshot: &FileSearchSnapshot);
    /// Called when the session becomes idle or is cancelled. Guaranteed to be
    /// called at least once per `update_query`.
    fn on_complete(&self);
}

/// Codex `FileSearchSession` — one walk, many queries.
pub struct FileSearchSession {
    inner: Arc<SessionInner>,
}

impl FileSearchSession {
    /// Update the query. Cheap relative to re-walking.
    pub fn update_query(&self, pattern_text: &str) {
        let _ = self
            .inner
            .work_tx
            .send(WorkSignal::QueryUpdated(pattern_text.to_string()));
    }
}

impl Drop for FileSearchSession {
    fn drop(&mut self) {
        self.inner.shutdown.store(true, Ordering::Relaxed);
        let _ = self.inner.work_tx.send(WorkSignal::Shutdown);
    }
}

/// Codex `create_session`.
pub fn create_session(
    roots: Vec<SearchRoot>,
    options: FileSearchOptions,
    reporter: Arc<dyn SessionReporter>,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> std::io::Result<FileSearchSession> {
    let FileSearchOptions {
        limit,
        exclude,
        threads,
        compute_indices,
        respect_gitignore,
        boost,
    } = options;
    let Some(primary) = roots.first() else {
        return Err(std::io::Error::other("at least one search root is required"));
    };
    let override_matcher = build_override_matcher(&primary.dir, &exclude)?;
    let (work_tx, work_rx) = mpsc::channel();

    let notify_tx = work_tx.clone();
    let notify = Arc::new(move || {
        let _ = notify_tx.send(WorkSignal::NucleoNotify);
    });
    let nucleo = Nucleo::new(Config::DEFAULT.match_paths(), notify, Some(threads.get()), 1);
    let injector = nucleo.injector();
    let cancelled = cancel_flag.unwrap_or_else(|| Arc::new(AtomicBool::new(false)));

    let inner = Arc::new(SessionInner {
        roots,
        limit: limit.get(),
        threads: threads.get(),
        compute_indices,
        respect_gitignore,
        boost,
        cancelled,
        shutdown: Arc::new(AtomicBool::new(false)),
        reporter,
        work_tx,
    });

    let matcher_inner = Arc::clone(&inner);
    thread::Builder::new()
        .name("zo-file-search-matcher".to_string())
        .spawn(move || matcher_worker(&matcher_inner, &work_rx, nucleo))?;
    let walker_inner = Arc::clone(&inner);
    thread::Builder::new()
        .name("zo-file-search-walker".to_string())
        .spawn(move || walker_worker(&walker_inner, override_matcher, &injector))?;

    Ok(FileSearchSession { inner })
}

/// Codex `run` — one query, answered when the walk is complete and the
/// matcher idle. The worker threads check `cancel_flag` periodically.
pub fn run(
    pattern_text: &str,
    roots: Vec<SearchRoot>,
    options: FileSearchOptions,
    cancel_flag: Option<Arc<AtomicBool>>,
) -> std::io::Result<FileSearchResults> {
    let reporter = Arc::new(RunReporter::default());
    let session = create_session(roots, options, Arc::clone(&reporter) as Arc<dyn SessionReporter>, cancel_flag)?;
    session.update_query(pattern_text);
    let snapshot = reporter.wait_for_complete();
    Ok(FileSearchResults {
        matches: snapshot.matches,
        total_match_count: snapshot.total_match_count,
    })
}

struct SessionInner {
    roots: Vec<SearchRoot>,
    limit: usize,
    threads: usize,
    compute_indices: bool,
    respect_gitignore: bool,
    boost: Option<Arc<dyn RankBoost>>,
    cancelled: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    reporter: Arc<dyn SessionReporter>,
    work_tx: Sender<WorkSignal>,
}

impl SessionInner {
    fn stopping(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) || self.shutdown.load(Ordering::Relaxed)
    }
}

/// Codex `WorkSignal`.
enum WorkSignal {
    QueryUpdated(String),
    NucleoNotify,
    WalkComplete,
    Shutdown,
}

/// Codex `build_override_matcher`, with [`ALWAYS_EXCLUDED`] folded in.
fn build_override_matcher(
    search_directory: &Path,
    exclude: &[String],
) -> std::io::Result<Option<ignore::overrides::Override>> {
    if exclude.is_empty() && ALWAYS_EXCLUDED.is_empty() {
        return Ok(None);
    }
    let mut override_builder = OverrideBuilder::new(search_directory);
    for exclude in ALWAYS_EXCLUDED.iter().copied().chain(exclude.iter().map(String::as_str)) {
        override_builder
            .add(&format!("!{exclude}"))
            .map_err(std::io::Error::other)?;
    }
    let matcher = override_builder.build().map_err(std::io::Error::other)?;
    Ok(Some(matcher))
}

/// Codex `get_file_path` — the deepest root the entry is under, and the path
/// relative to it.
fn root_of<'a>(path: &'a Path, roots: &[SearchRoot]) -> Option<(usize, &'a Path)> {
    let mut best: Option<(usize, &Path, usize)> = None;
    for (index, root) in roots.iter().enumerate() {
        if let Ok(relative) = path.strip_prefix(&root.dir) {
            let depth = root.dir.components().count();
            if best.is_none_or(|(_, _, best_depth)| depth > best_depth) {
                best = Some((index, relative, depth));
            }
        }
    }
    best.map(|(index, relative, _)| (index, relative))
}

/// Walks the search roots and feeds discovered paths into `nucleo` via the
/// injector (Codex `walker_worker`).
///
/// `require_git(true)` matches git's own ignore semantics: git never reads
/// `.gitignore` files from directories above the repository root. Without
/// this flag, the `ignore` crate reads `.gitignore` files from *all* ancestor
/// directories, allowing a broad parent ignore (e.g. `~/.gitignore` holding
/// `*`) to silently suppress every file in the walk.
fn walker_worker(
    inner: &Arc<SessionInner>,
    override_matcher: Option<ignore::overrides::Override>,
    injector: &Injector<IndexedEntry>,
) {
    let Some(first_root) = inner.roots.first() else {
        let _ = inner.work_tx.send(WorkSignal::WalkComplete);
        return;
    };
    let mut walk_builder = WalkBuilder::new(&first_root.dir);
    for root in inner.roots.iter().skip(1) {
        walk_builder.add(&root.dir);
    }
    walk_builder
        .threads(inner.threads)
        // Allow hidden entries.
        .hidden(false)
        // Follow symlinks to search their contents.
        .follow_links(true)
        // Keep ignore behaviour aligned with git repositories: only apply
        // gitignore rules when a git context exists.
        .require_git(true);
    if !inner.respect_gitignore {
        walk_builder
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .ignore(false)
            .parents(false);
    }
    if let Some(override_matcher) = override_matcher {
        walk_builder.overrides(override_matcher);
    }

    let walker = walk_builder.build_parallel();
    walker.run(|| {
        let mut seen = 0usize;
        let inner = Arc::clone(inner);
        let injector = injector.clone();
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return ignore::WalkState::Continue;
            };
            let path = entry.path();
            if let Some((root_index, relative)) = root_of(path, &inner.roots) {
                let root = &inner.roots[root_index];
                let match_type = match entry.file_type() {
                    Some(file_type) if file_type.is_dir() => MatchType::Directory,
                    _ => MatchType::File,
                };
                let wanted = !relative.as_os_str().is_empty()
                    && (!root.pages_only
                        || (match_type == MatchType::File && is_page(relative)));
                if wanted {
                    if let Some(relative) = relative.to_str() {
                        let display: Arc<str> = Arc::from(format!("{}{relative}", root.prefix));
                        injector.push(
                            IndexedEntry {
                                full_path: path.to_path_buf(),
                                display: Arc::clone(&display),
                                root: root_index,
                                match_type,
                            },
                            |_, columns| {
                                columns[0] = Utf32String::from(display.as_ref());
                            },
                        );
                    }
                }
            }
            seen += 1;
            if seen >= CHECK_INTERVAL {
                if inner.stopping() {
                    return ignore::WalkState::Quit;
                }
                seen = 0;
            }
            ignore::WalkState::Continue
        })
    });
    let _ = inner.work_tx.send(WorkSignal::WalkComplete);
}

fn is_page(relative: &Path) -> bool {
    relative
        .to_str()
        .is_some_and(|name| name.ends_with(crate::second_brain::PAGE_SUFFIX))
}

/// Codex `matcher_worker`: queries in, debounced snapshots out.
///
/// Codex multiplexes its signals and timers with `crossbeam_channel::select!`;
/// this is the same machine on `std::sync::mpsc` with a deadline — a query
/// update publishes at once, a nucleo notification after [`TICK_TIMEOUT_MS`],
/// and an idle worker looks at the cancel flag every [`IDLE_POLL`].
fn matcher_worker(
    inner: &Arc<SessionInner>,
    work_rx: &Receiver<WorkSignal>,
    mut nucleo: Nucleo<IndexedEntry>,
) {
    let config = Config::DEFAULT.match_paths();
    let mut matcher = Matcher::new(config);
    let mut last_query = String::new();
    let mut next_notify: Option<Instant> = None;
    let mut walk_complete = false;
    // zo: the settled state (walk over, matcher idle) is reported once even
    // when the top-N did not move — a reporter that holds back an empty
    // top-N while the search still runs (the popup's `loading...`) needs the
    // snapshot that says it is over. Any new signal unsettles it again.
    let mut settled_reported = false;

    loop {
        let timeout = next_notify.map_or(IDLE_POLL, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        });
        match work_rx.recv_timeout(timeout) {
            Ok(WorkSignal::QueryUpdated(query)) => {
                let append = query.starts_with(&last_query);
                nucleo.pattern.reparse(
                    0,
                    &query,
                    CaseMatching::Ignore,
                    Normalization::Smart,
                    append,
                );
                last_query = query;
                settled_reported = false;
                next_notify = Some(Instant::now());
            }
            Ok(WorkSignal::NucleoNotify) => {
                settled_reported = false;
                if next_notify.is_none() {
                    next_notify = Some(Instant::now() + Duration::from_millis(TICK_TIMEOUT_MS));
                }
            }
            Ok(WorkSignal::WalkComplete) => {
                walk_complete = true;
                settled_reported = false;
                if next_notify.is_none() {
                    next_notify = Some(Instant::now());
                }
            }
            Ok(WorkSignal::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if next_notify.is_some_and(|deadline| Instant::now() >= deadline) {
            next_notify = None;
            let status = nucleo.tick(TICK_TIMEOUT_MS);
            let settled = walk_complete && !status.running;
            if status.changed || (settled && !settled_reported) {
                let snapshot = build_snapshot(
                    inner,
                    &nucleo,
                    &mut matcher,
                    &last_query,
                    walk_complete,
                    settled,
                );
                settled_reported = settled;
                inner.reporter.on_update(&snapshot);
            }
            if !status.running && walk_complete {
                inner.reporter.on_complete();
            }
        }
        if inner.stopping() {
            break;
        }
    }
    // Cancelled or shut down: the reporter always hears the end.
    inner.reporter.on_complete();
}

/// The top-N of the matcher's current order, scored and (with a boost)
/// reranked over the [`RERANK_WINDOW`].
fn build_snapshot(
    inner: &SessionInner,
    nucleo: &Nucleo<IndexedEntry>,
    scorer: &mut Matcher,
    query: &str,
    walk_complete: bool,
    settled: bool,
) -> FileSearchSnapshot {
    let snapshot = nucleo.snapshot();
    let matched_count = snapshot.matched_item_count() as usize;
    let window = if inner.boost.is_some() {
        inner.limit.saturating_mul(RERANK_WINDOW)
    } else {
        inner.limit
    }
    .min(matched_count);
    let pattern = snapshot.pattern().column_pattern(0);
    let mut indices = Vec::<u32>::new();
    let mut matches: Vec<FileMatch> = (0..window)
        .filter_map(|n| {
            let item = snapshot.get_matched_item(u32::try_from(n).ok()?)?;
            indices.clear();
            let haystack = item.matcher_columns[0].slice(..);
            let score = pattern.indices(haystack, scorer, &mut indices)?;
            let boost = inner
                .boost
                .as_ref()
                .map_or(0, |boost| boost.boost(&item.data.full_path));
            let indices = inner.compute_indices.then(|| {
                let mut sorted = indices.clone();
                sorted.sort_unstable();
                sorted.dedup();
                sorted
            });
            Some(FileMatch {
                score: score.saturating_add(boost),
                path: PathBuf::from(item.data.display.as_ref()),
                match_type: item.data.match_type,
                root: inner.roots[item.data.root].dir.clone(),
                full_path: item.data.full_path.clone(),
                indices,
            })
        })
        .collect();
    if inner.boost.is_some() {
        // Stable: two entries the boost left level keep nucleo's order.
        matches.sort_by_key(|entry| std::cmp::Reverse(entry.score));
        matches.truncate(inner.limit);
    }
    FileSearchSnapshot {
        query: query.to_string(),
        matches,
        total_match_count: matched_count,
        scanned_file_count: snapshot.item_count() as usize,
        walk_complete,
        settled,
    }
}

/// Codex `RunReporter` — the blocking one-shot's reporter.
#[derive(Default)]
struct RunReporter {
    snapshot: RwLock<FileSearchSnapshot>,
    completed: (Condvar, Mutex<bool>),
}

impl SessionReporter for RunReporter {
    fn on_update(&self, snapshot: &FileSearchSnapshot) {
        let mut guard = self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = snapshot.clone();
    }

    fn on_complete(&self) {
        let (condvar, mutex) = &self.completed;
        let mut completed = mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *completed = true;
        condvar.notify_all();
    }
}

impl RunReporter {
    fn wait_for_complete(&self) -> FileSearchSnapshot {
        let (condvar, mutex) = &self.completed;
        let mut completed = mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*completed {
            completed = condvar
                .wait(completed)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        self.snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Codex `codex_utils_fuzzy_match::fuzzy_match` for the popup's non-file
/// candidates (skills): the matched character indices of `haystack`, sorted
/// and unique, with the score. nucleo's `Config::DEFAULT` — not the path
/// tuning — and a higher score is a better match, where Codex's own helper
/// counts the other way round.
#[must_use]
pub fn fuzzy_match(haystack: &str, needle: &str) -> Option<(Vec<usize>, u32)> {
    let pattern = nucleo::pattern::Pattern::parse(needle, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buffer = Vec::new();
    let haystack = nucleo::Utf32Str::new(haystack, &mut buffer);
    let mut indices = Vec::<u32>::new();
    let score = pattern.indices(haystack, &mut matcher, &mut indices)?;
    indices.sort_unstable();
    indices.dedup();
    Some((indices.into_iter().map(|index| index as usize).collect(), score))
}

// ---------------------------------------------------------------------------
// zo: the session's own rerank
// ---------------------------------------------------------------------------

/// What a file this session read or edited is worth on top of its match.
///
/// nucleo (`Config::DEFAULT.match_paths()`) scores a matched character at 16
/// and a boundary at +8, so a four-letter query lands a clean file-name match
/// near 100 and a scattered one near 50; measured on this workspace, the
/// top-20 of a typical query spans about 40 points. Half that keeps an exact
/// file name above a marginal touched match and lifts a touched file over an
/// untouched one of the same quality.
pub const TOUCHED_BOOST: u32 = 24;
/// How recently a file must have changed on disk to earn each tier, and what
/// the tier is worth. Read from the top: the first tier that fits is the one.
pub const RECENT_TIERS: [(Duration, u32); 3] = [
    (Duration::from_secs(60 * 60), 16),
    (Duration::from_secs(24 * 60 * 60), 8),
    (Duration::from_secs(7 * 24 * 60 * 60), 4),
];

/// zo's [`RankBoost`]: files this session touched, and files changed lately.
///
/// Touched paths are kept absolute and canonical so the same file noted as
/// `src/a.rs` and matched as `/repo/src/a.rs` is one entry. Recency is a stat
/// per snapshot candidate — at most `limit × RERANK_WINDOW` of them.
#[derive(Debug, Default)]
pub struct SessionBoost {
    touched: RwLock<HashSet<PathBuf>>,
}

impl SessionBoost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A tool read or wrote `path` (relative paths hang from `cwd`).
    pub fn note_touched(&self, cwd: &Path, path: &Path) {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        let key = absolute.canonicalize().unwrap_or(absolute);
        self.touched
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key);
    }

    /// A new conversation starts with nothing touched.
    pub fn forget_all(&self) {
        self.touched
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    #[must_use]
    pub fn touched_count(&self) -> usize {
        self.touched
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn is_touched(&self, full_path: &Path) -> bool {
        let touched = self
            .touched
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if touched.is_empty() {
            return false;
        }
        if touched.contains(full_path) {
            return true;
        }
        full_path
            .canonicalize()
            .is_ok_and(|canonical| touched.contains(&canonical))
    }
}

/// The tier a file modified `age` ago earns, from [`RECENT_TIERS`].
#[must_use]
pub fn recency_boost(age: Duration) -> u32 {
    RECENT_TIERS
        .iter()
        .find(|(within, _)| age <= *within)
        .map_or(0, |(_, boost)| *boost)
}

impl RankBoost for SessionBoost {
    fn boost(&self, full_path: &Path) -> u32 {
        let touched = if self.is_touched(full_path) {
            TOUCHED_BOOST
        } else {
            0
        };
        let recent = std::fs::metadata(full_path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .map_or(0, recency_boost);
        touched.saturating_add(recent)
    }
}

/// The measurement behind the numbers in the `@` popup's report: first rows
/// while the walk is still running, walk completion, and the per-keystroke
/// update once the index is warm — against a real tree.
///
/// ```text
/// ZO_FILE_SEARCH_BENCH_ROOT=<dir> \
/// ZO_FILE_SEARCH_BENCH_QUERIES=comp,app.rs,slash \
/// cargo test -p runtime --release --lib file_search::bench -- --ignored --nocapture
/// ```
///
/// `ZO_FILE_SEARCH_BENCH_BOOST=1` reranks with a [`SessionBoost`], so the
/// boost's own cost is in the numbers too, and
/// `ZO_FILE_SEARCH_BENCH_TOUCHED=a.rs,b/c.rs` names the files that boost
/// counts as this session's; every query's top rows are printed, so the
/// order with the boost off (Codex's) and on (zo's) can stand side by side.
#[cfg(test)]
mod bench {
    use super::*;
    use std::sync::Mutex;

    const ROOT_ENV: &str = "ZO_FILE_SEARCH_BENCH_ROOT";
    const QUERIES_ENV: &str = "ZO_FILE_SEARCH_BENCH_QUERIES";
    const BOOST_ENV: &str = "ZO_FILE_SEARCH_BENCH_BOOST";
    /// Comma-separated paths (relative to the root) the bench notes as
    /// touched before it runs — the session's read-or-edited files.
    const TOUCHED_ENV: &str = "ZO_FILE_SEARCH_BENCH_TOUCHED";
    /// How many of each query's top rows the order table prints.
    const ORDER_ROWS: usize = 5;
    const DEFAULT_QUERIES: &str = "comp,app.rs,slash,read";
    const SETTLE: Duration = Duration::from_secs(60);

    #[derive(Default)]
    struct Timeline {
        started: Option<Instant>,
        first_update_for: Mutex<Vec<(String, Duration, usize)>>,
        complete: Mutex<Option<(Duration, usize, usize)>>,
        latest: Mutex<Option<FileSearchSnapshot>>,
    }

    impl SessionReporter for Timeline {
        fn on_update(&self, snapshot: &FileSearchSnapshot) {
            let elapsed = self.started.map_or(Duration::ZERO, |started| started.elapsed());
            let mut seen = self
                .first_update_for
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The first result is the first snapshot with rows on it — the
            // one the popup shows — not the empty tick before the walk found
            // anything (the manager never forwards that one).
            let shows_rows = !snapshot.matches.is_empty() || snapshot.settled;
            if shows_rows && !seen.iter().any(|(query, _, _)| *query == snapshot.query) {
                seen.push((snapshot.query.clone(), elapsed, snapshot.scanned_file_count));
            }
            if snapshot.walk_complete {
                let mut complete = self
                    .complete
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if complete.is_none() {
                    *complete = Some((elapsed, snapshot.scanned_file_count, snapshot.total_match_count));
                }
            }
            *self
                .latest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(snapshot.clone());
        }

        fn on_complete(&self) {}
    }

    impl Timeline {
        fn wait_for(&self, query: &str) -> Duration {
            let deadline = Instant::now() + SETTLE;
            loop {
                if let Some((_, elapsed, _)) = self
                    .first_update_for
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .find(|(seen, _, _)| seen == query)
                {
                    return *elapsed;
                }
                assert!(Instant::now() < deadline, "no snapshot for {query:?} within {SETTLE:?}");
                thread::sleep(Duration::from_micros(200));
            }
        }

        fn wait_complete(&self) -> (Duration, usize, usize) {
            let deadline = Instant::now() + SETTLE;
            loop {
                if let Some(done) = *self
                    .complete
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                {
                    return done;
                }
                assert!(Instant::now() < deadline, "walk did not complete within {SETTLE:?}");
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn ms(duration: Duration) -> String {
        format!("{:.1}", duration.as_secs_f64() * 1000.0)
    }

    #[test]
    #[ignore = "a measurement against a real tree; run with --ignored --nocapture"]
    #[allow(clippy::too_many_lines)] // one measurement, four tables, in the order they are read
    fn first_result_and_keystroke_latency() {
        let root = std::env::var_os(ROOT_ENV)
            .map_or_else(|| std::env::current_dir().expect("cwd"), PathBuf::from);
        let queries: Vec<String> = std::env::var(QUERIES_ENV)
            .unwrap_or_else(|_| DEFAULT_QUERIES.to_string())
            .split(',')
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(str::to_string)
            .collect();
        let boost = std::env::var(BOOST_ENV).is_ok_and(|value| value == "1");
        let session_boost = Arc::new(SessionBoost::new());
        let touched: Vec<String> = std::env::var(TOUCHED_ENV)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect();
        for path in &touched {
            session_boost.note_touched(&root, Path::new(path));
        }
        let mut orders: Vec<(String, Vec<String>)> = Vec::new();

        println!("root: {}", root.display());
        println!("boost: {boost}");
        println!();
        println!("| query | first result (ms) | files scanned at first result | walk complete (ms) | files | matches |");
        println!("|---|---|---|---|---|---|");
        for query in &queries {
            let reporter = Arc::new(Timeline {
                started: Some(Instant::now()),
                ..Timeline::default()
            });
            let session = create_session(
                vec![SearchRoot::repo(&root)],
                FileSearchOptions {
                    compute_indices: true,
                    boost: boost.then(|| Arc::clone(&session_boost) as Arc<dyn RankBoost>),
                    ..FileSearchOptions::default()
                },
                Arc::clone(&reporter) as Arc<dyn SessionReporter>,
                None,
            )
            .expect("session");
            session.update_query(query);
            let first = reporter.wait_for(query);
            let scanned_at_first = reporter
                .first_update_for
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|(seen, _, _)| seen == query)
                .map_or(0, |(_, _, scanned)| *scanned);
            let (complete, files, matches) = reporter.wait_complete();
            let top: Vec<String> = reporter
                .latest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .matches
                        .iter()
                        .take(ORDER_ROWS)
                        .map(|entry| format!("{} ({})", entry.path.display(), entry.score))
                        .collect()
                })
                .unwrap_or_default();
            orders.push((query.clone(), top));
            println!(
                "| `{query}` | {} | {scanned_at_first} | {} | {files} | {matches} |",
                ms(first),
                ms(complete)
            );
            drop(session);
        }

        println!();
        println!("touched: {touched:?}");
        println!("| query | top rows (path (score)) |");
        println!("|---|---|");
        for (query, rows) in &orders {
            println!("| `{query}` | {} |", rows.join(" · "));
        }

        println!();
        println!("| query typed | keystroke | update (ms) |");
        println!("|---|---|---|");
        for query in &queries {
            let reporter = Arc::new(Timeline::default());
            let session = create_session(
                vec![SearchRoot::repo(&root)],
                FileSearchOptions {
                    compute_indices: true,
                    boost: boost.then(|| Arc::clone(&session_boost) as Arc<dyn RankBoost>),
                    ..FileSearchOptions::default()
                },
                Arc::clone(&reporter) as Arc<dyn SessionReporter>,
                None,
            )
            .expect("session");
            // Warm: the first character, waited to completion, so the keystrokes
            // below measure the matcher and not the walk.
            let warm: String = query.chars().take(1).collect();
            session.update_query(&warm);
            reporter.wait_complete();
            let mut typed = String::new();
            for ch in query.chars() {
                typed.push(ch);
                let started = Instant::now();
                reporter
                    .first_update_for
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .retain(|(seen, _, _)| *seen != typed);
                session.update_query(&typed);
                let deadline = Instant::now() + SETTLE;
                loop {
                    let seen = reporter
                        .first_update_for
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .iter()
                        .any(|(seen, _, _)| *seen == typed);
                    if seen {
                        break;
                    }
                    assert!(Instant::now() < deadline, "no update for {typed:?}");
                    thread::sleep(Duration::from_micros(100));
                }
                println!("| `{typed}` | `{ch}` | {} |", ms(started.elapsed()));
            }
            drop(session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicUsize;

    fn world(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for file in files {
            let path = dir.path().join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent");
            }
            fs::write(&path, b"x").expect("write");
        }
        dir
    }

    fn paths(results: &FileSearchResults) -> Vec<String> {
        results
            .matches
            .iter()
            .map(|entry| entry.path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_query_finds_files_by_fuzzy_path_and_reports_the_total() {
        let dir = world(&["src/composer.rs", "src/compose_tests.rs", "docs/notes.md"]);
        let results = run(
            "comp",
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions::default(),
            None,
        )
        .expect("search");
        let found = paths(&results);
        assert!(found.contains(&"src/composer.rs".to_string()), "{found:?}");
        assert!(found.contains(&"src/compose_tests.rs".to_string()), "{found:?}");
        assert!(!found.iter().any(|path| path.contains("notes")), "{found:?}");
        assert_eq!(results.total_match_count, 2);
    }

    #[test]
    fn indices_mark_the_matched_characters_sorted_and_unique() {
        let dir = world(&["src/composer.rs"]);
        let results = run(
            "cmp",
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions {
                compute_indices: true,
                ..FileSearchOptions::default()
            },
            None,
        )
        .expect("search");
        let indices = results.matches[0].indices.clone().expect("indices");
        assert!(indices.windows(2).all(|pair| pair[0] < pair[1]), "{indices:?}");
        let display: Vec<char> = "src/composer.rs".chars().collect();
        let matched: String = indices.iter().map(|&i| display[i as usize]).collect();
        assert_eq!(matched, "cmp");
    }

    #[test]
    fn gitignored_files_and_the_object_store_never_show() {
        let dir = world(&["src/keep.rs", "target/skip.rs", ".git/objects/ab/skipme"]);
        fs::write(dir.path().join(".gitignore"), "target/\n").expect("gitignore");
        // `require_git(true)`: a `.gitignore` counts only inside a repository.
        fs::create_dir_all(dir.path().join(".git")).expect(".git");
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").expect("HEAD");
        let results = run(
            "s",
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions::default(),
            None,
        )
        .expect("search");
        let found = paths(&results);
        assert!(found.contains(&"src/keep.rs".to_string()), "{found:?}");
        assert!(!found.iter().any(|path| path.starts_with("target")), "{found:?}");
        assert!(!found.iter().any(|path| path.starts_with(".git")), "{found:?}");
    }

    #[test]
    fn a_page_root_shows_only_pages_under_its_prefix() {
        let repo = world(&["src/alpha.rs"]);
        let vault = world(&["wiki/alpha-page.md", "wiki/alpha.png", "wiki/deep/alpha-two.md"]);
        let results = run(
            "alpha",
            vec![
                SearchRoot::repo(repo.path()),
                SearchRoot::pages(vault.path().join("wiki"), "wiki/"),
            ],
            FileSearchOptions::default(),
            None,
        )
        .expect("search");
        let found = paths(&results);
        assert!(found.contains(&"src/alpha.rs".to_string()), "{found:?}");
        assert!(found.contains(&"wiki/alpha-page.md".to_string()), "{found:?}");
        assert!(found.contains(&"wiki/deep/alpha-two.md".to_string()), "{found:?}");
        assert!(
            !found.iter().any(|path| Path::new(path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))),
            "{found:?}"
        );
        let page = results
            .matches
            .iter()
            .find(|entry| entry.path.to_string_lossy() == "wiki/alpha-page.md")
            .expect("page row");
        assert_eq!(page.full_path(), vault.path().join("wiki/alpha-page.md"));
        assert_eq!(page.root, vault.path().join("wiki"));
    }

    #[test]
    fn a_touched_file_outranks_a_better_untouched_match() {
        let dir = world(&["alpha.md", "notes/alpha_notes.md"]);
        let plain = run(
            "alpha",
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions::default(),
            None,
        )
        .expect("search");
        assert_eq!(paths(&plain)[0], "alpha.md", "nucleo prefers the short exact name");

        let boost = Arc::new(SessionBoost::new());
        boost.note_touched(dir.path(), Path::new("notes/alpha_notes.md"));
        let boosted = run(
            "alpha",
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions {
                boost: Some(boost),
                ..FileSearchOptions::default()
            },
            None,
        )
        .expect("search");
        assert_eq!(paths(&boosted)[0], "notes/alpha_notes.md", "{boosted:?}");
        assert_eq!(boosted.matches.len(), 2);
        assert_eq!(
            boosted.matches[0].score,
            plain.matches[1].score + TOUCHED_BOOST + recency_boost(Duration::ZERO)
        );
    }

    #[test]
    fn recency_tiers_read_from_the_top_and_stop_after_a_week() {
        assert_eq!(recency_boost(Duration::ZERO), RECENT_TIERS[0].1);
        assert_eq!(recency_boost(Duration::from_secs(2 * 60 * 60)), RECENT_TIERS[1].1);
        assert_eq!(recency_boost(Duration::from_secs(3 * 24 * 60 * 60)), RECENT_TIERS[2].1);
        assert_eq!(recency_boost(Duration::from_secs(30 * 24 * 60 * 60)), 0);
    }

    #[test]
    fn a_boost_is_applied_across_the_rerank_window_and_nothing_past_it_is_lost() {
        let files: Vec<String> = (0..60).map(|n| format!("row{n:02}.txt")).collect();
        let names: Vec<&str> = files.iter().map(String::as_str).collect();
        let dir = world(&names);
        let boost = Arc::new(SessionBoost::new());
        boost.note_touched(dir.path(), Path::new("row59.txt"));
        let results = run(
            "row",
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions {
                limit: NonZero::new(5).expect("limit"),
                boost: Some(boost),
                ..FileSearchOptions::default()
            },
            None,
        )
        .expect("search");
        assert_eq!(results.matches.len(), 5);
        assert_eq!(paths(&results)[0], "row59.txt", "{results:?}");
        assert_eq!(results.total_match_count, 60);
    }

    struct Counting {
        updates: AtomicUsize,
        settled: AtomicUsize,
        completes: AtomicUsize,
    }

    impl SessionReporter for Counting {
        fn on_update(&self, snapshot: &FileSearchSnapshot) {
            self.updates.fetch_add(1, Ordering::SeqCst);
            if snapshot.settled {
                self.settled.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn on_complete(&self) {
            self.completes.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn dropping_the_session_completes_the_reporter_and_stops_the_threads() {
        let dir = world(&["a.rs"]);
        let reporter = Arc::new(Counting {
            updates: AtomicUsize::new(0),
            settled: AtomicUsize::new(0),
            completes: AtomicUsize::new(0),
        });
        let session = create_session(
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions::default(),
            Arc::clone(&reporter) as Arc<dyn SessionReporter>,
            None,
        )
        .expect("session");
        session.update_query("a");
        let deadline = Instant::now() + Duration::from_secs(5);
        while reporter.completes.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(reporter.updates.load(Ordering::SeqCst) >= 1);
        drop(session);
        let deadline = Instant::now() + Duration::from_secs(5);
        while reporter.completes.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(reporter.completes.load(Ordering::SeqCst) >= 2, "the shutdown completes once more");
    }

    #[test]
    fn fuzzy_match_marks_the_characters_of_a_candidate_name() {
        let (indices, score) = fuzzy_match("typesafe-ai", "tsai").expect("match");
        let name: Vec<char> = "typesafe-ai".chars().collect();
        let matched: String = indices.iter().map(|&i| name[i]).collect();
        assert_eq!(matched, "tsai");
        assert!(score > 0);
        assert!(fuzzy_match("typesafe-ai", "zzz").is_none());
        assert_eq!(fuzzy_match("anything", "").map(|(indices, _)| indices), Some(Vec::new()));
    }

    #[test]
    fn a_query_with_no_match_still_hears_the_walk_end() {
        let dir = world(&["a.rs"]);
        let reporter = Arc::new(Counting {
            updates: AtomicUsize::new(0),
            settled: AtomicUsize::new(0),
            completes: AtomicUsize::new(0),
        });
        let session = create_session(
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions::default(),
            Arc::clone(&reporter) as Arc<dyn SessionReporter>,
            None,
        )
        .expect("session");
        session.update_query("zzz");
        let deadline = Instant::now() + Duration::from_secs(5);
        while reporter.settled.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(reporter.settled.load(Ordering::SeqCst) >= 1, "the search's end is a settled snapshot");
        let results = run(
            "zzz",
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions::default(),
            None,
        )
        .expect("search");
        assert!(results.matches.is_empty());
    }

    #[test]
    fn a_cancel_flag_ends_the_run() {
        let dir = world(&["a.rs"]);
        let cancel = Arc::new(AtomicBool::new(true));
        let results = run(
            "a",
            vec![SearchRoot::repo(dir.path())],
            FileSearchOptions::default(),
            Some(cancel),
        )
        .expect("search");
        assert!(results.matches.len() <= 1);
    }
}

