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
use zerocode_core::orchestration::coordinator_take::{
    BaseSeen, CommitSeen, CommitsSeen, PatchSeen,
};

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
        patches: Default::default(),
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

/// Whether `head` holds `commit` — git's own answer: exit 0 it does, and it says "does not" by a
/// failure with nothing on its standard error. A budgeted call hands back no exit code, so any
/// silent failure reads as "does not", a git killed by a signal too: the caller refuses and
/// writes nothing, and asking again answers it. An error is words there, so that, a git that
/// passed [`GIT_BUDGET`] and one that did not start are no answer (t-84720).
fn holds(vcs: &dyn Vcs, repo: &Path, head: &str, commit: &str) -> Option<bool> {
    match vcs.try_within(
        repo,
        &["merge-base", "--is-ancestor", commit, head],
        GIT_BUDGET,
    ) {
        Within::Said(_) => Some(true),
        Within::Refused(said) if said.trim().is_empty() => Some(false),
        Within::Refused(_) | Within::Silent => None,
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

/// The most of one diff kept: a change past it is no answer, and the review is refused as one
/// git was not asked about — never judged on half a diff.
const DIFF_MAX: usize = 32 * 1024 * 1024;

/// What `tested` and `source` each change, for a verification whose tested head is not the source
/// it names (t-97078): `tested`'s own commits from where it meets `source`, and the same number of
/// commits at the tip of `source`. A rebase keeps the count and the change; anything else reads as
/// another change. `None` when git gave no answer to one of the questions.
pub(crate) fn see_rebase(
    repo: &Path,
    tested: &str,
    source: &str,
) -> Option<(PatchSeen, PatchSeen)> {
    see_rebase_with(&LocalVcs, repo, tested, source)
}

/// What git says of `tested` and `source`, keyed as each was named, for a review whose tested head
/// is not written as the source it names: the commit each name resolves to, and — when they are
/// two commits — what each one changes ([`see_rebase`]). Two names of one commit (one written
/// short, t-97078 r2) are that commit, and nothing is diffed. `None` when git gave no answer.
pub(crate) fn see_heads(repo: &Path, tested: &str, source: &str) -> Option<CommitsSeen> {
    let mut seen = CommitsSeen {
        repo: repo.display().to_string(),
        ..Default::default()
    };
    let resolved = [tested, source].map(|named| resolve(&LocalVcs, repo, named).flatten());
    for (named, full) in [tested, source].into_iter().zip(&resolved) {
        if let Some(full) = full {
            seen.insert(
                named,
                CommitSeen {
                    full: Some(full.clone()),
                    ..CommitSeen::default()
                },
            );
        }
    }
    if let [Some(one), Some(other)] = &resolved
        && one == other
    {
        return Some(seen);
    }
    let (was, now) = see_rebase(repo, tested, source)?;
    seen.insert_patch(tested, was);
    seen.insert_patch(source, now);
    Some(seen)
}

fn see_rebase_with(
    vcs: &dyn Vcs,
    repo: &Path,
    tested: &str,
    source: &str,
) -> Option<(PatchSeen, PatchSeen)> {
    let meet = said(vcs, repo, &["merge-base", tested, source], None)?;
    let meet = meet.trim();
    let commits = count(vcs, repo, meet, tested)?;
    // A tested head `source` already holds has nothing of its own: `source`'s change is then all
    // it adds, which no empty change equals.
    let from = match commits {
        0 => meet.to_string(),
        _ => format!("{source}~{commits}"),
    };
    let was = patch(vcs, repo, meet, tested, commits)?;
    let now = patch(vcs, repo, &from, source, count(vcs, repo, &from, source)?)?;
    Some((was, now))
}

/// What git said in `repo` when it finished with 0 within the budget.
fn said(vcs: &dyn Vcs, repo: &Path, args: &[&str], max: Option<usize>) -> Option<String> {
    match vcs.try_within_max(repo, args, GIT_BUDGET, max) {
        Within::Said(text) => Some(text),
        Within::Refused(_) | Within::Silent => None,
    }
}

/// How many commits `to` holds that `from` does not.
fn count(vcs: &dyn Vcs, repo: &Path, from: &str, to: &str) -> Option<u32> {
    said(
        vcs,
        repo,
        &["rev-list", "--count", &format!("{from}..{to}")],
        None,
    )?
    .trim()
    .parse()
    .ok()
}

/// The change from `from` to `to`, file by file ([`patch_files`]).
fn patch(vcs: &dyn Vcs, repo: &Path, from: &str, to: &str, commits: u32) -> Option<PatchSeen> {
    let diff = said(
        vcs,
        repo,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--full-index",
            from,
            to,
        ],
        Some(DIFF_MAX),
    )?;
    // The diff is read as UTF-8, and a byte that is not becomes U+FFFD: two Latin-1 letters would
    // read the same (t-97078 r3, review t-97548 P3). Such a diff is no answer — never judged on
    // what it was turned into. A file that holds U+FFFD itself is refused too, on the safe side.
    if diff.contains(char::REPLACEMENT_CHARACTER) {
        return None;
    }
    Some(PatchSeen {
        commits,
        files: patch_files(&diff),
    })
}

/// Each file of a `git diff`, with the digest of its part: what the change itself is kept exactly,
/// and only what the base it stands on decides is let go — so the same change at other line
/// numbers, on another base, digests the same, and any other content does not.
///
/// - A hunk header is kept without its line numbers.
/// - The `index` line names the base's blob too, so it is left out — but a binary file's part is
///   only 「Binary files … differ」, so its content is the blob it becomes, read from that line
///   (the diff is asked with `--full-index`), as `git patch-id` counts a binary file.
/// - An added or removed line is kept byte for byte (t-97078 r2, review t-97475 P2), its line
///   ending too (r3).
///   `git patch-id --stable` drops every line's whitespace because it looks for one commit
///   cherry-picked elsewhere, where a false match only hides a duplicate; here a false match
///   approves code nobody tested, and an indent is code — Python's blocks, YAML, a Makefile's
///   tabs, the space inside a string.
/// - A context line and every other line — the base's own words — are kept without whitespace.
///
/// A file is named by its `diff --git` line.
pub(crate) fn patch_files(diff: &str) -> std::collections::BTreeMap<String, String> {
    use sha2::{Digest, Sha256};
    const FILE_HEAD: &str = "diff --git ";
    const INDEX: &str = "index ";
    const BINARY: [&str; 2] = ["Binary files ", "GIT binary patch"];
    let mut files = std::collections::BTreeMap::new();
    // The file open now, its digest so far, and the blob its `index` line says it becomes.
    let mut open: Option<(String, Sha256, Option<String>)> = None;
    let mut close = |open: Option<(String, Sha256, Option<String>)>| {
        if let Some((file, digest, _)) = open {
            files.insert(file, format!("{:x}", digest.finalize()));
        }
    };
    // Split on `\n` alone: `lines()` drops a `\r` before it, and a line that only gained one — a
    // script turned CRLF — is another change (t-97078 r3).
    for line in diff.split_terminator('\n') {
        if let Some(named) = line.strip_prefix(FILE_HEAD) {
            close(open.take());
            let file = named
                .rsplit_once(" b/")
                .map_or(named, |(_, file)| file)
                .to_string();
            open = Some((file, Sha256::new(), None));
            continue;
        }
        let Some((_, digest, becomes)) = open.as_mut() else {
            continue;
        };
        if let Some(blobs) = line.strip_prefix(INDEX) {
            *becomes = blobs
                .split_whitespace()
                .next()
                .and_then(|blobs| blobs.split_once(".."))
                .map(|(_, after)| after.to_string());
            continue;
        }
        let kept: String = if line.starts_with("@@") {
            "@@".to_string()
        } else if BINARY.iter().any(|head| line.starts_with(head)) {
            format!("binary {}", becomes.as_deref().unwrap_or_default())
        } else if line.starts_with(['+', '-']) {
            line.to_string()
        } else {
            line.chars().filter(|c| !c.is_whitespace()).collect()
        };
        digest.update(kept.as_bytes());
        digest.update(b"\n");
    }
    close(open);
    files
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

    /// t-97078: a branch tested on one base and rebased onto a later `main` is the same change,
    /// file by file; the same branch with one more line in one file is another change, and the
    /// file is named; a tested head the source already holds has no change of its own.
    #[test]
    fn a_rebased_head_reads_as_the_same_change_and_an_edited_one_does_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("lib.rs"), "fn one() {}\nfn two() {}\n").expect("write");
        git(repo, &["add", "lib.rs"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        git(repo, &["checkout", "-q", "-b", "work"]);
        std::fs::write(
            repo.join("lib.rs"),
            "fn one() {}\nfn two() {}\nfn three() {}\n",
        )
        .expect("write");
        git(repo, &["commit", "-q", "-am", "red"]);
        let tested = commit(repo, "green.rs");
        git(repo, &["checkout", "-q", "main"]);
        commit(repo, "other.rs");
        git(repo, &["checkout", "-q", "-b", "landed", "work"]);
        git(repo, &["rebase", "-q", "main"]);
        let rebased = git(repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join("green.rs"), "green.rs and more").expect("write");
        git(repo, &["commit", "-q", "--amend", "-a", "--no-edit"]);
        let edited = git(repo, &["rev-parse", "HEAD"]);

        let (was, now) = see_rebase(repo, &tested, &rebased).expect("git answers");
        assert_eq!(was.commits, 2);
        assert_eq!(was, now, "a rebase is the same change");
        assert!(was.files.contains_key("lib.rs") && was.files.contains_key("green.rs"));

        let (was, now) = see_rebase(repo, &tested, &edited).expect("git answers");
        assert_eq!(was.differing_files(&now), vec!["green.rs".to_string()]);

        let main = git(repo, &["rev-parse", "main"]);
        let (was, now) = see_rebase(repo, &main, &rebased).expect("git answers");
        assert_eq!(was.commits, 0, "main holds nothing the rebased head lacks");
        assert_ne!(was, now);
    }

    /// t-97078 r2 (review t-97475 P1): git says only 「Binary files … differ」 of a binary file, so
    /// two other contents of one file read the same unless the content git names in the `index`
    /// line is read too. The same content on another base is still the same change.
    #[test]
    fn two_binary_files_with_other_contents_are_two_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("img.bin"), [0u8, 1, 2, 3]).expect("write");
        git(repo, &["add", "img.bin"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        let binary = |branch: &str, from: &str, bytes: &[u8]| {
            git(repo, &["checkout", "-q", "-b", branch, from]);
            std::fs::write(repo.join("img.bin"), bytes).expect("write");
            git(repo, &["commit", "-q", "-am", branch]);
            git(repo, &["rev-parse", "HEAD"])
        };
        let x = binary("x", &base, &[0, 9, 9, 9]);
        let y = binary("y", &base, &[0, 7, 7, 7]);
        git(repo, &["checkout", "-q", "main"]);
        let later = commit(repo, "other.rs");
        let x_again = binary("x-again", &later, &[0, 9, 9, 9]);

        let seen = |from: &str, to: &str| patch(&LocalVcs, repo, from, to, 1).expect("git answers");
        assert_ne!(
            seen(&base, &x),
            seen(&base, &y),
            "two contents of one binary file read as one change"
        );
        assert_eq!(
            seen(&base, &x),
            seen(&later, &x_again),
            "the same binary content on another base is the same change"
        );
    }

    /// t-97078 r2 (review t-97475 P2): an added line's whitespace is its content — in Python, YAML
    /// or a Makefile the indent is the meaning, and inside a string it is the value — so a line
    /// moved out of its block by its indent alone is another change. The base's own lines and the
    /// line numbers still read the same on another base.
    #[test]
    fn a_change_of_indent_alone_is_another_change() {
        let diff = |context: &str, at: &str, added: &str| {
            format!(
                "diff --git a/a.py b/a.py\nindex 1111111..2222222 100644\n--- a/a.py\n+++ b/a.py\n\
                 @@ {at} @@\n {context}\n+{added}\n"
            )
        };
        let inside = patch_files(&diff("if x:", "-1,1 +1,2", "    y()"));
        assert_ne!(
            inside,
            patch_files(&diff("if x:", "-1,1 +1,2", "y()")),
            "a line moved out of its block by its indent reads as the same change"
        );
        assert_ne!(
            patch_files(&diff("s = 1", "-1,1 +1,2", "t = \"a b\"")),
            patch_files(&diff("s = 1", "-1,1 +1,2", "t = \"ab\"")),
            "a string that lost a space reads as the same change"
        );
        assert_eq!(
            inside,
            patch_files(&diff("if  x:", "-7,1 +7,2", "    y()")),
            "the base's own line and the line numbers count against a rebase"
        );
    }

    /// t-97078 r3 (review t-97548 P3): an added line is its bytes. A line that only gained a
    /// carriage return — a script turned CRLF in a conflict — is another change, and a diff git
    /// spoke in bytes that are not UTF-8 is no answer at all: two Latin-1 letters both read as
    /// U+FFFD, and the same text is not the same change.
    #[test]
    fn a_line_ending_or_a_byte_that_is_not_utf8_is_never_read_as_the_same_change() {
        let diff = |added: &str| {
            format!(
                "diff --git a/a.sh b/a.sh\nindex 1111111..2222222 100644\n--- a/a.sh\n+++ b/a.sh\n\
                 @@ -1,1 +1,2 @@\n echo\n+{added}\n"
            )
        };
        assert_ne!(
            patch_files(&diff("run")),
            patch_files(&diff("run\r")),
            "a line that only gained a carriage return reads as the same change"
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        let base = commit(repo, "latin1.txt");
        git(repo, &["checkout", "-q", "-b", "e-acute"]);
        std::fs::write(repo.join("latin1.txt"), b"caf\xe9\n").expect("write");
        git(repo, &["commit", "-q", "-am", "e-acute"]);
        let acute = git(repo, &["rev-parse", "HEAD"]);
        assert_eq!(
            patch(&LocalVcs, repo, &base, &acute, 1),
            None,
            "a diff whose bytes are not UTF-8 was read through U+FFFD and judged"
        );
    }

    /// t-97078 r2 (review t-97475 P3-3): a tested head written short and a source written in full
    /// are one commit, and the ledger takes the review as the source itself — not as a change
    /// that adds nothing.
    #[test]
    fn a_short_and_a_full_name_of_one_commit_are_that_commit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        commit(repo, "one");
        let full = commit(repo, "two");
        let short = full[..9].to_string();

        let seen = see_heads(repo, &short, &full).expect("git answers");
        assert_eq!(
            zerocode_core::orchestration::coordinator_take::rebased(Some(&seen), &short, &full),
            Ok(0),
            "{seen:?}"
        );
    }

    /// A git that answers one thing, within its budget and on no other road.
    struct Answering(fn() -> Within);

    impl Vcs for Answering {
        fn text(&self, _root: &Path, _args: &[&str]) -> Result<String, String> {
            panic!("a question with no budget")
        }

        fn text_and_code(&self, _root: &Path, _args: &[&str]) -> Result<(i32, String), String> {
            panic!("a question with no budget")
        }

        fn try_within_max(
            &self,
            _root: &Path,
            _args: &[&str],
            _budget: Duration,
            _max: Option<usize>,
        ) -> Within {
            (self.0)()
        }

        fn clone(
            &self,
            _url: &str,
            _into: &Path,
            _say: &mut dyn FnMut(zerocode_core::clone::CloneStep),
        ) -> Result<(), String> {
            panic!("a question with no budget")
        }
    }

    /// t-84720 (review t-78238 P3-2): every question here has the file's budget,
    /// and `holds` was the one that waited for as long as git took. A backfill
    /// asks it once for each of its commits, one after another.
    #[test]
    fn whether_a_ref_holds_a_commit_is_asked_within_the_budget() {
        let asked = |answer| holds(&Answering(answer), Path::new("/repo"), "main", "abc");
        assert_eq!(asked(|| Within::Said(String::new())), Some(true));
        assert_eq!(asked(|| Within::Refused(String::new())), Some(false));
        assert_eq!(
            asked(|| Within::Refused("fatal: Not a valid commit name abc".into())),
            None,
            "an error is no answer"
        );
        assert_eq!(asked(|| Within::Silent), None);
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
