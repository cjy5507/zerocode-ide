//! The session vault's walk, kept between calls — and between two runs of
//! the window.
//!
//! The panel re-asks `vault_sessions` on every keystroke, every grouping,
//! every sort and every filter press. Each of those changes the QUERY and
//! none of them changes the disk, so none of them reads it: a query is
//! answered from the walk held here.
//!
//! What a walk cost was measured on one machine on 2026-11-11 (t-103659; the
//! walk as it is now is measured by `cargo run --release -p zerocode-core
//! --example vault_scan_cost`): 10.6 s the first time a run opened the panel
//! and 1.7–2.3 s after — 169,127 folder entries asked about, 153,449 of them
//! one zo store's state rather than its sessions, and some 2,800 transcript
//! heads (about 550 MB) read to show a list in which a handful had changed. And every
//! ask, a keystroke too, listed the window's workspaces again: 28 git
//! processes, 0.39 s. So three things are kept, and a query waits for none
//! of them:
//!
//! - **the walk** — the sessions as the scan left them, the workspaces git
//!   listed for their group names, and what the scan could not read. A query
//!   borrows it; only the cards it shows are copied.
//! - **what each file gave** ([`zerocode_core::vault_index`]) — a walk opens
//!   only a transcript whose stamp moved since it was last read.
//! - **both, written down** in the window's own data folder ([`kept_file`])
//!   — never beside a session store, which is a vendor's. The panel's first
//!   answer after the window starts is what the last run knew, while a walk
//!   looks at what moved since.
//!
//! A walk runs on a thread of its own and one at a time ([`ScanCell`]). The
//! command never waits for it: it answers from what is held and says
//! `refreshing`, and the window asks again — the contract the usage panel
//! holds for the same reason.
//!
//! The neighbouring usage panel keeps its promise by shipping every row once
//! and cutting the ranges in the window. The vault cannot: its filter, sort
//! and grouping are Rust's on purpose, so that the webview never holds a
//! thousand sessions it is not drawing. Same promise, opposite side of the
//! boundary — which is why this is a store and not a bigger payload.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use zerocode_core::usage_stats::WorktreeRef;
use zerocode_core::vault::{
    self, ChildTranscript, Home, Limits, ScanCount, ScanIssue, VaultSession,
};
use zerocode_core::vault_index::Index;

use crate::scan_cell::ScanCell;

/// The folder under the window's own data where the walk is written down.
const KEPT_DIR: &str = "session-vault";

/// The file in [`KEPT_DIR`] that holds it.
const KEPT_FILE: &str = "walk.json";

/// The shape of that file. One written under another number is not read at
/// all: the walk reads the stores, as a first run does.
const KEPT_FORMAT: u32 = 1;

/// The agent this window runs under an isolated runtime, whose sessions are
/// read from the window's own data rather than from the person's home: its
/// source reads `.claude/projects` under whichever home it is handed, and the
/// window's config root is the home that resolves to that runtime.
const APP_STORE_AGENT: &str = "claude";

/// Where the walk is written down under `local_data_root`.
#[must_use]
pub fn kept_file(local_data_root: &Path) -> PathBuf {
    local_data_root.join(KEPT_DIR).join(KEPT_FILE)
}

/// One walk of the session stores, before any query has touched it.
///
/// The sessions are as the SCAN left them: `resume` still carries no settings.
/// That rewrite stays per-call on purpose — a launch override somebody edits in
/// settings has to reach the next card, and a base baked in here would need a
/// reload before anyone believed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Walk {
    pub sessions: Vec<VaultSession>,
    pub issues: Vec<ScanIssue>,
    pub truncated: bool,
    /// The workspaces this window manages, as git listed them for this walk —
    /// what a card's group is named by. Kept with the walk because listing
    /// them is two git processes a project, and a keystroke used to pay that.
    pub worktrees: Vec<WorktreeRef>,
    /// The machine's temporary folders, as this walk resolved them
    /// ([`zerocode_core::zo_project::temporary_roots`]): a conversation that
    /// ran under one is a measurement's, kept apart from a person's.
    pub temporary: Vec<PathBuf>,
}

