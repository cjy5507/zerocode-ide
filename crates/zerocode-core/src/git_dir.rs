//! Where a checkout keeps its git directory — read, not asked.
//!
//! `git rev-parse --absolute-git-dir` answers this, and it costs a process. The
//! answer is a file this application can read directly, and two callers want it
//! on paths that repaint: the branch in the titlebar and the operation a
//! checkout is halfway through. One process per repaint for a string that
//! changes when somebody moves a repository is a bad trade, so it is read.
//!
//! The subtlety this module exists for is LINKED WORKTREES, which is most of
//! what this application opens. There `.git` is not a directory but a file
//! holding `gitdir: <path>`, and the naive `<root>/.git/…` read answers
//! nothing for every one of them. It also points at the worktree's OWN git
//! directory (`…/.git/worktrees/<name>`) rather than the shared one, which is
//! the right end of the pointer for per-worktree state: `MERGE_HEAD`, the
//! rebase directories and `HEAD` all live there, one set per checkout.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Machine-local identities of one checkout and the repository that owns it.
///
/// The worktree id is derived from the per-worktree Git directory, so moving
/// the checkout itself does not change the id. The repository id is derived
/// from the shared Git directory and is therefore identical across all linked
/// worktrees of that repository. Both are deliberately machine-local: they
/// name local filesystem authorities, not portable repository content.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub repository_id: String,
    pub worktree_id: String,
}

/// What a checkout names its git directory, or the file that points at one.
const GIT_DIR_NAME: &str = ".git";

/// The git directory of the checkout rooted at `root`.
///
/// [`None`] means this is not the root of a checkout — an ordinary answer, not
/// a failure. It also means "ask git" for the callers that have a reason to:
/// a path INSIDE a checkout has no `.git` of its own, and `GIT_DIR` in the
/// environment is a spelling only git itself honours.
#[must_use]
pub fn of(root: &Path) -> Option<PathBuf> {
    let dot = root.join(GIT_DIR_NAME);
    if dot.is_dir() {
        return Some(dot);
    }
    // `gitdir:` is written with the platform's own separators, and it is
    // relative when the worktree sits beside its repository — both are what
    // `Path::join` already means on either platform.
    let pointer = std::fs::read_to_string(&dot).ok()?;
    let target = pointer.trim().strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    Some(root.join(target))
}

/// The git directory every worktree of this repository SHARES.
///
/// [`of`] answers with the per-worktree directory on purpose — that is where
/// `HEAD`, `MERGE_HEAD` and the rebase directories live, one set per checkout.
/// The things a repository has exactly one of live somewhere else, and
/// `commondir` beside the per-worktree directory is the pointer to it:
/// `config`, `packed-refs`, the object store. A main worktree has no
/// `commondir` file because there is nothing to point at — its own directory
/// is the shared one, which is why the file's absence is an answer here rather
/// than a failure.
///
/// This is `git rev-parse --git-common-dir` without the process.
#[must_use]
pub fn common_of(root: &Path) -> Option<PathBuf> {
    let own = of(root)?;
    let Ok(pointer) = std::fs::read_to_string(own.join("commondir")) else {
        return Some(own);
    };
    let target = pointer.trim();
    if target.is_empty() {
        return Some(own);
    }
    // Relative to the per-worktree directory, and usually exactly `../..`.
    Some(own.join(target))
}

/// The checkout that owns the repository `path` belongs to: the nearest
/// checkout at or above `path`, and then — when that one is a linked worktree
/// — the checkout it was cut from.
///
/// [`of`] answers only at a checkout's own root and only about that checkout,
/// which is the right end of the pointer for per-worktree state. This is the
/// other question: WHICH REPOSITORY are these words from. A worktree's shared
/// git directory is the main checkout's own ([`common_of`]), so its parent is
/// that checkout — and for a main checkout the walk simply arrives back at
/// itself.
///
/// [`None`] for a folder no checkout holds, and for a bare repository, which
/// has no checkout to name. The walk reads files rather than running `git
/// worktree list`, for the reason this module exists: one process per answer
/// is a bad trade for a string read from two small files.
#[must_use]
pub fn owning_checkout_of(path: &Path) -> Option<PathBuf> {
    let shared = normalized(path.ancestors().find_map(common_of)?);
    shared
        .file_name()
        .filter(|name| *name == OsStr::new(GIT_DIR_NAME))
        .and(shared.parent())
        .map(Path::to_path_buf)
}

