//! What a deletion would take that git does not show: the paths it is told to
//! ignore (t-34315).
//!
//! `git status --ignored=matching` names them and collapses a directory it
//! ignores whole into one entry, so a built checkout reports `target/` and not
//! forty thousand files. A name is not yet a fact a person can weigh. This
//! module adds the two that are — how much those entries hold on disk, and
//! which are the biggest — so the evidence panel can say "270 MB: node_modules,
//! target, output" instead of a bare tag.
//!
//! **The measurement is bounded, and says so.** A walk over a dependency tree
//! can run for seconds on a slow disk, and this read is one a person waits
//! for. It stops at a number of entries or at a time, whichever comes first,
//! and the answer then says `complete: false`: the size is a floor and the
//! panel words it that way. A number held back for the sake of being exact
//! would be a window that waits.
//!
//! **Nothing here follows a link and nothing here writes.** A `node_modules`
//! that is a link into another checkout holds nothing of this one's, and a
//! link that points at its own parent would never end. Each listing already
//! reads an entry as the link it is ([`Fs::read_dir`]).

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use zerocode_core::host::{About, Fs, LocalFs};
use zerocode_core::workspace_space::{MAX_SCAN_ENTRIES, format_bytes};

use crate::worktree_evidence::clipped;

/// Entries the answer names, biggest first.
///
/// Three: what one line of the panel carries before it stops being a sentence.
/// The rest are counted (`count`, less the ones named) and the cleanup review
/// lists every one of them.
pub const MAX_IGNORED_NAMED: usize = 3;

/// How long one measurement may run.
///
/// A second and a half. The read is one a person waits for after pressing a
/// button, and a dependency tree on a slow disk is the one thing in it whose
/// cost no bound of git's covers. Past this the size is a floor, never a
/// reason to hold the window.
pub const IGNORED_SCAN_BUDGET: Duration = Duration::from_millis(1_500);

/// The clock is read once per this many entries and not once per entry: a
/// hundred thousand reads of it would be a cost of their own, and a deadline
/// missed by a few hundred entries is a deadline kept.
const CLOCK_STRIDE: usize = 256;

/// One ignored entry, named for a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IgnoredEntry {
    /// Repository-relative, as git wrote it, without the separator git marks a
    /// directory with — `dir` says it instead.
    pub name: String,
    /// Bytes it occupies on disk, counted in blocks as `du` counts: what
    /// deleting it frees. A directory counts everything under it, as far as
    /// the walk got.
    pub bytes: u64,
    /// `bytes` written for a person, by the one rule the window's space scan
    /// uses ([`format_bytes`]), so the two screens never spell one size two
    /// ways.
    pub size_text: String,
    /// Whether it is a directory. A link is not one, whatever it points at.
    pub dir: bool,
}

/// The ignored paths of one checkout, counted, sized and named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IgnoredFacts {
    /// Entries git listed. A directory it ignores whole is ONE entry however
    /// many files it holds, so this is not a file count.
    pub count: usize,
    /// Bytes those entries occupy on disk, as far as the walk got.
    pub bytes: u64,
    /// `bytes` written for a person.
    pub size_text: String,
    /// Whether every entry was measured to its end. `false` — the walk stopped
    /// at a bound, a directory could not be listed, or a name was not found on
    /// disk — makes `bytes` a floor: the panel says "or more", and nothing may
    /// read it as the exact size.
    pub complete: bool,
    /// The biggest entries, biggest first, at most [`MAX_IGNORED_NAMED`].
    pub top: Vec<IgnoredEntry>,
}

impl IgnoredFacts {
    /// A checkout that holds no ignored path. The size is exactly nothing, so
    /// it is `complete`: nothing was left unread.
    #[must_use]
    pub fn none() -> Self {
        Self {
            count: 0,
            bytes: 0,
            size_text: format_bytes(0),
            complete: true,
            top: Vec::new(),
        }
    }
}

