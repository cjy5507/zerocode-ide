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
    BaseSeen, CommitSeen, CommitsSeen, PatchSeen, RELEASE_REMOTE_FLAG, RangeSeen,
};

/// How long one git question may take before it counts as no answer. Every question here reads
/// refs and history and moves no ref; the replay ([`Replayed`]) writes the tree and blobs it makes
/// as loose objects nothing points to, which `git gc` takes away (t-98182 r4, review t-99233 P3-C).
const GIT_BUDGET: Duration = Duration::from_secs(10);

/// What git says in `repo` about each of `commits`: the full hash, whether `base` holds it, and the
/// first release tag matching `tags` that holds it and that the remote `release` names holds
/// ([`Releases`]) — the last two asked only of a commit git knows, and the tag only of one the base
/// holds. With no `release` named no release is read, and the commit says why (t-119569).
pub(crate) fn see(
    repo: &Path,
    commits: &[String],
    base: Option<&str>,
    tags: Option<&str>,
    release: Option<&str>,
) -> CommitsSeen {
    see_with(&LocalVcs, repo, commits, base, tags, release)
}

/// [`see`], asking `vcs`. A base or a commit git gave no answer for is left out, and a name more
/// than one commit's hash starts with is said to be that ([`CommitsSeen::is_ambiguous`]).
fn see_with(
    vcs: &dyn Vcs,
    repo: &Path,
    commits: &[String],
    base: Option<&str>,
    tags: Option<&str>,
    release: Option<&str>,
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
        ..CommitsSeen::default()
    };
    let base_head = seen.base.as_ref().and_then(|base| base.head.clone());
    let mut releases = Releases::new(vcs, repo, tags, base, release);
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
        let (tag, unpublished) = match &full {
            Some(full) if in_base == Some(true) => releases.release_of(full),
            _ => (None, None),
        };
        seen.insert(
            named,
            CommitSeen {
                full,
                in_base,
                tag,
                unpublished,
            },
        );
    }
    seen
}

/// What git says in `repo` about the range each of `heads` reviewed (t-119569), against `base`,
/// written into `seen` ([`CommitsSeen::insert_range`]): the head's own commits — the ones `base`
/// did not hold when the work forked — each with whether `base`'s first-parent line holds that
/// change now ([`change_of`]: the same change by the same author at the same moment under the same
/// subject, as a cherry-pick or a rebase keeps it); the merges inside the range and its commits
/// that change nothing, apart; the commit of `base` the last of them landed as; the commits of
/// `base` that revert that landing; and the release of that landing as [`see`] reads one
/// ([`Releases`]: only from the remote `release` names). A head git gave no answer for is left
/// out, and the ledger says git was not asked.
pub(crate) fn see_ranges(
    seen: &mut CommitsSeen,
    repo: &Path,
    heads: &[String],
    base: &str,
    tags: &str,
    release: Option<&str>,
) {
    see_ranges_with(&LocalVcs, seen, repo, heads, base, tags, release);
}

fn see_ranges_with(
    vcs: &dyn Vcs,
    seen: &mut CommitsSeen,
    repo: &Path,
    heads: &[String],
    base: &str,
    tags: &str,
    release: Option<&str>,
) {
    let Some(Some(base_head)) = resolve(vcs, repo, base) else {
        return;
    };
    let mut releases = Releases::new(vcs, repo, Some(tags), Some(base), release);
    for named in heads {
        let Some(Some(head)) = resolve(vcs, repo, named) else {
            continue;
        };
        let Some(mut range) = range_of(vcs, repo, &head, &base_head) else {
            continue;
        };
        if let Some(landed) = range.landed_as.clone() {
            (range.tag, range.unpublished) = releases.release_of(&landed);
        }
        seen.insert_range(named, range);
    }
}

/// The remote whose copy of the base a release tag must sit on: where the base is pushed.
const ORIGIN: &str = "origin";

/// Release tags by name, each with the object it names (a tag object, or a commit for a light one).
type Tags = std::collections::BTreeMap<String, String>;

/// `remote` as a reason names it: its host and path, never the user name, password or token a URL
/// may carry before an `@` (t-119569) — the reasons reach the verb's answer, and with it the
/// ledger every pane reads. Everything up to a URL's last `@` goes, and so does an scp-like
/// remote's user (`user@host:path`); a remote's name and a path are as given.
fn shown_remote(remote: &str) -> String {
    match remote.split_once("://") {
        Some((scheme, rest)) => {
            let place = rest.rsplit_once('@').map_or(rest, |(_, place)| place);
            format!("{scheme}://{place}")
        }
        None if !remote.starts_with(['/', '.']) => remote
            .rsplit_once('@')
            .map_or(remote, |(_, place)| place)
            .to_string(),
        None => remote.to_string(),
    }
}

/// Where a release is read from (t-119569), for both of the backfill's roads: the release tags
/// `tags` names, the base as origin holds it (`published`, none where this repository has no
/// `origin/<base>`), and the remote releases are published to — named by the caller
/// ([`zerocode_core::orchestration::coordinator_take::RELEASE_REMOTE_FLAG`]), with no default: this
/// project's private origin may hold a release's tag before the lane publishes it (the public
/// snapshot needs it there first), so a tag there proves nothing unless origin is the remote
/// named. What that remote and this repository hold of the tags is asked once, and only where a tag
/// may count.
struct Releases<'a> {
    vcs: &'a dyn Vcs,
    repo: &'a Path,
    tags: Option<&'a str>,
    base: Option<&'a str>,
    remote: Option<&'a str>,
    published: Option<String>,
    on_remote: Option<Option<RemoteRefs>>,
    shared: Option<bool>,
    here: Option<Option<Tags>>,
}

impl<'a> Releases<'a> {
    fn new(
        vcs: &'a dyn Vcs,
        repo: &'a Path,
        tags: Option<&'a str>,
        base: Option<&'a str>,
        remote: Option<&'a str>,
    ) -> Self {
        let published = base
            .filter(|_| tags.is_some())
            .map(|base| format!("{ORIGIN}/{base}"))
            .filter(|branch| matches!(resolve(vcs, repo, branch), Some(Some(_))));
        Releases {
            vcs,
            repo,
            tags,
            base,
            remote,
            published,
            on_remote: None,
            shared: None,
            here: None,
        }
    }

