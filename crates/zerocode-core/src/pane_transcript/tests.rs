use super::*;
use crate::vault::Format;
use std::cell::Cell;
use std::collections::BTreeMap;

const HOME: &str = "/Users/dev";
const LINE: &str = "Refactor the drain test so it stops flaking";

/// A disk and a pane written out by hand: the rule's facts, with no I/O. The
/// two `asked_` cells say whether the rule looked at the open files or the
/// screen, which is what "the id is the only answer" means.
#[derive(Default)]
struct Disk {
    /// Every session file of every store, in the order a listing gives them.
    listing: Vec<PathBuf>,
    /// The id a file names on its first line.
    first_lines: BTreeMap<PathBuf, String>,
    /// The text of each file, for the screen match.
    texts: BTreeMap<PathBuf, String>,
    /// Files that are there, for a reported transcript.
    present: Vec<PathBuf>,
    open: Vec<PathBuf>,
    screen: Vec<String>,
    cut_short: bool,
    asked_open: Cell<bool>,
    asked_screen: Cell<bool>,
    /// How many times the store was listed, and how many first lines were read.
    walks: Cell<usize>,
    first_line_reads: Cell<usize>,
    /// The clock the memory reads. Unset, it is the real clock.
    clock: Cell<Option<Instant>>,
}

impl Facts for Disk {
    fn exists(&self, path: &Path) -> bool {
        self.present.iter().any(|held| held == path) || self.listing.iter().any(|held| held == path)
    }

    fn roots(&self, source: &AgentSource) -> Vec<PathBuf> {
        vault::roots(source, Path::new(HOME), None)
    }

    fn session_files(&self, source: &AgentSource) -> Option<Vec<PathBuf>> {
        self.walks.set(self.walks.get() + 1);
        if self.cut_short {
            return None;
        }
        let roots = self.roots(source);
        Some(
            self.listing
                .iter()
                .filter(|path| roots.iter().any(|root| path.starts_with(root)))
                .filter(|path| vault::wanted(source, path))
                .cloned()
                .collect(),
        )
    }

    fn first_line_id(&self, path: &Path) -> Option<String> {
        self.first_line_reads.set(self.first_line_reads.get() + 1);
        self.first_lines.get(path).cloned()
    }

    fn open_files(&self) -> Vec<PathBuf> {
        self.asked_open.set(true);
        self.open.clone()
    }

    fn screen_lines(&self) -> Vec<String> {
        self.asked_screen.set(true);
        self.screen.clone()
    }

    fn text_of(&self, path: &Path) -> Option<String> {
        self.texts.get(path).cloned()
    }

    fn now(&self) -> Instant {
        self.clock.get().unwrap_or_else(Instant::now)
    }
}

fn claude(id: &str) -> PathBuf {
    PathBuf::from(format!(
        "{HOME}/.claude/projects/-Users-dev-repo/{id}.jsonl"
    ))
}

fn claude_elsewhere(id: &str) -> PathBuf {
    PathBuf::from(format!(
        "{HOME}/.claude/projects/-Users-dev-other/{id}.jsonl"
    ))
}

fn zo(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "{HOME}/.zo/projects/Users-dev-repo/sessions/{name}.jsonl"
    ))
}

fn codex(stamp: &str) -> PathBuf {
    PathBuf::from(format!(
        "{HOME}/.codex/sessions/2026/10/10/rollout-{stamp}.jsonl"
    ))
}

fn disk_of(listing: Vec<PathBuf>) -> Disk {
    Disk {
        listing,
        ..Disk::default()
    }
}

#[test]
fn a_session_id_names_the_one_file_that_carries_it() {
    let disk = disk_of(vec![claude("aaa"), claude("bbb")]);
    assert_eq!(
        bind("claude", None, Some("bbb"), &disk),
        Ok((claude("bbb"), Via::SessionId))
    );
}

#[test]
fn two_panels_in_one_folder_each_name_their_own_file() {
    let disk = disk_of(vec![claude("aaa"), claude("bbb"), claude("ccc")]);
    assert_eq!(
        bind("claude", None, Some("aaa"), &disk),
        Ok((claude("aaa"), Via::SessionId))
    );
    assert_eq!(
        bind("claude", None, Some("bbb"), &disk),
        Ok((claude("bbb"), Via::SessionId))
    );
}

#[test]
fn a_file_that_appears_later_moves_no_panel() {
    // `zzz` is the newest file in the folder, and no panel here owns it.
    let disk = disk_of(vec![claude("aaa"), claude("bbb"), claude("zzz")]);
    assert_eq!(
        bind("claude", None, Some("aaa"), &disk),
        Ok((claude("aaa"), Via::SessionId))
    );
    assert_eq!(
        bind("claude", None, Some("new"), &disk),
        Err(Absent::NoSessionFile)
    );
}

