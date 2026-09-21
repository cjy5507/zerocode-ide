//! Session-based orchestration for `@` file searches — Codex 0.155.1
//! `tui/src/file_search.rs`.
//!
//! The composer publishes every change of the `@token`. This manager owns a
//! single [`runtime::file_search`] session for the current roots, updates the
//! query on every keystroke, and drops the session when the query becomes
//! empty. Snapshots come back on a channel the app loop selects on — Codex's
//! `AppEvent::FileSearchResult { query, matches }` — so the renderer only ever
//! reads memory: no frame touches the disk.
//!
//! Two things Codex's manager does not carry:
//!
//! * the roots: the session directory **and** the second brain's `wiki/`,
//!   when a vault is configured and set up ([`search_roots`]);
//! * the session's [`SessionBoost`] — every file a tool read or wrote goes in
//!   through [`FileSearchManager::note_touched`] and stands first in the
//!   popup from then on.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use runtime::file_search::{
    create_session, FileMatch, FileSearchOptions, FileSearchSession, FileSearchSnapshot,
    RankBoost, SearchRoot, SessionBoost, SessionReporter,
};
use runtime::second_brain::{SecondBrain, WIKI_DIR};
use tokio::sync::mpsc;

/// Codex `AppEvent::FileSearchResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileSearchResult {
    pub(crate) query: String,
    pub(crate) matches: Vec<FileMatch>,
}

/// What a vault page is shown and inserted as: `wiki/<page>` — the vault's
/// own directory name, so `@wiki/…` reads the same in the popup, in the
/// prompt and on the person's disk.
#[must_use]
pub(crate) fn vault_prefix() -> String {
    format!("{WIKI_DIR}/")
}

/// The roots one search covers: the session's directory, then the second
/// brain's `wiki/` when [`SecondBrain`] is configured and set up.
#[must_use]
pub(crate) fn search_roots(search_dir: &Path) -> Vec<SearchRoot> {
    let mut roots = vec![SearchRoot::repo(search_dir)];
    if let Some(vault) = SecondBrain::from_env().filter(SecondBrain::is_set_up) {
        roots.push(SearchRoot::pages(vault.wiki_dir(), vault_prefix()));
    }
    roots
}

pub(crate) struct FileSearchManager {
    state: Arc<Mutex<SearchState>>,
    roots: Vec<SearchRoot>,
    cwd: PathBuf,
    tx: mpsc::UnboundedSender<FileSearchResult>,
    boost: Arc<SessionBoost>,
}

struct SearchState {
    latest_query: String,
    session: Option<FileSearchSession>,
    session_token: usize,
}

impl FileSearchManager {
    pub(crate) fn new(search_dir: PathBuf, tx: mpsc::UnboundedSender<FileSearchResult>) -> Self {
        let roots = search_roots(&search_dir);
        Self::with_roots(search_dir, roots, tx)
    }

    /// A manager over explicit roots — what a test uses so the person's own
    /// vault, named in this process's environment, never joins its rows.
    pub(crate) fn with_roots(
        search_dir: PathBuf,
        roots: Vec<SearchRoot>,
        tx: mpsc::UnboundedSender<FileSearchResult>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(SearchState {
                latest_query: String::new(),
                session: None,
                session_token: 0,
            })),
            roots,
            cwd: search_dir,
            tx,
            boost: Arc::new(SessionBoost::new()),
        }
    }

    /// Updates the directory used for file searches — the session's cwd
    /// changed on `/resume`. Drops the current session so it is recreated
    /// with the new roots on the next query.
    pub(crate) fn update_search_dir(&mut self, new_dir: PathBuf) {
        self.roots = search_roots(&new_dir);
        self.cwd = new_dir;
        let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        st.session.take();
        st.latest_query.clear();
    }

    /// The roots the popup's rows come from.
    pub(crate) fn roots(&self) -> &[SearchRoot] {
        &self.roots
    }

    /// The vault's page root, when there is one — what tells a `Vault` row
    /// from a `File` row.
    pub(crate) fn page_root(&self) -> Option<&Path> {
        self.roots
            .iter()
            .find(|root| root.pages_only)
            .map(|root| root.dir.as_path())
    }

    /// Call whenever the user edits the `@` token.
    pub(crate) fn on_user_query(&self, query: &str) {
        let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if query == st.latest_query {
            return;
        }
        st.latest_query.clear();
        st.latest_query.push_str(query);
        if query.is_empty() {
            st.session.take();
            return;
        }
        if st.session.is_none() {
            self.start_session_locked(&mut st);
        }
        if let Some(session) = st.session.as_ref() {
            session.update_query(query);
        }
    }

    /// A tool read or wrote `path` in this session: it stands first from now.
    pub(crate) fn note_touched(&self, path: &Path) {
        self.boost.note_touched(&self.cwd, path);
    }

    /// A new conversation starts with nothing touched.
    pub(crate) fn forget_touched(&self) {
        self.boost.forget_all();
    }

    #[cfg(test)]
    pub(crate) fn touched_count(&self) -> usize {
        self.boost.touched_count()
    }

    fn start_session_locked(&self, st: &mut SearchState) {
        st.session_token = st.session_token.wrapping_add(1);
        let reporter = Arc::new(TuiSessionReporter {
            state: Arc::clone(&self.state),
            tx: self.tx.clone(),
            session_token: st.session_token,
        });
        let session = create_session(
            self.roots.clone(),
            FileSearchOptions {
                compute_indices: true,
                boost: Some(Arc::clone(&self.boost) as Arc<dyn RankBoost>),
                ..FileSearchOptions::default()
            },
            reporter,
            /*cancel_flag*/ None,
        );
        match session {
            Ok(session) => st.session = Some(session),
            Err(error) => {
                eprintln!("zo: file search session failed to start: {error}");
                st.session = None;
            }
        }
    }
}

