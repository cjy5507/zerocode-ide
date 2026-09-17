//! Which agent row sits under which — the tree inside one card.
//!
//! Orca calls two different things "lineage" and they must not be confused.
//! One is **worktree lineage**: which workspace was cut from which, stored per
//! worktree and re-verified against instance ids on every render. The other —
//! this one — is **agent row lineage** (`useWorktreeAgentRows-BZzQmOQU.js`,
//! 1.4.164, :14-100): inside a single card, the agent sessions are indented
//! into a subagent tree, so five agents in one workspace read as "one
//! coordinator and four it started" rather than as five equals.
//!
//! `board.rs`'s own doc has named this as missing since the board was written
//! ("messages, subagent lineage, a review pill, a repo icon"). This is that
//! line, and only that line — nothing here knows what a worktree is.
//!
//! ## The one invariant
//!
//! **No row disappears.** Everything else here is in service of that. A row
//! whose parent is not in the list is promoted to a root; a row standing on a
//! cycle is cut loose and becomes a root; a row that claims itself as its own
//! parent is a root. Orca does the same (:52-66) and for the same reason: this
//! is a *view* over rows somebody is already relying on to see their agents.
//! A tree that silently swallows the one agent waiting on a person is worse
//! than no tree at all — the person cannot even tell there is something to
//! look for. Losing the indent is a cosmetic failure; losing the row is not.
//!
//! ## Where the edge comes from
//!
//! Orca resolves a row's parent in a fixed order (`resolveAgentRowParentPaneKey`):
//! the orchestration record's `parentPaneKey` first, then the terminal handle
//! of the terminal that created it, then the coordinator's handle — the last
//! two looked up through the pane list, because a terminal handle is not a
//! pane key. This module takes the answer, not the question: the caller has
//! already resolved a key, and what is left is the shape. Keeping the two
//! apart is what lets the shape be tested exhaustively without inventing an
//! orchestration database to test it against.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Where one row sits in the tree its neighbours make.
///
/// Orca's own four fields (:76-81), and they are four rather than one depth
/// because a tree drawn with an indent alone cannot say where a branch ends:
/// the elbow under the last child and the joint above the first are what make
/// three levels readable at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lineage {
    /// How many parents stand above this row. A root is `0`.
    pub depth: u16,
    /// First among the rows that share this row's parent.
    pub is_first_sibling: bool,
    /// Last among them.
    pub is_last_sibling: bool,
    /// How many rows name this one as their parent, **directly** — what the
    /// disclosure counts. Descendants further down are their own parents'
    /// business; a chip that said "9" over three visible children would be
    /// counting something the person cannot see.
    pub child_count: usize,
}

/// A row nobody has arranged: alone at the top, with nothing under it.
///
/// Orca's `ROOT_LINEAGE` (:76-81) and this module's [`Default`], which is what
/// a card carries before [`arrange`] has ever looked at it — a lone row is
/// both the first and the last of its siblings, because it is all of them.
pub const ROOT_LINEAGE: Lineage = Lineage {
    depth: 0,
    is_first_sibling: true,
    is_last_sibling: true,
    child_count: 0,
};

impl Default for Lineage {
    fn default() -> Self {
        ROOT_LINEAGE
    }
}

/// A row, as the arrangement needs to see it: its own name, and the name it
/// gives for its parent.
///
/// Borrowed rather than owned so the caller — which is holding the real rows
/// anyway — does not copy every id to ask a question about their order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row<'a> {
    /// This row's own name, the one a child would point at.
    pub key: &'a str,
    /// Who this row says started it. Empty is "nobody".
    pub parent: &'a str,
}

/// One row's place, once the tree has been settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    /// Which row this is, as an index into what was handed in — so nothing
    /// about the row itself has to travel through here.
    pub index: usize,
    pub lineage: Lineage,
}

