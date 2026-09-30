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

/// The desk answers and acknowledges through the ledger's own verbs, as the
/// run's coordinator seat, and invents neither. A reply is `reply` (the one-
/// answer rule stands); an acknowledgement is the named batch with `--peek`
/// (nothing new is leased away from the coordinator); both go through the
/// handover's native door with the seat's own capability, from the main
/// window only. What is owed is the reply verb's own rules and the inbox's own
/// state. And the window never takes a letter off the desk itself: after a
/// press it asks the ledger again, and only a delivered notice offers an ack.
#[test]
fn the_desk_answers_and_acknowledges_through_the_ledgers_own_verbs_and_invents_neither() {
    let desk = shell_source("orchestration/desk.rs");
    let seat = block_after(&desk, "fn as_the_coordinator(");
    for held in [
        "Run::coordinator_live",
        "crate::agent_teams::current_pane_capability(team, pane)",
        "super::coordinator_handover::command(team, pane, capability, argv, now_ms)",
    ] {
        assert!(seat.contains(held), "the seat door lost `{held}`:\n{seat}");
    }
    let reply = block_after(&desk, "pub(crate) fn reply(");
    for word in ["\"reply\"", "\"--to-message\"", "\"--retry-request\""] {
        assert!(reply.contains(word), "the reply lost `{word}`:\n{reply}");
    }
    let acknowledge = block_after(&desk, "pub(crate) fn acknowledge(");
    for word in [
        "\"check\"",
        "\"--ack\"",
        "\"--peek\"",
        "\"--retry-request\"",
    ] {
        assert!(
            acknowledge.contains(word),
            "the acknowledgement lost `{word}`:\n{acknowledge}"
        );
    }
    let owed = block_after(&desk, "pub(crate) fn desk_letters(");
    for rule in [
        "InboxState::of(run, &address)",
        "run.awaits_answer(message)",
        ".question_is_answerable(message)",
        "run.quiet_notice_stands(message)",
    ] {
        assert!(owed.contains(rule), "what is owed lost `{rule}`:\n{owed}");
    }
    let inbox = block_after(&desk, "fn of(run: &'a Run, address: &str) -> Self {");
    for rule in [".pending_messages(address, &[])", ".open_delivery(address)"] {
        assert!(
            inbox.contains(rule),
            "the inbox's state lost `{rule}`:\n{inbox}"
        );
    }
    let board = shell_source("cmd/board.rs");
    for door in [
        "pub(crate) async fn desk_reply(",
        "pub(crate) async fn desk_ack(",
    ] {
        let body = block_after(&board, door);
        assert!(
            body.contains("crate::from_the_main_webview(&webview)?")
                && body.contains("spawn_blocking"),
            "`{door}` is open to a popped-out board or runs on the main thread:\n{body}"
        );
    }

    let window = window_source();
    for (name, press) in [
        (
            "sendDeskReply",
            block_after(window, "async function sendDeskReply("),
        ),
        (
            "ackDeskBatch",
            block_after(window, "async function ackDeskBatch("),
        ),
    ] {
        assert!(
            press.contains("refreshDeskLedger()"),
            "{name} does not ask the ledger again:\n{press}"
        );
        for invention in [
            "deskLedger.mail",
            ".mail.filter",
            ".mail.splice",
            "deskLedger =",
        ] {
            assert!(
                !press.contains(invention),
                "{name} takes a letter off the desk the ledger still holds (`{invention}`):\n{press}"
            );
        }
    }
    let act = block_after(window, "function deskLetterAct(");
    assert!(
        act.contains("letter.delivery === \"delivered\"") && act.contains("isPopout"),
        "an ack is offered for a letter the coordinator has not been handed:\n{act}"
    );
}

