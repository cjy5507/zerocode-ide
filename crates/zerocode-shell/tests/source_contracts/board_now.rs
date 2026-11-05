//! The task board in the Geist language the person passed (t-67607, draft B,
//! artifact p-90883818b247d08e v2): four tabs (the schedule's is t-67482's),
//! one sentence on top, the 「당신 차례」 card, the 「빌드」 card and the
//! 「지금 도는 일」 table — and not one of the ledger's English sentences on a
//! row.

use std::path::Path;

use super::support::{block_after, strip_comments, window_source};

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
        "press.error = boardRefusalWords(error);",
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
/// language the window speaks (`LOCALES`, ui/shell-i18n.js), and each run's
/// build record and status.json carry the list (`plan`). The window keeps no
/// table of any build's steps — it names a step by the words its record
/// carries (`buildWords`), so a build it never knew stands in its own words —
/// and so no catalog key for one.
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
    // The lane names itself the same way, and reports each run as any build
    // does — the board reads that record, never status.json.
    let name = lane
        .lines()
        .find_map(|line| line.strip_prefix("BUILD_WORDS='"))
        .and_then(|rest| rest.split('\'').next())
        .expect("tools/release/lane.sh no longer names itself (BUILD_WORDS)");
    assert!(
        name.split('|').count() == langs.len()
            && name.split('|').all(|word| !word.trim().is_empty()),
        "the lane is not named once in each of {langs:?}: {name}"
    );
    assert!(
        lane.contains("build report begin")
            && lane.contains("--plan-file -")
            && lane.contains("<<< \"$PLAN_JSON\""),
        "the lane does not report its runs, with its steps, through the build record"
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
    for (door, reads) in [
        (
            "function buildSegmentWords(",
            "buildWords(first.words, first.name)",
        ),
        (
            "function paintBuildLog(",
            "buildWords(step.words, step.name)",
        ),
    ] {
        assert!(
            block_after(window, door).contains(reads),
            "`{door}` does not name a step by its record's words"
        );
    }
}

/// A project's schedule shows that project's builds alone (t-83513): it reads
/// the build records of the checkout the window asks about — the folder every
/// checkout of its repository shares, off the one shelf the build card reads —
/// and none of the release lane's own files, so no lane stands on a project it
/// does not build. The status bar answers no lane at all: the installed build
/// is the machine's, and a project's builds are the build card's to ask
/// (`board_builds`).
#[test]
fn a_projects_schedule_reads_the_builds_of_its_own_repository_alone() {
    let board = shell_source("cmd/board.rs");
    assert!(
        block_after(&board, "pub(crate) async fn schedule_timeline(")
            .contains("worktree.map_or_else(|| state.active_root(), PathBuf::from)"),
        "the schedule is not asked for the checkout on screen"
    );
    let look = block_after(&board, "fn schedule_look(");
    assert!(
        look.contains("git_dir::shared_of(checkout)")
            && look.contains("BUILD_SHELF")
            && look.contains(".history(shared, checkout, now_ms)"),
        "the schedule's builds are not the records of the checkout's repository:\n{look}"
    );
    let runtime = shell_source("update_runtime.rs");
    for lane in [
        "fn lane_releases(",
        "fn lane_dir_for(",
        "STATUS_FILE",
        "LANE_GREEN",
        "OUT_DIR",
    ] {
        assert!(
            !runtime.contains(lane),
            "the window reads the release lane's own files again (`{lane}`)"
        );
    }
    let system = shell_source("cmd/system.rs");
    let status = block_after(&system, "pub(crate) fn release_status(");
    assert!(
        !status.contains("lane_status_for(") && !status.contains("worktree"),
        "the status bar answers a lane again:\n{status}"
    );
}