#[test]
fn two_files_carrying_one_id_name_neither() {
    let disk = disk_of(vec![claude("aaa"), claude_elsewhere("aaa")]);
    assert_eq!(
        bind("claude", None, Some("aaa"), &disk),
        Err(Absent::TwoSessionFiles)
    );
}

#[test]
fn a_session_id_never_falls_back_to_an_open_file() {
    let disk = Disk {
        open: vec![claude("aaa")],
        ..disk_of(vec![claude("aaa"), claude("bbb")])
    };
    // The panel's own file is named by its id, though another file is open.
    assert_eq!(
        bind("claude", None, Some("bbb"), &disk),
        Ok((claude("bbb"), Via::SessionId))
    );
    // A panel whose file is not written yet is not handed the open one.
    assert_eq!(
        bind("claude", None, Some("ccc"), &disk),
        Err(Absent::NoSessionFile)
    );
    assert!(
        !disk.asked_open.get(),
        "with a session id the open files are not asked"
    );
}

#[test]
fn a_pane_without_an_id_names_the_one_session_file_it_holds_open() {
    let disk = Disk {
        open: vec![
            PathBuf::from("/Users/dev/Library/Logs/agent.log"),
            claude("aaa"),
        ],
        ..disk_of(vec![claude("aaa"), claude("bbb")])
    };
    assert_eq!(
        bind("claude", None, None, &disk),
        Ok((claude("aaa"), Via::OpenFile))
    );
    assert!(
        !disk.asked_screen.get(),
        "an open file answers, so the screen is not read"
    );
}

#[test]
fn two_open_session_files_name_neither() {
    let disk = Disk {
        open: vec![claude("aaa"), claude("bbb")],
        ..disk_of(vec![claude("aaa"), claude("bbb")])
    };
    assert_eq!(bind("claude", None, None, &disk), Err(Absent::TwoOpenFiles));
}

#[test]
fn an_open_file_outside_the_agents_store_is_not_a_transcript() {
    let disk = Disk {
        open: vec![PathBuf::from("/Users/dev/Downloads/aaa.jsonl")],
        ..disk_of(vec![claude("aaa")])
    };
    // No session file is open, so the screen is asked. It holds no line of
    // the transcript, so no file is named.
    assert_eq!(
        bind("claude", None, None, &disk),
        Err(Absent::NoScreenMatch)
    );
}

#[test]
fn without_an_id_or_an_open_file_a_unique_screen_line_names_the_file() {
    let disk = Disk {
        texts: BTreeMap::from([
            (claude("aaa"), format!("user: {LINE}")),
            (claude("bbb"), "user: something else entirely".to_string()),
        ]),
        screen: vec!["> ".to_string(), LINE.to_string()],
        ..disk_of(vec![claude("aaa"), claude("bbb")])
    };
    assert_eq!(
        bind("claude", None, None, &disk),
        Ok((claude("aaa"), Via::ScreenMatch))
    );
}

#[test]
fn a_screen_line_held_by_two_files_names_neither() {
    let disk = Disk {
        texts: BTreeMap::from([
            (claude("aaa"), format!("user: {LINE}")),
            (claude("bbb"), format!("user: {LINE}")),
        ]),
        screen: vec![LINE.to_string()],
        ..disk_of(vec![claude("aaa"), claude("bbb")])
    };
    assert_eq!(
        bind("claude", None, None, &disk),
        Err(Absent::TwoScreenMatches)
    );
}

#[test]
fn a_newer_file_that_holds_no_screen_line_is_not_named() {
    let disk = Disk {
        texts: BTreeMap::from([
            (claude("aaa"), "user: an older conversation".to_string()),
            (claude("zzz"), "user: the newest conversation".to_string()),
        ]),
        screen: vec![LINE.to_string()],
        ..disk_of(vec![claude("aaa"), claude("zzz")])
    };
    assert_eq!(
        bind("claude", None, None, &disk),
        Err(Absent::NoScreenMatch)
    );
}

#[test]
fn a_short_screen_line_names_no_file() {
    let disk = Disk {
        texts: BTreeMap::from([(claude("aaa"), "user: ok".to_string())]),
        screen: vec!["ok".to_string()],
        ..disk_of(vec![claude("aaa")])
    };
    assert_eq!(
        bind("claude", None, None, &disk),
        Err(Absent::NoScreenMatch)
    );
}

