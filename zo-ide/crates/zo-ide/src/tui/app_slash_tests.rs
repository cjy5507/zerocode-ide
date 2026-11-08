//! Slash commands typed while a turn runs (adversarial review A, Z12): the
//! turn holds the session, so `/context` waits for the turn's end, and a
//! prompt command — a prompt in a file — waits in the queue, expanded, like a
//! line typed now. A prompt command is looked for in zo's home too, so each test
//! that types one brings a home of its own (`IsolatedHome`).

use super::{test_ui, Deferred, FooterView};

/// Writes `.zo/skills/<name>/SKILL.md` under `folder`.
fn write_skill(folder: &std::path::Path, name: &str, description: &str) {
    let skill = folder.join(".zo").join("skills").join(name);
    std::fs::create_dir_all(&skill).expect("the skill folder");
    std::fs::write(skill.join("SKILL.md"), format!("---\ndescription: {description}\n---\nReview it twice.\n"))
        .expect("the skill");
}

/// `/` lists the folder's skills after zo's own commands, tagged the way the
/// `@` popup tags a skill, and typing narrows them. They are read once, when
/// the composer starts a command — a frame never reads the disk for them.
#[test]
fn the_slash_popup_lists_the_folders_skills_after_zos_own() {
    let _lock = crate::test_env_lock();
    let _home = crate::support::IsolatedHome::new();
    let folder = tempfile::tempdir().expect("a working folder");
    write_skill(folder.path(), "dual-brain", "two reviewers");
    let mut ui = test_ui();
    ui.session_cwd = folder.path().to_path_buf();
    ui.composer.insert_str("/");
    let before = ui.popup().expect("zo's own commands");
    assert!(before.rows.iter().all(|row| row.tag.is_none()), "a frame read the skills from disk");

    ui.sync_popups();
    let popup = ui.popup().expect("the popup");
    assert_eq!(popup.rows.first().map(|row| row.name.as_str()), Some("/model"), "zo's own come first");
    let last = popup.rows.last().expect("a row");
    assert_eq!((last.name.as_str(), last.description.as_str(), last.tag), ("/dual-brain", "two reviewers", Some("Skill")));

    ui.composer.clear();
    ui.composer.insert_str("/du");
    let names: Vec<String> = ui.popup().expect("the popup").rows.into_iter().map(|row| row.name).collect();
    assert_eq!(names, ["/dual-brain"]);
}

/// A name typed out in full heads the popup, so Enter runs that name and not
/// a longer one the list holds before it — here the folder's `design-system`,
/// which precedes the person's own `design`.
#[test]
fn a_name_typed_in_full_heads_the_popup() {
    let _lock = crate::test_env_lock();
    let home = crate::support::IsolatedHome::new();
    let folder = tempfile::tempdir().expect("a working folder");
    write_skill(folder.path(), "design-system", "the folder's");
    let personal = home.zo_home.join("skills").join("design");
    std::fs::create_dir_all(&personal).expect("the person's skill folder");
    std::fs::write(personal.join("SKILL.md"), "---\ndescription: the person's\n---\nDesign.\n").expect("the skill");
    let mut ui = test_ui();
    ui.session_cwd = folder.path().to_path_buf();
    ui.composer.insert_str("/design");
    ui.sync_popups();
    let names: Vec<String> = ui.popup().expect("the popup").rows.into_iter().map(|row| row.name).collect();
    assert_eq!(names, ["/design", "/design-system"]);
}

/// A skill typed while a turn runs waits in the queue as the turn it is.
#[test]
fn a_skill_mid_turn_waits_in_the_queue_expanded() {
    let _lock = crate::test_env_lock();
    let _home = crate::support::IsolatedHome::new();
    let folder = tempfile::tempdir().expect("a working folder");
    write_skill(folder.path(), "dual-brain", "two reviewers");
    let mut ui = test_ui();
    ui.session_cwd = folder.path().to_path_buf();
    ui.turn_slash("dual-brain the parser");
    let queued = ui.pending_input.pop_next_turn().expect("the skill waits in the queue");
    assert!(queued.text.starts_with("/dual-brain the parser\n\nBase directory for this skill: "), "{:?}", queued.text);
    assert!(queued.text.ends_with("Review it twice.\n\nARGUMENTS: the parser\n"), "{:?}", queued.text);
}

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

#[test]
fn opening_a_session_keeps_the_current_owner_and_draft_until_commit() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use super::KeyOutcome;
    let mut ui = test_ui();
    ui.session_id = "current-session".into();
    ui.composer.insert_str("keep this draft");
    ui.session_opening = Some("chosen-session".into());
    assert!(matches!(ui.idle_event(&Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))), KeyOutcome::Nothing));
    assert_eq!(ui.composer.text(), "keep this draft");
    assert!(matches!(ui.idle_event(&Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))), KeyOutcome::CancelOpening));
    assert_eq!(ui.session_id, "current-session");
    assert_eq!(ui.composer.text(), "keep this draft");
}
