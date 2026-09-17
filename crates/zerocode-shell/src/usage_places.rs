//! Where a turn happened, and whether it happened somewhere this window knows.
//!
//! A transcript records the directory an agent was working in, and nothing
//! else about the place. Two questions are asked of that one string: which
//! **project** to file the turn under, and whether the place is one of the
//! workspaces this window manages — the second is what the usage screen's
//! 이 앱 scope is, and without it the screen can only ever say "everything on
//! this machine".
//!
//! Ported from Orca's `worktree-attribution.ts`, whose three rules are the
//! whole of it: an exact match wins, otherwise the **longest** known path that
//! contains the directory wins (`:59-66`, entries sorted by length descending
//! at `:75-77`), and a directory nothing contains is filed under itself
//! (`:135`). The longest-first order is not a nicety — a repository and a
//! worktree inside it are both known paths, and first-match-wins on an
//! unsorted list would file the worktree's turns under the repository.
//!
//! What is deliberately NOT here is a human word for the unknown case. Orca
//! writes `'Unknown location'` into the record itself (`:19-27`); a label
//! baked in back here would reach the window already in one language, and the
//! window's own gate forbids that. [`Place::label`] is `None` instead and the
//! window says the word in the reader's language.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One place a turn can be filed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Place {
    /// The key rows are grouped by — stable across scans and unique per place.
    ///
    /// `worktree:<path>` for a workspace this window knows, `cwd:<path>` for a
    /// directory it does not, and `unscoped` for a turn whose record named no
    /// directory at all. Orca's own three shapes (`worktree-attribution.ts:129`,
    /// `:137`, `:107`), with our worktree's path standing in for its id — a
    /// path IS the identity of a checkout here.
    pub(crate) key: String,
    /// What to call it, or `None` when there is nothing to call it but a word
    /// the window owns. Never a translatable sentence — see the module note.
    pub(crate) label: Option<String>,
    /// Whether this is one of the workspaces this window manages.
    pub(crate) ours: bool,
}

/// The key a turn with no directory on its record is filed under.
const UNSCOPED_KEY: &str = "unscoped";

/// Where that place always sits, so a record naming no directory needs no
/// branch of its own anywhere downstream.
const UNSCOPED_AT: u32 = 0;

/// How many path segments name an unknown directory.
///
/// Orca's `getDefaultProjectLabel` takes the last two (`worktree-attribution.ts:24`)
/// — enough that `2026/zerocode` and `2025/zerocode` read apart, short enough
/// that a deep path does not push the numbers off the row.
const UNKNOWN_LABEL_SEGMENTS: usize = 2;

/// Every workspace this window knows, ready to be asked about a directory.
///
/// Built once per scan and asked once per distinct directory — a corpus
/// repeats the same `cwd` for thousands of consecutive turns, which is why the
/// answers are memoised here rather than recomputed per turn (Orca caches the
/// same way, `:114-121`).
///
/// Asked for an INDEX rather than a place, and that is not a detail: [`of`] is
/// called once per distinct message — 56,669 times on this machine's own
/// corpus — and a `Place` handed back by value is two `String` clones each
/// time. The index is four bytes, it keys the rollup maps directly, and the
/// text is fetched once at the end when the rows are built.
///
/// [`of`]: Places::of
#[derive(Debug)]
pub(crate) struct Places {
    /// Known workspace roots, normalised, LONGEST FIRST, each naming a row of
    /// [`Places::held`]. See the module note for why the order is load-bearing.
    known: Vec<(String, u32)>,
    /// Every place this lookup has ever named, by index.
    held: Vec<Place>,
    /// Answers already given, by the directory string as the record wrote it.
    answered: HashMap<String, u32>,
}

