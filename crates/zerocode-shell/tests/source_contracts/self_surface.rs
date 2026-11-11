//! ZeroCode's own window as a Computer Use target (t-73906): what a test of
//! one function cannot see from inside it. The behaviour is the road's own
//! tests (`computer_use::self_surface`), core's rule (`self_surface::judge`)
//! and the window suite (`ui/tests/self-act.mjs`); these contracts hold that
//! every person-only command checks the second wall, that a command named for
//! a credential, an account or work that cannot be had back is decided for,
//! that the controls the window keeps for the person carry its mark, and
//! that the page presses its controls without an OS-like event, a focus move
//! or the clipboard.

use std::path::Path;

use super::quiet_children::code_only;
use super::support::block_after;

const ROAD: &str = include_str!("../../src/computer_use/self_surface.rs");

fn ui_file(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// The person-only commands, read off the road's own list.
fn person_only_commands() -> Vec<String> {
    let opens = "pub(crate) const PERSON_ONLY_COMMANDS: &[&str] = &[";
    let start = ROAD
        .find(opens)
        .expect("the road lists its person-only commands")
        + opens.len();
    let list = &ROAD[start..start + ROAD[start..].find("];").expect("the list closes")];
    let names: Vec<String> = list
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    assert!(names.len() > 10, "the list reads: {names:?}");
    names
}

/// A list of the road's, read off its source: the quoted names up to `];`.
fn road_list(opens: &str) -> Vec<String> {
    let start = ROAD
        .find(opens)
        .unwrap_or_else(|| panic!("the road has {opens}"))
        + opens.len();
    let list = &ROAD[start..start + ROAD[start..].find("];").expect("the list closes")];
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The commands the window registers, by the name the page invokes them by.
fn registered_commands() -> Vec<String> {
    let main = code_only(include_str!("../../src/main.rs"));
    let opens = "tauri::generate_handler![";
    let start = main.find(opens).expect("the window registers its commands") + opens.len();
    let list = &main[start..start + main[start..].find(']').expect("the list closes")];
    list.split(',')
        .map(|entry| {
            entry
                .trim()
                .rsplit("::")
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .filter(|name| !name.is_empty())
        .collect()
}

/// Every command passes the second wall in one place: the window's invoke
/// handler is wrapped, so a command on the person-only list refuses while an
/// agent presses in the window whichever crate defines it.
#[test]
fn the_invoke_handler_asks_the_second_wall_for_every_command() {
    let main = code_only(include_str!("../../src/main.rs"));
    assert!(
        main.contains(".invoke_handler(person_only_gate(tauri::generate_handler!["),
        "the window's commands are registered through person_only_gate"
    );
    let registered = registered_commands();
    for name in person_only_commands() {
        assert!(
            registered.contains(&name),
            "{name} is on the person-only list but not registered"
        );
    }
}

/// The rule, not a hand list: a registered command whose name touches a key,
/// a secret, a token, a password, a credential, an account or a login is on
/// the person-only list, or on the list of such names an agent may run (a
/// read, not a write), each with its reason in the road's source. A new
/// command of that kind fails here until someone decides which it is.
#[test]
fn a_command_named_for_a_credential_or_an_account_is_decided_for() {
    const WORDS: [&str; 7] = [
        "key",
        "secret",
        "token",
        "password",
        "credential",
        "account",
        "login",
    ];
    let person = person_only_commands();
    let allowed = road_list("pub(crate) const AGENT_ALLOWED_SENSITIVE: &[&str] = &[");
    let registered = registered_commands();
    let undecided: Vec<&String> = registered
        .iter()
        .filter(|name| WORDS.iter().any(|word| name.contains(word)))
        .filter(|name| !person.contains(name) && !allowed.contains(name))
        .collect();
    assert!(
        undecided.is_empty(),
        "undecided credential or account commands: {undecided:?}"
    );
    for name in &allowed {
        assert!(
            registered.contains(name),
            "{name} is allowed but not registered"
        );
        assert!(!person.contains(name), "{name} is on both lists");
    }
}

/// The same rule for work that cannot be had back (t-74975): a registered
/// command whose name discards, deletes, removes, pushes, resets, closes,
/// clears or aborts is on the person-only list, or on the list of such names
/// an agent may run — each with its reason in the road's source. An agent's
/// press on the source control panel's "discard changes" runs none of these
/// while the marks miss it; a new command of that kind fails here until
/// someone decides which it is.
#[test]
fn a_command_that_cannot_be_taken_back_is_decided_for() {
    const WORDS: [&str; 8] = [
        "discard", "delete", "remove", "push", "reset", "close", "clear", "abort",
    ];
    // Ending a session, a process or a program (t-75153, R7b): `end` as a
    // word of the name — `end_…` or `…_end_…`, never inside `append`,
    // `send`, `pending` or `render` — and `kill` and `terminate` as words,
    // never inside `skill`.
    let says = |name: &str, word: &str| {
        name == word
            || name.starts_with(&format!("{word}_"))
            || name.ends_with(&format!("_{word}"))
            || name.contains(&format!("_{word}_"))
    };
    let cannot_be_had_back = |name: &str| {
        WORDS.iter().any(|word| name.contains(word))
            || ["end", "kill", "terminate"]
                .iter()
                .any(|word| says(name, word))
    };
    // A switch that turns a question off is the person's to turn (t-74975's
    // set_skip_… and set_confirm_close_pinned, t-75153's set_computer_confirm).
    let turns_a_question_off = |name: &str| {
        name.starts_with("set_") && (name.contains("skip_") || name.contains("_confirm"))
    };
    let person = person_only_commands();
    // The ones the review found outside the wall (R1, t-73906): the person's
    // uncommitted work, their checkouts and projects, their browser's
    // sign-ins, Computer Use's own permissions, a push — and (R7b, t-75153)
    // the sessions the session manager ends and Computer Use's own
    // confirmations.
    for name in [
        "discard_paths",
        "delete_untracked",
        "scm_push",
        "remove_worktree",
        "remove_project",
        "close_lane",
        "delete_browser_profile",
        "reset_computer_use_permissions",
        "end_terminal_session",
        "end_all_terminal_sessions",
        "set_computer_confirm",
    ] {
        assert!(person.contains(&name.to_string()), "{name} is the person's");
    }
    let allowed = road_list("pub(crate) const AGENT_ALLOWED_DESTRUCTIVE_NAMED: &[&str] = &[");
    let registered = registered_commands();
    let undecided: Vec<&String> = registered
        .iter()
        .filter(|name| cannot_be_had_back(name) || turns_a_question_off(name))
        .filter(|name| !person.contains(name) && !allowed.contains(name))
        .collect();
    assert!(
        undecided.is_empty(),
        "undecided commands that cannot be taken back: {undecided:?}"
    );
    for name in &allowed {
        assert!(
            registered.contains(name),
            "{name} is allowed but not registered"
        );
        assert!(
            cannot_be_had_back(name) || turns_a_question_off(name),
            "{name} is allowed but its name says none of {WORDS:?}, end, kill or terminate"
        );
        assert!(!person.contains(name), "{name} is on both lists");
    }
}

/// t-75676: the input event `document.execCommand` sets off is trusted
/// whoever asked, and a trusted InputEvent reads as the person's typing — so
/// the page's own text insertion stands only inside `asHand`, answering it
/// with the hand of the flow that inserts (a paste carries the hand of its
/// own event). Before, a paste destination called from no person's event
/// wrote the cookie box as the person's.
#[test]
fn the_pages_own_text_insertion_carries_its_flows_hand() {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    let mut inserting = 0;
    for entry in std::fs::read_dir(&ui).expect("ui reads") {
        let path = entry.expect("ui entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("js") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("ui file reads");
        for line in source
            .lines()
            .filter(|line| line.contains("document.execCommand("))
        {
            inserting += 1;
            assert!(
                line.contains("asHand("),
                "{}: execCommand outside asHand: {line}",
                path.display()
            );
        }
    }
    assert!(inserting > 0, "the paste's insertion left the page");
    let boot = ui_file("shell-boot.js");
    assert!(
        block_after(&boot, "function handNow() {").contains("carriedHand"),
        "the hand a step carries is not the hand of the events it sets off"
    );
}

/// The page presses with the document's own calls: no synthetic key or
/// pointer event, no focus move, no clipboard.
#[test]
fn the_page_presses_without_keys_focus_or_the_clipboard() {
    let page = ui_file("shell-self-act.js");
    for forbidden in [
        ".focus(",
        "KeyboardEvent",
        "PointerEvent",
        "MouseEvent",
        "clipboard",
        "execCommand",
    ] {
        assert!(
            !page.contains(forbidden),
            "shell-self-act.js uses {forbidden}"
        );
    }
    assert!(page.contains("listen(\"computer:self-act\""));
    assert!(page.contains("invoke(\"computer_self_answer\""));
    let index = ui_file("index.html");
    assert!(
        index.contains("<script defer src=\"./shell-self-act.js\"></script>"),
        "index.html loads the page's half"
    );
}

/// The controls the window keeps for the person carry the mark the page
/// reads (`data-person-only`): the coordinator's seat card, the stop's
/// resume, a board card's answers, the inbox's answer, a terminal tab's close.
#[test]
fn the_persons_controls_carry_the_person_only_mark() {
    let index = ui_file("index.html");
    for id in [
        "coordinator-scrim",
        "computer-band-resume",
        "computer-band-end",
        "update-notice",
        "update-card",
    ] {
        let at = index
            .find(&format!("id=\"{id}\""))
            .unwrap_or_else(|| panic!("index.html has #{id}"));
        let tag_start = index[..at].rfind('<').unwrap_or(at);
        let tag_end = at + index[at..].find('>').unwrap_or(0);
        assert!(
            index[tag_start..tag_end].contains("data-person-only"),
            "#{id} carries data-person-only"
        );
    }
    let desk = index
        .find("class=\"task-board-desk\"")
        .expect("index.html has the coordinator desk");
    let desk_tag = &index[desk..desk + index[desk..].find('>').unwrap_or(0)];
    assert!(
        desk_tag.contains("data-person-only"),
        "the coordinator desk carries data-person-only"
    );
    // The inbox answers through the desk's own roads (t-67483), so its answer
    // and its buttons carry the desk's mark.
    let inbox = ui_file("shell-inbox.js");
    let inbox = block_after(&inbox, "function buildInboxView(");
    for part in [
        "reply.dataset.personOnly = \"\"",
        "actions.dataset.personOnly = \"\"",
        // A question closed without an answer (t-83281).
        "close.dataset.personOnly = \"\"",
        // A row opens its letter and a read leaves the ledger's ack (t-74825).
        "list.dataset.personOnly = \"\"",
    ] {
        assert!(inbox.contains(part), "the inbox's answer lost `{part}`");
    }
    let shell = ui_file("shell.js");
    for panel in ["function askPanelNode(", "function approvalPanelNode("] {
        assert!(
            block_after(&shell, panel).contains("panel.dataset.personOnly = \"\""),
            "{panel} marks its panel person-only"
        );
    }
    let status = ui_file("shell-status.js");
    assert!(
        block_after(&status, "function buildTabNode(").contains("dataset.personOnly"),
        "a terminal tab's close is marked person-only"
    );
}

/// The share holds every provider step: the one road to the helper and to
/// the page asks it before anything else, and a command is admitted to it
/// before it acts. Taking control and handing back are the person's.
#[test]
fn every_step_asks_the_share_and_the_band_is_the_persons() {
    let call = code_only(include_str!("../../src/computer_use/mod.rs"));
    let body = block_after(&call, "pub fn call(method: &str, params: Value)");
    let share = body
        .find("share::ensure_current()?")
        .expect("call asks the share");
    let route = body
        .find("self_surface::route(")
        .expect("call routes the page");
    assert!(
        share < route,
        "the share is asked before the page or the helper"
    );
    let runtime = code_only(include_str!("../../src/agent_tools_runtime.rs"));
    assert!(
        block_after(&runtime, "fn answer_computer_command_with(")
            .contains("computer_use::share::admit("),
        "a command is admitted to the share before it acts"
    );
    let person = person_only_commands();
    for name in [
        "computer_take_control",
        "computer_give_back",
        "computer_share_end",
        "computer_share_hand_over",
    ] {
        assert!(person.contains(&name.to_string()), "{name} is the person's");
    }
}

/// The ledger's ack of a letter is the person's: the desk and the inbox both
/// reach it, and the marks on their controls are the first wall (t-74825).
#[test]
fn the_ledger_ack_of_a_letter_is_a_person_only_command() {
    assert!(person_only_commands().contains(&"desk_ack".to_string()));
}

/// Taking a recipe or Flow away from the card, and standing a plain recipe up
/// as a Flow, are the person's alone (t-93871): both stand on the second wall's
/// list and are registered; their buttons wear the person-only mark (the first
/// wall); the delete asks one danger question before the command is invoked,
/// and both calls carry the hand their flow has. The agent's own road to a Flow
/// is `recipe-save --flow-check`, which is not a window command at all.
#[test]
fn deleting_and_registering_a_flow_from_the_card_are_the_persons_alone() {
    let person = person_only_commands();
    let registered = registered_commands();
    for name in ["flow_delete", "flow_register"] {
        assert!(person.contains(&name.to_string()), "{name} is the person's");
        assert!(
            registered.contains(&name.to_string()),
            "{name} is on the person-only list but not registered"
        );
    }
    let index = ui_file("index.html");
    let at = index
        .find("id=\"flow-delete\"")
        .expect("the Flow card has a delete button");
    let tag_start = index[..at].rfind('<').unwrap_or(at);
    let tag_end = at + index[at..].find('>').unwrap_or(0);
    assert!(
        index[tag_start..tag_end].contains("data-person-only"),
        "the Flow's delete button carries data-person-only"
    );
    let settings = ui_file("shell-settings.js");
    let button = block_after(&settings, "function flowRecipeButton(");
    assert!(
        button.contains("dataset.personOnly"),
        "a recipe row's doors carry the person-only mark"
    );
    let question = block_after(&settings, "async function requestFlowDeletion(");
    let asked = question.find("askConfirm(").expect("the delete asks first");
    let invoked = question
        .find("invoke(\"flow_delete\"")
        .expect("the delete invokes flow_delete");
    assert!(asked < invoked, "the question comes before the command");
    assert!(
        question.contains("danger: true") && question.contains("flowHand(started)"),
        "a destructive question, and the hand carried past its answer"
    );
    assert!(
        block_after(&settings, "async function registerFlow(").contains("{ hand }"),
        "the registration says the hand of the press that made it"
    );
    // The recipe store checks the slug itself: the page sends a slug, never a path.
    let store = code_only(include_str!("../../src/computer_use/recipes/register.rs"));
    let delete = block_after(&store, "pub fn flow_delete(");
    assert!(
        delete.contains("recipe_file(root, slug)?"),
        "the delete names its file through the slug check, never from the caller's words"
    );
    assert!(
        !store.contains("remove_dir"),
        "deleting a recipe removes one document, never a folder"
    );
}

/// A job that runs with nobody watching is the person's to save, to switch on
/// or off and to run now (2026-11-11): the three commands stand on the second
/// wall's list and are registered, and the controls that reach them wear the
/// person-only mark (the first wall) — the form's save and run, the detail's
/// run and switch, a row's switch and run, a Flow's schedule switch and the
/// vault's own switch-on. An agent may look at the page and fill the form; it
/// is told the window keeps these for the person, by the words every marked
/// control answers with. The call that awaits before it invokes takes its
/// press's hand first; that every call names a hand at all is the rule of
/// `the_second_wall_reads_the_hand_the_page_says_and_no_clock`.
#[test]
fn saving_switching_and_running_an_automation_are_the_persons_alone() {
    let person = person_only_commands();
    let registered = registered_commands();
    for name in [
        "save_automation",
        "enable_automation",
        "run_automation",
        // The vault's weekly review is written among those jobs, and a Flow's
        // policy decides whether its money step is pressed.
        "set_second_brain_weekly_review",
        "flow_set",
    ] {
        assert!(person.contains(&name.to_string()), "{name} is the person's");
        assert!(
            registered.contains(&name.to_string()),
            "{name} is on the person-only list but not registered"
        );
    }
    let index = ui_file("index.html");
    for id in [
        "auto-save",
        "auto-run",
        "auto-detail-run",
        "auto-detail-toggle",
        "second-brain-ingest-on",
        "second-brain-weekly-review",
    ] {
        let at = index
            .find(&format!("id=\"{id}\""))
            .unwrap_or_else(|| panic!("#{id} is on the page"));
        let tag_start = index[..at].rfind('<').unwrap_or(at);
        let tag_end = at + index[at..].find('>').unwrap_or(0);
        assert!(
            index[tag_start..tag_end].contains("data-person-only"),
            "#{id} carries data-person-only"
        );
    }
    let settings = ui_file("shell-settings.js");
    let rows = block_after(&settings, "function paintAutomations() {");
    assert!(
        rows.contains("role=\"switch\" data-person-only></button>")
            && rows.contains("run.dataset.personOnly = \"\""),
        "a row's switch and its run button carry the person-only mark"
    );
    assert!(
        block_after(&settings, "function flowScheduleRow(job) {")
            .contains("toggle.dataset.personOnly = \"\""),
        "a Flow's schedule switch carries the person-only mark"
    );
    assert!(
        block_after(&settings, "function buildFlowSegment(")
            .contains("button.dataset.personOnly = \"\""),
        "a Flow's policy and evidence buttons carry the person-only mark"
    );
    let ingest = block_after(&settings, "async function turnOnSecondBrainIngest() {");
    let taken = ingest
        .find("const hand = handNow();")
        .expect("the vault's switch-on takes its press's hand");
    let awaited = ingest.find("await ").expect("it awaits");
    assert!(
        taken < awaited && ingest.matches("{ hand });").count() == 2,
        "the hand is taken before anything is awaited and both commands carry it"
    );
    // The press that saves has what it saves in view: a command of the job's
    // own and the check before it sit in a fold, which opens as either gains
    // words and whose summary an agent cannot close.
    let fold = index
        .find("id=\"auto-details\"")
        .expect("the form folds its details");
    let summary = &index[fold..];
    let summary = &summary[summary.find("<summary").expect("the fold has a summary")..];
    assert!(
        summary[..summary.find('>').unwrap_or(0)].contains("data-person-only"),
        "the fold's summary carries data-person-only"
    );
    let shown = block_after(&settings, "function showAutoOwnCommands() {");
    assert!(
        shown.contains("el(\"auto-command\").value.trim() || el(\"auto-precheck\").value.trim()")
            && shown.contains("el(\"auto-details\").open = true"),
        "the fold opens while the command or the precheck has words"
    );
    let wired = "for (const id of [\"auto-precheck\", \"auto-command\"]) {\n  el(id).addEventListener(\"input\", showAutoOwnCommands);\n  el(id).addEventListener(\"change\", showAutoOwnCommands);\n}";
    assert!(
        settings.contains(wired),
        "words put into either field open the fold, whoever wrote them"
    );
}

/// A hand is read at the person's press, never in a form's `submit`
/// (2026-11-11). The browser submits as the press's default action, after the
/// press has been dispatched, and `handNow` answers `none` there — so a
/// person-only command called from `submit` is refused for the person's own
/// press. It was: a GitLab token's sign-in and a question closed without an
/// answer, both since they were written, and the automation's save the day it
/// became person-only.
///
/// Every handler the browser's own submission reaches is read — a
/// `addEventListener("submit", …)` and a `.onsubmit = …` alike — with the
/// functions it calls by name, one step down: none reads a hand, none names a
/// person-only command. A form whose submission does run one reads the hand
/// at its button's click and at Enter in a one-line field (`submitByPress`),
/// or submits inside its press, as the worker composer's button and key do.
///
/// A handler that is handed its work as a closure cannot be followed by
/// words: the question-closing form was that shape, and passed. So the
/// handlers are also counted, file by file. A new one is read by whoever adds
/// it before the count is moved: where does what it calls read its hand?
#[test]
fn a_forms_submit_never_reads_the_hand_of_a_person_only_command() {
    let person_only = person_only_commands();
    let scripts = ui_scripts();
    let mut counted = std::collections::BTreeMap::new();
    for (name, text) in &scripts {
        let listened = text
            .match_indices("addEventListener(\"submit\"")
            .map(|(at, _)| enclosing_call(text, at + "addEventListener(".len()));
        let assigned = text
            .match_indices(".onsubmit = ")
            .map(|(at, opens)| statement_at(text, at + opens.len()));
        for handler in listened.chain(assigned) {
            *counted.entry(name.as_str()).or_insert(0usize) += 1;
            let called = called_names(handler)
                .into_iter()
                .filter_map(|function| function_block(&scripts, function));
            for read in std::iter::once(handler).chain(called) {
                assert!(
                    !read.contains("handNow("),
                    "{name}: a form's submit reads a hand — in itself or one call down: {read}"
                );
                for command in &person_only {
                    assert!(
                        !read.contains(&format!("\"{command}\"")),
                        "{name}: a form's submit runs {command} — in itself or one call down: {read}"
                    );
                }
            }
        }
    }
    // Read 2026-11-11, each to the end of what it calls. One reaches a
    // person-only command further down than one call: the worker composer
    // (`shell.js`), whose button and key both submit inside the press, so its
    // hand is that press's. The new-chat form (`shell-workspace.js`) calls
    // `closeTab`, which ends a lane or a shell only for a lane's or a
    // terminal's tab: the tab it closes is neither, so nothing there is the
    // person's alone.
    let expected: std::collections::BTreeMap<&str, usize> = [
        ("shell-board-now.js", 1),
        ("shell-board.js", 1),
        ("shell-boot.js", 1),
        ("shell-composer.js", 1),
        ("shell-doc.js", 2),
        ("shell-flow.js", 1),
        ("shell-jev.js", 3),
        ("shell-knowledge.js", 1),
        ("shell-remote.js", 2),
        ("shell-settings.js", 1),
        ("shell-sftp.js", 2),
        ("shell-workspace.js", 1),
        ("shell.js", 1),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        counted, expected,
        "a form's submit handler was added or taken away: read where what it calls takes its hand, then move the count"
    );

    let boot = ui_file("shell-boot.js");
    let by_press = block_after(&boot, "function submitByPress(form, button, send) {");
    assert!(
        by_press.contains("form.addEventListener(\"submit\", (event) => event.preventDefault());")
            && by_press.contains("button.addEventListener(\"click\", press);")
            && by_press.contains("if (!button.disabled && form.reportValidity()) send(handNow());")
            && by_press.contains("event.isComposing")
            && by_press.contains("event.keyCode === 229"),
        "the press reads the hand, and submit only keeps the page where it is"
    );
    let settings = ui_file("shell-settings.js");
    let inbox = ui_file("shell-inbox.js");
    for (script, wired) in [
        (
            &settings,
            "submitByPress(el(\"auto-form\"), el(\"auto-save\"), (hand) => void saveAutomation(hand));",
        ),
        (
            &settings,
            "submitByPress(paste, send, (hand) => void connectGitlabHost(host, field, said, send, hand));",
        ),
        (&inbox, "submitByPress(form, go, (hand) => send(hand));"),
    ] {
        assert!(script.contains(wired), "a form sends by its press: {wired}");
    }
    // The question's close takes the hand it is handed, or reads it where the
    // caller is the press itself — before anything is awaited.
    let closing = block_after(&inbox, "async function closeQuestionWithoutAnswer(");
    let taken = closing
        .find("hand = handNow() },")
        .expect("closing a question takes its press's hand");
    let awaited = closing.find("await ").expect("it awaits");
    assert!(
        taken < awaited && closing.contains("retryRequest: closing.retry.id }, { hand });"),
        "the close is sent with the hand of its press"
    );
    let shell = ui_file("shell.js");
    let send_button = block_after(
        &shell,
        "send.addEventListener(\"click\", async (event) => {",
    );
    assert!(
        send_button.contains("event.preventDefault();\n    form.requestSubmit();"),
        "the worker composer's button submits inside its press"
    );
}

/// Every script the page loads from `ui/`, by file name, in name order.
fn ui_scripts() -> Vec<(String, String)> {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    let mut scripts: Vec<(String, String)> = std::fs::read_dir(&ui)
        .expect("ui reads")
        .filter_map(|entry| {
            let path = entry.expect("ui entry").path();
            let name = path.file_name()?.to_string_lossy().to_string();
            name.ends_with(".js")
                .then(|| (name, std::fs::read_to_string(&path).unwrap_or_default()))
        })
        .collect();
    scripts.sort();
    scripts
}

/// From `at` to the `;` that ends the statement, brackets balanced.
fn statement_at(text: &str, at: usize) -> &str {
    let mut depth = 0usize;
    for (offset, letter) in text[at..].char_indices() {
        match letter {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => return &text[at..at + offset],
            _ => {}
        }
    }
    &text[at..]
}

/// The names a piece of script calls: every word that stands before a `(`.
fn called_names(script: &str) -> std::collections::BTreeSet<&str> {
    script
        .match_indices('(')
        .filter_map(|(at, _)| {
            let start = script[..at]
                .char_indices()
                .rev()
                .find(|(_, letter)| {
                    !(letter.is_ascii_alphanumeric() || matches!(letter, '_' | '$'))
                })
                .map_or(0, |(before, letter)| before + letter.len_utf8());
            (start < at).then(|| &script[start..at])
        })
        .collect()
}

/// A function the page declares by that name, as its whole block.
fn function_block<'a>(scripts: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let opens = format!("function {name}(");
    scripts
        .iter()
        .find(|(_, text)| text.contains(&opens))
        .map(|(_, text)| block_after(text, &opens))
}

#[test]
fn requiring_a_share_is_a_persisted_person_only_setting() {
    let person = person_only_commands();
    assert!(person.contains(&"set_computer_share_required".to_string()));
    let settings = code_only(include_str!("../../src/cmd/settings.rs"));
    let body = block_after(&settings, "pub(crate) fn set_computer_share_required(");
    assert!(body.contains("commit_setting("));
    assert!(body.contains("setting_key::COMPUTER_SHARE_REQUIRED"));
    assert!(body.contains("settings.computer_share_required = on"));
}

#[test]
fn required_sharing_reads_the_saved_policy_and_rechecks_each_provider_step() {
    let runtime = code_only(include_str!("../../src/agent_tools_runtime.rs"));
    let install = block_after(&runtime, "fn install_share(app: AppHandle)");
    assert!(install.contains("share::install_required("));
    // R1 (t-74826): read at every provider step, so from memory — never the
    // disk and never the repository's lock a commit holds through its fsync
    // (t-67102). The person's toggle commits, and a commit is remembered.
    assert!(install.contains("remembered_settings("));
    assert!(
        !install.contains("load_settings("),
        "a step must not wait on the settings lock"
    );
    assert!(install.contains("snapshot.document.computer_share_required"));
    let road = code_only(include_str!("../../src/computer_use/share.rs"));
    let current = block_after(&road, "pub(crate) fn ensure_current()");
    assert!(current.contains("refresh_required()"));
    // A step in flight is judged without moving the share (t-74826), on the
    // reach it was admitted with and the window's clock (t-75217).
    assert!(current.contains("held.share.still_holds("));
    assert!(current.contains("whole_screen: admitted.whole_screen"));
    assert!(current.contains("now_ms()"));
    assert!(!current.contains(".gate("));
}

/// The second wall reads whose hand made a call, never a clock (t-74927): the
/// gate reads the header the page's one `invoke` writes, under the same name
/// on both sides; the page presses as the agent's hand; the hand is the
/// flow's, never the page's last input, and every person-only call in the
/// page carries the hand its flow has (t-75209); and the road that names the
/// program's hand is the update policy's alone.
#[test]
fn the_second_wall_reads_the_hand_the_page_says_and_no_clock() {
    let road = code_only(ROAD);
    let gate = block_after(&road, "pub(crate) fn person_only_gate<");
    assert!(
        gate.contains("hand_said(invoke.message.headers())"),
        "the gate reads the call's hand off its headers"
    );
    for clock in ["Instant", "hold_until", "HELD_UNTIL", "GRACE"] {
        assert!(!road.contains(clock), "the road keeps a clock: {clock}");
    }
    let header = ROAD
        .split("pub(crate) const HAND_HEADER: &str = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the road names its hand header");
    let boot = ui_file("shell-boot.js");
    assert!(
        boot.contains(&format!("const HAND_HEADER = \"{header}\";")),
        "shell-boot.js writes the header the gate reads ({header})"
    );
    let invoke = block_after(&boot, "const invoke = (command, args, options) => {");
    assert!(
        invoke.contains("hand = handNow()")
            && invoke.contains("[HAND_HEADER]: hand")
            && invoke.contains("invokeBackend("),
        "the page's one invoke says the hand on every call, the event's when the caller carries none"
    );
    assert!(
        boot.contains(
            "const { invoke: invokeBackend, Channel: TauriChannel } = window.__TAURI__.core;"
        ),
        "Tauri's own invoke is reached only through the page's"
    );
    assert!(
        !boot.contains("pageHand"),
        "the page keeps one hand for every flow"
    );
    let now = block_after(&boot, "function handNow() {");
    assert!(
        now.contains("agentPressing") && now.contains("window.event"),
        "the hand of a call is the event the page is answering"
    );
    assert!(
        block_after(&boot, "function flowHand(started) {").contains("started === \"agent\""),
        "a flow an agent's press started stays the agent's"
    );
    let page = ui_file("shell-self-act.js");
    assert!(
        block_after(&page, "function selfActDo(request) {")
            .contains("asAgentPress(() => selfActPress(node, request))"),
        "a press runs as the agent's hand"
    );
    assert!(
        !block_after(&page, "function selfActDo(request) {").contains("node.click()"),
        "selfActDo presses only inside asAgentPress"
    );
    let person_only = person_only_commands();
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    for entry in std::fs::read_dir(&ui).expect("ui reads") {
        let path = entry.expect("ui entry").path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !name.ends_with(".js") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if name != "shell-boot.js" {
            assert!(
                !text.contains("__TAURI__.core") && !text.contains("__TAURI_INTERNALS__.invoke"),
                "{name} reaches Tauri's invoke without saying the hand"
            );
        }
        for word in ["hand: \"person\"", "hand: \"agent\"", "hand: \"program\""] {
            assert!(!text.contains(word), "{name} names a hand itself: {word}");
        }
        assert!(
            name == "shell-update.js" || !text.contains(": \"program\")"),
            "{name} takes the program road"
        );
        for command in &person_only {
            let quoted = format!("\"{command}\"");
            for (at, _) in text.match_indices(&quoted) {
                let call = enclosing_call(&text, at);
                assert!(
                    call.contains("hand"),
                    "{name}: {command} is called without the hand its flow carries: {call}"
                );
            }
        }
    }
    let update = ui_file("shell-update.js");
    assert!(
        update.contains(
            "runUpdateDownload(knock === UPDATE_KNOCK.button ? flowHand(started) : \"program\")"
        ),
        "only the update policy's quiet download takes the program road"
    );
}

/// The call around `at`: from the last `(` before it to the `)` that closes
/// that one.
fn enclosing_call(text: &str, at: usize) -> &str {
    let open = text[..at]
        .rfind('(')
        .expect("a command name sits inside a call");
    let mut depth = 0usize;
    for (offset, letter) in text[open..].char_indices() {
        match letter {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    &text[open..]
}