/// What one measurement may still spend: entries, and time.
struct Allowance {
    deadline: Instant,
    entries_left: usize,
    spent: usize,
}

impl Allowance {
    fn new(budget: Duration, entries: usize) -> Self {
        let now = Instant::now();
        Self {
            deadline: now.checked_add(budget).unwrap_or(now),
            entries_left: entries,
            spent: 0,
        }
    }

    /// Pay for one entry. `false` once the entries or the time are gone — and
    /// from then on, so a stopped walk stays stopped.
    fn afford(&mut self) -> bool {
        if self.entries_left == 0 {
            return false;
        }
        if self.spent.is_multiple_of(CLOCK_STRIDE) && Instant::now() >= self.deadline {
            self.entries_left = 0;
            return false;
        }
        self.entries_left -= 1;
        self.spent += 1;
        true
    }
}

/// Count, size and name `ignored` — the paths `git status --ignored=matching`
/// listed for the checkout at `root`.
#[must_use]
pub fn measure(root: &Path, ignored: &[String]) -> IgnoredFacts {
    measure_within(
        &LocalFs,
        root,
        ignored,
        Allowance::new(IGNORED_SCAN_BUDGET, MAX_SCAN_ENTRIES),
    )
}

fn measure_within(
    fs: &dyn Fs,
    root: &Path,
    ignored: &[String],
    mut allowance: Allowance,
) -> IgnoredFacts {
    if ignored.is_empty() {
        return IgnoredFacts::none();
    }
    let mut parents = Parents::default();
    let mut sized: Vec<IgnoredEntry> = Vec::with_capacity(ignored.len());
    // The budget is gone: nothing more is walked.
    let mut stopped = false;
    // Something was not measured, so the size is a floor.
    let mut unmeasured = false;
    for name in ignored {
        let about = parents.about(fs, root, name);
        let dir = about.is_some_and(|held| held.is_dir);
        let mut bytes = about.map_or(0, |held| held.disk_bytes);
        // A name the listing of its directory does not hold — gone since git
        // looked, or spelled another way than the disk spells it (a decomposed
        // Korean name on macOS) — has no size to give, and says so.
        unmeasured |= about.is_none();
        // One entry of the budget for each name, so a list of ten thousand
        // ignored files is as bounded as a tree of them. A file's size needs no
        // walk — its listing already said it — so a spent budget leaves it exact.
        stopped |= !allowance.afford();
        if dir {
            if stopped {
                unmeasured = true;
            } else {
                let walked = walk(fs, &root.join(name), &mut allowance);
                bytes = bytes.saturating_add(walked.bytes);
                unmeasured |= walked.unread || walked.stopped;
                stopped |= walked.stopped;
            }
        }
        sized.push(IgnoredEntry {
            name: clipped(name),
            bytes,
            size_text: format_bytes(bytes),
            dir,
        });
    }
    let bytes = sized
        .iter()
        .fold(0_u64, |total, one| total.saturating_add(one.bytes));
    sized.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.name.cmp(&right.name))
    });
    sized.truncate(MAX_IGNORED_NAMED);
    IgnoredFacts {
        count: ignored.len(),
        bytes,
        size_text: format_bytes(bytes),
        complete: !unmeasured,
        top: sized,
    }
}

/// How far one walk got.
struct Walked {
    /// Bytes counted under the directory.
    bytes: u64,
    /// A directory could not be listed, or an entry could not be described, so
    /// what it holds is not in `bytes`.
    unread: bool,
    /// The budget ran out before the end.
    stopped: bool,
}