/// The sessions no file gives, and what reading them could not do.
pub type Rows = (Vec<VaultSession>, Vec<ScanIssue>);

/// What a walk needs of the window, owned: the walk runs on its own thread,
/// and neither the pane roster nor the project catalog crosses one.
pub struct Sources {
    /// The person's home: every agent's store but [`APP_STORE_AGENT`]'s.
    pub home: PathBuf,
    /// The window's config root: [`APP_STORE_AGENT`]'s store.
    pub app_home: PathBuf,
    /// [`kept_file`] of this window's data.
    pub kept_at: PathBuf,
    /// The helpers live panes say they started, read when the ask was made.
    pub roster: Vec<ChildTranscript>,
    /// The sessions no file gives — OpenCode's database rows, read by the
    /// caller's reader because this module carries no database. Handed the
    /// limit every reader honours alone.
    pub rows: Box<dyn FnOnce(usize) -> Rows + Send>,
    /// The workspaces this window manages.
    pub worktrees: Box<dyn FnOnce() -> Vec<WorktreeRef> + Send>,
}

/// What a query is answered from.
#[derive(Debug, Clone)]
pub struct Answer {
    /// The walk that is held. A walk that found nothing changed leaves the
    /// one that was held where it is.
    pub walk: Arc<Walk>,
    /// Whether a walk is looking at the disk right now.
    pub refreshing: bool,
}

/// What a walk reads through and leaves for the next one — and, written
/// down, what the next run of the window opens on.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Reading {
    /// [`KEPT_FORMAT`] as the file was written.
    format: u32,
    /// The build whose reader made the cards. Another build's are shown and
    /// not believed: its first walk reads every file again.
    build: String,
    index: Index,
    /// The sessions no file gave ([`Sources::rows`]).
    rows: Vec<VaultSession>,
    issues: Vec<ScanIssue>,
    truncated: bool,
    worktrees: Vec<WorktreeRef>,
    temporary: Vec<PathBuf>,
    /// What the last walk of this run did, in counts: one that opened a
    /// file is written down. Not written down itself: it says what a walk
    /// cost, and the next run's walk has its own.
    #[serde(skip)]
    count: ScanCount,
}

impl Reading {
    /// The list this reading was of, from what is written alone: no file
    /// opened, no folder walked, no git asked.
    fn walk(&self) -> Walk {
        let limit = Limits::DEFAULT.absolute_max;
        let (sessions, cut) = vault::sessions_of(&self.index, limit);
        let (sessions, dropped) = vault::merge(sessions, self.rows.clone(), limit);
        Walk {
            sessions,
            issues: self.issues.clone(),
            truncated: self.truncated || cut || dropped,
            worktrees: self.worktrees.clone(),
            temporary: self.temporary.clone(),
        }
    }
}

/// Where the last walk waits.
///
/// A type rather than a bare static so the rule can be tested without a
/// process-wide walk: the production one is [`answer`] below.
///
/// `force` is the panel being come to, or its reload button — an ask to look
/// at the disk again. There is deliberately no clock. An age bound would be a
/// second rule about freshness on a surface that already carries a visible
/// reload, and a walk now costs what a look costs.
pub struct Store {
    kept: ScanCell<Arc<Walk>>,
    /// `None` until a walk or the written-down one fills it.
    reading: Mutex<Option<Reading>>,
    /// Whether the written-down walk has been looked for. Once a run.
    opened: AtomicBool,
}