/// Codex `TuiSessionReporter`: a snapshot is forwarded only while it is the
/// current session's and the person is still typing a query.
struct TuiSessionReporter {
    state: Arc<Mutex<SearchState>>,
    tx: mpsc::UnboundedSender<FileSearchResult>,
    session_token: usize,
}

impl SessionReporter for TuiSessionReporter {
    fn on_update(&self, snapshot: &FileSearchSnapshot) {
        let st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if st.session_token != self.session_token
            || st.latest_query.is_empty()
            || snapshot.query.is_empty()
        {
            return;
        }
        drop(st);
        // zo: an empty top-N while the search still runs is not "no matches",
        // it is "nothing found yet" — the popup keeps saying `loading...`
        // until something is found or the search has settled.
        if snapshot.matches.is_empty() && !snapshot.settled {
            return;
        }
        let _ = self.tx.send(FileSearchResult {
            query: snapshot.query.clone(),
            matches: snapshot.matches.clone(),
        });
    }

    fn on_complete(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

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

    /// The first snapshot for `query` that carries a match — an early one
    /// with nothing found is never forwarded while the walk runs, and a
    /// completed walk that found nothing would be the caller's failure.
    async fn next_for(
        rx: &mut mpsc::UnboundedReceiver<FileSearchResult>,
        query: &str,
    ) -> FileSearchResult {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let result = rx.recv().await.expect("channel open");
                if result.query == query && !result.matches.is_empty() {
                    return result;
                }
            }
        })
        .await
        .expect("a snapshot for the query")
    }

    #[tokio::test]
    async fn a_query_starts_one_session_and_its_snapshot_names_the_query() {
        let dir = world(&["src/composer.rs", "README.md"]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let manager = FileSearchManager::with_roots(
            dir.path().to_path_buf(),
            vec![SearchRoot::repo(dir.path())],
            tx,
        );
        manager.on_user_query("comp");
        let result = next_for(&mut rx, "comp").await;
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].path, Path::new("src/composer.rs"));
        assert!(result.matches[0].indices.is_some(), "indices are computed for highlighting");
    }

    #[tokio::test]
    async fn an_empty_query_drops_the_session_and_nothing_stale_arrives() {
        let dir = world(&["src/composer.rs"]);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let manager = FileSearchManager::with_roots(
            dir.path().to_path_buf(),
            vec![SearchRoot::repo(dir.path())],
            tx,
        );
        manager.on_user_query("comp");
        let _ = next_for(&mut rx, "comp").await;
        manager.on_user_query("");
        {
            let st = manager.state.lock().unwrap();
            assert!(st.session.is_none());
            assert!(st.latest_query.is_empty());
        }
        manager.on_user_query("co");
        let result = next_for(&mut rx, "co").await;
        assert_eq!(result.query, "co");
    }

    #[tokio::test]
    async fn a_touched_file_is_noted_against_the_session_directory() {
        let dir = world(&["src/composer.rs"]);
        let (tx, _rx) = mpsc::unbounded_channel();
        let manager = FileSearchManager::new(dir.path().to_path_buf(), tx);
        manager.note_touched(Path::new("src/composer.rs"));
        assert_eq!(manager.touched_count(), 1);
        manager.forget_touched();
        assert_eq!(manager.touched_count(), 0);
    }

    #[test]
    fn the_roots_are_the_session_directory_and_a_set_up_vault_only() {
        let dir = world(&["a.rs"]);
        let roots = search_roots(dir.path());
        // Whatever the environment says, a vault that is not set up adds no root
        // and the session directory is always first.
        assert_eq!(roots[0], SearchRoot::repo(dir.path()));
        assert!(roots.len() <= 2);
        if let Some(pages) = roots.get(1) {
            assert!(pages.pages_only);
            assert_eq!(pages.prefix, vault_prefix());
            assert!(pages.dir.is_dir());
        }
    }
}
