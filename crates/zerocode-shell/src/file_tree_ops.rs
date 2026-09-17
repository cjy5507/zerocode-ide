//! 파일 트리 우클릭이 손대는 것들의 순수한 반 (P0-15 후반).
//!
//! Orca의 트리 우클릭(`file-explorer-row-context-menu.tsx:150-309`)은 새
//! 파일·폴더, 복제, 이름 바꾸기, 삭제까지 파일시스템에 손을 댄다. 손대는
//! 명령은 main.rs의 tauri 껍데기가 들지만, **판단**은 전부 여기 산다 —
//! 경로가 워크트리 담장 안인지, 복제본이 어떤 이름을 받는지, 이름이
//! 파일명으로 성립하는지. resume_watch·last_status와 같은 이유의 분리다:
//! 계약은 디스크 없이 시험할 수 있어야 한다.

use std::path::{Component, Path, PathBuf};

/// 트리가 내미는 이름 하나가 파일명으로 성립하는가 — 구분자·순회를 실은
/// 이름은 담장(root 검증)에 닿기 전에 여기서 거절된다. Orca의 rename
/// 입력도 한 조각 이름만 받는다(트리 행 인라인 편집).
pub fn valid_leaf_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\u{0}')
}

/// 요청된 경로가 이 워크트리의 담장 안인가.
///
/// 존재하는 조상까지 canonicalize해 비교한다 — 새로 만들 파일은 아직 없어
/// 스스로는 canonicalize가 안 되지만, `..`와 심링크로 담장을 걷어 나가는
/// 것은 조상의 실경로에서 이미 드러난다(reveal_skill의 양쪽-정규화 원칙,
/// main.rs).
pub fn inside_root(root: &Path, asked: &Path) -> bool {
    let Ok(real_root) = root.canonicalize() else {
        return false;
    };
    // `..`를 문자로 품은 경로는 정규화 전에 거절 — 존재하지 않는 꼬리의
    // `..`는 canonicalize가 접어 주지 않는다.
    if asked.components().any(|part| part == Component::ParentDir) {
        return false;
    }
    let mut probe = asked.to_path_buf();
    loop {
        match probe.canonicalize() {
            Ok(real) => return real.starts_with(&real_root),
            Err(_) => match probe.parent() {
                Some(parent) if parent != probe => probe = parent.to_path_buf(),
                _ => return false,
            },
        }
    }
}

/// 복제본의 이름 — Finder와 Orca가 같이 쓰는 규칙: `name copy.ext`,
/// 자리가 차 있으면 `name copy 2.ext`부터 올라간다. `exists`를 인자로
/// 받아 디스크 없이 시험한다.
pub fn duplicate_name(path: &Path, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let dir = path.parent()?;
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let extension = path
        .extension()
        .map(|tail| format!(".{}", tail.to_string_lossy()));
    let dressed = |label: &str| {
        let tail = extension.as_deref().unwrap_or("");
        dir.join(format!("{stem} {label}{tail}"))
    };
    let first = dressed("copy");
    if !exists(&first) {
        return Some(first);
    }
    // 상한은 관습이 아니라 안전벨트다: 999개의 사본이 있는 폴더는 이 규칙이
    // 풀 문제가 아니다.
    (2..1000)
        .map(|n| dressed(&format!("copy {n}")))
        .find(|candidate| !exists(candidate))
}

use crate::explorer_policy::ExplorerPolicy;
use crate::file_tree_io::{self, Identity, TrashReceipt};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Origin {
    Human,
    Agent { agent: String, pane: String },
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OpKind {
    Rename,
    Move,
    Create,
    Duplicate,
    Trash,
}

/// A path operation is one user's gesture, even when it moves several files.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TreeOp {
    pub kind: OpKind,
    pub from: Vec<PathBuf>,
    pub to: Vec<PathBuf>,
    pub by: Origin,
    pub count: usize,
}