/// The window shows the selected project's content alone (the build card's
/// plan, §8). The desk is published per project and asked for the checkout on
/// screen, with its roster cut to that desk's runs; the inbox is that
/// project's, the sidebar's other projects named only by their number; the
/// archive starts on the project and the artifacts on the project on screen;
/// and a switch of project asks each of them again.
#[test]
fn the_board_the_inbox_the_archive_and_the_artifacts_are_the_selected_projects() {
    let board = shell_source("cmd/board.rs");
    assert!(
        block_after(&board, "pub(crate) fn board_desk(")
            .contains("crate::orchestration::project_desk(&checkout)"),
        "the board's desk is not the checkout's project's"
    );
    let waiting = shell_source("cmd/waiting.rs");
    assert!(
        block_after(&waiting, "pub(crate) fn board_inbox(").contains("project_inbox("),
        "the inbox is not the checkout's project's"
    );
    let published = shell_source("orchestration.rs");
    assert!(
        published.contains("desk::Desks::read(ledger,")
            && published.contains("|run| desk::project_of(run) == project"),
        "the desk is not read once per project"
    );
    let code = strip_comments(window_source());
    let desk = block_after(&code, "function refreshDeskLedger(");
    assert!(
        desk.contains("invoke(\"board_desk\", { worktree: asked })")
            && desk.contains("if (asked !== activeWorktreePath)")
            && desk.contains("runs.has(row.run)"),
        "the board's desk or roster is not the checkout's project's:\n{desk}"
    );
    let inbox = block_after(&code, "async function refreshInbox(");
    assert!(
        inbox.contains("worktree: asked, projects:")
            && inbox.contains("if (asked !== activeWorktreePath)")
            && inbox.contains("paintProjectWaiting(next.elsewhere)"),
        "the inbox is not the checkout's project's:\n{inbox}"
    );
    assert!(
        code.contains("let vaultScope = \"project\";"),
        "the archive does not start on the selected project"
    );
    let picked = block_after(&code, "function artifactPickedProject(");
    assert!(
        picked.contains("if (artifactFilter.project === null) return artifactViewedProject();"),
        "the artifacts do not start on the project on screen:\n{picked}"
    );
    let follow = block_after(&code, "function followSelectedProject(");
    for ask in [
        "refreshDeskLedger()",
        "refreshInbox()",
        "refreshVault()",
        "followArtifactsProject()",
    ] {
        assert!(
            follow.contains(ask),
            "a switch of project does not ask `{ask}` again:\n{follow}"
        );
    }
    assert!(
        block_after(&code, "async function refreshWorktrees(").contains("followSelectedProject();"),
        "nothing follows a switch of project"
    );
}

/// The builds a board shows are the project's on screen: the window asks
/// `board_builds` with the checkout it shows and holds the answer with that
/// checkout, the backend reads the records of that checkout's repository (the
/// folder every checkout of it shares), and every place that says a build reads
/// the one answer for the checkout on screen.
#[test]
fn builds_stand_only_on_the_project_they_belong_to() {
    let board = shell_source("cmd/board.rs");
    let look = block_after(&board, "fn builds_look(");
    assert!(
        look.contains("git_dir::shared_of(checkout)") && look.contains(".look(&shared, checkout,"),
        "the builds are not read off the checkout's own repository:\n{look}"
    );
    assert!(
        block_after(&board, "fn builds_answer(").contains("builds_look(checkout, now_ms)"),
        "the card's answer is not the checkout's look"
    );
    let window = window_source();
    assert!(
        block_after(window, "function askBoardBuilds(")
            .contains("invoke(\"board_builds\", { worktree: asked, known })"),
        "the window does not say which checkout it asks the builds for"
    );
    assert!(
        block_after(window, "function buildsHere(")
            .contains("boardBuildsFor === activeWorktreePath"),
        "builds read for another checkout are shown"
    );
    for door in [
        "function paintDeskBuilds(",
        "function boardNextWords(",
        "function paintStackedWork(",
        "function paintStackedWorkCount(",
    ] {
        assert!(
            block_after(window, door).contains("buildsHere()"),
            "`{door}` draws builds read for another checkout"
        );
    }
    // The schedule the same way: asked for the checkout on screen, an answer for
    // another one dropped, and every place that draws it reading the one held
    // for the checkout on screen.
    let ask = block_after(window, "function refreshBoardSchedule(");
    assert!(
        ask.contains("worktree: asked")
            && ask.contains("if (asked !== activeWorktreePath) return;"),
        "the schedule does not say which checkout it asks for, or keeps another's answer:\n{ask}"
    );
    assert!(
        block_after(window, "function boardScheduleHere(")
            .contains("boardScheduleFor === activeWorktreePath"),
        "a schedule read for another checkout is shown"
    );
    for door in [
        "function paintBoardScheduleHost(",
        "function paintBoardScheduleHead(",
        "function boardScheduleGateWords(",
        "function boardScheduleLogName(",
    ] {
        assert!(
            block_after(window, door).contains("boardScheduleHere()"),
            "`{door}` draws a schedule read for another checkout"
        );
    }
}

