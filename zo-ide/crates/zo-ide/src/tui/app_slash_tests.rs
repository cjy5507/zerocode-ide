//! Slash commands typed while a turn runs (adversarial review A, Z12): the
//! turn holds the session, so `/context` waits for the turn's end, and a
//! prompt command — a prompt in a file — waits in the queue, expanded, like a
//! line typed now.

use super::{test_ui, Deferred, FooterView};

#[test]
fn slash_context_mid_turn_runs_when_the_turn_ends() {
    let mut ui = test_ui();
    ui.turn_slash("context");
    assert!(
        matches!(ui.deferred.as_slice(), [Deferred::Slash(command)] if command == "context"),
        "/context was not kept for the turn's end"
    );
}

#[test]
fn a_prompt_command_mid_turn_waits_in_the_queue_expanded() {
    let folder = tempfile::tempdir().expect("a working folder");
    let commands = folder.path().join(".zo").join("commands");
    std::fs::create_dir_all(&commands).expect("the command folder");
    std::fs::write(commands.join("fruit.md"), "Name the fruit $ARGUMENTS in one word.\n")
        .expect("the prompt command");
    let mut ui = test_ui();
    ui.session_cwd = folder.path().to_path_buf();
    ui.turn_slash("fruit pineapple");
    let queued = ui
        .pending_input
        .pop_next_turn()
        .expect("the expanded command waits in the queue");
    assert_eq!(queued.text.trim(), "Name the fruit pineapple in one word.");
}

/// zo's own commands come first: a command file that takes one of their names
/// never runs in its place (`find_prompt_command` matches without case).
#[test]
fn a_prompt_command_named_like_one_of_zos_never_runs_in_its_place() {
    let folder = tempfile::tempdir().expect("a working folder");
    let commands = folder.path().join(".zo").join("commands");
    std::fs::create_dir_all(&commands).expect("the command folder");
    std::fs::write(commands.join("Warnings.md"), "Pretend to be the warnings viewer.\n")
        .expect("the prompt command");
    let mut ui = test_ui();
    ui.session_cwd = folder.path().to_path_buf();
    ui.turn_slash("warnings");
    assert!(matches!(ui.footer_view, Some(FooterView::Warnings(_))), "zo's /warnings did not open");
    assert!(ui.pending_input.pop_next_turn().is_none(), "the command file ran in zo's place");
}
