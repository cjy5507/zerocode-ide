//! The answer door (t-26594): an answer a card prepared is typed into a pane
//! through one door that reads the screen again first, one answer at a time
//! per pane — and the screen is read only where a pane waits or an answer is
//! about to be typed, never per output chunk.
//!
//! The behaviour is `answer_door`'s own unit tests and the core's
//! `screen_menu`; these contracts hold what a test of one function cannot see:
//! that no road goes round the door, and where the reading is NOT allowed.

use std::path::Path;

use super::quiet_children::{code_only, shell_sources};
use super::support::{block_after, shipped_backend, strip_rust_comments, window_source};

fn ui_file(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// The door's source. A sibling module, read through its own `include_str!`
/// like the other sibling modules: the shipped-backend haystack is a fixed
/// list (`every_backend_module_is_in_the_shipped_haystack`) and a module that
/// owns its tests beside it stays out of it.
const DOOR: &str = include_str!("../../src/answer_door.rs");

/// zo's channel messages, a sibling module with its own tests like the door:
/// where the queue's steer for a zo pane is sent (t-86909).
const ZO_NOTICE: &str = include_str!("../../src/zo_notice.rs");

/// The words a turn door refuses with, which live in the core's catalog so no
/// door's module is where a wire or a channel reaches for them (t-86909).
const CORE_AGENT: &str = include_str!("../../../zerocode-core/src/agent.rs");

/// The door's shipped half, comments off: everything before its test module.
fn door_code() -> String {
    strip_rust_comments(
        DOOR.split_once("#[cfg(test)]")
            .map_or(DOOR, |(code, _)| code),
    )
}

/// The three commands that type what a card chose never write to the pty
/// themselves: they take the pane's lease and spend their keys through the
/// door, whose first key is typed only if the screen still shows the
/// question.
#[test]
fn every_answer_a_card_prepared_is_typed_through_the_door() {
    let shipped = shipped_backend();
    for command in [
        "fn answer_ask(",
        "fn answer_on_menu(",
        "fn answer_approval(",
    ] {
        assert!(
            shipped.contains(command),
            "{command} is gone — the road that types an answer was renamed or moved"
        );
        let body = block_after(shipped, command);
        assert!(
            !body.contains("write_input(") && !body.contains("with_terminal("),
            "{command} types into the pane itself instead of through the door:\n{body}"
        );
        assert!(
            body.contains("answer_door::"),
            "{command} does not use the answer door:\n{body}"
        );
    }
    // A menu takes one row, and which row is the door's to say.
    assert!(
        block_after(shipped, "fn answer_on_menu(")
            .contains("answer_door::chosen_row(question, selections)"),
        "the menu path takes its row from somewhere other than the door's one-row rule"
    );
    // The lease is taken BEFORE anything is read or typed, so two answers to
    // one pane cannot both verify against the screen neither has answered yet.
    for command in ["fn answer_ask(", "fn answer_approval("] {
        let body = block_after(shipped, command);
        let leased = body
            .find("answer_door::answer_lease(term)")
            .unwrap_or_else(|| panic!("{command} takes no lease"));
        let typed = body
            .find("answer_door::type_if_up(")
            .or_else(|| body.find("answer_on_menu("))
            .unwrap_or_else(|| panic!("{command} types nothing through the door"));
        assert!(leased < typed, "{command} types before it holds the lease");
    }
    // A refusal is a word the page can read, not a sentence nobody can match.
    let door = door_code();
    for word in [
        "\"question-changed\"",
        "\"answer-in-flight\"",
        "\"menu-needs-a-row\"",
    ] {
        assert!(door.contains(word), "the door lost its refusal word {word}");
    }
}

/// Read and written in one hold of the pane's lock — the pump needs that lock
/// to move output into the grid, so no redraw this window parsed can fall
/// between the look and the key — and one answer at a time on the lease a
/// `zerocode-ssh send` already takes.
#[test]
fn the_door_reads_and_types_in_one_hold_of_the_pane_and_leases_the_pane() {
    let door = door_code();
    let typing = block_after(&door, "fn type_if_up(");
    let held = typing
        .find("lock_pty(held)")
        .expect("type_if_up holds the pane");
    let looked = typing
        .find("shows(&pty, expect)")
        .expect("type_if_up looks first");
    let wrote = typing
        .find("pty.write_input(bytes)")
        .expect("type_if_up writes");
    assert!(
        held < looked && looked < wrote,
        "the look and the key are no longer one hold of the pane's lock:\n{typing}"
    );
    let lease = block_after(&door, "fn answer_lease(");
    assert!(
        lease.contains("ssh_send_guard::send_lease(term)") && lease.contains("try_lock_owned()"),
        "the pane's one-at-a-time rule is no longer the send lease, refused rather than waited for:\n{lease}"
    );
    // The walk of a menu looks before it takes the row: the selection has to be
    // on the chosen row in the same hold as the Enter.
    let choosing = block_after(&door, "fn choose_row(");
    assert!(
        choosing.contains("Expect::question(question).on_row(to)")
            && choosing.contains("press_enter()"),
        "a menu's row is taken without looking where the selection landed:\n{choosing}"
    );
}

/// The hand-over's exit command is a line the window types by itself, and a
/// menu takes its Enter for the highlighted row — which may be a permission
/// nobody gave. Its first key goes through the door, which refuses a pane that
/// is standing on a menu, before the rest of the line is walked.
#[test]
fn the_hand_overs_exit_command_is_not_typed_into_a_menu() {
    let hand = block_after(shipped_backend(), "fn hand_over_pane(");
    let door = hand
        .find("crate::answer_door::type_if_up(")
        .expect("the exit command's first key does not go through the door");
    let walk = hand
        .find("walk_key_groups(state, term, rest, step)")
        .expect("the rest of the exit command is not walked");
    assert!(
        hand.contains("answer_door::Expect::no_menu()") && door < walk,
        "the exit command is typed before the pane is looked at:\n{hand}"
    );
}

/// The cost claim. The reading is small but not free — the ignored
/// measurements beside `screen_menu` and `answer_door` print it: tens of
/// microseconds to read a screen, a few hundred with the copy out of the
/// terminal, a couple of milliseconds on the efficiency cores — and it is paid
/// where a person is waiting: when a pane that WAITS is listed for the board,
/// and when an answer is about to be typed. It is not paid per output chunk, so
/// a busy pane's throughput cannot be touched by it — which is held here by
/// where the reading is allowed to appear at all.
#[test]
fn the_screen_is_read_only_where_a_pane_waits_or_an_answer_is_about_to_be_typed() {
    // Every Rust source of the shell, comments and strings off — the same walk
    // the quiet-children contract takes, so a new sibling file is read too.
    for (name, source) in shell_sources() {
        let code = code_only(&source);
        if name != "answer_door.rs" {
            assert!(
                !code.contains("screen_menu::"),
                "{name} reads the screen's menu itself — the reading lives in answer_door.rs"
            );
        }
        // The secret watcher (t-26596) reads a pane once for each stretch of
        // output that has gone quiet — the moment the pane waits — and never per
        // chunk; its own tests pin that (`secret_watch::tests`).
        if ![
            "answer_door.rs",
            "cmd/terminal.rs",
            "cmd/board.rs",
            "cmd/wire.rs",
            "secret_watch.rs",
        ]
        .contains(&name.as_str())
        {
            assert!(
                !code.contains("answer_door::"),
                "{name} reaches the answer door — only the answer commands, the hand-over's exit command and the board's waiting cards may"
            );
        }
    }
    // The pump moves every output chunk of every pane. Nothing in it looks.
    let shipped = shipped_backend();
    let pump = block_after(shipped, "fn pump_loop(");
    assert!(
        !pump.contains("answer_door")
            && !pump.contains("screen_menu")
            && !pump.contains("read_menu"),
        "the pump reads a screen's menu per output chunk"
    );
    // The board asks only about a pane that waits and describes nothing, and
    // never waits for a terminal being parsed.
    let cards = block_after(shipped, "fn screen_cards(");
    assert!(
        cards.contains("HookState::NeedsAttention")
            && cards.contains("row.ask_prompt.is_none()")
            && cards.contains("row.approval.is_none()"),
        "the board reads the screen of a pane that is not waiting without a card of its own:\n{cards}"
    );
    let door = door_code();
    let reading = block_after(&door, "fn screen_card(");
    assert!(
        reading.contains("held.try_lock()") && !reading.contains("lock_pty("),
        "the board's reading of a screen waits for the terminal's lock on the window's own thread:\n{reading}"
    );
    // Taken before `agent_terms` is locked, as `living` is.
    let listing = block_after(shipped, "fn pane_agents(");
    let taken = listing
        .find("screen_cards(&state, &states)")
        .expect("pane_agents asks for the waiting panes' cards");
    let locked = listing
        .find(".agent_terms()")
        .expect("pane_agents lists the agent panes");
    assert!(
        taken < locked,
        "the waiting panes' screens are read while agent_terms is locked:\n{listing}"
    );
}

/// The contract above reads which files name `answer_door::`. A `pub use`
/// of the door's items in an allowed file would let any other file reach
/// them under that file's name, and the reading would never see it — so the
/// door is not handed on (t-84470, r1 P3-4). A caller outside the answer
/// commands asks them for what it needs to know, in functions of their own.
#[test]
fn no_source_hands_the_answer_door_on() {
    for (name, source) in shell_sources() {
        for line in code_only(&source).lines() {
            let line = line.trim_start();
            assert!(
                !(line.starts_with("pub") && line.contains("use ") && line.contains("answer_door")),
                "{name} hands the answer door on: {line}"
            );
        }
    }
}

/// What the page does with a refusal and with a wait it was only told about:
/// the backend refuses with a WORD, and the page turns it into its own
/// sentence in all five catalogs (the Korean default stands in the call, the
/// other four in the catalog file) on both roads that answer from a card; and
/// a wait the hook only signalled — a menu, a prompt with no payload — is
/// asked about again a bounded number of times as the program's screen
/// settles, which is how the menu's card reaches the board.
#[test]
fn a_refusal_has_words_in_five_catalogs_and_a_signalled_wait_asks_the_board_again() {
    let window = window_source();
    let refusal = block_after(window, "function answerRefusalWords(");
    let catalogs = ui_file("shell-i18n.js");
    for (word, key) in [
        ("question-changed", "board.ask.changed"),
        ("answer-in-flight", "board.ask.inFlight"),
        ("menu-needs-a-row", "board.ask.needsRow"),
    ] {
        assert!(
            refusal.contains(word) && refusal.contains(&format!("t(\"{key}\", \"")),
            "`{word}` is not turned into `{key}` with its Korean default:\n{refusal}"
        );
        assert_eq!(
            catalogs.matches(&format!("\"{key}\":")).count(),
            4,
            "`{key}` is not in each of the en, ja, zh and es catalogs"
        );
    }
    // Both roads hand a refusal to the one place that words it, which the
    // turn list (IDE design pass 2) reads beside the question as well.
    for road in ["function submitAsk(", "function decideApproval("] {
        assert!(
            block_after(window, road).contains("refuseAsk(draft, error)"),
            "{road} answers a refusal around the one place that words it"
        );
    }
    assert!(
        block_after(window, "function refuseAsk(").contains("answerRefusalWords(error)"),
        "a refused answer is shown as the bare word"
    );
    // The board is asked again from the hook's handler, only for a wait the
    // hook described nothing of, and the asking is dropped when the pane moves
    // to anything else.
    let handler = block_after(window, "listen(\"hook:agent\"");
    assert!(
        handler.contains("if (asking && !shaped) watchSignalledWait(term);")
            && handler.contains("else clearSignalledWait(term);"),
        "a signalled wait is no longer asked about again, or is asked about forever:\n{handler}"
    );
    assert!(
        window.contains("const SIGNALLED_WAIT_REPAINT_MS = [400, 1500, 4000];"),
        "the retry schedule is no longer three asks"
    );
}

/// The refusal that comes after the hand-over typed its exit command opens
/// with words the backend names once (`EXIT_TYPED_PANE_STANDS`) and the
/// window knows them by (`WIRE_EXIT_TYPED`, t-84850 r2): the window gives the
/// words that waited back to its input box rather than type them after
/// `/exit`. The two must be the same words, or the window types them at a
/// pane that may be reading `/exit` late or be a shell now.
#[test]
fn the_window_knows_the_refusal_that_comes_after_the_exit_command_was_typed() {
    let literal_after = |source: &'static str, opening: &str| -> &'static str {
        let from = source
            .find(opening)
            .unwrap_or_else(|| panic!("`{opening}` is gone"));
        let rest = &source[from + opening.len()..];
        let open = rest.find('"').expect("no string follows") + 1;
        let close = rest[open..].find('"').expect("the string does not end") + open;
        &rest[open..close]
    };
    let backend = literal_after(
        shipped_backend(),
        "pub(crate) const EXIT_TYPED_PANE_STANDS: &str =",
    );
    let window = literal_after(window_source(), "const WIRE_EXIT_TYPED =");
    assert!(!backend.is_empty(), "the backend's opening words are empty");
    assert_eq!(
        window, backend,
        "the window does not know the refusal the hand-over gives after typing its exit command"
    );
    let reading = literal_after(
        shipped_backend(),
        "pub(crate) const PANE_NOT_READING_YET: &str =",
    );
    let knows = literal_after(window_source(), "const WIRE_NOT_READING =");
    assert!(
        !knows.is_empty() && reading.starts_with(knows),
        "the window does not know the refusal for a pane not reading yet: {knows:?} / {reading:?}"
    );
    let hand = block_after(window_source(), "async function handPaneToWire(");
    assert!(
        hand.contains("startsWith(WIRE_NOT_READING)"),
        "the hand-over does not put the not-reading refusal in the person's words:\n{hand}"
    );
    assert!(
        hand.contains("startsWith(WIRE_EXIT_TYPED)"),
        "the hand-over does not tell a refusal after the exit command from one before it:\n{hand}"
    );
}

/// The conversation queue's 끼워 넣기 and 바로 보내기 (t-86909) put words into a
/// running turn, stop it and put the stopped turn's words in, and they never
/// write to a pane themselves: the keys go through the answer door — the words
/// and their Enter, the stop, the send now's words after the stop — on the
/// pane's lease, and the stop is reported to the window as a stop gesture the
/// way a person's Escape is, so the queue learns the turn ended. All three ask
/// the ledger's seats before anything is typed or sent, and a door that
/// refuses has not scrolled the pane to its foot first (r2).
#[test]
fn the_queues_steer_and_stop_type_only_through_the_door() {
    let shipped = shipped_backend();
    for (function, doors) in [
        (
            "fn type_queue_line(",
            &[
                "answer_door::answer_lease(",
                "answer_door::type_queued_line(",
            ][..],
        ),
        (
            "fn interrupt_pane(",
            &[
                "answer_door::answer_lease(",
                "answer_door::type_at_a_turn(",
                "observe_stop_gesture(",
            ][..],
        ),
    ] {
        let body = block_after(shipped, function);
        assert!(
            !body.contains("write_input("),
            "{function} types into the pane itself instead of through the door:\n{body}"
        );
        for door in doors {
            assert!(
                body.contains(door),
                "{function} does not use `{door}`:\n{body}"
            );
        }
        // A door that refuses leaves the pane's view where it was: the scroll to the foot comes
        // after the typing, never before it.
        let typed = doors
            .iter()
            .filter(|door| door.contains("type_"))
            .filter_map(|door| body.find(door))
            .min()
            .unwrap_or_else(|| panic!("{function} types nothing"));
        if let Some(scroll) = body.find("view_to_bottom") {
            assert!(
                typed < scroll,
                "{function} scrolls the pane before the door has said yes:\n{body}"
            );
        }
    }
    // The two roads that type a queued line — the steer's words and the words of a send now —
    // spend every key through the one helper above, and none writes to the pane itself.
    for function in ["fn steer_by_typing(", "fn send_now_pane("] {
        let body = block_after(shipped, function);
        assert!(
            body.contains("type_queue_line(") && !body.contains("write_input("),
            "{function} does not type through `type_queue_line`:\n{body}"
        );
    }
    // All three ask the door about the CLI too (r2 P3-3): a pane restored a moment ago may stand
    // "working" by a hook word that outlived the restart while its CLI is still loading, and a key
    // typed then goes to whatever reads the terminal. The hand-over asks the same before it types.
    for function in [
        "fn steer_by_typing(",
        "fn interrupt_pane(",
        "fn send_now_pane(",
    ] {
        let body = block_after(shipped, function);
        assert!(
            body.contains("reads_input: crate::human_input::reads_input(term)"),
            "{function} does not hand the door whether the pane's CLI has shown it reads its input:\n{body}"
        );
    }
    for function in ["fn steer_pane(", "fn interrupt_pane(", "fn send_now_pane("] {
        let body = block_after(shipped, function);
        let seat = body
            .find("turn_pane(")
            .unwrap_or_else(|| panic!("{function} does not ask the ledger's seats"));
        for acts in [
            "steer_by_typing(",
            "steer_turn(",
            "type_at_a_turn(",
            "type_queue_line(",
        ] {
            if let Some(at) = body.find(acts) {
                assert!(
                    seat < at,
                    "{function} acts before it asks the seats:\n{body}"
                );
            }
        }
    }
    let pane = block_after(shipped, "fn turn_pane(");
    assert!(
        pane.contains("ledger_pane_refusal(term)") && pane.contains("turn_seat_word("),
        "a pane the ledger drives is no longer refused first:\n{pane}"
    );
    // A zo pane is steered over its channel and a wire over its own request:
    // neither types a key.
    for (haystack, function) in [(ZO_NOTICE, "fn steer_turn("), (shipped, "fn steer(")] {
        let body = block_after(haystack, function);
        assert!(
            !body.contains("write_input(") && !body.contains("term_key"),
            "{function} types at a screen:\n{body}"
        );
    }
}

/// The page's two buttons that reach a pane ask the doors and no key of their
/// own (t-86909): the queue's steer and send-now functions never paste, never
/// press, never go round the road a send takes — the pane road asks
/// `pane_steer` and `pane_interrupt`, the wire road `wire_steer` and
/// `wire_interrupt`, and nothing else is on that road.
#[test]
fn the_pages_queue_buttons_ask_only_the_doors_and_type_nothing() {
    let window = window_source();
    for function in [
        "async function steerQueuedMessage(",
        "async function sendQueuedMessageNow(",
    ] {
        let body = block_after(window, function);
        for forbidden in [
            "term_key",
            "term_paste",
            "deliverToPane(",
            "composerDeliver(",
            ".interrupt()",
        ] {
            assert!(
                !body.contains(forbidden),
                "{function} reaches a pane by `{forbidden}`:\n{body}"
            );
        }
    }
    let road = block_after(window, "function composerRoad(");
    for door in [
        "invoke(\"wire_steer\", { id: run.wire, text })",
        "invoke(\"wire_interrupt\", { id: run.wire })",
        "invoke(\"pane_steer\", { term: run.term, text })",
        "invoke(\"pane_interrupt\", { term: run.term })",
        "invoke(\"pane_send_now\", { term: run.term, text })",
    ] {
        assert!(road.contains(door), "the road lost `{door}`:\n{road}");
    }
    // All four commands are registered in the window's invoke handler.
    let main = include_str!("../../src/main.rs");
    for command in [
        "wire_steer,",
        "cmd::terminal::pane_steer,",
        "cmd::terminal::pane_interrupt,",
        "cmd::terminal::pane_send_now,",
    ] {
        assert!(main.contains(command), "`{command}` is not registered");
    }
}

/// A turn door refuses with a WORD and the page words it (t-86909): every word
/// the core's `turn_refusal` names has its sentence in the page's table, the Korean default in
/// the call and the other four languages in the catalog file — and the page's
/// table names no word the door does not, except the three the page says itself
/// (a stop nobody confirmed, a saved edit with nothing in it, a second row
/// opened over unsaved changes). One list, so a
/// refusal added on one side cannot reach the person as the bare word.
#[test]
fn every_word_a_turn_door_refuses_with_has_a_sentence_in_five_catalogs() {
    let window = window_source();
    let catalogs = ui_file("shell-i18n.js");
    let table = block_after(window, "const QUEUE_ACT_REFUSALS = ");
    let words: Vec<&str> = block_after(CORE_AGENT, "pub mod turn_refusal {")
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("pub const ")?;
            let (_, value) = rest.split_once("= \"")?;
            value.strip_suffix("\";")
        })
        .collect();
    assert!(
        words.len() >= 8,
        "the door names too few refusal words: {words:?}"
    );
    for word in &words {
        if *word == "turn-landed" {
            // The one refusal after the words went in: the page does not say it,
            // it takes the row out and asks the person to look at the pane.
            assert!(
                block_after(window, "async function steerQueuedMessage(")
                    .contains("\"turn-landed\""),
                "the page no longer knows the refusal after the words went in"
            );
            continue;
        }
        assert!(
            table.contains(&format!("\"{word}\":")),
            "`{word}` has no sentence in QUEUE_ACT_REFUSALS:\n{table}"
        );
    }
    let mut keys = 0;
    for call in table.split("t(\"composer.queue.refuse.").skip(1) {
        let key = call.split('"').next().unwrap();
        assert_eq!(
            catalogs
                .matches(&format!("\"composer.queue.refuse.{key}\":"))
                .count(),
            4,
            "`composer.queue.refuse.{key}` is not in each of the en, ja, zh and es catalogs"
        );
        keys += 1;
    }
    assert_eq!(
        keys,
        words.len() - 1 + 2,
        "a refusal word and its sentence are out of step"
    );
}

