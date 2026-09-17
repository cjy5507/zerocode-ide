//! The PR depth's numbers, memory and cache (t-2733).
//!
//! Three small responsibilities that every surface of the PR depth leans on
//! and none should own: the ONE numbers table with its settings overlay, what
//! CI last said per checkout — so "it changed" is a compared fact and the
//! mail an agent gets is written only then — and the whole-PR diff the files
//! tab reads one file at a time, cached per PR under a byte cap so switching
//! files costs no second `gh`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use zerocode_core::checks::{Limits, OVERLAY_PREFIX, StackOrder, stack_order};

use crate::*;

/// The table, with the saved overlay laid over it. A settings file that will
/// not read answers the table alone — the numbers exist without a person.
pub(crate) fn limits(state: &AppState) -> Limits {
    load_settings(state.settings())
        .map(|snapshot| limits_of(&snapshot.document.checks))
        .unwrap_or_default()
}

/// The overlay's road from the settings document: `{"poll_ms": 6000}` under
/// the document's `checks` object is `checks.poll_ms` to the table
/// (`u64_overlay` is the one reader every numbers table shares).
pub(crate) fn limits_of(overlay: &serde_json::Value) -> Limits {
    let map: BTreeMap<String, u64> = u64_overlay(OVERLAY_PREFIX, overlay);
    Limits::default().overlaid(&map)
}

/// The ledger address of everybody seated in a checkout — the group the
/// ledger already resolves (`@worktree:<path>`), spelled once.
pub(crate) fn worktree_address(root: &Path) -> String {
    format!("@worktree:{}", root.display())
}

/// One durable observer for the background beat and the Checks panel.
pub(crate) fn remembered() -> &'static Mutex<crate::scm_observer::Observer> {
    static HELD: OnceLock<Mutex<crate::scm_observer::Observer>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(crate::scm_observer::Observer::default()))
}

pub(crate) fn note_checks(
    app: &tauri::AppHandle,
    root: &Path,
    review: &gh::HostedReview,
    ci: &zerocode_core::scm_observer::Ci,
    limits: &Limits,
) -> Result<(), String> {
    crate::scm_observer::note_checks(app, root, review, ci, limits)
}

/// The stack the panel's review is on, for the map: the order and the
/// reviews it names. `None` when the open reviews could not be read — the
/// panel then draws no map rather than a wrong one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PrStack {
    pub(crate) order: StackOrder,
    pub(crate) reviews: Vec<gh::StackReview>,
}

pub(crate) fn stack_for(
    root: &Path,
    review: &gh::HostedReview,
    limits: &Limits,
) -> Option<PrStack> {
    let reviews = gh::fetch_stack_reviews(root, &review.owner_repo, limits.stack_prs_max).ok()?;
    let links: Vec<_> = reviews.iter().map(gh::StackReview::link).collect();
    Some(PrStack {
        order: stack_order(review.number, &links),
        reviews,
    })
}

/// Whole PR diffs, keyed by (checkout, number), oldest PR out first when the
/// bytes run past the table's cap.
pub(crate) struct DiffLru {
    order: VecDeque<(PathBuf, u64)>,
    held: HashMap<(PathBuf, u64), String>,
    bytes: u64,
}

impl DiffLru {
    pub(crate) fn new() -> Self {
        Self {
            order: VecDeque::new(),
            held: HashMap::new(),
            bytes: 0,
        }
    }

    /// The cached diff, freshened to the back of the line.
    pub(crate) fn get(&mut self, root: &Path, number: u64) -> Option<String> {
        let key = (root.to_path_buf(), number);
        let text = self.held.get(&key)?.clone();
        self.order.retain(|held| held != &key);
        self.order.push_back(key);
        Some(text)
    }

    /// Keep a diff, evicting the oldest PRs until the cap holds. A diff past
    /// `diff_bytes_max` is not kept at all: one PR must not be the whole
    /// cache, and reading it again per file is the honest cost of its size.
    pub(crate) fn put(&mut self, root: &Path, number: u64, text: String, limits: &Limits) {
        let size = text.len() as u64;
        if size > limits.diff_bytes_max {
            return;
        }
        let key = (root.to_path_buf(), number);
        if let Some(old) = self.held.remove(&key) {
            self.bytes -= old.len() as u64;
            self.order.retain(|held| held != &key);
        }
        while self.bytes + size > limits.diff_cache_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(gone) = self.held.remove(&oldest) {
                self.bytes -= gone.len() as u64;
            }
        }
        self.bytes += size;
        self.held.insert(key.clone(), text);
        self.order.push_back(key);
    }
}