    /// The release of `landed` and, where a tag may hold it and none is read as its release, why:
    /// the first tag in version order that holds it, that the published base holds, and that the
    /// remote named holds at the same commit as here (each tag peeled to its commit) — or, on a
    /// remote that shares none of this history (a published snapshot keeps its own), a tag of that
    /// name. On a remote that shares this history a tag at another commit — an annotated tag made
    /// elsewhere, one moved onto a commit not fetched here — is no release. `(None, None)` where
    /// nothing was asked or no tag holds it.
    fn release_of(&mut self, landed: &str) -> (Option<String>, Option<String>) {
        let (Some(tags), Some(base)) = (self.tags, self.base) else {
            return (None, None);
        };
        let Some(published) = self.published.as_deref() else {
            return (
                None,
                Some(format!(
                    "there is no {ORIGIN}/{base} to read a published release from"
                )),
            );
        };
        let (vcs, repo) = (self.vcs, self.repo);
        let Some(held) = held_tags(vcs, repo, landed, tags, published) else {
            return (None, None);
        };
        let Some(first) = held.first() else {
            return (None, None);
        };
        let Some(asked) = self.remote else {
            return (
                None,
                Some(format!(
                    "{first} holds it, but no release remote was named ({RELEASE_REMOTE_FLAG} \
                     <remote or URL>) — a release is read only where the caller names it published"
                )),
            );
        };
        let remote = shown_remote(asked);
        let Some(there) = self
            .on_remote
            .get_or_insert_with(|| refs_on(vcs, repo, asked))
        else {
            return (
                None,
                Some(format!(
                    "{remote} could not be asked whether {first} is published there"
                )),
            );
        };
        let Some(here) = self.here.get_or_insert_with(|| tags_here(vcs, repo)) else {
            return (None, None);
        };
        let shared = *self
            .shared
            .get_or_insert_with(|| shares_history(vcs, repo, asked, &there.commits));
        // A tag there at another commit than here's, on a remote that shares this history.
        let mut elsewhere = None;
        let released = held
            .iter()
            .find(|tag| match there.tags.get(*tag) {
                None => false,
                Some(commit) if here.get(*tag) == Some(commit) => true,
                Some(_) if shared => {
                    elsewhere.get_or_insert_with(|| (*tag).clone());
                    false
                }
                Some(_) => true,
            })
            .cloned();
        match (released, elsewhere) {
            (Some(tag), _) => (Some(tag), None),
            (None, Some(tag)) => (
                None,
                Some(format!(
                    "{remote} holds {tag} at another commit than this repository's {tag} — fetch \
                     its tags and ask again"
                )),
            ),
            (None, None) => (
                None,
                Some(format!(
                    "{first} holds it, but {remote} has no such tag yet — the release has not been \
                     published there"
                )),
            ),
        }
    }
}