impl Places {
    /// A lookup over the workspaces named by `roots`, each with the word the
    /// sidebar calls it by.
    pub(crate) fn new(roots: impl IntoIterator<Item = (PathBuf, String)>) -> Self {
        // Index 0 is always the nowhere-named place, so a record with no
        // directory needs no branch anywhere downstream.
        let mut held = vec![Place {
            key: UNSCOPED_KEY.to_string(),
            label: None,
            ours: false,
        }];
        let mut known: Vec<(String, u32)> = Vec::new();
        for (path, name) in roots {
            let normal = normalize(&path);
            if known.iter().any(|(root, _)| *root == normal) {
                continue;
            }
            let at = u32::try_from(held.len()).unwrap_or(u32::MAX);
            held.push(Place {
                key: format!("worktree:{normal}"),
                label: Some(name),
                ours: true,
            });
            known.push((normal, at));
        }
        // Longest first, and the path breaks a tie so two runs answer alike.
        known.sort_by(|left, right| {
            right
                .0
                .len()
                .cmp(&left.0.len())
                .then_with(|| left.0.cmp(&right.0))
        });
        Self {
            known,
            held,
            answered: HashMap::new(),
        }
    }

    /// Which place `cwd` names — `None` on the record meaning "nowhere named".
    pub(crate) fn of(&mut self, cwd: Option<&str>) -> u32 {
        let Some(cwd) = cwd.filter(|held| !held.is_empty()) else {
            return UNSCOPED_AT;
        };
        if let Some(held) = self.answered.get(cwd) {
            return *held;
        }
        let answer = self.decide(cwd);
        self.answered.insert(cwd.to_string(), answer);
        answer
    }

    /// The place an index stands for.
    pub(crate) fn at(&self, index: u32) -> &Place {
        self.held
            .get(index as usize)
            .unwrap_or(&self.held[UNSCOPED_AT as usize])
    }

    fn decide(&mut self, cwd: &str) -> u32 {
        // The directory as git and the sidebar would write it. A transcript
        // records the path the agent was started with, which may walk a
        // symlink the catalogue's own path does not — resolving both sides is
        // the only way `/tmp/...` and `/private/tmp/...` meet (Orca resolves
        // with `realpath` for the same reason, `:29-36`).
        let normal = normalize(Path::new(cwd));
        let resolved = std::fs::canonicalize(cwd)
            .map(|held| normalize(&held))
            .unwrap_or_else(|_| normal.clone());
        for (root, at) in &self.known {
            if contains(root, &resolved) || contains(root, &normal) {
                return *at;
            }
        }
        let key = format!("cwd:{resolved}");
        if let Some(at) = self.held.iter().position(|place| place.key == key) {
            return u32::try_from(at).unwrap_or(UNSCOPED_AT);
        }
        let at = u32::try_from(self.held.len()).unwrap_or(UNSCOPED_AT);
        self.held.push(Place {
            key,
            label: Some(tail_label(&resolved)),
            ours: false,
        });
        at
    }
}

/// One path, written the one way this module compares paths.
///
/// Separators forward, no trailing slash, and case folded only where the
/// platform folds it — Orca's `normalizeComparablePath` (`:38-41`), whose
/// Windows branch exists because two spellings of one directory are one
/// directory there and two rows here.
fn normalize(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let trimmed = text.trim_end_matches('/');
    let held = if trimmed.is_empty() { &text } else { trimmed };
    if cfg!(windows) {
        held.to_lowercase()
    } else {
        held.to_string()
    }
}

/// Whether `child` is `parent` or sits under it.
///
/// Segment-wise, not a bare prefix: `/a/repo-two` starts with `/a/repo` and is
/// a different repository (Orca guards the same way with its `${parent}/`
/// suffix, `:43-47`).
fn contains(parent: &str, child: &str) -> bool {
    child == parent || child.starts_with(&format!("{parent}/"))
}

