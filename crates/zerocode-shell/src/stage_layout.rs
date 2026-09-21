//! The stage's document tabs and group tree, kept across the window closing.
//!
//! Orca persists its editor centre whole — the tabs (`unifiedTabs`), the
//! groups (`tabGroups`), the split tree (`tabGroupLayout`) and the focused
//! group — and validates all of it on the way back
//! (`hydrateUnifiedFormat`, I18nProvider-4EBrmTGg.js:57156). The measured
//! rules this file holds:
//!
//! - **transient tabs do not come back.** A diff, a conflict review, a
//!   check-details tab is a view of a moment
//!   (`isTransientEditorContentType`, :50810) — the checkout moves under
//!   it, and restoring one shows a comparison that is no longer true.
//! - **the tree is pruned to the groups that survived**
//!   (`pruneTabGroupLayoutForGroups`, :57137): a leaf whose group kept no
//!   tabs folds out, and a split that loses a child IS the other child.
//! - **focus falls back through the non-empty groups**
//!   (`selectHydratedActiveGroupId`, :50822): the stored group if it still
//!   has tabs, else the first that does, else the first at all.
//! - the ratio rules are [`crate::pane_layout`]'s — the stage tree and the
//!   terminal pane tree store the same node, so they share the same file's
//!   normalisation (even splits store nothing, uneven ones three decimals).
//!
//! Same discipline as the pane layouts: the window sends what it has, the
//! rules live here, and what comes back out is already normalised.

use crate::pane_layout::PaneNode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The most groups one stored stage is believed to hold — the same startup
/// trust argument as the pane tree's own cap.
const MAX_GROUPS: usize = 16;

/// The most tabs one stored group is believed to hold.
const MAX_TABS: usize = 64;

/// Stored document kinds whose reopen door requires a file path.
pub(crate) const STAGE_PATH_KINDS: &[&str] = &["file", "image", "mdview", "csv", "ipynb"];

/// Stored document kinds whose reopen door takes no path.
pub(crate) const STAGE_ARGLESS_KINDS: &[&str] = &[
    "board",
    "changes",
    "vault",
    "knowledge",
    "skills",
    "artifacts",
    "jev",
];

/// One document tab, as the file remembers it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StageTab {
    /// A kind from [`STAGE_PATH_KINDS`] or [`STAGE_ARGLESS_KINDS`]. The
    /// transient kinds (`diff`, `imagediff`) are refused by
    /// [`StageTab::keeps`], Orca's own rule.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A glance stays a glance across the restart (`isPreview`, persisted
    /// on Orca's unified tabs).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preview: bool,
    /// And a pin stays a pin (`isPinned`, the same store). A record wearing
    /// both is a hand edit — pinning kills the glance (`pinTab` sets
    /// `isPreview: false`, I18nProvider:58012) — and the pin wins.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// The unsaved typing, when there is some — Orca's `dirtyDraftContent`
    /// on the persisted open files (buildEditorSessionData,
    /// index-ftls8Hg_.js:14755: written only while dirty). Only a `file`
    /// carries one; on any other kind it is a hand edit and is shed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
}

impl StageTab {
    /// Whether this is something the window could reopen.
    ///
    /// Path kinds require a non-empty regular-file path. A `file` whose path
    /// is absent from disk survives only when it carries a draft, including an
    /// empty draft: that buffer is the last recoverable copy. Other errors and
    /// link-like or directory targets fall toward refusal. Transient kinds are
    /// out entirely (`isTransientEditorContentType`, :50810).
    fn keeps(&self) -> bool {
        if STAGE_ARGLESS_KINDS.contains(&self.kind.as_str()) {
            return self.path.is_none();
        }
        if !STAGE_PATH_KINDS.contains(&self.kind.as_str()) {
            return false;
        }
        let Some(path) = self.path.as_deref().filter(|path| !path.is_empty()) else {
            return false;
        };
        match std::fs::metadata(path) {
            Ok(found) => found.is_file(),
            Err(error) => {
                self.kind == "file"
                    && self.draft.is_some()
                    && error.kind() == std::io::ErrorKind::NotFound
                    && matches!(
                        std::fs::symlink_metadata(path),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound
                    )
            }
        }
    }
}