/// The tags matching `tags`, in version order, that hold `commit` and that `published` holds.
fn held_tags(
    vcs: &dyn Vcs,
    repo: &Path,
    commit: &str,
    tags: &str,
    published: &str,
) -> Option<Vec<String>> {
    let asked = [
        "tag",
        "--list",
        tags,
        "--contains",
        commit,
        "--merged",
        published,
        "--sort=v:refname",
    ];
    Some(
        said(vcs, repo, &asked, None)?
            .lines()
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// One remote's refs, as `ls-remote` lists them: each tag by name with the commit it names (an
/// annotated tag peeled to its commit), and the commits its tags and branches name — tags first.
struct RemoteRefs {
    tags: Tags,
    commits: Vec<String>,
}

/// The refs `remote` holds — a remote's name or a URL — asked of that remote itself (`ls-remote`):
/// a tag pushed and one only cut here look alike in this repository. `None` where the remote gave
/// no answer within the budget.
fn refs_on(vcs: &dyn Vcs, repo: &Path, remote: &str) -> Option<RemoteRefs> {
    let said = said(vcs, repo, &["ls-remote", "--", remote], None)?;
    let mut tags = Tags::new();
    let mut peeled = Tags::new();
    let mut heads = Vec::new();
    for line in said.lines() {
        let Some((id, name)) = line.split_once('\t') else {
            continue;
        };
        let (id, name) = (id.trim().to_string(), name.trim());
        match name.strip_prefix("refs/tags/") {
            Some(tag) => match tag.strip_suffix("^{}") {
                Some(tag) => {
                    peeled.insert(tag.to_string(), id);
                }
                None => {
                    tags.insert(tag.to_string(), id);
                }
            },
            None if name == "HEAD" || name.starts_with("refs/heads/") => heads.push(id),
            None => {}
        }
    }
    tags.extend(peeled);
    let mut commits: Vec<String> = tags.values().cloned().collect();
    commits.extend(heads);
    Some(RemoteRefs { tags, commits })
}

/// The tags this repository holds, each peeled to the commit it names.
fn tags_here(vcs: &dyn Vcs, repo: &Path) -> Option<Tags> {
    let asked = [
        "for-each-ref",
        "--format=%(refname:strip=2)%00%(objectname)%00%(*objectname)",
        "refs/tags",
    ];
    Some(
        said(vcs, repo, &asked, None)?
            .lines()
            .filter_map(|line| {
                let mut parts = line.split('\0');
                let (name, object, peeled) = (parts.next()?, parts.next()?, parts.next()?);
                let commit = if peeled.trim().is_empty() {
                    object
                } else {
                    peeled
                };
                Some((name.trim().to_string(), commit.trim().to_string()))
            })
            .collect(),
    )
}

/// How many of a remote's commits one question asks whether this repository has them.
const SHARED_ASKED: usize = 256;

/// Whether `remote` shares this repository's history (t-119569) — then only the same commit is
/// its release. Origin does, and so does a remote at origin's place; a shallow clone takes every
/// remote to, since it lacks the commits outside its depth whatever a remote holds. Any other
/// remote does where this repository has any of the commits its tags and branches name, asked of
/// every one of them, tags first. No answer from git counts as yes. Only a remote that shares
/// none of it — a published snapshot keeps a history of its own — is read by a tag's name.
fn shares_history(vcs: &dyn Vcs, repo: &Path, remote: &str, commits: &[String]) -> bool {
    if remote == ORIGIN || is_shallow(vcs, repo) || same_place(vcs, repo, remote, ORIGIN) {
        return true;
    }
    for asking in commits.chunks(SHARED_ASKED) {
        let mut asked = vec!["rev-list", "--no-walk", "--ignore-missing"];
        asked.extend(asking.iter().map(String::as_str));
        match said(vcs, repo, &asked, None) {
            Some(known) if known.trim().is_empty() => {}
            _ => return true,
        }
    }
    false
}

/// Whether this repository is a shallow clone; no answer from git counts as yes.
fn is_shallow(vcs: &dyn Vcs, repo: &Path) -> bool {
    said(vcs, repo, &["rev-parse", "--is-shallow-repository"], None)
        .is_none_or(|said| said.trim() != "false")
}

/// Whether `remote` and the configured remote `name` are one place: each read as its configured
/// URL where it names a remote, and as given where it is a URL; a trailing `/` or `.git` aside.
fn same_place(vcs: &dyn Vcs, repo: &Path, remote: &str, name: &str) -> bool {
    let place = |of: &str| {
        let url = said(vcs, repo, &["remote", "get-url", "--", of], None)
            .map_or_else(|| of.to_string(), |url| url.trim().to_string());
        url.trim_end_matches('/')
            .trim_end_matches(".git")
            .to_string()
    };
    place(remote) == place(name)
}

/// One head's range against `base_head`, its release aside. A head the base holds is its own range
/// of one, landed as the first commit of the base's first-parent line that holds it. Any other
/// head's range is its commits the base does not hold by ancestry — the merges among them and the
/// commits that change nothing said apart — each landed or not by a copy on the base's first-parent
/// line ([`change_of`]), and landed as the newest copy: the last change to arrive. Either way, the
/// commits of the base after the landing that revert it are said ([`reverts_of`]). `None` where git
/// gave no answer, or the base moved more than [`RANGE_BASE_MAX`] commits since the fork — too long
/// to read on a verb's way.
fn range_of(vcs: &dyn Vcs, repo: &Path, head: &str, base_head: &str) -> Option<RangeSeen> {
    if holds(vcs, repo, base_head, head)? {
        let asked = crate::worktree_landing::trunk_log(head, base_head);
        let said = said(vcs, repo, &asked.each_ref().map(String::as_str), None)?;
        let landed = match crate::worktree_landing::trunk_entry(&said, head)? {
            crate::worktree_landing::TrunkEntry::Head => head.to_string(),
            crate::worktree_landing::TrunkEntry::Commit { sha, .. } => sha.to_string(),
            crate::worktree_landing::TrunkEntry::Target => base_head.to_string(),
        };
        // What the landing brought: where a merge took the head, the merge and the side it merged,
        // up to the head. A head fast-forwarded onto the base's line gives git no place where the
        // work began, so there a revert of any commit the head holds counts ([`reverts_held_by`]).
        let reverted = if landed == head {
            reverts_held_by(vcs, repo, head, &format!("{head}..{base_head}"))?
        } else {
            let mut brought = vec![head.to_string(), landed.clone()];
            let side = logged(vcs, repo, &[format!("{landed}^1..{head}").as_str()])?;
            brought.extend(side.into_iter().map(|one| one.id));
            brought.sort();
            brought.dedup();
            reverts_of(vcs, repo, &brought, &format!("{landed}..{base_head}"))?
        };
        return Some(RangeSeen {
            commits: vec![(head.to_string(), true)],
            landed_as: Some(landed),
            reverted,
            ..RangeSeen::default()
        });
    }
    let ours = logged(vcs, repo, &["--reverse", &format!("{base_head}..{head}")])?;
    // Newest first, and the base's own line alone (ruling §4): a copy only a side branch holds —
    // tied on by `merge -s ours`, say — is no landing.
    let theirs = logged(
        vcs,
        repo,
        &[
            "--first-parent",
            "--no-merges",
            &format!("{head}..{base_head}"),
        ],
    )?;
    if theirs.len() > RANGE_BASE_MAX {
        return None;
    }
    let mut range = RangeSeen::default();
    let mut copies: Vec<usize> = Vec::new();
    for one in &ours {
        if one.merge {
            range.merges.push(one.id.clone());
            continue;
        }
        let change = change_kept(vcs, repo, &one.id)?;
        if change.empty {
            range.empty.push(one.id.clone());
            continue;
        }
        let mut copy = None;
        for (at, held) in theirs.iter().enumerate() {
            // Who wrote it, when and under what subject first, off the one log: a diff is read only
            // for a commit that matches all three.
            if held.wrote == one.wrote && change_kept(vcs, repo, &held.id)?.key == change.key {
                copy = Some(at);
                break;
            }
        }
        copies.extend(copy);
        range.commits.push((one.id.clone(), copy.is_some()));
    }
    if !range.commits.is_empty() && range.commits.iter().all(|(_, landed)| *landed) {
        range.landed_as = copies.iter().min().map(|at| theirs[*at].id.clone());
        let brought: Vec<String> = range
            .commits
            .iter()
            .map(|(commit, _)| commit.clone())
            .chain(copies.iter().map(|at| theirs[*at].id.clone()))
            .collect();
        range.reverted = reverts_of(vcs, repo, &brought, &format!("{head}..{base_head}"))?;
    }
    Some(range)
}

/// How many commits the base may have moved since a reviewed range forked before a landing proof
/// stops reading it: each is a line of one log, and a diff read once and kept by its id
/// ([`change_kept`]) only where it matches a commit of the range by its author, moment and subject;
/// a range that old is the coordinator's to write by hand.
const RANGE_BASE_MAX: usize = 1_000;

/// One commit as [`logged`] lists it.
struct Logged {
    id: String,
    /// Whether it has more than one parent.
    merge: bool,
    /// What [`WROTE`] says of it: its author, author date and subject.
    wrote: String,
}

/// The commits `asked` names — a range, and how to walk it — each with its parents counted and what
/// [`WROTE`] says of it.
fn logged(vcs: &dyn Vcs, repo: &Path, asked: &[&str]) -> Option<Vec<Logged>> {
    let mut args = vec!["log", "--no-color", "--date=raw", LISTED];
    args.extend_from_slice(asked);
    said(vcs, repo, &args, None)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (id, rest) = line.split_once('\0')?;
            let (parents, wrote) = rest.split_once('\0')?;
            Some(Logged {
                id: id.trim().to_string(),
                merge: parents.split_whitespace().count() > 1,
                wrote: wrote.to_string(),
            })
        })
        .collect()
}

/// What a commit's copy keeps when it is picked or rebased onto another branch, and a squash or a
/// rewrite does not: its author, author date and subject — as `git show -s` writes them.
const WROTE: &str = "--format=%an%x00%ae%x00%ad%x00%s";

/// [`WROTE`] after the commit's id and its parents, for a log of many.
const LISTED: &str = "--format=%H%x00%P%x00%an%x00%ae%x00%ad%x00%s";

/// What git's own revert writes in the commit it makes: `This reverts commit <id>.`
const REVERTS: &str = "This reverts commit ";

/// The commits of `range` that revert one of `brought`, each with the one it reverts, read off the
/// words git's own revert writes ([`REVERTS`]). Every parent's line is read: a revert a side branch
/// carried in reverts it all the same.
fn reverts_of(
    vcs: &dyn Vcs,
    repo: &Path,
    brought: &[String],
    range: &str,
) -> Option<Vec<(String, String)>> {
    if brought.is_empty() {
        return Some(Vec::new());
    }
    let greps: Vec<String> = brought
        .iter()
        .map(|one| format!("--grep={REVERTS}{one}"))
        .collect();
    let mut args = vec![
        "log",
        "--no-color",
        "--fixed-strings",
        "--format=%H%x1f%B%x1e",
    ];
    args.extend(greps.iter().map(String::as_str));
    args.push(range);
    let said = said(vcs, repo, &args, None)?;
    let mut found: Vec<(String, String)> = Vec::new();
    for record in said.split('\u{1e}') {
        let Some((id, words)) = record.trim_start().split_once('\u{1f}') else {
            continue;
        };
        for one in brought {
            let pair = (id.trim().to_string(), one.clone());
            if words.contains(&format!("{REVERTS}{one}")) && !found.contains(&pair) {
                found.push(pair);
            }
        }
    }
    Some(found)
}