/// Put the rows in tree order and say where each one sits.
///
/// The answer is a permutation: every input row comes back exactly once, a
/// parent always before its children, and rows that share a parent in the
/// order they arrived — so whatever the caller sorted by (the board sorts by
/// when a card last changed) still decides the order among siblings.
pub fn arrange(rows: &[Row<'_>]) -> Vec<Placed> {
    let count = rows.len();
    // Who answers to each name. First writer wins: two rows claiming one key
    // is a bug upstream, and pointing children at the first of them at least
    // leaves a tree rather than a fork that loses one of the two.
    let mut by_key: HashMap<&str, usize> = HashMap::with_capacity(count);
    for (index, row) in rows.iter().enumerate() {
        by_key.entry(row.key).or_insert(index);
    }

    // The claimed edges, before any of them are believed.
    let mut parent: Vec<Option<usize>> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            if row.parent.is_empty() {
                return None;
            }
            match by_key.get(row.parent).copied() {
                // A parent nobody in this list answers to is no parent: the
                // pane was closed, or it is a card in another column. The row
                // is promoted rather than dropped.
                Some(found) if found != index => Some(found),
                _ => None,
            }
        })
        .collect();

    // Break every cycle, at exactly one edge each.
    //
    // The walk reads the edges as they stand NOW, so cutting the first row
    // found on a ring is enough to take every other row on that same ring out
    // of danger — they are a chain hanging off the promoted one, and they keep
    // their parents. That is the point: a cycle is a bug in whoever recorded
    // the edges, and the repair should destroy as little as it can. Rows below
    // a cycle keep their parents too; they did nothing wrong.
    //
    // This is also what makes the arrangement total rather than bounded: after
    // this loop no ring can be left, so the descent below terminates because
    // the graph is a forest and not because a step limit stopped it.
    for index in 0..count {
        if stands_on_a_cycle(&parent, index) {
            parent[index] = None;
        }
    }

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut roots: Vec<usize> = Vec::new();
    for (index, above) in parent.iter().enumerate() {
        match above {
            Some(up) => children[*up].push(index),
            None => roots.push(index),
        }
    }

    // Depth first, parents before their children, siblings in the order they
    // arrived. Pushed in reverse so the stack pops them forwards.
    let mut placed: Vec<Placed> = Vec::with_capacity(count);
    let mut stack: Vec<(usize, u16, bool, bool)> = Vec::with_capacity(count);
    let last_root = roots.len().saturating_sub(1);
    for (spot, index) in roots.iter().enumerate().rev() {
        stack.push((*index, 0, spot == 0, spot == last_root));
    }
    while let Some((index, depth, first, last)) = stack.pop() {
        placed.push(Placed {
            index,
            lineage: Lineage {
                depth,
                is_first_sibling: first,
                is_last_sibling: last,
                child_count: children[index].len(),
            },
        });
        let mine = &children[index];
        let youngest = mine.len().saturating_sub(1);
        for (spot, child) in mine.iter().enumerate().rev() {
            stack.push((*child, depth.saturating_add(1), spot == 0, spot == youngest));
        }
    }
    placed
}

