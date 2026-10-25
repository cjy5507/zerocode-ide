//! The task board's schedule (t-67482): one read-only command over the build
//! line's queue and the release lane's logs, run off the main thread, and a
//! window part that asks it only while the view is seen — with a sentence for
//! every state the command can say.

use std::path::Path;

use super::support::{
    WINDOW_PARTS, block_after, strip_comments, strip_rust_comments, window_source,
};

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// What writes, moves, renames or locks a file, or opens the ledger's
/// database — none of it in the schedule's reading, nor in its command.
const WRITES: [&str; 15] = [
    "fs::write",
    "fs::rename",
    "fs::remove",
    "fs::create_dir",
    "fs::copy",
    "OpenOptions",
    "File::create",
    "File::options",
    ".set_len(",
    ".set_modified(",
    "flock",
    "try_lock",
    "rusqlite",
    "sqlite",
    "Ledger",
];

/// The build line and the lane are other processes' files: the command reads
/// them on the blocking pool, and the module never writes, moves, renames or
/// locks one — nor opens the ledger's database, which a second process must
/// not touch while the window holds it (trap 611). Its folders come from the
/// window's settings and constants, and its stage names from the logs.
#[test]
fn the_schedule_reads_two_folders_off_the_main_thread_and_writes_nothing() {
    let board = shell_source("cmd/board.rs");
    let command = block_after(&board, "pub(crate) async fn schedule_timeline(");
    assert!(
        command.contains("spawn_blocking")
            && command.contains("crate::update_runtime::release_dir()"),
        "the schedule reads on the main thread, or finds the lanes another way:\n{command}"
    );
    let module = strip_rust_comments(&shell_source("schedule_timeline.rs"));
    for write in WRITES {
        assert!(
            !module.contains(write),
            "the schedule touches what it only reads (`{write}`)"
        );
    }
    for spelled in [
        "/Users/",
        "workspaces/",
        ".local/share",
        "\"gate-root\"",
        "\"build-app\"",
        "\"green\"",
    ] {
        assert!(
            !module.contains(spelled),
            "the schedule spells a path or a lane's word itself (`{spelled}`)"
        );
    }
    for road in [
        "crate::update_runtime::OUT_DIR",
        "crate::update_runtime::STATUS_FILE",
        "crate::update_runtime::read_json(",
        "LINE_FOLDER",
    ] {
        assert!(module.contains(road), "the schedule lost `{road}`");
    }
}

/// The schedule reads other processes' files, and every such read goes through
/// the one reader that opens a plain file and nothing else, never past a cap
/// (`plain_read`, the review t-69069 P3 ④): no `std::fs::read`, no
/// `File::open`, no `read_to_string` in the module that could follow a link
/// or wait on a pipe with the book's lock held. The reader itself only opens
/// for reading. And the runner's caps are numbers the module reads off the
/// runner's script, spelled once as the fallback (⑥) — a second copy is a
/// number that drifts.
#[test]
fn the_schedule_reads_another_processs_files_only_through_the_plain_reader() {
    let module = strip_rust_comments(&shell_source("schedule_timeline.rs"));
    for open in [
        "std::fs::read(",
        "fs::read_to_string(",
        "File::open(",
        "fs::File::open(",
    ] {
        assert!(
            !module.contains(open),
            "the schedule opens a file of another process some other way (`{open}`)"
        );
    }
    assert!(
        module.matches("crate::plain_read::").count() >= 5,
        "the schedule lost the plain reader"
    );
    for number in ["900", "1800"] {
        assert_eq!(
            module.matches(number).count(),
            1,
            "the runner's cap {number} is spelled more than once (or not at all): read it off the script"
        );
    }
    let reader = strip_rust_comments(&shell_source("plain_read.rs"));
    let code = reader
        .split_once("#[cfg(test)]")
        .map_or(reader.as_str(), |(code, _)| code);
    assert!(
        code.contains(".read(true)") && code.contains("O_NOFOLLOW") && code.contains("O_NONBLOCK"),
        "the plain reader lost how it opens"
    );
    for write in WRITES
        .iter()
        .filter(|word| !["File::options", "OpenOptions"].contains(word))
        .chain(&[".write(", ".append(", ".create(", ".truncate("])
    {
        assert!(
            !code.contains(write),
            "the plain reader writes or creates (`{write}`)"
        );
    }
}

