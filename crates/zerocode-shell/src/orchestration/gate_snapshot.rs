//! A worker's tree, saved as a restore point without touching it (t-26583).
//!
//! A checkpoint the gate forces is made while the worker is working in that very
//! tree. A commit would move its HEAD and clear what it sees as changed; a stash
//! would take its files away. So the snapshot never uses the worker's index or
//! its branch: it builds a tree out of a temporary index — HEAD plus every
//! change and every new file the ignore rules do not name — and saves it as a
//! commit under a ref of its own,
//! `refs/zerocode/checkpoints/<worker>/<number>`. The worker's files, index,
//! branch and stash are exactly as they were; what was saved is one `git show`
//! or `git checkout <ref> -- .` away, and the newest few are kept.

use std::path::Path;

/// The refs under which checkpoints live.
const REF_ROOT: &str = "refs/zerocode/checkpoints";

/// How many checkpoints of one worker are kept: the newest three. A checkpoint
/// is a restore point, not a history — a fourth makes the first one a loss
/// nobody will count, and an object store that grows by a tree every few
/// minutes of a worker's life is the cost to a person's repository.
pub(super) const KEPT: usize = 3;

/// The identity the commit is made under: nobody's, and not a real address.
const IDENTITY: [&str; 4] = [
    "-c",
    "user.name=ZeroCode checkpoint",
    "-c",
    "user.email=checkpoint@zerocode.invalid",
];

/// Saves `checkout`'s tree under a ref of `worker`'s, numbered `number`; the ref,
/// or `None` for a tree with nothing to save — the same as its HEAD.
///
/// # Errors
///
/// What git said, when it said no: a checkout that is gone, a branch with no
/// commit yet, a tree git cannot read.
pub(super) fn save(checkout: &Path, worker: &str, number: u32) -> Result<Option<String>, String> {
    if !is_ref_part(worker) {
        return Err(format!("{worker} is not a name a ref can carry"));
    }
    if !checkout.is_dir() {
        return Err(format!("the checkout {} is gone", checkout.display()));
    }
    let head = git(checkout, &["rev-parse", "--verify", "HEAD"], None)?;
    let head = head.trim();
    let git_dir = git(checkout, &["rev-parse", "--absolute-git-dir"], None)?;
    let index = Path::new(git_dir.trim()).join(format!(
        "zerocode-checkpoint-{}-{number}.index",
        std::process::id()
    ));
    let saved = save_with(checkout, worker, number, head, &index);
    let _ = std::fs::remove_file(&index);
    saved
}

fn save_with(
    checkout: &Path,
    worker: &str,
    number: u32,
    head: &str,
    index: &Path,
) -> Result<Option<String>, String> {
    git(checkout, &["read-tree", head], Some(index))?;
    git(checkout, &["add", "-A"], Some(index))?;
    let tree = git(checkout, &["write-tree"], Some(index))?;
    let tree = tree.trim();
    let head_tree = git(checkout, &["rev-parse", &format!("{head}^{{tree}}")], None)?;
    if tree == head_tree.trim() {
        return Ok(None);
    }
    let message = format!("checkpoint({worker}) {number}");
    let mut commit = IDENTITY.to_vec();
    commit.extend(["commit-tree", tree, "-p", head, "-m", &message]);
    let commit = git(checkout, &commit, None)?;
    let reference = format!("{REF_ROOT}/{worker}/{number}");
    git(checkout, &["update-ref", &reference, commit.trim()], None)?;
    prune(checkout, worker);
    Ok(Some(reference))
}

/// Lets go of all but the newest [`KEPT`] checkpoints of `worker`.
fn prune(checkout: &Path, worker: &str) {
    let Ok(listed) = git(
        checkout,
        &[
            "for-each-ref",
            "--format=%(refname)",
            &format!("{REF_ROOT}/{worker}/"),
        ],
        None,
    ) else {
        return;
    };
    let mut held: Vec<(u32, &str)> = listed
        .lines()
        .filter_map(|line| Some((line.rsplit('/').next()?.parse().ok()?, line)))
        .collect();
    held.sort_unstable_by_key(|(number, _)| std::cmp::Reverse(*number));
    for (_, reference) in held.into_iter().skip(KEPT) {
        let _ = git(checkout, &["update-ref", "-d", reference], None);
    }
}

/// Whether a worker's id can be one component of a ref: letters, digits and the
/// three marks a worker id is made of.
fn is_ref_part(worker: &str) -> bool {
    !worker.is_empty()
        && worker
            .chars()
            .all(|glyph| glyph.is_ascii_alphanumeric() || matches!(glyph, '-' | '_' | '.'))
        && !worker.starts_with('.')
}

/// One git command in `checkout`, its output, or git's own first line. With
/// `index` it works in that index file and not the checkout's own.
fn git(checkout: &Path, args: &[&str], index: Option<&Path>) -> Result<String, String> {
    let mut command = crate::proc::quiet_command("git");
    command.arg("-C").arg(checkout).args(args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command
        .output()
        .map_err(|why| format!("git could not be run: {why}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            said.lines().next().unwrap_or_default().trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests;
