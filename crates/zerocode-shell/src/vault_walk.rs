//! The session vault's walk, kept between calls.
//!
//! One walk is fourteen directory trees plus a database — measured at ~400ms
//! over two hundred sessions on a machine holding 9,549 project directories —
//! and the panel re-asks `vault_sessions` on every keystroke, every grouping,
//! every sort and every filter press. Each of those changes the QUERY and none
//! of them changes the disk, so only the first has to read it.
//!
//! The window has said so all along: "the answer is kept and only the query is
//! re-asked, and the reload is what the refresh button is for". It kept the
//! VIEW, which is the answer to one query, so the next keystroke walked the
//! disk again anyway. What is kept here is what the disk actually said.
//!
//! The neighbouring usage panel keeps the same promise by shipping every row
//! once and cutting the ranges in the window. The vault cannot: its filter,
//! sort and grouping are Rust's on purpose, so that the webview never holds
//! two hundred sessions it is not drawing. Same promise, opposite side of the
//! boundary — which is why this is a store and not a bigger payload.

use std::sync::Mutex;

use zerocode_core::vault::{ScanIssue, VaultSession};

/// One walk of the session stores, before any query has touched it.
///
/// The sessions are as the SCAN left them: `resume` still carries no settings.
/// That rewrite stays per-call on purpose — a launch override somebody edits in
/// settings has to reach the next card, and a base baked in here would need a
/// reload before anyone believed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    pub sessions: Vec<VaultSession>,
    pub issues: Vec<ScanIssue>,
    pub truncated: bool,
}

/// Where the last walk waits.
///
/// A type rather than a bare static so the rule can be tested without a
/// process-wide walk: the production one is [`kept`] and [`keep`] below.
///
/// `force` is the refresh button and nothing else — the walk a person asked
/// for. There is deliberately no clock. An age bound would be a second rule
/// about freshness on a surface that already carries a visible reload, and the
/// case it would fix (a conversation begun while the panel stood open) is the
/// case somebody is about to press that button for.
pub struct Store {
    held: Mutex<Option<Walk>>,
}

impl Store {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            held: Mutex::new(None),
        }
    }

    /// The kept walk, or [`None`] when the disk has to be read.
    ///
    /// A forced ask does not throw away what is held. A walk that was asked for
    /// and then went wrong — the blocking hop cancelled, the window closed
    /// under it — would otherwise leave the panel with nothing where it had an
    /// answer a moment ago.
    #[must_use]
    pub fn kept(&self, force: bool) -> Option<Walk> {
        if force {
            return None;
        }
        self.held.lock().expect("vault walk lock").clone()
    }

    /// Keep what a walk found, for the queries after this one.
    pub fn keep(&self, walk: Walk) {
        *self.held.lock().expect("vault walk lock") = Some(walk);
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

/// This process's own walk.
static KEPT: Store = Store::new();

/// [`Store::kept`] on this process's walk.
#[must_use]
pub fn kept(force: bool) -> Option<Walk> {
    KEPT.kept(force)
}

/// [`Store::keep`] on this process's walk.
pub fn keep(walk: Walk) {
    KEPT.keep(walk);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(agent: &str, truncated: bool) -> Walk {
        Walk {
            sessions: Vec::new(),
            issues: vec![ScanIssue {
                agent: agent.into(),
                path: "/store".into(),
                reason: "unread-format".into(),
                count: 3,
            }],
            truncated,
        }
    }

    #[test]
    fn limit_queries_reuse_the_walk_with_zero_scan_calls() {
        use zerocode_core::vault::{self, Limits, VaultQuery};
        let store = Store::new();
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".claude/projects/fixture");
        std::fs::create_dir_all(&root).unwrap();
        for n in 0..250 {
            std::fs::write(root.join(format!("{n}.jsonl")), format!(r#"{{"type":"user","sessionId":"{n}","message":{{"role":"user","content":"ask"}}}}"#)).unwrap();
        }
        let mut scans = 0;
        let mut asks = 0;
        for (limit, force) in [
            (50, false),
            (100, false),
            (200, false),
            (0, false),
            (50, true),
        ] {
            let before = scans;
            let walk = store.kept(force).unwrap_or_else(|| {
                scans += 1;
                let (sessions, issues, truncated) =
                    vault::scan_with(home.path(), Limits::DEFAULT.absolute_max, &|_| None);
                let walk = Walk {
                    sessions,
                    issues,
                    truncated,
                };
                store.keep(walk.clone());
                walk
            });
            asks += 1;
            let answer = vault::view(
                walk.sessions,
                &VaultQuery {
                    limit: Some(limit),
                    ..VaultQuery::default()
                },
                walk.issues,
                walk.truncated,
                &[],
            );
            assert_eq!(
                answer.shown,
                Limits::DEFAULT.session_limit(Some(limit)).min(250)
            );
            let delta = scans - before;
            assert_eq!(delta, usize::from(asks == 1 || force));
            println!(
                "VAULT_WALK_COUNTER ask={asks} limit={limit} force={force} scan={delta} shown={}",
                answer.shown
            );
        }
    }

    /// A keystroke reads what the last walk said; the refresh button reads the
    /// disk. And a forced ask leaves the held walk where it was — the panel
    /// that had an answer keeps it if that walk never comes back.
    #[test]
    fn a_walk_is_kept_until_somebody_asks_for_a_new_one() {
        let store = Store::new();
        assert_eq!(
            store.kept(false),
            None,
            "a store nobody has walked into answers a query it cannot answer"
        );

        let first = walk("claude", false);
        store.keep(first.clone());
        assert_eq!(
            store.kept(false),
            Some(first.clone()),
            "the keystroke after a walk went back to the disk"
        );

        assert_eq!(
            store.kept(true),
            None,
            "the refresh button was answered from the very thing it asked to \
             replace"
        );
        assert_eq!(
            store.kept(false),
            Some(first),
            "a forced ask emptied the store, so a walk that never lands leaves \
             the panel with less than it had"
        );

        let second = walk("codex", true);
        store.keep(second.clone());
        assert_eq!(
            store.kept(false),
            Some(second),
            "a landed walk did not replace the one before it"
        );
    }
}