/// The window compares none of the lane's own words (the review t-69069 P3
/// ⑦): the answer says how a lane ended in the board's words, and the part
/// reads that.
#[test]
fn the_schedule_view_compares_no_word_of_the_lanes_own() {
    let part = WINDOW_PARTS
        .iter()
        .find(|(name, _)| *name == "shell-board-schedule.js")
        .map(|(_, source)| *source)
        .expect("ui/shell-board-schedule.js is a window part");
    let code = strip_comments(part);
    for word in ["\"green\"", "\"red\"", "'green'", "'red'"] {
        assert!(
            !code.contains(word),
            "the schedule view spells the lane's word {word}: the answer's `result` says it"
        );
    }
    assert!(
        code.contains("run.result"),
        "the schedule view does not read the lane's result"
    );
}

/// The command finds the build line by the window's workspace setting, and
/// reads that setting from memory (`remembered_settings`, t-67102). Through
/// the settings repository a look took its process lock and its
/// `.settings.lock`, waited behind any settings commit holding them through
/// its fsync, and could write the document back — from a view that asks
/// every five seconds while it is seen (the review t-69069, P2-1). The
/// command and the look it runs hold none of the module's writes either.
#[test]
fn the_schedule_reads_the_workspace_setting_from_memory_and_locks_nothing() {
    let board = shell_source("cmd/board.rs");
    let command = block_after(&board, "pub(crate) async fn schedule_timeline(");
    let look = block_after(&board, "fn schedule_look(");
    assert!(
        command.contains("schedule_look("),
        "the command looks at the schedule another way:\n{command}"
    );
    assert!(
        look.contains("remembered_settings("),
        "the schedule reads the workspace setting some other way:\n{look}"
    );
    for block in [command, look] {
        let code = strip_rust_comments(block);
        for word in WRITES.iter().chain(&[
            "load_settings_for_boot",
            "load_settings_resilient",
            "load_settings(",
            "mutate_settings",
            "mutate_json",
            "read_json",
            "write_json",
            ".settings.lock",
            "settings_lock",
        ]) {
            assert!(
                !code.contains(word),
                "the schedule's command reads the settings repository or writes (`{word}`):\n{code}"
            );
        }
    }
}

/// The schedule is the board's second tab (t-67482 over t-67607's draft B):
/// its block stands on the 「일정」 tab, the tab is the board's one choice
/// (`boardNowTab`, `chooseBoardTab`), and the view draws and asks only while
/// the surface the tabs just wrote stands on that tab with the ledger's runs
/// behind it. The 「지금 | 일정」 switch that came before the tabs is gone.
#[test]
fn the_schedule_is_the_boards_second_tab_and_opens_only_on_it() {
    let markup = include_str!("../../../../ui/index.html");
    let styles = include_str!("../../../../ui/shell.css");
    assert!(
        markup.contains(
            "<section class=\"task-board-schedule\" hidden data-board-tab-of=\"schedule\""
        ),
        "the schedule's block does not stand on the 「일정」 tab"
    );
    assert!(
        markup.contains("data-board-tab-button=\"schedule\"")
            && markup.contains("data-i18n=\"board.schedule.tab\">일정</button>"),
        "the 「일정」 tab has no button with its own words"
    );
    let window = window_source();
    for gone in [
        "data-task-board-view",
        "task-board-views",
        "is-schedule",
        "boardScheduleChoice.view",
    ] {
        assert!(
            !markup.contains(gone) && !styles.contains(gone) && !window.contains(gone),
            "the switch that came before the tabs is back (`{gone}`)"
        );
    }
    let open = block_after(window, "function paintBoardSchedule(");
    assert!(
        open.contains("surface.dataset.boardTab === \"schedule\"")
            && open.contains("surface.dataset.boardLedger === \"on\""),
        "the schedule opens on something other than its tab:\n{open}"
    );
    let desk = block_after(window, "function paintCoordinatorDesk(");
    let at = |needle: &str| desk.find(needle).unwrap_or(usize::MAX);
    assert!(
        at("paintBoardTabs(view);") < at("paintBoardSchedule(view);")
            && desk.matches("paintBoardSchedule(").count() == 1,
        "the schedule reads a tab the desk has not written yet:\n{desk}"
    );
}

