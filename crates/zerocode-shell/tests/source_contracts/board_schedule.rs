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
    for write in [
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
    ] {
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
