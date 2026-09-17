//! Watching the open files for the writes this window did not make.
//!
//! The stage's whole job is showing a checkout that agents rewrite, so a file
//! changing under an open tab is the ordinary case, not an edge. Until this
//! module the window only noticed when a tab was brought forward
//! (`checkFileMoved`) or a save was refused — a person watching an agent work
//! saw a stale file and no sign it was stale. Orca pushes: its main process
//! watches the paths the open tabs derive (out/main/index.js:50790-50880,
//! `@parcel/watcher` behind batching and per-path coalescing), and the
//! renderer re-reads clean tabs and marks dirty ones
//! (index-ftls8Hg_.js:147351-147462).
//!
//! This is that push, sized to what actually consumes it. The watch set is
//! the open file tabs — a handful of paths, never a tree — so the set is
//! statted whole on a timer instead of through a native watcher: no new
//! dependency, and a poll of ten files twice a second costs less than the
//! native watcher's own bookkeeping. What the event transport loses next to
//! Orca's, the shape gives back for free: coalescing is inherent (a poll
//! compares final states, so a save-by-rename is one `update`, never a
//! `delete`+`create` burst — the burst Orca's 75 ms debounces exist to absorb,
//! index-ftls8Hg_.js:147092), and the overflow event cannot happen because the
//! set is bounded by the tab strip.
//!
//! Nothing here decides what a change MEANS. The window hears `fs:changed`
//! and answers with the same function that answers when a tab is brought
//! forward, so the push and the pull cannot disagree.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

/// How often the open files are statted.
///
/// Half a second, not Orca's 75 ms: their constant debounces a native event
/// stream that fires per write, ours is the whole detection cadence. An agent
/// finishing a file is a human-scale event — half a second between the write
/// and the repaint reads as immediate, and halving it would double the idle
/// cost of every open tab for a difference nobody can see.
pub const POLL: Duration = Duration::from_millis(500);

/// What a file was, the last time anyone looked.
///
/// Modified time and length, not content: this is the cheap first question
/// asked twice a second, and the expensive one — "is it REALLY different from
/// what the tab holds" — is asked by the window through the same content
/// stamp a save uses, only after this one says something moved. A same-length
/// write inside the filesystem's mtime resolution would slip past, and APFS
/// keeps nanoseconds, so the miss needs two writers in the same nanosecond.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Snap {
    modified: Option<SystemTime>,
    len: u64,
}