/// One tab group: its tabs in strip order, which held the eye, and the
/// order the eye has been on them — Orca's `recentTabIds`, last is most
/// recent, which is what decides where the eye lands when a tab closes
/// (`pickNextActiveTab`, I18nProvider:50798).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StageGroup {
    pub tabs: Vec<StageTab>,
    #[serde(default)]
    pub active: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<usize>,
}

/// One worktree's stage: the split tree, the groups by leaf ordinal, and
/// the group that held the keyboard.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StageLayout {
    pub root: PaneNode,
    pub groups: Vec<StageGroup>,
    #[serde(default)]
    pub focused: usize,
}

impl StageLayout {
    /// The layout as the file will keep it, or `None` for one not worth
    /// keeping.
    ///
    /// Tabs that cannot come back are dropped first; a group that kept
    /// nothing folds out of the tree (`pruneTabGroupLayoutForGroups`), and
    /// focus falls back through the groups that still have tabs
    /// (`selectHydratedActiveGroupId`). A tree that disagrees with its own
    /// group count is a hand-edited file and is refused whole.
    pub fn normalized(self) -> Option<StageLayout> {
        if self.root.leaves() != self.groups.len()
            || self.groups.is_empty()
            || self.groups.len() > MAX_GROUPS
        {
            return None;
        }
        let kept_groups: Vec<Option<StageGroup>> = self
            .groups
            .into_iter()
            .map(|group| {
                let held = group.active;
                // Old index → new, so the recency order can follow the tabs
                // it names through the drop.
                let mut landed: Vec<Option<usize>> = Vec::new();
                let mut tabs: Vec<StageTab> = Vec::new();
                for mut tab in group.tabs.into_iter().take(MAX_TABS) {
                    if !tab.keeps() {
                        landed.push(None);
                        continue;
                    }
                    // The pin wins over the glance — a record wearing both
                    // is a hand edit, and pinning killed the glance the
                    // moment it happened (:58012).
                    if tab.pinned {
                        tab.preview = false;
                    }
                    // A draft belongs to an editor. On any other kind it is
                    // a hand edit — there is no field it could return to.
                    if tab.kind != "file" {
                        tab.draft = None;
                    }
                    landed.push(Some(tabs.len()));
                    tabs.push(tab);
                }
                if tabs.is_empty() {
                    return None;
                }
                // The active tab is revalidated by IDENTITY, not clamped:
                // Orca stores an id and checks it still exists. An ordinal
                // that merely still fits after a prune would land the eye
                // on whichever tab slid into the old seat.
                let active = landed.get(held).copied().flatten().unwrap_or(0);
                // Orca's `sanitizeRecentTabIds` (:50772): only tabs that
                // still exist, deduped from the end so the LATEST mention
                // wins, order kept — then the active tab is guaranteed a
                // seat (the hydration's own fallback push, :49766), so the
                // first close after a restore has somewhere to look.
                let mut seen = std::collections::HashSet::new();
                let mut reversed: Vec<usize> = Vec::new();
                for &old in group.recent.iter().rev() {
                    let Some(new) = landed.get(old).copied().flatten() else {
                        continue;
                    };
                    if seen.insert(new) {
                        reversed.push(new);
                    }
                }
                let mut recent: Vec<usize> = reversed.into_iter().rev().collect();
                if !recent.contains(&active) {
                    recent.push(active);
                }
                Some(StageGroup {
                    tabs,
                    active,
                    recent,
                })
            })
            .collect();
        let survivors: Vec<bool> = kept_groups.iter().map(Option::is_some).collect();
        let root = prune_leaves(self.root, &survivors, &mut 0)?;
        let groups: Vec<StageGroup> = kept_groups.into_iter().flatten().collect();
        // The stored focus, when its group survived WITH tabs — every kept
        // group has tabs here, so surviving is the whole test — else the
        // first group standing.
        let focused = renumbered(&survivors, self.focused).unwrap_or(0);
        Some(StageLayout {
            root: root.normalized(),
            groups,
            focused,
        })
    }
}

