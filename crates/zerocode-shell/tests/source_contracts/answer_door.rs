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
