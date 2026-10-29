//! The shape of a terminal tab's panes, kept across the window closing.
//!
//! Orca serialises each tab's pane tree into its session store and rebuilds
//! it on the way back up (`serializePaneTree` / `replayTerminalLayout`,
//! I18nProvider-4EBrmTGg.js:46100-46226). The tree it writes is leaves and
//! splits with two measured rules on the ratio:
//!
//! - **an even split carries no number.** A ratio within 5e-3 of one half is
//!   omitted entirely (`Math.abs(r - 0.5) > 5e-3`, :46127) — absent means
//!   even, and most splits are even, so most splits serialise to nothing.
//! - **an uneven one is rounded to three decimals** (`Math.round(r * 1e3) /
//!   1e3`, :46128) — a divider parked by a human hand does not deserve
//!   fifteen digits of the flexbox arithmetic that happened to land it there.
//!
//! Both rules live HERE, not in the window: the window sends the ratios it
//! has, and what goes into the file is decided in one place — the same
//! reason the vault's filtering and the board's columns are Rust's. What
//! comes back out is already normalised, so the window never has to know the
//! rules exist.
//!
//! Titles, the active pane and the expanded pane travel as **leaf ordinals**
//! rather than shell ids. Orca's leaf ids are UUIDs that outlive a restart;
//! ours are process-lifetime pty numbers, so the stored tree names its
//! leaves by position — left to right, top to bottom, the order
//! `paneLeaves` walks — and the window maps positions onto the fresh shells
//! it spawns.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// How near one half a ratio can sit and still be "even" — Orca's own
/// tolerance (I18nProvider-4EBrmTGg.js:46127).
const EVEN_ENOUGH: f64 = 5e-3;

/// The most leaves one stored tab is believed to hold.
///
/// The window itself has no such limit, but this file is parsed on trust at
/// startup, and a hand-edited tree of ten thousand leaves would be ten
/// thousand shells spawned before the first frame. Sixteen is beyond any
/// layout a person can read, and a tab that claims more is refused whole.
const MAX_LEAVES: usize = 16;

/// One node of a stored pane tree — Orca's own two spellings
/// (`{type:"leaf"}` / `{type:"split", direction, first, second, ratio?}`),
/// minus the leaf id, which is positional here (see the module note).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PaneNode {
    Leaf,
    Split {
        direction: SplitDirection,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ratio: Option<f64>,
    },
}

/// `vertical` divides left|right, `horizontal` top/bottom — the window's own
/// spellings, which are Orca's.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Vertical,
    Horizontal,
}

impl PaneNode {
    pub(crate) fn leaves(&self) -> usize {
        match self {
            PaneNode::Leaf => 1,
            PaneNode::Split { first, second, .. } => first.leaves() + second.leaves(),
        }
    }

    /// The two measured rules, applied to every split on the way down.
    ///
    /// A ratio that is not a proportion at all — NaN, zero, one, anything
    /// outside the open interval — is dropped rather than repaired: absent
    /// means even, and even is the only safe reading of a number that never
    /// described a division.
    pub(crate) fn normalized(self) -> PaneNode {
        match self {
            PaneNode::Leaf => PaneNode::Leaf,
            PaneNode::Split {
                direction,
                first,
                second,
                ratio,
            } => PaneNode::Split {
                direction,
                first: Box::new(first.normalized()),
                second: Box::new(second.normalized()),
                ratio: ratio
                    .filter(|value| value.is_finite() && *value > 0.0 && *value < 1.0)
                    .filter(|value| (value - 0.5).abs() > EVEN_ENOUGH)
                    .map(|value| (value * 1e3).round() / 1e3),
            },
        }
    }
}

/// A conversation asleep in one leaf: which agent, and the session its
/// vendor knows the conversation by.
///
/// What Orca's per-pane sleeping record keeps (its cold restore reads
/// `sleepingRecord.agent` + `sleepingRecord.providerSession`,
/// index-ftls8Hg_.js:95637-95647), minus the launch config this window does
/// not have. The window fills these from the sessions the hook bridge
/// reported, and only for the ones the backend called resumable — a stored
/// wake that fails is worse than a plain shell.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WakeAgent {
    pub agent: String,
    /// `session_id` or `conversation_id` — the two spellings Orca's
    /// normaliser accepts (`normalizeAgentProviderSession`,
    /// I18nProvider-4EBrmTGg.js:27961-27966). Anything else is a file
    /// somebody edited, and is dropped rather than handed to an argv.
    pub key: String,
    pub id: String,
    /// One agent resumes BY its transcript path rather than by the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// Whether this conversation was MID-TURN at the last persist — its
    /// state was `working` — as the hook said it then.
    ///
    /// Kept in the record, and no longer what decides a continue (t-7812 E).
    /// A person's own tab is reopened as it stood, never told to go on; a
    /// worker's wake is told by the goodbye's own reading of its turn and the
    /// commands under its pane (`restart_nudge_runtime::worker_nudge`), which
    /// a crash leaves unsaid rather than guessed from this mark.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
}

impl WakeAgent {
    /// Is this something `resume_session` could be handed?
    ///
    /// The full argv validation lives in `zerocode_core::resume_argv` and
    /// runs again at wake time — this only refuses what is visibly not a
    /// record: empty names, a key that is neither spelling, an id no vendor
    /// mints. The point of checking here too is that a refused entry
    /// disappears from the FILE, so it cannot sit there failing every boot.
    fn keeps(&self) -> bool {
        !self.agent.trim().is_empty()
            && matches!(self.key.as_str(), "session_id" | "conversation_id")
            && zerocode_core::provider_session::is_usable_session_id(&self.id)
    }
}

/// One terminal tab, as the file remembers it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TabLayout {
    /// The name the FILE knows this tab by, minted here on the way in.
    ///
    /// Positions cannot serve: a tab restored from this file stands on the
    /// strip with no shells in it until somebody looks at it (Orca's
    /// connect-on-render), and in the meantime tabs around it are opened,
    /// closed and pinned to the front. Every later save would then hand the
    /// screens of one tab to another. A name minted once and echoed back
    /// unchanged is the only thing in this record that survives being
    /// re-ordered — which is why both the screens below and
    /// the replay (`replay_stored_screen`) are addressed by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub root: PaneNode,
    /// Pane titles a PERSON gave, by leaf ordinal — sparse, because most
    /// panes are unnamed and a tab with no names should store no object at
    /// all.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub titles: HashMap<usize, String>,
    /// The name each pane was BORN with, by leaf ordinal — the launch's word
    /// for a shell seated in a division (ZO, Codex, an action's name), or the
    /// words that started it. Kept apart from `titles` because the window's
    /// bar treats the two differently: the X takes a person's title back,
    /// and over a born name it closes the pane.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub names: HashMap<usize, String>,
    /// The conversations asleep in this tab, by leaf ordinal — the leaves
    /// the window wakes with the agent's own resume command instead of a
    /// plain shell.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub agents: HashMap<usize, WakeAgent>,
    /// The agent each leaf was RUNNING, by ordinal — sparse, and a
    /// different question from `agents` above. That one is a conversation to
    /// re-enter; this one is a program that was there.
    ///
    /// The two come apart at `zo`, which wires no hook at all and so never
    /// reports a session — leaving nothing in the record to say the pane held
    /// an agent. A pane stored with nothing is a pane stored as a bare shell,
    /// and a set of bare shells is exactly the shape the activation rule
    /// stands aside for in favour of the chosen default agent. Reported live:
    /// the workspace that was running `zo` came back running `claude`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub running: HashMap<usize, String>,
    /// Which leaf held the keyboard, by ordinal.
    #[serde(default)]
    pub active: usize,
    /// The leaf expanded over its siblings, when one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<usize>,
    /// Whether this tab was pinned to its strip (`isPinned`, mirrored onto
    /// Orca's terminal tabs by `patchTerminalTabPinned`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// The one tab of the set that was being LOOKED at — Orca's
    /// `activeTabId`, kept in the same session record as the tab set itself
    /// (`getDefaultWorkspaceSession`, out/main/index.js@163639).
    ///
    /// It rides the tab rather than the worktree because it is the tab that
    /// moves: a set that comes back in a different order still knows which of
    /// its members the eye was on. And it decides which single tab SPAWNS on
    /// the way back — the others come up asleep — so a file claiming two of
    /// them is refused down to one in [`store`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    /// The live shell ids by ordinal, from the session that wrote the file —
    /// Orca's `ptyIdsByLeafId`. Only the exit capture reads them: pty ids
    /// die with their processes, so to any LATER session these are numbers,
    /// not handles.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub terms: HashMap<usize, u32>,
    /// What each leaf's screen held when the window closed, by ordinal —
    /// Orca's `buffersByLeafId`. Written at exit from the grids the backend
    /// owns, replayed into the fresh shells on restore.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub buffers: HashMap<usize, String>,
    /// The conversations this tab stood for that did not come back, and that
    /// a newer conversation took the place of (t-12063). The file keeps them
    /// so a pane that could not wake is not forgotten the moment somebody
    /// types a new `zo` into the shell standing in for it — which is how
    /// 2026-09-28's lost the 08:30 conversation — and the window names them
    /// in one line when a new conversation starts in the workspace. Never
    /// woken on their own: which conversation a pane holds is the person's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owed: Vec<WakeAgent>,
    /// Whether this tab's shells were standing when the window went — the
    /// exit's own word ([`capture_exit`]), never the window's: a save from the
    /// window does not carry it (t-14036). The next boot puts back, behind the
    /// workspace in front, every workspace whose conversations were running
    /// ([`standing_worktrees`]); a conversation asleep at the exit stays asleep
    /// until somebody steps into its workspace.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub awake: bool,
}

/// How many owed conversations one tab keeps: the newest few, never a pile.
const MAX_OWED: usize = 8;

/// Words by leaf ordinal, bounded against the tree they point into: entries
/// past the last leaf and entries that say nothing go. One reader for a
/// person's titles and the born names, so the two cannot be bounded apart.
fn leaf_words(words: HashMap<usize, String>, leaves: usize) -> HashMap<usize, String> {
    words
        .into_iter()
        .filter(|(at, _)| *at < leaves)
        .map(|(at, said)| (at, said.trim().to_string()))
        .filter(|(_, said)| !said.is_empty())
        .collect()
}

impl TabLayout {
    /// The layout as the file will keep it, or `None` for one not worth
    /// keeping.
    ///
    /// Everything positional is checked against the tree it points into: an
    /// active or expanded ordinal past the last leaf is a file somebody
    /// edited, and the safe readings are the first pane and "not expanded".
    /// Titles that point nowhere or say nothing are dropped with them, and
    /// so is a wake record that could never wake.
    pub fn normalized(self) -> Option<TabLayout> {
        let leaves = self.root.leaves();
        if leaves == 0 || leaves > MAX_LEAVES {
            return None;
        }
        let titles = leaf_words(self.titles, leaves);
        let names = leaf_words(self.names, leaves);
        let agents: HashMap<usize, WakeAgent> = self
            .agents
            .into_iter()
            .filter(|(at, held)| *at < leaves && held.keeps())
            .collect();
        // A program this window has no launch for is a file somebody
        // edited: the slug is spent on `launch_agent_tab` at wake time, and
        // the catalogue asked here is the one that door asks.
        let running = self
            .running
            .into_iter()
            .filter(|(at, agent)| *at < leaves && zerocode_core::agent_spec(agent).is_some())
            .collect();
        let terms = self
            .terms
            .into_iter()
            .filter(|(at, _)| *at < leaves)
            .collect();
        // A buffer past the capture's own cap is a file somebody edited —
        // the capture cannot write one — and feeding it whole to a fresh
        // terminal spends memory on trusting it.
        let buffers = self
            .buffers
            .into_iter()
            .filter(|(at, held)| {
                *at < leaves
                    && !held.is_empty()
                    && held.len() <= zerocode_pty::SCROLLBACK_BUFFER_BYTE_LIMIT
            })
            .collect();
        // Owed conversations that could be named again, once each, the
        // newest kept — and never one a leaf of this tab now holds.
        let mut owed: Vec<WakeAgent> = Vec::new();
        for held in self.owed.into_iter().rev() {
            let named = |one: &WakeAgent| one.agent == held.agent && one.id == held.id;
            if held.keeps() && !owed.iter().any(named) && !agents.values().any(named) {
                owed.push(held);
            }
        }
        owed.truncate(MAX_OWED);
        owed.reverse();
        Some(TabLayout {
            id: self.id,
            root: self.root.normalized(),
            titles,
            names,
            agents,
            running,
            active: if self.active < leaves { self.active } else { 0 },
            expanded: self.expanded.filter(|at| *at < leaves),
            pinned: self.pinned,
            focused: self.focused,
            terms,
            buffers,
            owed,
            awake: self.awake,
        })
    }
}