#[derive(Clone, Debug)]
enum Step {
    Move {
        from: PathBuf,
        to: PathBuf,
        identity: Identity,
    },
    Trash {
        path: PathBuf,
        identity: Identity,
    },
    Restore {
        receipt: TrashReceipt,
        identity: Identity,
    },
}
impl Step {
    fn check(&self, root: &Path) -> Result<(), String> {
        match self {
            Self::Move { from, to, identity } => {
                checked_path(root, from)?;
                checked_path(root, to)?;
                identity.check(from)?;
                file_tree_io::vacant(to)?;
                if to.starts_with(from) || !to.parent().is_some_and(Path::is_dir) {
                    return Err("tree.invalidMove".into());
                }
            }
            Self::Trash { path, identity } => {
                checked_path(root, path)?;
                identity.check(path)?;
            }
            Self::Restore { receipt, identity } => {
                checked_path(root, &receipt.original)?;
                receipt.check(identity)?;
            }
        }
        Ok(())
    }
    fn apply(&self) -> Result<Self, String> {
        match self {
            Self::Move { from, to, identity } => {
                file_tree_io::move_no_replace(from, to)?;
                Ok(Self::Move {
                    from: to.clone(),
                    to: from.clone(),
                    identity: identity.clone(),
                })
            }
            Self::Trash { path, identity } => Ok(Self::Restore {
                receipt: file_tree_io::trash_path(path)?,
                identity: identity.clone(),
            }),
            Self::Restore { receipt, identity } => {
                receipt.restore()?;
                Ok(Self::Trash {
                    path: receipt.original.clone(),
                    identity: identity.clone(),
                })
            }
        }
    }
}

struct FailedSteps {
    error: String,
    recovery: Vec<Step>,
}

fn apply_steps(root: &Path, steps: &[Step]) -> Result<Vec<Step>, FailedSteps> {
    for step in steps {
        step.check(root).map_err(|error| FailedSteps {
            error,
            recovery: Vec::new(),
        })?;
    }
    let mut inverse = Vec::new();
    for step in steps {
        match step.apply() {
            Ok(reverse) => inverse.push(reverse),
            Err(error) => {
                let mut recovery = Vec::new();
                for reverse in inverse.into_iter().rev() {
                    if reverse.check(root).and_then(|()| reverse.apply()).is_err() {
                        recovery.push(reverse);
                    }
                }
                return Err(FailedSteps {
                    error: if recovery.is_empty() {
                        error
                    } else {
                        "tree.rollbackFailed".into()
                    },
                    recovery,
                });
            }
        }
    }
    inverse.reverse();
    Ok(inverse)
}

#[derive(Clone)]
struct Entry {
    op: TreeOp,
    steps: Vec<Step>,
}
#[derive(Default)]
struct Stacks {
    cap: usize,
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
}

/// Session-owned history. Roots are canonical keys and never share stacks.
#[derive(Default)]
pub(crate) struct History {
    roots: HashMap<PathBuf, Stacks>,
}

pub(crate) fn checked_path(root: &Path, asked: &Path) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|_| "tree.invalidMove")?;
    let path = root.join(asked);
    if !inside_root(&root, &path)
        || path == root
        || path.canonicalize().is_ok_and(|path| path == root)
    {
        return Err("tree.invalidMove".into());
    }
    // Canonicalize the parent, not the leaf: moving a symlink means moving
    // its directory entry. This also removes path aliases from group checks.
    let parent = path
        .parent()
        .ok_or("tree.invalidMove")?
        .canonicalize()
        .map_err(|_| "tree.invalidMove")?;
    Ok(parent.join(path.file_name().ok_or("tree.invalidMove")?))
}

impl History {
    fn record(&mut self, root: &Path, entry: Entry, cap: usize) {
        let stack = self.roots.entry(root.to_path_buf()).or_default();
        stack.cap = cap;
        stack.redo.clear();
        stack.undo.push_back(entry);
        while stack.undo.len() > cap {
            stack.undo.pop_front();
        }
    }

