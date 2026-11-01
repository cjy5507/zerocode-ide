//! t-68069: one folder, two conversations, two panes — each pane opens its own.
//!
//! On 2026-10-21 two workers shared one checkout. After a tab closed, one
//! pane came back on its neighbour's conversation instead of its own: an
//! agent's own "continue" takes the newest conversation of its folder, and
//! two processes then wrote one transcript.
//!
//! Driven through the production roads behind a fake launcher, as
//! `restore_door.rs` is: the door's wake (`wake_conversation`) and the
//! ledger's summons (`Catalog::command_for`). What each started command line
//! opens is read by [`opened_by`], which does what the CLI does with its
//! selectors over a real temporary folder.

use super::restore_door::Door;
use super::*;
use crate::conversation_wake::WakeRoad;
use std::time::{Duration, SystemTime};
use zerocode_core::{LaunchOverride, ProviderSession, SessionKey};

const OLDER: &str = "68069a17-0000-4000-8000-00000000000a";
const NEWEST: &str = "68069b04-0000-4000-8000-00000000000b";
const NEVER_WRITTEN: &str = "68069c55-0000-4000-8000-00000000000c";
/// A launch line that saved the agent's own "continue".
const SAVED_CONTINUE: &str = "--continue";

/// The folder both panes stand in, holding `OLDER` and `NEWEST`.
struct Folder(tempfile::TempDir);

