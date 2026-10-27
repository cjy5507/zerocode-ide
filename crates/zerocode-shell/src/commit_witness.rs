//! What git says about the commits a coordinator's `hand-in` or `task-backfill` names (t-78075).
//!
//! The ledger runs no git, so the window asks before the plan and hands the answers in beside the
//! receipt files it reads for a report ([`zerocode_core::orchestration::evidence::ReceiptFiles`]).
//! The ledger then judges only what was said here: a commit git does not know, or one the base
//! does not hold, is refused there, never guessed. A question git gave no answer to — it did not
//! start, or ran past [`GIT_BUDGET`] — is left out, so the ledger says git was not asked instead
//! of saying the commit is unknown.

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
    see_with(&LocalVcs, repo, commits, base, tags)
}

/// [`see`], asking `vcs`. A base or a commit git gave no answer for is left out.
fn see_with(
    vcs: &dyn Vcs,
    repo: &Path,
    commits: &[String],
    base: Option<&str>,
    tags: Option<&str>,
) -> CommitsSeen {
    let mut seen = CommitsSeen {
        repo: repo.display().to_string(),
        base: base.and_then(|name| {
            resolve(vcs, repo, name).map(|head| BaseSeen {
                name: name.to_string(),
                head,
            })
        }),
        tags: tags.map(str::to_string),
        commits: Default::default(),
    };
    let base_head = seen.base.as_ref().and_then(|base| base.head.clone());
    for named in commits {
        let Some(full) = resolve(vcs, repo, named) else {
            continue;
        };
        let in_base = match (&full, &base_head) {
            (Some(full), Some(head)) => holds(vcs, repo, head, full),
            _ => None,
        };
        let tag = match (&full, tags) {
            (Some(full), Some(tags)) if in_base != Some(false) => first_tag(vcs, repo, full, tags),
            _ => None,
        };
        seen.insert(named, CommitSeen { full, in_base, tag });
    }
    seen
}

/// The commit `name` names in `repo`, in full: `Some(None)` when git knows no such commit, and
/// `None` when git gave no answer at all.
fn resolve(vcs: &dyn Vcs, repo: &Path, name: &str) -> Option<Option<String>> {
    let asked = format!("{name}^{{commit}}");
    match vcs.try_within(
        repo,
        &["rev-parse", "--verify", "--quiet", &asked],
        GIT_BUDGET,
    ) {
        Within::Said(full) => Some(Some(full.trim().to_string()).filter(|full| !full.is_empty())),
        Within::Refused(_) => Some(None),
        Within::Silent => None,
    }
}

/// Whether `head` holds `commit` — git's own answer, by exit code: 0 it does, 1 it does not, and
/// anything else is no answer.
fn holds(vcs: &dyn Vcs, repo: &Path, head: &str, commit: &str) -> Option<bool> {
    match vcs.text_and_code(repo, &["merge-base", "--is-ancestor", commit, head]) {
        Ok((0, _)) => Some(true),
        Ok((1, _)) => Some(false),
        _ => None,
    }
}

/// The first tag matching `tags`, in version order, that holds `commit`.
fn first_tag(vcs: &dyn Vcs, repo: &Path, commit: &str, tags: &str) -> Option<String> {
    match vcs.try_within(
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
        let out = crate::proc::quiet_command("git")
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

    /// A git that never answers: it did not start, or ran past its budget.
    struct Silent;

    impl Vcs for Silent {
        fn text(&self, _root: &Path, _args: &[&str]) -> Result<String, String> {
            Err("git did not start".to_string())
        }

        fn text_and_code(&self, _root: &Path, _args: &[&str]) -> Result<(i32, String), String> {
            Err("git did not start".to_string())
        }

        fn try_within_max(
            &self,
            _root: &Path,
            _args: &[&str],
            _budget: Duration,
            _max: Option<usize>,
        ) -> Within {
            Within::Silent
        }

        fn clone(
            &self,
            _url: &str,
            _into: &Path,
            _say: &mut dyn FnMut(zerocode_core::clone::CloneStep),
        ) -> Result<(), String> {
            Err("git did not start".to_string())
        }
    }

    /// No answer is not an answer: a commit or a base git said nothing about is left out, so the
    /// ledger says git was not asked rather than that the commit is not one.
    #[test]
    fn a_git_that_says_nothing_leaves_its_commits_out_instead_of_calling_them_unknown() {
        let commit = "0123456789abcdef0123456789abcdef01234567".to_string();
        let seen = see_with(
            &Silent,
            Path::new("/Users/dev/repo"),
            std::slice::from_ref(&commit),
            Some("main"),
            Some("v*"),
        );
        assert_eq!(seen.commit(&commit), None, "{seen:?}");
        assert_eq!(seen.base, None, "{seen:?}");
        assert_eq!(
            seen.repo,
            Path::new("/Users/dev/repo").display().to_string()
        );
    }
}
