//! A worker's tree, saved as a restore point without touching it (t-26583).
//!
//! A checkpoint the gate forces is made while the worker is working in that very
//! tree. A commit would move its HEAD and clear what it sees as changed; a stash
//! would take its files away. So the snapshot never uses the worker's index or
//! its branch: it builds a tree out of a COPY of the worker's index — everything
//! it already knows of every file, then every change and every new file the
//! ignore rules do not name — and saves it as a commit under a ref of its own,
//! `refs/zerocode/checkpoints/<worker>/<number>`. The worker's files, index,
//! branch and stash are exactly as they were; what was saved is one `git show`
//! or `git checkout <ref> -- .` away. The newest few of each worker are kept
//! ([`KEPT`]) and no more than [`REFS_KEPT_MAX`] in all, so that a repository the
//! gate has worked in for a month holds what it held on the first day.
//!
//! Two things keep it cheap on a machine that cannot spare it, because it runs
//! on the beat while the worker works: the copy of the index carries each file's
//! stat, so `add` hashes only what changed and not the whole tree; and one
//! `rev-parse` answers everything the snapshot needs to know of the checkout.

use std::collections::HashMap;
use std::path::Path;

/// The refs under which checkpoints live.
const REF_ROOT: &str = "refs/zerocode/checkpoints";

/// How many checkpoints of one worker are kept: the newest three. A checkpoint
/// is a restore point, not a history — a fourth makes the first one a loss
/// nobody will count, and an object store that grows by a tree every few
/// minutes of a worker's life is the cost to a person's repository.
pub(super) const KEPT: usize = 3;

/// How many checkpoints one repository holds in all, whoever's they are: sixty,
/// twenty workers' worth of the newest three. Without a bound the refs of every
/// worker that ever ran would stay in a person's repository for good.
pub(super) const REFS_KEPT_MAX: usize = 60;

/// The identity the commit is made under: nobody's, and not a real address.
const IDENTITY: [&str; 4] = [
    "-c",
    "user.name=ZeroCode checkpoint",
    "-c",
    "user.email=checkpoint@zerocode.invalid",
];

/// What the snapshot stands on: the commit the worker is on, that commit's tree,
/// and the worker's own index — the one thing of the worker's it reads.
struct Base<'a> {
    head: &'a str,
    head_tree: &'a str,
    own_index: &'a Path,
}

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
    let facts = git(
        checkout,
        &["rev-parse", "HEAD", "HEAD^{tree}", "--absolute-git-dir"],
        None,
    )?;
    let mut said = facts.lines().map(str::trim);
    let (Some(head), Some(head_tree), Some(git_dir)) = (said.next(), said.next(), said.next())
    else {
        return Err("git did not say where the checkout stands".to_string());
    };
    let git_dir = Path::new(git_dir);
    let own_index = git_dir.join("index");
    let index = git_dir.join(format!(
        "zerocode-checkpoint-{}-{number}.index",
        std::process::id()
    ));
    let base = Base {
        head,
        head_tree,
        own_index: &own_index,
    };
    let saved = save_with(checkout, worker, number, &base, &index);
    let _ = std::fs::remove_file(&index);
    saved
}

fn save_with(
    checkout: &Path,
    worker: &str,
    number: u32,
    base: &Base<'_>,
    index: &Path,
) -> Result<Option<String>, String> {
    // A repository with no index yet has nothing to copy: it starts from HEAD.
    if std::fs::copy(base.own_index, index).is_err() {
        git(checkout, &["read-tree", base.head], Some(index))?;
    }
    git(checkout, &["add", "-A"], Some(index))?;
    let tree = git(checkout, &["write-tree"], Some(index))?;
    let tree = tree.trim();
    if tree == base.head_tree {
        return Ok(None);
    }
    let message = format!("checkpoint({worker}) {number}");
    let mut commit = IDENTITY.to_vec();
    commit.extend(["commit-tree", tree, "-p", base.head, "-m", &message]);
    let commit = git(checkout, &commit, None)?;
    let reference = format!("{REF_ROOT}/{worker}/{number}");
    git(checkout, &["update-ref", &reference, commit.trim()], None)?;
    prune(checkout);
    Ok(Some(reference))
}

/// Lets go of what a restore point is not for: all but the newest [`KEPT`] of each
/// worker, and everything past the newest [`REFS_KEPT_MAX`] in the repository.
/// Newest by the commit's time and never by number — a later attempt of a worker
/// numbers its checkpoints from one again, and its first is the newest there is.
fn prune(checkout: &Path) {
    let Ok(listed) = git(
        checkout,
        &[
            "for-each-ref",
            "--sort=-refname",
            "--sort=-committerdate",
            "--format=%(refname)",
            &format!("{REF_ROOT}/"),
        ],
        None,
    ) else {
        return;
    };
    let mut of_worker: HashMap<&str, usize> = HashMap::new();
    let mut kept = 0;
    for reference in listed.lines() {
        let count = of_worker.entry(worker_of(reference)).or_insert(0);
        if *count < KEPT && kept < REFS_KEPT_MAX {
            *count += 1;
            kept += 1;
        } else {
            let _ = git(checkout, &["update-ref", "-d", reference], None);
        }
    }
}

/// `refs/zerocode/checkpoints/<worker>/<number>` → `<worker>`.
fn worker_of(reference: &str) -> &str {
    reference
        .strip_prefix(REF_ROOT)
        .and_then(|rest| rest.trim_start_matches('/').split('/').next())
        .unwrap_or_default()
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
/// `index` it works in that index file and not the checkout's own. The one door
/// every git of this module goes in by, so that none is the one that takes an
/// optional lock a worker's own `add` or `commit` would wait for.
fn git(checkout: &Path, args: &[&str], index: Option<&Path>) -> Result<String, String> {
    let mut command = zerocode_core::host::lock_free_git(crate::proc::quiet_command("git"));
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(checkout)
        .args(args);
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