/// The worker roster reads the rows the board and the navigator already read
/// — `ledger_agents`, through the one shared reader — and every fact of a
/// worker's health is the ledger's own function: the question it waits on
/// (`Run::awaiting_reply`), the wall its attempt met as the ledger reads it
/// back (`newest_wall`), the reconciler's proof its pane is gone. Its git
/// facts are the reclaim sweep's and the cleanup screen's own counts, for
/// checkouts this window catalogues, bounded, off the main thread.
#[test]
fn the_roster_reads_the_ledgers_rows_and_the_trees_own_counts() {
    let orchestration = shell_source("orchestration.rs");
    let rows = block_after(&orchestration, "fn ledger_agents_for_seats(");
    for fact in [
        "run.awaiting_reply(&worker.id)",
        "zerocode_core::orchestration::newest_wall(run, &one.id)",
        "worker.pane_missing_since_ms",
        "worker.model.clone()",
        "worker.effort.clone()",
    ] {
        assert!(rows.contains(fact), "the worker row lost `{fact}`:\n{rows}");
    }
    let runtime = shell_source("worktree_runtime.rs");
    let facts = block_after(&runtime, "pub(super) fn desk_checkout_facts(");
    for count in [
        ".commits_beyond(&worktree.path, base, branch)",
        "cleanup_git_evidence(",
        ".take(DESK_CHECKOUTS_MAX)",
        "orchestrator.list()",
    ] {
        assert!(
            facts.contains(count),
            "the checkout facts lost `{count}`:\n{facts}"
        );
    }
    let board = shell_source("cmd/board.rs");
    let door = block_after(&board, "pub(crate) async fn desk_checkouts(");
    assert!(
        door.contains("spawn_blocking"),
        "the roster's git runs on the main thread:\n{door}"
    );

    let window = window_source();
    let refresh = block_after(window, "function refreshDeskLedger(");
    assert!(
        refresh.contains("readLedgerAgents()") && !refresh.contains("invoke(\"ledger_agents\""),
        "the roster reads the ledger's workers a second way:\n{refresh}"
    );
}

/// What the desk owes is counted in one place (t-9456): 「답할 우편」 is the
/// ledger's own reading of a wait — `Run::awaits_answer`, the function
/// `Run::awaiting_reply` answers with — and 「소식」 is one table, `DESK_NEWS`,
/// that holds both the day a notice stands and the one kind that folds into a
/// line per quiet episode. The window draws the backend's three numbers and
/// its two lists as they come: it counts no letter, filters none, and names
/// no kind a letter to answer. The desk that said 「답할 우편 46」 over no
/// question at all was a screen counting every unacknowledged notice.
#[test]
fn the_desks_numbers_are_the_backends_and_the_window_counts_no_letter() {
    let desk = shell_source("orchestration/desk.rs");
    let table = block_after(&desk, "const DESK_NEWS: NewsTable = NewsTable {");
    for row in [
        "stands_ms: 24 * 60 * 60 * 1000,",
        "(MessageKind::WentQuiet, NewsLine::PerEpisode),",
        "(MessageKind::WorkerDied, NewsLine::PerNotice),",
    ] {
        assert!(table.contains(row), "the news table lost `{row}`:\n{table}");
    }
    assert_eq!(
        desk.matches("24 * 60 * 60 * 1000").count(),
        1,
        "a notice's day is spelled outside the table"
    );
    let letters = block_after(&desk, "pub(crate) fn desk_letters(");
    for rule in ["DESK_NEWS.line(message.kind)", "DESK_NEWS.stands_ms"] {
        assert!(
            letters.contains(rule),
            "the letters stopped reading the table (`{rule}`):\n{letters}"
        );
    }
    let snapshot = block_after(&desk, "pub(crate) fn desk_snapshot(");
    for count in ["mail: mail.len(),", "news: news.len(),", "folded,"] {
        assert!(
            snapshot.contains(count),
            "the desk's numbers lost `{count}`:\n{snapshot}"
        );
    }
    let core = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../zerocode-core/src/orchestration.rs"),
    )
    .expect("the core ledger");
    let waiting = block_after(
        &core,
        "fn awaiting_reply(run: &Run, worker_id: &str) -> bool {",
    );
    assert!(
        waiting.contains("awaits_answer(run, question)"),
        "a worker's wait and the desk's letters read two rules:\n{waiting}"
    );

    let window = window_source();
    let mail = block_after(window, "function paintDeskMail(");
    for number in [
        "const counts = deskLedger?.counts ?? {};",
        "Number(counts.mail)",
        "Number(counts.news)",
        "Number(counts.folded)",
        "{ count: owed }",
        "{ count: told }",
    ] {
        assert!(
            mail.contains(number),
            "the desk draws a number of its own (`{number}` missing):\n{mail}"
        );
    }
    let list = block_after(window, "function paintDeskLetterList(");
    for body in [mail, list] {
        for recount in [
            "letters.filter(",
            "news.filter(",
            ".mail.filter(",
            ".news.filter(",
            "letter.kind ===",
            "count: letters.length",
            "count: news.length",
        ] {
            assert!(
                !body.contains(recount),
                "the desk counts or picks letters itself (`{recount}`):\n{body}"
            );
        }
    }
    let kinds = block_after(window, "const DESK_MAIL = Object.freeze({");
    assert!(
        !kinds.contains("went_quiet: { state: \"needs-attention\""),
        "a quiet worker is drawn as waiting on the person:\n{kinds}"
    );
    let letter = block_after(window, "function paintDeskLetter(");
    assert!(
        !letter.contains("DESK_MAIL.went_quiet"),
        "a kind the table does not know is drawn as a silence:\n{letter}"
    );
}

