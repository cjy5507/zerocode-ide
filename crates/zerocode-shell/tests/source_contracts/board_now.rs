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

/// The type ladder is held by its values (review t-68962 P3 ③, t-70028).
///
/// Three window scenes compare a drawn size against the token meant to draw
/// it — they read `--text-2xs` and its neighbours instead of spelling `11px`,
/// so a token that moves takes its assertion along and both still agree. The
/// ladder itself is a decision — the Geist steps the person passed (t-67607):
/// 11 for a number inside a badge and a figure's legend, 12 for the smallest
/// words, then 13 · 14 · 16 · 24 — so it is held here, by value, and Geist's
/// named styles stand on those steps rather than on numbers of their own.
#[test]
fn the_type_ladder_is_held_by_its_values() {
    let tokens = include_str!("../../../../ui/tokens.css");
    let root = block_after(tokens, ":root {");
    for step in [
        "--text-3xs: 11px;",
        "--text-2xs: 12px;",
        "--text-xs: 12px;",
        "--text-sm: 13px;",
        "--text-md: 14px;",
        "--text-lg: 16px;",
        "--text-xl: 24px;",
    ] {
        assert!(root.contains(step), "the type ladder moved off `{step}`");
    }
    for (style, step) in [
        ("--type-heading-24:", "var(--text-xl)"),
        ("--type-heading-14:", "var(--text-md)"),
        ("--type-label-14:", "var(--text-md)"),
        ("--type-label-13:", "var(--text-sm)"),
        ("--type-label-12:", "var(--text-xs)"),
    ] {
        let line = root
            .lines()
            .find(|line| line.trim_start().starts_with(style))
            .unwrap_or_default();
        assert!(
            line.contains(step),
            "`{style}` no longer stands on `{step}`: {line}"
        );
    }
}

/// Every step the release lane runs is named by the lane itself (t-83513): its
/// table (`PHASE_WORDS`, tools/release/lane.sh) gives each step a name in every
/// language the window speaks (`LOCALES`, ui/shell-i18n.js), and status.json
/// carries the list (`plan`). The window keeps no table of a lane's steps — a
/// lane it never knew stands in its own words — and so no catalog key for one.
#[test]
fn every_release_step_is_named_by_the_lane_itself() {
    let lane = include_str!("../../../../tools/release/lane.sh");
    let langs: Vec<&str> = lane
        .lines()
        .find_map(|line| line.strip_prefix("PHASE_LANGS=\""))
        .and_then(|rest| rest.split('"').next())
        .expect("tools/release/lane.sh no longer names its PHASE_LANGS")
        .split_whitespace()
        .collect();
    let i18n = include_str!("../../../../ui/shell-i18n.js");
    let locales: Vec<&str> = i18n
        .split_once("const LOCALES = [")
        .and_then(|(_, rest)| rest.split_once("];"))
        .map(|(list, _)| list)
        .expect("ui/shell-i18n.js no longer lists its LOCALES")
        .split("{ code: \"")
        .skip(1)
        .filter_map(|tail| tail.split('"').next())
        .filter(|code| *code != "system")
        .collect();
    assert_eq!(
        langs, locales,
        "the lane names its steps in other languages than the window speaks"
    );
    let table = lane
        .split_once("PHASE_WORDS='")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(table, _)| table)
        .expect("tools/release/lane.sh no longer carries its PHASE_WORDS");
    let steps: Vec<Vec<&str>> = table
        .lines()
        .map(|line| line.split('|').collect())
        .collect();
    assert!(
        steps.len() >= 10,
        "the lane's table lost its steps: {table}"
    );
    for step in &steps {
        assert_eq!(
            step.len(),
            langs.len() + 1,
            "the lane's `{}` step is not named once in each of {langs:?}",
            step[0]
        );
        assert!(
            step.iter()
                .all(|word| !word.trim().is_empty() && !word.contains(['"', '\\'])),
            "a word of the lane's `{}` step is empty or needs escaping: {step:?}",
            step[0]
        );
    }
    assert!(
        lane.contains("\"plan\":%s") && lane.contains("\"$PLAN_JSON\""),
        "status.json no longer carries the lane's steps"
    );
    let window = window_source();
    assert!(
        !window.contains("DESK_RELEASE_PHASES") && !window.contains("board.now.phase"),
        "the window keeps a table of a lane's step names again"
    );
    assert!(
        !i18n.contains("\"board.now.phase"),
        "a catalog still names a lane's step"
    );
    let word = block_after(window, "function deskReleasePhaseWord(");
    assert!(
        word.contains("lane?.plan") && word.contains("String(name)"),
        "a step's name is not read off the lane's own plan:\n{word}"
    );
}