impl TabLayout {
    /// This tab with every pane name that names nothing replaced (t-11540).
    ///
    /// A name a pane was stored under is often the prompt that was last
    /// typed into it, and the prompt a long conversation last got is `계속`:
    /// the coordinator's pane came back from each restart named 「계속」 on
    /// the board, the sidebar and the relation map, and a stored name outranks
    /// every later prompt. So a title or born name that fails
    /// [`zerocode_core::transcript::names_the_turn`] is replaced by the newest
    /// prompt that passes in the conversation asleep in that leaf, or dropped
    /// when there is none — the card then says the workspace, which is better
    /// than a word that says go on. Asked where the window restores a tab, not
    /// in [`TabLayout::normalized`]: this one reads a file.
    #[must_use]
    pub fn named_by_their_turns(mut self) -> TabLayout {
        let agents = &self.agents;
        let rename = |words: &mut HashMap<usize, String>| {
            words.retain(|at, said| {
                if zerocode_core::transcript::names_the_turn(said) {
                    return true;
                }
                let named = agents
                    .get(at)
                    .and_then(|held| held.transcript_path.as_deref())
                    .and_then(|path| {
                        zerocode_core::transcript::last_naming_prompt(Path::new(path))
                    });
                let Some(named) = named else {
                    return false;
                };
                *said = named;
                true
            });
        };
        rename(&mut self.titles);
        rename(&mut self.names);
        self
    }
}

/// Every worktree's terminal tabs, by the worktree's absolute path.
pub type Layouts = HashMap<String, Vec<TabLayout>>;

/// What the file holds, or nothing — a missing or unreadable file is a
/// window that has never been closed, not an error.
pub fn read(file: &Path) -> Layouts {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Layouts::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// The one writer of the layouts file (t-7812 D).
///
/// The file holds every worktree's set, and each save reads it whole,
/// changes one part and writes it whole. Two saves at once — the restore of
/// one workspace persisting while another's tab closes — each read the same
/// file and the second write erased the first one's change; and a write cut
/// short left half a file for the next boot. So every change goes through
/// here: one lock for the process, the change asked of the file as it is
/// under that lock, and the result put in place by one durable replace.
/// `change` answers whether it changed anything; nothing is written when
/// it did not. Answers whether the file was replaced (t-76489): a change
/// that wrote nothing is not one the file holds.
pub(crate) fn rewrite(
    file: &Path,
    change: impl FnOnce(&mut Layouts) -> bool,
) -> std::io::Result<bool> {
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _writing = WRITING.lock().unwrap_or_else(|held| held.into_inner());
    let mut held = read(file);
    if !change(&mut held) {
        return Ok(false);
    }
    let text = serde_json::to_vec(&held).map_err(std::io::Error::other)?;
    crate::durable_file::replace_bytes(file, &text).map(|_| true)
}

/// The window's own save of one worktree's set, through [`rewrite`] — or
/// nothing, once the window is leaving (t-7812 D).
///
/// From the first exit signal the file is the list the next window restores.
/// What the window saves after it describes the window dying: a shell that
/// ended on the way out, and the tab the window pruned around it. Asked
/// under the writer's own lock, so a save cannot land between the exit's own
/// snapshot and the process ending. Then one leaf per conversation across
/// the whole file, under the same lock (t-76489, [`one_wake_each`]): `holds`
/// says which shell holds a conversation now, `None` when it cannot tell.
/// Answers what the save did
/// ([`Saved`]), or `None` when the set was not written.
pub(crate) fn save_window_set(
    file: &Path,
    worktree: String,
    tabs: Vec<TabLayout>,
    leaving: &dyn Fn() -> bool,
    holds: &dyn Fn(&WakeAgent, u32) -> Option<bool>,
) -> std::io::Result<Option<Saved>> {
    let mut saved = None;
    rewrite(file, |held| {
        if leaving() {
            return false;
        }
        let named = store(held, worktree, tabs);
        saved = Some((named, one_wake_each(held, holds)));
        true
    })?;
    Ok(saved)
}

/// What a window's save answers ([`save_window_set`]): the name each tab was
/// stored under ([`store`]), and each conversation it took off a leaf because
/// another leaf names it ([`one_wake_each`]).
pub(crate) type Saved = (Vec<Option<u64>>, Vec<String>);

/// Put one worktree's tabs into `layouts`, normalised, pruning as it goes.
///
/// An empty list removes the key: a worktree whose last terminal tab closed
/// must not resurrect it on the next visit. Entries for worktrees that no
/// longer exist on disk go at the same time — this map is keyed by checkout
/// paths, and checkouts are things people delete.
///
/// Answers the name each tab was stored under, in the order given — `None`
/// for one not worth keeping (t-14036). A tab born in this session reaches
/// here nameless; handed its name back, the window can say where it stands
/// on the stage, and every later save names it the same.
pub fn store(layouts: &mut Layouts, worktree: String, tabs: Vec<TabLayout>) -> Vec<Option<u64>> {
    layouts.retain(|path, _| Path::new(path).is_dir());
    let held = layouts.get(&worktree).cloned().unwrap_or_default();
    let mut next = held
        .iter()
        .chain(tabs.iter())
        .filter_map(|tab| tab.id)
        .max()
        .unwrap_or(0)
        + 1;
    let mut looked_at = false;
    let mut kept: Vec<TabLayout> = Vec::new();
    let mut named: Vec<Option<u64>> = Vec::new();
    for tab in tabs {
        let Some(mut tab) = tab.normalized() else {
            named.push(None);
            continue;
        };
        // A tab that comes back without its screens keeps the ones the file
        // already holds for it. This is the whole reason the ids exist: a
        // restored tab nobody has looked at yet has no shells to serialise,
        // so the window can only echo the record — and without this line the
        // very first save of the session would wipe the last screen of every
        // tab the person had not clicked on yet.
        if tab.buffers.is_empty()
            && let Some(before) = tab
                .id
                .and_then(|id| held.iter().find(|one| one.id == Some(id)))
        {
            tab.buffers.clone_from(&before.buffers);
        }
        if tab.id.is_none() {
            tab.id = Some(next);
            next += 1;
        }
        // One tab was being looked at. Two is a file somebody edited, and the
        // reading that costs least is the first — the restore spawns exactly
        // the tab this marks and lets the rest sleep.
        if tab.focused {
            tab.focused = !looked_at;
            looked_at = true;
        }
        named.push(tab.id);
        kept.push(tab);
    }
    if kept.is_empty() {
        layouts.remove(&worktree);
    } else {
        layouts.insert(worktree, kept);
    }
    named
}

/// The exit's screens, and its word on which tabs were standing (t-14036).
///
/// Every leaf the session named by its pty id is asked for its screen, and
/// the ids are spent — they die with this process, and a number left in the
/// file would point the NEXT session's capture at somebody else's shell.
/// `screen` answers `None` for a terminal that is already gone. A tab is
/// `awake` when any of its leaves still had a terminal to ask; a tab this
/// window never woke names no ids, keeps the screens it already had, and is
/// not awake.
pub(crate) fn capture_exit(layouts: &mut Layouts, screen: impl Fn(u32) -> Option<String>) {
    for tabs in layouts.values_mut() {
        for layout in tabs.iter_mut() {
            let mut standing = false;
            for (ordinal, term) in std::mem::take(&mut layout.terms) {
                let Some(written) = screen(term) else {
                    continue;
                };
                standing = true;
                if !written.is_empty() {
                    layout.buffers.insert(ordinal, written);
                }
            }
            layout.awake = standing;
        }
    }
}

/// The workspaces the next boot puts back behind the one in front: those
/// whose sets hold a tab the exit found standing with a conversation or an
/// agent in it (t-14036). In path order, so two boots walk them alike; a
/// checkout gone from the disk is not one.
pub(crate) fn standing_worktrees(layouts: &Layouts) -> Vec<String> {
    let mut standing: Vec<String> = layouts
        .iter()
        .filter(|(path, tabs)| {
            Path::new(path).is_dir()
                && tabs
                    .iter()
                    .any(|tab| tab.awake && (!tab.agents.is_empty() || !tab.running.is_empty()))
        })
        .map(|(path, _)| path.clone())
        .collect();
    standing.sort();
    standing
}

/// What [`without_finished`] left of a set of tabs (t-57717).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Left {
    /// Every tab in the order it was given — `None` for one whose every leaf
    /// held a struck conversation, which is not worth keeping.
    pub(crate) tabs: Vec<Option<TabLayout>>,
    /// The worker each struck conversation was, in the order struck.
    pub(crate) struck: Vec<String>,
}

impl Left {
    /// The tabs kept, in order.
    pub(crate) fn kept(self) -> Vec<TabLayout> {
        self.tabs.into_iter().flatten().collect()
    }
}

/// A set of tabs without the conversations `gone` names (t-57717): each leaf
/// whose wake it answers a worker for keeps its place in the tree and comes
/// back a plain shell — its wake and the program it ran both go — and a tab
/// whose every leaf was one is not kept at all, so a workspace whose stored
/// set held nothing else opens as one that stored nothing. A person's
/// conversation and a leaf that held no conversation stay as they were.
///
/// The ledger's half is `gone`: the read roads strike a finished worker's
/// conversation, the window's own save strikes any worker's. On 2026-10-15
/// and 16 three finished workers' conversations came back off this file as
/// agent panes nobody had called, each on the first visit to its checkout.
pub(crate) fn without_finished(
    tabs: Vec<TabLayout>,
    gone: impl Fn(&WakeAgent) -> Option<String>,
) -> Left {
    let mut struck = Vec::new();
    let tabs = tabs
        .into_iter()
        .map(|mut tab| {
            let leaves = tab.root.leaves();
            let mut ordinals: Vec<usize> = tab.agents.keys().copied().collect();
            ordinals.sort_unstable();
            let mut leaves_struck = 0;
            for at in ordinals {
                let Some(worker) = tab.agents.get(&at).and_then(&gone) else {
                    continue;
                };
                tab.agents.remove(&at);
                tab.running.remove(&at);
                struck.push(worker);
                if at < leaves {
                    leaves_struck += 1;
                }
            }
            (leaves_struck < leaves).then_some(tab)
        })
        .collect();
    Left { tabs, struck }
}

/// A pane the window found running a conversation as it went: its shell,
/// the workspace its agent last reported from — empty when it named none —
/// the conversation, and whether its agent is known to hold its terminal
/// (`sure`) or the window could not tell (t-76489 r6).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LivePane {
    pub(crate) term: u32,
    pub(crate) worktree: String,
    pub(crate) wake: WakeAgent,
    pub(crate) sure: bool,
}

/// Write down every live pane the stored sets do not name, each as a tab of
/// its own in the set of the workspace it reported from (t-12063). Answers
/// what it did, pane by pane, in the order given.
///
/// The webview's saves are the list the next window restores, and on
/// 2026-09-28 one of them had no line for a zo pane that was open and whose
/// conversation the backend knew by id: the restart never tried it. On
/// 2026-10-26 one had no line for a run's coordinator — a claude pane in the
/// tab beside the zo it had split — and the next window brought back the zo
/// alone (t-76489). The exit runs this inside its own write, before the
/// screens are captured, so a tab it writes carries its last screen like any
/// other.
///
/// Every pane in `live` holds its conversation now, so a leaf names it for
/// this pane only when the leaf's shell is this pane's. Any other leaf naming
/// it stands for a pane that does not hold it — one gone, a shell its agent
/// was quit back to, a tab this window never woke — and is not this pane's
/// record: a dead pane's leaf alone used to answer for the living pane, and
/// with no screen to capture its tab was stored asleep and never woke. So:
/// a leaf holding the pane's shell while naming another conversation names
/// this one now, and owes the one it named, never dropping it; else a leaf
/// naming it in the pane's workspace from a pane not `standing` now stands
/// for this pane and keeps its place; else, for a pane that named no
/// workspace, a tab this window never woke in another workspace keeps it,
/// as before, and is said to be all that does ([`Filed::Asleep`],
/// t-77269); else the pane is written as a tab of its own. The other
/// leaves naming it come back plain shells — one conversation, one wake.
///
/// A tab this window never woke, in another workspace, is a record nobody
/// here can confirm, and the pane is one whose hold on the conversation is
/// confirmed now. A pane that named its workspace is written there
/// whatever such a record says, of any agent, and the record comes back a
/// plain shell on every road: on 2026-10-26 the record of a workspace not
/// opened in front answered for a claude coordinator running in another,
/// and that one was stored nowhere and said nowhere lost (t-76489 r3). zo
/// is no exception (r4): it opens a conversation from the folder its
/// sessions live in, and the folder it runs it from now is one — or a
/// shared `ZO_SESSION_ROOT` opens it from any. Only between two records
/// with no pane confirming either does nobody here know which folder zo
/// opens it from, and [`one_wake_each`] keeps both.
///
/// All of that is for a pane known to hold its conversation (`sure`): the
/// conversation stays on its leaf, so no other leaf's copy is lost. A pane
/// the window could not tell of only adds (t-76489 r6): on its own empty
/// leaf, else as a tab of its own in its workspace when no leaf there
/// names it from a pane still standing (r7), and it takes no record off
/// any leaf and puts none over another conversation. In r5 such a pane
/// took a pane known to hold the same conversation off its leaf, as a
/// record of a pane gone.
pub(crate) fn file_unheld(
    layouts: &mut Layouts,
    live: Vec<LivePane>,
    standing: impl Fn(u32) -> bool,
) -> Vec<Filed> {
    let mut filed = Vec::new();
    for pane in live {
        if !pane.wake.keeps() || named_at(layouts, &pane.wake, pane.term) {
            filed.push(Filed::Named);
            continue;
        }
        if !pane.sure {
            filed.push(add_unconfirmed(layouts, pane, &standing));
            continue;
        }
        let (afar, stale): (Vec<_>, Vec<_>) = leaves_naming(layouts, &pane.wake)
            .into_iter()
            .partition(|(path, _, _, term)| term.is_none() && *path != pane.worktree);
        // The records afar that give way to this pane: none for a pane
        // placed nowhere; every one otherwise.
        let placed = !pane.worktree.is_empty();
        let yielded: &[_] = if placed { &afar } else { &[] };
        let holder = layouts.values().flatten().find_map(|tab| {
            let at = tab
                .terms
                .iter()
                .find_map(|(at, term)| (*term == pane.term).then_some(*at))?;
            Some((tab.agents.get(&at).cloned(), at))
        });
        if let Some((before, at)) = holder {
            // What the leaf named is owed unless another leaf names it: a
            // conversation standing elsewhere is not one this tab lost.
            let owed = before.filter(|before| {
                before.keeps() && !names_anywhere(layouts, before, Some(pane.term))
            });
            strike(layouts, &stale);
            strike(layouts, yielded);
            if let Some(tab) = layouts
                .values_mut()
                .flatten()
                .find(|tab| tab.terms.values().any(|term| *term == pane.term))
            {
                tab.agents.insert(at, pane.wake.clone());
                tab.running.insert(at, pane.wake.agent.clone());
                tab.owed.extend(owed);
            }
            filed.push(Filed::Renamed);
            continue;
        }
        let relinked = stale.iter().position(|(path, _, _, term)| {
            !term.is_some_and(&standing)
                && (*path == pane.worktree || (pane.worktree.is_empty() && term.is_some()))
        });
        if let Some(place) = relinked {
            let (path, index, at, _) = &stale[place];
            if let Some(tab) = layouts.get_mut(path).and_then(|tabs| tabs.get_mut(*index)) {
                tab.terms.insert(*at, pane.term);
                tab.running.insert(*at, pane.wake.agent.clone());
            }
            let mut others = stale;
            others.remove(place);
            strike(layouts, &others);
            strike(layouts, yielded);
            filed.push(Filed::Relinked);
            continue;
        }
        if !afar.is_empty() && !placed {
            filed.push(Filed::Asleep);
            continue;
        }
        if pane.worktree.is_empty() {
            filed.push(Filed::Unplaced);
            continue;
        }
        strike(layouts, &stale);
        strike(layouts, yielded);
        filed.push(Filed::Tab(tab_of_its_own(layouts, pane)));
    }
    filed
}