/// What `target` is right now, or `None` for "absent".
///
/// A directory answers `None` on the tabs lane: the watch list holds files
/// the window has open as text, and a path that BECAME a directory is not
/// that file anymore — the tab over it should hear "deleted", not watch the
/// directory's mtime. A lane that asked for directories (the artifact store's)
/// gets the folder's own stamp, which moves when an entry is added or removed.
fn snap(target: &Path, directories: bool) -> Option<Snap> {
    let meta = std::fs::metadata(target).ok()?;
    if !(meta.is_file() || (directories && meta.is_dir())) {
        return None;
    }
    Some(Snap {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

/// The lane the window's open tabs ride in — the one [`WatchSet::replace`]
/// speaks for. Other lanes ([`WatchSet::replace_lane`]) share the thread and
/// the poll and nothing else: a replacement on one never touches another.
pub const TABS_LANE: &str = "tabs";

/// One thing that happened to one watched file, as the window hears it.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
pub struct Change {
    /// `create`, `update` or `delete` — Orca's own three kinds
    /// (out/main/index.js:50803), minus `overflow`, which a bounded set
    /// cannot produce.
    pub kind: &'static str,
    /// Project-relative — the same spelling the window opened the tab by,
    /// so the tab is found by equality and never by re-resolving.
    pub path: String,
    /// Which lane the entry rode in. Not on the wire: the `fs:changed`
    /// payload is the tabs lane's and keeps Orca's shape; the other lanes
    /// are routed before anything is emitted.
    #[serde(skip)]
    pub lane: &'static str,
}

/// The `fs:changed` payload: every change one poll found, together.
///
/// A batch rather than one event per file, matching Orca's payload shape
/// (out/main/index.js:50861) — a checkout-wide operation like a branch switch
/// touches every open file at once, and one event per file is a repaint per
/// file.
#[derive(Serialize, Clone)]
pub struct FsChanged {
    pub events: Vec<Change>,
}

/// One watched file: the name the window knows it by, the place on disk it
/// resolved to, and what it looked like at the last poll.
struct Entry {
    lane: &'static str,
    path: String,
    target: PathBuf,
    held: Option<Snap>,
    /// Whether a directory at `target` counts as present (see [`snap`]).
    directory: bool,
}

/// The set of files being watched, shared between the command that replaces
/// it and the thread that polls it.
#[derive(Default)]
pub struct WatchSet {
    entries: Mutex<Vec<Entry>>,
    /// Rung when the set changes, so a poller parked on an empty set starts
    /// the moment the first file opens instead of a tick later.
    wake: Condvar,
}

impl WatchSet {
    /// Replace the whole set.
    ///
    /// The window sends every open file each time any tab opens or closes,
    /// rather than add/remove deltas: the set is derived state, and a full
    /// replacement cannot drift from what is actually open.
    ///
    /// Each path arrives with where it resolved to, or `None` when it did not
    /// resolve. A file that stops resolving mid-session is usually one that
    /// was deleted while its tab stayed open — its entry keeps the address it
    /// had, so the file coming BACK is still seen. A path that never resolved
    /// has no address to keep and is dropped: it cannot be watched, and the
    /// resolver refusing it is also what keeps an escaping path out of the
    /// set entirely.
    ///
    /// Replacing is never itself an event. A file that survives the
    /// replacement keeps its history; a new one is baselined at what it is
    /// right now, because the window read it as the tab opened and there is
    /// nothing older to compare against.
    pub fn replace(&self, next: Vec<(String, Option<PathBuf>)>) {
        self.replace_lane(TABS_LANE, next, false);
    }

    /// Replace one lane of the set, leaving every other lane as it was.
    ///
    /// The artifact store rides its own lane: its folders are registered by
    /// the backend, not sent by the window, and a tab opening must not drop
    /// them — nor may a store re-aiming its lane baseline a tab's file. With
    /// `directories`, a folder counts as present and its own stamp is what
    /// moves; the tabs lane never asks for that (see [`snap`]).
    pub fn replace_lane(
        &self,
        lane: &'static str,
        next: Vec<(String, Option<PathBuf>)>,
        directories: bool,
    ) {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let (mine, others): (Vec<Entry>, Vec<Entry>) = std::mem::take(&mut *entries)
            .into_iter()
            .partition(|held| held.lane == lane);
        let mut fresh: Vec<Entry> = next
            .into_iter()
            .filter_map(|(path, resolved)| {
                let previous = mine.iter().find(|held| held.path == path);
                let target = resolved.or_else(|| previous.map(|held| held.target.clone()))?;
                let held = match previous {
                    Some(prior) if prior.target == target => prior.held.clone(),
                    _ => snap(&target, directories),
                };
                Some(Entry {
                    lane,
                    path,
                    target,
                    held,
                    directory: directories,
                })
            })
            .collect();
        let mut kept = others;
        kept.append(&mut fresh);
        *entries = kept;
        self.wake.notify_all();
    }

    /// Look at every watched file once, and say what moved since the last
    /// look.
    ///
    /// The classification is the comparison of two answers to [`snap`]:
    /// absent→present is `create`, present→absent is `delete`, and two
    /// presents that differ are `update`. A file that stayed absent says
    /// nothing — a delete is reported once, not on every poll it remains
    /// deleted.
    fn poll(&self) -> Vec<Change> {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changes = Vec::new();
        for entry in entries.iter_mut() {
            let now = snap(&entry.target, entry.directory);
            let kind = match (&entry.held, &now) {
                (None, Some(_)) => "create",
                (Some(_), None) => "delete",
                (Some(was), Some(is)) if was != is => "update",
                _ => continue,
            };
            entry.held = now;
            changes.push(Change {
                kind,
                path: entry.path.clone(),
                lane: entry.lane,
            });
        }
        changes
    }

    /// The poller. Runs for the life of the window on its own thread.
    ///
    /// Parked while the set is empty — a window full of terminals pays
    /// nothing for this feature — and woken by [`Self::replace`] the moment the
    /// first file opens. Each tick waits `period` on the same condvar, so a
    /// set change interrupts the wait instead of landing a tick late; the
    /// poll after the wait reads whatever the set is by then.
    ///
    /// `tell` is called outside the lock, with only non-empty batches: an
    /// emit reaches into the webview, and holding the set's lock across that
    /// would stall the command replacing it.
    pub fn run(&self, period: Duration, tell: impl Fn(Vec<Change>)) {
        loop {
            {
                let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
                while entries.is_empty() {
                    entries = self
                        .wake
                        .wait(entries)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                let _tick = self
                    .wake
                    .wait_timeout(entries, period)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            let changes = self.poll();
            if !changes.is_empty() {
                tell(changes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(dir: &Path, name: &str) -> (String, Option<PathBuf>) {
        (name.to_string(), Some(dir.join(name)))
    }

    /// The window re-sends the whole set on every tab open and close. If the
    /// replacement reset baselines, every terminal opened would make every
    /// open file look freshly changed.
    #[test]
    fn replacing_the_set_is_never_itself_an_event() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "held").expect("write");
        let watch = WatchSet::default();
        watch.replace(vec![resolved(dir.path(), "a.txt")]);
        assert_eq!(watch.poll(), Vec::<Change>::new());
        watch.replace(vec![resolved(dir.path(), "a.txt")]);
        assert_eq!(
            watch.poll(),
            Vec::<Change>::new(),
            "re-sending the same set moved the baseline"
        );
    }

    /// The three kinds, told apart by the two snapshots alone — and a file
    /// that stays deleted is reported deleted once, not once per poll.
    #[test]
    fn a_rewrite_a_delete_and_a_return_each_say_what_happened() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        // The rewrite changes the length, so the comparison cannot depend on
        // the filesystem's mtime resolution to notice it.
        std::fs::write(&file, "one").expect("write");
        let watch = WatchSet::default();
        watch.replace(vec![resolved(dir.path(), "a.txt")]);

        std::fs::write(&file, "one, but longer").expect("rewrite");
        assert_eq!(
            watch.poll(),
            vec![Change {
                kind: "update",
                path: "a.txt".into(),
                lane: TABS_LANE,
            }]
        );

        std::fs::remove_file(&file).expect("remove");
        assert_eq!(
            watch.poll(),
            vec![Change {
                kind: "delete",
                path: "a.txt".into(),
                lane: TABS_LANE,
            }]
        );
        assert_eq!(
            watch.poll(),
            Vec::<Change>::new(),
            "a file that stays deleted is deleted once, not every poll"
        );

        std::fs::write(&file, "back").expect("recreate");
        assert_eq!(
            watch.poll(),
            vec![Change {
                kind: "create",
                path: "a.txt".into(),
                lane: TABS_LANE,
            }]
        );
    }

    /// A deleted file's tab stays open, and closing SOME OTHER tab re-sends
    /// the set — at which point the deleted file no longer resolves. Its
    /// entry must keep the address it had, or the file coming back is
    /// invisible.
    #[test]
    fn a_deleted_file_keeps_its_address_across_a_replace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "held").expect("write");
        let watch = WatchSet::default();
        watch.replace(vec![resolved(dir.path(), "a.txt")]);

        std::fs::remove_file(&file).expect("remove");
        assert_eq!(watch.poll().len(), 1, "the delete itself is heard");

        // The re-send, as the command would build it: the path no longer
        // resolves, so it arrives with no address.
        watch.replace(vec![("a.txt".to_string(), None)]);
        assert_eq!(watch.poll(), Vec::<Change>::new());

        std::fs::write(&file, "back").expect("recreate");
        assert_eq!(
            watch.poll(),
            vec![Change {
                kind: "create",
                path: "a.txt".into(),
                lane: TABS_LANE,
            }],
            "the return went unseen — the replace dropped the address"
        );
    }

    /// A path that never resolved is not watchable, and holding an entry for
    /// it would keep the poller awake statting nothing.
    #[test]
    fn a_path_that_never_resolved_is_not_watched() {
        let watch = WatchSet::default();
        watch.replace(vec![("ghost.txt".to_string(), None)]);
        assert!(
            watch
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty(),
            "an unresolvable path took a seat in the watch set"
        );
    }

    /// Two lanes on one set: the window replacing its tabs lane leaves the
    /// artifact lane's folders in place, a folder gaining a file is heard as
    /// an update on that lane, and the tabs lane still refuses directories.
    #[test]
    fn a_second_lane_survives_the_tabs_lane_being_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().join("store");
        std::fs::create_dir_all(&folder).expect("mkdir");
        std::fs::write(dir.path().join("a.txt"), "held").expect("write");
        let watch = WatchSet::default();
        watch.replace_lane(
            "artifacts",
            vec![("artifacts:root".to_string(), Some(folder.clone()))],
            true,
        );
        watch.replace(vec![resolved(dir.path(), "a.txt")]);
        watch.replace(vec![("store".to_string(), Some(folder.clone()))]);
        assert_eq!(
            watch
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .map(|held| held.lane)
                .collect::<Vec<_>>(),
            vec!["artifacts", TABS_LANE],
            "replacing the tabs lane touched the other lane, or a directory took a tabs seat"
        );
        // A folder's own stamp moves when a file lands in it.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(folder.join("new.md"), "landed").expect("write");
        let heard = watch.poll();
        assert_eq!(
            heard,
            vec![Change {
                kind: "update",
                path: "artifacts:root".into(),
                lane: "artifacts",
            }]
        );
    }

    /// The thread itself: parked on an empty set, woken by the first file,
    /// and reporting a rewrite within a tick of it landing.
    #[test]
    fn the_poller_wakes_for_the_first_file_and_reports_a_rewrite() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "held").expect("write");

        let watch = Arc::new(WatchSet::default());
        let (tell, told) = std::sync::mpsc::channel();
        let runner = Arc::clone(&watch);
        // Detached on purpose: `run` never returns, and the test binary
        // exiting is what takes it down — the same life it has in the app.
        std::thread::spawn(move || {
            runner.run(Duration::from_millis(20), move |changes| {
                let _ = tell.send(changes);
            });
        });

        watch.replace(vec![resolved(dir.path(), "a.txt")]);
        std::fs::write(&file, "held, then rewritten").expect("rewrite");

        let heard = told
            .recv_timeout(Duration::from_secs(5))
            .expect("the poller never reported the rewrite");
        assert_eq!(
            heard,
            vec![Change {
                kind: "update",
                path: "a.txt".into(),
                lane: TABS_LANE,
            }]
        );
    }
}
