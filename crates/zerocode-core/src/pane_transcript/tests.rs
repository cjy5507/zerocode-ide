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
    /// How many times the open files were asked.
    open_asks: Cell<usize>,
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
        self.open_asks.set(self.open_asks.get() + 1);
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

// ------------------------------------------ the found answers asked again (t-43204)

#[test]
fn a_file_found_through_the_open_files_moves_to_the_file_the_pane_opens_next() {
    let listing = vec![claude("aaa"), claude("bbb")];
    let on_a = Disk {
        open: vec![claude("aaa")],
        ..disk_of(listing.clone())
    };
    let on_b = Disk {
        open: vec![claude("bbb")],
        ..disk_of(listing)
    };
    let memo = Mutex::new(Memo::<u32>::default());
    let start = Instant::now();
    on_a.clock.set(Some(start));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &on_a),
        Ok((claude("aaa"), Via::OpenFile))
    );
    // The pane now holds B open. A stands until the interval has passed.
    on_b.clock
        .set(Some(start + OPEN_FILE_RECHECK - Duration::from_millis(1)));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &on_b),
        Ok((claude("aaa"), Via::OpenFile)),
        "inside the interval the file found earlier stands"
    );
    on_b.clock.set(Some(start + OPEN_FILE_RECHECK));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &on_b),
        Ok((claude("bbb"), Via::OpenFile)),
        "at the interval the file the pane holds open now is named"
    );
}

#[test]
fn a_file_found_through_the_open_files_stands_while_nothing_is_open() {
    let listing = vec![claude("aaa")];
    let on_a = Disk {
        open: vec![claude("aaa")],
        ..disk_of(listing.clone())
    };
    let nothing_open = disk_of(listing);
    let memo = Mutex::new(Memo::<u32>::default());
    let start = Instant::now();
    on_a.clock.set(Some(start));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &on_a),
        Ok((claude("aaa"), Via::OpenFile))
    );
    // Ten minutes with nothing open: every ask finds no file, and A stands at
    // each. The asks come no closer together than the open-file interval: a
    // "no file" answer never brings the next ask forward.
    let mut asks = 0;
    let mut last_ask = 0;
    for second in 1..=600 {
        nothing_open
            .clock
            .set(Some(start + Duration::from_secs(second)));
        assert_eq!(
            bind_remembered(&memo, 7, "claude", None, None, &nothing_open),
            Ok((claude("aaa"), Via::OpenFile)),
            "at {second} s, with nothing open, the file found earlier stands"
        );
        if nothing_open.open_asks.get() > asks {
            asks = nothing_open.open_asks.get();
            assert!(
                second - last_ask >= OPEN_FILE_RECHECK.as_secs(),
                "the open files were asked at {last_ask} s and again at {second} s, \
                 closer than the open-file interval"
            );
            last_ask = second;
        }
    }
    assert!(asks >= 1, "the open files were asked again at least once");
}

#[test]
fn a_file_found_through_the_screen_is_asked_again_after_its_interval() {
    let line_b = "Now the drain test is the second conversation";
    let listing = vec![claude("aaa"), claude("bbb")];
    let texts = BTreeMap::from([
        (claude("aaa"), format!("user: {LINE}")),
        (claude("bbb"), format!("user: {line_b}")),
    ]);
    let on_a = Disk {
        texts: texts.clone(),
        screen: vec![LINE.to_string()],
        ..disk_of(listing.clone())
    };
    let on_b = Disk {
        texts,
        screen: vec![line_b.to_string()],
        ..disk_of(listing)
    };
    let memo = Mutex::new(Memo::<u32>::default());
    let start = Instant::now();
    on_a.clock.set(Some(start));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &on_a),
        Ok((claude("aaa"), Via::ScreenMatch))
    );
    on_b.clock
        .set(Some(start + SCREEN_RECHECK - Duration::from_millis(1)));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &on_b),
        Ok((claude("aaa"), Via::ScreenMatch)),
        "inside the interval the file found earlier stands"
    );
    on_b.clock.set(Some(start + SCREEN_RECHECK));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &on_b),
        Ok((claude("bbb"), Via::ScreenMatch)),
        "at the interval the file that holds the new screen line is named"
    );
}

#[test]
fn a_file_found_through_the_screen_is_not_asked_before_its_interval_after_a_miss() {
    let line_nowhere = "A line that no session file carries";
    let listing = vec![claude("aaa")];
    let texts = BTreeMap::from([(claude("aaa"), format!("user: {LINE}"))]);
    let on_a = Disk {
        texts: texts.clone(),
        screen: vec![LINE.to_string()],
        ..disk_of(listing.clone())
    };
    let off_a = Disk {
        texts,
        screen: vec![line_nowhere.to_string()],
        ..disk_of(listing)
    };
    let memo = Mutex::new(Memo::<u32>::default());
    let start = Instant::now();
    on_a.clock.set(Some(start));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &on_a),
        Ok((claude("aaa"), Via::ScreenMatch))
    );
    // At the interval the screen line is in no file: A stands.
    off_a.clock.set(Some(start + SCREEN_RECHECK));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &off_a),
        Ok((claude("aaa"), Via::ScreenMatch)),
        "a no-file answer does not replace the file found earlier"
    );
    let walks = off_a.walks.get();
    // That no-file answer does not bring the next ask forward: the store is not
    // walked again before a whole screen interval has passed.
    for since_miss in [
        ABSENT_RECHECK,
        ABSENT_RECHECK * 2,
        ABSENT_RECHECK * 4,
        SCREEN_RECHECK - Duration::from_millis(1),
    ] {
        off_a.clock.set(Some(start + SCREEN_RECHECK + since_miss));
        assert_eq!(
            bind_remembered(&memo, 7, "claude", None, None, &off_a),
            Ok((claude("aaa"), Via::ScreenMatch))
        );
        assert_eq!(
            off_a.walks.get(),
            walks,
            "{since_miss:?} after the miss the store is not walked again"
        );
    }
    off_a.clock.set(Some(start + SCREEN_RECHECK * 2));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &off_a),
        Ok((claude("aaa"), Via::ScreenMatch))
    );
    assert_eq!(
        off_a.walks.get(),
        walks + 1,
        "at the next screen interval the store is walked once"
    );
}

