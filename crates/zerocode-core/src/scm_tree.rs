//! The changed files as their directory tree.
//!
//! Source Control lists paths, and a list is the right shape until a change
//! touches forty files across nine directories — at which point the thing a
//! person is looking for is a FOLDER, and the list makes them read every row
//! to find it. Orca's answer is a view mode (`sourceControlViewMode`), and
//! this is the rule half of it: build the tree, fold the chains nobody needs
//! to see, and flatten it back to rows the window can draw.
//!
//! ## What is measured, and against what
//!
//! `buildSourceControlTree`, `compactSourceControlTree` and
//! `flattenSourceControlTree` (`source-control-tree.ts`, 1.4.184), including
//! the key grammar the collapse state is stored under and the ordering rule —
//! directories before files, directories by name, files by the order the flat
//! list already uses.
//!
//! ## Why the compaction is not cosmetic
//!
//! A changed-file set is a thin slice of a repository, so its tree is mostly
//! corridors: `crates/zerocode-core/src/` with one file at the end is four
//! rows to say one thing. Folding a single-child chain into one row named
//! `crates/zerocode-core/src` is what VS Code does and what Orca copied, and
//! without it the tree view is longer than the list it replaced.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// One row of the flattened tree, as the window draws it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Row {
    /// A folder. `key` is what a collapse is remembered under.
    Directory {
        key: String,
        /// The folded chain, already joined — `a/b/c`.
        name: String,
        path: String,
        depth: usize,
        /// Files underneath it, at every depth.
        file_count: usize,
        /// Paths under it, so a bulk hand can act on exactly what is shown.
        paths: Vec<String>,
    },
    /// A changed file, carrying the index of the entry it came from so the
    /// window can draw the row it already knows how to draw.
    File {
        key: String,
        name: String,
        path: String,
        depth: usize,
        /// Index into the entry list this tree was built from.
        at: usize,
    },
}

impl Row {
    /// The key a collapse is remembered under, for either kind.
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Directory { key, .. } | Self::File { key, .. } => key,
        }
    }
}

/// A node while the tree is still being built.
#[derive(Debug)]
enum Node {
    Directory {
        name: String,
        path: String,
        children: Vec<Node>,
    },
    File {
        name: String,
        path: String,
        at: usize,
    },
}

/// Splits a path the way both sides of this window already do: forward
/// slashes, no empty segments, no `.` — a leading `./` is how a status line
/// spells "here" and it is not a directory.
fn segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

/// Inserts one path, creating the directories it needs.
fn insert(children: &mut Vec<Node>, parts: &[&str], prefix: &str, path: &str, at: usize) {
    let Some((head, rest)) = parts.split_first() else {
        return;
    };
    let here = if prefix.is_empty() {
        (*head).to_string()
    } else {
        format!("{prefix}/{head}")
    };
    if rest.is_empty() {
        children.push(Node::File {
            name: (*head).to_string(),
            path: path.to_string(),
            at,
        });
        return;
    }
    // An existing directory of this name takes the child; two files under one
    // folder must not build two folders.
    if let Some(Node::Directory { children: held, .. }) = children
        .iter_mut()
        .find(|child| matches!(child, Node::Directory { name, .. } if name == head))
    {
        insert(held, rest, &here, path, at);
        return;
    }
    let mut held = Vec::new();
    insert(&mut held, rest, &here, path, at);
    children.push(Node::Directory {
        name: (*head).to_string(),
        path: here,
        children: held,
    });
}

/// Directories first and by name, then files in the order they arrived.
///
/// The file order is the caller's: the flat list has already sorted them the
/// way this product sorts changed files, and re-sorting here would make the
/// two views disagree about which file comes first.
fn arrange(children: &mut [Node]) {
    children.sort_by(|left, right| match (left, right) {
        (Node::Directory { name: a, .. }, Node::Directory { name: b, .. }) => a.cmp(b),
        (Node::Directory { .. }, Node::File { .. }) => std::cmp::Ordering::Less,
        (Node::File { .. }, Node::Directory { .. }) => std::cmp::Ordering::Greater,
        (Node::File { at: a, .. }, Node::File { at: b, .. }) => a.cmp(b),
    });
    for child in children.iter_mut() {
        if let Node::Directory { children, .. } = child {
            arrange(children);
        }
    }
}

