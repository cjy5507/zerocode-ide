//! The task board in the Geist language the person passed (t-67607, draft B,
//! artifact p-90883818b247d08e v2): four tabs (the schedule's is t-67482's),
//! one sentence on top, the 「당신 차례」 card, the 「빌드 줄」 card and the
//! 「지금 도는 일」 table — and not one of the ledger's English sentences on a
//! row.

use std::path::Path;

use super::support::{block_after, window_source};

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// A window file read when the test runs, not when it compiles: a file the
/// board has yet to grow fails its assertion instead of the build.
fn ui_source(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("ui/{name}: {why}"))
}

/// Every kind of decision the desk sends is said in the person's language.
///
/// The ledger writes a hold as prose (`unexamined_question`, `examined_facts`
/// in zerocode-core), and that prose reached the board's rows as it was —
/// "w-… exited in … before reporting, and nobody has looked at what it left
/// there yet". The desk now sends the hold's kind beside it (`desk_gate`), and
/// the board keeps one table of what each kind means and what to do
/// (`DESK_HOLD_SUMMARY`), so a kind the backend can send and the table does
/// not know is a sentence the person cannot read. `asked` is the one kind that
/// is somebody's own words and is drawn as they wrote them.
#[test]
fn every_hold_the_desk_sends_is_said_in_the_persons_language() {
    let desk = shell_source("orchestration/desk.rs");
    let reading = block_after(&desk, "fn desk_gate(");
    let kinds: Vec<&str> = reading
        .split("=> \"")
        .skip(1)
        .filter_map(|tail| tail.split('"').next())
        .collect();
    assert!(
        kinds.len() >= 5 && kinds.contains(&"asked") && kinds.contains(&"unexamined"),
        "the desk's hold kinds could not be read: {kinds:?}"
    );
    let window = window_source();
    let table = block_after(window, "const DESK_HOLD_SUMMARY = Object.freeze({");
    for kind in kinds.iter().filter(|kind| **kind != "asked") {
        assert!(
            table.contains(&format!("  {kind}: {{ key: \"board.now.hold")),
            "the desk sends a `{kind}` hold the board cannot say in words:\n{table}"
        );
    }
    // The ledger's sentence stands behind 「원문 보기」 and nowhere else on a
    // row: the only readers of a gate's question are the words for an `asked`
    // gate and the raw line.
    let words = block_after(window, "function deskHoldWords(");
    assert!(
        words.contains("if (gate.kind === \"asked\") return gate.question;"),
        "a hold's own question is drawn for a kind that is not somebody's words:\n{words}"
    );
    let row = block_after(window, "function paintBoardHold(");
    assert_eq!(
        row.matches("gate.question").count(),
        1,
        "the hold row prints the ledger's sentence outside 「원문 보기」:\n{row}"
    );
    assert!(
        row.contains("boardRawLine(row, ask.key, held ? gate.question : \"\");"),
        "the ledger's sentence left its fold:\n{row}"
    );
}

/// The board's four tabs are one list — 지금 · 일정 · 워커 · 기록, the
/// schedule's since t-67482 — and the markup and the stylesheet carry
/// exactly them: a tab with no rule in shell.css would leave every other
/// tab's blocks standing under it.
#[test]
fn the_four_tabs_are_one_list_and_the_markup_carries_them() {
    let window = window_source();
    let markup = include_str!("../../../../ui/index.html");
    let styles = include_str!("../../../../ui/shell.css");
    let tabs = block_after(window, "const BOARD_TABS = Object.freeze([");
    let ids: Vec<&str> = tabs
        .split("{ id: \"")
        .skip(1)
        .filter_map(|tail| tail.split('"').next())
        .collect();
    assert_eq!(ids, ["now", "schedule", "workers", "history"], "{tabs}");
    for id in &ids {
        assert!(
            markup.contains(&format!("data-board-tab-button=\"{id}\"")),
            "the `{id}` tab has no button in ui/index.html"
        );
        assert!(
            styles.contains(&format!(
                "[data-board-tab=\"{id}\"] [data-board-tab-of]:not([data-board-tab-of=\"{id}\"])"
            )),
            "the `{id}` tab hides no other tab's blocks in ui/shell.css"
        );
    }
    // Each desk block names the tab it stands on, and every tab it names is
    // one of the four.
    let blocks = block_after(window, "const DESK_BLOCKS = Object.freeze([");
    for named in blocks.split("tab: \"").skip(1) {
        let tab = named.split('"').next().unwrap_or_default();
        assert!(
            ids.contains(&tab),
            "a desk block stands on a tab nobody can choose: `{tab}`"
        );
    }
    // The person's buttons speak through `data-tip`, never a `title`.
    let now = ui_source("shell-board-now.js");
    assert!(
        !now.contains("\"title\"") && !now.contains(".title ="),
        "the board's new buttons carry a native tooltip instead of `data-tip`"
    );
}

/// The two presses that answer for the person are the coordinator seat's,
/// on the person's press only, and their answer is drawn on the row.
#[test]
fn a_press_on_the_board_is_the_seats_and_its_answer_is_drawn() {
    let desk = shell_source("orchestration/desk.rs");
    for (door, verb) in [
        ("pub(crate) fn resolve_hold(", "\"gate-resolve\","),
        ("pub(crate) fn pass_review(", "\"task-update\","),
    ] {
        let body = block_after(&desk, door);
        assert!(
            body.contains("as_the_coordinator(") && body.contains(verb),
            "`{door}` no longer acts as the run's coordinator seat:\n{body}"
        );
    }
    let hold = block_after(&desk, "pub(crate) fn resolve_hold(");
    assert!(
        hold.contains("HOLD_ACTS") && !hold.contains("\"harvest\""),
        "a hold's answer is spelled in the window instead of read from the core:\n{hold}"
    );
    let window = window_source();
    let press = block_after(window, "async function pressBoardAsk(");
    for part in [
        "press.said = said(answer ?? {});",
        "press.error = String(error);",
        "retryRequest: press.retry.id",
    ] {
        assert!(
            press.contains(part),
            "the press lost `{part}` — its answer or its retry name is not kept:\n{press}"
        );
    }
}