    pub(crate) fn relocate(
        &mut self,
        root: &Path,
        pairs: &[(PathBuf, PathBuf)],
        kind: OpKind,
        by: Origin,
        policy: &ExplorerPolicy,
    ) -> Result<TreeOp, String> {
        let root = root.canonicalize().map_err(|_| "tree.invalidMove")?;
        if pairs.is_empty() || pairs.len() > policy.move_cap {
            return Err("tree.tooMany".into());
        }
        let mut steps = Vec::new();
        let mut from = Vec::new();
        let mut to = Vec::new();
        for (source, target) in pairs {
            let source = checked_path(&root, source)?;
            let target = checked_path(&root, target)?;
            if from
                .iter()
                .any(|held: &PathBuf| source.starts_with(held) || held.starts_with(&source))
                || to.contains(&target)
            {
                return Err("tree.invalidMove".into());
            }
            steps.push(Step::Move {
                identity: Identity::read(&source)?,
                from: source.clone(),
                to: target.clone(),
            });
            from.push(source);
            to.push(target);
        }
        let op = TreeOp {
            kind,
            from,
            to,
            by,
            count: steps.len(),
        };
        self.perform(&root, op, &steps, policy.history_cap)
    }

    fn perform(
        &mut self,
        root: &Path,
        op: TreeOp,
        steps: &[Step],
        cap: usize,
    ) -> Result<TreeOp, String> {
        match apply_steps(root, steps) {
            Ok(inverse) => {
                self.record(
                    root,
                    Entry {
                        op: op.clone(),
                        steps: inverse,
                    },
                    cap,
                );
                Ok(op)
            }
            Err(failure) => {
                if !failure.recovery.is_empty() {
                    self.record(
                        root,
                        Entry {
                            op,
                            steps: failure.recovery,
                        },
                        cap,
                    );
                }
                Err(failure.error)
            }
        }
    }