/// Folds a directory that holds exactly one directory and nothing else into
/// its child, joining the names.
fn compact(node: Node) -> Node {
    match node {
        Node::File { .. } => node,
        Node::Directory {
            mut name,
            mut path,
            mut children,
        } => {
            // Only a lone DIRECTORY child folds. A folder holding one file is
            // two facts — the folder and the file — and folding it would hide
            // the folder a bulk hand acts on.
            while children.len() == 1 && matches!(children[0], Node::Directory { .. }) {
                let Node::Directory {
                    name: child_name,
                    path: child_path,
                    children: grandchildren,
                } = children.remove(0)
                else {
                    unreachable!("checked by the guard above")
                };
                name = format!("{name}/{child_name}");
                path = child_path;
                children = grandchildren;
            }
            Node::Directory {
                name,
                path,
                children: children.into_iter().map(compact).collect(),
            }
        }
    }
}

/// Every file path under a node, in the order the rows would show them.
fn paths_under(node: &Node, into: &mut Vec<String>) {
    match node {
        Node::File { path, .. } => into.push(path.clone()),
        Node::Directory { children, .. } => {
            for child in children {
                paths_under(child, into);
            }
        }
    }
}

/// Walks the tree into rows, stopping at a folded directory.
fn walk(node: &Node, area: &str, depth: usize, folded: &BTreeSet<String>, into: &mut Vec<Row>) {
    match node {
        Node::File { name, path, at } => into.push(Row::File {
            key: format!("{area}::{path}"),
            name: name.clone(),
            path: path.clone(),
            depth,
            at: *at,
        }),
        Node::Directory {
            name,
            path,
            children,
        } => {
            let mut paths = Vec::new();
            paths_under(node, &mut paths);
            let key = format!("dir::{area}::{path}");
            let shut = folded.contains(&key);
            into.push(Row::Directory {
                key,
                name: name.clone(),
                path: path.clone(),
                depth,
                file_count: paths.len(),
                paths,
            });
            if shut {
                return;
            }
            for child in children {
                walk(child, area, depth + 1, folded, into);
            }
        }
    }
}