/// [`file_unheld`] for a pane the window could not tell holds its
/// conversation (t-76489 r6): it adds the conversation where it is
/// missing and takes nothing off any leaf. A leaf naming it in the pane's
/// workspace — or, for a pane that named none, anywhere — from a pane
/// still `standing` stands for it as it is; else its own leaf names it
/// when that leaf names nothing; else it is a tab of its own in its
/// workspace, beside whatever else names it. A conversation two leaves
/// name wakes in one of them, and the other comes back a plain shell
/// (`spawnStoredLeaf`): two leaves lose nothing, and taking one off on a
/// guess loses the pane that held it.
///
/// A leaf whose pane is gone, or that names no pane, is a record no screen
/// is captured for, and it does not stand for this pane (t-76489 r7): in
/// r6 such a record's name alone answered, nothing pointed at the living
/// pane, its tab was stored asleep, and a workspace not in front never
/// came back. For a pane that named no workspace, with no empty leaf of its
/// own, a dead pane's leaf is tied to it — the leaf keeps its conversation
/// and its place — and a record of no pane, which nothing here confirms,
/// keeps it as before ([`Filed::Asleep`], t-77269).
fn add_unconfirmed(layouts: &mut Layouts, pane: LivePane, standing: impl Fn(u32) -> bool) -> Filed {
    let placed = !pane.worktree.is_empty();
    let naming: Vec<_> = leaves_naming(layouts, &pane.wake)
        .into_iter()
        .filter(|(path, ..)| !placed || *path == pane.worktree)
        .collect();
    if naming.iter().any(|(.., term)| term.is_some_and(&standing)) {
        return Filed::Named;
    }
    let empty_own = layouts.values_mut().flatten().find_map(|tab| {
        let at = tab
            .terms
            .iter()
            .find_map(|(at, term)| (*term == pane.term).then_some(*at))?;
        (!tab.agents.contains_key(&at)).then_some((tab, at))
    });
    if let Some((tab, at)) = empty_own {
        tab.running.insert(at, pane.wake.agent.clone());
        tab.agents.insert(at, pane.wake);
        return Filed::Renamed;
    }
    if !placed {
        let dead = naming.iter().find(|(.., term)| term.is_some());
        if let Some((path, index, at, _)) = dead
            && let Some(tab) = layouts.get_mut(path).and_then(|tabs| tabs.get_mut(*index))
        {
            tab.terms.insert(*at, pane.term);
            tab.running.insert(*at, pane.wake.agent.clone());
            return Filed::Relinked;
        }
        if !naming.is_empty() {
            return Filed::Asleep;
        }
        return Filed::Unplaced;
    }
    Filed::Tab(tab_of_its_own(layouts, pane))
}

/// Write `pane` as a tab of its own at the end of its workspace's set,
/// never in front, its shell kept so the exit captures its screen; answers
/// the tab's name.
fn tab_of_its_own(layouts: &mut Layouts, pane: LivePane) -> u64 {
    let tabs = layouts.entry(pane.worktree).or_default();
    let id = tabs.iter().filter_map(|tab| tab.id).max().unwrap_or(0) + 1;
    let agent = pane.wake.agent.clone();
    tabs.push(TabLayout {
        id: Some(id),
        root: PaneNode::Leaf,
        titles: HashMap::new(),
        names: HashMap::new(),
        agents: HashMap::from([(0, pane.wake)]),
        running: HashMap::from([(0, agent)]),
        active: 0,
        expanded: None,
        pinned: false,
        focused: false,
        terms: HashMap::from([(0, pane.term)]),
        buffers: HashMap::new(),
        owed: Vec::new(),
        awake: false,
    });
    id
}

/// Whether a leaf of `layouts` names `wake`'s conversation — any but the one
/// whose shell is `besides`.
fn names_anywhere(layouts: &Layouts, wake: &WakeAgent, besides: Option<u32>) -> bool {
    layouts.values().flatten().any(|tab| {
        tab.agents.iter().any(|(at, one)| {
            one.agent == wake.agent
                && one.id == wake.id
                && (besides.is_none() || tab.terms.get(at).copied() != besides)
        })
    })
}

/// Whether the leaf whose shell is `term` names `wake`'s conversation.
fn named_at(layouts: &Layouts, wake: &WakeAgent, term: u32) -> bool {
    layouts.values().flatten().any(|tab| {
        tab.agents.iter().any(|(at, one)| {
            one.agent == wake.agent && one.id == wake.id && tab.terms.get(at) == Some(&term)
        })
    })
}

/// Every leaf naming `wake`'s conversation — its workspace, tab, ordinal and
/// the shell it last stood for — by workspace path, tab and ordinal, so two
/// exits read them alike.
fn leaves_naming(layouts: &Layouts, wake: &WakeAgent) -> Vec<(String, usize, usize, Option<u32>)> {
    let mut paths: Vec<&String> = layouts.keys().collect();
    paths.sort();
    let mut naming = Vec::new();
    for path in paths {
        for (index, tab) in layouts[path].iter().enumerate() {
            let mut ordinals: Vec<usize> = tab
                .agents
                .iter()
                .filter(|(_, one)| one.agent == wake.agent && one.id == wake.id)
                .map(|(at, _)| *at)
                .collect();
            ordinals.sort_unstable();
            for at in ordinals {
                naming.push((path.clone(), index, at, tab.terms.get(&at).copied()));
            }
        }
    }
    naming
}

/// Take the conversation off each leaf `leaves` lists: it keeps its place
/// as a plain shell, its wake and its program gone.
fn strike(layouts: &mut Layouts, leaves: &[(String, usize, usize, Option<u32>)]) {
    for (path, index, at, _) in leaves {
        if let Some(tab) = layouts.get_mut(path).and_then(|tabs| tabs.get_mut(*index)) {
            tab.agents.remove(at);
            tab.running.remove(at);
        }
    }
}

/// What [`file_unheld`] did with one live pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filed {
    /// The leaf holding its shell already names its conversation, or it
    /// names none that could wake: nothing written. For a pane the window
    /// could not tell of, also: a leaf of its workspace names it from a pane
    /// still standing.
    Named,
    /// The pane named no workspace, no leaf holds its shell, and only
    /// records of no pane name its conversation — a tab this window never
    /// woke (t-77269). The records keep it as they are, for nobody here can
    /// tell which folder it belongs in, but no screen ties them to the pane:
    /// the exit stores their tab asleep, and the next window brings it back
    /// only when that tab's workspace is opened. Nothing written, and said.
    Asleep,
    /// The stored leaf holding its shell now names its conversation — for
    /// a pane the window could not tell of, only a leaf that named nothing.
    Renamed,
    /// The leaf that named its conversation from a pane that is gone now
    /// stands for this pane, in its place — for a pane the window could
    /// not tell of, only one that named no workspace and had no empty leaf.
    Relinked,
    /// No leaf names it or holds its shell, and it named no workspace to be
    /// written into (an empty one): nothing written.
    Unplaced,
    /// Written as a tab of its own, under this name.
    Tab(u64),
}