/// The queue's doors refuse in WORDS, never in a sentence of one language (t-86909 r2, P3-7): the
/// Korean sentences the first cut returned — "끼워 넣을 글이 비어 있습니다", "이 zo 판의 세션을 아직
/// 모릅니다", "터미널이 떠 있지 않습니다", zo's timeout — reached a person who reads another
/// language exactly as written. The page words each refusal in its five catalogs
/// (`every_word_a_turn_door_refuses_with_has_a_sentence_in_five_catalogs`). Comments may be Korean;
/// what a door returns may not.
#[test]
fn the_queue_doors_refuse_in_words_and_not_in_korean_sentences() {
    let shipped = shipped_backend();
    let hangul = |text: &str| {
        text.chars()
            .any(|ch| ('\u{AC00}'..='\u{D7A3}').contains(&ch))
    };
    for (haystack, function) in [
        (shipped, "fn turn_pane("),
        (shipped, "fn steer_pane("),
        (shipped, "fn steer_by_typing("),
        (shipped, "fn type_queue_line("),
        (shipped, "fn interrupt_pane("),
        (shipped, "fn send_now_pane("),
        (shipped, "fn steer("),
        (ZO_NOTICE, "fn steer_turn("),
        (ZO_NOTICE, "fn steer_turn_within("),
    ] {
        let body = strip_rust_comments(block_after(haystack, function));
        assert!(
            !hangul(&body),
            "{function} refuses in a Korean sentence, not in a word:\n{body}"
        );
    }
    let door = door_code();
    for function in [
        "fn before_the_screen(",
        "fn type_after_a_turn(",
        "fn type_queued_line(",
    ] {
        let body = block_after(&door, function);
        assert!(
            !hangul(body),
            "{function} refuses in a Korean sentence, not in a word:\n{body}"
        );
    }
}