/// A task the coordinator folded, handed over or retired reads 닫힘 and its
/// reason in words — never 실패 (t-19159, t-15558 slice 1).
///
/// Reported as the whole problem of the old tasks: work folded into another
/// task or handed to the other run could only be written `completed` or
/// `failed`, and the board showed the second as 실패. What is pinned is that the
/// desk has a stage of its own for it and draws it without the failed tone,
/// that the three reasons live in one table (`CLOSED_REASONS`) and say
/// 접힘 → t-…, 넘김 → run-…, 낡음, that the word is one key read by every
/// surface, and that all five languages carry those words.
#[test]
fn a_closed_task_reads_closed_with_its_reason_and_is_never_called_failed() {
    let window = window_source();
    let stages = block_after(window, "const DESK_STAGES = Object.freeze([");
    assert!(
        stages.contains(
            r#"{ id: "closed", flow: false, tone: "", key: "board.closed", word: "닫힘" }"#
        ),
        "the desk has no closed stage, or draws it in a signal tone:\n{stages}"
    );
    let reasons = block_after(window, "const CLOSED_REASONS = Object.freeze({");
    for held in [
        r#"key: "board.closedFolded", word: "접힘 → {{target}}""#,
        r#"key: "board.closedHandedOver", word: "넘김 → {{target}}""#,
        r#"key: "board.closedOutdated", word: "낡음""#,
    ] {
        assert!(
            reasons.contains(held),
            "the reasons lost `{held}`:\n{reasons}"
        );
    }
    let word = block_after(window, "function ledgerReviewWord(facts) {");
    assert!(
        word.contains(r#"if (facts.closed) return t("board.closed", "닫힘");"#),
        "a closed task's card does not say closed:\n{word}"
    );
    assert!(
        word.find("facts.closed") < word.find("stageFailed"),
        "a closed task is read as failed before it is read as closed:\n{word}"
    );
    let i18n = include_str!("../../../../ui/shell-i18n.js");
    for key in [
        "board.closed",
        "board.closedFolded",
        "board.closedHandedOver",
        "board.closedOutdated",
    ] {
        assert_eq!(
            i18n.matches(&format!("\"{key}\":")).count(),
            4,
            "{key} is not in every language but the source's own Korean"
        );
    }
    // The backend's stage list is the order the desk draws; closed is last.
    let desk = shell_source("orchestration/desk.rs");
    let table = block_after(&desk, "pub(crate) const STAGES: [&str; 9] = [");
    assert!(
        table
            .find("\"failed\"")
            .is_some_and(|failed| table.find("\"closed\"") > Some(failed)),
        "the backend's stages do not end with closed:\n{table}"
    );
}

/// A completed task nothing can review reads 완료 — 검토 기록 없음 and is
/// never called 검증 대기 (t-19328, t-15558 slice 2).
///
/// Reported as the pile of old completed work whose every attempt ended
/// handing nothing in: `task-update` refuses any review of it for ever, and the
/// board called it 검증 대기 all the same. What is pinned is that the LEDGER
/// decides it — one definition in core (`Run::handed_nothing_in`, the settle
/// pass's own reading) shipped as a flag on the review the rows already carry —
/// that neither the desk's stage nor the window judges it a second time, that
/// every surface reads the one word from the one seam and the one key, and
/// that the desk draws the backend's stages in the backend's order.
#[test]
fn a_completed_task_nothing_can_review_reads_no_review_record_and_never_awaiting_review() {
    // One definition, in core, reused — by the settle pass and by the review.
    let core = include_str!("../../../zerocode-core/src/orchestration.rs");
    assert_eq!(
        core.matches("fn handed_nothing_in(").count(),
        1,
        "the predicate is defined more than once"
    );
    for (opens, what) in [
        ("fn quiet_unhanded(", "the settle pass's listing"),
        (
            "pub fn review_of(&self, task: &Task) -> ReviewFacts {",
            "the review the rows carry",
        ),
    ] {
        assert!(
            block_after(core, opens).contains(".handed_nothing_in("),
            "{what} does not ask the one definition"
        );
    }

    // The desk's stage reads the core's review; it does not count attempts.
    let desk = shell_source("orchestration/desk.rs");
    let stage = block_after(
        &desk,
        "fn stage_of(run: &Run, task: &Task) -> &'static str {",
    );
    assert!(
        stage.contains("run.review_of(task)") && stage.contains(".unreviewable"),
        "the desk's stage does not read the ledger's own fact:\n{stage}"
    );
    assert!(
        !stage.contains(".dispatches") && !stage.contains(".source"),
        "the desk's stage counts attempts itself:\n{stage}"
    );

    // The window reads the flag and nothing else, ahead of 실패 and 검증 대기.
    let window = window_source();
    let word = block_after(window, "function ledgerReviewWord(facts) {");
    assert!(
        word.contains(
            r#"if (review.unreviewable) return t("board.noReviewRecord", "완료 — 검토 기록 없음");"#
        ),
        "a completed task nothing can review does not say so:\n{word}"
    );
    assert!(
        word.find("review.unreviewable") < word.find("stageFailed")
            && word.find("review.unreviewable") < word.find("board.awaitingReview"),
        "the word is read as failed or as waiting before it is read as unreviewable:\n{word}"
    );

    // The desk's stage, the board's live map and the sidebar all place it.
    let stages = block_after(window, "const DESK_STAGES = Object.freeze([");
    assert!(
        stages.contains(
            r#"{ id: "unreviewable", flow: false, tone: "", key: "board.noReviewRecord", word: "완료 — 검토 기록 없음" }"#
        ),
        "the desk has no stage for it, or draws it in a signal tone:\n{stages}"
    );
    let live = block_after(window, "const AGENT_GRAPH_LIVE_STAGES = Object.freeze([");
    assert!(
        live.contains(
            r#"{ stage: "unreviewable", flag: "unreviewable", key: "board.noReviewRecord", word: "완료 — 검토 기록 없음" }"#
        ),
        "the live map cannot read the word back as a stage:\n{live}"
    );
    let placed = block_after(window, "const LEDGER_REVIEW_PHASE = Object.freeze({");
    assert!(
        placed.contains("unreviewable: \"unreviewable\""),
        "the sidebar does not place the stage:\n{placed}"
    );
    let rank = block_after(window, "const WORKTREE_PHASE_RANK = Object.freeze({");
    assert!(
        rank.contains("unreviewable: 2"),
        "a checkout's phase ranking does not know the stage:\n{rank}"
    );

    // The window draws the backend's stages, in the backend's order.
    let table = block_after(&desk, "pub(crate) const STAGES: [&str; 10] = [");
    let backend: Vec<&str> = table
        .split("];")
        .next()
        .unwrap_or_default()
        .split('"')
        .skip(1)
        .step_by(2)
        .collect();
    let drawn: Vec<&str> = stages
        .split("]);")
        .next()
        .unwrap_or_default()
        .split("id: \"")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap_or_default())
        .collect();
    assert_eq!(
        drawn, backend,
        "the desk draws its stages in an order the backend does not send"
    );
    assert!(
        backend.contains(&"unreviewable"),
        "the backend has no stage for it: {backend:?}"
    );

    // One key, read by every surface, in every language but the source's Korean.
    let i18n = include_str!("../../../../ui/shell-i18n.js");
    assert_eq!(
        i18n.matches("\"board.noReviewRecord\":").count(),
        4,
        "board.noReviewRecord is not in every language but the source's own Korean"
    );
}
