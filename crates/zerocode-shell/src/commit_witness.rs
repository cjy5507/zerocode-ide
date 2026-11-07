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
/// refs and history and moves no ref; the replay ([`Replayed`]) writes the tree and blobs it makes
/// as loose objects nothing points to, which `git gc` takes away (t-98182 r4, review t-99233 P3-C).
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

/// [`see`], asking `vcs`. A base or a commit git gave no answer for is left out, and a name more
/// than one commit's hash starts with is said to be that ([`CommitsSeen::is_ambiguous`]).
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
        ambiguous: Default::default(),
    };
    let base_head = seen.base.as_ref().and_then(|base| base.head.clone());
    for named in commits {
        let Some(full) = resolve(vcs, repo, named) else {
            continue;
        };
        // A name git knows no commit for may be the start of two (t-98182 r3, review t-99153
        // P3-6): said apart, so the hand-in names it longer instead of hearing it is no commit.
        if full.is_none() && ambiguous(vcs, repo, named) {
            seen.insert_ambiguous(named);
        }
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

/// Whether more than one commit's hash starts with `name` — why git named no commit for it, told
/// apart from a name git knows nothing of (t-98182 L4). Each object git lists for the prefix is
/// asked whether it is a commit: among a blob and a tree git names the one commit it finds, so
/// only two commits leave a commit's name without an answer.
fn ambiguous(vcs: &dyn Vcs, repo: &Path, name: &str) -> bool {
    let asked = format!("--disambiguate={name}");
    let Some(listed) = said(vcs, repo, &["rev-parse", &asked], None) else {
        return false;
    };
    listed
        .lines()
        .map(str::trim)
        .filter(|full| matches!(resolve(vcs, repo, full), Some(Some(_))))
        .nth(1)
        .is_some()
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

/// Git's setting that writes every file name as ASCII, a byte that is not quoted in octal
/// (t-98182 r3, review t-99153 P2-2), for each question that reads names.
const QUOTED_PATHS: &str = "core.quotePath=true";

/// The prefix the change's diff is asked to write before each file's old name, and the one
/// [`file_named`] takes off (t-98182 r5, review t-99295 P3-β): one written apart from the other
/// read every file by its whole `diff --git` line.
const SRC_PREFIX: &str = "a/";

/// The prefix before each file's new name, as [`SRC_PREFIX`] is before its old one.
const DST_PREFIX: &str = "b/";

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
/// short, t-97078 r2) are that commit, and nothing is diffed; nor is anything when a name is
/// short for more than one commit (t-98182 L4), which is said instead. `None` when git gave no
/// answer.
pub(crate) fn see_heads(repo: &Path, tested: &str, source: &str) -> Option<CommitsSeen> {
    let mut seen = CommitsSeen {
        repo: repo.display().to_string(),
        ..Default::default()
    };
    let resolved = [tested, source].map(|named| resolve(&LocalVcs, repo, named).flatten());
    for (named, full) in [tested, source].into_iter().zip(&resolved) {
        match full {
            Some(full) => seen.insert(
                named,
                CommitSeen {
                    full: Some(full.clone()),
                    ..CommitSeen::default()
                },
            ),
            None if ambiguous(&LocalVcs, repo, named) => seen.insert_ambiguous(named),
            None => {}
        }
    }
    if !seen.ambiguous.is_empty() {
        return Some(seen);
    }
    if let [Some(one), Some(other)] = &resolved
        && one == other
    {
        return Some(seen);
    }
    // Asked by the full hashes git named (t-98182 r4, review t-99233 P3-A): a branch named as a
    // short hash makes the name ambiguous, and git says so beside its answer.
    let [tested_full, source_full] = &resolved;
    let (was, now) = see_rebase(
        repo,
        tested_full.as_deref().unwrap_or(tested),
        source_full.as_deref().unwrap_or(source),
    )?;
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
    let replayed = match commits {
        0 => Replayed::Same,
        _ => replay(vcs, repo, meet, &from, tested, source)?,
    };
    let context = match replayed {
        Replayed::Unasked => Context::Kept,
        _ => Context::Dropped,
    };
    let was = patch(vcs, repo, meet, tested, commits, context)?;
    let mut now = patch(
        vcs,
        repo,
        &from,
        source,
        count(vcs, repo, &from, source)?,
        context,
    )?;
    replayed.mark(&was, &mut now);
    Some((was, now))
}

/// What git wrote replaying the tested head's own commits onto the base the source's commits stand
/// on — the rebase as git would have made it (t-98182 r3, review t-99153 P2-1).
///
/// The added and removed lines alone do not say where they went: a guard the tested head put in
/// one function and a landing that put the same line in another read the same without the lines
/// around them. A rebase git made cleanly moves no line, so its tree is the source's; a conflict
/// resolved by hand is exactly what was not tested.
enum Replayed {
    /// Git wrote what the source holds — or there was nothing of the tested head's to replay.
    Same,
    /// Git wrote another content in these files, named as `git diff` names them.
    Differs(Vec<String>),
    /// Git stopped on a conflict: the source is a hand-resolved rebase.
    Conflict,
    /// This git cannot replay a change onto another base (`merge-tree --merge-base` came in git
    /// 2.40), so the lines beside each change are compared instead, as before t-98182 L3.
    Unasked,
}

/// What a git that does not know `merge-tree --write-tree --merge-base` says on its standard
/// error: an option it does not know, or the usage of its older `merge-tree`. Any other failure is
/// read as a conflict (t-98182 r4, review t-99233 P3-A), so a warning beside a conflict — a name
/// that is also a branch's — never sends the replay down the older comparison. The words are
/// git's own, untranslated: the replay is asked in the C locale (r5, review t-99295 P3-α).
const UNKNOWN_QUESTION: [&str; 2] = ["unknown option", "usage:"];

/// What a file's part of the change reads as when git's replay of the tested change does not
/// write what the source holds in it: no digest, so no tested change equals it.
const NOT_REPLAYED: &str = "not what git writes replaying the tested change";

/// What every file the tested head changes reads as when git's replay of that change stops on a
/// conflict: the files git conflicted in are not said when it fails, so none of them is taken.
const CONFLICTED: &str = "git's replay of the tested change stops on a conflict";

impl Replayed {
    /// Write onto `now` — the source's change — what the replay found against it.
    fn mark(self, was: &PatchSeen, now: &mut PatchSeen) {
        let (files, mark): (Vec<String>, _) = match self {
            Self::Same | Self::Unasked => return,
            Self::Differs(files) => (files, NOT_REPLAYED),
            Self::Conflict => (was.files.keys().cloned().collect(), CONFLICTED),
        };
        for file in files {
            now.files.insert(file, mark.to_string());
        }
    }
}

/// Git's replay of `tested`'s commits since `meet` onto `onto` ([`Replayed`]), against what
/// `source` holds. `None` when git gave no answer.
fn replay(
    vcs: &dyn Vcs,
    repo: &Path,
    meet: &str,
    onto: &str,
    tested: &str,
    source: &str,
) -> Option<Replayed> {
    let merge_base = format!("--merge-base={meet}");
    let args = ["merge-tree", "--write-tree", &merge_base, onto, tested];
    // Asked in the C locale: a git older than 2.40 with its messages translated says it does not
    // know the question in the person's language, and every pure rebase read as a conflict.
    let tree = match vcs.try_within_untranslated(repo, &args, GIT_BUDGET) {
        Within::Said(said) => said.lines().next().unwrap_or_default().trim().to_string(),
        // A conflict is git's exit 1 (the files are on its standard output, which a failure
        // does not hand back); a git that does not know the question says so on its standard
        // error, and every other failure is refused as a conflict is.
        Within::Refused(said) if UNKNOWN_QUESTION.iter().any(|words| said.contains(words)) => {
            return Some(Replayed::Unasked);
        }
        Within::Refused(_) => return Some(Replayed::Conflict),
        Within::Silent => return None,
    };
    let holds = said(
        vcs,
        repo,
        &["rev-parse", &format!("{source}^{{tree}}")],
        None,
    )?;
    if !tree.is_empty() && holds.trim() == tree {
        return Some(Replayed::Same);
    }
    let differs = said(
        vcs,
        repo,
        &[
            "-c",
            QUOTED_PATHS,
            "diff",
            "--name-only",
            "--no-ext-diff",
            &tree,
            source,
        ],
        None,
    )?;
    let mut files: Vec<String> = differs
        .lines()
        .map(|file| {
            file.strip_prefix('"')
                .and_then(|file| file.strip_suffix('"'))
                .unwrap_or(file)
                .to_string()
        })
        .filter(|file| !file.is_empty())
        .collect();
    // Two trees git names apart differ somewhere, even where it lists no file.
    if files.is_empty() {
        files.push(NOT_REPLAYED.to_string());
    }
    Some(Replayed::Differs(files))
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

/// The change from `from` to `to`, file by file ([`digest_files`]).
fn patch(
    vcs: &dyn Vcs,
    repo: &Path,
    from: &str,
    to: &str,
    commits: u32,
    context: Context,
) -> Option<PatchSeen> {
    // Read as the bytes git wrote (t-98182 L5, review t-97548 r3 P3-1): read as UTF-8, every byte
    // that is not became one U+FFFD, two Latin-1 letters read the same, and so a diff holding one
    // had to be refused whole — a pure rebase of any file that is not UTF-8 among them.
    // A name is asked quoted (t-98182 r3, review t-99153 P2-2): under `core.quotePath=false` git
    // writes a name that is not UTF-8 as its bytes, and two such names read as one.
    // Each side's prefix is said (r4, review t-99233 P3-B): a person's `diff.noprefix` writes a
    // `diff --git` line with none, and every file was then named by its whole line.
    let src_prefix = format!("--src-prefix={SRC_PREFIX}");
    let dst_prefix = format!("--dst-prefix={DST_PREFIX}");
    let args = [
        "-c",
        QUOTED_PATHS,
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--full-index",
        &src_prefix,
        &dst_prefix,
        from,
        to,
    ];
    match vcs.try_within_max_bytes(repo, &args, GIT_BUDGET, Some(DIFF_MAX)) {
        Within::Said(diff) => Some(PatchSeen {
            commits,
            files: digest_files(diff, context),
        }),
        Within::Refused(_) | Within::Silent => None,
    }
}

/// Whether a change's digest holds the base's own lines beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    /// Left out: git replayed the change and said where it went ([`Replayed`]).
    Dropped,
    /// Kept without their whitespace, and each hunk counted: git could not replay the change,
    /// so the lines around it are what says where it went.
    Kept,
}

/// [`digest_files`] as git's replay leaves it ([`Context::Dropped`]) — what the tests read.
#[cfg(test)]
fn patch_files(diff: impl AsRef<[u8]>) -> std::collections::BTreeMap<String, String> {
    digest_files(diff, Context::Dropped)
}

/// Each file of a `git diff`, with the digest of its part: what the change itself is kept exactly,
/// and only what the base it stands on decides is let go — so the same change at other line
/// numbers, on another base, digests the same, and any other content does not.
///
/// - A hunk header and a context line are left out under [`Context::Dropped`] (t-98182 L3,
///   reviews t-97475 P3-5 and t-97548 P3-2): both are the base's — its line numbers, its own
///   lines beside the change, and where git cut the hunks around them — so a `main` that changed
///   a line next to the change, or put lines between two of its hunks, refused a pure rebase.
///   What stands for the change is then the added and removed lines, in order, in the file they
///   are in, and where they went is git's replay to say ([`Replayed`], r3). Under
///   [`Context::Kept`] a context line is kept without whitespace and a hunk header as `@@`.
/// - The `index` line names the base's blob too, so it is left out — but a binary file's part is
///   only 「Binary files … differ」, so its content is the blob it becomes, read from that line
///   (the diff is asked with `--full-index`), as `git patch-id` counts a binary file.
/// - An added or removed line is kept byte for byte (t-97078 r2, review t-97475 P2), its line
///   ending too (r3), and in whatever encoding it was written (t-98182 L5): the diff is bytes.
///   `git patch-id --stable` drops every line's whitespace because it looks for one commit
///   cherry-picked elsewhere, where a false match only hides a duplicate; here a false match
///   approves code nobody tested, and an indent is code — Python's blocks, YAML, a Makefile's
///   tabs, the space inside a string.
/// - Every other line — a file's own header words — is kept without whitespace.
///
/// A file is named by its `diff --git` line ([`file_named`]).
fn digest_files(
    diff: impl AsRef<[u8]>,
    context: Context,
) -> std::collections::BTreeMap<String, String> {
    use sha2::{Digest, Sha256};
    const FILE_HEAD: &[u8] = b"diff --git ";
    const INDEX: &[u8] = b"index ";
    const HUNK: &[u8] = b"@@";
    const BINARY: [&[u8]; 2] = [b"Binary files ", b"GIT binary patch"];
    // A line both sides of a hunk hold; git writes an empty one as a bare space, or as nothing
    // under `diff.suppressBlankEmpty`.
    const CONTEXT: u8 = b' ';
    const CHANGED: [u8; 2] = [b'+', b'-'];
    let mut files = std::collections::BTreeMap::new();
    // The file open now, its digest so far, and the blob its `index` line says it becomes.
    let mut open: Option<(String, Sha256, Option<String>)> = None;
    // A name git writes twice keeps both parts, in order (t-98182 r4, review t-99233 P3-B): a
    // rename `p` → `q` is named by its whole line, `a/p b/q`, as a file at that path is, and a
    // file whose type changed is two parts under one name — one written over the other hid it.
    let mut close = |open: Option<(String, Sha256, Option<String>)>| {
        if let Some((file, digest, _)) = open {
            let digest = format!("{:x}", digest.finalize());
            files
                .entry(file)
                .and_modify(|before: &mut String| {
                    before.push(' ');
                    before.push_str(&digest);
                })
                .or_insert(digest);
        }
    };
    // Split on `\n` alone: a line that only gained a `\r` — a script turned CRLF — is another
    // change (t-97078 r3).
    for line in diff.as_ref().split(|byte| *byte == b'\n') {
        if let Some(named) = line.strip_prefix(FILE_HEAD) {
            close(open.take());
            open = Some((file_named(named), Sha256::new(), None));
            continue;
        }
        let Some((_, digest, becomes)) = open.as_mut() else {
            continue;
        };
        if let Some(blobs) = line.strip_prefix(INDEX) {
            *becomes = String::from_utf8_lossy(blobs)
                .split_whitespace()
                .next()
                .and_then(|blobs| blobs.split_once(".."))
                .map(|(_, after)| after.to_string());
            continue;
        }
        match line.first() {
            None | Some(&CONTEXT) if context == Context::Dropped => continue,
            _ if line.starts_with(HUNK) => match context {
                Context::Dropped => continue,
                Context::Kept => digest.update(HUNK),
            },
            _ if BINARY.iter().any(|head| line.starts_with(head)) => {
                digest.update(b"binary ");
                digest.update(becomes.as_deref().unwrap_or_default().as_bytes());
            }
            Some(first) if CHANGED.contains(first) => digest.update(line),
            _ => digest.update(
                line.iter()
                    .copied()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .collect::<Vec<u8>>(),
            ),
        }
        digest.update(b"\n");
    }
    close(open);
    files
}

/// The file a `diff --git` line names, past its `diff --git `: the path both its halves name —
/// `a/<path> b/<path>`, or `"a/<path>" "b/<path>"` when git quoted it — and the whole line when
/// they name two (a rename), so no two lines read as one file (t-98182 r3, review t-99153 P2-2).
/// Cut at its last ` b/`, as it was before, a path holding one read as what follows it. The diff is asked with
/// `core.quotePath=true`, so a name is ASCII; a byte that is not reads as U+FFFD only in a name
/// git wrote unasked.
fn file_named(named: &[u8]) -> String {
    let named = String::from_utf8_lossy(named);
    let (open, between, close) = if named.starts_with('"') {
        (
            format!("\"{SRC_PREFIX}"),
            format!("\" \"{DST_PREFIX}"),
            "\"",
        )
    } else {
        (SRC_PREFIX.to_string(), format!(" {DST_PREFIX}"), "")
    };
    let halves = named
        .strip_prefix(open.as_str())
        .and_then(|inner| inner.strip_suffix(close))
        .and_then(|inner| {
            let half = inner.len().checked_sub(between.len())? / 2;
            let (one, rest) = inner.split_at_checked(half)?;
            (rest.strip_prefix(between.as_str())? == one).then_some(one)
        });
    halves.unwrap_or(&named).to_string()
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

        let seen = |from: &str, to: &str| {
            patch(&LocalVcs, repo, from, to, 1, Context::Dropped).expect("git answers")
        };
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
        let inside = patch_files(diff("if x:", "-1,1 +1,2", "    y()"));
        assert_ne!(
            inside,
            patch_files(diff("if x:", "-1,1 +1,2", "y()")),
            "a line moved out of its block by its indent reads as the same change"
        );
        assert_ne!(
            patch_files(diff("s = 1", "-1,1 +1,2", "t = \"a b\"")),
            patch_files(diff("s = 1", "-1,1 +1,2", "t = \"ab\"")),
            "a string that lost a space reads as the same change"
        );
        assert_eq!(
            inside,
            patch_files(diff("if  x:", "-7,1 +7,2", "    y()")),
            "the base's own line and the line numbers count against a rebase"
        );
    }

    /// t-97078 r3 (review t-97548 P3): an added line is its bytes. A line that only gained a
    /// carriage return — a script turned CRLF in a conflict — is another change, and so is a byte
    /// that is not UTF-8: two Latin-1 letters both read as U+FFFD, and the diff is read in its own
    /// bytes so they never read as one (t-98182 L5).
    #[test]
    fn a_line_ending_or_a_byte_that_is_not_utf8_is_never_read_as_the_same_change() {
        let diff = |added: &str| {
            format!(
                "diff --git a/a.sh b/a.sh\nindex 1111111..2222222 100644\n--- a/a.sh\n+++ b/a.sh\n\
                 @@ -1,1 +1,2 @@\n echo\n+{added}\n"
            )
        };
        assert_ne!(
            patch_files(diff("run")),
            patch_files(diff("run\r")),
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
        git(repo, &["checkout", "-q", "-b", "e-grave", &base]);
        std::fs::write(repo.join("latin1.txt"), b"caf\xe8\n").expect("write");
        git(repo, &["commit", "-q", "-am", "e-grave"]);
        let grave = git(repo, &["rev-parse", "HEAD"]);
        assert_ne!(
            patch(&LocalVcs, repo, &base, &acute, 1, Context::Dropped).expect("git answers"),
            patch(&LocalVcs, repo, &base, &grave, 1, Context::Dropped).expect("git answers"),
            "two bytes that are not UTF-8 were read through U+FFFD as one"
        );
    }

    /// t-98182 L3 (reviews t-97475 P3-5, t-97548 P3-2): a pure rebase onto a `main` that changed
    /// a line next to the change — inside git's three lines of context — is the same change: the
    /// base's own lines are not the change, and neither is where git cut its hunks. Only the added
    /// and removed lines and the file they are in are compared, and those still exactly.
    #[test]
    fn a_rebase_onto_a_main_that_changed_a_neighbouring_line_is_the_same_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let lines = |edit: &dyn Fn(&mut Vec<String>)| {
            let mut lines: Vec<String> = (1..=12).map(|n| format!("line {n}")).collect();
            edit(&mut lines);
            lines.join("\n") + "\n"
        };
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("lib.txt"), lines(&|_| {})).expect("write");
        git(repo, &["add", "lib.txt"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        git(repo, &["checkout", "-q", "-b", "work"]);
        std::fs::write(
            repo.join("lib.txt"),
            lines(&|lines| lines.insert(6, "added by work".to_string())),
        )
        .expect("write");
        git(repo, &["commit", "-q", "-am", "work"]);
        let tested = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        std::fs::write(
            repo.join("lib.txt"),
            lines(&|lines| lines[3] = "line 4 changed by main".to_string()),
        )
        .expect("write");
        git(repo, &["commit", "-q", "-am", "main"]);
        git(repo, &["checkout", "-q", "-b", "landed", "work"]);
        git(repo, &["rebase", "-q", "main"]);
        let rebased = git(repo, &["rev-parse", "HEAD"]);

        let (was, now) = see_rebase(repo, &tested, &rebased).expect("git answers");
        assert_eq!(
            was, now,
            "a neighbouring line main changed reads as another change"
        );

        // Where git cuts its hunks is the base's doing too: the same lines in one hunk or in two.
        let head =
            "diff --git a/a.rs b/a.rs\nindex 1111111..2222222 100644\n--- a/a.rs\n+++ b/a.rs\n";
        assert_eq!(
            patch_files(format!("{head}@@ -1,2 +1,4 @@\n a\n+b\n c\n+d\n")),
            patch_files(format!(
                "{head}@@ -1,1 +1,2 @@\n a\n+b\n@@ -9,1 +10,2 @@\n c\n+d\n"
            )),
            "the same lines in one hunk and in two read as two changes"
        );
        assert_ne!(
            patch_files(format!("{head}@@ -1,1 +1,2 @@\n a\n+b\n")),
            patch_files(format!("{head}@@ -1,1 +1,2 @@\n a\n+b \n")),
            "an added line that gained a space reads as the same change"
        );
    }

    /// A commit on `from`, on a branch named `branch`, that writes `lib.rs` as `text`.
    fn lib_on(repo: &Path, from: &str, branch: &str, text: &str) -> String {
        git(repo, &["checkout", "-q", "-B", branch, from]);
        std::fs::write(repo.join("lib.rs"), text).expect("write");
        git(repo, &["commit", "-q", "-am", branch]);
        git(repo, &["rev-parse", "HEAD"])
    }

    /// A repository whose `lib.rs` is `base`, with a later `main` that added another file: the
    /// base and that `main`.
    fn base_and_a_later_main(repo: &Path, base: &str) -> (String, String) {
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("lib.rs"), base).expect("write");
        git(repo, &["add", "lib.rs"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        let first = git(repo, &["rev-parse", "HEAD"]);
        let later = commit(repo, "other.rs");
        (first, later)
    }

    /// t-98182 r3 (review t-99153 P2-1): the added and removed lines alone do not say where they
    /// went. A guard the tested head put in `transfer()` and a hand-resolved landing put in
    /// `refund()` is the same `+` line, and so is one `check();` removed from `a()` or from `b()`
    /// — two changes nobody tested as one. Git replays the tested change onto the landing's base,
    /// and the landing is the same change only when that writes what it holds.
    #[test]
    fn the_same_lines_put_in_another_function_are_another_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let (base, main) = base_and_a_later_main(
            repo,
            "fn transfer() {\n    debit();\n    credit();\n}\n\nfn refund() {\n    credit();\n}\n",
        );
        let tested = lib_on(
            repo,
            &base,
            "tested",
            "fn transfer() {\n    require_auth()?;\n    debit();\n    credit();\n}\n\n\
             fn refund() {\n    credit();\n}\n",
        );
        let landed = lib_on(
            repo,
            &main,
            "landed",
            "fn transfer() {\n    debit();\n    credit();\n}\n\n\
             fn refund() {\n    require_auth()?;\n    credit();\n}\n",
        );
        let (was, now) = see_rebase(repo, &tested, &landed).expect("git answers");
        assert_eq!(
            was.differing_files(&now),
            vec!["lib.rs".to_string()],
            "a guard landed in another function reads as the change that was tested"
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let (base, _) = base_and_a_later_main(
            repo,
            "fn a() {\n    check();\n    go();\n}\nfn b() {\n    check();\n    stop();\n}\n",
        );
        let tested = lib_on(
            repo,
            &base,
            "tested",
            "fn a() {\n    go();\n}\nfn b() {\n    check();\n    stop();\n}\n",
        );
        let landed = lib_on(
            repo,
            &base,
            "landed",
            "fn a() {\n    check();\n    go();\n}\nfn b() {\n    stop();\n}\n",
        );
        let (was, now) = see_rebase(repo, &tested, &landed).expect("git answers");
        assert_eq!(
            was.differing_files(&now),
            vec!["lib.rs".to_string()],
            "the other copy of a removed line reads as the line that was removed"
        );

        // A rebase git stops on, resolved by hand: the tested change is not what landed.
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let (base, _) = base_and_a_later_main(repo, "fn pay() {\n    debit();\n}\n");
        let main = lib_on(repo, "main", "main", "fn pay() {\n    debit(1);\n}\n");
        let tested = lib_on(repo, &base, "tested", "fn pay() {\n    debit(2);\n}\n");
        let landed = lib_on(repo, &main, "landed", "fn pay() {\n    debit(2);\n}\n");
        let (was, now) = see_rebase(repo, &tested, &landed).expect("git answers");
        assert_eq!(
            now.files.get("lib.rs").map(String::as_str),
            Some(CONFLICTED),
            "a hand-resolved conflict is not said to be one"
        );
        assert_eq!(was.differing_files(&now), vec!["lib.rs".to_string()]);
    }

    /// A git whose replay (`merge-tree`) fails saying the first on its standard error when it is
    /// asked untranslated, and the second — the person's translation — when it is not: every
    /// other question is this machine's git's.
    struct MergeTreeSays(&'static str, &'static str);

    /// A git older than 2.40, which has no `merge-tree --merge-base`.
    const NO_MERGE_BASE: MergeTreeSays =
        MergeTreeSays::untranslated("error: unknown option `merge-base=…'");

    impl MergeTreeSays {
        /// A git that says `said` in every locale.
        const fn untranslated(said: &'static str) -> Self {
            Self(said, said)
        }

        fn replays(args: &[&str]) -> bool {
            args.first() == Some(&"merge-tree")
        }
    }

    impl Vcs for MergeTreeSays {
        fn text(&self, root: &Path, args: &[&str]) -> Result<String, String> {
            LocalVcs.text(root, args)
        }

        fn text_and_code(&self, root: &Path, args: &[&str]) -> Result<(i32, String), String> {
            LocalVcs.text_and_code(root, args)
        }

        fn try_within_max(
            &self,
            root: &Path,
            args: &[&str],
            budget: Duration,
            max: Option<usize>,
        ) -> Within {
            if Self::replays(args) {
                return Within::Refused(self.1.to_string());
            }
            LocalVcs.try_within_max(root, args, budget, max)
        }

        fn try_within_untranslated(&self, root: &Path, args: &[&str], budget: Duration) -> Within {
            if Self::replays(args) {
                return Within::Refused(self.0.to_string());
            }
            LocalVcs.try_within_untranslated(root, args, budget)
        }

        fn try_within_max_bytes(
            &self,
            root: &Path,
            args: &[&str],
            budget: Duration,
            max: Option<usize>,
        ) -> Within<Vec<u8>> {
            if Self::replays(args) {
                return Within::Refused(self.1.to_string());
            }
            LocalVcs.try_within_max_bytes(root, args, budget, max)
        }

        fn clone(
            &self,
            url: &str,
            into: &Path,
            say: &mut dyn FnMut(zerocode_core::clone::CloneStep),
        ) -> Result<(), String> {
            Vcs::clone(&LocalVcs, url, into, say)
        }
    }

    /// t-98182 r3 (review t-99153 P2-1): a git that cannot replay the change keeps the base's own
    /// lines beside each changed line, as before L3 — a neighbour `main` changed is then another
    /// change, and a guard moved into another function is too.
    #[test]
    fn a_git_that_cannot_replay_the_change_compares_the_lines_beside_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let (base, main) = base_and_a_later_main(
            repo,
            "fn transfer() {\n    debit();\n    credit();\n}\n\nfn refund() {\n    credit();\n}\n",
        );
        let tested = lib_on(
            repo,
            &base,
            "tested",
            "fn transfer() {\n    require_auth()?;\n    debit();\n    credit();\n}\n\n\
             fn refund() {\n    credit();\n}\n",
        );
        let landed = lib_on(
            repo,
            &main,
            "landed",
            "fn transfer() {\n    debit();\n    credit();\n}\n\n\
             fn refund() {\n    require_auth()?;\n    credit();\n}\n",
        );
        let (was, now) =
            see_rebase_with(&NO_MERGE_BASE, repo, &tested, &landed).expect("git answers");
        assert_eq!(
            was.differing_files(&now),
            vec!["lib.rs".to_string()],
            "an old git reads a guard landed in another function as the change that was tested"
        );

        git(repo, &["checkout", "-q", "-B", "rebased", &tested]);
        git(repo, &["rebase", "-q", &main]);
        let rebased = git(repo, &["rev-parse", "HEAD"]);
        let (was, now) =
            see_rebase_with(&NO_MERGE_BASE, repo, &tested, &rebased).expect("git answers");
        assert_eq!(
            was, now,
            "an old git refuses a pure rebase onto another file"
        );
    }

    /// t-98182 r4 (review t-99233 P3-A): a replay git stops on is a conflict whatever it says
    /// beside it. A branch named as the tested head's short hash makes git warn that the name is
    /// ambiguous on its standard error, and the conflict then read as a git that cannot replay —
    /// whose comparison takes a hand-resolved binary file as the tested one. The heads are asked
    /// by their full hashes, and only a git that says it does not know the question is old.
    #[test]
    fn a_conflict_git_warns_beside_is_still_a_conflict() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("b.bin"), b"A\0base").expect("write");
        git(repo, &["add", "b.bin"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        let binary = |branch: &str, from: &str, bytes: &[u8]| {
            git(repo, &["checkout", "-q", "-B", branch, from]);
            std::fs::write(repo.join("b.bin"), bytes).expect("write");
            git(repo, &["commit", "-q", "-am", branch]);
            git(repo, &["rev-parse", "HEAD"])
        };
        let main = binary("main", &base, b"A\0main");
        let tested = binary("tested", &base, b"A\0test");
        let landed = binary("landed", &main, b"A\0test");
        let short = tested[..9].to_string();
        git(repo, &["branch", &short, &tested]);

        let seen = see_heads(repo, &short, &landed).expect("git answers");
        let refused =
            zerocode_core::orchestration::coordinator_take::rebased(Some(&seen), &short, &landed)
                .expect_err("a hand-resolved conflict git warned beside was taken");
        assert!(refused.contains("b.bin"), "{refused}");

        let replayed = |says: &'static str| {
            see_rebase_with(&MergeTreeSays::untranslated(says), repo, &tested, &landed)
                .expect("git answers")
        };
        let (was, now) = replayed("warning: refname 'abc' is ambiguous.\n");
        assert_eq!(
            now.files.get("b.bin").map(String::as_str),
            Some(CONFLICTED),
            "a failed replay with words beside it read as a git that cannot replay"
        );
        assert_eq!(was.differing_files(&now), vec!["b.bin".to_string()]);
        let (was, now) = replayed("usage: git merge-tree <base-tree> <branch1> <branch2>\n");
        assert_eq!(
            was, now,
            "a git that does not know the question is compared the old way"
        );
    }

    /// t-98182 r5 (review t-99295 P3-α): a git older than 2.40 whose messages are translated says
    /// it does not know the question in the person's language. Read as a conflict, every pure
    /// rebase was refused; the replay is asked in the C locale, so such a git is compared as an
    /// old git is.
    #[test]
    fn an_old_git_in_another_language_is_asked_in_the_c_locale() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let (base, main) = base_and_a_later_main(repo, "fn transfer() {\n    debit();\n}\n");
        let tested = lib_on(
            repo,
            &base,
            "tested",
            "fn transfer() {\n    require_auth()?;\n    debit();\n}\n",
        );
        git(repo, &["checkout", "-q", "-B", "rebased", &tested]);
        git(repo, &["rebase", "-q", &main]);
        let rebased = git(repo, &["rev-parse", "HEAD"]);
        for old in [
            MergeTreeSays(
                "error: unknown option `merge-base=…'",
                "오류: 알 수 없는 옵션 `merge-base=…'",
            ),
            MergeTreeSays(
                "usage: git merge-tree <base-tree> <branch1> <branch2>",
                "사용법: git merge-tree <base-tree> <branch1> <branch2>",
            ),
        ] {
            let (was, now) = see_rebase_with(&old, repo, &tested, &rebased).expect("git answers");
            assert_eq!(
                was, now,
                "a pure rebase is refused when an old git says {:?} in another language",
                old.0
            );
        }
    }

    /// t-98182 r4 (review t-99233 P3-B): a rename `p` → `q` is named by its whole `diff --git`
    /// line, `a/p b/q`, and so is a file at the path `a/p b/q`. The second part git wrote under
    /// that name wrote over the first, and a change to the file went unseen — an old git took a
    /// landing that put other lines in it as the tested change. Both parts are kept, in order; and
    /// a person's `diff.noprefix` does not change how git names a file.
    #[test]
    fn a_rename_named_as_another_files_path_does_not_hide_that_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        let lines: String = (1..=20).map(|n| format!("{n}\n")).collect();
        std::fs::write(repo.join("p"), lines).expect("write");
        std::fs::create_dir_all(repo.join("a/p b")).expect("mkdir");
        std::fs::write(repo.join("a/p b/q"), "1\n").expect("write");
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        let moved = |branch: &str, added: &str| {
            git(repo, &["checkout", "-q", "-B", branch, &base]);
            git(repo, &["mv", "p", "q"]);
            std::fs::write(repo.join("a/p b/q"), format!("1\n{added}\n")).expect("write");
            git(repo, &["commit", "-q", "-am", branch]);
            git(repo, &["rev-parse", "HEAD"])
        };
        let tested = moved("tested", "T");
        let landed = moved("landed", "EVIL");
        let differing = || {
            let (was, now) =
                see_rebase_with(&NO_MERGE_BASE, repo, &tested, &landed).expect("git answers");
            was.differing_files(&now)
        };
        assert_eq!(
            differing(),
            vec!["a/p b/q".to_string()],
            "a rename named as a file's path hid that file's other lines"
        );
        git(repo, &["config", "diff.noprefix", "true"]);
        assert_eq!(
            differing(),
            vec!["a/p b/q".to_string()],
            "diff.noprefix changed how a file is named"
        );
    }

    /// t-98182 L5 (review t-97548 r3 P3-1): a diff is read as the bytes git wrote. A pure rebase
    /// of a file that is not UTF-8 — a Latin-1, EUC-KR or Shift-JIS source, or a line holding
    /// U+FFFD itself — is the same change, where reading the diff as UTF-8 had to refuse it; and two
    /// Latin-1 letters that both read as U+FFFD are still two changes.
    #[test]
    fn a_rebase_of_a_file_that_is_not_utf8_is_read_in_its_own_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("latin1.txt"), b"caf\xe9\nna\xefve \xef\xbf\xbd\n")
            .expect("write");
        git(repo, &["add", "latin1.txt"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        let base = git(repo, &["rev-parse", "HEAD"]);
        let letter = |branch: &str, from: &str, line: &[u8]| {
            git(repo, &["checkout", "-q", "-b", branch, from]);
            let mut bytes = std::fs::read(repo.join("latin1.txt")).expect("read");
            bytes.extend_from_slice(line);
            std::fs::write(repo.join("latin1.txt"), bytes).expect("write");
            git(repo, &["commit", "-q", "-am", branch]);
            git(repo, &["rev-parse", "HEAD"])
        };
        let grave = letter("grave", &base, b"cr\xe8me\n");
        let circumflex = letter("circumflex", &base, b"cr\xeame\n");
        git(repo, &["checkout", "-q", "main"]);
        commit(repo, "other.rs");
        git(repo, &["checkout", "-q", "-b", "landed", "grave"]);
        git(repo, &["rebase", "-q", "main"]);
        let rebased = git(repo, &["rev-parse", "HEAD"]);

        let (was, now) =
            see_rebase(repo, &grave, &rebased).expect("a diff that is not UTF-8 was no answer");
        assert_eq!(
            was, now,
            "a pure rebase of a Latin-1 file reads as another change"
        );
        let (was, now) = see_rebase(repo, &grave, &circumflex).expect("git answers");
        assert_eq!(
            was.differing_files(&now),
            vec!["latin1.txt".to_string()],
            "two Latin-1 letters read as one change"
        );
    }

    /// t-98182 r3 (review t-99153 P2-2): a file is named by its bytes. Under `core.quotePath=false`
    /// — common where people write Korean — git writes a name that is not UTF-8 as it is, and
    /// `caf\xe8.txt` and `caf\xe9.txt` both read as `caf\u{FFFD}.txt`; a name holding ` b/` cut
    /// at its last one read `x b/y` and `z b/y` both as `y`. One file's part then wrote over the
    /// other's, and a change to the first went unseen.
    #[test]
    fn two_files_whose_names_read_alike_are_two_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "core.quotePath", "false"]);
        let names: [&[u8]; 4] = [b"caf\xe8.txt", b"caf\xe9.txt", b"x b/y", b"z b/y"];
        let mut stream = Vec::new();
        let mut write_commit = |branch: &str, mark: u32, from: Option<u32>, adds: [&str; 4]| {
            stream.extend_from_slice(
                format!(
                    "commit refs/heads/{branch}\nmark :{mark}\n\
                     committer dev <dev@example.com> 1700000000 +0000\ndata 0\n"
                )
                .as_bytes(),
            );
            if let Some(from) = from {
                stream.extend_from_slice(format!("from :{from}\n").as_bytes());
            }
            for (name, added) in names.iter().zip(adds) {
                let body = format!("a\n{added}");
                stream.extend_from_slice(b"M 644 inline ");
                stream.extend_from_slice(name);
                stream.extend_from_slice(format!("\ndata {}\n{body}\n", body.len()).as_bytes());
            }
        };
        write_commit("main", 1, None, [""; 4]);
        write_commit("tested", 2, Some(1), ["x\n", "y\n", "1\n", "2\n"]);
        write_commit("landed", 3, Some(1), ["EVIL\n", "y\n", "9\n", "2\n"]);
        let fed = dir.path().join("stream");
        std::fs::write(&fed, stream).expect("write");
        let imported = crate::proc::quiet_command("git")
            .current_dir(repo)
            .args(["fast-import", "--quiet"])
            .stdin(std::fs::File::open(&fed).expect("open"))
            .status()
            .expect("git runs");
        assert!(imported.success());

        let seen = |head: &str| {
            patch(&LocalVcs, repo, "main", head, 1, Context::Dropped).expect("git answers")
        };
        let (was, now) = (seen("tested"), seen("landed"));
        assert_eq!(was.files.len(), 4, "two files read as one: {was:?}");
        assert_eq!(
            was.differing_files(&now),
            vec!["caf\\350.txt".to_string(), "x b/y".to_string()],
            "a change to a file whose name reads like another's went unseen"
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

    /// A repository with enough commits on `main` that two hashes share their first four digits,
    /// git's shortest name: twelve hundred among 65,536 prefixes all but always do, and the pair
    /// is looked for. The four digits, and the commits they start.
    fn two_commits_one_prefix(repo: &Path) -> (String, Vec<String>) {
        use std::fmt::Write as _;
        git(repo, &["init", "-q", "-b", "main"]);
        let mut stream = String::new();
        for n in 0..1200 {
            let body = n.to_string();
            write!(
                stream,
                "commit refs/heads/main\ncommitter dev <dev@example.com> {} +0000\ndata {}\n{body}\n\
                 M 644 inline f\ndata {}\n{body}\n",
                1_700_000_000 + n,
                body.len(),
                body.len(),
            )
            .expect("write");
        }
        let fed = repo.join(".git").join("stream");
        std::fs::write(&fed, stream).expect("write");
        let imported = crate::proc::quiet_command("git")
            .current_dir(repo)
            .args(["fast-import", "--quiet"])
            .stdin(std::fs::File::open(&fed).expect("open"))
            .status()
            .expect("git runs");
        assert!(imported.success());
        let mut by_prefix = std::collections::BTreeMap::<String, Vec<String>>::new();
        for full in git(repo, &["rev-list", "main"]).lines() {
            by_prefix
                .entry(full[..4].to_string())
                .or_default()
                .push(full.to_string());
        }
        by_prefix
            .into_iter()
            .find(|(_, fulls)| fulls.len() > 1)
            .expect("two commits share four digits")
    }

    /// t-98182 L4 (review t-97475 r2 P3-3): a short name that more than one commit's hash starts
    /// with is no commit git can name, and the ledger says that — not that git was not asked, and
    /// not that the two heads change different things.
    #[test]
    fn a_short_name_more_than_one_commit_starts_with_is_refused_as_that() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let (short, fulls) = two_commits_one_prefix(repo);
        let other = git(repo, &["rev-parse", "main"]);

        let seen = see_heads(repo, &short, &other);
        let refused =
            zerocode_core::orchestration::coordinator_take::rebased(seen.as_ref(), &short, &other)
                .expect_err("an ambiguous name was taken");
        assert!(
            refused.contains(&short) && refused.contains("more than one commit"),
            "{short} names {fulls:?}, and the refusal does not say so: {refused}"
        );
    }

    /// t-98182 r3 (review t-99153 P3-6): a hand-in's commit is read by [`see`], and a short name
    /// more than one commit's hash starts with is said to be that there too.
    #[test]
    fn a_hand_ins_short_name_more_than_one_commit_starts_with_is_said_to_be_that() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let (short, fulls) = two_commits_one_prefix(repo);

        let seen = see(repo, std::slice::from_ref(&short), Some("main"), None);
        assert!(
            seen.is_ambiguous(&short),
            "{short} names {fulls:?}: {seen:?}"
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