/// The rows one section shows in tree mode.
///
/// `paths` arrives in the order the flat list already decided, and `area`
/// names the section so two sections showing the same directory keep their
/// own collapse state — a conflict folder and a changed folder are the same
/// path and not the same row.
#[must_use]
pub fn rows(area: &str, paths: &[String], folded: &BTreeSet<String>) -> Vec<Row> {
    let mut roots: Vec<Node> = Vec::new();
    for (at, path) in paths.iter().enumerate() {
        let parts = segments(path);
        if parts.is_empty() {
            continue;
        }
        insert(&mut roots, &parts, "", path, at);
    }
    arrange(&mut roots);
    let roots: Vec<Node> = roots.into_iter().map(compact).collect();
    let mut out = Vec::new();
    for root in &roots {
        walk(root, area, 0, folded, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|one| (*one).to_string()).collect()
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Directory { name, depth, .. } => format!("{}📁 {name}", "  ".repeat(*depth)),
                Row::File { name, depth, .. } => format!("{}{name}", "  ".repeat(*depth)),
            })
            .collect()
    }

    /// Files become their folders, and the folders come first.
    #[test]
    fn the_paths_become_the_folders_that_hold_them() {
        let shown = rows(
            "unstaged",
            &paths(&["src/main.rs", "README.md", "src/lib.rs", "docs/plan.md"]),
            &BTreeSet::new(),
        );
        assert_eq!(
            names(&shown),
            vec![
                "📁 docs",
                "  plan.md",
                "📁 src",
                "  main.rs",
                "  lib.rs",
                "README.md",
            ],
            "directories come first and by name, files keep the order they arrived in"
        );
    }

    /// The file order is the CALLER's, never re-sorted here.
    ///
    /// The flat list has already decided how this product orders changed
    /// files; sorting again would make the two views disagree about which
    /// file comes first, which reads as one of them being wrong.
    #[test]
    fn the_file_order_belongs_to_the_list_that_came_in() {
        let shown = rows(
            "unstaged",
            &paths(&["src/z.rs", "src/a.rs", "src/m.rs"]),
            &BTreeSet::new(),
        );
        assert_eq!(names(&shown), vec!["📁 src", "  z.rs", "  a.rs", "  m.rs"]);
    }

    /// A corridor of single directories folds into one row.
    #[test]
    fn a_chain_of_lone_directories_becomes_one_row() {
        let shown = rows(
            "staged",
            &paths(&["crates/zerocode-core/src/lib.rs"]),
            &BTreeSet::new(),
        );
        assert_eq!(
            names(&shown),
            vec!["📁 crates/zerocode-core/src", "  lib.rs"],
            "four rows were spent saying one thing"
        );
        // The folded row keeps the DEEPEST path, because that is the folder a
        // bulk hand would act on.
        let Row::Directory { path, key, .. } = &shown[0] else {
            panic!("a directory")
        };
        assert_eq!(path, "crates/zerocode-core/src");
        assert_eq!(key, "dir::staged::crates/zerocode-core/src");
    }

    /// A folder holding one FILE does not fold — the folder is its own fact.
    #[test]
    fn a_folder_holding_one_file_keeps_its_row() {
        let shown = rows("unstaged", &paths(&["docs/plan.md"]), &BTreeSet::new());
        assert_eq!(names(&shown), vec!["📁 docs", "  plan.md"]);
    }

    /// The chain stops folding where the tree branches.
    #[test]
    fn the_fold_stops_where_the_tree_branches() {
        let shown = rows(
            "unstaged",
            &paths(&["a/b/c/one.rs", "a/b/d/two.rs"]),
            &BTreeSet::new(),
        );
        assert_eq!(
            names(&shown),
            vec!["📁 a/b", "  📁 c", "    one.rs", "  📁 d", "    two.rs"],
            "the shared corridor folds, the branch below it does not"
        );
    }

    /// A folded directory keeps its row and hides what is under it.
    #[test]
    fn a_folded_directory_shows_itself_and_nothing_below() {
        let all = paths(&["src/main.rs", "src/lib.rs", "README.md"]);
        let mut folded = BTreeSet::new();
        folded.insert("dir::unstaged::src".to_string());
        let shown = rows("unstaged", &all, &folded);
        assert_eq!(names(&shown), vec!["📁 src", "README.md"]);
        // Its count still speaks for what it holds, which is the whole point
        // of folding it.
        let Row::Directory {
            file_count, paths, ..
        } = &shown[0]
        else {
            panic!("a directory")
        };
        assert_eq!(*file_count, 2);
        assert_eq!(
            paths,
            &vec!["src/main.rs".to_string(), "src/lib.rs".to_string()]
        );
    }

    /// Two sections showing the same folder keep their own collapse.
    #[test]
    fn one_folder_in_two_sections_is_two_keys() {
        let shut = |area: &str| {
            let mut folded = BTreeSet::new();
            folded.insert(format!("dir::{area}::src"));
            rows(area, &paths(&["src/main.rs"]), &folded)
        };
        // Folding it in one section leaves the other standing open.
        assert_eq!(names(&shut("staged")), vec!["📁 src"]);
        let other = rows("unstaged", &paths(&["src/main.rs"]), &{
            let mut folded = BTreeSet::new();
            folded.insert("dir::staged::src".to_string());
            folded
        });
        assert_eq!(
            names(&other),
            vec!["📁 src", "  main.rs"],
            "a fold in one section closed the same folder in another"
        );
    }

    /// A file at the root has no folder, and a `./` prefix is not one.
    #[test]
    fn a_root_file_stands_alone() {
        let shown = rows(
            "untracked",
            &paths(&["./README.md", "LICENSE"]),
            &BTreeSet::new(),
        );
        assert_eq!(names(&shown), vec!["README.md", "LICENSE"]);
        let Row::File { path, at, key, .. } = &shown[0] else {
            panic!("a file")
        };
        // The path is carried through untouched — it is what a command is
        // given — and `at` points back at the row the list already knows.
        assert_eq!(path, "./README.md");
        assert_eq!(*at, 0);
        assert_eq!(key, "untracked::./README.md");
    }

    /// Nothing in, nothing out.
    #[test]
    fn an_empty_section_has_no_rows() {
        assert!(rows("staged", &[], &BTreeSet::new()).is_empty());
        assert!(rows("staged", &paths(&["", "."]), &BTreeSet::new()).is_empty());
    }
}
