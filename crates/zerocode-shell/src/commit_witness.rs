//! What git says about the commits a coordinator's `hand-in` or `task-backfill` names (t-78075).
//!
//! The ledger runs no git, so the window asks before the plan and hands the answers in beside the
//! receipt files it reads for a report ([`zerocode_core::orchestration::evidence::ReceiptFiles`]).
//! The ledger then judges only what was said here: a commit git does not know, or one the base
//! does not hold, is refused there, never guessed.

use std::path::Path;
use std::time::Duration;

use zerocode_core::host::{LocalVcs, Vcs, Within};
use zerocode_core::orchestration::coordinator_take::{BaseSeen, CommitSeen, CommitsSeen};

/// How long one git question may take before it counts as no answer. Every question here reads
/// refs and history and writes nothing.
const GIT_BUDGET: Duration = Duration::from_secs(10);

/// What git says in `repo` about each of `commits`: the full hash, whether `base` holds it, and
/// the first release tag matching `tags` that does — the last two asked only of a commit git
/// knows, and the tag only of one the base holds when a base is named.
pub(crate) fn see(
    repo: &Path,
    commits: &[String],
    base: Option<&str>,
    tags: Option<&str>,
) -> CommitsSeen {
    let mut seen = CommitsSeen {
        repo: repo.display().to_string(),
        base: base.map(|name| BaseSeen {
            name: name.to_string(),
            head: resolve(repo, name),
        }),
        tags: tags.map(str::to_string),
        commits: Default::default(),
    };
    let base_head = seen.base.as_ref().and_then(|base| base.head.clone());
    for named in commits {
        let full = resolve(repo, named);
        let in_base = match (&full, &base_head) {
            (Some(full), Some(head)) => holds(repo, head, full),
            _ => None,
        };
        let tag = match (&full, tags) {
            (Some(full), Some(tags)) if in_base != Some(false) => first_tag(repo, full, tags),
            _ => None,
        };
        seen.insert(named, CommitSeen { full, in_base, tag });
    }
    seen
}

/// The commit `name` names in `repo`, in full; `None` when git knows no such commit.
fn resolve(repo: &Path, name: &str) -> Option<String> {
    let asked = format!("{name}^{{commit}}");
    match LocalVcs.try_within(
        repo,
        &["rev-parse", "--verify", "--quiet", &asked],
        GIT_BUDGET,
    ) {
        Within::Said(full) => Some(full.trim().to_string()).filter(|full| !full.is_empty()),
        Within::Refused(_) | Within::Silent => None,
    }
}

/// Whether `head` holds `commit` — git's own answer, by exit code: 0 it does, 1 it does not, and
/// anything else is no answer.
fn holds(repo: &Path, head: &str, commit: &str) -> Option<bool> {
    match LocalVcs.text_and_code(repo, &["merge-base", "--is-ancestor", commit, head]) {
        Ok((0, _)) => Some(true),
        Ok((1, _)) => Some(false),
        _ => None,
    }
}

/// The first tag matching `tags`, in version order, that holds `commit`.
fn first_tag(repo: &Path, commit: &str, tags: &str) -> Option<String> {
    match LocalVcs.try_within(
        repo,
        &[
            "tag",
            "--list",
            tags,
            "--contains",
            commit,
            "--sort=v:refname",
        ],
        GIT_BUDGET,
    ) {
        Within::Said(listed) => listed
            .lines()
            .map(str::trim)
            .find(|tag| !tag.is_empty())
            .map(str::to_string),
        Within::Refused(_) | Within::Silent => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "dev")
            .env("GIT_AUTHOR_EMAIL", "dev@example.com")
            .env("GIT_COMMITTER_NAME", "dev")
            .env("GIT_COMMITTER_EMAIL", "dev@example.com")
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(repo: &Path, file: &str) -> String {
        std::fs::write(repo.join(file), file).expect("write");
        git(repo, &["add", file]);
        git(repo, &["commit", "-q", "-m", file]);
        git(repo, &["rev-parse", "HEAD"])
    }

    /// A throwaway repository: `main` with a released commit and an unreleased one after it, and a
    /// side branch `main` does not hold.
    #[test]
    fn git_answers_for_a_commit_the_base_holds_one_it_does_not_and_one_it_does_not_know() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        let released = commit(repo, "one");
        git(repo, &["tag", "v1.0.0"]);
        let unreleased = commit(repo, "two");
        git(repo, &["checkout", "-q", "-b", "side"]);
        let aside = commit(repo, "three");
        git(repo, &["checkout", "-q", "main"]);
        let unknown = "0123456789abcdef0123456789abcdef01234567".to_string();
        let short = released[..9].to_string();

        let seen = see(
            repo,
            &[
                short.clone(),
                unreleased.clone(),
                aside.clone(),
                unknown.clone(),
            ],
            Some("main"),
            Some("v*"),
        );
        let main = git(repo, &["rev-parse", "main"]);
        assert_eq!(
            seen.base,
            Some(BaseSeen {
                name: "main".to_string(),
                head: Some(main),
            })
        );
        assert_eq!(
            seen.commit(&short),
            Some(&CommitSeen {
                full: Some(released.clone()),
                in_base: Some(true),
                tag: Some("v1.0.0".to_string()),
            }),
            "an abbreviated name resolves to the full commit, in the release that holds it"
        );
        assert_eq!(
            seen.commit(&unreleased),
            Some(&CommitSeen {
                full: Some(unreleased.clone()),
                in_base: Some(true),
                tag: None,
            })
        );
        assert_eq!(
            seen.commit(&aside).map(|one| one.in_base),
            Some(Some(false))
        );
        assert_eq!(
            seen.commit(&unknown),
            Some(&CommitSeen::default()),
            "a commit git does not know is said to be unknown, not guessed"
        );

        // A base git does not know resolves to nothing, and nothing is measured against it.
        let blind = see(repo, std::slice::from_ref(&released), Some("nope"), None);
        assert_eq!(blind.base.as_ref().and_then(|base| base.head.clone()), None);
        assert_eq!(blind.commit(&released).map(|one| one.in_base), Some(None));
    }
}
