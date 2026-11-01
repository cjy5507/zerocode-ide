//! t-68069: one folder, two conversations, two panes — each pane opens its own.
//!
//! On 2026-10-21 two workers shared one checkout. After a tab closed, one
//! pane came back on its neighbour's conversation instead of its own: an
//! agent's own "continue" takes the newest conversation of its folder, and
//! two processes then wrote one transcript.
//!
//! Driven through the production roads behind a fake launcher, as
//! `restore_door.rs` is: the door's wake (`wake_conversation`), the ledger's
//! summons (`Catalog::command_for`) and the launch door's judgment
//! (`neighbour_in_folder`, `unnamed_launch_args`). What each started command
//! line opens is read by [`opened_by`], which does what the CLI does with its
//! selectors over a real temporary folder.

use super::restore_door::Door;
use super::*;
use crate::conversation_wake::{
    FolderPanes, PaneLaunches, ReportedWord, WakeRoad, neighbour_in_folder, unnamed_launch_args,
    word_on_report,
};
use std::collections::HashMap;
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
    if let Some(at) = words
        .iter()
        .position(|word| matches!(*word, "--resume" | "-r"))
    {
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

/// t-68069 P3-5: zo's `-r` is `--resume` by another name (`zo-ide`
/// `ide/args.rs`): `-r latest` is the folder's newest.
#[test]
fn a_summons_carries_no_short_resume_either() {
    let folder = Folder::new();
    let catalog = Catalog::new(vec![(
        "zo".to_string(),
        LaunchOverride {
            args: Some("-r latest".to_string()),
            env: None,
        },
    )]);
    let command = catalog.command_for("zo", "", &[]).expect("the summons");
    assert_eq!(
        opened_by(&folder, &command),
        None,
        "zo was summoned onto the folder's newest: {command}"
    );
}

/// The launch door's judgment in the production order: the book and the args,
/// over a window whose panes have not reported anything yet. Answers the
/// command line it starts.
struct LaunchDoor<'a> {
    book: PaneLaunches,
    folder: &'a Folder,
}

/// A window whose agents have not reported: nothing stands in its pane tables,
/// so only the book can see them.
struct NoReportYet;

impl FolderPanes for NoReportYet {
    fn agent_panes(&self) -> HashMap<u32, &'static str> {
        HashMap::new()
    }
    fn pane_folder(&self, _term: u32) -> Option<std::path::PathBuf> {
        None
    }
    fn shell_in_front(&self, _term: u32) -> bool {
        false
    }
}

impl<'a> LaunchDoor<'a> {
    fn new(folder: &'a Folder) -> Self {
        Self {
            book: PaneLaunches::new(),
            folder,
        }
    }

    /// Open a pane with `--continue` saved. `restored_leaf` is what a restore's
    /// id-less leaf says.
    fn launch(&self, term: u32, restored_leaf: bool) -> Option<String> {
        let (neighbour, opening) =
            neighbour_in_folder(&self.book, &NoReportYet, term, "claude", self.folder.path());
        opening.opened();
        let planned = vec![SAVED_CONTINUE.to_string()];
        let launch = unnamed_launch_args(Vec::new(), &planned, restored_leaf, neighbour);
        let command = std::iter::once("claude".to_string())
            .chain(launch.args)
            .collect::<Vec<_>>()
            .join(" ");
        opened_by(self.folder, &command)
    }
}

/// t-68069 P2-3: two panes opened one after another with `--continue` saved.
/// The first has not reported anything when the second asks: the second starts
/// a new conversation, and one transcript has one writer.
#[test]
fn two_panes_opened_in_a_row_with_a_saved_continue_do_not_share_a_conversation() {
    let folder = Folder::new();
    let door = LaunchDoor::new(&folder);
    assert_eq!(
        door.launch(1, false).as_deref(),
        Some(NEWEST),
        "alone in the folder, the person's setting stands"
    );
    assert_eq!(
        door.launch(2, false),
        None,
        "the second pane took the first's conversation before the first had reported"
    );
}

/// t-68069 P2-3: a restore whose id-less leaf comes before the leaf naming the
/// folder's newest conversation. The id-less leaf starts a new conversation, so
/// the named leaf keeps its own and exactly one process opens it.
#[test]
fn an_id_less_leaf_before_a_named_one_leaves_it_its_conversation() {
    const ID_LESS: u32 = 680_730;
    const NAMED: u32 = 680_731;
    let (_window, _store) = PrivateWindow::boot();
    let folder = Folder::new();
    let launch_door = LaunchDoor::new(&folder);
    let door = Door::new(folder.path());
    door.saves_launch_args(SAVED_CONTINUE);
    door.on_road(WakeRoad::RestoreFront);

    let id_less = launch_door.launch(ID_LESS, true);
    door.wake_with(NAMED, "claude", folder.record(NEWEST))
        .expect("the named leaf's wake");
    let started = door.started();
    let (_, command) = started
        .iter()
        .find(|(held, _)| *held == NAMED)
        .expect("the named leaf started");
    let named = opened_by(&folder, command);

    assert_eq!(
        named.as_deref(),
        Some(NEWEST),
        "the named leaf lost its own"
    );
    assert_eq!(
        id_less, None,
        "the id-less leaf took the conversation the named leaf names: two processes on one \
         transcript"
    );
}