#[test]
fn a_listing_cut_short_names_no_file() {
    let disk = Disk {
        cut_short: true,
        texts: BTreeMap::from([(claude("aaa"), format!("user: {LINE}"))]),
        screen: vec![LINE.to_string()],
        ..disk_of(vec![claude("aaa")])
    };
    assert_eq!(
        bind("claude", None, Some("aaa"), &disk),
        Err(Absent::NoSessionFile)
    );
    assert_eq!(
        bind("claude", None, None, &disk),
        Err(Absent::NoScreenMatch)
    );
}

#[test]
fn a_codex_file_carries_its_id_on_its_first_line_not_in_its_name() {
    let disk = Disk {
        first_lines: BTreeMap::from([
            (codex("2026-10-10T09-00-00"), "cdx-first".to_string()),
            (codex("2026-10-10T10-00-00"), "cdx-second".to_string()),
        ]),
        ..disk_of(vec![
            codex("2026-10-10T09-00-00"),
            codex("2026-10-10T10-00-00"),
        ])
    };
    assert_eq!(
        bind("codex", None, Some("cdx-second"), &disk),
        Ok((codex("2026-10-10T10-00-00"), Via::SessionId))
    );
}

#[test]
fn a_zo_session_carries_its_id_on_its_first_line() {
    let disk = Disk {
        first_lines: BTreeMap::from([
            (zo("session-1000-0"), "session-1000-0".to_string()),
            (zo("session-1000-1"), "session-1000-1".to_string()),
        ]),
        ..disk_of(vec![zo("session-1000-0"), zo("session-1000-1")])
    };
    assert_eq!(
        bind("zo", None, Some("session-1000-1"), &disk),
        Ok((zo("session-1000-1"), Via::SessionId))
    );
}

#[test]
fn a_reported_file_that_exists_is_used_first() {
    let reported = claude("bbb");
    let disk = Disk {
        present: vec![claude("bbb")],
        ..disk_of(vec![claude("aaa"), claude("bbb")])
    };
    assert_eq!(
        bind("claude", Some(reported.as_path()), Some("bbb"), &disk),
        Ok((claude("bbb"), Via::Reported))
    );
}

#[test]
fn a_reported_file_that_is_gone_falls_back_to_the_session_id() {
    let gone = claude("gone");
    let disk = disk_of(vec![claude("bbb")]);
    assert_eq!(
        bind("claude", Some(gone.as_path()), Some("bbb"), &disk),
        Ok((claude("bbb"), Via::SessionId))
    );
}

#[test]
fn an_agent_with_no_store_row_has_no_transcript() {
    let disk = disk_of(vec![claude("aaa")]);
    assert_eq!(
        bind("kimi", None, Some("kimi-1"), &disk),
        Err(Absent::NoAdapter)
    );
}

#[test]
fn a_store_this_window_cannot_read_yet_says_so() {
    let disk = Disk::default();
    assert_eq!(
        bind("grok", None, Some("grok-1"), &disk),
        Err(Absent::UnreadFormat)
    );
    assert_eq!(
        bind("antigravity", None, Some("agy-1"), &disk),
        Err(Absent::UnreadFormat)
    );
}

#[test]
fn each_shape_says_where_its_id_shows_and_which_shapes_are_readable() {
    assert_eq!(Format::ClaudeLines.id_placement(), IdPlacement::FileStem);
    assert_eq!(
        Format::CodexRollout.id_placement(),
        IdPlacement::FirstLineNamed
    );
    assert_eq!(Format::ZoLines.id_placement(), IdPlacement::FirstLine);
    assert_eq!(
        Format::AntigravityTranscript.id_placement(),
        IdPlacement::Folder
    );
    assert_eq!(Format::Unread.id_placement(), IdPlacement::Unstated);
    assert!(Format::ClaudeLines.readable());
    assert!(Format::CodexRollout.readable());
    assert!(Format::ZoLines.readable());
    assert!(!Format::AntigravityTranscript.readable());
    assert!(!Format::Unread.readable());
}