impl Store {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            kept: ScanCell::new(),
            reading: Mutex::new(None),
            opened: AtomicBool::new(false),
        }
    }

    /// Whether an ask may start a walk, so its caller gathers [`Sources`]:
    /// a forced one, and the first of a run. Every other ask is a question
    /// about the walk already held, and reads nothing.
    #[must_use]
    pub fn may_walk(&self, force: bool) -> bool {
        force || !self.opened.load(Ordering::SeqCst)
    }

    /// Answer from what is held, and start a walk when the ask is one that
    /// may ([`Self::may_walk`]) and came with its `sources`.
    ///
    /// Never waits for a walk. A forced ask does not throw away what is held
    /// either: a walk that was asked for and then went wrong would otherwise
    /// leave the panel with nothing where it had an answer a moment ago.
    pub fn answer(&'static self, force: bool, sources: Option<Sources>, build: &str) -> Answer {
        if let Some(sources) = sources {
            let first = !self.opened.swap(true, Ordering::SeqCst);
            if first {
                self.open(&sources.kept_at, build);
            }
            if force || first {
                let build = build.to_string();
                self.kept.start(move || self.walk(sources, build));
            }
        }
        Answer {
            walk: self.kept.held().unwrap_or_default(),
            refreshing: self.kept.running(),
        }
    }

    /// Hold what the last run wrote down, when it wrote anything this build
    /// can read. A file that cannot be read is no file: the walk reads the
    /// stores, as the first run did.
    fn open(&self, kept_at: &Path, build: &str) {
        let Some(mut reading) = std::fs::read(kept_at)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Reading>(&bytes).ok())
            .filter(|reading| reading.format == KEPT_FORMAT)
        else {
            return;
        };
        if reading.build != build {
            reading.index.distrust();
        }
        self.kept.hold(Arc::new(reading.walk()));
        if let Ok(mut held) = self.reading.lock() {
            *held = Some(reading);
        }
    }

    /// One walk: the stores through the index, the rows no file gives, the
    /// helpers of live panes, the workspaces. What it found is held when it
    /// differs from what was — else the walk that was held stays — and
    /// written down when a file was read or the list moved.
    fn walk(&self, sources: Sources, build: String) -> Arc<Walk> {
        let Sources {
            home,
            app_home,
            kept_at,
            roster,
            rows,
            worktrees,
        } = sources;
        let before = self.kept.held();
        let mut reading = self
            .reading
            .lock()
            .ok()
            .and_then(|mut held| held.take())
            .unwrap_or_default();
        let limit = Limits::DEFAULT.absolute_max;
        let found = vault::scan_homes(
            &[
                Home {
                    dir: &home,
                    reads: |source| source.slug != APP_STORE_AGENT,
                },
                Home {
                    dir: &app_home,
                    reads: |source| source.slug == APP_STORE_AGENT,
                },
            ],
            limit,
            &vault::env_home,
            &mut reading.index,
        );
        let mut issues = found.issues;
        let (rows, row_issues) = rows(limit);
        issues.extend(row_issues);
        let (mut sessions, dropped) = vault::merge(found.sessions, rows.clone(), limit);
        // What is written down is the walk without the helpers of live
        // panes: they are read off a roster the next run does not have.
        let written_issues = issues.clone();
        vault::read_roster_children(&mut sessions, &roster, &mut issues);
        let walk = Walk {
            sessions,
            issues,
            truncated: found.truncated || dropped,
            worktrees: worktrees(),
            temporary: zerocode_core::zo_project::temporary_roots(),
        };
        let changed = before.as_ref().is_none_or(|kept| **kept != walk);
        reading = Reading {
            format: KEPT_FORMAT,
            build,
            index: reading.index,
            rows,
            issues: written_issues,
            truncated: walk.truncated,
            worktrees: walk.worktrees.clone(),
            temporary: walk.temporary.clone(),
            count: found.count,
        };
        if changed || reading.count.read > 0 {
            write_down(&kept_at, &reading);
        }
        if let Ok(mut held) = self.reading.lock() {
            *held = Some(reading);
        }
        match before {
            Some(kept) if !changed => kept,
            _ => Arc::new(walk),
        }
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

/// Write the walk down for the next run: a file only its owner can read, in
/// a folder only its owner can open ([`crate::durable_file::replace_bytes`]),
/// because it holds the first lines of conversations. One that cannot be
/// written is one read again, no more.
fn write_down(kept_at: &Path, reading: &Reading) {
    if let Ok(bytes) = serde_json::to_vec(reading) {
        let _ = crate::durable_file::replace_bytes(kept_at, &bytes);
    }
}

/// This process's own walk.
static KEPT: Store = Store::new();

/// [`Store::may_walk`] on this process's walk.
#[must_use]
pub fn may_walk(force: bool) -> bool {
    KEPT.may_walk(force)
}

/// [`Store::answer`] on this process's walk.
pub fn answer(force: bool, sources: Option<Sources>, build: &str) -> Answer {
    KEPT.answer(force, sources, build)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicUsize;

    const BUILD: &str = "build-a";

    /// A store of its own for one test, standing as long as a walk's thread
    /// may need it.
    fn store() -> &'static Store {
        Box::leak(Box::new(Store::new()))
    }

    /// A home with `sessions` Claude conversations under the window's own
    /// store, each one line long.
    fn stand(app_home: &Path, sessions: usize) {
        let root = app_home.join(".claude/projects/fixture");
        std::fs::create_dir_all(&root).expect("mkdir");
        for n in 0..sessions {
            write_session(app_home, n, "ask");
        }
    }

    fn write_session(app_home: &Path, n: usize, words: &str) {
        std::fs::write(
            app_home.join(format!(".claude/projects/fixture/{n}.jsonl")),
            format!(
                r#"{{"type":"user","sessionId":"{n}","cwd":"/w/fixture","message":{{"role":"user","content":"{words} {n}"}}}}"#
            ),
        )
        .expect("write");
    }

    /// What a walk of the fixture needs, counting how often git and the
    /// database were asked.
    fn sources(dir: &Path, asked: &'static AtomicUsize) -> Sources {
        Sources {
            home: dir.join("home"),
            app_home: dir.join("app"),
            kept_at: kept_file(&dir.join("data")),
            roster: Vec::new(),
            rows: Box::new(|_| {
                asked.fetch_add(1, Ordering::SeqCst);
                (Vec::new(), Vec::new())
            }),
            worktrees: Box::new(|| {
                asked.fetch_add(1, Ordering::SeqCst);
                Vec::new()
            }),
        }
    }

    /// The answer once no walk is running.
    fn settled(store: &'static Store) -> Answer {
        for _ in 0..500 {
            let answer = store.answer(false, None, BUILD);
            if !answer.refreshing {
                return answer;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the walk never ended");
    }

    fn counter() -> &'static AtomicUsize {
        Box::leak(Box::new(AtomicUsize::new(0)))
    }

    /// The first ask of a run answers at once with nothing and says a walk is
    /// looking; the walk's answer is there for the ask after it.
    #[test]
    fn the_first_ask_does_not_wait_for_the_walk() {
        let dir = tempfile::tempdir().expect("temp");
        stand(&dir.path().join("app"), 3);
        let store = store();
        // The walk stands at a gate until the first answer has been read.
        let (open, gate) = std::sync::mpsc::channel::<()>();
        let mut held = sources(dir.path(), counter());
        held.rows = Box::new(move |_| {
            let _ = gate.recv();
            (Vec::new(), Vec::new())
        });
        let first = store.answer(false, Some(held), BUILD);
        assert_eq!(
            (first.walk.sessions.len(), first.refreshing),
            (0, true),
            "the first ask waited for the disk, or did not start a walk"
        );
        open.send(()).expect("the walk is waiting");
        assert_eq!(settled(store).walk.sessions.len(), 3);
    }

    /// A keystroke — any ask that is not forced, after the first — reads
    /// what the last walk said: no file, no database, and git zero times
    /// (the counter is asked once by each: the rows and the workspace list).
    #[test]
    fn a_query_starts_no_walk_and_asks_nothing() {
        let dir = tempfile::tempdir().expect("temp");
        stand(&dir.path().join("app"), 2);
        let store = store();
        let asked = counter();
        store.answer(false, Some(sources(dir.path(), asked)), BUILD);
        settled(store);
        let walked = asked.load(Ordering::SeqCst);
        assert!(
            !store.may_walk(false),
            "a query after the first is told to gather a walk's sources"
        );
        // Even handed sources, an unforced ask after the first walks nothing.
        write_session(&dir.path().join("app"), 9, "new");
        let answer = store.answer(false, Some(sources(dir.path(), asked)), BUILD);
        assert_eq!(
            (
                answer.refreshing,
                answer.walk.sessions.len(),
                asked.load(Ordering::SeqCst)
            ),
            (false, 2, walked),
            "a query read the disk"
        );
    }

    /// The reload button and the panel being come to: a walk starts, the
    /// answer is what was held, and a walk that found nothing new leaves the
    /// held walk where it is — the window is handed the list it already drew.
    #[test]
    fn a_forced_ask_walks_again_and_the_walk_moves_only_with_the_disk() {
        let dir = tempfile::tempdir().expect("temp");
        stand(&dir.path().join("app"), 2);
        let store = store();
        store.answer(false, Some(sources(dir.path(), counter())), BUILD);
        let first = settled(store);

        let held = store.answer(true, Some(sources(dir.path(), counter())), BUILD);
        assert_eq!(
            held.walk.sessions.len(),
            2,
            "a forced ask emptied the panel while its walk ran"
        );
        assert!(
            Arc::ptr_eq(&settled(store).walk, &first.walk),
            "a walk that found nothing new replaced the walk that was held"
        );

        write_session(&dir.path().join("app"), 7, "new");
        store.answer(true, Some(sources(dir.path(), counter())), BUILD);
        let moved = settled(store);
        assert_eq!(moved.walk.sessions.len(), 3);
        assert!(!Arc::ptr_eq(&moved.walk, &first.walk));
    }

    /// What the store's last walk did, in counts.
    fn last_walk(store: &Store) -> ScanCount {
        store
            .reading
            .lock()
            .expect("lock")
            .as_ref()
            .map(|reading| reading.count)
            .unwrap_or_default()
    }

    /// A walk opens the files that moved and no other: all of them the first
    /// time, none when nothing moved, one when one conversation went on.
    #[test]
    fn a_walk_opens_no_file_that_has_not_moved() {
        let dir = tempfile::tempdir().expect("temp");
        stand(&dir.path().join("app"), 5);
        let store = store();
        let mut opened = Vec::new();
        store.answer(false, Some(sources(dir.path(), counter())), BUILD);
        settled(store);
        opened.push(last_walk(store).read);

        store.answer(true, Some(sources(dir.path(), counter())), BUILD);
        settled(store);
        opened.push(last_walk(store).read);

        write_session(&dir.path().join("app"), 3, "a second and longer ask");
        store.answer(true, Some(sources(dir.path(), counter())), BUILD);
        settled(store);
        opened.push(last_walk(store).read);
        assert_eq!(opened, [5, 0, 1]);
    }

    /// The next run of the window opens on what this one wrote down, before
    /// its own walk has read anything — and that walk opens only what moved.
    #[test]
    fn the_next_run_opens_on_what_was_written_down_and_reads_only_what_moved() {
        let dir = tempfile::tempdir().expect("temp");
        stand(&dir.path().join("app"), 4);
        let earlier = store();
        earlier.answer(false, Some(sources(dir.path(), counter())), BUILD);
        settled(earlier);
        assert!(kept_file(&dir.path().join("data")).is_file());

        // One conversation went on between the two runs.
        write_session(&dir.path().join("app"), 2, "a longer second ask");
        let later = store();
        let first = later.answer(false, Some(sources(dir.path(), counter())), BUILD);
        assert_eq!(
            first.walk.sessions.len(),
            4,
            "the next run opened on an empty panel"
        );
        let after = settled(later);
        assert!(
            after
                .walk
                .sessions
                .iter()
                .any(|session| session.title.starts_with("a longer second ask")),
            "the walk did not read the conversation that moved"
        );
        let read = later
            .reading
            .lock()
            .expect("lock")
            .as_ref()
            .map(|reading| reading.index.len());
        assert_eq!(read, Some(4));
    }

    /// What another build wrote down is shown, and none of it is believed:
    /// the walk reads every file again and what it finds replaces it.
    #[test]
    fn another_builds_cards_are_shown_until_this_builds_walk_has_read_them() {
        let dir = tempfile::tempdir().expect("temp");
        stand(&dir.path().join("app"), 2);
        let earlier = store();
        earlier.answer(false, Some(sources(dir.path(), counter())), "build-old");
        settled(earlier);

        let later = store();
        let first = later.answer(false, Some(sources(dir.path(), counter())), BUILD);
        assert_eq!(
            first.walk.sessions.len(),
            2,
            "the old build's list was dropped"
        );
        settled(later);
        let written: Reading = serde_json::from_slice(
            &std::fs::read(kept_file(&dir.path().join("data"))).expect("read"),
        )
        .expect("the walk is written down");
        assert_eq!(
            written.build, BUILD,
            "the new build's walk did not write its own reading down"
        );
    }

    /// A file written under another format number is not shown at all — its
    /// cards may not be cards — and the run's own walk replaces it.
    #[test]
    fn another_formats_file_is_not_opened() {
        let dir = tempfile::tempdir().expect("temp");
        stand(&dir.path().join("app"), 2);
        let earlier = store();
        earlier.answer(false, Some(sources(dir.path(), counter())), BUILD);
        settled(earlier);
        let kept_at = kept_file(&dir.path().join("data"));
        let mut written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&kept_at).expect("read")).expect("json");
        written["format"] = serde_json::json!(KEPT_FORMAT + 1);
        std::fs::write(&kept_at, serde_json::to_vec(&written).expect("json")).expect("write");

        let later = store();
        later.open(&kept_at, BUILD);
        assert!(
            later.kept.held().is_none(),
            "a file of another format was opened"
        );
    }

    /// What is written down holds the first lines of conversations: the file
    /// is its owner's alone, and so is the folder it lies in.
    #[cfg(unix)]
    #[test]
    fn the_written_down_walk_is_its_owners_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("temp");
        stand(&dir.path().join("app"), 1);
        let store = store();
        store.answer(false, Some(sources(dir.path(), counter())), BUILD);
        settled(store);
        let kept_at = kept_file(&dir.path().join("data"));
        let mode = |path: &Path| path.metadata().expect("meta").permissions().mode() & 0o777;
        assert_eq!(
            (mode(&kept_at), mode(kept_at.parent().expect("a folder"))),
            (0o600, 0o700)
        );
    }

    /// A written-down walk that cannot be read is no walk: the run reads the
    /// stores, as a first run does.
    #[test]
    fn a_torn_file_is_read_as_no_file() {
        let dir = tempfile::tempdir().expect("temp");
        stand(&dir.path().join("app"), 1);
        let kept_at = kept_file(&dir.path().join("data"));
        std::fs::create_dir_all(kept_at.parent().expect("a folder")).expect("mkdir");
        std::fs::write(&kept_at, b"{\"build\":").expect("write");
        let store = store();
        store.answer(false, Some(sources(dir.path(), counter())), BUILD);
        assert_eq!(settled(store).walk.sessions.len(), 1);
    }
}
