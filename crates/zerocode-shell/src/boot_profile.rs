//! A synthetic profile sized like a long-lived machine, for measuring a boot
//! (t-20078). Test-only: it is the reproduction's data maker, not product code.
//!
//! It writes into a folder NAMED BY THE CALLER and nowhere else — never the
//! person's state folders. The harness points `HOME` (and the ledger's own
//! data-root variable) at that folder before it starts a window, so the
//! window reads the synthetic files and the person's are not on any road.
//!
//! The sizes are the ones the slow restart of 2026-10-01 had, named below so
//! nobody has to remember why a number is what it is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use zerocode_core::SessionKey;
use zerocode_core::agent_teams::{LEADER_PANE, Team};
use zerocode_core::orchestration::{Launcher, Ledger, plan, receipt_actor};

use crate::pane_layout::{PaneNode, SplitDirection, TabLayout, WakeAgent};

/// The folder the profile is written into. Unset means the test does nothing:
/// a plain `cargo test` must never write a profile anywhere.
const PROFILE_DIR_ENV: &str = "ZEROCODE_BOOT_PROFILE_DIR";

/// The workspaces (colon-separated, existing checkouts) the layouts are filed
/// under. The harness makes them; the generator only names them in the file.
const PROFILE_WORKTREES_ENV: &str = "ZEROCODE_BOOT_PROFILE_WORKTREES";

/// Tasks in the ledger: the 2026-10-01 machine's ledger held about this many
/// (authority.sqlite was 29 MB).
const LEDGER_TASKS: u32 = 19_000;

/// Terminals with stored scrollback across all workspaces: the person's
/// restart put back about twenty.
const TERMINALS: usize = 20;

/// Scrollback kept per terminal: pane-layouts.json was 4.8 MB over about
/// twenty terminals, so roughly 250 KB each once the escapes are spelled in
/// JSON — under the capture's own cap ([`zerocode_pty::SCROLLBACK_BUFFER_BYTE_LIMIT`]), which the file
/// would otherwise drop on read.
const SCROLLBACK_BYTES_PER_TERMINAL: usize = 180 * 1024;

/// Leaves in one synthetic tab: two side by side, the common split.
const LEAVES_PER_TAB: usize = 2;

/// A line of scrollback with a colour change in it, repeated: terminal
/// output is mostly short coloured lines, and the escape bytes are what make
/// the stored text and its replay cost like the real thing.
const SCROLLBACK_LINE: &str =
    "\u{1b}[32m✔\u{1b}[0m build step finished in 1.2s \u{1b}[2m(cached)\u{1b}[0m\r\n";

struct AnyAgent;

impl Launcher for AnyAgent {
    fn command_for(&self, agent: &str, _: &str, _: &[String]) -> Result<String, String> {
        Ok(agent.to_string())
    }
}

fn scrollback() -> String {
    SCROLLBACK_LINE
        .repeat(SCROLLBACK_BYTES_PER_TERMINAL / SCROLLBACK_LINE.len() + 1)
        .chars()
        .take(SCROLLBACK_BYTES_PER_TERMINAL)
        .collect()
}

/// The programs the conversations were running, alternated: the person's
/// restart woke a mix, and each wake is one more process the boot starts.
const WAKE_AGENTS: [&str; 2] = ["claude", "codex"];

/// A session id in the shape the wake accepts (a UUID); the number makes each
/// tab's conversation its own.
fn session_id(number: usize) -> String {
    format!("00000000-0000-4000-8000-{number:012}")
}

fn tab(number: usize, awake: bool, buffers: usize) -> TabLayout {
    let agent = WAKE_AGENTS[number % WAKE_AGENTS.len()];
    TabLayout {
        id: None,
        root: PaneNode::Split {
            direction: SplitDirection::Vertical,
            first: Box::new(PaneNode::Leaf),
            second: Box::new(PaneNode::Leaf),
            ratio: None,
        },
        titles: HashMap::new(),
        names: HashMap::new(),
        agents: HashMap::from([(
            0,
            WakeAgent {
                agent: agent.to_string(),
                key: "session_id".to_string(),
                id: session_id(number),
                transcript_path: None,
                interrupted: false,
            },
        )]),
        running: HashMap::from([(0, agent.to_string())]),
        active: 0,
        expanded: None,
        pinned: false,
        focused: false,
        terms: HashMap::new(),
        buffers: (0..buffers.min(LEAVES_PER_TAB))
            .map(|leaf| (leaf, scrollback()))
            .collect(),
        owed: Vec::new(),
        awake,
    }
}