/// The commits of `range` that revert a commit `head` holds, each with the one it reverts — for a
/// head fast-forwarded onto the base, where git cannot tell which of the commits under it were the
/// work: a revert of any of them counts, an older commit's too (and the coordinator then writes that
/// landing by hand). A commit git does not know, or no answer from git, counts as well: nothing is
/// written on a guess.
fn reverts_held_by(
    vcs: &dyn Vcs,
    repo: &Path,
    head: &str,
    range: &str,
) -> Option<Vec<(String, String)>> {
    let grep = format!("--grep={REVERTS}");
    let asked = [
        "log",
        "--no-color",
        "--fixed-strings",
        grep.as_str(),
        "--format=%H%x1f%B%x1e",
        range,
    ];
    let said = said(vcs, repo, &asked, None)?;
    let mut found: Vec<(String, String)> = Vec::new();
    for record in said.split('\u{1e}') {
        let Some((id, words)) = record.trim_start().split_once('\u{1f}') else {
            continue;
        };
        for named in words.split(REVERTS).skip(1) {
            let reverted: String = named.chars().take_while(char::is_ascii_hexdigit).collect();
            if reverted.len() < 7 || holds(vcs, repo, head, &reverted) == Some(false) {
                continue;
            }
            let pair = (id.trim().to_string(), reverted);
            if !found.contains(&pair) {
                found.push(pair);
            }
        }
    }
    Some(found)
}

/// [`change_of`] kept by commit id for as long as the window runs (t-119569): a commit never
/// changes, so its change is asked of git once — by the 「쌓인 일」 tab's rows and a landing proof
/// alike. A commit git gave no answer for is not kept, and is asked again next time.
fn change_kept(vcs: &dyn Vcs, repo: &Path, commit: &str) -> Option<Change> {
    let cache = || {
        static KEPT: std::sync::OnceLock<
            std::sync::Mutex<std::collections::HashMap<String, Change>>,
        > = std::sync::OnceLock::new();
        KEPT.get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    if let Some(change) = cache().get(commit) {
        return Some(change.clone());
    }
    let change = change_of(vcs, repo, commit)?;
    let mut kept = cache();
    if kept.len() >= CHANGE_KEYS_KEPT {
        kept.clear();
    }
    kept.insert(commit.to_string(), change.clone());
    Some(change)
}

/// [`Change::key`] of `commit`, kept ([`change_kept`]) — what the 「쌓인 일」 tab counts each change
/// once by.
pub(crate) fn change_key_kept(vcs: &dyn Vcs, repo: &Path, commit: &str) -> Option<String> {
    change_kept(vcs, repo, commit).map(|change| change.key)
}

/// How many changes a window keeps before it starts again: every unlanded commit of every
/// checkout it lists and every range it proves, many times over (178 unlanded on 11-20).
const CHANGE_KEYS_KEPT: usize = 8_192;

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

/// What one commit's change is matched by (t-119569, the other provider's check B · C · D): who
/// wrote it, when and under what subject ([`WROTE`]), and its change against its first parent, file
/// by file as a replayed change is digested ([`Context::Dropped`] — line numbers, hunks and the
/// base's own lines aside, every added and removed line exactly), folded into one SHA-256 and cut to
/// [`CHANGE_KEY_CHARS`]. A copy a cherry-pick or a rebase makes keeps all of it, so it keys the same
/// wherever it was picked; a commit that only happens to make the same lines — elsewhere in the
/// file, or none at all — keeps none of who, when and what it said, and keys apart.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Change {
    key: String,
    /// Whether the commit changes nothing (`--allow-empty`): there is nothing of it to land.
    empty: bool,
}

/// [`Change`] of `commit`: `None` where git gave no answer — a root commit, a diff past
/// [`DIFF_MAX`], a git that did not start — and the caller then keys the commit by its own id.
fn change_of(vcs: &dyn Vcs, repo: &Path, commit: &str) -> Option<Change> {
    use sha2::{Digest, Sha256};
    let wrote = said(
        vcs,
        repo,
        &["show", "-s", "--no-color", "--date=raw", WROTE, commit],
        None,
    )?;
    let parent = format!("{commit}^");
    let patch = patch(vcs, repo, &parent, commit, 1, Context::Dropped)?;
    let mut digest = Sha256::new();
    digest.update(wrote.trim_end_matches('\n').as_bytes());
    digest.update(b"\0\0");
    for (file, part) in &patch.files {
        digest.update(file.as_bytes());
        digest.update(b"\0");
        digest.update(part.as_bytes());
        digest.update(b"\n");
    }
    let mut key = format!("{:x}", digest.finalize());
    key.truncate(CHANGE_KEY_CHARS);
    Some(Change {
        key,
        empty: patch.files.is_empty(),
    })
}