/// The tree minus the leaves whose groups kept nothing — a split that loses
/// a child is the other child, and `None` is a stage with nothing left.
fn prune_leaves(node: PaneNode, survivors: &[bool], at: &mut usize) -> Option<PaneNode> {
    match node {
        PaneNode::Leaf => {
            let keeps = survivors.get(*at).copied().unwrap_or(false);
            *at += 1;
            keeps.then_some(PaneNode::Leaf)
        }
        PaneNode::Split {
            direction,
            first,
            second,
            ratio,
        } => {
            let first = prune_leaves(*first, survivors, at);
            let second = prune_leaves(*second, survivors, at);
            match (first, second) {
                (Some(first), Some(second)) => Some(PaneNode::Split {
                    direction,
                    first: Box::new(first),
                    second: Box::new(second),
                    ratio,
                }),
                (Some(one), None) | (None, Some(one)) => Some(one),
                (None, None) => None,
            }
        }
    }
}

/// Where an ordinal lands after the dropped groups close ranks, or `None`
/// when its own group was dropped.
fn renumbered(survivors: &[bool], ordinal: usize) -> Option<usize> {
    if !survivors.get(ordinal).copied().unwrap_or(false) {
        return None;
    }
    Some(survivors[..ordinal].iter().filter(|kept| **kept).count())
}

/// Every worktree's stage, by the worktree's absolute path.
pub type Layouts = HashMap<String, StageLayout>;