/// Does this row's own ancestry lead back to it?
///
/// Walks up at most once per row: a chain longer than the list has repeated
/// something, and if that something were this row we would already have said
/// so. The bound is not a guess at a maximum depth — it is what stops a cycle
/// somewhere ABOVE this row from spinning here forever.
fn stands_on_a_cycle(parent: &[Option<usize>], start: usize) -> bool {
    let mut at = parent[start];
    for _ in 0..parent.len() {
        match at {
            None => return false,
            Some(one) if one == start => return true,
            Some(one) => at = parent[one],
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(key: &'a str, parent: &'a str) -> Row<'a> {
        Row { key, parent }
    }

    /// What comes back, keyed, so a test can read the shape rather than the
    /// indices.
    fn shape<'a>(rows: &[Row<'a>]) -> Vec<(&'a str, Lineage)> {
        arrange(rows)
            .into_iter()
            .map(|one| (rows[one.index].key, one.lineage))
            .collect()
    }

    /// Every row that went in comes out, once, whatever it claimed.
    ///
    /// This is the invariant the whole module exists for, so it is asked of
    /// every shape at once rather than trusted to the tests below: an orphan,
    /// a two-row cycle, a self-parent, a row hanging under a cycle, a row
    /// pointing at a name nobody has, and an ordinary child.
    #[test]
    fn no_row_ever_disappears() {
        let rows = [
            row("a", ""),
            row("b", "a"),
            row("orphan", "gone"),
            row("self", "self"),
            row("loop-1", "loop-2"),
            row("loop-2", "loop-1"),
            row("under-loop", "loop-1"),
            row("empty-name", ""),
        ];
        let drawn = shape(&rows);
        assert_eq!(drawn.len(), rows.len(), "{drawn:?}");
        let mut names: Vec<&str> = drawn.iter().map(|(key, _)| *key).collect();
        names.sort_unstable();
        let mut expected: Vec<&str> = rows.iter().map(|one| one.key).collect();
        expected.sort_unstable();
        assert_eq!(names, expected);

        // And the empty list is a list, not a panic.
        assert!(arrange(&[]).is_empty());
    }

    /// A row whose parent is not in the list is a root, not a casualty.
    ///
    /// This is the common case, not the exotic one: the parent's pane was
    /// closed, or the parent is a card the column filter kept out. Either way
    /// the child is still a running agent somebody has to see.
    #[test]
    fn an_orphan_is_promoted_rather_than_dropped() {
        // A real root stands FIRST on purpose: an orphan quietly adopted by
        // whichever row happened to be handed in first would still be drawn,
        // and a test whose orphan is that row cannot tell the two apart.
        let rows = [row("host", ""), row("child", "vanished"), row("other", "")];
        let drawn = shape(&rows);
        assert_eq!(
            drawn
                .iter()
                .map(|(key, one)| (*key, one.depth))
                .collect::<Vec<_>>(),
            vec![("host", 0), ("child", 0), ("other", 0)]
        );
        assert!(
            drawn.iter().all(|(_, one)| one.child_count == 0),
            "an orphan was adopted by a row it never named: {drawn:?}"
        );
        // Three roots beside each other: the first is first, the last is last.
        assert!(drawn[0].1.is_first_sibling && !drawn[0].1.is_last_sibling);
        assert!(!drawn[1].1.is_first_sibling && !drawn[1].1.is_last_sibling);
        assert!(!drawn[2].1.is_first_sibling && drawn[2].1.is_last_sibling);
    }

    /// A row that names itself is a root — the edge is dropped before anything
    /// walks it, so this cannot become an infinite indent.
    #[test]
    fn a_row_cannot_be_its_own_parent() {
        let drawn = shape(&[row("me", "me")]);
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].1, ROOT_LINEAGE);
    }

    /// A cycle is broken at ONE edge — the first row on the ring becomes a
    /// root and the rest of the ring hangs off it as an ordinary chain.
    ///
    /// Cutting every member would also be safe, and it would throw away three
    /// true edges to repair one false one. The repair is deterministic because
    /// it walks in the caller's order, so two paints of the same rows draw the
    /// same tree — a ring that reshuffled itself every second would look like
    /// the agents were moving.
    #[test]
    fn a_cycle_is_broken_at_one_edge_and_nothing_under_it_is_lost() {
        let rows = [
            row("a", "c"),
            row("b", "a"),
            row("c", "b"),
            row("leaf", "c"),
        ];
        let drawn = shape(&rows);
        assert_eq!(
            drawn
                .iter()
                .map(|(key, one)| (*key, one.depth))
                .collect::<Vec<_>>(),
            vec![("a", 0), ("b", 1), ("c", 2), ("leaf", 3)]
        );
        // Same rows, rotated: whichever is handed in first is the one promoted,
        // and the answer is still a tree with every row in it.
        let rotated = shape(&[
            row("c", "b"),
            row("a", "c"),
            row("b", "a"),
            row("leaf", "c"),
        ]);
        assert_eq!(
            rotated
                .iter()
                .map(|(key, one)| (*key, one.depth))
                .collect::<Vec<_>>(),
            vec![("c", 0), ("a", 1), ("b", 2), ("leaf", 1)]
        );
    }

    /// Depth counts parents, siblings keep the order they arrived in, and a
    /// parent is always drawn before its children.
    #[test]
    fn depth_and_sibling_flags_describe_the_drawn_tree() {
        let rows = [
            row("root", ""),
            row("first", "root"),
            row("second", "root"),
            row("deep", "second"),
            row("third", "root"),
        ];
        let drawn = shape(&rows);
        assert_eq!(
            drawn.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            vec!["root", "first", "second", "deep", "third"],
            "a parent was drawn after one of its children"
        );
        assert_eq!(
            drawn.iter().map(|(_, one)| one.depth).collect::<Vec<_>>(),
            vec![0, 1, 1, 2, 1]
        );
        assert_eq!(
            drawn
                .iter()
                .map(|(_, one)| (one.is_first_sibling, one.is_last_sibling))
                .collect::<Vec<_>>(),
            vec![
                // The only root.
                (true, true),
                // first / second / third, with `deep` an only child between.
                (true, false),
                (false, false),
                (true, true),
                (false, true),
            ]
        );
        // The chip counts direct children only: `root` has three, not four.
        assert_eq!(drawn[0].1.child_count, 3);
        assert_eq!(drawn[2].1.child_count, 1);
        assert_eq!(drawn[3].1.child_count, 0);
    }

    /// Siblings arrive in the caller's order, and that order is the caller's
    /// business — the board has already sorted by when each card last changed,
    /// and re-sorting here would quietly overrule it.
    #[test]
    fn siblings_keep_the_order_they_arrived_in() {
        let forward = shape(&[row("p", ""), row("x", "p"), row("y", "p")]);
        assert_eq!(
            forward.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            vec!["p", "x", "y"]
        );
        let backward = shape(&[row("p", ""), row("y", "p"), row("x", "p")]);
        assert_eq!(
            backward.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            vec!["p", "y", "x"]
        );
    }

    /// Two rows with one name is a bug upstream; it must not become a lost row
    /// or a hang here.
    #[test]
    fn a_repeated_key_still_draws_both_rows() {
        let rows = [row("twin", ""), row("twin", ""), row("kid", "twin")];
        let drawn = shape(&rows);
        assert_eq!(drawn.len(), 3);
        assert_eq!(
            drawn.iter().filter(|(_, one)| one.depth == 1).count(),
            1,
            "the child attached to both twins or to neither: {drawn:?}"
        );
    }

    /// The default is the root constant, not a zeroed struct: a card the
    /// arrangement has not touched is a row standing alone, and a row standing
    /// alone is the first and the last of its siblings.
    #[test]
    fn the_default_lineage_is_the_root_one() {
        assert_eq!(Lineage::default(), ROOT_LINEAGE);
        // Spelled as the whole value rather than as four claims about it: the
        // constant is what an unarranged row wears, and "alone at the top" is
        // one fact, not four.
        assert_eq!(
            ROOT_LINEAGE,
            Lineage {
                depth: 0,
                is_first_sibling: true,
                is_last_sibling: true,
                child_count: 0,
            }
        );
    }

    /// A long chain does not blow the stack or the depth field, and every row
    /// on it still comes back.
    #[test]
    fn a_deep_chain_stays_a_chain() {
        let names: Vec<String> = (0..500).map(|one| format!("n{one}")).collect();
        let rows: Vec<Row<'_>> = names
            .iter()
            .enumerate()
            .map(|(index, key)| Row {
                key,
                parent: if index == 0 {
                    ""
                } else {
                    names[index - 1].as_str()
                },
            })
            .collect();
        let drawn = arrange(&rows);
        assert_eq!(drawn.len(), names.len());
        assert_eq!(drawn[0].lineage.depth, 0);
        assert_eq!(drawn[499].lineage.depth, 499);
    }
}