/// Every call of the composer's file that puts anything at a pane or a wire, counted
/// (t-86909 r2). The queue's claim — nothing the buttons do types at a pane but through the
/// answer door — holds only while no call is added beside the doors: this list is the grep, and
/// a new `term_key` or `term_paste` in this file fails here until someone reads why.
#[test]
fn the_calls_the_composer_makes_at_a_pane_are_these_and_the_queue_adds_none() {
    let composer = ui_file("shell-composer.js");
    for (call, times, what) in [
        (
            "invoke(\"term_paste\"",
            1,
            "deliverToPane: the composer's own send and the queue's ordinary delivery of a message that waited",
        ),
        (
            "invoke(\"term_key\"",
            3,
            "deliverToPane's Enter, the pane road's stop (the stop button and Esc — the person's own), the permission chip's key",
        ),
        ("invoke(\"pane_steer\"", 1, "the queue's 끼워 넣기"),
        (
            "invoke(\"pane_interrupt\"",
            1,
            "the queue's stop before a 바로 보내기",
        ),
        (
            "invoke(\"pane_send_now\"",
            1,
            "the words of a 바로 보내기, once its stop has gone",
        ),
        (
            "invoke(\"wire_steer\"",
            1,
            "the queue's 끼워 넣기 on a wire",
        ),
        (
            "invoke(\"wire_interrupt\"",
            2,
            "the wire's stop (the stop button and Esc) and the queue's stop before a 바로 보내기",
        ),
        ("invoke(\"wire_send\"", 1, "a message to a wire"),
    ] {
        assert_eq!(
            composer.matches(call).count(),
            times,
            "`{call}` in shell-composer.js — {what}"
        );
    }
    // The queue's own section reaches a pane by none of the raw ones; its ordinary delivery and
    // the door for the words after a stop are the two ways a message leaves it.
    let from = composer
        .find("/* ---- 대기열의 행마다")
        .expect("the queue's section moved");
    let to = composer
        .find("/* ---- the agent·model chip")
        .expect("the chip section moved");
    let queue = &composer[from..to];
    for raw in ["term_key", "term_paste", "deliverToPane(", ".interrupt()"] {
        assert!(
            !queue.contains(raw),
            "the queue's section reaches a pane by `{raw}`"
        );
    }
    // A message that came to the front by 바로 보내기 leaves `settleComposerQueue` by its own function
    // — which asks the door, and nothing else — before the ordinary road is even considered.
    let settle = block_after(&composer, "function settleComposerQueue(");
    let front = settle
        .find("deliverQueueFront(")
        .expect("the words after a stop no longer go through their own delivery");
    let ordinary = settle
        .find("composerDeliver(")
        .expect("the ordinary delivery moved");
    assert!(
        front < ordinary,
        "a message that came to the front by 바로 보내기 is delivered by the door before the ordinary road:\n{settle}"
    );
    let door = block_after(&composer, "async function deliverQueueFront(");
    assert!(
        door.contains("composerRoad(run).sendFront(")
            && !door.contains("composerDeliver(")
            && !door.contains("deliverToPane("),
        "the front's delivery reaches the pane by something other than the door:\n{door}"
    );
    assert_eq!(queue.matches("composerDeliver(").count(), 1);
    assert_eq!(queue.matches("sendFront(").count(), 1);
}