/// Bytes under `dir`, and how far the walk got.
///
/// A stack and not a recursion: a deep tree would overflow this thread's stack
/// and take the window with it. Only the unexplored siblings along the path
/// being walked are held, not the tree.
///
/// A directory that cannot be listed counts as nothing, as it does in the
/// window's space scan — one folder the process may not open must not make the
/// rest of a nine-gigabyte tree unmeasurable — and the answer says so
/// (`unread`), which makes the size a floor. A directory is listed whole before
/// it is paid for (`Fs::read_dir` hands back its entries at once), so the
/// bounds cut between directories and not inside one: the limit the window's
/// space scan has too.
fn walk(fs: &dyn Fs, dir: &Path, allowance: &mut Allowance) -> Walked {
    let mut walked = Walked {
        bytes: 0,
        unread: false,
        stopped: false,
    };
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(listing) = fs.read_dir(&next) else {
            walked.unread = true;
            continue;
        };
        for found in listing {
            if !allowance.afford() {
                walked.stopped = true;
                return walked;
            }
            let Some(about) = found.about else {
                walked.unread = true;
                continue;
            };
            if about.is_dir {
                pending.push(found.path);
            } else {
                walked.bytes = walked.bytes.saturating_add(about.disk_bytes);
            }
        }
    }
    walked
}

/// The facts of an entry, read from the listing of the directory it sits in.
///
/// git names ignored paths in its own order, so the ones under one directory
/// arrive together; the last listing is kept and the next name under the same
/// directory costs a lookup, not another listing. The listing is also how an
/// entry's own size is read at all: [`Fs`] has no stat of a single path, and
/// adding one would be a second way to answer what `read_dir` already does.
#[derive(Default)]
struct Parents {
    held: Option<(PathBuf, HashMap<OsString, About>)>,
}