/// The last [`UNKNOWN_LABEL_SEGMENTS`] segments of a path nobody claimed.
fn tail_label(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() >= UNKNOWN_LABEL_SEGMENTS {
        parts[parts.len() - UNKNOWN_LABEL_SEGMENTS..].join("/")
    } else {
        parts
            .last()
            .map_or_else(|| path.to_string(), |last| (*last).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places() -> Places {
        Places::new([
            (PathBuf::from("/work/repo"), "main".to_string()),
            (
                PathBuf::from("/work/repo/.zo/feature"),
                "feature".to_string(),
            ),
            (PathBuf::from("/work/repo-two"), "other".to_string()),
        ])
    }

    /// The rule the sort exists for: a workspace nested inside a repository is
    /// filed under the workspace, not under the repository that contains it.
    #[test]
    fn the_longest_known_path_wins_over_the_one_that_merely_contains_it() {
        let mut known = places();
        let inner = known.of(Some("/work/repo/.zo/feature/src"));
        assert_eq!(known.at(inner).key, "worktree:/work/repo/.zo/feature");
        assert_eq!(known.at(inner).label.as_deref(), Some("feature"));
        assert!(known.at(inner).ours);
        let outer = known.of(Some("/work/repo/src"));
        assert_eq!(known.at(outer).key, "worktree:/work/repo");
        assert_eq!(known.at(outer).label.as_deref(), Some("main"));
    }

    /// A sibling whose name merely starts with a known one is not inside it.
    #[test]
    fn a_name_that_starts_the_same_is_not_the_same_place() {
        let mut known = places();
        let sibling = known.of(Some("/work/repo-two/src"));
        assert_eq!(known.at(sibling).key, "worktree:/work/repo-two");
        let stranger = known.of(Some("/work/repository"));
        assert_eq!(known.at(stranger).key, "cwd:/work/repository");
        assert!(!known.at(stranger).ours);
    }

    /// An unknown directory keeps enough of itself to be told from its
    /// neighbours, and a record with no directory at all says so.
    #[test]
    fn a_place_nobody_claims_is_filed_under_its_own_last_two_segments() {
        let mut known = places();
        let outside = known.of(Some("/Users/someone/2026/zerocode"));
        assert_eq!(known.at(outside).key, "cwd:/Users/someone/2026/zerocode");
        assert_eq!(known.at(outside).label.as_deref(), Some("2026/zerocode"));
        let nowhere = known.of(None);
        assert_eq!(nowhere, UNSCOPED_AT);
        assert_eq!(known.at(nowhere).key, UNSCOPED_KEY);
        // The window owns this word, not the backend — see the module note.
        assert_eq!(known.at(nowhere).label, None);
        assert!(!known.at(nowhere).ours);
        assert_eq!(known.of(Some("")), UNSCOPED_AT);
    }

    /// The same directory is decided once however often it is asked about —
    /// a corpus repeats one `cwd` for thousands of consecutive turns — and two
    /// spellings of one unknown directory are still one row.
    #[test]
    fn one_directory_is_decided_once_and_remembered() {
        let mut known = places();
        let first = known.of(Some("/elsewhere/scratch"));
        let again = known.of(Some("/elsewhere/scratch"));
        assert_eq!(first, again);
        assert_eq!(known.answered.len(), 1);
        // A second spelling of the same directory finds the row already there
        // rather than minting a second one beside it.
        let trailing = known.of(Some("/elsewhere/scratch/"));
        assert_eq!(trailing, first);
        assert_eq!(known.held.len(), 5, "an unknown place was minted twice");
    }

    /// Two spellings of one root are one row, so a repository listed twice
    /// cannot answer twice.
    #[test]
    fn a_root_listed_twice_is_one_row() {
        let known = Places::new([
            (PathBuf::from("/work/repo"), "main".to_string()),
            (PathBuf::from("/work/repo/"), "main again".to_string()),
        ]);
        assert_eq!(known.known.len(), 1);
        assert_eq!(known.at(1).label.as_deref(), Some("main"));
    }
}