/// The build card's section of the window's code: the card, its foot and the
/// order editor.
fn build_card_section(window: &str) -> &str {
    window
        .find("/* ---- 빌드 ----")
        .and_then(|from| window[from..].split_once("/* 데스크가 보이는 동안의 1분 박자 하나"))
        .map(|(card, _)| card)
        .expect("ui/shell-board.js lost its build card section")
}

/// The build card, the line above it and 쌓인 일 know no project and no tool:
/// any build that leaves its record stands in its own words. None of them spells
/// the release lane, its files, the build line or the words of one project's
/// builds — they read the record and nothing else.
#[test]
fn the_build_card_knows_no_project_and_no_tool() {
    let window = window_source();
    let card = build_card_section(window);
    let code = [
        card,
        block_after(window, "function boardNextWords("),
        block_after(window, "function pileBuildsSection("),
        block_after(window, "function paintStackedWorkCount("),
    ]
    .map(strip_comments);
    // The build line's folder, as a path or as a name — `board-build-line` is
    // the card's own first line.
    for spelled in [
        "lane.sh",
        "/build-line",
        "\"build-line\"",
        "status.json",
        "release_status",
        "releaseLane",
        "zerocode",
        "새 버전 발행",
        "결합 게이트",
    ] {
        for part in &code {
            assert!(
                !part.contains(spelled),
                "the build card spells `{spelled}`:\n{part}"
            );
        }
    }
}

/// The order file has one reader, one checker and one writer, in Rust (the
/// build card's plan, step 5): the card and its editor hold no detection table
/// and compose no TOML. What a project's root suggests rides the builds'
/// answer, the file to be written is Rust's text (`build_order_preview`), and
/// it is written only through `build_order_write`, over the text the editor
/// was opened at.
#[test]
fn the_order_file_is_read_detected_and_written_in_rust_alone() {
    let code = strip_comments(build_card_section(window_source()));
    for spelled in [
        "[[build]]",
        "build.toml",
        "gradlew",
        "pom.xml",
        "Dockerfile",
        "package.json",
        "Cargo.toml",
        "go.mod",
        "Makefile",
        "justfile",
    ] {
        assert!(
            !code.contains(spelled),
            "the build card spells `{spelled}`: the order file and what detects builds are Rust's"
        );
    }
    let preview = block_after(&code, "async function previewBuildOrder(");
    assert!(
        preview.contains("invoke(\"build_order_preview\", { file: { build: order.builds } })"),
        "the file to be written is not Rust's text:\n{preview}"
    );
    let save = block_after(&code, "async function saveBuildOrder(");
    assert!(
        save.contains("invoke(\"build_order_write\", {")
            && save.contains("version: order.draft.version ?? null")
            && save.contains("order.problem !== \"\" || order.preview === \"\""),
        "the order is written some other way than the one previewed, over the text it was opened at:\n{save}"
    );
    let answer = block_after(&code, "function askBoardBuilds(");
    assert!(
        answer.contains("suggest: Array.isArray(answer.suggest)"),
        "what the root suggests does not ride the builds' answer:\n{answer}"
    );
}

/// What a tool's output looks like is the progress table's alone
/// (`build_progress.toml`, read by `zo build`): the card draws the record's
/// `progress` and spells no tool's line.
#[test]
fn a_steps_progress_is_read_by_the_table_alone() {
    let card = strip_comments(build_card_section(window_source()));
    for spelled in ["> Task", "Compiling", "[INFO]", "Step "] {
        assert!(
            !card.contains(spelled),
            "the build card spells a tool's line `{spelled}`"
        );
    }
    assert!(
        card.contains("step?.progress?.done") && card.contains("step?.progress?.of"),
        "the build card does not draw the record's progress"
    );
}