#[test]
fn a_file_found_through_the_open_files_is_not_asked_before_its_interval_after_a_miss() {
    let listing = vec![claude("aaa")];
    let on_a = Disk {
        open: vec![claude("aaa")],
        ..disk_of(listing.clone())
    };
    let nothing_open = disk_of(listing);
    let memo = Mutex::new(Memo::<u32>::default());
    let start = Instant::now();
    on_a.clock.set(Some(start));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &on_a),
        Ok((claude("aaa"), Via::OpenFile))
    );
    // At the interval nothing is open: A stands, and the open files were asked once.
    nothing_open.clock.set(Some(start + OPEN_FILE_RECHECK));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &nothing_open),
        Ok((claude("aaa"), Via::OpenFile)),
        "a no-file answer does not replace the file found earlier"
    );
    let asks = nothing_open.open_asks.get();
    assert_eq!(asks, 1, "at the interval the open files are asked once");
    // That no-file answer does not bring the next ask forward.
    for since_miss in [
        ABSENT_RECHECK,
        ABSENT_RECHECK * 2,
        OPEN_FILE_RECHECK - Duration::from_millis(1),
    ] {
        nothing_open
            .clock
            .set(Some(start + OPEN_FILE_RECHECK + since_miss));
        assert_eq!(
            bind_remembered(&memo, 7, "claude", None, None, &nothing_open),
            Ok((claude("aaa"), Via::OpenFile))
        );
        assert_eq!(
            nothing_open.open_asks.get(),
            asks,
            "{since_miss:?} after the miss the open files are not asked again"
        );
    }
    nothing_open.clock.set(Some(start + OPEN_FILE_RECHECK * 2));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, None, &nothing_open),
        Ok((claude("aaa"), Via::OpenFile))
    );
    assert_eq!(
        nothing_open.open_asks.get(),
        asks + 1,
        "at the next open-file interval the open files are asked once"
    );
}

// ------------------------------------------ the no-file answers backed off (t-43204)

#[test]
fn consecutive_no_file_answers_wait_twice_as_long_each_time_up_to_the_cap() {
    let disk = disk_of(Vec::new());
    let memo = Mutex::new(Memo::<u32>::default());
    let mut asked_at = Instant::now();
    disk.clock.set(Some(asked_at));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
        Err(Absent::NoSessionFile)
    );
    for wait in [2, 4, 8, 16, 32, 60, 60] {
        let wait = Duration::from_secs(wait);
        let walks = disk.walks.get();
        disk.clock
            .set(Some(asked_at + wait - Duration::from_millis(1)));
        assert_eq!(
            bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
            Err(Absent::NoSessionFile)
        );
        assert_eq!(
            disk.walks.get(),
            walks,
            "the store is not listed before the {wait:?} wait is over"
        );
        disk.clock.set(Some(asked_at + wait));
        assert_eq!(
            bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
            Err(Absent::NoSessionFile)
        );
        assert_eq!(
            disk.walks.get(),
            walks + 1,
            "the store is listed once the {wait:?} wait is over"
        );
        asked_at += wait;
    }
}

#[test]
fn a_changed_question_waits_the_first_interval_again() {
    let disk = disk_of(Vec::new());
    let memo = Mutex::new(Memo::<u32>::default());
    let mut asked_at = Instant::now();
    disk.clock.set(Some(asked_at));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
        Err(Absent::NoSessionFile)
    );
    for wait in [2, 4, 8, 16, 32, 60] {
        asked_at += Duration::from_secs(wait);
        disk.clock.set(Some(asked_at));
        assert_eq!(
            bind_remembered(&memo, 7, "claude", None, Some("aaa"), &disk),
            Err(Absent::NoSessionFile)
        );
    }
    // A new session id is asked at once, and its next ask comes ABSENT_RECHECK later.
    let walks = disk.walks.get();
    disk.clock.set(Some(asked_at));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("bbb"), &disk),
        Err(Absent::NoSessionFile)
    );
    assert_eq!(
        disk.walks.get(),
        walks + 1,
        "a new question is asked at once"
    );
    disk.clock
        .set(Some(asked_at + ABSENT_RECHECK - Duration::from_millis(1)));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("bbb"), &disk),
        Err(Absent::NoSessionFile)
    );
    assert_eq!(
        disk.walks.get(),
        walks + 1,
        "inside the first interval the new question is not asked again"
    );
    disk.clock.set(Some(asked_at + ABSENT_RECHECK));
    assert_eq!(
        bind_remembered(&memo, 7, "claude", None, Some("bbb"), &disk),
        Err(Absent::NoSessionFile)
    );
    assert_eq!(
        disk.walks.get(),
        walks + 2,
        "after the first interval the new question is asked again"
    );
}
