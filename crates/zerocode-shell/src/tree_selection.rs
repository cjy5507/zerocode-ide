//! What the person has selected in the file tree, offered to the agents of
//! that workspace with the person's next prompt (t-24298).
//!
//! The window says the selection whenever it moves (`tree_selection`); a
//! turn's brief asks for one line per pane ([`brief_line`], one of
//! `pane_guard`'s brief contributors). The line names the selected paths
//! absolutely, and it is said once per selection per pane: the next prompt is
//! when "this" means them, and repeating it on every prompt after would be
//! noise the agent pays for.
//!
//! Only the agents whose prompt hook takes context at a turn's start carry it
//! (`AgentKind::hook_additional_context` — Claude and Codex today). Every
//! other agent's prompt has no road for it, and nothing is faked for them.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

/// The most paths one brief names. A selection wider than this is a person
/// gathering files to move rather than pointing at one; the first eight
/// still say where they are pointing, and eight absolute paths stay far under
/// the bridge's brief cap (`zerocode_hookd::TURN_BRIEF_CHAR_CAP`).
pub(crate) const TREE_SELECTION_PATHS: usize = 8;

/// The selection the window last said, and which panes have been told it.
#[derive(Debug, Default)]
pub(crate) struct Selections {
    root: String,
    paths: Vec<String>,
    told: HashSet<String>,
}

impl Selections {
    /// The window's selection, relative to `root`. Stub (red).
    pub(crate) fn hold(&mut self, _root: &str, _paths: Vec<String>) {}

    /// The line for `pane_key`'s next prompt, in at most `room` characters,
    /// if it works in the selection's workspace and has not been told this
    /// selection yet. Stub (red).
    pub(crate) fn offer(&mut self, _worktree: &str, _pane_key: &str, _room: usize) -> Option<String> {
        None
    }
}

static HELD: LazyLock<Mutex<Selections>> = LazyLock::new(Mutex::default);

/// The window's word that the tree's selection moved.
pub(crate) fn hold(root: &str, paths: Vec<String>) {
    HELD.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .hold(root, paths);
}

/// A turn's brief, asked for this module's line in at most `room`
/// characters.
pub(crate) fn brief_line(ask: &zerocode_hookd::TurnBriefAsk, room: usize) -> Option<String> {
    HELD.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .offer(&ask.worktree, &ask.pane_key, room)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/Users/dev/repo";
    /// The whole brief's room — what the first voice is handed.
    const ROOM: usize = zerocode_hookd::TURN_BRIEF_CHAR_CAP;

    #[test]
    fn a_selection_is_offered_once_to_each_pane_of_its_workspace() {
        let mut held = Selections::default();
        held.hold(ROOT, vec!["src/main.rs".to_string()]);
        let said = held
            .offer(ROOT, "term-1", ROOM)
            .expect("a pane in the selection's workspace was told nothing");
        assert!(said.contains("/Users/dev/repo/src/main.rs"), "{said}");
        assert!(said.contains("file tree"), "{said}");
        // Once per selection per pane: the next prompt is the one it is for.
        assert_eq!(held.offer(ROOT, "term-1", ROOM), None);
        // Another pane of the same workspace has its own next prompt.
        assert!(held.offer(&format!("{ROOT}/"), "term-2", ROOM).is_some());
        // A pane of another workspace is not pointed at this one's files.
        assert_eq!(held.offer("/Users/dev/other", "term-3", ROOM), None);
        // The same selection said again is not news; a new one is.
        held.hold(ROOT, vec!["src/main.rs".to_string()]);
        assert_eq!(held.offer(ROOT, "term-1", ROOM), None);
        held.hold(ROOT, vec!["src/lib.rs".to_string(), "README.md".to_string()]);
        let again = held.offer(ROOT, "term-1", ROOM).expect("a new selection was not offered");
        assert!(
            again.contains("/Users/dev/repo/src/lib.rs")
                && again.contains("/Users/dev/repo/README.md")
                && !again.contains("main.rs"),
            "{again}"
        );
        // Nothing selected is nothing said.
        held.hold(ROOT, Vec::new());
        assert_eq!(held.offer(ROOT, "term-4", ROOM), None);
    }

    #[test]
    fn a_selection_names_only_paths_inside_its_workspace_and_no_more_than_a_glance() {
        let mut held = Selections::default();
        let mut paths: Vec<String> = (0..20).map(|at| format!("src/f{at}.rs")).collect();
        paths.push("../outside.rs".to_string());
        paths.push("/etc/hosts".to_string());
        paths.insert(0, "../escape.rs".to_string());
        held.hold(ROOT, paths);
        let said = held.offer(ROOT, "term-1", ROOM).expect("a selection said nothing");
        assert_eq!(
            said.matches("/Users/dev/repo/src/").count(),
            TREE_SELECTION_PATHS,
            "{said}"
        );
        assert!(!said.contains("outside") && !said.contains("escape") && !said.contains("/etc/"));
        assert!(said.chars().count() <= zerocode_hookd::TURN_BRIEF_CHAR_CAP);
    }

    #[test]
    fn a_selection_says_only_what_fits_its_room_and_tells_nobody_it_could_not_reach() {
        let mut held = Selections::default();
        held.hold(ROOT, vec!["src/a.rs".to_string(), "src/b.rs".to_string()]);
        let whole = held.offer(ROOT, "term-1", ROOM).expect("the whole selection");
        // One path less fits in a room cut just short of the whole line.
        let mut cut = Selections::default();
        cut.hold(ROOT, vec!["src/a.rs".to_string(), "src/b.rs".to_string()]);
        let room = whole.chars().count() - 1;
        let shorter = cut.offer(ROOT, "term-1", room).expect("a shorter line");
        assert!(shorter.chars().count() <= room, "{shorter}");
        assert!(shorter.contains("src/a.rs") && !shorter.contains("src/b.rs"), "{shorter}");
        // No room for even one path says nothing — and that pane is still
        // owed the selection, which a roomier turn then says.
        let mut tight = Selections::default();
        tight.hold(ROOT, vec!["src/a.rs".to_string()]);
        assert_eq!(tight.offer(ROOT, "term-2", 10), None);
        assert!(tight.offer(ROOT, "term-2", ROOM).is_some());
    }
}