impl Parents {
    fn about(&mut self, fs: &dyn Fs, root: &Path, name: &str) -> Option<About> {
        let path = Path::new(name);
        let parent = match path.parent() {
            Some(inside) if !inside.as_os_str().is_empty() => root.join(inside),
            _ => root.to_path_buf(),
        };
        let leaf = path.file_name()?;
        if self.held.as_ref().map(|(at, _)| at) != Some(&parent) {
            self.held = fs.read_dir(&parent).ok().map(|listing| {
                let by_name = listing
                    .into_iter()
                    .filter_map(|found| {
                        Some((found.path.file_name()?.to_os_string(), found.about?))
                    })
                    .collect();
                (parent, by_name)
            });
        }
        self.held.as_ref()?.1.get(leaf).copied()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use zerocode_core::host::{DirEntry, ReadDir};

    use super::*;

    /// A tree held in memory: the bounds can be tested without writing a
    /// hundred thousand files, and the listings can be counted.
    #[derive(Default)]
    struct Tree {
        dirs: HashMap<PathBuf, Vec<DirEntry>>,
        listed: AtomicUsize,
    }

    impl Tree {
        fn file(&mut self, path: &str, bytes: u64) -> &mut Self {
            self.put(path, false, bytes)
        }

        /// A directory's own blocks, which the walk of its parent counts.
        fn dir(&mut self, path: &str) -> &mut Self {
            self.put(path, true, 4096)
        }

        /// A directory the fixture has no listing for: it answers `NoAccess`, as
        /// one the process may not open does.
        fn locked(&mut self, path: &str) -> &mut Self {
            self.dir(path)
        }

        fn put(&mut self, path: &str, is_dir: bool, disk_bytes: u64) -> &mut Self {
            let path = PathBuf::from(path);
            let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
            self.dirs.entry(parent).or_default().push(DirEntry {
                path,
                about: Some(About { is_dir, disk_bytes }),
            });
            self
        }

        fn listings(&self) -> usize {
            self.listed.load(Ordering::Relaxed)
        }
    }

    impl Fs for Tree {
        fn read_dir(&self, dir: &Path) -> Result<Vec<DirEntry>, ReadDir> {
            self.listed.fetch_add(1, Ordering::Relaxed);
            self.dirs.get(dir).cloned().ok_or(ReadDir::NoAccess)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.contains_key(path)
        }

        fn read_to_string(&self, _path: &Path) -> Option<String> {
            None
        }

        fn modified_ms(&self, _path: &Path) -> Option<i64> {
            None
        }
    }

    fn names(ignored: &[&str]) -> Vec<String> {
        ignored.iter().map(ToString::to_string).collect()
    }

    fn unbounded() -> Allowance {
        Allowance::new(Duration::from_secs(3_600), usize::MAX)
    }

    #[test]
    fn a_directory_counts_everything_under_it_and_the_biggest_come_first() {
        let mut tree = Tree::default();
        tree.file("/checkout/.env", 100)
            .dir("/checkout/node_modules")
            .file("/checkout/node_modules/a.js", 1_000)
            .dir("/checkout/node_modules/pkg")
            .file("/checkout/node_modules/pkg/b.js", 2_000)
            .dir("/checkout/target")
            .file("/checkout/target/out.bin", 50_000)
            .file("/checkout/notes.log", 300);

        let facts = measure_within(
            &tree,
            Path::new("/checkout"),
            &names(&["node_modules", "target", ".env", "notes.log"]),
            unbounded(),
        );

        assert_eq!(facts.count, 4);
        assert!(facts.complete);
        // A directory's own blocks and the files below it; the sub-directory's
        // own blocks are not counted, as in the window's space scan.
        let total = (4_096 + 50_000) + (4_096 + 1_000 + 2_000) + 300 + 100;
        assert_eq!(facts.bytes, total);
        assert_eq!(facts.size_text, format_bytes(total));
        let named: Vec<(&str, bool)> = facts
            .top
            .iter()
            .map(|one| (one.name.as_str(), one.dir))
            .collect();
        assert_eq!(
            named,
            vec![
                ("target", true),
                ("node_modules", true),
                ("notes.log", false)
            ],
            "{facts:?}"
        );
        assert_eq!(facts.top[0].size_text, format_bytes(54_096));
    }

    #[test]
    fn names_under_a_tracked_directory_are_read_from_their_own_parent_once() {
        let mut tree = Tree::default();
        tree.dir("/checkout/ui")
            .file("/checkout/ui/.DS_Store", 6_000)
            .dir("/checkout/ui/cache")
            .file("/checkout/ui/cache/x", 10);

        let facts = measure_within(
            &tree,
            Path::new("/checkout"),
            &names(&["ui/.DS_Store", "ui/cache"]),
            unbounded(),
        );

        assert_eq!(facts.bytes, 6_000 + 4_096 + 10, "{facts:?}");
        assert_eq!(facts.top[0].name, "ui/.DS_Store");
        // `ui/` once for both names, then `ui/cache` once to walk it.
        assert_eq!(tree.listings(), 2);
    }

    #[test]
    fn a_walk_out_of_entries_says_its_size_is_a_floor_and_leaves_the_rest_unwalked() {
        let mut tree = Tree::default();
        tree.dir("/checkout/big").dir("/checkout/later");
        for at in 0..10 {
            tree.file(&format!("/checkout/big/f{at}"), 1_000);
        }
        tree.file("/checkout/later/g", 7_000);

        // Five entries of budget: one for `big` and four of the ten files below
        // it, so `big` is cut short and `later` is never reached.
        let facts = measure_within(
            &tree,
            Path::new("/checkout"),
            &names(&["big", "later"]),
            Allowance::new(Duration::from_secs(3_600), 5),
        );

        assert!(!facts.complete, "{facts:?}");
        assert_eq!(facts.count, 2);
        // What was counted is a floor: the directory's own blocks are always
        // known, the files read before the stop add to them, and `later` was
        // not walked, so its 7,000 bytes are not in.
        assert!(facts.bytes >= 4_096 + 4_096, "{facts:?}");
        assert!(facts.bytes < 4_096 + 10_000 + 4_096 + 7_000, "{facts:?}");
        let later = facts.top.iter().find(|one| one.name == "later");
        assert_eq!(later.map(|one| one.bytes), Some(4_096), "{facts:?}");
    }

    #[test]
    fn a_walk_out_of_time_says_its_size_is_a_floor() {
        let mut tree = Tree::default();
        tree.dir("/checkout/big").file("/checkout/big/f", 9_000);

        let facts = measure_within(
            &tree,
            Path::new("/checkout"),
            &names(&["big"]),
            Allowance::new(Duration::ZERO, usize::MAX),
        );

        assert!(!facts.complete, "{facts:?}");
        assert_eq!(
            facts.bytes, 4_096,
            "only what the listing of its parent said"
        );
        assert_eq!(facts.count, 1);
    }

    #[test]
    fn a_directory_that_cannot_be_listed_counts_as_nothing_says_so_and_stops_nobody() {
        let mut tree = Tree::default();
        tree.locked("/checkout/locked")
            .dir("/checkout/open")
            .file("/checkout/open/f", 5_000);

        let facts = measure_within(
            &tree,
            Path::new("/checkout"),
            &names(&["locked", "open"]),
            unbounded(),
        );

        // What the locked directory holds is unknown, so the size is a floor;
        // the open one beside it was still counted.
        assert!(!facts.complete, "{facts:?}");
        assert_eq!(facts.bytes, 4_096 + 4_096 + 5_000, "{facts:?}");
    }

    #[test]
    fn a_name_the_listing_does_not_hold_is_an_entry_with_no_size_and_a_floor() {
        let mut tree = Tree::default();
        tree.file("/checkout/kept", 10);

        let facts = measure_within(
            &tree,
            Path::new("/checkout"),
            &names(&["vanished"]),
            unbounded(),
        );

        assert_eq!(facts.count, 1);
        assert_eq!(facts.bytes, 0);
        assert!(!facts.top[0].dir);
        assert!(!facts.complete, "nothing was measured, so nothing is exact");
    }

    #[test]
    fn a_file_needs_no_walk_so_a_spent_budget_still_sizes_it_exactly() {
        let mut tree = Tree::default();
        tree.file("/checkout/f", 7_000);

        let facts = measure_within(
            &tree,
            Path::new("/checkout"),
            &names(&["f"]),
            Allowance::new(Duration::ZERO, usize::MAX),
        );

        assert!(facts.complete, "{facts:?}");
        assert_eq!(facts.bytes, 7_000);
    }

    #[test]
    fn nothing_ignored_is_none_and_complete() {
        let facts = measure_within(&Tree::default(), Path::new("/checkout"), &[], unbounded());
        assert_eq!(facts, IgnoredFacts::none());
        assert_eq!(facts.count, 0);
        assert!(facts.complete);
        assert!(facts.top.is_empty());
    }

    #[test]
    fn only_the_biggest_are_named_and_a_tie_goes_by_name() {
        let mut tree = Tree::default();
        for name in ["d", "c", "b", "a"] {
            tree.file(&format!("/checkout/{name}"), 8_192);
        }
        let facts = measure_within(
            &tree,
            Path::new("/checkout"),
            &names(&["d", "c", "b", "a"]),
            unbounded(),
        );
        let named: Vec<&str> = facts.top.iter().map(|one| one.name.as_str()).collect();
        assert_eq!(named, vec!["a", "b", "c"]);
        assert_eq!(facts.count, 4);
        assert_eq!(facts.bytes, 4 * 8_192);
    }

    /// The real filesystem, and the one rule that keeps a measurement honest:
    /// a link is the link, not what it points at.
    #[cfg(unix)]
    #[test]
    fn a_link_into_another_tree_holds_nothing_of_this_one() {
        let elsewhere = tempfile::tempdir().expect("another tree");
        std::fs::write(elsewhere.path().join("big.bin"), vec![b'x'; 1 << 20]).expect("big file");
        let root = tempfile::tempdir().expect("a checkout");
        std::os::unix::fs::symlink(elsewhere.path(), root.path().join("node_modules"))
            .expect("a link");

        let facts = measure(root.path(), &names(&["node_modules"]));

        assert!(facts.complete, "{facts:?}");
        assert!(
            facts.bytes < 1 << 20,
            "the link was followed: {} bytes",
            facts.bytes
        );
        assert!(!facts.top[0].dir, "a link is not a directory: {facts:?}");
    }
}
