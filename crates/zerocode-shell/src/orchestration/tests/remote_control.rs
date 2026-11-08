//! The phone link's first step: the Claude panes a person opens start with
//! Claude Code's Remote Control, and nothing the ledger starts does.
//!
//! Driven through the production roads behind the fake launcher, as
//! `one_folder.rs` is: the door's wake (`wake_conversation`) and the ledger's
//! summons (`Catalog::command_for`).

use super::restore_door::Door;
use super::*;
use zerocode_core::remote_control::ClaudeRemoteControl;
use zerocode_core::{ProviderSession, SessionKey};

const WRITTEN: &str = "c0de0001-0000-4000-8000-00000000000a";
const NEVER_WRITTEN: &str = "c0de0002-0000-4000-8000-00000000000b";

/// A checkout holding one written conversation.
fn checkout() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a checkout");
    std::fs::write(dir.path().join(format!("{WRITTEN}.jsonl")), "{}\n")
        .expect("a written conversation");
    dir
}

fn record(folder: &std::path::Path, id: &str) -> ProviderSession {
    ProviderSession {
        key: SessionKey::SessionId,
        id: id.to_string(),
        transcript_path: Some(
            folder
                .join(format!("{id}.jsonl"))
                .to_string_lossy()
                .into_owned(),
        ),
    }
}

fn started_command(door: &Door, term: u32) -> String {
    door.started()
        .into_iter()
        .find(|(held, _)| *held == term)
        .map(|(_, command)| command)
        .expect("it started")
}

#[test]
fn a_pane_that_never_spoke_comes_back_reachable_from_a_phone() {
    const TERM: u32 = 990_801;
    let (_window, _store) = PrivateWindow::boot();
    let folder = checkout();
    let door = Door::new(folder.path());
    door.keeps_remote_control(ClaudeRemoteControl::Mine);
    door.wake_with(TERM, "claude", record(folder.path(), NEVER_WRITTEN))
        .expect("a pane that never spoke starts anew");
    let command = started_command(&door, TERM);
    assert!(
        command.contains("--remote-control --remote-control-session-name-prefix"),
        "the fresh start lost its phone link: {command}"
    );
}

#[test]
fn a_pane_that_never_spoke_comes_back_plain_when_the_person_turned_it_off() {
    const TERM: u32 = 990_802;
    let (_window, _store) = PrivateWindow::boot();
    let folder = checkout();
    let door = Door::new(folder.path());
    door.keeps_remote_control(ClaudeRemoteControl::Off);
    door.wake_with(TERM, "claude", record(folder.path(), NEVER_WRITTEN))
        .expect("a pane that never spoke starts anew");
    let command = started_command(&door, TERM);
    assert!(
        !command.contains("--remote-control"),
        "a phone link the person turned off: {command}"
    );
}

#[test]
fn a_written_conversation_resumes_without_a_second_remote_session() {
    const TERM: u32 = 990_803;
    let (_window, _store) = PrivateWindow::boot();
    let folder = checkout();
    let door = Door::new(folder.path());
    door.keeps_remote_control(ClaudeRemoteControl::Mine);
    door.wake_with(TERM, "claude", record(folder.path(), WRITTEN))
        .expect("the conversation resumes");
    let command = started_command(&door, TERM);
    assert!(
        command.contains(&format!("--resume {WRITTEN}")) && !command.contains("--remote-control"),
        "a resume must reconnect its own remote session, not open another: {command}"
    );
}

#[test]
fn a_summoned_worker_never_reaches_a_phone() {
    let command = Catalog::new(Vec::new())
        .command_for("claude", "brief the worker", &[])
        .expect("the summons");
    assert!(
        !command.contains("--remote-control"),
        "a worker reports to its coordinator, not to a phone: {command}"
    );
}