/// The board shows a release lane only on a project of the repository the lane
/// says it releases (t-83513): the lane writes the folder every checkout of its
/// repository shares (`git_common_dir`), the window holds the two folders to
/// one identity on disk — never a path's spelling or a repository's name — and
/// both the status bar's answer and the schedule's lanes go through it.
#[test]
fn a_lane_stands_only_on_the_repository_it_releases() {
    let lane = include_str!("../../../../tools/release/lane.sh");
    assert!(
        lane.contains("rev-parse --path-format=absolute --git-common-dir")
            && lane.contains("\"git_common_dir\":\"%s\""),
        "the lane no longer says which repository it releases"
    );
    let runtime = shell_source("update_runtime.rs");
    let releases = block_after(&runtime, "pub(crate) fn lane_releases(");
    assert!(
        releases.contains("git_dir::project_root(") && releases.contains("same_directory("),
        "the lane's repository is not told by the shared folder's identity:\n{releases}"
    );
    for spelled in [
        "\"zerocode",
        "file_name",
        "to_string_lossy",
        "== named",
        "starts_with",
    ] {
        assert!(
            !releases.contains(spelled),
            "the lane's repository is told by a spelling (`{spelled}`):\n{releases}"
        );
    }
    let same = block_after(&runtime, "fn same_directory(");
    assert!(
        same.contains(".dev()") && same.contains(".ino()"),
        "one folder is not told by its device and inode:\n{same}"
    );
    let system = shell_source("cmd/system.rs");
    assert!(
        block_after(&system, "pub(crate) fn release_status(").contains("lane_status_for("),
        "the status bar's lane is answered for every project"
    );
    let board = shell_source("cmd/board.rs");
    assert!(
        block_after(&board, "fn schedule_look(").contains("lane_dir_for("),
        "the schedule reads the lanes on every project"
    );
    let window = window_source();
    assert!(
        block_after(window, "function askReleaseStatus(")
            .contains("invoke(\"release_status\", { worktree: asked })"),
        "the window does not say which checkout it asks the lane for"
    );
    for (door, reader) in [
        ("function paintDeskRelease(", "releaseLaneHere()"),
        ("function boardNextWords(", "releaseLaneHere()"),
        ("function paintStackedWork(", "releaseLaneHere()"),
    ] {
        assert!(
            block_after(window, door).contains(reader),
            "`{door}` draws a lane read for another checkout"
        );
    }
}

/// The sentences the build line card and the line above it gained (t-83513)
/// are in the four catalogs beside the Korean one where they are used.
#[test]
fn the_build_lines_new_sentences_are_in_the_four_catalogs() {
    let i18n = include_str!("../../../../ui/shell-i18n.js");
    let window = window_source();
    for key in [
        "board.schedule.noLine",
        "board.now.nextLate",
        "board.now.nextStalled",
        "board.desk.releaseStalled",
        "board.desk.releaseStalledCap",
    ] {
        assert!(
            window.contains(&format!("t(\"{key}\", \"")),
            "`{key}` is not asked for with its Korean words"
        );
        assert_eq!(
            i18n.matches(&format!("\"{key}\":")).count(),
            4,
            "`{key}` is not in exactly the four catalogs (en, ja, zh, es)"
        );
    }
}