fn diff_cache() -> &'static Mutex<DiffLru> {
    static HELD: OnceLock<Mutex<DiffLru>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(DiffLru::new()))
}

/// The whole diff of one PR, from the cache or from `gh` once.
pub(crate) fn pr_diff(root: &Path, number: u64, limits: &Limits) -> Result<String, gh::GhError> {
    if let Some(held) = diff_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(root, number)
    {
        return Ok(held);
    }
    let text = gh::fetch_pr_diff(root, number)?;
    diff_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .put(root, number, text.clone(), limits);
    Ok(text)
}

/// One file's section of a whole unified diff: from its `diff --git` header
/// to the next file's. Matched on the `b/` side, so a renamed file is found
/// under the name the files list shows.
pub(crate) fn file_section<'a>(diff: &'a str, path: &str) -> Option<&'a str> {
    let wanted = format!(" b/{path}");
    let mut start = None;
    let mut at = 0usize;
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if let Some(from) = start {
                return Some(&diff[from..at]);
            }
            if line.trim_end_matches(['\n', '\r']).ends_with(&wanted) {
                start = Some(at);
            }
        }
        at += line.len();
    }
    start.map(|from| &diff[from..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test-only door onto the cache, beside the tests so the shipped half
    /// of this file ends where they begin.
    impl DiffLru {
        fn len(&self) -> usize {
            self.held.len()
        }
    }

    /// Oldest PR out first past the cap, a touched PR is young again, and a
    /// diff over the single-PR ceiling is never kept.
    #[test]
    fn the_diff_cache_forgets_the_oldest_pr_past_its_bytes() {
        let limits = Limits {
            diff_cache_bytes: 10,
            diff_bytes_max: 6,
            ..Limits::default()
        };
        let root = Path::new("/wt/repo");
        let mut cache = DiffLru::new();
        cache.put(root, 1, "aaaa".into(), &limits);
        cache.put(root, 2, "bbbb".into(), &limits);
        assert_eq!(cache.len(), 2);
        // Touch 1 so 2 is the oldest, then push past the cap.
        assert_eq!(cache.get(root, 1).as_deref(), Some("aaaa"));
        cache.put(root, 3, "cccc".into(), &limits);
        assert_eq!(
            cache.get(root, 2),
            None,
            "the untouched PR should have gone"
        );
        assert_eq!(cache.get(root, 1).as_deref(), Some("aaaa"));
        assert_eq!(cache.get(root, 3).as_deref(), Some("cccc"));
        cache.put(root, 4, "seven77".into(), &limits);
        assert_eq!(cache.get(root, 4), None, "a diff over the ceiling was kept");
        assert_eq!(cache.len(), 2);
    }

    /// A file's section runs from its header to the next file's, a rename is
    /// found under its new name, and an unknown path is `None`.
    #[test]
    fn a_files_section_is_cut_from_the_whole_diff() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n\
                    diff --git a/old.rs b/new.rs\nsimilarity index 90%\nrename from old.rs\nrename to new.rs\n";
        let first = file_section(diff, "src/a.rs").expect("first file");
        assert!(first.starts_with("diff --git a/src/a.rs"));
        assert!(
            first.ends_with("+new\n"),
            "the section ran into the next file: {first:?}"
        );
        let renamed = file_section(diff, "new.rs").expect("renamed file");
        assert!(renamed.ends_with("rename to new.rs\n"));
        assert_eq!(file_section(diff, "src/a.r"), None, "a prefix matched");
        assert_eq!(file_section(diff, "missing.rs"), None);
    }

    /// The document's `checks` object is the overlay, whole numbers only.
    #[test]
    fn the_overlay_reads_the_documents_checks_object() {
        let limits = limits_of(&serde_json::json!({
            "poll_ms": 6000, "files_max": "many", "diff_cache_bytes": 1.5
        }));
        assert_eq!(limits.poll_ms, 6000);
        assert_eq!(limits.files_max, Limits::default().files_max);
        assert_eq!(limits.diff_cache_bytes, Limits::default().diff_cache_bytes);
        assert_eq!(limits_of(&serde_json::json!({})), Limits::default());
        assert_eq!(limits_of(&serde_json::Value::Null), Limits::default());
    }
}