/// What the file holds, or nothing — a missing or unreadable file is a
/// window that has never been closed, not an error.
pub fn read(file: &Path) -> Layouts {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Layouts::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Put one worktree's stage into `layouts`, normalised, pruning as it goes
/// — the same door shape as the pane layouts', for the same reasons.
pub fn store(layouts: &mut Layouts, worktree: String, layout: Option<StageLayout>) {
    layouts.retain(|path, _| Path::new(path).is_dir());
    match layout.and_then(StageLayout::normalized) {
        Some(kept) => {
            layouts.insert(worktree, kept);
        }
        None => {
            layouts.remove(&worktree);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_layout::SplitDirection;

    fn leaf() -> PaneNode {
        PaneNode::Leaf
    }

    fn split(first: PaneNode, second: PaneNode) -> PaneNode {
        PaneNode::Split {
            direction: SplitDirection::Horizontal,
            first: Box::new(first),
            second: Box::new(second),
            ratio: None,
        }
    }

    fn tab(kind: &str, path: Option<&str>) -> StageTab {
        StageTab {
            kind: kind.to_string(),
            path: path.map(str::to_string),
            preview: false,
            pinned: false,
            draft: None,
        }
    }

    /// The pin survives the trip, and a record wearing pin AND glance — a
    /// hand edit; pinning kills the glance live — restores as pinned only.
    #[test]
    fn a_pin_survives_and_beats_the_glance() {
        let layout = StageLayout {
            root: leaf(),
            groups: vec![StageGroup {
                tabs: vec![StageTab {
                    kind: "board".to_string(),
                    path: None,
                    preview: true,
                    pinned: true,
                    draft: None,
                }],
                active: 0,
                recent: Vec::new(),
            }],
            focused: 0,
        };
        let kept = layout.normalized().expect("keeps");
        assert!(kept.groups[0].tabs[0].pinned);
        assert!(!kept.groups[0].tabs[0].preview);
    }

    /// The transient rule and the path rule, together: a diff never comes
    /// back, a file that left the disk never comes back, and the argless
    /// kinds always do.
    #[test]
    fn a_stored_tab_comes_back_only_if_the_window_could_reopen_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("kept.rs");
        std::fs::write(&real, "here").expect("write");
        let layout = StageLayout {
            root: leaf(),
            groups: vec![StageGroup {
                tabs: vec![
                    tab("file", Some(real.to_str().expect("utf8"))),
                    tab("file", Some("/nowhere/gone.rs")),
                    tab("diff", None),
                    tab("imagediff", None),
                    tab("board", None),
                    tab("lane", None),
                ],
                active: 4,
                recent: Vec::new(),
            }],
            focused: 0,
        };
        let kept = layout.normalized().expect("keeps");
        let kinds: Vec<&str> = kept.groups[0]
            .tabs
            .iter()
            .map(|t| t.kind.as_str())
            .collect();
        assert_eq!(kinds, vec!["file", "board"]);
        // The active ordinal pointed at the BOARD, and the board survived —
        // the eye follows the tab through the drops, not the seat number.
        // (This clamped to 0 before 2026-08-09; Orca revalidates the stored
        // active by identity, so a prune must not hand the eye to whichever
        // tab slid into the old seat.)
        assert_eq!(kept.groups[0].active, 1);
    }

    /// A group that kept nothing folds out of the tree — the sibling IS the
    /// stage — and the focus renumbers around the fold.
    #[test]
    fn an_emptied_group_folds_and_focus_closes_ranks() {
        let layout = StageLayout {
            root: split(leaf(), split(leaf(), leaf())),
            groups: vec![
                StageGroup {
                    tabs: vec![tab("diff", None)],
                    active: 0,
                    recent: Vec::new(),
                },
                StageGroup {
                    tabs: vec![tab("board", None)],
                    active: 0,
                    recent: Vec::new(),
                },
                StageGroup {
                    tabs: vec![tab("changes", None)],
                    active: 0,
                    recent: Vec::new(),
                },
            ],
            focused: 2,
        };
        let kept = layout.normalized().expect("keeps");
        assert_eq!(kept.groups.len(), 2);
        assert!(matches!(kept.root, PaneNode::Split { .. }));
        assert_eq!(kept.root.leaves(), 2);
        assert_eq!(kept.focused, 1, "ordinal 2 closes ranks to 1");

        // The focused group itself emptied: focus falls to the first group
        // standing (`selectHydratedActiveGroupId`).
        let refocused = StageLayout {
            root: split(leaf(), leaf()),
            groups: vec![
                StageGroup {
                    tabs: vec![tab("vault", None)],
                    active: 0,
                    recent: Vec::new(),
                },
                StageGroup {
                    tabs: vec![tab("diff", None)],
                    active: 0,
                    recent: Vec::new(),
                },
            ],
            focused: 1,
        };
        let kept = refocused.normalized().expect("keeps");
        assert_eq!(kept.focused, 0);
        assert!(matches!(kept.root, PaneNode::Leaf));
    }

    /// A tree that disagrees with its own group count is a hand-edited file
    /// and is refused whole — and so is a stage with nothing left at all.
    #[test]
    fn a_file_that_disagrees_with_itself_is_refused_whole() {
        let mismatched = StageLayout {
            root: split(leaf(), leaf()),
            groups: vec![StageGroup {
                tabs: vec![tab("board", None)],
                active: 0,
                recent: Vec::new(),
            }],
            focused: 0,
        };
        assert_eq!(mismatched.normalized(), None);
        let empty = StageLayout {
            root: leaf(),
            groups: vec![StageGroup {
                tabs: vec![tab("diff", None)],
                active: 0,
                recent: Vec::new(),
            }],
            focused: 0,
        };
        assert_eq!(empty.normalized(), None);
    }

    /// The store door: normalising on the way in, an unkeepable layout
    /// removing the key, and dead checkouts swept — the pane layouts' own
    /// manners.
    #[test]
    fn the_store_normalises_and_an_empty_stage_removes_the_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worktree = dir.path().to_str().expect("utf8").to_string();
        let mut layouts = Layouts::new();
        store(
            &mut layouts,
            worktree.clone(),
            Some(StageLayout {
                root: leaf(),
                groups: vec![StageGroup {
                    tabs: vec![tab("board", None)],
                    active: 0,
                    recent: Vec::new(),
                }],
                focused: 0,
            }),
        );
        assert!(layouts.contains_key(&worktree));
        store(&mut layouts, worktree.clone(), None);
        assert!(!layouts.contains_key(&worktree));
    }

    /// The recency order follows the tabs it names through a drop: pruned
    /// tabs leave it, the surviving indices remap, the latest mention of a
    /// tab wins the dedup, and the active tab always ends up holding a seat
    /// — Orca's `sanitizeRecentTabIds` plus the hydration's fallback push.
    #[test]
    fn the_recency_order_survives_the_prune_remapped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "x").expect("write");
        let kept = file.to_str().expect("utf8");
        let layout = StageLayout {
            root: leaf(),
            groups: vec![StageGroup {
                // 0: gone from disk (pruned), 1: board, 2: file, 3: vault.
                tabs: vec![
                    tab("file", Some("/definitely/gone.rs")),
                    tab("board", None),
                    tab("file", Some(kept)),
                    tab("vault", None),
                ],
                active: 3,
                // Mentions the pruned tab, mentions the board twice — the
                // LATER mention wins — and never mentions the active vault.
                recent: vec![1, 0, 2, 1],
            }],
            focused: 0,
        };
        let held = layout.normalized().expect("kept");
        let group = &held.groups[0];
        // Tabs 1,2,3 survive as 0,1,2; active 3 → 2.
        assert_eq!(group.tabs.len(), 3);
        assert_eq!(group.active, 2);
        // recent [1,0,2,1]: the pruned 0 leaves; the earlier mention of 1
        // yields to its later one, so the kept order is old [2, 1] → new
        // [1, 0]; the active (new 2) takes its guaranteed seat at the end.
        assert_eq!(group.recent, vec![1, 0, 2]);

        // And a draft on anything but a file is shed as the hand edit it is.
        let mut edited = tab("board", None);
        edited.draft = Some("typed".into());
        let mut drafted = tab("file", Some(kept));
        drafted.draft = Some("kept typing".into());
        let layout = StageLayout {
            root: leaf(),
            groups: vec![StageGroup {
                tabs: vec![edited, drafted],
                active: 0,
                recent: Vec::new(),
            }],
            focused: 0,
        };
        let held = layout.normalized().expect("kept");
        assert_eq!(held.groups[0].tabs[0].draft, None);
        assert_eq!(held.groups[0].tabs[1].draft.as_deref(), Some("kept typing"));
    }

    /// Every UI-stored kind survives an actual store/file/read/normalise trip,
    /// including a missing file whose recoverable draft is the empty string.
    #[test]
    fn all_stored_kinds_and_an_empty_missing_draft_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let worktree = dir.path().to_str().expect("utf8").to_string();
        let existing = dir.path().join("document.txt");
        std::fs::write(&existing, "here").expect("existing file");
        let existing = existing.to_str().expect("utf8");
        let mut tabs: Vec<StageTab> = STAGE_PATH_KINDS
            .iter()
            .map(|kind| tab(kind, Some(existing)))
            .chain(STAGE_ARGLESS_KINDS.iter().map(|kind| tab(kind, None)))
            .collect();
        let missing = dir.path().join("missing.txt");
        let mut drafted = tab("file", Some(missing.to_str().expect("utf8")));
        drafted.draft = Some(String::new());
        tabs.push(drafted);

        let mut layouts = Layouts::new();
        store(
            &mut layouts,
            worktree.clone(),
            Some(StageLayout {
                root: leaf(),
                groups: vec![StageGroup {
                    tabs,
                    active: 0,
                    recent: Vec::new(),
                }],
                focused: 0,
            }),
        );
        let file = dir.path().join("stage-layouts.json");
        std::fs::write(
            &file,
            serde_json::to_vec(&layouts).expect("serialised layouts"),
        )
        .expect("stored layouts");
        let held = read(&file)
            .remove(&worktree)
            .and_then(StageLayout::normalized)
            .expect("restored layout");

        let expected: Vec<&str> = STAGE_PATH_KINDS
            .iter()
            .chain(STAGE_ARGLESS_KINDS)
            .copied()
            .chain(std::iter::once("file"))
            .collect();
        let kinds: Vec<&str> = held.groups[0]
            .tabs
            .iter()
            .map(|tab| tab.kind.as_str())
            .collect();
        assert_eq!(kinds, expected);
        assert_eq!(
            held.groups[0]
                .tabs
                .last()
                .and_then(|tab| tab.draft.as_deref()),
            Some("")
        );
        assert!(
            !missing.exists(),
            "normalising a draft created its missing file"
        );
    }

    /// A draft cannot turn an empty path, a directory, or a path-bearing
    /// argless kind into a restorable document.
    #[test]
    fn invalid_stage_paths_are_refused_even_when_they_carry_a_draft() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut empty = tab("file", Some(""));
        empty.draft = Some(String::new());
        let mut folder = tab("file", Some(dir.path().to_str().expect("utf8")));
        folder.draft = Some("typing".to_string());
        let layout = StageLayout {
            root: leaf(),
            groups: vec![StageGroup {
                tabs: vec![
                    empty,
                    folder,
                    tab("board", Some("not-an-argument")),
                    tab("board", None),
                ],
                active: 0,
                recent: Vec::new(),
            }],
            focused: 0,
        };

        let held = layout.normalized().expect("the valid board remains");
        assert_eq!(held.groups[0].tabs, vec![tab("board", None)]);
    }
}