/// The card's buttons start `zo build` outside the window and ask it to stop:
/// the window writes no build record, a press starts only the installed zo as
/// a group of its own, and it allows no command but the ones the order file
/// runs and the person was shown.
#[test]
fn the_cards_buttons_start_zo_build_and_write_no_record() {
    let buttons = include_str!("../../src/cmd/build_buttons.rs");
    let (shipped, _) = buttons.split_once("#[cfg(test)]").unwrap_or((buttons, ""));
    for written in [
        "builds::begin(",
        "builds::save(",
        "start_step(",
        "end_step(",
    ] {
        assert!(
            !shipped.contains(written),
            "the window writes a build record itself: `{written}`"
        );
    }
    assert!(
        shipped.contains("zo_path_under(&home)")
            && shipped.contains(".arg(\"build\")")
            && shipped.contains("command.process_group(0);"),
        "a press does not start the installed `zo build` as a group of its own"
    );
    assert!(
        shipped.contains(".filter(|command| allowed.contains(command))"),
        "a press allows commands beyond the ones the order file runs and the person was shown"
    );
    let code = strip_comments(build_card_section(window_source()));
    assert!(
        code.contains("invoke(\"build_start\", { worktree: activeWorktreePath,")
            && code.contains("invoke(\"build_stop\", { worktree: activeWorktreePath,"),
        "the card's buttons do not name the checkout on screen"
    );
}

/// Every sentence the build card, the line above it and 쌓인 일 say of a build is
/// asked for with its Korean words and is in the four other catalogs — and the
/// lane's own card left no word behind.
#[test]
fn the_build_cards_sentences_are_in_the_four_catalogs() {
    let i18n = include_str!("../../../../ui/shell-i18n.js");
    let window = window_source();
    let mut keys: Vec<&str> = window
        .match_indices("\"board.build.")
        .filter_map(|(at, _)| window[at + 1..].split('"').next())
        .collect();
    // The order editor's words its code asks for; the ones its markup carries are below.
    for asked in ["t(\"buildOrder.", "key: \"buildOrder."] {
        keys.extend(window.match_indices(asked).filter_map(|(at, said)| {
            window[at + said.len() - "buildOrder.".len()..]
                .split('"')
                .next()
        }));
    }
    keys.extend([
        "board.now.nextBuild",
        "board.now.nextLate",
        "board.now.nextStalled",
        "board.now.justStarted",
        "pile.builds",
        "pile.titleHalt",
    ]);
    keys.sort_unstable();
    keys.dedup();
    assert!(
        keys.len() > 20,
        "the build card's words are not found: {keys:?}"
    );
    let asked_with_words = |key: &str| {
        window
            .match_indices(&format!("t(\"{key}\","))
            .any(|(at, call)| window[at + call.len()..].trim_start().starts_with('"'))
            || window.contains(&format!("key: \"{key}\", word: \""))
    };
    for key in keys {
        assert!(
            asked_with_words(key),
            "`{key}` is not asked for with its Korean words"
        );
        assert_eq!(
            i18n.matches(&format!("\"{key}\":")).count(),
            4,
            "`{key}` is not in exactly the four catalogs (en, ja, zh, es)"
        );
    }
    // The words the order editor's and the approval's markup carries (`data-i18n`), with their Korean inside.
    let markup = include_str!("../../../../ui/index.html");
    for dialog in ["buildOrder.", "buildApprove."] {
        let marked: Vec<&str> = markup
            .match_indices(&format!("data-i18n=\"{dialog}"))
            .filter_map(|(at, said)| markup[at + said.len() - dialog.len()..].split('"').next())
            .collect();
        assert!(
            marked.len() >= 3,
            "the `{dialog}` dialog's markup lost its words: {marked:?}"
        );
        for key in marked {
            assert_eq!(
                i18n.matches(&format!("\"{key}\":")).count(),
                4,
                "`{key}` is not in exactly the four catalogs (en, ja, zh, es)"
            );
        }
    }
    for gone in [
        "\"board.now.line",
        "\"board.desk.releaseGreen\"",
        "\"board.desk.releaseRed\"",
    ] {
        assert!(
            !window.contains(gone),
            "the lane's card left `{gone}` behind"
        );
    }
}
