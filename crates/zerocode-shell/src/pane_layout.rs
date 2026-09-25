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
}

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
        let agents = self
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
        })
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
/// it did not.
pub(crate) fn rewrite(
    file: &Path,
    change: impl FnOnce(&mut Layouts) -> bool,
) -> std::io::Result<()> {
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _writing = WRITING.lock().unwrap_or_else(|held| held.into_inner());
    let mut held = read(file);
    if !change(&mut held) {
        return Ok(());
    }
    let text = serde_json::to_vec(&held).map_err(std::io::Error::other)?;
    crate::durable_file::replace_bytes(file, &text).map(|_| ())
}

/// The window's own save of one worktree's set, through [`rewrite`] — or
/// nothing, once the window is leaving (t-7812 D).
///
/// From the first exit signal the file is the list the next window restores.
/// What the window saves after it describes the window dying: a shell that
/// ended on the way out, and the tab the window pruned around it. Asked
/// under the writer's own lock, so a save cannot land between the exit's own
/// snapshot and the process ending. Answers whether the set was written.
pub(crate) fn save_window_set(
    file: &Path,
    worktree: String,
    tabs: Vec<TabLayout>,
    leaving: &dyn Fn() -> bool,
) -> std::io::Result<bool> {
    let mut saved = false;
    rewrite(file, |held| {
        if leaving() {
            return false;
        }
        store(held, worktree, tabs);
        saved = true;
        true
    })?;
    Ok(saved)
}

/// Put one worktree's tabs into `layouts`, normalised, pruning as it goes.
///
/// An empty list removes the key: a worktree whose last terminal tab closed
/// must not resurrect it on the next visit. Entries for worktrees that no
/// longer exist on disk go at the same time — this map is keyed by checkout
/// paths, and checkouts are things people delete.
pub fn store(layouts: &mut Layouts, worktree: String, tabs: Vec<TabLayout>) {
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
    for tab in tabs {
        let Some(mut tab) = tab.normalized() else {
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
        kept.push(tab);
    }
    if kept.is_empty() {
        layouts.remove(&worktree);
    } else {
        layouts.insert(worktree, kept);
    }
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
                        )
                        .expect("a save");
                        assert!(saved, "a window that is not leaving was refused");
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
            )
            .expect("a save")
        );
        let before = std::fs::read(&file).expect("the list");
        // The last tab's shell ended on the way out; the window saves the
        // set without it.
        assert!(!save_window_set(&file, tree.clone(), Vec::new(), &|| true).expect("a refusal"));
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
}