/// How many hex digits of a change key go on the wire: 64 bits, against a few hundred changes a window
/// holds at most — a collision there is a chance of about one in 10^14.
const CHANGE_KEY_CHARS: usize = 16;

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

    /// The range a verified review read, as the landing proof asks git (t-119569): a range picked
    /// onto `main` beside a line `main` changed (where git's own patch identity calls the copy
    /// another change) lands once every commit of it is there, as the newest copy; one commit
    /// still missing is no landing; a tag cut before it holds none of it; and a head merged as it
    /// is landed as its merge.
    #[test]
    fn a_reviewed_range_lands_once_every_change_is_in_main_as_its_newest_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("file"), "a\nb\nc\n").expect("write");
        git(repo, &["add", "file"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        git(repo, &["checkout", "-q", "-b", "work"]);
        std::fs::write(repo.join("file"), "a\nB\nc\n").expect("write");
        git(repo, &["commit", "-q", "-am", "first"]);
        let first = git(repo, &["rev-parse", "HEAD"]);
        let head = commit(repo, "second");
        git(repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("file"), "z\na\nb\nc\n").expect("write");
        git(repo, &["commit", "-q", "-am", "elsewhere"]);
        git(repo, &["tag", "v1.0.0"]);
        git(repo, &["cherry-pick", &first]);
        let range = |repo: &Path| {
            let mut seen = CommitsSeen::default();
            see_ranges(&mut seen, repo, &[head.clone()], "main", "v*", None);
            seen.range(&head)
                .cloned()
                .expect("git answered for the range")
        };

        let partial = range(repo);
        assert_eq!(
            partial.commits,
            vec![(first.clone(), true), (head.clone(), false)],
            "the first change is in main beside a line main moved; the second is not"
        );
        assert_eq!((partial.landed_as, partial.tag), (None, None));

        git(repo, &["cherry-pick", &head]);
        let copy = git(repo, &["rev-parse", "HEAD"]);
        let landed = range(repo);
        assert!(
            landed.commits.iter().all(|(_, landed)| *landed),
            "{landed:?}"
        );
        assert_eq!(
            landed.landed_as.as_deref(),
            Some(copy.as_str()),
            "the newest copy"
        );
        assert_eq!(
            landed.tag, None,
            "v1.0.0 was cut before it, and holds none of it"
        );

        // A head merged as it is: its own range of one, landed as the merge that took it.
        git(repo, &["checkout", "-q", "-b", "side"]);
        let side = commit(repo, "side");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--no-ff", "-m", "take side", "side"]);
        let merge = git(repo, &["rev-parse", "HEAD"]);
        let mut seen = CommitsSeen::default();
        see_ranges(&mut seen, repo, &[side.clone()], "main", "v*", None);
        let merged = seen.range(&side).expect("git answered for the merged head");
        assert_eq!(merged.commits, vec![(side.clone(), true)]);
        assert_eq!(merged.landed_as.as_deref(), Some(merge.as_str()));
    }

    /// What git says of the range `head` reviewed, against `main` (t-119569).
    fn reviewed_range(repo: &Path, head: &str) -> RangeSeen {
        let mut seen = CommitsSeen::default();
        see_ranges(&mut seen, repo, &[head.to_string()], "main", "v*", None);
        seen.range(head)
            .cloned()
            .unwrap_or_else(|| panic!("git answered for the range {head}"))
    }

    /// A throwaway repository on `main` with one commit, and `work` forked from it.
    fn forked(dir: &Path) -> &Path {
        git(dir, &["init", "-q", "-b", "main"]);
        commit(dir, "base");
        git(dir, &["checkout", "-q", "-b", "work"]);
        dir
    }

    /// The same lines another commit put in another place of the same file (check B · C) are
    /// another change: the copy a cherry-pick or a rebase makes keeps its author, its moment and its
    /// subject, and a commit that only happens to add the same line elsewhere keeps none of them.
    #[test]
    fn the_same_lines_another_commit_put_elsewhere_in_the_file_are_another_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("file"), "fn a() {\n}\n\nfn b() {\n}\n").expect("write");
        git(repo, &["add", "file"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        git(repo, &["checkout", "-q", "-b", "work"]);
        std::fs::write(
            repo.join("file"),
            "fn a() {\n    return;\n}\n\nfn b() {\n}\n",
        )
        .expect("write");
        git(repo, &["commit", "-q", "-am", "a returns"]);
        let head = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        std::fs::write(
            repo.join("file"),
            "fn a() {\n}\n\nfn b() {\n    return;\n}\n",
        )
        .expect("write");
        git(repo, &["commit", "-q", "-am", "b returns"]);
        let range = reviewed_range(repo, &head);
        assert_eq!(
            range.commits,
            vec![(head.clone(), false)],
            "main added the same line to another function: {range:?}"
        );
        assert_eq!(range.landed_as, None);
    }

    /// Commits that change nothing (check B · D) are nothing to land: said apart from the range's
    /// commits, never matched — not even to another commit that changes nothing — and beside a
    /// change that landed they hold nothing back.
    #[test]
    fn commits_that_change_nothing_are_said_apart_and_hold_nothing_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = forked(dir.path());
        git(repo, &["commit", "-q", "--allow-empty", "-m", "a note"]);
        let note = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        git(
            repo,
            &["commit", "-q", "--allow-empty", "-m", "another note"],
        );
        let only = reviewed_range(repo, &note);
        assert!(only.commits.is_empty(), "{only:?}");
        assert_eq!(only.empty, vec![note.clone()], "{only:?}");
        assert_eq!(only.landed_as, None);

        git(repo, &["checkout", "-q", "work"]);
        let change = commit(repo, "change");
        git(
            repo,
            &["commit", "-q", "--allow-empty", "-m", "a last note"],
        );
        let last = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["cherry-pick", &change]);
        let copy = git(repo, &["rev-parse", "HEAD"]);
        let beside = reviewed_range(repo, &last);
        assert_eq!(beside.commits, vec![(change.clone(), true)], "{beside:?}");
        assert_eq!(beside.empty, vec![note.clone(), last.clone()], "{beside:?}");
        assert_eq!(beside.landed_as.as_deref(), Some(copy.as_str()));
    }

    /// A merge inside the range (check B · E) is said apart: what it changes itself matches no
    /// commit of `main`, so the ledger proves no landing by copies of the rest.
    #[test]
    fn a_merge_inside_a_range_is_said_apart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = forked(dir.path());
        let change = commit(repo, "change");
        git(repo, &["checkout", "-q", "main"]);
        commit(repo, "elsewhere");
        git(repo, &["checkout", "-q", "work"]);
        git(repo, &["merge", "-q", "--no-ff", "-m", "take main", "main"]);
        let merge = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["cherry-pick", &change]);
        let range = reviewed_range(repo, &merge);
        assert_eq!(range.merges, vec![merge.clone()], "{range:?}");
        assert_eq!(range.commits, vec![(change.clone(), true)], "{range:?}");
    }

    /// A landing `main` reverted (check B · F) is said with its revert — a copy picked onto `main`
    /// and reverted, and a head merged as it is and reverted by a revert of its merge.
    #[test]
    fn a_landing_main_reverted_is_said_with_its_revert() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = forked(dir.path());
        let change = commit(repo, "change");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["cherry-pick", &change]);
        let copy = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["revert", "--no-edit", &copy]);
        let revert = git(repo, &["rev-parse", "HEAD"]);
        let picked = reviewed_range(repo, &change);
        assert_eq!(
            picked.landed_as.as_deref(),
            Some(copy.as_str()),
            "{picked:?}"
        );
        assert_eq!(
            picked.reverted,
            vec![(revert.clone(), copy.clone())],
            "{picked:?}"
        );

        git(repo, &["checkout", "-q", "-b", "side"]);
        let side = commit(repo, "side");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--no-ff", "-m", "take side", "side"]);
        let merge = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["revert", "--no-edit", "-m", "1", &merge]);
        let undone = git(repo, &["rev-parse", "HEAD"]);
        let merged = reviewed_range(repo, &side);
        assert_eq!(
            merged.landed_as.as_deref(),
            Some(merge.as_str()),
            "{merged:?}"
        );
        assert_eq!(
            merged.reverted,
            vec![(undone.clone(), merge.clone())],
            "{merged:?}"
        );

        // Fast-forwarded onto main: git cannot tell where the work began on main's line, so a
        // revert of a commit under the head counts as one of the landing.
        git(repo, &["checkout", "-q", "-b", "ahead"]);
        let first = commit(repo, "first");
        let last = commit(repo, "last");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["merge", "-q", "--ff-only", "ahead"]);
        git(repo, &["revert", "--no-edit", &first]);
        let first_undone = git(repo, &["rev-parse", "HEAD"]);
        let forwarded = reviewed_range(repo, &last);
        assert_eq!(
            forwarded.landed_as.as_deref(),
            Some(last.as_str()),
            "{forwarded:?}"
        );
        assert_eq!(
            forwarded.reverted,
            vec![(first_undone.clone(), first.clone())],
            "{forwarded:?}"
        );
    }

    /// Only `main`'s first-parent line proves a copy (check B · B, ruling §4): a copy a side branch
    /// holds, tied on by `merge -s ours` — which kept `main`'s tree as it was — is no landing.
    #[test]
    fn a_copy_only_a_side_branch_of_main_holds_proves_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = forked(dir.path());
        let change = commit(repo, "change");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["checkout", "-q", "-b", "side"]);
        git(repo, &["cherry-pick", &change]);
        git(repo, &["checkout", "-q", "main"]);
        git(
            repo,
            &[
                "merge",
                "-q",
                "-s",
                "ours",
                "-m",
                "keep main as it was",
                "side",
            ],
        );
        let range = reviewed_range(repo, &change);
        assert_eq!(range.commits, vec![(change.clone(), false)], "{range:?}");
        assert_eq!(range.landed_as, None);
    }

    /// A release counts once the remote named — origin here — holds its tag (check B · A): with no
    /// origin there is no published base to read and the range says so; a tag only local is none;
    /// pushed, it is the release.
    #[test]
    fn a_release_tag_counts_once_origin_holds_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = forked(dir.path());
        let change = commit(repo, "change");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["cherry-pick", &change]);
        let copy = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["tag", "v1.0.1"]);

        let alone = released_range(repo, &change, Some("origin"));
        assert_eq!(alone.landed_as.as_deref(), Some(copy.as_str()));
        assert_eq!(alone.tag, None, "no origin, no release: {alone:?}");
        assert!(
            alone
                .unpublished
                .as_deref()
                .is_some_and(|why| why.contains("origin/main")),
            "{alone:?}"
        );

        let origin = tempfile::tempdir().expect("tempdir");
        git(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
        let url = origin.path().to_str().expect("a UTF-8 path");
        git(repo, &["remote", "add", "origin", url]);
        git(repo, &["push", "-q", "origin", "main"]);
        let local = released_range(repo, &change, Some("origin"));
        assert_eq!(local.tag, None, "origin holds main, not the tag: {local:?}");
        assert!(
            local
                .unpublished
                .as_deref()
                .is_some_and(|why| why.contains("v1.0.1")),
            "{local:?}"
        );

        git(repo, &["push", "-q", "origin", "v1.0.1"]);
        let released = released_range(repo, &change, Some("origin"));
        assert_eq!(released.tag.as_deref(), Some("v1.0.1"), "{released:?}");
        assert_eq!(released.unpublished, None, "{released:?}");
    }

    /// What git says of the range `head` reviewed, against `main`, a release read only where
    /// `release` names the remote releases are published to (t-119569).
    fn released_range(repo: &Path, head: &str, release: Option<&str>) -> RangeSeen {
        let mut seen = CommitsSeen::default();
        see_ranges(&mut seen, repo, &[head.to_string()], "main", "v*", release);
        seen.range(head)
            .cloned()
            .unwrap_or_else(|| panic!("git answered for the range {head}"))
    }

    /// A bare repository standing for a remote, and its path as the URL a caller names it by.
    fn bare(dir: &tempfile::TempDir) -> String {
        git(dir.path(), &["init", "-q", "--bare", "-b", "main"]);
        dir.path().to_str().expect("a UTF-8 path").to_string()
    }

    /// A release counts only where the caller names it published (t-119569, the delta check's 2):
    /// the private origin may hold a release's tag before the lane publishes it — the public
    /// snapshot needs it there first — so a tag on origin proves nothing unless origin is the
    /// remote named. With none named no release is read, and the range says so; a remote named
    /// that lacks the tag is no release; one that holds a tag of that name is — even on its own
    /// history, as the public snapshot repository is.
    #[test]
    fn a_release_counts_only_where_the_caller_names_it_published() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = forked(dir.path());
        let change = commit(repo, "change");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["cherry-pick", &change]);
        git(repo, &["tag", "v1.0.1"]);
        let origin = tempfile::tempdir().expect("tempdir");
        let origin_url = bare(&origin);
        git(repo, &["remote", "add", "origin", origin_url.as_str()]);
        git(repo, &["push", "-q", "origin", "main", "v1.0.1"]);

        let unnamed = released_range(repo, &change, None);
        assert_eq!(
            unnamed.tag, None,
            "origin holds the tag, and no remote was named: {unnamed:?}"
        );
        assert!(
            unnamed
                .unpublished
                .as_deref()
                .is_some_and(|why| why.contains("--release-remote")),
            "{unnamed:?}"
        );

        let public = tempfile::tempdir().expect("tempdir");
        let public_url = bare(&public);
        let lacking = released_range(repo, &change, Some(public_url.as_str()));
        assert_eq!(
            lacking.tag, None,
            "the remote named has no such tag: {lacking:?}"
        );
        assert!(
            lacking
                .unpublished
                .as_deref()
                .is_some_and(|why| why.contains("v1.0.1") && why.contains(&public_url)),
            "{lacking:?}"
        );

        // Published there: a tag of that name on the public repository's own history.
        let snapshot = tempfile::tempdir().expect("tempdir");
        git(snapshot.path(), &["init", "-q", "-b", "main"]);
        commit(snapshot.path(), "snapshot");
        git(snapshot.path(), &["tag", "v1.0.1"]);
        git(
            snapshot.path(),
            &["push", "-q", public_url.as_str(), "main", "v1.0.1"],
        );
        let shipped = released_range(repo, &change, Some(public_url.as_str()));
        assert_eq!(shipped.tag.as_deref(), Some("v1.0.1"), "{shipped:?}");
        assert_eq!(shipped.unpublished, None, "{shipped:?}");
        assert_eq!(
            released_range(repo, &change, Some("origin")).tag.as_deref(),
            Some("v1.0.1"),
            "origin, named, is read as any remote is"
        );
    }

    /// A remote that shares this repository's history counts a release only at the same commit
    /// (t-119569): a tag of that name there whose object this repository lacks — an annotated tag
    /// another clone made, or one moved onto a commit not fetched here — is no proof the release
    /// holds the landing, so it is said, and not read as published. Only a remote with a history
    /// of its own, as a published snapshot is, is read by the tag's name.
    #[test]
    fn a_remote_sharing_this_history_counts_a_release_only_at_the_same_commit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = forked(dir.path());
        let change = commit(repo, "change");
        git(repo, &["checkout", "-q", "main"]);
        let before = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["cherry-pick", &change]);
        git(repo, &["tag", "v1.0.1"]);
        let origin = tempfile::tempdir().expect("tempdir");
        let origin_url = bare(&origin);
        git(repo, &["remote", "add", "origin", origin_url.as_str()]);
        git(repo, &["push", "-q", "origin", "main"]);

        // Another clone tags the commit before the landing — annotated, so its tag object is one
        // this repository never fetched.
        let other = tempfile::tempdir().expect("tempdir");
        git(other.path(), &["clone", "-q", origin_url.as_str(), "."]);
        git(
            other.path(),
            &["tag", "-a", "-m", "elsewhere", "v1.0.1", &before],
        );
        git(other.path(), &["push", "-q", "origin", "v1.0.1"]);
        let annotated = released_range(repo, &change, Some("origin"));
        assert_eq!(
            annotated.tag, None,
            "origin's v1.0.1 is not this release: {annotated:?}"
        );
        assert!(
            annotated
                .unpublished
                .as_deref()
                .is_some_and(|why| why.contains("v1.0.1") && why.contains("another commit")),
            "{annotated:?}"
        );

        // Moved onto a commit this repository has not fetched.
        commit(other.path(), "unfetched");
        git(other.path(), &["tag", "-f", "v1.0.1"]);
        git(other.path(), &["push", "-q", "-f", "origin", "v1.0.1"]);
        let unfetched = released_range(repo, &change, Some("origin"));
        assert_eq!(
            unfetched.tag, None,
            "origin's v1.0.1 names another commit: {unfetched:?}"
        );

        // Put back at this release's commit, it is the release.
        git(repo, &["push", "-q", "-f", "origin", "v1.0.1"]);
        assert_eq!(
            released_range(repo, &change, Some("origin")).tag.as_deref(),
            Some("v1.0.1")
        );
    }

    /// Whether a remote shares this history is asked of every commit its tags and branches name,
    /// its tags first (t-119569, the second delta check's 1): a mirror of this history whose main
    /// moved on, with more branches than are asked at once ahead of everything else, still shares
    /// it by the old release it holds — so its v1.0.1 over a commit not fetched here is no release.
    #[test]
    fn a_mirror_of_this_history_shares_it_however_many_branches_come_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = forked(dir.path());
        let change = commit(repo, "change");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["tag", "v1.0.0"]);
        git(repo, &["cherry-pick", &change]);
        git(repo, &["tag", "v1.0.1"]);
        let origin = tempfile::tempdir().expect("tempdir");
        let origin_url = bare(&origin);
        git(repo, &["remote", "add", "origin", origin_url.as_str()]);
        git(repo, &["push", "-q", "origin", "main"]);
        let mirror = tempfile::tempdir().expect("tempdir");
        let mirror_url = bare(&mirror);
        git(repo, &["push", "-q", mirror_url.as_str(), "main", "v1.0.0"]);
        let other = tempfile::tempdir().expect("tempdir");
        git(other.path(), &["clone", "-q", mirror_url.as_str(), "."]);
        let moved = commit(other.path(), "moved");
        for at in 0..70 {
            git(other.path(), &["branch", &format!("b{at:02}"), &moved]);
        }
        git(other.path(), &["tag", "v1.0.1"]);
        git(other.path(), &["push", "-q", "origin", "--all"]);
        git(other.path(), &["push", "-q", "origin", "v1.0.1"]);
        let mirrored = released_range(repo, &change, Some(mirror_url.as_str()));
        assert_eq!(
            mirrored.tag, None,
            "the mirror's v1.0.1 is another commit: {mirrored:?}"
        );
        assert!(
            mirrored
                .unpublished
                .as_deref()
                .is_some_and(|why| why.contains("another commit")),
            "{mirrored:?}"
        );
    }

    /// A shallow clone takes every remote to share its history (t-119569, the second delta check's
    /// 1): the commits outside its depth are ones it lacks whatever the remote holds, so a remote
    /// naming only those is no remote of another history — its v1.0.1 over an old commit is no
    /// release.
    #[test]
    fn a_shallow_clone_takes_every_remote_to_share_its_history() {
        let full = tempfile::tempdir().expect("tempdir");
        git(full.path(), &["init", "-q", "-b", "main"]);
        let old = commit(full.path(), "old");
        commit(full.path(), "recent");
        let source = tempfile::tempdir().expect("tempdir");
        let source_url = bare(&source);
        git(full.path(), &["push", "-q", source_url.as_str(), "main"]);
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let shallow_from = format!("file://{source_url}");
        git(
            repo,
            &["clone", "-q", "--depth", "1", shallow_from.as_str(), "."],
        );
        git(repo, &["checkout", "-q", "-b", "work"]);
        let change = commit(repo, "change");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["cherry-pick", &change]);
        git(repo, &["tag", "v1.0.1"]);
        git(repo, &["push", "-q", "origin", "main"]);
        // Another place holding the same history's old commit, tagged v1.0.1 there.
        let mirror = tempfile::tempdir().expect("tempdir");
        let mirror_url = bare(&mirror);
        git(full.path(), &["tag", "v1.0.1", &old]);
        let main_at_old = format!("{old}:refs/heads/main");
        git(
            full.path(),
            &[
                "push",
                "-q",
                mirror_url.as_str(),
                main_at_old.as_str(),
                "v1.0.1",
            ],
        );
        let cut = released_range(repo, &change, Some(mirror_url.as_str()));
        assert_eq!(
            cut.tag, None,
            "the mirror's v1.0.1 is an old commit: {cut:?}"
        );
        assert!(
            cut.unpublished
                .as_deref()
                .is_some_and(|why| why.contains("another commit")),
            "{cut:?}"
        );
    }

    /// A release remote's URL is never written with the credentials it carries (t-119569): every
    /// reason names it by its host and path alone.
    #[test]
    fn a_release_remote_is_named_without_the_credentials_its_url_carries() {
        for (given, shown) in [
            (
                "https://user:SECRET@example.com/org/repo.git",
                "https://example.com/org/repo.git",
            ),
            (
                "https://SECRET@example.com/repo",
                "https://example.com/repo",
            ),
            (
                "ssh://git@example.com:22/repo.git",
                "ssh://example.com:22/repo.git",
            ),
            ("git@example.com:org/repo.git", "example.com:org/repo.git"),
            ("origin", "origin"),
            ("/srv/git/repo.git", "/srv/git/repo.git"),
        ] {
            assert_eq!(shown_remote(given), shown, "{given}");
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = forked(dir.path());
        let change = commit(repo, "change");
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["cherry-pick", &change]);
        git(repo, &["tag", "v1.0.1"]);
        let origin = tempfile::tempdir().expect("tempdir");
        let origin_url = bare(&origin);
        git(repo, &["remote", "add", "origin", origin_url.as_str()]);
        git(repo, &["push", "-q", "origin", "main"]);
        // Nothing listens there: git cannot ask it, and the reason says so — host and path only.
        let range = released_range(
            repo,
            &change,
            Some("https://user:SECRET-TOKEN@127.0.0.1:9/zerocode.git"),
        );
        let why = range.unpublished.expect("why no release is read");
        assert!(
            why.contains("127.0.0.1:9/zerocode.git") && !why.contains("SECRET"),
            "{why}"
        );
    }

    /// The backfill's old road — commits a result names, with no review — reads a release by the
    /// same rule (t-119569, the delta check's 1): a tag only local, or one only a remote nobody
    /// named holds, is no release; the remote named holding it is.
    #[test]
    fn the_old_road_reads_a_release_only_where_the_caller_names_it_published() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        let released = commit(repo, "one");
        git(repo, &["tag", "v1.0.0"]);
        let origin = tempfile::tempdir().expect("tempdir");
        let origin_url = bare(&origin);
        git(repo, &["remote", "add", "origin", origin_url.as_str()]);
        git(repo, &["push", "-q", "origin", "main"]);
        let public = tempfile::tempdir().expect("tempdir");
        let public_url = bare(&public);
        let read = |release: Option<&str>| {
            see(
                repo,
                std::slice::from_ref(&released),
                Some("main"),
                Some("v*"),
                release,
            )
            .commit(&released)
            .cloned()
            .expect("git answered for the commit")
        };

        let local = read(Some(public_url.as_str()));
        assert_eq!(
            (local.in_base, local.tag.as_deref()),
            (Some(true), None),
            "only local: {local:?}"
        );
        assert!(local.unpublished.is_some(), "{local:?}");
        git(repo, &["push", "-q", "origin", "v1.0.0"]);
        let unnamed = read(None);
        assert_eq!(
            unnamed.tag, None,
            "origin holds it, and no remote was named: {unnamed:?}"
        );
        assert!(unnamed.unpublished.is_some(), "{unnamed:?}");
        git(repo, &["push", "-q", public_url.as_str(), "v1.0.0"]);
        assert_eq!(
            read(Some(public_url.as_str())).tag.as_deref(),
            Some("v1.0.0"),
            "published where named"
        );
    }

    /// MEASUREMENT (ignored, t-119569): the 「쌓인 일」 count of a real repository's checkouts as the
    /// window counts it — summed checkout by checkout, each commit once, and each change once by its
    /// key — against `origin/main`. Read only: `PILE_REPO=<repo> cargo test -p zerocode-shell --
    /// --ignored --nocapture the_piles_count_on_a_real_repository`.
    #[test]
    #[ignore = "reads the repository PILE_REPO names"]
    fn the_piles_count_on_a_real_repository() {
        let Ok(repo) = std::env::var("PILE_REPO") else {
            return;
        };
        let repo = Path::new(&repo);
        let mut summed = 0;
        let mut commits = std::collections::BTreeSet::new();
        let mut changes = std::collections::BTreeSet::new();
        for checkout in git(repo, &["worktree", "list", "--porcelain"])
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
        {
            let head = git(Path::new(checkout), &["rev-parse", "HEAD"]);
            for commit in git(repo, &["cherry", "origin/main", &head])
                .lines()
                .filter_map(|line| line.strip_prefix("+ "))
            {
                summed += 1;
                commits.insert(commit.to_string());
                changes.insert(
                    change_key_kept(&LocalVcs, repo, commit).unwrap_or_else(|| commit.to_string()),
                );
            }
        }
        println!(
            "pile: summed {summed} · each commit once {} · each change once {}",
            commits.len(),
            changes.len()
        );
    }

    /// A commit's change key (t-119569) is the same for the same change picked onto another branch —
    /// where its parent, its line numbers and its id all differ — and differs for any other change, so
    /// the 「쌓인 일」 tab counts the copy a stack's checkout holds once with the commit it was copied
    /// from.
    #[test]
    fn the_same_change_picked_onto_another_branch_keys_the_same_and_another_change_does_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("file"), "a\nb\nc\n").expect("write");
        git(repo, &["add", "file"]);
        git(repo, &["commit", "-q", "-m", "base"]);
        git(repo, &["checkout", "-q", "-b", "work"]);
        std::fs::write(repo.join("file"), "a\nB\nc\n").expect("write");
        git(repo, &["commit", "-q", "-am", "the change"]);
        let original = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        git(repo, &["checkout", "-q", "-b", "stack"]);
        // The stack stands on another base: lines above the change moved, so its line numbers did too.
        std::fs::write(repo.join("file"), "z\na\nb\nc\n").expect("write");
        git(repo, &["commit", "-q", "-am", "elsewhere"]);
        git(repo, &["cherry-pick", &original]);
        let copy = git(repo, &["rev-parse", "HEAD"]);
        let other = commit(repo, "another");
        assert_ne!(original, copy, "a cherry-pick is a commit of its own");

        let key =
            |commit: &str| change_key_kept(&LocalVcs, repo, commit).expect("git keys the change");
        assert_eq!(
            key(&original),
            key(&copy),
            "the same change, picked elsewhere"
        );
        assert_ne!(key(&original), key(&other), "another change");
        assert_eq!(key(&original).len(), CHANGE_KEY_CHARS);
        let root = git(repo, &["rev-list", "--max-parents=0", "HEAD"]);
        assert_eq!(
            change_key_kept(&LocalVcs, repo, &root),
            None,
            "a root commit has no parent to read against"
        );
    }

    /// A throwaway repository: `main` with a tagged commit and an untagged one after it, and a
    /// side branch `main` does not hold. Asked with no release remote named, no tag is read as a
    /// release (t-119569): the commit is said with why.
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
            None,
        );
        let main = git(repo, &["rev-parse", "main"]);
        assert_eq!(
            seen.base,
            Some(BaseSeen {
                name: "main".to_string(),
                head: Some(main),
            })
        );
        let tagged = seen
            .commit(&short)
            .expect("git answered for the short name");
        assert_eq!(
            (
                tagged.full.as_deref(),
                tagged.in_base,
                tagged.tag.as_deref()
            ),
            (Some(released.as_str()), Some(true), None),
            "an abbreviated name resolves to the full commit; no release is read with no remote named"
        );
        assert!(
            tagged.unpublished.is_some(),
            "the commit says why: {tagged:?}"
        );
        let untagged = seen
            .commit(&unreleased)
            .expect("git answered for the commit");
        assert_eq!(
            (
                untagged.full.as_deref(),
                untagged.in_base,
                untagged.tag.as_deref()
            ),
            (Some(unreleased.as_str()), Some(true), None)
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
        let blind = see(
            repo,
            std::slice::from_ref(&released),
            Some("nope"),
            None,
            None,
        );
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

        let seen = see(repo, std::slice::from_ref(&short), Some("main"), None, None);
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
            None,
        );
        assert_eq!(seen.commit(&commit), None, "{seen:?}");
        assert_eq!(seen.base, None, "{seen:?}");
        assert_eq!(
            seen.repo,
            Path::new("/Users/dev/repo").display().to_string()
        );
    }
}
