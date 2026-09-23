//! The coordinator's desk on the task board (t-6588,
//! docs/design/agent-board-round4.md): what a coordinator typed the ledger's
//! CLI, `df -g`, `uptime`, the lane's `status.sh` and `xcrun simctl list` for,
//! answered from what this window already holds — one reader per fact, and no
//! fact the ledger judges judged a second time.

use std::path::Path;

use super::support::{block_after, window_source};

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// The strip's disk word is the verdict the next `--worktree` summons meets —
/// the core's `worktree_room` beside the checkouts the board's beat counted
/// the summons' way — measured on the ledger's own volume with the ledger's
/// own `statvfs`, spelled in the ledger's own ladder. Its load is the
/// window's one reader of it (the walk bench no longer spawns `sysctl`), and
/// its answer is read off the main thread (`simctl` and `adb` are processes).
#[test]
fn the_machine_strip_speaks_the_ledgers_disk_rule_and_reads_the_load_once() {
    let desk = shell_source("orchestration/desk.rs");
    let read = block_after(&desk, "pub(crate) fn machine_load(");
    for held in [
        "super::board_ledger_snapshot().held_checkouts",
        "super::free_bytes_at(ledger_volume)",
        "zerocode_core::workspace_space::format_bytes(free_bytes)",
        "worktree_room(free_bytes, held)",
        "crate::emulator::booted_devices()",
        "load_average()",
    ] {
        assert!(read.contains(held), "the strip lost `{held}`:\n{read}");
    }
    assert!(
        !read.contains("1024") && !read.contains("WORKTREE_BUDGET_BYTES"),
        "the strip carries a disk threshold of its own:\n{read}"
    );
    let orchestration = shell_source("orchestration.rs");
    let refresh = block_after(&orchestration, "pub(crate) fn refresh_board_ledger(");
    assert!(
        refresh.contains("zerocode_core::orchestration::held_checkouts(ledger).len()"),
        "the held checkouts are not counted the summons' way:\n{refresh}"
    );
    let bench = shell_source("emulator/walk_bench.rs");
    assert!(
        !bench.contains("vm.loadavg") && bench.contains("desk::load_average()"),
        "the walk bench reads the load a second way"
    );
    let board = shell_source("cmd/board.rs");
    let command = block_after(&board, "pub(crate) async fn machine_load(");
    assert!(
        command.contains("spawn_blocking"),
        "the strip's processes run on the main thread:\n{command}"
    );

    // The window: lent devices are the status bar's one sentence, from the
    // one state, and a disk verdict it does not know is not drawn.
    let window = window_source();
    let segments = block_after(window, "function deskMachineSegments(");
    assert!(
        segments.contains("emulatorLoansWords(now)"),
        "the strip counts lent devices a second way:\n{segments}"
    );
    let chip = block_after(window, "function paintEmulatorLoans(");
    assert!(
        chip.contains("emulatorLoansWords("),
        "the status bar's loan chip says its own sentence:\n{chip}"
    );
    let rooms = block_after(window, "const DESK_DISK_ROOM = Object.freeze({");
    for word in ["refused:", "tight:", "room:"] {
        assert!(
            rooms.contains(word),
            "the strip lost the verdict `{word}`:\n{rooms}"
        );
    }
}