/// The layouts file's content: every terminal's scrollback spread over the
/// named workspaces, tabs after the first standing awake so the boot's
/// "put back the workspaces behind" road has work too.
fn layouts(worktrees: &[String]) -> HashMap<String, Vec<TabLayout>> {
    let mut filed: HashMap<String, Vec<TabLayout>> = HashMap::new();
    let tabs = TERMINALS.div_ceil(LEAVES_PER_TAB);
    for at in 0..tabs {
        let worktree = worktrees[at % worktrees.len()].clone();
        filed
            .entry(worktree)
            .or_default()
            .push(tab(at, at >= worktrees.len(), LEAVES_PER_TAB));
    }
    filed
}

fn ledger() -> Ledger {
    let mut ledger = Ledger::new();
    let mut team = Team::new("boot-profile", "boot-profile-capability", 1);
    let actor = receipt_actor("claude", SessionKey::SessionId, "boot-profile");
    let spoke = |ledger: &mut Ledger, team: &mut Team, line: &str| {
        let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        let decided = plan(ledger, team, &AnyAgent, &argv, LEADER_PANE, 0, Some(&actor));
        assert_eq!(
            decided.reply.exit_code, 0,
            "{line}: {}",
            decided.reply.stderr
        );
    };
    spoke(
        &mut ledger,
        &mut team,
        "run-create --name boot-profile --retry-request open-boot-profile",
    );
    for at in 0..LEDGER_TASKS {
        spoke(
            &mut ledger,
            &mut team,
            &format!("task-create --spec slice-{at} --retry-request task-{at}"),
        );
    }
    ledger
}

fn target() -> Option<PathBuf> {
    std::env::var_os(PROFILE_DIR_ENV)
        .filter(|named| !named.is_empty())
        .map(PathBuf::from)
}

fn write_profile(dir: &Path, worktrees: &[String]) {
    std::fs::create_dir_all(dir).expect("profile folder");
    let layouts = serde_json::to_vec(&layouts(worktrees)).expect("layouts");
    std::fs::write(
        dir.join(crate::app_paths::artifact_file::PANE_LAYOUTS),
        layouts,
    )
    .expect("layouts file");
    let ledger = serde_json::to_vec(&ledger()).expect("ledger");
    std::fs::write(dir.join("orchestration.json"), ledger).expect("ledger file");
}

#[test]
#[ignore = "writes a profile into ZEROCODE_BOOT_PROFILE_DIR; the boot harness runs it"]
fn writes_the_synthetic_boot_profile() {
    let Some(dir) = target() else {
        panic!("{PROFILE_DIR_ENV} names the folder the profile is written into");
    };
    let worktrees: Vec<String> = std::env::var(PROFILE_WORKTREES_ENV)
        .unwrap_or_default()
        .split(':')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    assert!(
        !worktrees.is_empty(),
        "{PROFILE_WORKTREES_ENV} names the checkouts"
    );
    write_profile(&dir, &worktrees);
}

#[test]
fn the_profile_is_sized_like_the_machine_it_stands_for() {
    let filed = layouts(&["/synthetic/one".to_string(), "/synthetic/two".to_string()]);
    let terminals: usize = filed.values().flatten().map(|tab| tab.buffers.len()).sum();
    assert_eq!(terminals, TERMINALS);
    let bytes = serde_json::to_vec(&filed).expect("layouts").len();
    // 4.8 MB on the machine; within a fifth of it is "like".
    assert!(
        (3_800_000..=5_800_000).contains(&bytes),
        "layouts are {bytes} bytes"
    );
    assert!(
        filed.values().flatten().all(|tab| tab
            .buffers
            .values()
            .all(|held| held.len() <= zerocode_pty::SCROLLBACK_BUFFER_BYTE_LIMIT)),
        "a buffer past the capture's cap is dropped on read"
    );
}