impl Folder {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("the shared folder");
        for (id, age) in [(OLDER, 600), (NEWEST, 5)] {
            let file = std::fs::File::create(dir.path().join(format!("{id}.jsonl")))
                .expect("a conversation");
            file.set_modified(SystemTime::now() - Duration::from_secs(age))
                .expect("its age");
        }
        Self(dir)
    }

    fn path(&self) -> &std::path::Path {
        self.0.path()
    }

    /// The conversation whose file moved last: what a bare "continue" takes.
    fn newest(&self) -> String {
        let mut files: Vec<(SystemTime, String)> = std::fs::read_dir(self.path())
            .expect("the folder")
            .map(|entry| {
                let entry = entry.expect("an entry");
                (
                    entry
                        .metadata()
                        .and_then(|held| held.modified())
                        .expect("a time"),
                    entry
                        .path()
                        .file_stem()
                        .expect("a stem")
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .collect();
        files.sort();
        files.pop().expect("a conversation").1
    }

    fn record(&self, id: &str) -> ProviderSession {
        ProviderSession {
            key: SessionKey::SessionId,
            id: id.to_string(),
            transcript_path: Some(
                self.path()
                    .join(format!("{id}.jsonl"))
                    .to_string_lossy()
                    .into_owned(),
            ),
        }
    }
}

/// The conversation a started command line opens: the one its selector names,
/// the folder's newest for a bare "continue", or `None` for a new one. What
/// `claude` and `zo` do with these words (`zo-ide` `ide/args.rs`, Claude Code's
/// own `--help`), kept to the words the launch roads can carry.
fn opened_by(folder: &Folder, command: &str) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    if let Some(at) = words.iter().position(|word| *word == "--resume") {
        return words.get(at + 1).map(|named| (*named).to_string());
    }
    words
        .iter()
        .any(|word| matches!(*word, "--continue" | "-c"))
        .then(|| folder.newest())
}

fn saved_continue() -> LaunchOverride {
    LaunchOverride {
        args: Some(SAVED_CONTINUE.to_string()),
        env: None,
    }
}

/// The picture this task is named for. Both panes' records name a written
/// conversation; a saved "continue" stands in the launch args. Each pane opens
/// its own, the older one too, on every road a stored record comes by. Green
/// since #12982 took the selectors out of a resume's base.
#[test]
fn two_panes_in_one_folder_each_open_their_own_conversation() {
    const FIRST: u32 = 680_690;
    let (_window, _store) = PrivateWindow::boot();
    let folder = Folder::new();
    let door = Door::new(folder.path());
    door.saves_launch_args(SAVED_CONTINUE);
    let mut wrong = Vec::new();
    for (road, term) in [
        WakeRoad::RestoreFront,
        WakeRoad::RestoreBehind,
        WakeRoad::Visit,
        WakeRoad::Stage,
        WakeRoad::Reopen,
    ]
    .into_iter()
    .zip((FIRST..).step_by(2))
    {
        door.on_road(road);
        for (at, id) in [OLDER, NEWEST].into_iter().enumerate() {
            let term = term + u32::try_from(at).expect("two panes");
            door.wake_with(term, "claude", folder.record(id))
                .expect("the wake");
            let started = door.started();
            let (_, command) = started.iter().find(|(held, _)| *held == term).expect("it");
            let opened = opened_by(&folder, command);
            if opened.as_deref() != Some(id) {
                wrong.push(format!("{}: {id} opened {opened:?}", road.word()));
            }
        }
        // The next road's panes wake the same conversations.
        door.close_all();
    }
    assert_eq!(wrong, Vec::<String>::new());
}

/// t-68069 ①, the road still open at v1.1.113. A pane whose own conversation
/// was never written comes back as a new conversation (`fresh_command`), and
/// that launch kept the saved "continue": the CLI took the folder's newest,
/// its neighbour's, and two processes wrote one transcript. Wrong picks 1 → 0.
#[test]
fn a_pane_that_never_spoke_does_not_take_its_neighbours_conversation() {
    const NEIGHBOUR: u32 = 680_710;
    const NEVER_SPOKE: u32 = 680_711;
    let (_window, _store) = PrivateWindow::boot();
    let folder = Folder::new();
    let door = Door::new(folder.path());
    door.saves_launch_args(SAVED_CONTINUE);
    door.wake_with(NEIGHBOUR, "claude", folder.record(NEWEST))
        .expect("the neighbour");
    door.wake_with(NEVER_SPOKE, "claude", folder.record(NEVER_WRITTEN))
        .expect("a pane that never spoke starts anew");

    let started = door.started();
    let command = |term: u32| {
        started
            .iter()
            .find(|(held, _)| *held == term)
            .map(|(_, command)| command.as_str())
            .expect("it started")
    };
    assert_eq!(
        opened_by(&folder, command(NEIGHBOUR)).as_deref(),
        Some(NEWEST)
    );
    assert_eq!(
        opened_by(&folder, command(NEVER_SPOKE)),
        None,
        "the pane that never spoke took a conversation: {}",
        command(NEVER_SPOKE)
    );
}

/// t-68069 ①: a summons is a new conversation whatever the launch args saved.
/// It kept the saved "continue" too: a worker summoned beside a neighbour
/// started on the folder's newest, the very picture of 2026-10-21. Zo spells
/// the same selector (`zo-ide` `ide/args.rs`), on its own row.
#[test]
fn a_summons_beside_a_neighbour_starts_its_own_conversation() {
    let folder = Folder::new();
    for agent in ["claude", "zo"] {
        let catalog = Catalog::new(vec![(agent.to_string(), saved_continue())]);
        let command = catalog.command_for(agent, "", &[]).expect("the summons");
        assert_eq!(
            opened_by(&folder, &command),
            None,
            "{agent} was summoned onto the folder's newest: {command}"
        );
    }
}

/// t-68069 ①: `latest` is not a conversation, it is zo's word for the folder's
/// newest (`runtime/session_control.rs`), and the record of a pane may not
/// carry that word onto a command line. Nothing starts, and the door says so.
#[test]
fn a_record_naming_the_folders_newest_opens_nothing() {
    const TERM: u32 = 680_720;
    let (_window, _store) = PrivateWindow::boot();
    let folder = Folder::new();
    let door = Door::new(folder.path());
    for (at, word) in ["latest", "last", "recent", "LATEST"]
        .into_iter()
        .enumerate()
    {
        let term = TERM + u32::try_from(at).expect("four words");
        let woke = door.wake(term, "zo", word);
        assert!(woke.is_err(), "{word} opened {woke:?}");
        assert!(
            door.started().iter().all(|(held, _)| *held != term),
            "{word} started a process"
        );
    }
}

/// t-68069 ①: the automation road starts a new conversation too. Read off its
/// source because a run's launch needs a whole window; the scene above holds
/// what the words do.
#[test]
fn an_automations_launch_carries_no_selector() {
    let source = include_str!("../../automation_runtime.rs");
    assert!(
        source.contains("launch_args_without_selectors(&plan.args)"),
        "an automation's launch keeps a saved selector"
    );
    assert!(
        !source.contains("words.extend(plan.args);"),
        "an automation's launch keeps a saved selector"
    );
}