/// One leaf per conversation across every stored set (t-76489), where a
/// pane is known to hold it: a conversation two leaves name keeps the leaf
/// whose shell `holds` it now, and every other leaf naming it keeps its
/// place as a plain shell, its wake and its program gone. Answers each
/// conversation it took off a leaf, once per leaf.
///
/// With no leaf whose pane is known to hold it, every leaf naming it stays
/// (t-76489 r6). `holds` answers `None` for a shell it cannot tell of — a
/// pane busy past its patience, a launcher whose group was not read whole
/// — and its `Some(false)` is a reading of process names, which a road
/// that failed without saying so has turned into a pane holding nothing
/// (r5's review: a ConPTY's Claude whose image would not be read, a group
/// list libproc failed with 0). A record taken off on such a word is a
/// conversation lost; two leaves naming one conversation lose nothing, for
/// it wakes in one and the other comes back a plain shell
/// (`spawnStoredLeaf`). Before r6, with every leaf known to hold nothing
/// the first by order in one workspace kept it.
///
/// On 2026-10-26 23:03 a coordinator split `zo --resume` of a conversation a
/// person's tab beside it already held; the split died at zo's writer lease.
/// A record naming one conversation twice is what wakes it twice: the second
/// resume meets the first's lease, and the leaf that stood for the living
/// pane can be the one that loses. The leaf that keeps it is the one whose
/// pane holds it, so a dead pane's copy never takes the living one's place.
pub(crate) fn one_wake_each(
    layouts: &mut Layouts,
    holds: impl Fn(&WakeAgent, u32) -> Option<bool>,
) -> Vec<String> {
    let mut paths: Vec<String> = layouts.keys().cloned().collect();
    paths.sort();
    // Every leaf naming a conversation, in the order the rule reads them:
    // the conversation, and the leaf — its workspace, tab and ordinal, and
    // whether its shell holds the conversation now (`None`: cannot tell). A
    // leaf with no pane holds nothing. Asked only of a conversation two
    // leaves name: the asking reads each pane's terminal.
    type Leaf = (String, usize, usize, Option<bool>);
    type Place = (String, usize, usize);
    let mut named: Vec<((String, String), Place)> = Vec::new();
    for path in &paths {
        for (index, tab) in layouts[path].iter().enumerate() {
            let mut ordinals: Vec<usize> = tab.agents.keys().copied().collect();
            ordinals.sort_unstable();
            for at in ordinals {
                let wake = &tab.agents[&at];
                named.push((
                    (wake.agent.clone(), wake.id.clone()),
                    (path.clone(), index, at),
                ));
            }
        }
    }
    let naming: Vec<((String, String), Leaf)> = named
        .iter()
        .map(|(conversation, (path, index, at))| {
            let twice = named
                .iter()
                .filter(|(other, _)| other == conversation)
                .count()
                > 1;
            let tab = &layouts[path][*index];
            let held = if twice {
                tab.terms
                    .get(at)
                    .map_or(Some(false), |term| holds(&tab.agents[at], *term))
            } else {
                Some(false)
            };
            (conversation.clone(), (path.clone(), *index, *at, held))
        })
        .collect();
    let mut struck = Vec::new();
    let mut seen: Vec<&(String, String)> = Vec::new();
    for (conversation, _) in &naming {
        if seen.contains(&conversation) {
            continue;
        }
        seen.push(conversation);
        let leaves: Vec<&Leaf> = naming
            .iter()
            .filter(|(other, _)| other == conversation)
            .map(|(_, leaf)| leaf)
            .collect();
        if leaves.len() < 2 {
            continue;
        }
        // The leaf whose pane holds it; with none, every leaf stays and the
        // boot's one-process judge decides which one wakes it.
        let Some(keeper) = leaves.iter().position(|(.., held)| *held == Some(true)) else {
            continue;
        };
        for (place, (path, index, at, _)) in leaves.into_iter().enumerate() {
            if place == keeper {
                continue;
            }
            let Some(tab) = layouts.get_mut(path).and_then(|tabs| tabs.get_mut(*index)) else {
                continue;
            };
            tab.agents.remove(at);
            tab.running.remove(at);
            struck.push(format!(
                "{} {}",
                conversation.0,
                crate::restart_nudge_runtime::session_prefix(&conversation.1)
            ));
        }
    }
    struck
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(ratio: Option<f64>) -> PaneNode {
        PaneNode::Split {
            direction: SplitDirection::Vertical,
            first: Box::new(PaneNode::Leaf),
            second: Box::new(PaneNode::Leaf),
            ratio,
        }
    }

    fn ratio_of(node: &PaneNode) -> Option<f64> {
        match node {
            PaneNode::Split { ratio, .. } => *ratio,
            PaneNode::Leaf => None,
        }
    }

    /// A tab of nothing but its tree. Every test below sets the two or three
    /// fields it is actually about on top of this, so the next field this
    /// record grows is not spelled out in nine places that do not care.
    fn bare(root: PaneNode) -> TabLayout {
        TabLayout {
            id: None,
            root,
            titles: HashMap::new(),
            names: HashMap::new(),
            agents: HashMap::new(),
            running: HashMap::new(),
            active: 0,
            expanded: None,
            pinned: false,
            focused: false,
            terms: HashMap::new(),
            buffers: HashMap::new(),
            owed: Vec::new(),
            awake: false,
        }
    }

    /// The two measured rules: within 5e-3 of even the number is omitted,
    /// outside it three decimals survive (I18nProvider-4EBrmTGg.js:46127-46128).
    #[test]
    fn an_even_split_stores_no_ratio_and_an_uneven_one_stores_three_decimals() {
        assert_eq!(ratio_of(&split(Some(0.5)).normalized()), None);
        assert_eq!(ratio_of(&split(Some(0.503)).normalized()), None);
        assert_eq!(ratio_of(&split(Some(0.497)).normalized()), None);
        assert_eq!(ratio_of(&split(Some(0.5051)).normalized()), Some(0.505));
        assert_eq!(
            ratio_of(&split(Some(0.700449)).normalized()),
            Some(0.7),
            "three decimals, not the flexbox arithmetic's fifteen"
        );
    }

    /// A number that never described a division reads as even, whatever it
    /// says — NaN and out-of-range both come from a file somebody edited.
    #[test]
    fn a_ratio_that_is_not_a_proportion_reads_as_even() {
        assert_eq!(ratio_of(&split(Some(f64::NAN)).normalized()), None);
        assert_eq!(ratio_of(&split(Some(0.0)).normalized()), None);
        assert_eq!(ratio_of(&split(Some(1.0)).normalized()), None);
        assert_eq!(ratio_of(&split(Some(-3.0)).normalized()), None);
        assert_eq!(ratio_of(&split(Some(f64::INFINITY)).normalized()), None);
    }

    /// Ordinals are checked against the tree they point into, and titles that
    /// point nowhere or say nothing go with them.
    #[test]
    fn ordinals_past_the_last_leaf_fall_back_and_empty_titles_drop() {
        let layout = TabLayout {
            titles: HashMap::from([
                (0, "  build  ".to_string()),
                (1, "   ".to_string()),
                (7, "ghost".to_string()),
            ]),
            active: 9,
            expanded: Some(4),
            ..bare(split(None))
        }
        .normalized()
        .expect("a two-leaf tab keeps");
        assert_eq!(layout.titles, HashMap::from([(0, "build".to_string())]));
        assert_eq!(layout.active, 0, "an active pane past the end is the first");
        assert_eq!(
            layout.expanded, None,
            "an expanded pane past the end is none"
        );
    }

    /// A stored name that only says go on is replaced by the newest request
    /// in the conversation asleep in its leaf, or dropped when that leaf has
    /// none to read; a name that names something is left alone (t-11540).
    #[test]
    fn a_stored_go_on_name_is_renamed_by_its_conversation_or_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript = dir.path().join("session.jsonl");
        let line = |content: &str| {
            serde_json::json!({ "type": "user", "message": { "role": "user", "content": content } })
                .to_string()
        };
        std::fs::write(
            &transcript,
            format!("{}\n{}\n", line("land the board fix"), line("계속")),
        )
        .expect("write");
        let wake = WakeAgent {
            agent: "claude".to_string(),
            key: "session_id".to_string(),
            id: "7f0c2e7e-3a52-4d2b-9d0e-2f3b2c1d0a9e".to_string(),
            transcript_path: Some(transcript.to_string_lossy().into_owned()),
            interrupted: false,
        };
        let layout = TabLayout {
            titles: HashMap::from([(0, "계속".to_string()), (1, "ㄱㄱ".to_string())]),
            names: HashMap::from([(0, "계속".to_string()), (1, "ZO".to_string())]),
            agents: HashMap::from([(0, wake)]),
            ..bare(split(None))
        }
        .named_by_their_turns();
        assert_eq!(
            layout.titles,
            HashMap::from([(0, "land the board fix".to_string())]),
            "a leaf with no conversation to read loses the word"
        );
        assert_eq!(
            layout.names,
            HashMap::from([(0, "land the board fix".to_string()), (1, "ZO".to_string())])
        );
    }

    /// The names panes were born with are bounded the same way as titles,
    /// and stay apart from them: a record that says both keeps both.
    #[test]
    fn born_names_are_bounded_like_titles_and_kept_apart_from_them() {
        let layout = TabLayout {
            titles: HashMap::from([(0, "빌드 감시".to_string())]),
            names: HashMap::from([
                (0, "Claude".to_string()),
                (1, "  ZO  ".to_string()),
                (2, "ghost".to_string()),
                (3, "   ".to_string()),
            ]),
            ..bare(split(None))
        }
        .normalized()
        .expect("a two-leaf tab keeps");
        assert_eq!(layout.titles, HashMap::from([(0, "빌드 감시".to_string())]));
        assert_eq!(
            layout.names,
            HashMap::from([(0, "Claude".to_string()), (1, "ZO".to_string())])
        );
        let json = serde_json::to_string(&layout).expect("serializes");
        assert!(
            json.contains("\"names\":{") && json.contains("\"titles\":{"),
            "both maps reach the file:\n{json}"
        );
        let bare_json = serde_json::to_string(&bare(split(None))).expect("serializes");
        assert!(
            !bare_json.contains("names"),
            "a tab with no born names stores no object for them:\n{bare_json}"
        );
    }

    /// A tree wider than any layout a person can read is refused whole — it
    /// would be that many shells spawned before the first frame.
    #[test]
    fn a_tree_beyond_the_leaf_cap_is_refused_whole() {
        let mut root = PaneNode::Leaf;
        for _ in 0..MAX_LEAVES {
            root = PaneNode::Split {
                direction: SplitDirection::Horizontal,
                first: Box::new(root),
                second: Box::new(PaneNode::Leaf),
                ratio: None,
            };
        }
        assert_eq!(bare(root).normalized(), None);
    }

    /// A wake record that could never wake disappears from the file — an
    /// unknown key spelling, an id that would read as a flag, an ordinal
    /// pointing past the tree. What stays is exactly what `resume_session`
    /// can be handed.
    #[test]
    fn a_wake_record_that_could_never_wake_is_dropped() {
        let wake = |key: &str, id: &str| WakeAgent {
            interrupted: false,
            agent: "claude".to_string(),
            key: key.to_string(),
            id: id.to_string(),
            transcript_path: None,
        };
        let layout = TabLayout {
            agents: HashMap::from([
                (0, wake("session_id", "abc-123")),
                (1, wake("api_key", "abc-456")),
                (7, wake("session_id", "past-the-tree")),
            ]),
            ..bare(split(None))
        }
        .normalized()
        .expect("a two-leaf tab keeps");
        assert_eq!(
            layout.agents.keys().collect::<Vec<_>>(),
            vec![&0],
            "the unknown key spelling and the out-of-tree ordinal survived"
        );

        // An id that starts with a dash reaches an argv as a FLAG — the
        // shared validator refuses it here, before it can sit in the file.
        let flagged = TabLayout {
            agents: HashMap::from([(0, wake("session_id", "--resume"))]),
            ..bare(split(None))
        }
        .normalized()
        .expect("keeps");
        assert!(flagged.agents.is_empty(), "a flag-shaped id was stored");
    }

    /// The program a leaf was running is kept for agents this window can
    /// actually start, and for leaves that exist.
    ///
    /// Both halves are spent at wake time: the slug goes to
    /// `launch_agent_tab`, which asks this same catalogue, and the ordinal
    /// indexes the tree the mount walks. A slug nothing knows would open an
    /// error where a pane belongs, which is worse than the plain shell it
    /// replaced.
    #[test]
    fn a_running_program_is_kept_only_where_it_could_be_started_again() {
        let layout = TabLayout {
            running: HashMap::from([
                (0, "zo".to_string()),
                (1, "not-an-agent".to_string()),
                (7, "claude".to_string()),
            ]),
            ..bare(split(None))
        }
        .normalized()
        .expect("a two-leaf tab keeps");
        assert_eq!(
            layout.running,
            HashMap::from([(0, "zo".to_string())]),
            "an agent nothing can launch, or one past the tree, survived"
        );
    }

    /// The stored spelling is Orca's own — `type`/`direction` tagged, ratio
    /// only when it says something — so the file reads as what it copies.
    #[test]
    fn the_stored_shape_is_orcas_spelling() {
        let json = serde_json::to_string(&TabLayout {
            active: 1,
            ..bare(split(Some(0.7)))
        })
        .expect("serialises");
        assert_eq!(
            json,
            r#"{"root":{"type":"split","direction":"vertical","first":{"type":"leaf"},"second":{"type":"leaf"},"ratio":0.7},"active":1}"#
        );
        let back: TabLayout = serde_json::from_str(&json).expect("parses");
        assert_eq!(back.root, split(Some(0.7)));
    }

    /// Closing the last tab removes the worktree's entry — a layout that
    /// resurrected closed tabs on the next visit would be the window arguing
    /// with the person who closed them.
    #[test]
    fn an_empty_save_removes_the_worktree_and_dead_checkouts_go_with_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let alive = dir.path().to_string_lossy().into_owned();
        let mut layouts = Layouts::from([(
            "/checkout/that/no/longer/exists".to_string(),
            vec![bare(PaneNode::Leaf)],
        )]);
        store(&mut layouts, alive.clone(), vec![bare(PaneNode::Leaf)]);
        assert_eq!(layouts.len(), 1, "the dead checkout's entry went");
        assert!(layouts.contains_key(&alive));
        store(&mut layouts, alive.clone(), Vec::new());
        assert!(
            !layouts.contains_key(&alive),
            "an empty save removes the key"
        );
    }

    /// A tab the window has not taken up yet comes back with no screens in
    /// it — it has no shells to serialise — and the file keeps the ones it
    /// already had for that tab. Without this the first save of a session
    /// would blank the last screen of every tab nobody had clicked on.
    #[test]
    fn a_tab_that_comes_back_without_its_screens_keeps_the_ones_on_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let here = dir.path().to_string_lossy().into_owned();
        let mut layouts = Layouts::new();

        // First sitting: two tabs, both with a screen captured on the way out.
        store(
            &mut layouts,
            here.clone(),
            vec![
                TabLayout {
                    buffers: HashMap::from([(0, "왼쪽 화면".to_string())]),
                    ..bare(PaneNode::Leaf)
                },
                TabLayout {
                    buffers: HashMap::from([(0, "오른쪽 화면".to_string())]),
                    ..bare(PaneNode::Leaf)
                },
            ],
        );
        let named: Vec<u64> = layouts[&here].iter().filter_map(|tab| tab.id).collect();
        assert_eq!(named.len(), 2, "every stored tab is given a name");
        assert_ne!(named[0], named[1], "two tabs share one name");

        // Next sitting: the window echoes both records back — no buffers,
        // because neither tab has been looked at — and closes the first.
        store(
            &mut layouts,
            here.clone(),
            vec![TabLayout {
                id: Some(named[1]),
                ..bare(PaneNode::Leaf)
            }],
        );
        assert_eq!(
            layouts[&here][0].buffers,
            HashMap::from([(0, "오른쪽 화면".to_string())]),
            "the surviving tab's screen followed its name, not its position"
        );

        // And a tab genuinely emptied — a live one whose grid serialised to
        // nothing — is not handed the screen of the tab that had its name.
        store(
            &mut layouts,
            here.clone(),
            vec![TabLayout {
                id: Some(named[1]),
                terms: HashMap::from([(0, 7)]),
                buffers: HashMap::from([(0, "새 화면".to_string())]),
                ..bare(PaneNode::Leaf)
            }],
        );
        assert_eq!(
            layouts[&here][0].buffers,
            HashMap::from([(0, "새 화면".to_string())]),
            "a screen the window actually sent was overwritten by the old one"
        );
    }

    /// One tab was being looked at. A file claiming two is one somebody
    /// edited, and the restore spawns whichever it marks — so the second
    /// claim is dropped rather than waking a second shell nobody asked for.
    #[test]
    fn only_one_tab_of_a_set_can_be_the_one_being_looked_at() {
        let dir = tempfile::tempdir().expect("tempdir");
        let here = dir.path().to_string_lossy().into_owned();
        let mut layouts = Layouts::new();
        let looked = |focused: bool| TabLayout {
            focused,
            ..bare(PaneNode::Leaf)
        };
        store(
            &mut layouts,
            here.clone(),
            vec![looked(false), looked(true), looked(true)],
        );
        assert_eq!(
            layouts[&here]
                .iter()
                .map(|tab| tab.focused)
                .collect::<Vec<_>>(),
            vec![false, true, false]
        );
    }

    /// A record from a build that knew more than this one still parses — the
    /// fields it does not recognise are ignored, and everything it shares is
    /// read. The file is one shape across versions, not a version gate.
    #[test]
    fn a_record_carrying_fields_this_build_does_not_know_still_parses() {
        let held: TabLayout = serde_json::from_str(
            r#"{"id":12,"root":{"type":"leaf"},"active":0,"focused":true,
                "browserTabsByWorktree":{"x":1},"openFiles":["a.rs"]}"#,
        )
        .expect("unknown fields are ignored, not refused");
        assert_eq!(held.id, Some(12));
        assert!(held.focused);
        // And what this build writes reads back as itself.
        let json = serde_json::to_string(&held).expect("serialises");
        assert_eq!(
            serde_json::from_str::<TabLayout>(&json).expect("parses"),
            held
        );
    }

    /// Checkouts that exist, for [`store`]'s pruning to keep.
    fn checkouts(root: &Path, count: usize) -> Vec<String> {
        (0..count)
            .map(|at| {
                let tree = root.join(format!("wt-{at}"));
                std::fs::create_dir(&tree).expect("a checkout");
                tree.to_string_lossy().into_owned()
            })
            .collect()
    }

    /// t-7812 D: one writer for the whole file. Sixteen worktrees saving at
    /// once, twenty-five times each, keep every worktree they wrote — a save
    /// that read the file before another's write and wrote after it used to
    /// erase that one's set.
    #[test]
    fn saves_from_many_threads_keep_every_worktree_they_wrote() {
        let root = tempfile::tempdir().expect("a config root");
        let file = root.path().join("pane-layouts.json");
        let trees = checkouts(root.path(), 16);
        std::thread::scope(|scope| {
            for tree in &trees {
                let file = &file;
                scope.spawn(move || {
                    for _ in 0..25 {
                        let saved = save_window_set(
                            file,
                            tree.clone(),
                            vec![bare(PaneNode::Leaf)],
                            &|| false,
                            &|_, _| Some(false),
                        )
                        .expect("a save");
                        assert!(saved.is_some(), "a window that is not leaving was refused");
                    }
                });
            }
        });
        let held = read(&file);
        assert_eq!(
            held.len(),
            trees.len(),
            "a save erased another worktree's set"
        );
        for tree in &trees {
            assert_eq!(held[tree].len(), 1, "{tree} lost its tab");
        }
    }

    /// t-7812 D: once the window is leaving, what it saves describes the
    /// window dying — a shell ended on the way out, the tab pruned around it
    /// — and the file stays the list the next window restores. The exit's
    /// own snapshot still writes.
    #[test]
    fn a_leaving_window_s_save_leaves_the_restore_list_as_it_stood() {
        let root = tempfile::tempdir().expect("a config root");
        let file = root.path().join("pane-layouts.json");
        let tree = checkouts(root.path(), 1).remove(0);
        assert!(
            save_window_set(
                &file,
                tree.clone(),
                vec![bare(PaneNode::Leaf), bare(split(None))],
                &|| false,
                &|_, _| Some(false),
            )
            .expect("a save")
            .is_some()
        );
        let before = std::fs::read(&file).expect("the list");
        // The last tab's shell ended on the way out; the window saves the
        // set without it.
        assert!(
            save_window_set(&file, tree.clone(), Vec::new(), &|| true, &|_, _| Some(
                false
            ))
            .expect("a refusal")
            .is_none()
        );
        assert_eq!(
            std::fs::read(&file).expect("the list"),
            before,
            "a leaving window rewrote the next window's restore list"
        );
        assert_eq!(read(&file)[&tree].len(), 2);
        // The exit's snapshot is the one write left, through the same door.
        rewrite(&file, |held| {
            held.get_mut(&tree).expect("the set")[0]
                .buffers
                .insert(0, "last screen".to_string());
            true
        })
        .expect("the snapshot");
        assert_eq!(read(&file)[&tree][0].buffers[&0], "last screen");
    }

    fn zo(id: &str) -> WakeAgent {
        WakeAgent {
            agent: "zo".to_string(),
            key: "session_id".to_string(),
            id: id.to_string(),
            transcript_path: None,
            interrupted: false,
        }
    }

    /// A conversation a newer one took the place of stays in the file, once,
    /// the newest few — and never one a leaf of the same tab holds again
    /// (t-12063).
    #[test]
    fn an_owed_conversation_is_kept_once_and_never_one_the_tab_holds() {
        let mut owed: Vec<WakeAgent> = (0..12)
            .map(|at| zo(&format!("session-17900000000{at:02}-0")))
            .collect();
        owed.push(zo("session-1790000000011-0"));
        owed.push(zo("session-1790000000099-0"));
        owed.push(WakeAgent {
            key: "neither".to_string(),
            ..zo("session-1790000000098-0")
        });
        let layout = TabLayout {
            agents: HashMap::from([(0, zo("session-1790000000099-0"))]),
            owed,
            ..bare(PaneNode::Leaf)
        }
        .normalized()
        .expect("a one-leaf tab keeps");
        let kept: Vec<&str> = layout.owed.iter().map(|one| one.id.as_str()).collect();
        assert_eq!(
            kept,
            vec![
                "session-1790000000004-0",
                "session-1790000000005-0",
                "session-1790000000006-0",
                "session-1790000000007-0",
                "session-1790000000008-0",
                "session-1790000000009-0",
                "session-1790000000010-0",
                "session-1790000000011-0",
            ],
            "the newest eight, once each, none the tab holds, none that could never wake"
        );
        let text = serde_json::to_string(&bare(PaneNode::Leaf)).expect("serialise");
        assert!(
            !text.contains("owed"),
            "a tab that owes nothing stores no field: {text}"
        );
    }

    /// The exit writes down a live pane no stored tab holds, as a tab of its
    /// own in its workspace, and leaves alone a pane a tab holds and a
    /// conversation a tab already names (t-12063). A tab this window never
    /// woke in another workspace keeps its conversation for a pane that
    /// named no workspace; a pane that named one is written there, and that
    /// tab comes back a plain shell (t-76489 r4).
    #[test]
    fn a_live_pane_no_tab_holds_is_written_down_in_its_own_workspace() {
        let mut layouts = Layouts::new();
        layouts.insert(
            "/w/a".to_string(),
            vec![TabLayout {
                id: Some(52),
                agents: HashMap::from([(0, zo("session-1790000000001-0"))]),
                terms: HashMap::from([(0, 3)]),
                ..bare(PaneNode::Leaf)
            }],
        );
        layouts.insert(
            "/w/b".to_string(),
            vec![TabLayout {
                id: Some(7),
                agents: HashMap::from([(0, zo("session-1790000000009-0"))]),
                ..bare(PaneNode::Leaf)
            }],
        );
        let live = |term: u32, worktree: &str, id: &str| LivePane {
            term,
            worktree: worktree.to_string(),
            wake: zo(id),
            sure: true,
        };
        let filed = file_unheld(
            &mut layouts,
            vec![
                live(3, "/w/a", "session-1790000000001-0"),
                live(5, "/w/a", "session-1790000000002-0"),
                live(21, "/w/c", "session-1790000000003-0"),
                live(8, "", "session-1790000000009-0"),
                live(9, "/w/a", "--resume"),
                live(10, "/w/a", "session-1790000000009-0"),
            ],
            |_| true,
        );
        assert_eq!(
            filed,
            vec![
                Filed::Named,
                Filed::Tab(53),
                Filed::Tab(1),
                Filed::Asleep,
                Filed::Named,
                Filed::Tab(54),
            ],
            "the panes nobody holds, and only those"
        );
        let a = &layouts["/w/a"];
        assert_eq!(a.len(), 3);
        assert_eq!(a[2].agents[&0].id, "session-1790000000009-0");
        assert_eq!(a[2].terms[&0], 10);
        let written = &a[1];
        assert_eq!(written.id, Some(53), "named past the set's own names");
        assert_eq!(written.agents[&0].id, "session-1790000000002-0");
        assert_eq!(written.running[&0], "zo");
        assert_eq!(written.terms[&0], 5, "the exit's capture finds its screen");
        assert!(!written.focused);
        let c = &layouts["/w/c"];
        assert_eq!(c.len(), 1, "a workspace with no set gets one");
        assert_eq!(c[0].id, Some(1));
        assert_eq!(
            layouts["/w/b"].len(),
            1,
            "a tab this window never woke keeps its place"
        );
        assert!(
            layouts["/w/b"][0].agents.is_empty(),
            "the conversation named twice"
        );
        assert_none_lost(
            &layouts,
            &["1", "2", "3", "9"].map(|at| zo(&format!("session-179000000000{at}-0"))),
            "t-12063",
        );
    }

    /// The conversations a set of tabs names, leaf by leaf, as `agent id`.
    fn named_in(layouts: &Layouts, worktree: &str) -> Vec<Vec<Option<String>>> {
        layouts[worktree]
            .iter()
            .map(|tab| {
                (0..tab.root.leaves())
                    .map(|at| {
                        tab.agents
                            .get(&at)
                            .map(|one| format!("{} {}", one.agent, one.id))
                    })
                    .collect()
            })
            .collect()
    }

    const COORDINATOR: &str = "68b2661a-1c20-4c40-9ad5-419af3097d57";
    const ZO_COORDINATOR: &str = "session-1792758210243-0";

    /// Every conversation in `living` is still named by some leaf of
    /// `layouts` (t-76489 r6): no living conversation was lost.
    fn assert_none_lost(layouts: &Layouts, living: &[WakeAgent], scene: &str) {
        for wake in living {
            assert!(
                !leaves_naming(layouts, wake).is_empty(),
                "{scene}: the living {} {} was lost",
                wake.agent,
                wake.id
            );
        }
    }

    /// t-76489 red ③, 2026-10-26 23:03 as the window lived it: the claude
    /// coordinator's tab (term 1) took a split (term 17) of `zo --resume` of
    /// the zo conversation a person's tab beside it (term 16) already held;
    /// the split died at zo's writer lease. A record naming one conversation
    /// in two leaves wakes it twice, and the leaf that keeps it is the one
    /// whose pane holds it: the dead split's leaf becomes a plain shell, the
    /// claude leaf beside it and the person's zo tab stay whole. With no pane
    /// known to hold it every leaf keeps it (r6: in r1 the first did, on a
    /// guess). Before, both leaves were kept.
    #[test]
    fn a_conversation_two_leaves_name_stays_with_the_pane_that_holds_it() {
        let mut layouts = Layouts::new();
        layouts.insert(
            "/w/zerocode".to_string(),
            vec![
                TabLayout {
                    id: Some(1),
                    agents: HashMap::from([(0, claude(COORDINATOR)), (1, zo(ZO_COORDINATOR))]),
                    running: HashMap::from([(0, "claude".to_string()), (1, "zo".to_string())]),
                    terms: HashMap::from([(0, 1), (1, 17)]),
                    ..bare(split(None))
                },
                TabLayout {
                    id: Some(2),
                    agents: HashMap::from([(0, zo(ZO_COORDINATOR))]),
                    running: HashMap::from([(0, "zo".to_string())]),
                    terms: HashMap::from([(0, 16)]),
                    ..bare(PaneNode::Leaf)
                },
            ],
        );
        let live = HashMap::from([(1, COORDINATOR), (16, ZO_COORDINATOR)]);
        let struck = one_wake_each(&mut layouts, |wake, term| {
            Some(live.get(&term) == Some(&wake.id.as_str()))
        });
        assert_eq!(struck, vec![format!("zo {}", &ZO_COORDINATOR[..8])]);
        assert_eq!(
            named_in(&layouts, "/w/zerocode"),
            vec![
                vec![Some(format!("claude {COORDINATOR}")), None],
                vec![Some(format!("zo {ZO_COORDINATOR}"))],
            ],
            "the living pane's leaf lost its conversation, or the dead split kept it"
        );
        assert!(
            !layouts["/w/zerocode"][0].running.contains_key(&1),
            "the struck leaf would start a fresh zo where the dead split was"
        );
        assert_none_lost(
            &layouts,
            &[claude(COORDINATOR), zo(ZO_COORDINATOR)],
            "the 23:03 split",
        );
        // Asleep — no pane known to hold it — in one workspace's set: both
        // leaves keep it (r6). The boot wakes it in one, and the other comes
        // back a plain shell. In two workspaces' sets nobody here can tell
        // which folder opens it, and both stay for the boot's judge.
        let asleep_tab = || TabLayout {
            agents: HashMap::from([(0, zo(ZO_COORDINATOR))]),
            ..bare(PaneNode::Leaf)
        };
        let mut asleep = Layouts::new();
        asleep.insert("/w/a".to_string(), vec![asleep_tab(), asleep_tab()]);
        let before = asleep.clone();
        assert!(one_wake_each(&mut asleep, |_, _| Some(false)).is_empty());
        assert_eq!(asleep, before, "a record taken off on a guess");
        let mut apart = Layouts::new();
        apart.insert("/w/a".to_string(), vec![asleep_tab()]);
        apart.insert("/w/b".to_string(), vec![asleep_tab()]);
        let before = apart.clone();
        assert!(one_wake_each(&mut apart, |_, _| Some(false)).is_empty());
        assert_eq!(apart, before, "a guess between two folders");
        // One conversation per leaf already: nothing moves.
        let before = layouts.clone();
        assert!(one_wake_each(&mut layouts, |_, _| Some(false)).is_empty());
        assert_eq!(layouts, before);
    }

    /// t-76489 r5: a leaf whose pane cannot be told of — busy past its
    /// patience, a launcher whose group was not read whole — loses nothing
    /// on that word. With no pane known to hold the conversation and one
    /// that may, every leaf naming it stays, even the one known to run a
    /// command; a pane known to hold it still keeps it alone. Before, a pane
    /// that could not tell answered as one that held nothing, and the first
    /// by order kept the conversation.
    #[test]
    fn a_leaf_whose_pane_cannot_be_told_of_loses_nothing() {
        let tab = |id: u64, term: u32| TabLayout {
            id: Some(id),
            agents: HashMap::from([(0, claude(COORDINATOR))]),
            running: HashMap::from([(0, "claude".to_string())]),
            terms: HashMap::from([(0, term)]),
            ..bare(PaneNode::Leaf)
        };
        let set = || {
            let mut layouts = Layouts::new();
            layouts.insert(
                "/w/zerocode".to_string(),
                vec![tab(1, 3), tab(2, 8), tab(3, 9)],
            );
            layouts
        };
        let mut layouts = set();
        let before = layouts.clone();
        let struck = one_wake_each(&mut layouts, |_, term| match term {
            3 => Some(false),
            _ => None,
        });
        assert!(struck.is_empty(), "a guess took {struck:?}");
        assert_eq!(layouts, before);

        let mut layouts = set();
        let struck = one_wake_each(&mut layouts, |_, term| match term {
            9 => Some(true),
            3 => Some(false),
            _ => None,
        });
        assert_eq!(struck.len(), 2);
        assert_eq!(
            named_in(&layouts, "/w/zerocode"),
            vec![
                vec![None],
                vec![None],
                vec![Some(format!("claude {COORDINATOR}"))]
            ],
            "the pane known to hold it lost it"
        );
        assert_none_lost(&layouts, &[claude(COORDINATOR)], "r5");
    }

    /// t-76489 r6, the codex review's first finding of r5: pane A (term 3)
    /// is known to hold coordinator C, and pane B (term 8) — a launcher
    /// whose group was not read whole — cannot be told of, its window
    /// records naming C too. Both reach the exit's write. A's leaf keeps C
    /// on every road: B's leaf named C as well, or the last save left B
    /// out, or two panes nobody can tell of name C and none is known.
    /// Before, B came to the write as if known, took A's leaf off as a
    /// record of a pane gone, and wrote C over its own — or, with no leaf,
    /// as a tab of its own after taking A's off.
    #[test]
    fn a_pane_not_known_to_hold_its_conversation_takes_no_record_away() {
        let main = "/w/zerocode".to_string();
        let tab = |id: u64, term: u32| TabLayout {
            id: Some(id),
            agents: HashMap::from([(0, claude(COORDINATOR))]),
            running: HashMap::from([(0, "claude".to_string())]),
            terms: HashMap::from([(0, term)]),
            ..bare(PaneNode::Leaf)
        };
        let pane = |term: u32, sure: bool| LivePane {
            term,
            worktree: main.clone(),
            wake: claude(COORDINATOR),
            sure,
        };
        let holds = |_: &WakeAgent, term: u32| match term {
            3 => Some(true),
            _ => None,
        };
        let a_keeps = |layouts: &Layouts, scene: &str| {
            assert!(
                layouts[&main]
                    .iter()
                    .any(|tab| tab.agents.get(&0) == Some(&claude(COORDINATOR))
                        && tab.terms.get(&0) == Some(&3)),
                "{scene}: A's leaf lost C: {layouts:#?}"
            );
            assert_none_lost(layouts, &[claude(COORDINATOR)], scene);
        };

        // B's leaf names C too: the save's rule leaves A's alone, and B,
        // not known, adds nothing over it.
        let mut layouts = Layouts::from([(main.clone(), vec![tab(1, 3), tab(2, 8)])]);
        let doubled = one_wake_each(&mut layouts, holds);
        assert_eq!(doubled.len(), 1, "the pane known to hold C keeps it alone");
        let filed = file_unheld(&mut layouts, vec![pane(3, true), pane(8, false)], |_| true);
        assert_eq!(filed, vec![Filed::Named, Filed::Named]);
        a_keeps(&layouts, "B's leaf named C");
        // In the other order of the panes, the same.
        let mut layouts = Layouts::from([(main.clone(), vec![tab(1, 3), tab(2, 8)])]);
        one_wake_each(&mut layouts, holds);
        file_unheld(&mut layouts, vec![pane(8, false), pane(3, true)], |_| true);
        a_keeps(&layouts, "B first");

        // The last save left B out: no leaf of B's at all.
        let mut layouts = Layouts::from([(main.clone(), vec![tab(1, 3)])]);
        let before = layouts.clone();
        assert!(one_wake_each(&mut layouts, holds).is_empty());
        let filed = file_unheld(&mut layouts, vec![pane(3, true), pane(8, false)], |_| true);
        assert_eq!(filed, vec![Filed::Named, Filed::Named]);
        assert_eq!(layouts, before, "B took A's leaf, or wrote C beside it");

        // Two panes nobody can tell of, C on no leaf: the first adds it,
        // and the second takes nothing of the first's.
        let mut layouts = Layouts::from([(main.clone(), vec![shell_of(5)])]);
        let filed = file_unheld(&mut layouts, vec![pane(8, false), pane(9, false)], |_| true);
        assert_eq!(filed, vec![Filed::Tab(2), Filed::Named]);
        assert_eq!(layouts[&main][1].agents[&0], claude(COORDINATOR));
        assert_eq!(layouts[&main][1].terms[&0], 8);
        assert_none_lost(&layouts, &[claude(COORDINATOR)], "two not known");
    }

    /// t-76489 r6: a pane not known to hold its conversation only adds. Its
    /// own leaf names it when that leaf named nothing; a leaf naming another
    /// conversation keeps it, and the pane is a tab of its own beside it; a
    /// pane that named no workspace and has no empty leaf is placed nowhere,
    /// and a record afar stands for it. Nothing any leaf named goes.
    #[test]
    fn a_pane_not_known_to_hold_its_conversation_only_adds() {
        let main = "/w/zerocode".to_string();
        let other = claude("3b7f0c2e-0000-4000-8000-000000000004");
        let unknown = |term: u32, worktree: &str| LivePane {
            term,
            worktree: worktree.to_string(),
            wake: claude(COORDINATOR),
            sure: false,
        };
        let naming = |wake: &WakeAgent, term: Option<u32>| TabLayout {
            id: Some(1),
            agents: HashMap::from([(0, wake.clone())]),
            running: HashMap::from([(0, wake.agent.clone())]),
            terms: term
                .map(|term| HashMap::from([(0, term)]))
                .unwrap_or_default(),
            ..bare(PaneNode::Leaf)
        };

        let mut layouts = Layouts::from([(main.clone(), vec![shell_of(8)])]);
        let filed = file_unheld(&mut layouts, vec![unknown(8, &main)], |_| true);
        assert_eq!(filed, vec![Filed::Renamed]);
        assert_eq!(layouts[&main][0].agents[&0], claude(COORDINATOR));
        assert_eq!(layouts[&main][0].running[&0], "claude");

        let mut layouts = Layouts::from([(main.clone(), vec![naming(&other, Some(8))])]);
        let filed = file_unheld(&mut layouts, vec![unknown(8, &main)], |_| true);
        assert_eq!(filed, vec![Filed::Tab(2)]);
        assert_eq!(
            layouts[&main][0].agents[&0], other,
            "the leaf's own was put over"
        );
        assert!(layouts[&main][0].owed.is_empty());
        assert_eq!(layouts[&main][1].agents[&0], claude(COORDINATOR));
        assert_none_lost(&layouts, &[other.clone(), claude(COORDINATOR)], "beside");

        let mut layouts = Layouts::from([(main.clone(), vec![naming(&other, Some(8))])]);
        let before = layouts.clone();
        let filed = file_unheld(&mut layouts, vec![unknown(8, "")], |_| true);
        assert_eq!(filed, vec![Filed::Unplaced]);
        assert_eq!(layouts, before);

        // A record afar — a tab never woken in another workspace — keeps
        // it for a pane placed nowhere, and stays beside the pane's own tab
        // for one placed here.
        let afar = || {
            Layouts::from([(
                "/w/other".to_string(),
                vec![naming(&claude(COORDINATOR), None)],
            )])
        };
        let mut layouts = afar();
        let filed = file_unheld(&mut layouts, vec![unknown(8, "")], |_| true);
        assert_eq!(filed, vec![Filed::Asleep]);
        assert_eq!(layouts, afar());
        let mut layouts = afar();
        let filed = file_unheld(&mut layouts, vec![unknown(8, &main)], |_| true);
        assert_eq!(filed, vec![Filed::Tab(1)]);
        assert_eq!(
            layouts["/w/other"],
            afar()["/w/other"],
            "the record afar went"
        );
        assert_eq!(layouts[&main][0].agents[&0], claude(COORDINATOR));
    }

    /// t-76489 r7, the codex review's finding of r6: coordinator C runs in
    /// pane 9, which the last save left out and nobody can tell of, and the
    /// only record of C in its workspace is an old one — its pane 5 gone, or
    /// no pane named at all. The old record stays, and pane 9 is written
    /// beside it as a tab of its own: the exit captures its screen, the tab
    /// stands, and the next boot puts the workspace back with C in it. A
    /// record whose pane still stands answers for it as before, and a pane
    /// placed nowhere names C on its own empty leaf, else ties a dead pane's
    /// record to itself. Before, the old record's name alone answered
    /// `Named`: nothing pointed at pane 9, the tab was stored asleep, and a
    /// workspace not in front never came back, with nothing said of it.
    #[test]
    fn a_record_of_no_living_pane_does_not_answer_for_a_pane_not_known_to_hold_it() {
        let root = tempfile::tempdir().expect("the checkouts");
        let main = root.path().join("zerocode");
        std::fs::create_dir_all(&main).expect("a checkout");
        let main = main.to_string_lossy().into_owned();
        let record = |term: Option<u32>| TabLayout {
            id: Some(1),
            agents: HashMap::from([(0, claude(COORDINATOR))]),
            running: HashMap::from([(0, "claude".to_string())]),
            terms: term
                .map(|term| HashMap::from([(0, term)]))
                .unwrap_or_default(),
            ..bare(PaneNode::Leaf)
        };
        let unknown = |worktree: &str| LivePane {
            term: 9,
            worktree: worktree.to_string(),
            wake: claude(COORDINATOR),
            sure: false,
        };
        let standing = |term: u32| term == 9;
        let screens = |term: u32| standing(term).then(|| format!("screen of {term}"));

        for (scene, term) in [
            ("a dead pane's record", Some(5)),
            ("a record of no pane", None),
        ] {
            let mut layouts = Layouts::from([(main.clone(), vec![record(term)])]);
            let filed = file_unheld(&mut layouts, vec![unknown(&main)], standing);
            assert_eq!(filed, vec![Filed::Tab(2)], "{scene}");
            assert_eq!(
                layouts[&main][0],
                record(term),
                "{scene}: the old record was changed"
            );
            capture_exit(&mut layouts, screens);
            let tab = &layouts[&main][1];
            assert_eq!(tab.agents[&0], claude(COORDINATOR), "{scene}");
            assert!(
                tab.awake,
                "{scene}: the living coordinator's tab was stored asleep"
            );
            assert_eq!(tab.buffers[&0], "screen of 9", "{scene}: no screen");
            assert_eq!(standing_worktrees(&layouts), vec![main.clone()], "{scene}");
            assert_none_lost(&layouts, &[claude(COORDINATOR)], scene);
        }

        // A record whose pane still stands answers for it: nothing added.
        let mut layouts = Layouts::from([(main.clone(), vec![record(Some(5))])]);
        let before = layouts.clone();
        let filed = file_unheld(&mut layouts, vec![unknown(&main)], |term| {
            term == 5 || term == 9
        });
        assert_eq!(filed, vec![Filed::Named]);
        assert_eq!(layouts, before);

        // A pane placed nowhere names C on its own empty leaf, beside the
        // dead pane's record.
        let mut layouts = Layouts::from([(main.clone(), vec![record(Some(5)), shell_of(9)])]);
        let filed = file_unheld(&mut layouts, vec![unknown("")], standing);
        assert_eq!(filed, vec![Filed::Renamed]);
        assert_eq!(layouts[&main][0], record(Some(5)));
        assert_eq!(layouts[&main][1].agents[&0], claude(COORDINATOR));
        capture_exit(&mut layouts, screens);
        assert_eq!(standing_worktrees(&layouts), vec![main.clone()]);

        // With no leaf of its own, the dead pane's record is tied to it —
        // the record keeps its conversation and its place — and a record of
        // no pane, which a pane placed nowhere cannot confirm, stands for
        // it as before.
        let mut layouts = Layouts::from([(main.clone(), vec![record(Some(5))])]);
        let filed = file_unheld(&mut layouts, vec![unknown("")], standing);
        assert_eq!(filed, vec![Filed::Relinked]);
        assert_eq!(layouts[&main][0].agents[&0], claude(COORDINATOR));
        assert_eq!(layouts[&main][0].terms[&0], 9);
        capture_exit(&mut layouts, screens);
        assert!(layouts[&main][0].awake);
        assert_eq!(standing_worktrees(&layouts), vec![main.clone()]);
        let mut layouts = Layouts::from([(main.clone(), vec![record(None)])]);
        let filed = file_unheld(&mut layouts, vec![unknown("")], standing);
        assert_eq!(filed, vec![Filed::Asleep]);
        assert_eq!(layouts[&main], vec![record(None)]);
    }

    /// A stored plain shell that stood for pane `term`.
    fn shell_of(term: u32) -> TabLayout {
        TabLayout {
            id: Some(1),
            terms: HashMap::from([(0, term)]),
            ..bare(PaneNode::Leaf)
        }
    }

    /// t-76489 red ④, the exit's net: a run's coordinator conversation the
    /// saved set does not name — the 10-26 file had no line for the claude
    /// coordinator at all — is written down in its workspace as a tab of its
    /// own, never in front; a leaf that holds its shell while naming another
    /// conversation names it now, and owes the one it named only when no
    /// other leaf stands for it; and with no workspace to name it is placed
    /// nowhere and said so. Before, only zo panes were filed, and a pane a
    /// leaf held by its shell was that leaf's whatever the leaf named.
    #[test]
    fn the_exit_writes_down_a_coordinator_no_saved_leaf_names() {
        let mut layouts = Layouts::new();
        layouts.insert(
            "/w/zerocode".to_string(),
            vec![TabLayout {
                id: Some(1),
                agents: HashMap::from([(0, zo(ZO_COORDINATOR))]),
                running: HashMap::from([(0, "zo".to_string())]),
                terms: HashMap::from([(0, 16)]),
                focused: true,
                ..bare(PaneNode::Leaf)
            }],
        );
        let pane = |term: u32, worktree: &str, wake: WakeAgent| LivePane {
            term,
            worktree: worktree.to_string(),
            wake,
            sure: true,
        };
        let filed = file_unheld(
            &mut layouts,
            vec![
                pane(1, "/w/zerocode", claude(COORDINATOR)),
                pane(16, "/w/zerocode", zo(ZO_COORDINATOR)),
                pane(4, "", claude("3b7f0c2e-0000-4000-8000-000000000001")),
            ],
            |_| true,
        );
        assert_eq!(filed, vec![Filed::Tab(2), Filed::Named, Filed::Unplaced]);
        let set = &layouts["/w/zerocode"];
        assert_eq!(set.len(), 2);
        assert_eq!(set[1].agents[&0], claude(COORDINATOR));
        assert_eq!(set[1].running[&0], "claude");
        assert!(!set[1].focused, "the filed tab took the person's eye");
        assert!(set[0].focused);
        assert!(
            !layouts.contains_key(""),
            "an unplaced pane was written under no path"
        );
        assert_none_lost(&layouts, &[claude(COORDINATOR), zo(ZO_COORDINATOR)], "r1 ④");

        // The leaf holding the coordinator's shell names a conversation a
        // tab beside it stands for: it names the coordinator now, owing
        // nothing; one standing nowhere else is owed.
        let mut held = Layouts::new();
        held.insert(
            "/w/zerocode".to_string(),
            vec![
                TabLayout {
                    id: Some(1),
                    agents: HashMap::from([(0, zo(ZO_COORDINATOR))]),
                    terms: HashMap::from([(0, 1)]),
                    ..bare(PaneNode::Leaf)
                },
                TabLayout {
                    id: Some(2),
                    agents: HashMap::from([(0, zo(ZO_COORDINATOR))]),
                    terms: HashMap::from([(0, 16)]),
                    ..bare(PaneNode::Leaf)
                },
                TabLayout {
                    id: Some(3),
                    agents: HashMap::from([(0, zo("session-1792000000000-0"))]),
                    terms: HashMap::from([(0, 21)]),
                    ..bare(PaneNode::Leaf)
                },
            ],
        );
        let filed = file_unheld(
            &mut held,
            vec![
                pane(1, "/w/zerocode", claude(COORDINATOR)),
                pane(
                    21,
                    "/w/zerocode",
                    claude("3b7f0c2e-0000-4000-8000-000000000002"),
                ),
            ],
            |_| true,
        );
        assert_eq!(filed, vec![Filed::Renamed, Filed::Renamed]);
        let set = &held["/w/zerocode"];
        assert_eq!(set[0].agents[&0], claude(COORDINATOR));
        assert_eq!(set[0].running[&0], "claude");
        assert!(
            set[0].owed.is_empty(),
            "owed a conversation another tab stands for"
        );
        assert_eq!(set[2].owed, vec![zo("session-1792000000000-0")]);
        assert_none_lost(
            &held,
            &[
                claude(COORDINATOR),
                claude("3b7f0c2e-0000-4000-8000-000000000002"),
            ],
            "r1 ④ renamed",
        );
    }

    /// t-76489 r2 ①, the codex review's first finding: a person ends
    /// conversation C in pane A (term 3) and reopens it in pane B (term 8).
    /// A's shell stays, and so do the window's records of A's agent and
    /// session — the 이어서 menu reopens from them. Only B's agent is in
    /// front. The save's judge is [`crate::cmd::settings::pane_holds`]: B's
    /// leaf keeps C, A's comes back a plain shell. Before, the records alone
    /// said both held it, A came first, and B's leaf lost C and its program.
    #[test]
    fn a_pane_whose_agent_was_quit_does_not_keep_the_conversation_it_ended() {
        let mut layouts = Layouts::new();
        layouts.insert(
            "/w/zerocode".to_string(),
            vec![
                TabLayout {
                    id: Some(1),
                    agents: HashMap::from([(0, claude(COORDINATOR))]),
                    running: HashMap::from([(0, "claude".to_string())]),
                    terms: HashMap::from([(0, 3)]),
                    ..bare(PaneNode::Leaf)
                },
                TabLayout {
                    id: Some(2),
                    agents: HashMap::from([(0, claude(COORDINATOR))]),
                    running: HashMap::from([(0, "claude".to_string())]),
                    terms: HashMap::from([(0, 8)]),
                    focused: true,
                    ..bare(PaneNode::Leaf)
                },
            ],
        );
        let session = zerocode_core::ProviderSession {
            key: zerocode_core::SessionKey::SessionId,
            id: COORDINATOR.to_string(),
            transcript_path: None,
        };
        let sessions = HashMap::from([(3, session.clone()), (8, session)]);
        let agents = HashMap::from([(3, "claude"), (8, "claude")]);
        let in_front = |term: u32| Some(term == 8);
        let struck = one_wake_each(&mut layouts, |wake, term| {
            crate::cmd::settings::pane_holds(&sessions, &agents, &in_front, wake, term)
        });
        assert_eq!(struck, vec![format!("claude {}", &COORDINATOR[..8])]);
        assert_eq!(
            named_in(&layouts, "/w/zerocode"),
            vec![vec![None], vec![Some(format!("claude {COORDINATOR}"))]],
            "the pane that ended the conversation kept it, and the one that reopened it lost it"
        );
        let set = &layouts["/w/zerocode"];
        assert!(!set[0].running.contains_key(&0));
        assert_eq!(set[1].running[&0], "claude", "B lost its program");
        assert!(set[1].focused, "B lost the person's eye");
        assert_none_lost(&layouts, &[claude(COORDINATOR)], "r2 ①");
        // The records alone, as before: the shell in front of A counts for
        // nothing, so the first by order would keep it.
        assert_eq!(
            crate::cmd::settings::pane_holds(
                &sessions,
                &agents,
                &|_| Some(true),
                &claude(COORDINATOR),
                3
            ),
            Some(true)
        );
        assert_eq!(
            crate::cmd::settings::pane_holds(
                &sessions,
                &agents,
                &in_front,
                &claude(COORDINATOR),
                3
            ),
            Some(false)
        );
    }

    /// t-76489 r2 ②, the codex review's second finding: the saved set names
    /// coordinator C at one leaf only, and that leaf's pane (term 5) is gone;
    /// C lives in pane 9, which the last save left out. The exit's write ties
    /// the leaf to pane 9, so its screen is captured, the tab stands, and the
    /// next boot puts the workspace back with C in it. Before, the dead leaf's
    /// name alone answered `Named`: nothing was tied to pane 9, the tab was
    /// stored asleep, and a workspace not in front never came back.
    #[test]
    fn a_dead_panes_leaf_alone_does_not_answer_for_the_living_pane() {
        let root = tempfile::tempdir().expect("the checkouts");
        let main = root.path().join("zerocode");
        let other = root.path().join("other");
        std::fs::create_dir_all(&main).expect("a checkout");
        std::fs::create_dir_all(&other).expect("a checkout");
        let main = main.to_string_lossy().into_owned();
        let other = other.to_string_lossy().into_owned();
        let dead_leaf = |term: u32| TabLayout {
            id: Some(1),
            agents: HashMap::from([(0, claude(COORDINATOR))]),
            running: HashMap::from([(0, "claude".to_string())]),
            terms: HashMap::from([(0, term)]),
            ..bare(PaneNode::Leaf)
        };
        let living = |worktree: &str| LivePane {
            term: 9,
            worktree: worktree.to_string(),
            wake: claude(COORDINATOR),
            sure: true,
        };
        let standing = |term: u32| term == 9;
        let screens = |term: u32| standing(term).then(|| format!("screen of {term}"));

        let mut layouts = Layouts::new();
        layouts.insert(main.clone(), vec![dead_leaf(5)]);
        let filed = file_unheld(&mut layouts, vec![living(&main)], standing);
        assert_eq!(filed, vec![Filed::Relinked]);
        capture_exit(&mut layouts, screens);
        let tab = &layouts[&main][0];
        assert_eq!(tab.agents[&0], claude(COORDINATOR));
        assert!(
            tab.awake,
            "the tab of the living coordinator was stored asleep"
        );
        assert_eq!(
            tab.buffers[&0], "screen of 9",
            "its screen was not captured"
        );
        assert_eq!(standing_worktrees(&layouts), vec![main.clone()]);

        // A pane that reported no workspace: the dead leaf is still its.
        let mut layouts = Layouts::new();
        layouts.insert(main.clone(), vec![dead_leaf(5)]);
        let filed = file_unheld(&mut layouts, vec![living("")], standing);
        assert_eq!(filed, vec![Filed::Relinked]);
        capture_exit(&mut layouts, screens);
        assert_eq!(standing_worktrees(&layouts), vec![main.clone()]);

        // The dead leaf in another workspace's set: zo opens a conversation
        // from its own folder, so the living pane is written where it is,
        // and the other set's leaf comes back a plain shell.
        let mut layouts = Layouts::new();
        layouts.insert(other.clone(), vec![dead_leaf(5)]);
        let filed = file_unheld(&mut layouts, vec![living(&main)], standing);
        assert_eq!(filed, vec![Filed::Tab(1)]);
        capture_exit(&mut layouts, screens);
        assert_eq!(layouts[&main][0].agents[&0], claude(COORDINATOR));
        assert!(layouts[&other][0].agents.is_empty(), "C named twice");
        assert_eq!(standing_worktrees(&layouts), vec![main.clone()]);

        // A leaf whose shell is still open, its agent quit — pane 5 stands
        // but does not hold C: the living pane gets a tab of its own, and
        // the quit pane's leaf stays where it is as a plain shell.
        let mut layouts = Layouts::new();
        layouts.insert(main.clone(), vec![dead_leaf(5)]);
        let filed = file_unheld(&mut layouts, vec![living(&main)], |term| {
            term == 5 || term == 9
        });
        assert_eq!(filed, vec![Filed::Tab(2)]);
        assert_eq!(
            named_in(&layouts, &main),
            vec![vec![None], vec![Some(format!("claude {COORDINATOR}"))]]
        );
        assert_none_lost(&layouts, &[claude(COORDINATOR)], "r2 ②");
    }

    /// t-76489 r3 ②, the codex review's second finding of r2: workspace A's
    /// set names coordinator C in a tab this window never woke (no shell),
    /// while C runs in pane 9 of workspace B, which the last save left out.
    /// The pane's hold is confirmed and A's record is not: C is written into
    /// B as a tab of its own, its screen captured and its tab standing, so
    /// the next boot puts B back with C in it; A's leaf comes back a plain
    /// shell, C named once. When B's own leaf holds pane 9 while naming
    /// another conversation, that leaf names C and A's record goes too.
    /// A zo coordinator is no exception (r4, the codex review of r3): its
    /// pane in B is written there and A's record goes. A pane that named no
    /// workspace still leaves the record afar its word. Before, A's record
    /// alone answered `Named`: nothing was written for pane 9, A's tab was
    /// stored asleep, and neither a restore nor a notice followed — and in
    /// r3 so still for zo.
    #[test]
    fn a_record_afar_does_not_answer_for_a_coordinator_running_elsewhere() {
        let root = tempfile::tempdir().expect("the checkouts");
        let path = |name: &str| {
            let at = root.path().join(name);
            std::fs::create_dir_all(&at).expect("a checkout");
            at.to_string_lossy().into_owned()
        };
        let (a, b) = (path("a"), path("b"));
        let unwoken = |wake: WakeAgent| TabLayout {
            id: Some(4),
            running: HashMap::from([(0, wake.agent.clone())]),
            agents: HashMap::from([(0, wake)]),
            ..bare(PaneNode::Leaf)
        };
        let living = |wake: WakeAgent| LivePane {
            term: 9,
            worktree: b.clone(),
            wake,
            sure: true,
        };
        let standing = |term: u32| term == 9;
        let screens = |term: u32| standing(term).then(|| format!("screen of {term}"));

        let mut layouts = Layouts::new();
        layouts.insert(a.clone(), vec![unwoken(claude(COORDINATOR))]);
        let filed = file_unheld(&mut layouts, vec![living(claude(COORDINATOR))], standing);
        assert_eq!(filed, vec![Filed::Tab(1)], "the record afar answered for B");
        capture_exit(&mut layouts, screens);
        let tab = &layouts[&b][0];
        assert_eq!(tab.agents[&0], claude(COORDINATOR));
        assert_eq!(tab.running[&0], "claude");
        assert_eq!(tab.buffers[&0], "screen of 9");
        assert!(tab.awake, "B's coordinator was stored asleep");
        assert!(
            layouts[&a][0].agents.is_empty() && layouts[&a][0].running.is_empty(),
            "C named twice"
        );
        assert_eq!(standing_worktrees(&layouts), vec![b.clone()]);
        assert_none_lost(&layouts, &[claude(COORDINATOR)], "r3 ② tab");

        // B's leaf holds pane 9 and names another conversation: it names C
        // now, owes the one it named, and A's record goes on this road too.
        let other = claude("3b7f0c2e-0000-4000-8000-000000000003");
        let mut layouts = Layouts::new();
        layouts.insert(a.clone(), vec![unwoken(claude(COORDINATOR))]);
        layouts.insert(
            b.clone(),
            vec![TabLayout {
                id: Some(1),
                agents: HashMap::from([(0, other.clone())]),
                terms: HashMap::from([(0, 9)]),
                ..bare(PaneNode::Leaf)
            }],
        );
        let filed = file_unheld(&mut layouts, vec![living(claude(COORDINATOR))], standing);
        assert_eq!(filed, vec![Filed::Renamed]);
        assert_eq!(layouts[&b][0].agents[&0], claude(COORDINATOR));
        assert_eq!(layouts[&b][0].owed, vec![other]);
        assert!(layouts[&a][0].agents.is_empty(), "C named twice");
        assert!(one_wake_each(&mut layouts, |_, _| Some(false)).is_empty());
        assert_none_lost(&layouts, &[claude(COORDINATOR)], "r3 ② renamed");

        // A zo coordinator running in B is written into B as well: B is a
        // folder it opens the conversation from, for it runs it there now.
        let mut layouts = Layouts::new();
        layouts.insert(a.clone(), vec![unwoken(zo(ZO_COORDINATOR))]);
        let filed = file_unheld(&mut layouts, vec![living(zo(ZO_COORDINATOR))], standing);
        assert_eq!(
            filed,
            vec![Filed::Tab(1)],
            "the record afar answered for B's zo"
        );
        capture_exit(&mut layouts, screens);
        let tab = &layouts[&b][0];
        assert_eq!(tab.agents[&0], zo(ZO_COORDINATOR));
        assert_eq!(tab.running[&0], "zo");
        assert_eq!(tab.buffers[&0], "screen of 9");
        assert!(tab.awake, "B's zo coordinator was stored asleep");
        assert!(
            layouts[&a][0].agents.is_empty() && layouts[&a][0].running.is_empty(),
            "the zo conversation named twice"
        );
        assert_eq!(standing_worktrees(&layouts), vec![b.clone()]);
        assert_none_lost(&layouts, &[zo(ZO_COORDINATOR)], "r4 ② zo");

        // A zo pane that named no workspace leaves the record afar its
        // word, as before: nobody here can place it.
        let mut layouts = Layouts::new();
        layouts.insert(a.clone(), vec![unwoken(zo(ZO_COORDINATOR))]);
        let before = layouts.clone();
        let unplaced = LivePane {
            worktree: String::new(),
            ..living(zo(ZO_COORDINATOR))
        };
        let filed = file_unheld(&mut layouts, vec![unplaced], standing);
        assert_eq!(filed, vec![Filed::Asleep]);
        assert_eq!(layouts, before);

        // And two records of one zo conversation in two workspaces, no pane
        // holding either, both stay: nobody here can tell which folder
        // opens it.
        let mut layouts = Layouts::new();
        layouts.insert(a.clone(), vec![unwoken(zo(ZO_COORDINATOR))]);
        layouts.insert(b.clone(), vec![unwoken(zo(ZO_COORDINATOR))]);
        let before = layouts.clone();
        assert!(one_wake_each(&mut layouts, |_, _| Some(false)).is_empty());
        assert_eq!(layouts, before);
    }

    /// The file names every tab it keeps and hands the names back in the
    /// order the tabs came (t-14036): a tab born in this session reaches the
    /// file nameless, and the window can only seat a named tab on its stage.
    /// A tab not worth keeping is answered with nothing, in its place.
    #[test]
    fn a_save_hands_back_the_name_each_tab_was_kept_under() {
        let dir = tempfile::tempdir().expect("tempdir");
        let here = dir.path().to_string_lossy().into_owned();
        let mut layouts = Layouts::new();
        let named = |id: Option<u64>| TabLayout {
            id,
            ..bare(PaneNode::Leaf)
        };
        let refused = TabLayout {
            root: (0..=MAX_LEAVES).fold(PaneNode::Leaf, |tree, _| split_of(tree)),
            ..bare(PaneNode::Leaf)
        };
        let names = store(
            &mut layouts,
            here.clone(),
            vec![named(Some(7)), named(None), refused, named(None)],
        );
        assert_eq!(names, vec![Some(7), Some(8), None, Some(9)]);
        // And the names stay: the same tabs saved again under the names they
        // were handed keep them.
        let again = store(
            &mut layouts,
            here.clone(),
            vec![named(Some(7)), named(Some(8)), named(Some(9))],
        );
        assert_eq!(again, vec![Some(7), Some(8), Some(9)]);
    }

    fn split_of(first: PaneNode) -> PaneNode {
        PaneNode::Split {
            direction: SplitDirection::Vertical,
            first: Box::new(first),
            second: Box::new(PaneNode::Leaf),
            ratio: None,
        }
    }

    /// The exit says which tabs were standing, and nothing else does
    /// (t-14036): a tab whose leaves still had a terminal is awake, a tab
    /// this session never woke — no pty ids — is not, whatever an earlier
    /// exit said, and keeps the screen it had. The next boot puts back the
    /// workspaces that were running a conversation, in path order, and only
    /// those still on the disk.
    #[test]
    fn the_exit_marks_the_tabs_that_were_standing_and_the_boot_finds_their_workspaces() {
        let root = tempfile::tempdir().expect("tempdir");
        let trees = checkouts(root.path(), 3);
        let conversation = TabLayout {
            agents: HashMap::from([(0, zo("session-1790000000031-0"))]),
            running: HashMap::from([(0, "zo".to_string())]),
            ..bare(PaneNode::Leaf)
        };
        let mut layouts = Layouts::new();
        // The first workspace ran its conversation; its terminal is live.
        layouts.insert(
            trees[0].clone(),
            vec![TabLayout {
                terms: HashMap::from([(0, 41)]),
                ..conversation.clone()
            }],
        );
        // The second held one asleep since an earlier exit that found it
        // running, and a plain shell that was standing.
        layouts.insert(
            trees[1].clone(),
            vec![
                TabLayout {
                    awake: true,
                    buffers: HashMap::from([(0, "earlier screen".to_string())]),
                    ..conversation.clone()
                },
                TabLayout {
                    terms: HashMap::from([(0, 42)]),
                    ..bare(PaneNode::Leaf)
                },
            ],
        );
        // The third's conversation had ended before the window went.
        layouts.insert(
            trees[2].clone(),
            vec![TabLayout {
                terms: HashMap::from([(0, 43)]),
                ..conversation.clone()
            }],
        );
        // And a checkout since deleted.
        layouts.insert(
            root.path().join("gone").to_string_lossy().into_owned(),
            vec![TabLayout {
                awake: true,
                ..conversation
            }],
        );
        capture_exit(&mut layouts, |term| {
            (term != 43).then(|| format!("screen {term}"))
        });

        let tree = |at: usize| &layouts[&trees[at]];
        assert!(tree(0)[0].awake && tree(0)[0].terms.is_empty());
        assert_eq!(tree(0)[0].buffers[&0], "screen 41");
        assert!(
            !tree(1)[0].awake,
            "a tab this session never woke was called standing"
        );
        assert_eq!(tree(1)[0].buffers[&0], "earlier screen");
        assert!(tree(1)[1].awake);
        assert!(
            !tree(2)[0].awake,
            "a tab whose terminal had ended was called standing"
        );
        assert_eq!(standing_worktrees(&layouts), vec![trees[0].clone()]);
    }

    fn claude(id: &str) -> WakeAgent {
        WakeAgent {
            agent: "claude".to_string(),
            key: "session_id".to_string(),
            id: id.to_string(),
            transcript_path: None,
            interrupted: false,
        }
    }

    /// A finished worker's conversation, standing in for the three
    /// 2026-10-15/16 cases, and the worker it was (t-57717).
    const FINISHED: &str = "57717a0c-0000-4000-8000-0000000000f1";

    fn finished(wake: &WakeAgent) -> Option<String> {
        (wake.id == FINISHED).then(|| "w-54907".to_string())
    }

    /// t-57717: the record the first visit to a finished worker's checkout
    /// read — one tab, its one leaf that worker's conversation — is not kept,
    /// so the workspace opens as one that stored nothing; a tab whose other
    /// leaf is a person's keeps the person's leaf as it was and the struck
    /// leaf comes back a plain shell; a person's tab and a plain shell are
    /// untouched; and each struck conversation names its worker.
    #[test]
    fn a_finished_workers_conversation_leaves_the_set_and_a_persons_stays() {
        let worker_tab = TabLayout {
            id: Some(1),
            agents: HashMap::from([(0, claude(FINISHED))]),
            running: HashMap::from([(0, "claude".to_string())]),
            focused: true,
            awake: true,
            ..bare(PaneNode::Leaf)
        };
        let shared = TabLayout {
            id: Some(2),
            agents: HashMap::from([
                (0, claude(FINISHED)),
                (1, claude("3f1e2d4c-0000-4000-8000-00000000c1a0")),
            ]),
            running: HashMap::from([(0, "claude".to_string()), (1, "claude".to_string())]),
            titles: HashMap::from([(0, "the worker's pane".to_string())]),
            ..bare(split(None))
        };
        let persons = TabLayout {
            id: Some(3),
            agents: HashMap::from([(0, zo("session-1790551803628-0"))]),
            running: HashMap::from([(0, "zo".to_string())]),
            ..bare(PaneNode::Leaf)
        };
        let shell = TabLayout {
            id: Some(4),
            ..bare(PaneNode::Leaf)
        };

        let left = without_finished(
            vec![worker_tab, shared.clone(), persons.clone(), shell.clone()],
            finished,
        );

        assert_eq!(
            left.struck,
            vec!["w-54907".to_string(), "w-54907".to_string()]
        );
        assert_eq!(left.tabs.len(), 4, "a tab's place in the answer moved");
        assert_eq!(left.tabs[0], None, "the finished worker's tab came back");
        let kept = left.tabs[1].as_ref().expect("the tab a person shares");
        assert_eq!(kept.agents.keys().copied().collect::<Vec<_>>(), vec![1]);
        assert_eq!(kept.running.keys().copied().collect::<Vec<_>>(), vec![1]);
        assert_eq!(kept.root, shared.root, "the tree lost a leaf");
        assert_eq!(kept.titles, shared.titles, "the leaf's name went with it");
        assert_eq!(left.tabs[2].as_ref(), Some(&persons));
        assert_eq!(left.tabs[3].as_ref(), Some(&shell));
        assert_eq!(
            left.kept()
                .into_iter()
                .filter_map(|tab| tab.id)
                .collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
    }

    /// t-57717: nothing struck is the same set — a ledger that names no
    /// worker for any conversation leaves the record exactly as it stood.
    #[test]
    fn a_set_with_no_finished_conversation_is_left_as_it_stands() {
        let tabs = vec![
            TabLayout {
                id: Some(1),
                agents: HashMap::from([(0, claude("3f1e2d4c-0000-4000-8000-00000000c1a0"))]),
                running: HashMap::from([(0, "claude".to_string())]),
                awake: true,
                ..bare(PaneNode::Leaf)
            },
            bare(split(Some(0.3))),
        ];
        let left = without_finished(tabs.clone(), finished);
        assert!(left.struck.is_empty());
        assert_eq!(left.kept(), tabs);
    }

    /// t-57717: a workspace whose only standing conversation was a finished
    /// worker's is not put back behind the front — the boot reads the set
    /// with the finished leaves already gone.
    #[test]
    fn a_workspace_standing_only_for_a_finished_worker_is_not_put_back() {
        let root = tempfile::tempdir().expect("the checkouts");
        let worker_checkout = root.path().join("t-54563");
        let persons_checkout = root.path().join("main");
        std::fs::create_dir_all(&worker_checkout).expect("the worker's checkout");
        std::fs::create_dir_all(&persons_checkout).expect("the person's checkout");
        let mut layouts = Layouts::new();
        layouts.insert(
            worker_checkout.to_string_lossy().into_owned(),
            vec![TabLayout {
                agents: HashMap::from([(0, claude(FINISHED))]),
                running: HashMap::from([(0, "claude".to_string())]),
                awake: true,
                ..bare(PaneNode::Leaf)
            }],
        );
        layouts.insert(
            persons_checkout.to_string_lossy().into_owned(),
            vec![TabLayout {
                agents: HashMap::from([(0, zo("session-1790551803628-0"))]),
                awake: true,
                ..bare(PaneNode::Leaf)
            }],
        );
        let before = standing_worktrees(&layouts);
        for tabs in layouts.values_mut() {
            *tabs = without_finished(std::mem::take(tabs), finished).kept();
        }
        assert_eq!(before.len(), 2);
        assert_eq!(
            standing_worktrees(&layouts),
            vec![persons_checkout.to_string_lossy().into_owned()]
        );
    }
}