    pub(crate) fn move_group(
        &mut self,
        root: &Path,
        paths: &[PathBuf],
        dir: &Path,
        by: Origin,
        policy: &ExplorerPolicy,
    ) -> Result<TreeOp, String> {
        if paths.len() > policy.move_cap {
            return Err("tree.tooMany".into());
        }
        let dir = root
            .join(dir)
            .canonicalize()
            .map_err(|_| "tree.invalidMove")?;
        if !dir.is_dir() || !inside_root(root, &dir) {
            return Err("tree.invalidMove".into());
        }
        let paths = top_level_paths(root, paths)?;
        let pairs = paths
            .into_iter()
            .map(|path| {
                let to = dir.join(path.file_name().ok_or("tree.invalidMove")?);
                Ok((path, to))
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.relocate(root, &pairs, OpKind::Move, by, policy)
    }

    pub(crate) fn trash(
        &mut self,
        root: &Path,
        paths: &[PathBuf],
        policy: &ExplorerPolicy,
    ) -> Result<TreeOp, String> {
        let root = root.canonicalize().map_err(|_| "tree.invalidMove")?;
        if paths.is_empty() || paths.len() > policy.move_cap {
            return Err("tree.tooMany".into());
        }
        let paths = top_level_paths(&root, paths)?;
        let steps = paths
            .iter()
            .map(|path| {
                Ok(Step::Trash {
                    identity: Identity::read(path)?,
                    path: path.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let op = TreeOp {
            kind: OpKind::Trash,
            count: paths.len(),
            from: paths,
            to: Vec::new(),
            by: Origin::Human,
        };
        self.perform(&root, op, &steps, policy.history_cap)
    }

    pub(crate) fn created(
        &mut self,
        root: &Path,
        path: &Path,
        kind: OpKind,
        policy: &ExplorerPolicy,
    ) -> Result<TreeOp, String> {
        let root = root.canonicalize().map_err(|_| "tree.invalidMove")?;
        let path = checked_path(&root, path)?;
        let identity = Identity::read(&path)?;
        let op = TreeOp {
            kind,
            from: Vec::new(),
            to: vec![path.clone()],
            by: Origin::Human,
            count: 1,
        };
        self.record(
            &root,
            Entry {
                op: op.clone(),
                steps: vec![Step::Trash { path, identity }],
            },
            policy.history_cap,
        );
        Ok(op)
    }

    /// Only an observed successful path change with the same filesystem
    /// identity is admitted; Write/Edit tool activity never calls this.
    pub(crate) fn observed(
        &mut self,
        root: &Path,
        pairs: &[(PathBuf, PathBuf, Identity)],
        by: Origin,
        policy: &ExplorerPolicy,
    ) -> Result<TreeOp, String> {
        let root = root.canonicalize().map_err(|_| "tree.invalidMove")?;
        if pairs.is_empty() || pairs.len() > policy.move_cap {
            return Err("tree.tooMany".into());
        }
        let mut steps = Vec::new();
        for (from, to, identity) in pairs.iter().rev() {
            let from = checked_path(&root, from)?;
            let to = checked_path(&root, to)?;
            file_tree_io::vacant(&from)?;
            identity.check(&to)?;
            steps.push(Step::Move {
                from: to,
                to: from,
                identity: identity.clone(),
            });
        }
        let op = TreeOp {
            kind: OpKind::Move,
            from: pairs.iter().map(|p| p.0.clone()).collect(),
            to: pairs.iter().map(|p| p.1.clone()).collect(),
            by,
            count: pairs.len(),
        };
        self.record(
            &root,
            Entry {
                op: op.clone(),
                steps,
            },
            policy.history_cap,
        );
        Ok(op)
    }

    pub(crate) fn undo(&mut self, root: &Path) -> Result<TreeOp, String> {
        self.replay(root, false)
    }
    pub(crate) fn redo(&mut self, root: &Path) -> Result<TreeOp, String> {
        self.replay(root, true)
    }
    fn replay(&mut self, root: &Path, redo: bool) -> Result<TreeOp, String> {
        let root = root.canonicalize().map_err(|_| "tree.invalidMove")?;
        let stack = self.roots.get_mut(&root).ok_or("tree.emptyHistory")?;
        let entry = if redo {
            stack.redo.pop()
        } else {
            stack.undo.pop_back()
        }
        .ok_or("tree.emptyHistory")?;
        match apply_steps(&root, &entry.steps) {
            Ok(inverse) => {
                let op = entry.op;
                let next = Entry {
                    op: op.clone(),
                    steps: inverse,
                };
                if redo {
                    stack.undo.push_back(next);
                } else {
                    stack.redo.push(next);
                }
                Ok(op)
            }
            Err(failure) => {
                let op = entry.op.clone();
                if redo {
                    stack.redo.push(entry);
                } else {
                    stack.undo.push_back(entry);
                }
                if !failure.recovery.is_empty() {
                    stack.undo.push_back(Entry {
                        op,
                        steps: failure.recovery,
                    });
                    while stack.undo.len() > stack.cap {
                        stack.undo.pop_front();
                    }
                }
                Err(failure.error)
            }
        }
    }
}

fn top_level_paths(root: &Path, paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut paths = paths
        .iter()
        .map(|path| checked_path(root, path))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    paths.dedup();
    let mut kept: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !kept.iter().any(|parent| path.starts_with(parent)) {
            kept.push(path);
        }
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    impl History {
        fn rename(
            &mut self,
            root: &Path,
            from: &Path,
            to: &Path,
            cap: usize,
        ) -> Result<TreeOp, String> {
            self.relocate(
                root,
                &[(from.into(), to.into())],
                OpKind::Rename,
                Origin::Human,
                &ExplorerPolicy {
                    history_cap: cap,
                    ..Default::default()
                },
            )
        }
        fn move_paths(
            &mut self,
            root: &Path,
            paths: &[PathBuf],
            dir: &Path,
            cap: usize,
        ) -> Result<TreeOp, String> {
            self.move_group(
                root,
                paths,
                dir,
                Origin::Human,
                &ExplorerPolicy {
                    history_cap: cap,
                    ..Default::default()
                },
            )
        }
    }
    use std::collections::HashSet;

    #[test]
    fn tree_history_rename_undo_redo_preserves_content_edits() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        let from = root.join("before");
        let to = root.join("after");
        std::fs::write(&from, "first").unwrap();
        let mut history = History::default();
        history.rename(root, &from, &to, 3).unwrap();
        std::fs::write(&to, "changed contents").unwrap();
        history.undo(root).unwrap();
        assert_eq!(std::fs::read_to_string(&from).unwrap(), "changed contents");
        history.redo(root).unwrap();
        assert!(to.exists() && !from.exists());
    }

    #[test]
    fn tree_history_undo_refuses_an_intervening_file_and_keeps_its_entry() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        let from = root.join("before");
        let to = root.join("after");
        std::fs::write(&from, "mine").unwrap();
        let mut history = History::default();
        history.rename(root, &from, &to, 3).unwrap();
        std::fs::write(&from, "someone else's").unwrap();
        assert!(history.undo(root).is_err());
        assert_eq!(std::fs::read_to_string(&from).unwrap(), "someone else's");
        std::fs::remove_file(&from).unwrap();
        history.undo(root).unwrap();
        assert_eq!(std::fs::read_to_string(&from).unwrap(), "mine");
    }

    #[test]
    fn tree_history_group_move_is_one_undo_and_preflights_all_conflicts() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        let paths = [root.join("a"), root.join("b")];
        let dest = root.join("dest");
        std::fs::create_dir(&dest).unwrap();
        for path in &paths {
            std::fs::write(path, "mine").unwrap();
        }
        let mut history = History::default();
        history.move_paths(root, &paths, &dest, 3).unwrap();
        std::fs::write(&paths[1], "someone else's").unwrap();
        assert!(history.undo(root).is_err());
        assert!(!paths[0].exists() && dest.join("a").exists());
        std::fs::remove_file(&paths[1]).unwrap();
        history.undo(root).unwrap();
        assert!(paths.iter().all(|path| path.exists()));
        assert!(history.undo(root).is_err());
    }

    #[test]
    fn tree_history_drops_oldest_and_isolates_worktrees() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        let mut history = History::default();
        std::fs::write(root.join("a"), "mine").unwrap();
        for (from, to) in [("a", "b"), ("b", "c"), ("c", "d")] {
            history
                .rename(root, &root.join(from), &root.join(to), 2)
                .unwrap();
        }
        let other = tempfile::tempdir().unwrap();
        assert!(history.undo(other.path()).is_err());
        history.undo(root).unwrap();
        history.undo(root).unwrap();
        assert!(history.undo(root).is_err());
        assert!(root.join("b").exists());
    }

    #[test]
    fn tree_history_trash_and_creation_have_exact_restore_receipts() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        let path = root.join("undo-trash-fixture");
        std::fs::write(&path, "kept").unwrap();
        let mut history = History::default();
        let policy = ExplorerPolicy::default();
        history
            .trash(root, std::slice::from_ref(&path), &policy)
            .unwrap();
        assert!(!path.exists());
        std::fs::write(&path, "occupied").unwrap();
        assert!(history.undo(root).is_err());
        std::fs::remove_file(&path).unwrap();
        history.undo(root).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept");
        history.redo(root).unwrap();
        history.undo(root).unwrap();
        history
            .created(root, &path, OpKind::Create, &policy)
            .unwrap();
        history.undo(root).unwrap();
        assert!(!path.exists());
        history.redo(root).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "kept");
    }