#[test]
fn a_session_meta_line_names_its_id_for_codex_and_zo_and_nothing_else() {
    assert_eq!(
        vault::session_meta_id(r#"{"type":"session_meta","payload":{"id":"019a-codex"}}"#),
        Some("019a-codex".to_string())
    );
    assert_eq!(
        vault::session_meta_id(r#"{"type":"session_meta","session_id":"session-1000-0"}"#),
        Some("session-1000-0".to_string())
    );
    assert_eq!(
        vault::session_meta_id(r#"{"type":"message","session_id":"session-1000-0"}"#),
        None
    );
    assert_eq!(vault::session_meta_id("not json"), None);
    assert_eq!(
        vault::session_meta_id(r#"{"type":"session_meta","session_id":"-rm"}"#),
        None
    );
}

// ---------------------------------------------------------------- the memory
//
// Each test asks through the window's memory of a pane (herdr 4 follow-up,
// t-42948). Pane 7 is the panel that asks; pane 1 and pane 2 are the others.

#[test]
fn a_named_file_is_not_walked_again_on_the_next_poll() {
    let disk = disk_of(vec![claude("aaa"), claude("bbb")]);
    let memo = Mutex::new(Memo::<u32>::default());
    for _ in 0..64 {
        assert_eq!(
            bind_remembered(&memo, 7, "claude", None, Some("bbb"), &disk),
            Ok((claude("bbb"), Via::SessionId))
        );
    }
    assert_eq!(
        disk.walks.get(),
        1,
        "64 polls of one panel list the store once"
    );
}

#[test]
fn an_open_file_that_comes_and_goes_does_not_flip_the_answer() {
    let listing = vec![claude("aaa"), claude("bbb")];
    let open = Disk {
        open: vec![claude("aaa")],
        ..disk_of(listing.clone())
    };
    let closed = disk_of(listing);
    let memo = Mutex::new(Memo::<u32>::default());
    // The agent opens and closes its file between polls. The file is there on
    // every poll, so the panel names it on every poll.
    for poll in 0..6 {
        let disk = if poll % 2 == 0 { &open } else { &closed };
        assert_eq!(
            bind_remembered(&memo, 7, "claude", None, None, disk),
            Ok((claude("aaa"), Via::OpenFile)),
            "poll {poll} names the file that is still there"
        );
    }
}

#[test]
fn a_remembered_file_that_is_gone_is_not_named() {
    let open = Disk {
        open: vec![claude("aaa")],
        ..disk_of(vec![claude("aaa")])
    };
    let memo = Mutex::new(Memo::<u32>::default());
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &open),
        Ok((claude("aaa"), Via::OpenFile))
    );
    let gone = disk_of(Vec::new());
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &gone),
        Err(Absent::NoScreenMatch)
    );
}

#[test]
fn a_no_file_answer_is_asked_again_only_after_the_recheck_interval() {
    let disk = disk_of(Vec::new());
    let start = Instant::now();
    let memo = Mutex::new(Memo::<u32>::default());
    disk.clock.set(Some(start));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
        Err(Absent::NoSessionFile)
    );
    disk.clock
        .set(Some(start + ABSENT_RECHECK - Duration::from_millis(1)));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
        Err(Absent::NoSessionFile)
    );
    assert_eq!(
        disk.walks.get(),
        1,
        "inside the interval the store is not listed again"
    );
    disk.clock.set(Some(start + ABSENT_RECHECK));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
        Err(Absent::NoSessionFile)
    );
    assert_eq!(
        disk.walks.get(),
        2,
        "at the interval the store is listed again"
    );
}

#[test]
fn a_remembered_answer_is_never_handed_to_another_panel() {
    let listing = vec![claude("aaa"), claude("bbb")];
    let one = Disk {
        open: vec![claude("aaa")],
        ..disk_of(listing.clone())
    };
    let nothing_open = disk_of(listing);
    let memo = Mutex::new(Memo::<u32>::default());
    assert_eq!(
        bind_remembered(&memo, 1, "claude", None, None, &one),
        Ok((claude("aaa"), Via::OpenFile))
    );
    // Pane two holds no file open and its screen names nothing. Pane one's
    // answer must not reach it, though both panels are Claude panels with no id.
    assert_eq!(
        bind_remembered(&memo, 2, "claude", None, None, &nothing_open),
        Err(Absent::NoScreenMatch)
    );
    assert_eq!(
        bind_remembered(&memo, 1, "claude", None, None, &one),
        Ok((claude("aaa"), Via::OpenFile))
    );
}

#[test]
fn a_changed_session_id_is_asked_again() {
    let disk = disk_of(vec![claude("aaa"), claude("bbb")]);
    let memo = Mutex::new(Memo::<u32>::default());
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
        Ok((claude("aaa"), Via::SessionId))
    );
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("bbb"), &disk),
        Ok((claude("bbb"), Via::SessionId))
    );
}

#[test]
fn a_report_that_appears_wins_over_a_remembered_answer() {
    let disk = disk_of(vec![claude("aaa"), claude("ccc")]);
    let memo = Mutex::new(Memo::<u32>::default());
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
        Ok((claude("aaa"), Via::SessionId))
    );
    let reported = claude("ccc");
    assert_eq!(
        bind_remembered(
            &memo,
            7,
            "claude",
            Some(reported.as_path()),
            Some("aaa"),
            &disk
        ),
        Ok((reported.clone(), Via::Reported))
    );
}