/// t-68069 P2-3: the production door says what the scene above assumes. A
/// restored leaf says so, the launch door passes it on, and a launch's line is
/// written with its judgment before its process exists.
#[test]
fn a_restored_leaf_says_so_and_the_launch_door_judges_before_it_spawns() {
    let window = include_str!("../../../../../ui/shell-term.js");
    assert!(
        window.contains(r#"launchAgentTab({ agent: launched, prompt: "", restoredLeaf: true,"#),
        "a restore's id-less leaf no longer says it is one"
    );
    let source = include_str!("../../cmd/terminal.rs");
    let door = &source[source.find("fn launch_agent_tab(").expect("the door")..];
    let judged = door.find("neighbour_in_folder(").expect("the judgment");
    let spawned = door.find("let spawned = if").expect("the spawn");
    assert!(
        judged < spawned,
        "the pane is judged after its process exists"
    );
    assert!(
        door.contains("restored_leaf.unwrap_or(false)"),
        "the launch door dropped a restored leaf's word"
    );
    let named = door
        .find("state.pane_sessions().insert(")
        .expect("the record");
    assert!(
        named < spawned,
        "a named conversation is written after its process exists"
    );
}

/// t-68069 P2-1: a wake's process reports before the wake writes its record, as
/// zo's `source: "resume"` does right after its channel. Driven through the
/// door's own road: inside its wake the report is that wake's conversation, so
/// the pane's head and the coordinator hear nothing. Once settled, the record
/// answers.
#[test]
fn a_report_before_the_wakes_record_is_no_surprise() {
    const TERM: u32 = 680_740;
    let (_window, _store) = PrivateWindow::boot();
    let folder = Folder::new();
    let door = Door::new(folder.path());
    let reported = folder.record(NEWEST);
    let heard: std::sync::Arc<Mutex<Vec<Option<ReportedWord>>>> = Default::default();
    *door.during_prepare.lock().unwrap() = Some(Box::new({
        let heard = std::sync::Arc::clone(&heard);
        let reported = reported.clone();
        move || {
            let word = word_on_report("claude", TERM, None, &reported, || true, |_| None);
            heard.lock().unwrap().push(word);
        }
    }));
    door.wake_with(TERM, "claude", reported).expect("the wake");
    assert_eq!(
        *heard.lock().unwrap(),
        vec![None],
        "the door's own conversation was said as one the CLI picked"
    );
}

/// t-68069 P2-1: the ledger's reseat has no pane number to claim by, so its
/// record is written before the process exists: the ask carries it, and the
/// split host writes it first. Read off the source because the split needs a
/// whole window; `an_id_less_leaf…` and the ledger's own tests hold the rest.
#[test]
fn a_reseat_writes_its_conversation_before_its_process_exists() {
    let ledger = include_str!("../../orchestration.rs");
    assert!(
        ledger.contains("session: sleeper_record(&prepared.worker),"),
        "the reseat's ask does not carry its conversation"
    );
    let source = include_str!("../../agent_tools_runtime.rs");
    let split = &source[source.find("fn split(").expect("the split")..];
    let written = split
        .find("state.pane_sessions().insert(term, session);")
        .expect("the record");
    let spawned = split.find("PtyLane::spawn(").expect("the spawn");
    assert!(
        written < spawned,
        "the reseat's record comes after its process"
    );
}

/// t-68069 P3-6: a vault row's door answers as every other door does, and waits
/// out a wake on a thread of its own.
#[test]
fn a_vault_row_is_answered_with_the_pane_and_keeps_no_runtime_thread_waiting() {
    let source = include_str!("../../cmd/fs.rs");
    let door = source.find("fn resume_vault_session(").expect("the door");
    assert!(
        source[..door]
            .trim_end()
            .ends_with("#[tauri::command(async)]\npub(crate)"),
        "the vault's door holds a runtime worker across its wait"
    );
    let body = &source[door..];
    assert!(
        body.contains("ConversationWake::standing(holder)")
            && body.contains("RESEAT_CLAIM_TERM")
            && !body.contains("터미널 {holder}"),
        "the vault's door names a pane number it may not have, or names no way to it"
    );
    let window = include_str!("../../../../../ui/shell.js");
    let reopening = &window[window
        .find("async function resumeVaultSession(")
        .expect("the window's door")..];
    assert!(
        reopening.contains("if (woke.standing)") && reopening.contains("focusAgentPane("),
        "the window does not take the person to the pane holding the conversation"
    );
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