    #[test]
    fn tree_history_refuses_replacements_roots_and_duplicate_targets() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        let mut history = History::default();
        std::fs::write(root.join("a"), "mine").unwrap();
        history
            .rename(root, &root.join("a"), &root.join("b"), 3)
            .unwrap();
        std::fs::rename(root.join("b"), root.join("kept-elsewhere")).unwrap();
        std::fs::write(root.join("b"), "someone else's").unwrap();
        assert!(history.undo(root).is_err());
        assert!(
            history
                .trash(root, &[root.to_path_buf()], &ExplorerPolicy::default())
                .is_err()
        );
        std::fs::create_dir(root.join("dest")).unwrap();
        let pairs = [
            (root.join("b"), root.join("dest/same")),
            (root.join("kept-elsewhere"), root.join("dest/same")),
        ];
        assert!(
            history
                .relocate(
                    root,
                    &pairs,
                    OpKind::Move,
                    Origin::Human,
                    &ExplorerPolicy::default()
                )
                .is_err()
        );
        assert!(root.join("b").exists() && root.join("kept-elsewhere").exists());
    }

    #[test]
    fn tree_history_new_operations_clear_redo_and_parent_selection_moves_once() {
        let yard = tempfile::tempdir().unwrap();
        let root = yard.path();
        std::fs::create_dir_all(root.join("dir")).unwrap();
        std::fs::create_dir_all(root.join("dest")).unwrap();
        std::fs::write(root.join("dir/a"), "mine").unwrap();
        let mut history = History::default();
        let moved = history
            .move_paths(
                root,
                &[root.join("dir"), root.join("dir/a")],
                &root.join("dest"),
                3,
            )
            .unwrap();
        assert_eq!(moved.count, 1);
        history.undo(root).unwrap();
        history
            .rename(root, &root.join("dir/a"), &root.join("dir/b"), 3)
            .unwrap();
        assert!(history.redo(root).is_err());
    }

    /// 이름 한 조각의 담장: 구분자·순회·NUL은 파일명이 아니다.
    #[test]
    fn a_leaf_name_carries_no_road() {
        assert!(valid_leaf_name("notes.md"));
        assert!(valid_leaf_name("한글 이름.txt"));
        for bad in ["", ".", "..", "a/b", "a\\b", "a\u{0}b"] {
            assert!(!valid_leaf_name(bad), "{bad:?} slipped the fence");
        }
    }

    /// 담장 판정: 안은 안이고, `..`와 심링크는 걷어 나가지 못한다.
    #[test]
    fn the_fence_holds_against_dotdot_and_symlinks() {
        let yard = tempfile::tempdir().expect("root");
        let root = yard.path();
        std::fs::create_dir(root.join("inner")).expect("inner");
        assert!(inside_root(root, &root.join("inner/new.txt")));
        assert!(inside_root(root, &root.join("brand-new-dir/deeper/file")));
        assert!(!inside_root(root, &root.join("inner/../../outside.txt")));
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        assert!(!inside_root(root, &elsewhere.path().join("far.txt")));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(elsewhere.path(), root.join("gate")).expect("symlink");
            assert!(
                !inside_root(root, &root.join("gate/steal.txt")),
                "a symlink walked the fence"
            );
        }
    }

    /// 복제 이름은 Finder의 사다리다: copy, copy 2, copy 3…
    #[test]
    fn a_duplicate_climbs_the_finder_ladder() {
        let taken: HashSet<PathBuf> = [
            PathBuf::from("/w/a copy.rs"),
            PathBuf::from("/w/a copy 2.rs"),
        ]
        .into();
        let exists = |candidate: &Path| taken.contains(candidate);
        assert_eq!(
            duplicate_name(Path::new("/w/a.rs"), |_| false),
            Some(PathBuf::from("/w/a copy.rs"))
        );
        assert_eq!(
            duplicate_name(Path::new("/w/a.rs"), exists),
            Some(PathBuf::from("/w/a copy 3.rs"))
        );
        // 확장자 없는 이름도 같은 사다리를 탄다.
        assert_eq!(
            duplicate_name(Path::new("/w/Makefile"), |_| false),
            Some(PathBuf::from("/w/Makefile copy"))
        );
    }
}
