//! Slash commands typed while a turn runs (adversarial review A, Z12): the
//! turn holds the session, so `/context` waits for the turn's end, and a
//! prompt command — a prompt in a file — waits in the queue, expanded, like a
//! line typed now. A prompt command is looked for in zo's home too, so each test
//! that types one brings a home of its own (`IsolatedHome`).

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
    let _lock = crate::test_env_lock();
    let _home = crate::support::IsolatedHome::new();
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

/// The two prompt-command tests here read nothing from the person's home
/// (t-81313 P3-1). `turn_slash` reaches it two calls away — `slash::prompt_command`
/// asks the commands crate for `commands/` in zo's home, the trust store and the
/// settings — where the scan of `conversation_support` that wants every home
/// reader to hold the env lock does not look. A person who has a `/fruit` of
/// their own would see the first test fail on it, and, if that file grants Bash
/// to a `!` line, the test would run it.
///
/// So each test is run again in a process of its own, whose `HOME` is a folder
/// holding such a `/fruit`: a stand-in, never the real home. Both must pass,
/// and the `!` line must not run.
#[test]
fn the_prompt_command_tests_never_read_the_persons_home() {
    let person = tempfile::tempdir().expect("a stand-in for the person's home");
    let commands = person.path().join(".zo").join("commands");
    std::fs::create_dir_all(&commands).expect("the person's command folder");
    let marker = person.path().join("their-line-ran");
    // `!` lines run in the Bash tool's POSIX shell — Git's `sh` on Windows,
    // where a backslash is an escape — so the path is spelled with forward
    // slashes there.
    let path = marker.display().to_string();
    let path = if cfg!(windows) { path.replace('\\', "/") } else { path };
    std::fs::write(
        commands.join("fruit.md"),
        format!("---\nallowed-tools: Bash\n---\n!touch {path}\nThe person's own fruit, not $ARGUMENTS.\n"),
    )
    .expect("the person's own /fruit");

    let module = module_path!()
        .split_once("::")
        .map_or(module_path!(), |(_crate, path)| path);
    for name in [
        "a_prompt_command_mid_turn_waits_in_the_queue_expanded",
        "a_prompt_command_named_like_one_of_zos_never_runs_in_its_place",
    ] {
        let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
            .arg(format!("{module}::{name}"))
            .args(["--exact", "--nocapture", "--test-threads=1"])
            .env("HOME", person.path())
            .env_remove("ZO_CONFIG_HOME")
            .env_remove("ZO_HOME")
            .env("ZO_DISABLE_KEYCHAIN", "1")
            .output()
            .expect("run a child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{name} read the person's home:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!marker.exists(), "a test ran the `!` line of the person's own command");
}

/// zo's own commands come first: a command file that takes one of their names
/// never runs in its place (`find_prompt_command` matches without case).
#[test]
fn a_prompt_command_named_like_one_of_zos_never_runs_in_its_place() {
    let _lock = crate::test_env_lock();
    let _home = crate::support::IsolatedHome::new();
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