/// Stable local identity of a checkout without spawning Git.
///
/// Canonicalization is best-effort for the same reason [`of`] is: a checkout
/// can remain readable while one of its ancestors is temporarily unavailable.
/// In that case the normalized pointer path is still a deterministic local
/// answer and callers may retry after the filesystem recovers.
#[must_use]
pub fn identity(root: &Path) -> Option<Identity> {
    let worktree = normalized(of(root)?);
    let repository = normalized(common_of(root)?);
    Some(Identity {
        repository_id: opaque_path_id("repo", &repository),
        worktree_id: opaque_path_id("worktree", &worktree),
    })
}

/// Opaque local id for a filesystem authority.
///
/// Public so the bounded Git snapshotter and the repaint-safe direct reader use
/// one byte framing and cannot mint different ids for the same directory.
#[must_use]
pub fn opaque_path_id(prefix: &str, path: &Path) -> String {
    let mut digest = Sha256::new();
    hash_frame(&mut digest, prefix.as_bytes(), &os_key(path.as_os_str()));
    format!("{prefix}-{:x}", digest.finalize())
}

fn normalized(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn hash_frame(digest: &mut Sha256, label: &[u8], value: &[u8]) {
    digest.update((label.len() as u64).to_le_bytes());
    digest.update(label);
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

#[cfg(unix)]
fn os_key(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;

    value.as_bytes().to_vec()
}

#[cfg(windows)]
fn os_key(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt as _;

    value
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>()
}

#[cfg(not(any(unix, windows)))]
fn os_key(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().into_owned().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_checkout_and_a_linked_worktree_both_answer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(plain.join(".git")).expect("mkdir");
        assert_eq!(of(&plain), Some(plain.join(".git")));

        // 연결된 워크트리: `.git`은 디렉터리가 아니라 **파일**이고, 그 끝이
        // 이 체크아웃만의 git 디렉터리다(공유 디렉터리가 아니다 — `MERGE_HEAD`도
        // 리베이스 디렉터리도 체크아웃마다 하나씩 거기 있다).
        let linked = dir.path().join("linked");
        std::fs::create_dir_all(&linked).expect("mkdir");
        std::fs::write(
            linked.join(".git"),
            "gitdir: ../plain/.git/worktrees/linked\n",
        )
        .expect("write");
        assert_eq!(
            of(&linked),
            Some(linked.join("../plain/.git/worktrees/linked"))
        );

        // 체크아웃이 아닌 것과, 아무 말도 안 적힌 포인터는 답이 없다 — 오류가
        // 아니라 답이 없는 것이고, 부르는 쪽은 그때 git에게 물어도 된다.
        assert_eq!(of(dir.path()), None);
        let empty = dir.path().join("empty");
        std::fs::create_dir_all(&empty).expect("mkdir");
        std::fs::write(empty.join(".git"), "gitdir:   \n").expect("write");
        assert_eq!(of(&empty), None);
        let foreign = dir.path().join("foreign");
        std::fs::create_dir_all(&foreign).expect("mkdir");
        std::fs::write(foreign.join(".git"), "not a pointer\n").expect("write");
        assert_eq!(of(&foreign), None);
    }

    #[test]
    fn the_shared_directory_is_where_commondir_points_and_otherwise_here() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("plain");
        let shared = plain.join(".git");
        std::fs::create_dir_all(shared.join("worktrees").join("linked")).expect("mkdir");

        // 본체는 가리킬 것이 없다 — `commondir` 파일이 없는 것이 답이지 실패가
        // 아니다. 자기 디렉터리가 곧 공유 디렉터리다.
        assert_eq!(common_of(&plain), Some(shared.clone()));

        // 연결된 워크트리는 자기 것과 공유하는 것이 **다르다**: `HEAD`는 자기
        // 것에, `config`는 공유하는 것에 있다.
        let linked = dir.path().join("linked");
        std::fs::create_dir_all(&linked).expect("mkdir");
        let own = shared.join("worktrees").join("linked");
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", own.display())).expect("write");
        std::fs::write(own.join("commondir"), "../..\n").expect("write");
        assert_eq!(of(&linked), Some(own.clone()));
        assert_eq!(common_of(&linked), Some(own.join("../..")));

        // 빈 포인터는 가리키지 않는 것과 같다.
        std::fs::write(own.join("commondir"), "  \n").expect("write");
        assert_eq!(common_of(&linked), Some(own));

        // 체크아웃이 아니면 여기도 답이 없다.
        assert_eq!(common_of(dir.path()), None);
    }

    #[test]
    fn a_worktree_is_owned_by_the_checkout_it_was_cut_from_and_a_stranger_is_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = dir.path().join("main");
        let shared = main.join(".git");
        let own = shared.join("worktrees").join("linked");
        std::fs::create_dir_all(&own).expect("git dirs");
        std::fs::create_dir_all(main.join("crates").join("tools")).expect("a folder inside");

        // 본체는 자기 자신이 주인이고, 그 안의 폴더도 같은 답을 낸다 — 세션이
        // 체크아웃 뿌리가 아니라 그 아래에서 열려도 같은 저장소다.
        assert_eq!(
            owning_checkout_of(&main).as_deref(),
            Some(normalized(main.clone()).as_path())
        );
        assert_eq!(
            owning_checkout_of(&main.join("crates").join("tools")).as_deref(),
            Some(normalized(main.clone()).as_path())
        );

        // 연결된 워크트리는 자기를 잘라 낸 체크아웃이 주인이다.
        let linked = dir.path().join("linked");
        std::fs::create_dir_all(linked.join("ui")).expect("linked checkout");
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", own.display()))
            .expect("pointer");
        std::fs::write(own.join("commondir"), "../..\n").expect("common pointer");
        for asked in [linked.clone(), linked.join("ui")] {
            assert_eq!(
                owning_checkout_of(&asked).as_deref(),
                Some(normalized(main.clone()).as_path()),
                "{}",
                asked.display()
            );
        }

        // 남의 저장소의 워크트리는 남의 체크아웃이 주인이다.
        let stranger = dir.path().join("stranger");
        let stranger_own = stranger.join(".git").join("worktrees").join("side");
        std::fs::create_dir_all(&stranger_own).expect("stranger git dirs");
        let side = dir.path().join("side");
        std::fs::create_dir_all(&side).expect("side checkout");
        std::fs::write(
            side.join(".git"),
            format!("gitdir: {}\n", stranger_own.display()),
        )
        .expect("pointer");
        std::fs::write(stranger_own.join("commondir"), "../..\n").expect("common pointer");
        assert_eq!(
            owning_checkout_of(&side).as_deref(),
            Some(normalized(stranger).as_path())
        );

        // 체크아웃이 아무 데도 없으면 답이 없다.
        let nowhere = tempfile::tempdir().expect("tempdir");
        assert_eq!(owning_checkout_of(nowhere.path()), None);
    }

    #[test]
    fn identities_share_a_repository_and_survive_a_worktree_move() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("plain");
        let shared = plain.join(".git");
        let own = shared.join("worktrees").join("linked");
        std::fs::create_dir_all(&own).expect("git dirs");

        let linked = dir.path().join("linked");
        std::fs::create_dir_all(&linked).expect("linked checkout");
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", own.display()))
            .expect("pointer");
        std::fs::write(own.join("commondir"), "../..\n").expect("common pointer");

        let main = identity(&plain).expect("main identity");
        let before = identity(&linked).expect("linked identity");
        assert_eq!(main.repository_id, before.repository_id);
        assert_ne!(main.worktree_id, before.worktree_id);

        let moved = dir.path().join("moved-linked");
        std::fs::rename(&linked, &moved).expect("move checkout");
        let after = identity(&moved).expect("moved identity");
        assert_eq!(before, after);
    }
}