/// The view asks only while it is seen, with the revision it holds, and asks a
/// log only by a name its answer listed; every state word the command writes
/// has the view's own reading, and the part is one the contracts read.
#[test]
fn the_schedule_view_asks_only_while_seen_and_says_every_state() {
    let part = WINDOW_PARTS
        .iter()
        .find(|(name, _)| *name == "shell-board-schedule.js")
        .map(|(_, source)| *source)
        .expect("ui/shell-board-schedule.js is a window part");
    let window = window_source();
    let poller = block_after(window, "const boardSchedulePoller = idlePoller({");
    assert!(
        poller.contains("wanted: () => boardScheduleHosts().length > 0")
            && poller.contains("every: BOARD_SCHEDULE.everyMs"),
        "the schedule polls while nobody sees it:\n{poller}"
    );
    let ask = block_after(window, "function refreshBoardSchedule(");
    assert!(
        ask.contains("invoke(\"schedule_timeline\", { known,"),
        "the schedule asks without the revision it holds:\n{ask}"
    );
    let log = block_after(window, "function openBoardScheduleLog(");
    assert!(
        log.contains("log: ask") && !log.contains("read_text_file"),
        "a log is read some other way:\n{log}"
    );
    let code = strip_comments(part);
    for tooltip in ["title =", ".title=", "\"title\""] {
        assert!(
            !code.contains(tooltip),
            "a tooltip outside the one delegated listener (`{tooltip}`)"
        );
    }

    // Every state the backend writes, read off its own constants.
    let module = shell_source("schedule_timeline.rs");
    let states = module
        .split_once("/* A job's state, one word each")
        .and_then(|(_, rest)| rest.split_once("/* The runner's three classes"))
        .map(|(states, _)| states)
        .expect("the backend's state words");
    let words: Vec<&str> = states
        .lines()
        .filter_map(|line| line.trim().strip_prefix("const "))
        .filter_map(|line| line.split_once("= \""))
        .filter_map(|(_, rest)| rest.strip_suffix("\";"))
        .collect();
    assert_eq!(words.len(), 8, "the backend's states moved: {words:?}");
    let table = block_after(window, "const BOARD_SCHEDULE_JOB_STATES = Object.freeze({");
    for word in words {
        assert!(
            table.contains(&format!("{word}:")),
            "the view has no reading for the state `{word}`:\n{table}"
        );
    }
}

/// What git says of the board's tasks (t-71840) is asked at the open
/// repository alone — the root the window stands in — through the artifacts
/// tab's own reading, whose answer is kept by the base's commit
/// (`artifact_flow::git_facts`), and only while the desk holds a task. The
/// reader of the two folders asks git nothing itself, so a quiet look stays a
/// look at files.
#[test]
fn the_schedule_asks_git_only_at_the_open_repository_through_the_flows_reading() {
    let board = shell_source("cmd/board.rs");
    let look = strip_rust_comments(block_after(&board, "fn schedule_look("));
    assert!(
        look.contains("match repo.as_deref()")
            && look.contains("crate::artifact_flow::git_facts(root, now_ms)")
            && look.contains("crate::artifact_flow::landings(")
            && look.contains("crate::orchestration::board_ledger_snapshot()"),
        "the schedule asks git, or finds the desk's tasks, some other way:\n{look}"
    );
    let module = strip_rust_comments(&shell_source("schedule_timeline.rs"));
    for asked in ["git_facts(", "vcs()", "landings("] {
        assert!(
            !module.contains(asked),
            "the schedule's reader asks git or the desk itself (`{asked}`)"
        );
    }
}
