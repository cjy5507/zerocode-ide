//! Reclaiming the checkout a finished worker leaves behind.
//!
//! # What was missing
//!
//! One road already reclaims a worker's checkout, and it is a narrow one:
//! [`crate::schedule_completed_worker_cleanup`] fires when a `worker_done`
//! arrives saying `ok:true`, on a seat whose summons asked for auto-release,
//! for a checkout THIS PROCESS cut and still remembers cutting
//! (`isolated_worker_terms` is a map in memory). Everything else a worker can
//! do to end leaves its directory standing for good:
//!
//!   * the window restarted, so the map that named the checkout is empty;
//!   * the report said `ok:false`, and a failure is not a reason to keep a
//!     build tree forever;
//!   * nothing was reported at all — the pane died, the coordinator released
//!     or abandoned the seat, the restart sweep settled it;
//!   * the summons did not ask for auto-release.
//!
//! `cargo` puts `target/` inside the worktree, which is the right design —
//! the build dies with the checkout it belongs to — and it is also why the
//! leftovers are measured in gigabytes rather than in kilobytes. Eleven point
//! seven of them on this machine, and an `ENOSPC` that took the ledger
//! runtime down with it.
//!
//! # Where the trigger is
//!
//! On the beat that already exists ([`crate::beat_standing_orders`], once a
//! second on a blocking thread), and not on the settlement road. The
//! settlement road is one of the four ways a worker ends and the other three
//! do not pass through it; a trigger there would close a quarter of the hole
//! and read as if it had closed all of it. The beat sees every state the
//! ledger has ever written, including the ones written by a window that is
//! no longer running.
//!
//! It is not the paint path — the renderer's frames come off the pump's own
//! round, and this rides beside `orchestration::tick` on the blocking task
//! the pump spawns once a second. What a pass costs when there is nothing to
//! do is one ledger read plus one `is_dir` per settled checkout, and nothing
//! else: which candidates are due is answered out of memory, and a pass with
//! nothing due returns before it reads the settings document or the project
//! catalog. Measured on a ledger holding forty released workers in forty
//! checkouts, in a debug build:
//!
//! | | |
//! |---|---|
//! | `RuntimeActor::view` alone | 1.456 ms |
//! | `settled_checkouts` (that read, plus the walk) | 1.551 ms |
//! | forty `is_dir` calls | 0.045 ms |
//! | **one idle pass** | **1.60 ms** |
//! | `ledger_states_by_term`, which the board already reads | 1.465 ms |
//!
//! So a pass is one actor round trip and about a tenth of a millisecond of
//! this file's own work — the same read the board makes to draw itself, once
//! a second, on a thread nothing is painting from.
//!
//! The occupancy question added on top of that is not in the table because
//! an idle pass never asks it: a beat with no settled checkout still standing
//! returns before it. A pass that DOES ask pays three uncontended mutex
//! acquisitions plus one per pane this window has seated — all of it in
//! memory, with no ledger read, no disk and no git behind it.
//!
//! Git is spawned only for a candidate that has never been judged, or whose
//! refusal is [`RE_JUDGE_AFTER`] old or was written before the repository's
//! branches last moved (a few `stat`s, see [`base_mark`]), and never for more than
//! [`JUDGMENTS_PER_PASS`] of them in one pass. One judgment is a handful of
//! git invocations plus the catalog walk in [`owning_repository`], which is
//! the same walk `known_worktree_context` makes for every hand removal. The
//! ceiling that matters is therefore not the beat but the memory: a checkout
//! that was kept costs that once every fifteen minutes, so a window holding a
//! dozen standing orphans spends a few seconds of background git when it
//! boots and next to nothing afterwards.
//!
//! # What it will not do
//!
//! Delete work. Every question below is asked so that its uncertain answer
//! keeps the directory:
//!
//!   * a checkout this window still has a terminal open inside. Asked of
//!     both registries at once ([`crate::occupied_checkouts`]), because the
//!     ledger's `Released` is written BEFORE the pane closes and there is no
//!     bound on the gap. This one is not a `Keep`: it drops the candidate
//!     for the pass instead of being remembered, so the directory is judged
//!     afresh on the first beat after the pane goes rather than fifteen
//!     minutes later — a checkout nobody can ever reclaim is how the disk
//!     fills, and a full disk takes the ledger runtime down with it;
//!   * a checkout the window is standing in;
//!   * one this window cannot prove it made (`Ownership`), which is every
//!     workspace a person cut by hand;
//!   * one git reports locked, or the repository's own main checkout;
//!   * one halfway through a merge, a rebase or a cherry-pick;
//!   * one with any uncommitted or untracked file in it;
//!   * one holding an ignored FILE. `git worktree remove` deletes ignored
//!     paths without a word, and taking `target/` is the whole point — but
//!     `.env` is an ignored path too, and this road has nobody to show a list
//!     to first. A directory is a build tree; anything else is somebody's;
//!   * one on a detached `HEAD`, whose commits have no branch to outlive the
//!     directory;
//!   * one whose branch carries commits neither its base nor the repository's
//!     main line (`origin/main`, `main`) has taken. A base that is not
//!     recorded, or is recorded and cannot be resolved, is no reason by
//!     itself: the main line holding every commit answers where they belong
//!     (t-76403). The branch is never its own proof.
//!
//! The last of those is the one the hand road does not ask.
//! [`zerocode_orchestrator::Orchestrator::remove`] leaves the branch behind,
//! so committed work is never unreachable — but a finished worker's branch
//! that nothing has merged is work somebody still has to go and look at, and
//! the directory is where they would look. `Removal::ConfirmedIfClean` is the
//! only removal spelled here; there is no path through this file that forces.
//!
//! # One judgment, and one road at a time
//!
//! The report road used to remove on git's word alone, and git's "clean"
//! counts neither an ignored path nor a branch nothing has merged. On
//! 2026-09-28 a worker reported `ok:true` with commits its base had not
//! taken, and eighteen seconds later its checkout was gone: the report and
//! the dark and light screenshots under `output/`, and ten gigabytes of
//! `target/` its coordinator had moved in to build with. That road now hands
//! its checkout here ([`judge_reported`]) and gets the judgment the beat
//! gives every other finished worker's — with the repository that cut the
//! checkout standing in for the catalog, because this process made the cut
//! and the catalog need not list a leader's repository at all.
//!
//! And no two roads read one directory at once. The sweep reached that same
//! checkout while the report road was taking it apart, read `git status` off
//! a tree that was half gone, and wrote "was kept: 1334 uncommitted change(s)
//! … D CHANGELOG.md" — a leftover that never existed, and the reading a dead
//! worker's hold is told. Every road that judges or removes a checkout takes
//! [`crate::CheckoutHeld`] first, and a road that finds it held stands aside:
//! the holder is asking the same questions, and its answer stands.
//!
//! What the judgment still takes, knowingly: a directory is a build tree. Git
//! folds an ignored `output/` into one line exactly as it folds `target/`, so
//! a checkout with no commit of its own — a worker that only measured or
//! reported — goes with its `output/` once it is judged, as it always did on
//! the beat. Keeping every ignored directory that no build tool marked as a
//! cache would keep a JavaScript repository's `dist/`, `.next/` and
//! `node_modules/` for good, which is the full disk this file exists to
//! prevent; evidence a person still needs belongs outside the checkout.
//!
//! And a checkout that is kept still gives its build trees back (t-78886):
//! what kept it is the work in it, and a directory a build tool tagged as its
//! cache is no part of that work (`shed_build_trees`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Emitter, Manager};
use zerocode_core::conflict::ConflictOperation;
use zerocode_core::orchestration::Examined;
use zerocode_orchestrator::{Orchestrator, Worktree, same_worktree_path};

use crate::orchestration;
use crate::orchestration::hand_in_keep::{Clearance, Mode, before_cleanup};
use crate::{AppState, CheckoutHeld, ShellStateExt, WorkspaceCreationPrefs};

/// How long a checkout that was kept waits before the question is put to git
/// again. A person commits and merges what a worker left, and the reclaim
/// should follow them without being asked — but not by spending a `git
/// status` per checkout per second while they think about it.
const RE_JUDGE_AFTER: i64 = 15 * 60 * 1_000;

/// How many checkouts one pass will spend git on.
///
/// A window that boots holding a dozen orphans judges them over a dozen
/// beats rather than in one, because the alternative is a burst of thirty
/// git processes on the same blocking pool the beat's own work runs on.
const JUDGMENTS_PER_PASS: usize = 2;

/// What one pass decided about one checkout, so a refusal that has not
/// changed is written down once rather than once a second.
struct Judged {
    reason: ReclaimReason,
    at_ms: i64,
    /// Where the repository's branches stood when the judgment began
    /// (t-76403): a refusal waits for [`RE_JUDGE_AFTER`] unless this has moved
    /// since, because a branch moving is how work lands. `None` for a
    /// refusal nobody stamped, which then waits out the clock like any other.
    base_mark: Option<BaseMark>,
    /// The first look that found nobody standing in a kept checkout, and
    /// whose hand-in it was ([`shed_build_trees`]). Carried across the
    /// refusals written after it, and dropped once somebody stands in it.
    quiet: Option<Quiet>,
}

/// A kept checkout first found with nobody in it.
#[derive(Clone)]
struct Quiet {
    worker: String,
    since_ms: i64,
}

/// The modification times of the files a branch move writes in the
/// repository's shared git directory — see [`base_mark`].
type BaseMark = Vec<Option<std::time::SystemTime>>;

/// Whether a refusal is old enough to be asked again, or the repository's
/// branches moved since it was written (t-76403). `mark` reads the stamp now
/// and is asked only of a refusal that carries one.
fn due_again(held: Option<&Judged>, now_ms: i64, mark: impl FnOnce() -> BaseMark) -> bool {
    let Some(held) = held else {
        return true;
    };
    now_ms.saturating_sub(held.at_ms) >= RE_JUDGE_AFTER
        || held.base_mark.as_ref().is_some_and(|then| *then != mark())
}

/// The git directory a linked checkout's `.git` file names — where git keeps
/// that checkout's own state, its `locked` mark among it. `None` for a
/// repository's primary checkout, whose `.git` is a directory.
fn linked_git_dir(checkout: &Path) -> Option<PathBuf> {
    let said = std::fs::read_to_string(checkout.join(".git")).ok()?;
    let linked = PathBuf::from(said.trim().strip_prefix("gitdir:")?.trim());
    Some(if linked.is_absolute() {
        linked
    } else {
        checkout.join(linked)
    })
}

/// The shared git directory of a checkout: its own `.git` for a repository's
/// primary checkout, and the `commondir` a linked one's git directory names.
fn shared_git_dir(checkout: &Path) -> Option<PathBuf> {
    let dot = checkout.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let linked = linked_git_dir(checkout)?;
    let common = std::fs::read_to_string(linked.join("commondir")).ok()?;
    Some(linked.join(common.trim()))
}

/// Where the repository's top-level branches and the remote's stood, by
/// `stat` alone (t-76403). Git writes a ref as a lock file renamed over it, so
/// a landing — a merge into `main`, a push, a fetch — changes the directory
/// the ref sits in; a worker's own `wt/…` branch lives one directory down and
/// moves only that one. The primary checkout's `HEAD` log and the packed refs
/// cover a repository whose refs were packed. Empty when the directory cannot
/// be found, which then never reads as a move.
fn base_mark(checkout: &Path) -> BaseMark {
    let Some(git_dir) = shared_git_dir(checkout) else {
        return Vec::new();
    };
    [
        "packed-refs",
        "refs/heads",
        "refs/remotes/origin",
        "logs/HEAD",
    ]
    .iter()
    .map(|file| {
        std::fs::metadata(git_dir.join(file))
            .and_then(|meta| meta.modified())
            .ok()
    })
    .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ReclaimKind {
    Active,
    OpenConversation,
    Locked,
    IgnoredFile,
    NotManaged,
    Kept,
    /// Commits the worker made itself are not in the base yet: the checkout
    /// waits for them to land (t-73910).
    Unlanded,
}

/// The reclaimer's observation, shared with the catalogue without another
/// git walk. Transient occupancy never enters the timed refusal cache.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ReclaimReason {
    kind: ReclaimKind,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    checked_ms: Option<i64>,
    /// What an [`ReclaimKind::Unlanded`] checkout waits for: the commits on
    /// its branch only, and the base they have to reach (t-73910). The sidebar
    /// says them, so a row that stays says why and until when.
    #[serde(skip_serializing_if = "Option::is_none")]
    waits: Option<Waiting>,
}

/// The work a kept checkout waits on (t-73910): how many commits on its
/// branch no other branch holds — the worker's own — and the base they have
/// to land in before the checkout goes.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Waiting {
    own: usize,
    base: String,
}

static REASON_REVISION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn reason_revision() -> u64 {
    REASON_REVISION.load(Ordering::Relaxed)
}

fn reason_changed() {
    REASON_REVISION.fetch_add(1, Ordering::Relaxed);
}

fn transient_reasons() -> &'static Mutex<HashMap<String, ReclaimReason>> {
    static REASONS: OnceLock<Mutex<HashMap<String, ReclaimReason>>> = OnceLock::new();
    REASONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn transient_reason(path: &str, reason: Option<ReclaimReason>) {
    let mut held = transient_reasons()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    if held.get(path) == reason.as_ref() {
        return;
    }
    if let Some(reason) = reason {
        held.insert(path.to_string(), reason);
    } else {
        held.remove(path);
    }
    reason_changed();
}

fn replace_transient_reasons(next: HashMap<String, ReclaimReason>) {
    let mut held = transient_reasons()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    if *held != next {
        *held = next;
        reason_changed();
    }
}

fn active_reason() -> ReclaimReason {
    ReclaimReason {
        kind: ReclaimKind::Active,
        detail: "the window is showing this checkout".to_string(),
        checked_ms: None,
        waits: None,
    }
}

fn occupied_reason(count: usize) -> ReclaimReason {
    ReclaimReason {
        kind: ReclaimKind::OpenConversation,
        detail: format!("this window holds {count} terminal(s) in this checkout"),
        checked_ms: None,
        waits: None,
    }
}

pub(crate) fn reason_for(path: &Path) -> Option<ReclaimReason> {
    let transient = transient_reasons()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    if let Some((_, reason)) = transient
        .iter()
        .find(|(at, _)| same_worktree_path(Path::new(at), path))
    {
        return Some(reason.clone());
    }
    drop(transient);
    judged()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .iter()
        .find(|(at, _)| same_worktree_path(Path::new(at), path))
        .map(|(_, held)| ReclaimReason {
            checked_ms: Some(held.at_ms),
            ..held.reason.clone()
        })
}

/// How a refusal reads. A checkout with work in it is KEPT — the directory
/// stays because of what is in it. A repository's own checkout, or a path
/// no catalogued project lists was never
/// a candidate: saying it "was kept" read as a leftover where there was none,
/// once per boot for every worker that ever ran in the main checkout
/// (2026-09-16, forty such lines for two long-gone workers).
#[derive(Clone, Copy)]
enum Standing {
    Kept,
    NotOurs,
}

impl Standing {
    const fn phrase(self) -> &'static str {
        match self {
            Self::Kept => "was kept",
            Self::NotOurs => "is not this window's to reclaim",
        }
    }
}

fn judged() -> &'static Mutex<HashMap<String, Judged>> {
    static JUDGED: OnceLock<Mutex<HashMap<String, Judged>>> = OnceLock::new();
    JUDGED.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The verdict on one checkout.
enum Verdict {
    /// Nothing stands in the way, and this is what made it safe.
    ///
    /// The two names rather than a sentence: a person who finds a directory
    /// gone is owed the reason it went, and that reason has to reach them in
    /// their own language. There is exactly one shape of yes here — the
    /// branch is clean and its base has taken every commit — so the window
    /// composes the sentence and this carries the two facts in it.
    Reclaim {
        branch: String,
        base: String,
        /// The ignored directories the removal takes with it — the build
        /// trees this whole road exists to get back. Named in the window log
        /// so "reclaimed" is an accounting rather than an announcement.
        takes: Vec<String>,
        /// The branch carries no commit of its own: `base` is another branch
        /// holding its head, not the base it was cut from (t-73910). Asked of
        /// every process on the machine before it goes.
        elsewhere: bool,
        /// What made it safe is the repository's default branch holding its
        /// head, with no record of the cut to say where the commits belong
        /// (t-76403). Asked of every process on the machine before it goes,
        /// as `elsewhere` is, though the branch did have commits of its own.
        by_default_branch: bool,
    },
    /// It stays, and this is why. Prose, because the noes are many and each
    /// one is a different fact; they are written to the window log, where the
    /// existing automatic cleanup already writes exactly this news.
    Keep {
        because: String,
        kind: ReclaimKind,
        waits: Option<Waiting>,
    },
}

/// What the window is told when a checkout is reclaimed.
///
/// The path AND why it was safe: a toast that says only "removed" is the
/// silent deletion with a noise on top of it.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Reclaimed {
    pub(crate) path: String,
    /// The last component, for a sentence that has to fit on one line.
    pub(crate) name: String,
    pub(crate) worker: String,
    pub(crate) agent: String,
    /// The branch that survives the directory, and the base that has already
    /// taken every commit on it.
    pub(crate) branch: String,
    pub(crate) base: String,
    /// The task whose hand-in this checkout stood beside (t-45336) — a red, a
    /// verify, an evidence checkout — when that is why it went: `base` is then
    /// the branch and head the worker handed in, and the window says so in
    /// its own sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) beside: Option<String>,
    /// The worker made no commit of its own (t-73910): every commit on
    /// `branch` is on `base`, another branch, and the window says so.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) elsewhere: bool,
}

/// One pass over the checkouts the ledger has finished with.
///
/// Called from the beat. Takes no lock the beat holds and spawns nothing: it
/// runs to the end on the blocking thread it was called on, which is what
/// bounds it — a pass that judged everything at once would be a pass that
/// could outlast the second it rides.
pub(crate) fn sweep(app: &AppHandle, now_ms: i64) {
    // What is still a candidate, in the order `settled_checkouts` fixed. A
    // directory that is already gone is the ordinary steady state after a
    // successful reclaim — the worker row goes on naming its checkout for as
    // long as the row lives — and one `is_dir` is the whole cost of skipping
    // it forever.
    let standing: Vec<orchestration::SettledCheckout> = orchestration::settled_checkouts()
        .into_iter()
        .filter(|one| Path::new(&one.path).is_dir())
        .collect();
    if standing.is_empty() {
        replace_transient_reasons(HashMap::new());
        let mut marks = judged().lock().unwrap_or_else(|held| held.into_inner());
        if !marks.is_empty() {
            marks.clear();
            reason_changed();
        }
        return;
    }

    // What this window is still holding a terminal inside, dropped BEFORE the
    // judgment and before the fifteen-minute memory.
    //
    // The ledger can say `Released` while the pane is alive: `worker-stop`
    // and `worker-release` write the state first and close the terminal
    // after, with no bound on the gap and no closing at all if the effect is
    // refused for a stale incarnation. A new `Released` candidate is due on
    // the very next beat, so that window is exactly where a sweep meets a
    // working agent.
    //
    // A live terminal is NOT a `Verdict::Keep`. A keep is remembered for
    // `RE_JUDGE_AFTER`, and marking one here would hold gigabytes for a
    // quarter of an hour after the pane went — the failure that fills the
    // disk. Dropping the candidate instead means it is judged afresh on the
    // first beat after the terminal closes, with no timer and no new state.
    let state = app.state::<AppState>();
    let active = state.active_root();
    let occupied = crate::occupied_checkouts(&state);
    let mut transient = HashMap::new();
    let standing: Vec<orchestration::SettledCheckout> = standing
        .into_iter()
        .filter(|one| {
            let terminals = crate::occupancy_of(&occupied, Path::new(&one.path));
            let reason = if same_worktree_path(&active, Path::new(&one.path)) {
                Some(active_reason())
            } else if terminals > 0 {
                Some(occupied_reason(terminals))
            } else {
                None
            };
            if let Some(reason) = reason {
                transient.insert(one.path.clone(), reason);
                false
            } else {
                true
            }
        })
        .collect();
    replace_transient_reasons(transient);
    // Keep transient occupancy out of the refusal memory even when it
    // removes the last candidate; moving away makes the next beat eligible.

    // Prune the refusal cache after publishing transient reasons, even when
    // occupancy excluded every candidate. Which candidates are due is still
    // answered from memory alone: a pass with none due returns before it
    // reads settings or the project catalog, let alone starts git.
    let due: Vec<orchestration::SettledCheckout> = {
        let mut marks = judged().lock().unwrap_or_else(|held| held.into_inner());
        // Forget every checkout that stopped being a candidate, so a window
        // running for days holds one entry per standing orphan and not one
        // per worker it has ever settled. A checkout seated again by a
        // returning coordinator leaves this way too, and is judged afresh
        // when it next settles.
        let live: std::collections::HashSet<&str> =
            standing.iter().map(|one| one.path.as_str()).collect();
        let before = marks.len();
        marks.retain(|path, _| live.contains(path.as_str()));
        if marks.len() != before {
            reason_changed();
        }
        standing
            .iter()
            .filter(|one| {
                due_again(marks.get(&one.path), now_ms, || {
                    base_mark(Path::new(&one.path))
                })
            })
            .take(JUDGMENTS_PER_PASS)
            .cloned()
            .collect()
    };
    if due.is_empty() {
        return;
    }

    let state = app.state::<AppState>();
    let road = Road::Sweep {
        projects: crate::stored_projects(state.config_root()),
        prefs: crate::load_settings_for_boot(state.settings())
            .document
            .workspace_creation_prefs,
    };
    let here = Where {
        active: state.active_root(),
        data_root: state.local_data_root().to_path_buf(),
    };
    for candidate in due {
        judge_and_act(app, &candidate, &road, &here, now_ms);
    }
}

/// A worker said `ok:true` on an auto-release seat and its terminal is gone:
/// the report road hands its checkout to the judgment the beat gives every
/// other finished worker's (see *One judgment, and one road at a time*).
///
/// Judged under the row the ledger settled it as, found by path, because the
/// key a refusal is remembered under is that row's: a checkout kept here is
/// then not judged again a second later by the beat. One the ledger does not
/// count finished with — a retry seated in it with `--inherit-checkout`, a
/// ledger that did not answer — is left to the beat, which judges it once it
/// is; nothing is remembered for it, so no clock holds it back.
pub(crate) fn judge_reported(
    app: &AppHandle,
    worker: &str,
    cutter: &Orchestrator,
    path: &Path,
    now_ms: i64,
) {
    let state = app.state::<AppState>();
    let here = Where {
        active: state.active_root(),
        data_root: state.local_data_root().to_path_buf(),
    };
    let Some(candidate) = orchestration::settled_checkouts()
        .into_iter()
        .find(|one| same_worktree_path(Path::new(&one.path), path))
    else {
        crate::note_window_event(
            &here.data_root,
            &format!(
                "orchestration: the checkout worker {worker} left at {} was kept: the ledger \
                 does not count it finished with yet, so the beat judges it once it does",
                path.display()
            ),
        );
        return;
    };
    judge_and_act(
        app,
        &candidate,
        &Road::Report(cutter.clone()),
        &here,
        now_ms,
    );
}

/// How many hand-ins one pass looks beside. A look lists the repository's
/// worktrees and may judge several checkouts, each a handful of git
/// processes, so one hand-in a pass is the same order of work the sweep's
/// [`JUDGMENTS_PER_PASS`] allows.
const HAND_INS_PER_PASS: usize = 1;

/// What the road beside hand-ins remembers of one hand-in it looked at.
struct Looked {
    at_ms: i64,
    /// The checkouts of the task it found standing beside the worker's own on
    /// its first look — what the ledger is told was left at the hand-in.
    found: Vec<String>,
    /// Whether one of them is still standing, so the pass looks again after
    /// [`RE_JUDGE_AFTER`]. A hand-in with nothing left beside it is not looked
    /// at again by this window.
    standing: bool,
}

/// A hand-in, as the road beside hand-ins remembers it: the worker checkout,
/// the task and the head.
type LookedKey = (String, String, String);

/// Every hand-in looked at.
fn looked() -> &'static Mutex<HashMap<LookedKey, Looked>> {
    static LOOKED: OnceLock<Mutex<HashMap<LookedKey, Looked>>> = OnceLock::new();
    LOOKED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn looked_key(one: &orchestration::HandedIn) -> LookedKey {
    (one.checkout.clone(), one.task.clone(), one.head.clone())
}

/// One pass beside the hand-ins the ledger holds (t-45336).
///
/// A worker that handed in a head leaves the checkouts it cut beside its own
/// — a red to prove the test fails, a detached verify, a folder of evidence —
/// and the ledger never named them, so no other road ever judges them: one task
/// came to stand in the sidebar four times, each row saying it had unlanded
/// commits and no agent. This judges every OTHER checkout of the task, in the
/// repository that owns the worker's own, with the handed-in head as the
/// measure (`Holding::HandIn`): one the head holds — every commit in it
/// already in the head, on the branch the worker handed in from — and nobody
/// uses goes, without force; anything else stays, with its reason.
///
/// A hand-in is looked at when it is new to this window and again
/// [`RE_JUDGE_AFTER`] after a look that left something standing. One whose
/// worker checkout is gone is not looked at: there is no branch left to name
/// as the holder, and once the work lands the cleanup review takes what is
/// left. An idle pass costs one ledger read, one `is_dir` per hand-in and a
/// lookup in memory.
pub(crate) fn sweep_handed_in(app: &AppHandle, now_ms: i64) {
    let handed: Vec<orchestration::HandedIn> = orchestration::handed_in_heads()
        .into_iter()
        .filter(|one| Path::new(&one.checkout).is_dir())
        .collect();
    let due: Vec<orchestration::HandedIn> = {
        let mut marks = looked().lock().unwrap_or_else(|held| held.into_inner());
        let live: std::collections::HashSet<LookedKey> = handed.iter().map(looked_key).collect();
        marks.retain(|key, _| live.contains(key));
        handed
            .into_iter()
            .filter(|one| {
                marks.get(&looked_key(one)).is_none_or(|mark| {
                    mark.standing && now_ms.saturating_sub(mark.at_ms) >= RE_JUDGE_AFTER
                })
            })
            .take(HAND_INS_PER_PASS)
            .collect()
    };
    if due.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    let projects = crate::stored_projects(state.config_root());
    let prefs = crate::load_settings_for_boot(state.settings())
        .document
        .workspace_creation_prefs;
    let here = Where {
        active: state.active_root(),
        data_root: state.local_data_root().to_path_buf(),
    };
    for one in due {
        let (found, standing) = look_beside(app, &one, &projects, &prefs, &here, now_ms);
        let mut marks = looked().lock().unwrap_or_else(|held| held.into_inner());
        let found = marks
            .get(&looked_key(&one))
            .map_or(found, |earlier| earlier.found.clone());
        marks.insert(
            looked_key(&one),
            Looked {
                at_ms: now_ms,
                found,
                standing,
            },
        );
    }
}

/// What a look beside one hand-in finds before it judges anything.
enum Besides {
    /// No holder to name — the worker's own checkout is detached, or its branch
    /// no longer holds the head it handed in — so nothing beside it can go on
    /// this road.
    NoHolder,
    /// Git could not list the repository's worktrees; looked at again later.
    Unlisted,
    /// The road, and every other checkout of the task beside the worker's own.
    Found(Road, Vec<Worktree>),
}

/// The road beside one hand-in and the checkouts it is to judge (t-45336), in
/// the repository `owner` that lists the worker's own checkout `own`. The
/// branch `own` is on must hold the handed-in head: that branch is what keeps
/// a detached checkout's commits reachable once it goes, and a worker that
/// moved it elsewhere after handing in leaves a head this road cannot vouch for.
fn besides_of_hand_in(
    owner: Orchestrator,
    own: Worktree,
    one: &orchestration::HandedIn,
) -> Besides {
    let Some(holder) = own.branch.clone() else {
        return Besides::NoHolder;
    };
    if !matches!(
        owner.commits_beyond(&own.path, &format!("refs/heads/{holder}"), &one.head),
        Ok(Some(0))
    ) {
        return Besides::NoHolder;
    }
    let Ok(beside) = beside_of(&owner, &own, &one.task) else {
        return Besides::Unlisted;
    };
    let road = Road::HandedIn(Box::new(Beside {
        owner,
        task: one.task.clone(),
        head: one.head.clone(),
        holder,
        checkout: own.path,
    }));
    Besides::Found(road, beside)
}

/// Every OTHER checkout of `task` the repository lists beside the worker's own
/// (t-45336): not the repository's own checkout, not the worker's, and named
/// for the task by the window's one reading of those names ([`carries_task`]).
/// Locked, dirty or busy ones are listed too — the judgment says why each one
/// stays — because what is listed here is also what the ledger is told was left.
fn beside_of(
    owner: &Orchestrator,
    own: &Worktree,
    task: &str,
) -> Result<Vec<Worktree>, zerocode_orchestrator::OrchestratorError> {
    Ok(owner
        .list()?
        .into_iter()
        .filter(|known| {
            !known.is_main
                && !same_worktree_path(&known.path, &own.path)
                && carries_task(known, task, owner.branch_prefix())
        })
        .collect())
}

/// The checkouts of a task found beside a worker's own when its hand-in was
/// first looked at (t-45336) — what the landing watch tells the ledger was
/// left, by the worker checkout it hands the ledger as a witness. Empty for a
/// checkout no hand-in was looked beside.
pub(crate) fn found_beside(checkout: &str) -> Vec<String> {
    let wanted = checkout.trim_end_matches('/');
    let marks = looked().lock().unwrap_or_else(|held| held.into_inner());
    let mut found: Vec<String> = marks
        .iter()
        .filter(|((at, _, _), _)| at.as_str() == wanted)
        .flat_map(|(_, mark)| mark.found.iter().cloned())
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Look beside one hand-in: every other checkout of its task in the repository
/// that owns the worker's own, judged against the head. Answers what it found
/// there and whether one of them still stands.
fn look_beside(
    app: &AppHandle,
    one: &orchestration::HandedIn,
    projects: &[String],
    prefs: &WorkspaceCreationPrefs,
    here: &Where,
    now_ms: i64,
) -> (Vec<String>, bool) {
    // The worker's own checkout names the repository; unlisted, nothing beside
    // it can be measured.
    let Ok((owner, own)) = owning_repository(projects, prefs, Path::new(&one.checkout)) else {
        return (Vec::new(), false);
    };
    let (road, beside) = match besides_of_hand_in(owner, own, one) {
        Besides::NoHolder => return (Vec::new(), false),
        Besides::Unlisted => return (Vec::new(), true),
        Besides::Found(road, beside) => (road, beside),
    };
    let found: Vec<String> = beside
        .iter()
        .map(|known| known.path.to_string_lossy().into_owned())
        .collect();
    if beside.is_empty() {
        return (found, false);
    }
    let occupied = crate::occupied_checkouts(&app.state::<AppState>());
    let mut standing = false;
    for known in beside {
        let candidate = orchestration::SettledCheckout {
            path: known.path.to_string_lossy().into_owned(),
            worker: one.worker.clone(),
            agent: one.agent.clone(),
            run: one.run.clone(),
        };
        // What the sweep drops before any git: a pane of this window in it, or
        // a worker the ledger still counts summoned seated in it — and a
        // refusal remembered less than `RE_JUDGE_AFTER` ago.
        let recently_kept = judged()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&candidate.path)
            .is_some_and(|held| now_ms.saturating_sub(held.at_ms) < RE_JUDGE_AFTER);
        if crate::occupancy_of(&occupied, &known.path) == 0
            && !orchestration::checkout_is_summoned(&candidate.path)
            && !recently_kept
        {
            judge_and_act(app, &candidate, &road, here, now_ms);
        }
        if known.path.is_dir() {
            standing = true;
        }
    }
    (found, standing)
}

/// The road a judgment came in by, and the one thing each road holds that the
/// other does not: how the repository that owns the checkout is found.
enum Road {
    /// The beat, for every checkout the ledger has settled. The owner is found
    /// through the sidebar's catalog, the only set of repositories this window
    /// may resolve a reported path against, with this person's workspace root
    /// on top: `worker_worktree` cuts under exactly that orchestrator, and one
    /// built without the preferences would judge the checkouts it made to be
    /// somebody else's.
    Sweep {
        projects: Vec<String>,
        prefs: WorkspaceCreationPrefs,
    },
    /// A worker's `ok:true` report: the repository that cut its checkout,
    /// which this process still holds.
    Report(Orchestrator),
    /// A worker handed in a head (t-45336), and this is a checkout it cut
    /// BESIDE its own for the same task — a red, a verify, an evidence
    /// checkout. Judged against the head it handed in rather than against a
    /// recorded base, in the repository that owns the worker's own checkout.
    HandedIn(Box<Beside>),
}

/// What a judgment of a checkout beside a hand-in holds that the other roads
/// do not (t-45336).
struct Beside {
    /// The repository that owns the worker's own checkout, found through the
    /// sidebar's catalog with this person's workspace root on top.
    owner: Orchestrator,
    task: String,
    /// The commit the worker handed in.
    head: String,
    /// The local branch of the worker's own checkout, which holds `head` — the
    /// branch a detached checkout's commits stay reachable from once it goes.
    holder: String,
    /// The worker's own checkout: what it handed in is kept before anything
    /// beside it goes, because the keeping reads the task's checkouts too.
    checkout: PathBuf,
}

/// What a checkout's commits must already be in for it to go.
#[derive(Clone, Copy)]
enum Holding<'a> {
    /// The base its branch was cut from — a finished worker's own checkout.
    Base,
    /// The head a worker handed in, on the branch that holds it — a checkout
    /// beside that worker's own (t-45336).
    HandIn {
        task: &'a str,
        head: &'a str,
        holder: &'a str,
    },
}

impl Road {
    /// The name the window log's removal line carries, so the line says which
    /// door took the directory (`note_worktree_removal`).
    fn source(&self) -> &'static str {
        match self {
            Self::Sweep { .. } => "reclaim",
            Self::Report(_) => "completed-worker",
            Self::HandedIn(_) => "handed-in",
        }
    }

    /// What this road measures a checkout's commits against.
    fn holding(&self) -> Holding<'_> {
        match self {
            Self::Sweep { .. } | Self::Report(_) => Holding::Base,
            Self::HandedIn(beside) => Holding::HandIn {
                task: &beside.task,
                head: &beside.head,
                holder: &beside.holder,
            },
        }
    }

    /// The branch that keeps a detached checkout's commits reachable, on the
    /// one road that may take a detached checkout at all.
    fn holder(&self) -> Option<&str> {
        match self {
            Self::Sweep { .. } | Self::Report(_) => None,
            Self::HandedIn(beside) => Some(&beside.holder),
        }
    }
}

/// The window's own facts one judgment reads, gathered once per road.
struct Where {
    /// The checkout the file surfaces are standing in, which is never a
    /// candidate whatever the ledger says.
    active: PathBuf,
    data_root: PathBuf,
}

/// How many processes on this machine stand inside a folder; `None` is "could
/// not tell".
type ProcessCount<'a> = &'a dyn Fn(&Path) -> Option<usize>;

/// How many bytes the volume holding a folder still has free; `None` is
/// "could not tell".
type FreeBytes<'a> = &'a dyn Fn(&Path) -> Option<u64>;

/// The two questions asked again once git has answered, because every git
/// process is time for the world to move: a coordinator coming back seats its
/// sleeper in the directory, a pane opens in it. Handed in rather than read
/// here, so the one judgment runs where no window is — its tests.
struct Recheck<'a> {
    /// Does the ledger still count the checkout finished with?
    settled: &'a dyn Fn(&str) -> bool,
    /// How many terminals does this window hold inside it?
    terminals: &'a dyn Fn(&Path) -> usize,
    /// Is anything a worker handed in from this checkout still to be kept
    /// (t-32798)? Asked first of all, under the claim, because a checkout is
    /// where the report and the evidence were written and a removal is the end of
    /// them — and asked again by every road that takes a checkout, so the answer
    /// is one.
    keeping: &'a dyn Fn(&Path) -> Clearance,
    /// How many processes on this machine stand inside it — any process, not
    /// only this window's panes. Asked only on the road beside a hand-in
    /// (t-45336): a checkout a worker cut by hand is where a person's shell in
    /// another app, or a build the coordinator runs, may be standing, and this
    /// window's pane registry knows neither. `None` from the question is
    /// "could not tell", which keeps the checkout.
    processes: Option<ProcessCount<'a>>,
    /// The same count, asked before a checkout whose branch carries no
    /// commit of its own goes (t-73910): that removal rests on the worker
    /// being over, and a process standing in the directory says it is not.
    /// `None` keeps every such checkout.
    everyone: Option<ProcessCount<'a>>,
    /// The disk's free bytes, asked before a kept checkout's build goes on
    /// its first quiet look ([`DISK_FLOOR`]). `None` never reads as low.
    free_bytes: Option<FreeBytes<'a>>,
}

/// Put the stamp read before a judgment on the refusal that judgment wrote
/// (t-76403). A refusal from an earlier judgment — a keeping that was only
/// waiting its turn writes none — keeps its own stamp and so stays due.
fn stamp_kept(path: &str, mark: BaseMark, now_ms: i64) {
    if let Some(held) = judged()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .get_mut(path)
        && held.at_ms == now_ms
    {
        held.base_mark = Some(mark);
    }
}

/// What one judgment came to, for the road that asked.
enum Settled {
    /// It went, and these are the two names that made it safe.
    Reclaimed {
        branch: String,
        base: String,
        elsewhere: bool,
    },
    /// It stays, and the window log says why.
    Kept,
    /// Another road holds it, or has already taken it, and that road's answer
    /// stands.
    Elsewhere,
}

/// Judge one checkout and carry out the answer, then tell the window.
fn judge_and_act(
    app: &AppHandle,
    candidate: &orchestration::SettledCheckout,
    road: &Road,
    here: &Where,
    now_ms: i64,
) {
    let state = app.state::<AppState>();
    let terminals = |path: &Path| crate::checkout_occupancy(&state, path);
    // The beat asks without reading a file: a hand-in not yet kept is started on
    // a thread of its own and the checkout waits for the next beat. Beside a
    // hand-in, the worker's own hand-in is asked too: the keeping reads the
    // task's other checkouts, so what it named in this one is kept first.
    let keeping = |path: &Path| match road {
        Road::HandedIn(beside) => {
            before_cleanup(path, Mode::Wait).worse(before_cleanup(&beside.checkout, Mode::Wait))
        }
        Road::Sweep { .. } | Road::Report(_) => before_cleanup(path, Mode::Wait),
    };
    // A finished worker's checkout goes only while the ledger still counts it
    // finished with; one beside a hand-in, which the ledger never named, only
    // while nobody the ledger still counts summoned has been seated in it.
    let settled = |path: &str| match road {
        Road::HandedIn(_) => !orchestration::checkout_is_summoned(path),
        Road::Sweep { .. } | Road::Report(_) => orchestration::checkout_is_settled(path),
    };
    let processes = crate::system_runtime::processes_standing_in;
    let recheck = Recheck {
        settled: &settled,
        terminals: &terminals,
        keeping: &keeping,
        processes: matches!(road, Road::HandedIn(_)).then_some(&processes as ProcessCount<'_>),
        everyone: Some(&processes),
        free_bytes: Some(&orchestration::free_bytes_at),
    };
    // Read before git is asked anything, so a branch that moves while the
    // judgment runs is seen as moved by the next beat.
    let mark = base_mark(Path::new(&candidate.path));
    let settled = settle(candidate, road, here, &recheck, now_ms);
    if matches!(settled, Settled::Kept) {
        stamp_kept(&candidate.path, mark, now_ms);
    }
    match settled {
        Settled::Reclaimed {
            branch,
            base,
            elsewhere,
        } => {
            // The ledger hears a removal it did not ask for when it rests on
            // other branches (t-73910): the run's coordinator reads why the
            // checkout went, in the place it reads every other observation.
            if elsewhere {
                orchestration::post_observation_once(
                    Some(&candidate.run),
                    &format!("run:{}", candidate.run),
                    &format!(
                        "reclaimed {}: worker {} made no commit of its own; every commit on \
                         {branch} is on {base}, and {branch} stays",
                        candidate.path, candidate.worker
                    ),
                    &format!("reclaimed-elsewhere:{}", candidate.path),
                    now_ms,
                );
            }
            // The sidebar's existing refresh, and the sentence for the
            // person. Two events because they answer different questions:
            // one is "the list changed", the other is "something was deleted
            // and here is why".
            let _ = app.emit("worktree:removed", candidate.path.clone());
            let _ = app.emit(
                "worktree:reclaimed",
                Reclaimed {
                    path: candidate.path.clone(),
                    name: Path::new(&candidate.path).file_name().map_or_else(
                        || candidate.path.clone(),
                        |name| name.to_string_lossy().into_owned(),
                    ),
                    worker: candidate.worker.clone(),
                    agent: candidate.agent.clone(),
                    branch,
                    base,
                    beside: match road {
                        Road::HandedIn(beside) => Some(beside.task.clone()),
                        Road::Sweep { .. } | Road::Report(_) => None,
                    },
                    elsewhere,
                },
            );
        }
        // Said by the report road alone: it watched this worker finish, and
        // its silence would read as a removal that never came. The beat
        // simply meets the checkout again on a later pass.
        Settled::Elsewhere if matches!(road, Road::Report(_)) => crate::note_window_event(
            &here.data_root,
            &format!(
                "orchestration: the checkout worker {} left at {} is being judged or removed \
                 on another road; that road's answer stands",
                candidate.worker, candidate.path
            ),
        ),
        Settled::Elsewhere | Settled::Kept => {}
    }
}

/// Every question, then the act: the one judgment both roads take, under the
/// claim that keeps every other road out of the directory until it is done.
fn settle(
    candidate: &orchestration::SettledCheckout,
    road: &Road,
    here: &Where,
    recheck: &Recheck<'_>,
    now_ms: i64,
) -> Settled {
    let path = PathBuf::from(&candidate.path);
    let data_root = here.data_root.as_path();
    if same_worktree_path(&here.active, &path) {
        transient_reason(&candidate.path, Some(active_reason()));
        judged()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .remove(&candidate.path);
        if matches!(road, Road::Report(_)) {
            crate::note_window_event(
                data_root,
                &format!(
                    "orchestration: the checkout worker {} left at {} is active; the next beat rechecks it after the window moves",
                    candidate.worker, candidate.path
                ),
            );
        }
        return Settled::Kept;
    }
    transient_reason(&candidate.path, None);
    // Before git is asked anything: a road taking this directory apart turns
    // every answer below into a reading of a half-deleted tree.
    let Some(held) = CheckoutHeld::take(&path) else {
        return Settled::Elsewhere;
    };
    // Asked again under the claim — the road that held it a moment ago may
    // have been the one removing it.
    if !path.is_dir() {
        return Settled::Elsewhere;
    }
    // What the worker handed in is kept before anything else is asked of this
    // directory (t-32798). A keeping that is only waiting for its turn is not
    // written down and not remembered — the next beat asks again; one that
    // failed is written down once, with its reason, like every other refusal.
    match (recheck.keeping)(&path) {
        Clearance::Clear => {}
        Clearance::Keeping(_) => return Settled::Kept,
        Clearance::Held(because) => {
            remember(candidate, Standing::Kept, &because, data_root, now_ms);
            return Settled::Kept;
        }
    }
    // Beside a hand-in, before git is asked anything: a process standing in
    // the directory is a person's shell, a build, an agent another window runs
    // — somebody's, whatever git would say (t-45336). Remembered like every
    // refusal, so a shell left open there is asked about again after
    // `RE_JUDGE_AFTER` and not once a second.
    if let Some(processes) = recheck.processes {
        let refusal = match processes(&path) {
            Some(0) => None,
            Some(count) => Some(format!("{count} process(es) on this machine stand in it")),
            None => Some("the processes standing in it could not be read".to_string()),
        };
        if let Some(because) = refusal {
            remember(candidate, Standing::Kept, &because, data_root, now_ms);
            return Settled::Kept;
        }
    }
    // A checkout beside a hand-in is not the dead worker's own, so nothing
    // read of it is told to that worker's hold.
    let tells_the_hold = !matches!(road, Road::HandedIn(_));
    let owner = match road {
        Road::Sweep { projects, prefs } => owning_repository(projects, prefs, &path),
        Road::Report(cutter) => listed_by(cutter, &path),
        Road::HandedIn(beside) => listed_by(&beside.owner, &path),
    };
    let (orchestrator, known) = match owner {
        Ok(found) => found,
        Err(refusal) => {
            if tells_the_hold {
                orchestration::checkout_examined(
                    &candidate.worker,
                    Examined::Unknown {
                        why: refusal.clone(),
                    },
                    now_ms,
                );
            }
            remember(candidate, Standing::NotOurs, &refusal, data_root, now_ms);
            return Settled::Kept;
        }
    };
    let (verdict, examined) = look(&orchestrator, &known, road.holding());
    let standing = if matches!(examined, Examined::Shared) {
        Standing::NotOurs
    } else {
        Standing::Kept
    };
    // What the look found reaches the hold the ledger opened at the worker's
    // death, whatever the verdict below does with the directory: the same
    // reading answers both, so the two can never disagree.
    if tells_the_hold {
        orchestration::checkout_examined(&candidate.worker, examined, now_ms);
    }
    let (branch, base, takes, elsewhere, by_default_branch) = match verdict {
        Verdict::Keep {
            because,
            kind,
            waits,
        } => {
            remember_as(
                candidate, standing, &because, kind, waits, data_root, now_ms,
            );
            shed_build_trees(candidate, &orchestrator, kind, recheck, data_root, now_ms);
            return Settled::Kept;
        }
        Verdict::Reclaim {
            branch,
            base,
            takes,
            elsewhere,
            by_default_branch,
        } => (branch, base, takes, elsewhere, by_default_branch),
    };
    // A branch with no commit of its own goes only once nothing stands in the
    // directory at all (t-73910) — remembered like every refusal. So does one
    // whose commits only the default branch vouches for (t-76403): that is an
    // answer about the commits, not about who stands in the directory.
    if elsewhere || by_default_branch {
        let refusal = match recheck.everyone.map(|count| count(&path)) {
            Some(Some(0)) => None,
            Some(Some(count)) => Some(format!("{count} process(es) on this machine stand in it")),
            Some(None) | None => Some("the processes standing in it could not be read".to_string()),
        };
        if let Some(because) = refusal {
            remember(candidate, Standing::Kept, &because, data_root, now_ms);
            return Settled::Kept;
        }
    }
    // Asked again, against the ledger as it stands. Everything above took git
    // processes to answer, and a coordinator coming back in that time seats
    // its sleeper straight into this directory.
    if !(recheck.settled)(&candidate.path) {
        remember(
            candidate,
            Standing::Kept,
            "the ledger seated somebody in it again",
            data_root,
            now_ms,
        );
        return Settled::Kept;
    }
    // And the panes, asked again for the same reason the ledger is: a
    // terminal opened while git was answering is somebody working in this
    // directory.
    //
    // Written straight to the log rather than through `remember`. A mark here
    // would hold the checkout for `RE_JUDGE_AFTER` past the moment the pane
    // closed, and the cheap filter in `sweep` already makes this line rare
    // enough that it will not repeat on a beat.
    let terminals = (recheck.terminals)(&path);
    if terminals > 0 {
        transient_reason(&candidate.path, Some(occupied_reason(terminals)));
        crate::note_window_event(
            data_root,
            &format!(
                "orchestration: the checkout worker {} left at {} was kept: this window \
                 opened {terminals} terminal(s) in it while it was being judged",
                candidate.worker,
                path.display()
            ),
        );
        return Settled::Kept;
    }
    // And every process on this machine, on the road that asks (t-45336), for
    // the reason the panes are asked again: one may have come to stand in it
    // while git was answering. Written to the log, never remembered — the
    // early question above remembers it on the next beat.
    if let Some(processes) = recheck.processes
        && processes(&path) != Some(0)
    {
        crate::note_window_event(
            data_root,
            &format!(
                "orchestration: the checkout beside worker {}'s hand-in at {} was kept: a \
                 process came to stand in it while it was being judged",
                candidate.worker,
                path.display()
            ),
        );
        return Settled::Kept;
    }
    match reclaim(&orchestrator, &held, data_root, road.holder()) {
        Ok(()) => {
            transient_reason(&candidate.path, None);
            if judged()
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .remove(&candidate.path)
                .is_some()
            {
                reason_changed();
            }
            let took = if takes.is_empty() {
                "no ignored path with it".to_string()
            } else {
                takes.join(", ")
            };
            let because = match road {
                Road::HandedIn(beside) => format!(
                    "it stood beside worker {}'s hand-in of {}; {branch} is clean and held by \
                     {base}; it took {took}",
                    candidate.worker, beside.task
                ),
                Road::Sweep { .. } | Road::Report(_) if elsewhere => format!(
                    "worker {} left it with no commit of its own; {branch} is clean and every \
                     commit on it is on {base}; it took {took}",
                    candidate.worker
                ),
                Road::Sweep { .. } | Road::Report(_) => format!(
                    "worker {} left it; {branch} is clean and fully in {base}; it took {took}",
                    candidate.worker
                ),
            };
            crate::note_worktree_removal(data_root, road.source(), &path, &because);
            Settled::Reclaimed {
                branch,
                base,
                elsewhere,
            }
        }
        Err(refusal) => {
            remember(candidate, Standing::Kept, &refusal, data_root, now_ms);
            Settled::Kept
        }
    }
}

/// What a build tool writes into a directory it keeps as a cache — the Cache
/// Directory Tagging Specification, which cargo follows for every `target/`.
const CACHE_DIRECTORY_TAG: &str = "CACHEDIR.TAG";

/// The first bytes of that file, which make it the tag and not a file of the
/// same name.
const CACHE_DIRECTORY_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Whether `tree` holds the cache tag a build tool writes: a regular file
/// (not a link, and not a pipe — opening one waits for a writer that may never
/// come) whose first bytes are the signature. Only that many bytes are read,
/// so a tag file of any size answers at once (review t-79535 P3-8).
fn carries_cache_tag(tree: &Path) -> bool {
    use std::io::Read as _;
    let tag = tree.join(CACHE_DIRECTORY_TAG);
    if !std::fs::symlink_metadata(&tag).is_ok_and(|meta| meta.is_file()) {
        return false;
    }
    let wanted = u64::try_from(CACHE_DIRECTORY_SIGNATURE.len()).unwrap_or(u64::MAX);
    let mut head = Vec::new();
    std::fs::File::open(&tag)
        .and_then(|file| file.take(wanted).read_to_end(&mut head))
        .is_ok()
        && head == CACHE_DIRECTORY_SIGNATURE
}

/// What a build tree set aside to go is renamed to, in the checkout's own git
/// directory — the name and the place a later look finds it by.
const SET_ASIDE: &str = "zerocode-shed-";

/// Free disk under which a kept checkout's build goes on its first quiet look
/// rather than its second: one worktree's budget and half again, where the
/// next summons is close to refused for room and a warm build waiting for a
/// retry is the dearer thing to keep.
const DISK_FLOOR: u64 = zerocode_core::orchestration::WORKTREE_BUDGET_BYTES / 2 * 3;

/// The build trees of a finished worker's checkout that is kept (t-78886,
/// adversarial review O9): the work in it stays for whoever comes to look,
/// and the build goes.
///
/// A kept checkout used to keep everything in it, `target/` with the rest —
/// on 2026-10-28 one of 14 GiB and one of 9 GiB stood in checkouts whose
/// branches waited to land, and the disk fell under its floor four times in
/// 25 minutes. A build tree is the one thing in a checkout nobody needs back:
/// the next build makes it again from the commits that are kept.
///
/// So only what is provably a build tree goes: an ignored directory, a real
/// one and not a link (a shared directory is the repository's own), holding
/// the cache tag a build tool wrote. And only from a checkout this window
/// cut, kept for what is in it rather than for who has it — a locked one is a
/// person's word to leave it be, and one that is not ours is somebody's —
/// while the ledger still counts it finished with and no terminal and no
/// process on this machine stands in it. Every doubt keeps the build.
///
/// Not on the first look that finds it quiet (review r1 P2-3): a task sent
/// back is seated again in the same checkout (`--retry-of
/// --inherit-checkout`) within minutes, and a build gone then is a cold one.
/// It goes on a look [`RE_JUDGE_AFTER`] later that still finds nobody, under
/// the same hand-in — or on the first, once the disk is under
/// [`DISK_FLOOR`].
///
/// And never deleted where it stands (review r1 P2-2): removing 14 GiB takes
/// tens of seconds, and a build started in that time would write into a tree
/// half gone. Each tree is first renamed into the checkout's own git
/// directory — one rename on one volume, invisible to `git status` and to a
/// `git add -A` in the checkout — then everyone is asked again, and only then
/// is what was set aside deleted. A build that came meanwhile starts from an
/// empty place, or finds its tree put back. What could not be deleted is
/// written to the window log and deleted on a later look, by its name.
fn shed_build_trees(
    candidate: &orchestration::SettledCheckout,
    orchestrator: &Orchestrator,
    kind: ReclaimKind,
    recheck: &Recheck<'_>,
    data_root: &Path,
    now_ms: i64,
) {
    if !matches!(
        kind,
        ReclaimKind::Kept | ReclaimKind::IgnoredFile | ReclaimKind::Unlanded
    ) {
        return;
    }
    let path = Path::new(&candidate.path);
    let Some(own_git_dir) = linked_git_dir(path) else {
        return;
    };
    if !nobody_stands_in(candidate, recheck, &own_git_dir) {
        set_quiet(&candidate.path, |_| None);
        return;
    }
    clear_set_aside(candidate, &own_git_dir, data_root);
    let mut since_ms = now_ms;
    set_quiet(&candidate.path, |held| {
        since_ms = held
            .filter(|quiet| quiet.worker == candidate.worker)
            .map_or(now_ms, |quiet| quiet.since_ms);
        Some(Quiet {
            worker: candidate.worker.clone(),
            since_ms,
        })
    });
    let low = recheck
        .free_bytes
        .and_then(|free| free(path))
        .is_some_and(|free| free < DISK_FLOOR);
    if now_ms.saturating_sub(since_ms) < RE_JUDGE_AFTER && !low {
        return;
    }
    let Ok(loss) = orchestrator.pending_loss(path) else {
        return;
    };
    let mut aside: Vec<(String, PathBuf)> = Vec::new();
    for (number, one) in loss.ignored.into_iter().enumerate() {
        // Git names an ignored directory with its slash; the rename back
        // names it without.
        let one = one.trim_end_matches('/').to_string();
        let tree = path.join(&one);
        let tagged = std::fs::symlink_metadata(&tree).is_ok_and(|meta| meta.is_dir())
            && carries_cache_tag(&tree);
        if !tagged {
            continue;
        }
        let parked = own_git_dir.join(format!("{SET_ASIDE}{now_ms}-{number}"));
        match std::fs::rename(&tree, &parked) {
            Ok(()) => aside.push((one, parked)),
            Err(error) => crate::note_window_event(
                data_root,
                &format!(
                    "orchestration: the build tree {one} in the checkout worker {} left at {} \
                     stays: it could not be set aside ({error})",
                    candidate.worker,
                    path.display()
                ),
            ),
        }
    }
    if aside.is_empty() {
        return;
    }
    // Asked again: the git process and the renames were time for a build or
    // a pane to come back, and what it needs is put back where it was.
    if !nobody_stands_in(candidate, recheck, &own_git_dir) {
        set_quiet(&candidate.path, |_| None);
        let put_back: Vec<&str> = aside
            .iter()
            .filter(|(one, parked)| {
                let tree = path.join(one);
                !tree.exists() && std::fs::rename(parked, &tree).is_ok()
            })
            .map(|(one, _)| one.as_str())
            .collect();
        crate::note_window_event(
            data_root,
            &format!(
                "orchestration: somebody came to stand in the checkout worker {} left at {} \
                 while its build was set aside; put back: {}",
                candidate.worker,
                path.display(),
                if put_back.is_empty() {
                    "none".to_string()
                } else {
                    put_back.join(", ")
                }
            ),
        );
        return;
    }
    set_quiet(&candidate.path, |_| None);
    let went = clear_set_aside(candidate, &own_git_dir, data_root);
    let went: Vec<&str> = aside
        .iter()
        .filter(|(_, parked)| went.contains(parked))
        .map(|(one, _)| one.as_str())
        .collect();
    if !went.is_empty() {
        crate::note_window_event(
            data_root,
            &format!(
                "orchestration: the checkout worker {} left at {} stays, and its build \
                 tree(s) went: {}",
                candidate.worker,
                path.display(),
                went.join(", ")
            ),
        );
    }
}

/// The ledger still counts the checkout finished with, git does not report it
/// locked, and no terminal and no process on this machine stands in it.
fn nobody_stands_in(
    candidate: &orchestration::SettledCheckout,
    recheck: &Recheck<'_>,
    own_git_dir: &Path,
) -> bool {
    let path = Path::new(&candidate.path);
    (recheck.settled)(&candidate.path)
        && !own_git_dir.join("locked").exists()
        && (recheck.terminals)(path) == 0
        && recheck.everyone.and_then(|count| count(path)) == Some(0)
}

/// Change the first quiet look remembered for a kept checkout.
fn set_quiet(path: &str, change: impl FnOnce(Option<&Quiet>) -> Option<Quiet>) {
    if let Some(held) = judged()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .get_mut(path)
    {
        held.quiet = change(held.quiet.as_ref());
    }
}

/// Delete every build tree set aside in a checkout's git directory — this
/// look's and any an earlier look could not finish — and say which could
/// not go. The paths that went.
fn clear_set_aside(
    candidate: &orchestration::SettledCheckout,
    own_git_dir: &Path,
    data_root: &Path,
) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(own_git_dir) else {
        return Vec::new();
    };
    let mut went = Vec::new();
    for entry in entries.flatten() {
        let parked = entry.path();
        let ours = entry.file_name().to_string_lossy().starts_with(SET_ASIDE)
            && std::fs::symlink_metadata(&parked).is_ok_and(|meta| meta.is_dir());
        if !ours {
            continue;
        }
        match std::fs::remove_dir_all(&parked) {
            Ok(()) => went.push(parked),
            Err(error) => crate::note_window_event(
                data_root,
                &format!(
                    "orchestration: a build tree set aside from the checkout worker {} left at \
                     {} could not be deleted ({error}); a later look tries again: {}",
                    candidate.worker,
                    candidate.path,
                    parked.display()
                ),
            ),
        }
    }
    went
}

/// Write a refusal down — once per refusal, not once per beat.
///
/// The window's own log is where the existing automatic cleanup already puts
/// exactly this news ("worker N checkout retained at P"), and a kept checkout
/// is not an event a person has to be interrupted for: the directory is still
/// there, with its work in it, which is the outcome they wanted. What they
/// are owed is a line saying why it is still there when its worker is gone —
/// worded by its [`Standing`], so a checkout that was never a candidate does
/// not read as a leftover.
fn remember(
    candidate: &orchestration::SettledCheckout,
    standing: Standing,
    because: &str,
    data_root: &Path,
    now_ms: i64,
) {
    let kind = if matches!(standing, Standing::NotOurs) {
        ReclaimKind::NotManaged
    } else {
        ReclaimKind::Kept
    };
    remember_as(candidate, standing, because, kind, None, data_root, now_ms);
}

fn remember_as(
    candidate: &orchestration::SettledCheckout,
    standing: Standing,
    because: &str,
    kind: ReclaimKind,
    waits: Option<Waiting>,
    data_root: &Path,
    now_ms: i64,
) {
    let reason = ReclaimReason {
        kind,
        detail: because.to_string(),
        checked_ms: None,
        waits,
    };
    let repeat = {
        let mut marks = judged().lock().unwrap_or_else(|held| held.into_inner());
        let held = marks.get(&candidate.path);
        let repeat = held.is_some_and(|held| held.reason == reason);
        let quiet = held.and_then(|held| held.quiet.clone());
        marks.insert(
            candidate.path.clone(),
            Judged {
                reason,
                at_ms: now_ms,
                base_mark: None,
                quiet,
            },
        );
        repeat
    };
    if repeat {
        return;
    }
    reason_changed();
    crate::note_window_event(
        data_root,
        &format!(
            "orchestration: the checkout worker {} left at {} {}: {because}",
            candidate.worker,
            candidate.path,
            standing.phrase()
        ),
    );
}

/// The repository that owns this checkout, opened the way the window opens it
/// everywhere else.
///
/// NOT `Orchestrator::open(<the checkout>)`, which is the obvious thing and
/// the wrong one. Inside a linked worktree `rev-parse --show-toplevel`
/// answers the WORKTREE, so an orchestrator opened there takes the checkout
/// itself as its repository root — and then the worktree root it derives is a
/// different directory (it is hashed off that root), the ownership rule reads
/// every managed checkout as `external`, and `git worktree remove` would run
/// with its working directory inside the tree it is removing.
///
/// So the owner is found the way [`crate::known_worktree_context`] finds it:
/// by asking each repository in the sidebar's catalog for its own worktree
/// list. That also settles the authority question — a path reported by a
/// worker is not permission to touch a directory, and a checkout no catalogued
/// repository lists is not this window's to remove.
///
/// The person's workspace-creation preferences go on top, because the
/// worktree root the ownership rule compares against comes from them:
/// `worker_worktree` cuts under exactly this orchestrator, and one built
/// without the preferences would judge the checkouts it made to be somebody
/// else's.
fn owning_repository(
    projects: &[String],
    prefs: &WorkspaceCreationPrefs,
    path: &Path,
) -> Result<(Orchestrator, Worktree), String> {
    for stored in projects {
        let Ok(orchestrator) = Orchestrator::open(stored) else {
            continue;
        };
        let Ok(listed) = orchestrator.list() else {
            continue;
        };
        let Some(known) = listed
            .into_iter()
            .find(|candidate| same_worktree_path(&candidate.path, path))
        else {
            continue;
        };
        let orchestrator = crate::apply_workspace_creation_prefs(orchestrator, prefs)?;
        return Ok((orchestrator, known));
    }
    Err("no project in this window's catalog lists it as a worktree".to_string())
}

/// The report road's owner: the repository that cut the checkout, asked for
/// its own entry. No catalog walk — this process made the cut, with the
/// person's workspace root already on it — but the same worktree list the
/// catalog road reads, so a path git no longer lists is refused, not trusted.
fn listed_by(cutter: &Orchestrator, path: &Path) -> Result<(Orchestrator, Worktree), String> {
    let listed = cutter.list().map_err(|error| {
        format!("the repository that cut it could not list its worktrees: {error}")
    })?;
    let known = listed
        .into_iter()
        .find(|candidate| same_worktree_path(&candidate.path, path))
        .ok_or_else(|| "the repository that cut it no longer lists it as a worktree".to_string())?;
    Ok((cutter.clone(), known))
}

/// Every question, cheapest first, each one answered so that its uncertain
/// answer keeps the directory.
/// A refusal whose reason the ledger's hold carries as "could not be
/// examined": the verdict and the fact are one sentence.
fn unknown(because: String) -> (Verdict, Examined) {
    unknown_as(because, ReclaimKind::Kept)
}

fn unknown_as(because: String, kind: ReclaimKind) -> (Verdict, Examined) {
    (
        Verdict::Keep {
            because: because.clone(),
            kind,
            waits: None,
        },
        Examined::Unknown { why: because },
    )
}

/// A refusal because the checkout is the repository's own or was not cut
/// by this window: whatever a worker left there stays with its owner.
fn shared(because: String) -> (Verdict, Examined) {
    (
        Verdict::Keep {
            because,
            kind: ReclaimKind::NotManaged,
            waits: None,
        },
        Examined::Shared,
    )
}

/// The verdict alone, for the tests that only ever asked for it — the beat
/// itself reads both halves through `look`.
#[cfg(test)]
fn judge(orchestrator: &Orchestrator, known: &Worktree) -> Verdict {
    look(orchestrator, known, Holding::Base).0
}

/// One reading of a settled checkout, answering two questions from it: what
/// the reclaimer may do with the directory, and what a dead worker left in
/// it. Kept as one function on purpose — a second reading for the second
/// question would be a second chance to disagree.
fn look(
    orchestrator: &Orchestrator,
    known: &Worktree,
    holding: Holding<'_>,
) -> (Verdict, Examined) {
    let path = known.path.as_path();
    if known.is_main {
        return shared("it is the repository's own checkout".to_string());
    }
    if known.locked {
        return unknown_as(
            "git reports that the worktree is locked".to_string(),
            ReclaimKind::Locked,
        );
    }
    // The same ownership rule the hand-rolled automatic cleanups ask, and for
    // the same reason: a reported path is not authority over a directory, and
    // a workspace somebody made themselves is theirs whatever the ledger
    // seated in it.
    let ownership = crate::worktree_ownership(orchestrator, known);
    if ownership != zerocode_core::Ownership::ZerocodeManaged {
        return shared(format!("its ownership is `{}`", ownership.slug()));
    }
    match orchestrator.conflict_operation(path) {
        Ok(ConflictOperation::Unknown) => {}
        Ok(halfway) => {
            return unknown(format!("a {} is in progress in it", halfway.id()));
        }
        Err(error) => {
            return unknown(format!(
                "git could not say what it is in the middle of: {error}"
            ));
        }
    }
    let takes = match orchestrator.pending_loss(path) {
        Ok(loss) => {
            let changes = loss.uncommitted_display();
            if !changes.is_empty() {
                return (
                    Verdict::Keep {
                        because: format!(
                            "{} uncommitted change(s) live only here: {}",
                            changes.len(),
                            changes.join(", ")
                        ),
                        kind: ReclaimKind::Kept,
                        waits: None,
                    },
                    Examined::Uncommitted {
                        changes: changes.len(),
                    },
                );
            }
            /* And the ignored paths, which git deletes with the worktree
             * whatever anybody thinks about them. Taking `target/` is the
             * POINT — that is where the gigabytes are, and it is why cargo
             * building inside the checkout is the right design — but an
             * ignored path is also where a `.env` lives, and this road has
             * nobody to show a list to before it acts.
             *
             * So the answer is asked of the disk: a directory is a build
             * tree, and anything else is a file that exists nowhere but
             * here. One of those is enough to keep the checkout. It costs a
             * person whose global excludes drop a stray file into every
             * worktree a reclaim they would have allowed — and the refusal
             * says which file, so that is a thing they can answer. The other
             * way round costs them the file.
             *
             * Read from disk rather than from the record because the record
             * cannot say: `parse_status` sheds the trailing separator git
             * uses to mark a directory, on purpose — these strings go back
             * to git as pathspecs. */
            let (dirs, files): (Vec<String>, Vec<String>) = loss
                .ignored
                .into_iter()
                .partition(|one| path.join(one).is_dir());
            if !files.is_empty() {
                return unknown_as(
                    format!(
                        "{} ignored file(s) exist only here and would go with it: {}",
                        files.len(),
                        files.join(", ")
                    ),
                    ReclaimKind::IgnoredFile,
                );
            }
            dirs
        }
        Err(error) => return unknown(format!("git could not read its status: {error}")),
    };
    // Beside a hand-in the last questions are the hand-in's own (t-45336): the
    // same task, and every commit already in the head the worker handed in.
    if let Holding::HandIn { task, head, holder } = holding {
        return held_by_hand_in(orchestrator, known, task, head, holder, takes);
    }
    let Some(branch) = known.branch.as_deref() else {
        return unknown(
            "its HEAD is detached, so removal would leave no branch holding its commits"
                .to_string(),
        );
    };
    let recorded = orchestrator.creation_bases().get(branch).cloned();
    let counted = recorded
        .as_deref()
        .map(|base| orchestrator.commits_beyond(path, base, branch));
    // The repository's main line holding every commit is an answer to where
    // they belong that no record of the cut is needed for (t-76403). A worker
    // that moved to a branch of its own naming has no recorded base, and two
    // such heads landed in `main` and stood kept for good. Asked whenever the
    // recorded base does not say "all taken": missing, gone, or behind.
    if !matches!(counted, Some(Ok(Some(0))))
        && let Some(main_line) = default_branch_holding(orchestrator, path, branch)
    {
        return (
            Verdict::Reclaim {
                branch: branch.to_string(),
                base: main_line,
                takes,
                elsewhere: false,
                by_default_branch: true,
            },
            Examined::Landed,
        );
    }
    let (Some(base), Some(counted)) = (recorded, counted) else {
        return unknown(format!(
            "nothing recorded what {branch} was cut from, so where its commits belong is unknown"
        ));
    };
    match counted {
        Ok(Some(0)) => (
            Verdict::Reclaim {
                branch: branch.to_string(),
                base,
                takes,
                elsewhere: false,
                by_default_branch: false,
            },
            Examined::Landed,
        ),
        Ok(Some(outstanding)) => unlanded(orchestrator, path, branch, base, outstanding, takes),
        Ok(None) => unknown(format!(
            "{base} cannot be resolved here, so whether {branch} has landed is unknown"
        )),
        Err(error) => unknown(format!(
            "git could not compare {branch} with {base}: {error}"
        )),
    }
}

/// The repository's main line, when it already holds every commit on
/// `branch` (t-76403): `origin/main` or `main` as the remote's own word names
/// them, else the first default-looking name that resolves, with its
/// `origin/` twin.
///
/// Every doubt answers `None` and keeps the checkout: a name that does not
/// resolve, a count git could not read, git failing. The branch itself is
/// never its own proof — a worker's branch called `develop` is "in" `develop`
/// by being it, nor is the `origin/` copy of what it pushed. And what makes a yes safe is the same as for a recorded base:
/// the commits are reachable from a branch that is not this one, and the
/// removal leaves the branch standing.
fn default_branch_holding(
    orchestrator: &Orchestrator,
    path: &Path,
    branch: &str,
) -> Option<String> {
    orchestrator
        .default_branches(path, &crate::worktree_runtime::DEFAULT_BASE_NAMES)
        .ok()?
        .into_iter()
        .filter(|name| name != branch && name.strip_prefix("origin/") != Some(branch))
        .find(|name| matches!(orchestrator.commits_beyond(path, name, branch), Ok(Some(0))))
}

/// A clean checkout whose branch carries `outstanding` commits its base has
/// not taken (t-73910). They are the worker's own work, and the checkout
/// waits for them to land — unless no commit on the branch is its own: a
/// branch cut on another worker's unlanded branch carries that branch's
/// commits, and when another local branch holds its head, the removal takes
/// nothing from anybody. On 2026-10-25 two released workers' checkouts stood
/// as 「미반영 19 · 정리 보류」 with not one commit of their own. A count that
/// cannot be read keeps the checkout, waiting on every commit.
fn unlanded(
    orchestrator: &Orchestrator,
    path: &Path,
    branch: &str,
    base: String,
    outstanding: usize,
    takes: Vec<String>,
) -> (Verdict, Examined) {
    let held = orchestrator
        .commits_held_elsewhere(path, &base, branch)
        .ok()
        .flatten();
    if let Some(zerocode_orchestrator::HeldElsewhere {
        own: 0,
        holder: Some(holder),
    }) = held
    {
        return (
            Verdict::Reclaim {
                branch: branch.to_string(),
                base: holder,
                takes,
                elsewhere: true,
                by_default_branch: false,
            },
            Examined::Landed,
        );
    }
    let own = held.map_or(outstanding, |held| held.own);
    (
        Verdict::Keep {
            because: format!(
                "{outstanding} commit(s) on {branch} are not in {base} yet, {own} of them on no \
                 other branch"
            ),
            kind: ReclaimKind::Unlanded,
            waits: Some(Waiting {
                own,
                base: base.clone(),
            }),
        },
        Examined::Unlanded {
            branch: branch.to_string(),
            base,
            commits: outstanding,
        },
    )
}

/// The last questions for a checkout beside a hand-in (t-45336): is it the
/// same task's by the names this window gives its checkouts, and does the head
/// the worker handed in already hold every commit in it. Asked after the
/// questions every checkout answers — standing, ownership, halfway operations,
/// uncommitted changes, ignored files — so what reaches here is clean.
///
/// `holder` is the branch of the worker's own checkout; `base` in the verdict
/// names it with the head, the two facts that make the removal safe: the
/// commits stay reachable from that branch once the directory is gone.
fn held_by_hand_in(
    orchestrator: &Orchestrator,
    known: &Worktree,
    task: &str,
    head: &str,
    holder: &str,
    takes: Vec<String>,
) -> (Verdict, Examined) {
    if !carries_task(known, task, orchestrator.branch_prefix()) {
        return shared(format!("its name does not carry {task}"));
    }
    let Some(at) = known.head.as_deref() else {
        return unknown("git did not say which commit it is on".to_string());
    };
    match orchestrator.commits_beyond(&known.path, head, at) {
        Ok(Some(0)) => (
            Verdict::Reclaim {
                branch: known
                    .branch
                    .clone()
                    .unwrap_or_else(|| format!("the detached HEAD {}", short_commit(at))),
                base: format!("{holder} at {}", short_commit(head)),
                takes,
                elsewhere: false,
                by_default_branch: false,
            },
            Examined::Landed,
        ),
        Ok(Some(outstanding)) => unknown(format!(
            "{outstanding} commit(s) in it are not in the head {task} handed in"
        )),
        Ok(None) => unknown(format!(
            "{head} cannot be resolved here, so whether it holds this checkout is unknown"
        )),
        Err(error) => unknown(format!("git could not compare it with {head}: {error}")),
    }
}

/// Whether a checkout is one of `task`'s by the names this window gives them
/// (t-45336): read by the one reader of those names,
/// [`zerocode_orchestrator::naming::checkout_task_id`] — the sidebar folds a
/// task's checkouts under one row by the same answer.
fn carries_task(known: &Worktree, task: &str, prefix: &str) -> bool {
    known
        .path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .and_then(|folder| {
            zerocode_orchestrator::naming::checkout_task_id(folder, known.branch.as_deref(), prefix)
        })
        == Some(task)
}

/// A commit as a sentence carries it: the first nine digits — what
/// `git log --oneline` prints for a repository with this many objects, so the
/// line can be pasted back into git.
fn short_commit(commit: &str) -> &str {
    commit.get(..9).unwrap_or(commit)
}

/// Take the checkout away, through the gate every automatic removal goes
/// through.
///
/// The shared links come off first, for the reason the hand road takes them
/// off first: a symlink into the primary's `node_modules` is untracked as far
/// as git is concerned, so leaving it makes an ordinary delete fail with "use
/// force" — and there is no force here, so it would simply mean this window
/// could never reclaim a checkout in a repository that shares directories,
/// which is this one.
///
/// The project's archive script is NOT run, matching the existing automatic
/// road: a teardown somebody wrote is a program, and running one unattended
/// on a beat is a decision of its own rather than a detail of this one.
fn reclaim(
    orchestrator: &Orchestrator,
    held: &CheckoutHeld,
    data_root: &Path,
    holder: Option<&str>,
) -> Result<(), String> {
    let path = held.path();
    crate::unshare_project_directories(orchestrator.repo_root(), path);
    match holder {
        // Beside a hand-in (t-45336): a detached checkout goes only while the
        // branch that holds the handed-in head holds its commit too.
        Some(holder) => crate::remove_automatic_held_by(orchestrator, held, holder)?,
        None => crate::remove_automatic_worktree(orchestrator, held)?,
    }
    // After the removal and not before, exactly as the hand road orders it:
    // history is the one thing somebody who lands back on a refused removal
    // still wants.
    crate::forget_worktree_history(data_root, &path.to_string_lossy());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let output = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// A repository with one commit on `main`, and the worktree root this
    /// window's ownership rule recognises as its own.
    struct Bench {
        _temp: tempfile::TempDir,
        repo: PathBuf,
        worktrees: PathBuf,
    }

    impl Bench {
        fn open() -> Self {
            let temp = tempfile::tempdir().expect("a reclaim repository");
            // Git writes down a checkout's real path (macOS's temporary folder
            // is a link into /private) and the ownership rule reads the root as
            // text, so the bench's root is the real one, as a window's is.
            // Use a compatible spelling too: git cannot create a worktree
            // from the verbatim disk path std returns on Windows.
            let real = dunce::canonicalize(temp.path()).expect("the real temporary path");
            let repo = real.join("repo");
            std::fs::create_dir_all(&repo).expect("repo");
            git(&repo, &["init", "-q", "-b", "main"]);
            git(&repo, &["config", "user.name", "ZeroCode Test"]);
            git(&repo, &["config", "user.email", "test@zerocode"]);
            git(&repo, &["commit", "--allow-empty", "-q", "-m", "first"]);
            let worktrees = real.join("managed");
            Self {
                _temp: temp,
                repo,
                worktrees,
            }
        }

        fn orchestrator(&self) -> Orchestrator {
            Orchestrator::open(&self.repo)
                .expect("open repository")
                .with_worktree_root(self.worktrees.clone())
        }

        /// The catalog and the preferences a window with this bench in it
        /// would hand the sweep — the pair `owning_repository` resolves with.
        fn window(&self) -> (Vec<String>, WorkspaceCreationPrefs) {
            (
                vec![self.repo.to_string_lossy().into_owned()],
                WorkspaceCreationPrefs {
                    directory: self.worktrees.to_string_lossy().into_owned(),
                    nest_workspaces: false,
                    history: Vec::new(),
                },
            )
        }

        /// The entry git's own worktree list holds for this path — what
        /// `owning_repository` hands the judgment.
        fn listed(&self, path: &Path) -> Worktree {
            self.orchestrator()
                .list()
                .expect("worktree list")
                .into_iter()
                .find(|candidate| same_worktree_path(&candidate.path, path))
                .unwrap_or_else(|| panic!("git does not list {}", path.display()))
        }

        /// A checkout cut the way a worker's is: under the managed root, on a
        /// prefixed branch, with the base it was cut from written down.
        fn cut(&self, name: &str) -> PathBuf {
            let path = self.worktrees.join(name);
            let branch = format!(
                "wt/{}",
                zerocode_orchestrator::naming::branch_component_from_checkout(name)
            );
            git(
                &self.repo,
                &[
                    "worktree",
                    "add",
                    "--no-track",
                    "-b",
                    &branch,
                    &path.to_string_lossy(),
                    "main",
                ],
            );
            git(
                &path,
                &[
                    "config",
                    "--local",
                    &format!("branch.{branch}.base"),
                    "main",
                ],
            );
            path
        }

        /// Ignore rules committed on `main`, so every checkout cut afterwards
        /// carries them the way a real repository's `.gitignore` does.
        fn ignore_on_main(&self, rules: &str) {
            std::fs::write(self.repo.join(".gitignore"), rules).expect("ignore rules");
            git(&self.repo, &["add", ".gitignore"]);
            git(&self.repo, &["commit", "-q", "-m", "ignore rules"]);
        }

        /// A cargo build tree at `at`: the cache tag cargo writes into every
        /// target directory, and something of size under it.
        fn build_tree(at: &Path) {
            std::fs::create_dir_all(at.join("debug")).expect("build tree");
            std::fs::write(at.join("CACHEDIR.TAG"), CACHE_TAG).expect("cache tag");
            std::fs::write(at.join("debug/big"), vec![0u8; 4096]).expect("build output");
        }

        /// A warmed build tree made elsewhere and moved into `checkout` — a
        /// rename on one filesystem, which is what a coordinator's `mv` is.
        fn move_in_warm_target(&self, checkout: &Path) {
            let warm = self.repo.with_file_name("warm-target");
            Self::build_tree(&warm);
            std::fs::rename(&warm, checkout.join("target")).expect("move the warm build in");
        }
    }

    /// The first line of the tag cargo writes into every target directory (the
    /// Cache Directory Tagging Specification) — what makes a fixture's
    /// `target/` the shape a real build leaves.
    const CACHE_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

    /// A PNG's eight signature bytes: a screenshot as far as anything here
    /// looks.
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

    /// The ledger's row for a finished worker's checkout, the shape
    /// `settled_checkouts` hands either road.
    fn finished(path: &Path, worker: &str) -> orchestration::SettledCheckout {
        orchestration::SettledCheckout {
            path: path.to_string_lossy().into_owned(),
            worker: worker.to_string(),
            agent: "claude".to_string(),
            run: "run-1".to_string(),
        }
    }

    /// The window's two last questions, answered the way a finished worker's
    /// checkout answers them: the ledger is done with it and no pane is in it.
    fn nobody_left() -> Recheck<'static> {
        fn settled(_: &str) -> bool {
            true
        }
        fn terminals(_: &Path) -> usize {
            0
        }
        fn kept(_: &Path) -> Clearance {
            Clearance::Clear
        }
        Recheck {
            settled: &settled,
            terminals: &terminals,
            keeping: &kept,
            processes: None,
            everyone: None,
            free_bytes: None,
        }
    }

    #[test]
    fn a_canonical_reclaim_root_can_create_a_git_worktree() {
        let bench = Bench::open();
        let path = bench.cut("canonical-root");
        assert!(
            path.join(".git").is_file(),
            "git did not create a linked worktree under {}",
            bench.worktrees.display()
        );
    }

    /// A checkout is resolved through its OWNING repository, never by opening
    /// git at the checkout itself.
    ///
    /// The trap this pins down is silent in both directions. Inside a linked
    /// worktree `rev-parse --show-toplevel` answers the worktree, so an
    /// orchestrator opened there believes the checkout is the repository: the
    /// worktree root it derives is hashed off that path, so the ownership rule
    /// calls every managed checkout `external` and NOTHING is ever reclaimed;
    /// and `git worktree remove` would then run with its working directory
    /// inside the tree it is removing.
    #[test]
    fn a_checkout_is_resolved_through_the_repository_that_owns_it() {
        let bench = Bench::open();
        let path = bench.cut("t-1140");
        let (projects, prefs) = bench.window();

        // The wrong road, measured rather than asserted about: this is what
        // `Orchestrator::open(<the checkout>)` actually answers.
        let opened_there = Orchestrator::open(&path).expect("open at the checkout");
        assert!(
            same_worktree_path(opened_there.repo_root(), &path),
            "git stopped answering the worktree here, and this trap changed shape"
        );

        let (orchestrator, known) =
            owning_repository(&projects, &prefs, &path).expect("the owning repository");
        assert!(
            same_worktree_path(orchestrator.repo_root(), &bench.repo),
            "the sweep resolved {} instead of the repository that owns it",
            orchestrator.repo_root().display()
        );
        assert!(same_worktree_path(&known.path, &path));
        assert_eq!(known.branch.as_deref(), Some("wt/t-1140"));
        // And the preferences came with it, which is what makes the ownership
        // rule recognise a checkout this window cut.
        assert!(
            matches!(judge(&orchestrator, &known), Verdict::Reclaim { .. }),
            "the resolved orchestrator does not know its own worktree root"
        );

        // A path no catalogued project lists is refused rather than resolved.
        let stranger = tempfile::tempdir().expect("a stranger");
        assert!(
            owning_repository(&projects, &prefs, stranger.path()).is_err(),
            "a directory outside every catalogued repository resolved anyway"
        );
        assert!(
            owning_repository(&[], &prefs, &path).is_err(),
            "an empty project catalog still handed out a repository"
        );
    }

    /// The whole point: a finished worker's checkout, clean and landed, is
    /// reclaimable — and the sentence says why, because a person who finds it
    /// gone is owed the reason.
    #[test]
    fn a_look_reports_the_same_facts_its_verdict_is_made_of() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();

        let landed = bench.cut("landed");
        assert_eq!(
            look(&orchestrator, &bench.listed(&landed), Holding::Base).1,
            Examined::Landed
        );

        let unlanded = bench.cut("unlanded-look");
        std::fs::write(unlanded.join("work.txt"), "real work\n").expect("write");
        git(&unlanded, &["add", "work.txt"]);
        git(&unlanded, &["commit", "-q", "-m", "real work"]);
        assert!(
            matches!(
                look(&orchestrator, &bench.listed(&unlanded), Holding::Base).1,
                Examined::Unlanded { commits: 1, .. }
            ),
            "{:?}",
            look(&orchestrator, &bench.listed(&unlanded), Holding::Base).1
        );

        let dirty = bench.cut("dirty-look");
        std::fs::write(dirty.join("notes.txt"), "half a thought\n").expect("write");
        assert_eq!(
            look(&orchestrator, &bench.listed(&dirty), Holding::Base).1,
            Examined::Uncommitted { changes: 1 }
        );

        // The repository's own checkout is shared work, not a worker cut.
        let (verdict, examined) = look(&orchestrator, &bench.listed(&bench.repo), Holding::Base);
        assert_eq!(examined, Examined::Shared);
        assert!(
            matches!(verdict, Verdict::Keep { ref because, .. } if because.contains("repository's own"))
        );
    }

    #[test]
    fn a_clean_landed_checkout_is_reclaimable_and_says_why() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();
        let path = bench.cut("t-1140");

        let Verdict::Reclaim {
            branch,
            base,
            takes,
            elsewhere,
            ..
        } = judge(&orchestrator, &bench.listed(&path))
        else {
            panic!("a clean, landed worker checkout was not reclaimable");
        };
        assert_eq!(
            (branch.as_str(), base.as_str()),
            ("wt/t-1140", "main"),
            "the reclaim does not carry the two names its sentence is made of"
        );
        assert!(takes.is_empty(), "{takes:?}");
        assert!(
            !elsewhere,
            "a branch fully in its own base rests on no other branch"
        );
    }

    /// And the removal reaches the build directory, which is the whole reason
    /// the reclaim exists. `target/` is ignored, and git deletes ignored paths
    /// with the worktree — so a checkout that has built is still clean, and
    /// removing it is what gets the gigabytes back.
    #[test]
    fn reclaiming_takes_the_build_directory_with_it() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();
        let path = bench.cut("t-1140");
        std::fs::write(path.join(".gitignore"), "target/\n").expect("ignore file");
        git(&path, &["add", ".gitignore"]);
        git(&path, &["commit", "-q", "-m", "ignore the build"]);
        git(&bench.repo, &["merge", "-q", "--ff-only", "wt/t-1140"]);
        std::fs::create_dir_all(path.join("target/debug")).expect("build directory");
        std::fs::write(path.join("target/debug/big"), vec![0u8; 4096]).expect("build output");

        let Verdict::Reclaim { takes, .. } = judge(&orchestrator, &bench.listed(&path)) else {
            panic!("a built checkout reads as dirty, so nothing would ever be reclaimed");
        };
        assert_eq!(
            takes,
            vec!["target".to_string()],
            "the reclaim does not account for the build tree it is about to take"
        );

        // But an ignored FILE is not a build tree, and one is enough to keep
        // the checkout: this road has nobody to show a list to before it acts.
        std::fs::write(path.join(".gitignore"), "target/\n.env\n").expect("ignore file");
        git(&path, &["add", ".gitignore"]);
        git(&path, &["commit", "-q", "-m", "ignore the secret too"]);
        git(&bench.repo, &["merge", "-q", "--ff-only", "wt/t-1140"]);
        std::fs::write(path.join(".env"), "TOKEN=real\n").expect("secret");
        let Verdict::Keep { because, .. } = judge(&orchestrator, &bench.listed(&path)) else {
            panic!("a checkout holding an ignored file was reclaimable");
        };
        assert!(
            because.contains(".env"),
            "the refusal does not name the file that would have gone: {because}"
        );
        std::fs::remove_file(path.join(".env")).expect("take the secret back out");
        assert!(matches!(
            judge(&orchestrator, &bench.listed(&path)),
            Verdict::Reclaim { .. }
        ));
        let data_root = tempfile::tempdir().expect("data root");
        let held = CheckoutHeld::take(&path).expect("no other road holds it");
        reclaim(&orchestrator, &held, data_root.path(), None).expect("reclaim");
        assert!(!path.exists(), "the checkout survived its own reclaim");
    }

    /// Every refusal, on one bench, because the list of them IS the contract:
    /// each of these is somebody's work, and each one that answered "reclaim"
    /// would be work deleted by a beat nobody asked.
    #[test]
    fn nothing_with_work_in_it_is_ever_reclaimed() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();

        // Uncommitted, and untracked: neither exists in any commit, so the
        // directory is the only place they are.
        let dirty = bench.cut("dirty");
        std::fs::write(dirty.join("notes.txt"), "half a thought\n").expect("write");
        let Verdict::Keep { because, .. } = judge(&orchestrator, &bench.listed(&dirty)) else {
            panic!("a checkout with an untracked file was reclaimable");
        };
        assert!(
            because.contains("uncommitted") && because.contains("notes.txt"),
            "the refusal does not say what would have been lost: {because}"
        );

        // Committed but unlanded. `remove` would leave the branch, so nothing
        // becomes unreachable — and the directory is still where a person
        // goes to look at work nothing has taken yet.
        let unlanded = bench.cut("unlanded");
        std::fs::write(unlanded.join("work.txt"), "real work\n").expect("write");
        git(&unlanded, &["add", "work.txt"]);
        git(&unlanded, &["commit", "-q", "-m", "real work"]);
        let Verdict::Keep { because, .. } = judge(&orchestrator, &bench.listed(&unlanded)) else {
            panic!("a checkout holding unlanded commits was reclaimable");
        };
        assert!(
            because.contains("1 commit(s)") && because.contains("wt/unlanded"),
            "the refusal does not say what has not landed: {because}"
        );

        // A base nobody wrote down. The question has no answer here, and no
        // answer keeps the directory.
        // With a commit the repository's main line lacks: one it holds in
        // full is reclaimed whatever the record says (t-76403).
        let unrecorded = bench.cut("unrecorded");
        git(
            &unrecorded,
            &["config", "--local", "--unset", "branch.wt/unrecorded.base"],
        );
        commit_work(&unrecorded, "unrecorded.txt");
        let Verdict::Keep { because, .. } = judge(&orchestrator, &bench.listed(&unrecorded)) else {
            panic!("a checkout with no recorded base was reclaimable");
        };
        assert!(because.contains("cut from"), "{because}");

        // A base that was written down and then deleted.
        let orphaned = bench.cut("orphaned");
        commit_work(&orphaned, "orphaned.txt");
        git(
            &orphaned,
            &[
                "config",
                "--local",
                "branch.wt/orphaned.base",
                "a-branch-that-went-away",
            ],
        );
        let Verdict::Keep { because, .. } = judge(&orchestrator, &bench.listed(&orphaned)) else {
            panic!("a checkout whose base is gone was reclaimable");
        };
        assert!(because.contains("cannot be resolved"), "{because}");

        // Locked, which is somebody saying so out loud.
        let locked = bench.cut("locked");
        git(
            &bench.repo,
            &["worktree", "lock", &locked.to_string_lossy()],
        );
        let Verdict::Keep { because, .. } = judge(&orchestrator, &bench.listed(&locked)) else {
            panic!("a locked checkout was reclaimable");
        };
        assert!(because.contains("locked"), "{because}");
        git(
            &bench.repo,
            &["worktree", "unlock", &locked.to_string_lossy()],
        );

        // Halfway through a merge: `git status` may be clean between steps of
        // a rebase, and a directory in the middle of an operation is not a
        // directory to take away.
        let halfway = bench.cut("halfway");
        std::fs::write(halfway.join("conflict.txt"), "ours\n").expect("write");
        git(&halfway, &["add", "conflict.txt"]);
        git(&halfway, &["commit", "-q", "-m", "ours"]);
        git(
            &halfway,
            &["checkout", "-q", "-b", "wt/halfway-theirs", "main"],
        );
        std::fs::write(halfway.join("conflict.txt"), "theirs\n").expect("write");
        git(&halfway, &["add", "conflict.txt"]);
        git(&halfway, &["commit", "-q", "-m", "theirs"]);
        git(&halfway, &["checkout", "-q", "wt/halfway"]);
        let merged = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(&halfway)
            .args(["merge", "wt/halfway-theirs"])
            .output()
            .expect("git merge");
        assert!(!merged.status.success(), "the merge should have conflicted");
        let Verdict::Keep { because, .. } = judge(&orchestrator, &bench.listed(&halfway)) else {
            panic!("a checkout mid-merge was reclaimable");
        };
        assert!(because.contains("in progress"), "{because}");

        // And the window's own workspace, whatever else is true of it.
        let landed = bench.cut("landed");
        assert!(
            matches!(
                settle(
                    &finished(&landed, "w-active-kept"),
                    &Road::Report(orchestrator.clone()),
                    &Where {
                        active: landed.clone(),
                        data_root: bench.repo.clone()
                    },
                    &nobody_left(),
                    1_000
                ),
                Settled::Kept
            ),
            "the workspace the window is showing was reclaimable"
        );

        for survivor in [
            &dirty,
            &unlanded,
            &unrecorded,
            &orphaned,
            &locked,
            &halfway,
            &landed,
        ] {
            assert!(survivor.is_dir(), "{} disappeared", survivor.display());
        }
    }

    /// A checkout somebody made by hand is a workspace, not this window's
    /// litter — even when the ledger seated a worker in it and that worker
    /// has finished.
    #[test]
    fn a_checkout_this_window_did_not_make_is_never_reclaimed() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();
        let temp = tempfile::tempdir().expect("a foreign directory");
        let foreign = temp.path().join("mine");
        git(
            &bench.repo,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                "mine",
                &foreign.to_string_lossy(),
                "main",
            ],
        );

        let Verdict::Keep { because, .. } = judge(&orchestrator, &bench.listed(&foreign)) else {
            panic!("a hand-made checkout was reclaimable");
        };
        assert!(because.contains("ownership is `external`"), "{because}");
        assert!(foreign.is_dir(), "the hand-made checkout disappeared");
    }

    /// t-73910: a worker checkout whose branch was moved onto another
    /// worker's unlanded branch — two commits beyond `main`, not one of them
    /// its own — the way the two released workers of 2026-10-25 stood as
    /// 「미반영 19 · 정리 보류」. Answers the checkout and the branch holding
    /// its commits.
    fn cut_on_anothers_branch(bench: &Bench, name: &str) -> (PathBuf, String) {
        let other = bench.cut(&format!("other-of-{name}"));
        for step in ["one", "two"] {
            std::fs::write(other.join(format!("{step}.txt")), step).expect("work");
            git(&other, &["add", "."]);
            git(&other, &["commit", "-q", "-m", step]);
        }
        let holder = bench
            .listed(&other)
            .branch
            .expect("the other checkout's branch");
        let path = bench.cut(name);
        git(&path, &["reset", "-q", "--hard", &holder]);
        (path, holder)
    }

    /// Every process count a recheck may answer for a removal resting on
    /// other branches, and nothing else in the way.
    fn nobody_left_and(everyone: ProcessCount<'_>) -> Recheck<'_> {
        Recheck {
            everyone: Some(everyone),
            ..nobody_left()
        }
    }

    /// The coordinator's four conditions, all met (t-73910): the worker is
    /// over and nothing stands in the directory, no commit is its own, the
    /// tree is clean, and the window cut it. It goes; its branch stays, the
    /// reclaim names the branch holding the commits, and the log says why.
    #[test]
    fn a_finished_checkout_with_no_commit_of_its_own_goes_and_says_which_branch_holds_them() {
        let bench = Bench::open();
        bench.ignore_on_main("target/\n");
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let (projects, prefs) = bench.window();
        let road = Road::Sweep { projects, prefs };
        let (path, holder) = cut_on_anothers_branch(&bench, "t-73948");
        Bench::build_tree(&path.join("target"));

        let orchestrator = bench.orchestrator();
        assert_eq!(
            orchestrator
                .commits_beyond(&path, "main", "wt/t-73948")
                .expect("git"),
            Some(2),
            "the plain count reads the other branch's commits as this one's"
        );
        assert_eq!(
            orchestrator
                .commits_held_elsewhere(&path, "main", "wt/t-73948")
                .expect("git"),
            Some(zerocode_orchestrator::HeldElsewhere {
                own: 0,
                holder: Some(holder.clone()),
            })
        );

        let settled = settle(
            &finished(&path, "w-73948"),
            &road,
            &here,
            &nobody_left_and(&|_| Some(0)),
            1_000,
        );
        let Settled::Reclaimed {
            branch,
            base,
            elsewhere,
        } = settled
        else {
            panic!("a finished checkout with no commit of its own was kept");
        };
        assert_eq!(
            (branch.as_str(), base.as_str(), elsewhere),
            ("wt/t-73948", holder.as_str(), true)
        );
        assert!(!path.exists(), "the checkout survived its reclaim");
        let listed = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(&bench.repo)
            .args(["branch", "--list", "wt/t-73948"])
            .output()
            .expect("git");
        assert!(
            !String::from_utf8_lossy(&listed.stdout).trim().is_empty(),
            "the branch went with the directory"
        );
        let said = std::fs::read_to_string(data_root.path().join("window-errors.log"))
            .expect("the window log");
        assert!(
            said.lines()
                .any(|line| line.contains("no commit of its own")
                    && line.contains(&format!("every commit on it is on {holder}"))),
            "no removal line says the commits are on another branch:\n{said}"
        );
    }

    /// Each condition missing keeps the checkout (t-73910): a process
    /// standing in it or a count nobody could read, a ledger that seated
    /// somebody in it again, one commit of the worker's own, an untracked
    /// file, and a checkout this window did not cut. The one with work of its
    /// own says what it waits for, in the catalogue's row.
    #[test]
    fn a_checkout_missing_any_condition_of_the_four_is_kept() {
        let _window = crate::orchestration::tests::the_window();
        let bench = Bench::open();
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let road = Road::Report(bench.orchestrator());
        let kept = |path: &Path, recheck: &Recheck<'_>, why: &str| {
            let settled = settle(&finished(path, "w-kept"), &road, &here, recheck, 1_000);
            assert!(matches!(settled, Settled::Kept), "{why}: the checkout went");
            assert!(path.is_dir(), "{why}: the directory is gone");
        };

        let (busy, busy_holder) = cut_on_anothers_branch(&bench, "t-busy");
        kept(
            &busy,
            &nobody_left_and(&|_| Some(1)),
            "a process standing in it",
        );
        let (unread, _) = cut_on_anothers_branch(&bench, "t-unread");
        kept(
            &unread,
            &nobody_left_and(&|_| None),
            "a process count nobody could read",
        );
        let (seated, _) = cut_on_anothers_branch(&bench, "t-seated");
        kept(
            &seated,
            &Recheck {
                settled: &|_| false,
                ..nobody_left_and(&|_| Some(0))
            },
            "a ledger that seated somebody in it again",
        );

        let (own, _) = cut_on_anothers_branch(&bench, "t-own");
        std::fs::write(own.join("mine.txt"), "the worker's own").expect("work");
        git(&own, &["add", "mine.txt"]);
        git(&own, &["commit", "-q", "-m", "the worker's own"]);
        kept(
            &own,
            &nobody_left_and(&|_| Some(0)),
            "one commit of its own",
        );
        let reason = catalog_reclaim(&bench, &own);
        assert_eq!(reason["kind"], "unlanded", "{reason}");
        assert_eq!(
            reason["waits"],
            serde_json::json!({ "own": 1, "base": "main" }),
            "the row does not say what the checkout waits for: {reason}"
        );

        let (dirty, _) = cut_on_anothers_branch(&bench, "t-dirty");
        std::fs::write(dirty.join("notes.txt"), "half a thought").expect("write");
        kept(&dirty, &nobody_left_and(&|_| Some(0)), "an untracked file");

        let temp = tempfile::tempdir().expect("a foreign directory");
        let foreign = temp.path().join("mine");
        git(
            &bench.repo,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                "mine",
                &foreign.to_string_lossy(),
                &busy_holder,
            ],
        );
        let Verdict::Keep { because, .. } = judge(&bench.orchestrator(), &bench.listed(&foreign))
        else {
            panic!("a hand-made checkout on another branch was reclaimable");
        };
        assert!(because.contains("ownership is `external`"), "{because}");
    }

    /// The same refusal must not fill the log with a line a second, and a
    /// refusal that CHANGES must not be swallowed by the one before it.
    #[test]
    fn one_refusal_is_written_down_once() {
        let data_root = tempfile::tempdir().expect("data root");
        let candidate = orchestration::SettledCheckout {
            path: "/wt/t-1140".to_string(),
            worker: "w-1".to_string(),
            agent: "claude".to_string(),
            run: "run-1".to_string(),
        };
        let log = data_root.path().join("window-errors.log");

        remember(
            &candidate,
            Standing::Kept,
            "it is dirty",
            data_root.path(),
            1_000,
        );
        remember(
            &candidate,
            Standing::Kept,
            "it is dirty",
            data_root.path(),
            2_000,
        );
        let said = std::fs::read_to_string(&log).expect("the window log");
        assert_eq!(
            said.lines()
                .filter(|line| line.contains("/wt/t-1140"))
                .count(),
            1,
            "an unchanged refusal was written twice:\n{said}"
        );
        assert!(said.contains("was kept: it is dirty"), "{said}");

        remember(
            &candidate,
            Standing::Kept,
            "its base is gone",
            data_root.path(),
            3_000,
        );
        let said = std::fs::read_to_string(&log).expect("the window log");
        assert!(
            said.contains("its base is gone"),
            "a refusal that changed was swallowed:\n{said}"
        );

        // And the mark is dropped once the checkout stops being a candidate,
        // so a window running for days holds one entry per standing orphan.
        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&candidate.path);
    }

    /// A worker that ran in the main checkout, or in a directory no project
    /// lists, left nothing behind to keep: the line says so, in those words.
    #[test]
    fn a_checkout_that_was_never_ours_is_not_said_to_be_kept() {
        let data_root = tempfile::tempdir().expect("data root");
        let candidate = orchestration::SettledCheckout {
            path: "/repo/main-checkout".to_string(),
            worker: "w-2".to_string(),
            agent: "codex".to_string(),
            run: "run-1".to_string(),
        };
        let log = data_root.path().join("window-errors.log");

        remember(
            &candidate,
            Standing::NotOurs,
            "it is the repository's own checkout",
            data_root.path(),
            1_000,
        );
        let said = std::fs::read_to_string(&log).expect("the window log");
        assert!(
            said.contains("left at /repo/main-checkout is not this window's to reclaim: it is the repository's own checkout")
                && !said.contains("was kept"),
            "a checkout that was never a candidate read as a leftover:\n{said}"
        );

        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&candidate.path);
    }

    #[test]
    fn leaving_the_active_checkout_does_not_wait_for_a_cached_refusal() {
        let bench = Bench::open();
        let path = bench.cut("finished-active");
        let data_root = tempfile::tempdir().expect("data root");
        let candidate = finished(&path, "w-active");
        let mut here = Where {
            active: path.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let road = Road::Report(bench.orchestrator());
        assert!(matches!(
            settle(&candidate, &road, &here, &nobody_left(), 1_000),
            Settled::Kept
        ));
        assert!(path.is_dir(), "the window's active checkout was removed");
        assert!(
            !judged().lock().unwrap().contains_key(&candidate.path),
            "standing in a checkout left a fifteen-minute refusal behind"
        );
        here.active = bench.repo.clone();
        assert!(
            matches!(
                settle(&candidate, &road, &here, &nobody_left(), 1_001),
                Settled::Reclaimed { .. }
            ),
            "leaving the checkout did not make it immediately reclaimable"
        );
        assert!(!path.exists());
    }

    fn catalog_reclaim(bench: &Bench, path: &Path) -> serde_json::Value {
        serde_json::to_value(crate::WorktreeEntry::new(bench.listed(path), &bench.repo))
            .expect("the catalogue row")["reclaim"]
            .clone()
    }

    #[test]
    fn the_catalog_reads_transient_reclaim_reasons_without_caching_a_refusal() {
        let _window = crate::orchestration::tests::the_window();
        let bench = Bench::open();
        let path = bench.cut("kept-open");
        let data_root = tempfile::tempdir().expect("data root");
        let candidate = finished(&path, "w-kept-open");
        let road = Road::Report(bench.orchestrator());
        let mut here = Where {
            active: path.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        assert!(matches!(
            settle(&candidate, &road, &here, &nobody_left(), 1_000),
            Settled::Kept
        ));
        assert_eq!(catalog_reclaim(&bench, &path)["kind"], "active");
        assert!(!judged().lock().unwrap().contains_key(&candidate.path));

        here.active = bench.repo.clone();
        let occupied = Recheck {
            terminals: &|_| 1,
            ..nobody_left()
        };
        assert!(matches!(
            settle(&candidate, &road, &here, &occupied, 1_001),
            Settled::Kept
        ));
        assert_eq!(catalog_reclaim(&bench, &path)["kind"], "open_conversation");
        assert!(!judged().lock().unwrap().contains_key(&candidate.path));
        assert!(matches!(
            settle(&candidate, &road, &here, &nobody_left(), 1_002),
            Settled::Reclaimed { .. }
        ));
    }

    #[test]
    fn the_catalog_reads_the_lock_reason_from_reclaim() {
        let _window = crate::orchestration::tests::the_window();
        let bench = Bench::open();
        let path = bench.cut("kept-locked");
        git(&bench.repo, &["worktree", "lock", path.to_str().unwrap()]);
        let data_root = tempfile::tempdir().expect("data root");
        let candidate = finished(&path, "w-kept-locked");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        assert!(matches!(
            settle(
                &candidate,
                &Road::Report(bench.orchestrator()),
                &here,
                &nobody_left(),
                1_000
            ),
            Settled::Kept
        ));
        let said = catalog_reclaim(&bench, &path);
        assert_eq!(said["kind"], "locked");
        assert!(said["detail"].as_str().unwrap().contains("locked"));
        assert!(path.is_dir());
    }

    #[test]
    fn the_catalog_reads_ignored_files_as_the_reclaim_reason() {
        let _window = crate::orchestration::tests::the_window();
        let bench = Bench::open();
        bench.ignore_on_main("target/\n.notes-local\n");
        let path = bench.cut("kept-notes");
        bench.move_in_warm_target(&path);
        std::fs::write(path.join(".notes-local"), "keep this local note").unwrap();
        let data_root = tempfile::tempdir().expect("data root");
        let candidate = finished(&path, "w-kept-notes");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        assert!(matches!(
            settle(
                &candidate,
                &Road::Report(bench.orchestrator()),
                &here,
                &nobody_left(),
                1_000
            ),
            Settled::Kept
        ));
        let said = catalog_reclaim(&bench, &path);
        assert_eq!(said["kind"], "ignored_file");
        assert!(said["detail"].as_str().unwrap().contains(".notes-local"));
        assert!(path.join(".notes-local").is_file());
    }

    /// The accident of 2026-09-28 on a bench (t-12773): a worker reported
    /// `ok:true` holding a commit its base had not taken, screenshots under an
    /// ignored `output/`, and a warmed `target/` its coordinator had moved in.
    /// The report road removed on git's word alone and took all of it. It
    /// takes the one judgment now: the checkout stays with every one of those
    /// in it, and the window log names the question that kept it.
    #[test]
    fn a_reported_checkout_holding_unlanded_work_keeps_its_evidence_and_its_build() {
        let bench = Bench::open();
        bench.ignore_on_main("output/\ntarget/\n");
        let path = bench.cut("t-12563");
        std::fs::write(path.join("work.rs"), "fn unseen_badges() {}\n").expect("work");
        git(&path, &["add", "work.rs"]);
        git(&path, &["commit", "-q", "-m", "the worker's own commit"]);
        std::fs::create_dir_all(path.join("output/t-12563")).expect("evidence folder");
        for shot in ["dark.png", "light.png"] {
            std::fs::write(path.join("output/t-12563").join(shot), PNG).expect("screenshot");
        }
        bench.move_in_warm_target(&path);
        let data_root = tempfile::tempdir().expect("data root");
        let candidate = finished(&path, "w-12563");

        let settled = settle(
            &candidate,
            &Road::Report(bench.orchestrator()),
            &Where {
                active: bench.repo.clone(),
                data_root: data_root.path().to_path_buf(),
            },
            &nobody_left(),
            1_000,
        );

        assert!(
            matches!(settled, Settled::Kept),
            "the report road took a checkout holding a commit main has not taken"
        );
        for survivor in [
            "work.rs",
            "output/t-12563/dark.png",
            "output/t-12563/light.png",
            "target/CACHEDIR.TAG",
            "target/debug/big",
        ] {
            assert!(
                path.join(survivor).is_file(),
                "{survivor} went with the checkout"
            );
        }
        let said = std::fs::read_to_string(data_root.path().join("window-errors.log"))
            .expect("the window log");
        assert!(
            said.contains("was kept: 1 commit(s) on wt/t-12563 are not in main yet")
                && !said.contains("worktree removed"),
            "the log does not name the question that kept it:\n{said}"
        );

        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&candidate.path);
    }

    /// A kept checkout keeps its work and loses its build (t-78886, review
    /// O9). A finished worker's branch waiting to land kept the whole checkout
    /// — on 2026-10-28 one `target/` of 14 GiB and one of 9 GiB, and the disk
    /// fell under its floor four times in 25 minutes. The commit, the
    /// uncommitted file and the evidence stay for whoever comes to look; every
    /// build tree a build tool tagged goes, nested ones too — once no process
    /// stands in the directory, never through a link, and never an ignored
    /// folder nobody tagged.
    #[test]
    fn a_kept_checkout_keeps_its_work_and_its_build_trees_go_once_nobody_stands_in_it() {
        let bench = Bench::open();
        bench.ignore_on_main("output/\ntarget/\nshared/\n");
        let path = bench.cut("t-78886-kept");
        std::fs::create_dir_all(path.join("tools")).expect("tools");
        std::fs::write(path.join("tools/work.rs"), "fn waiting_to_land() {}\n").expect("work");
        git(&path, &["add", "tools/work.rs"]);
        git(&path, &["commit", "-q", "-m", "the worker's own commit"]);
        std::fs::write(path.join("draft.rs"), "fn not_committed() {}\n").expect("draft");
        std::fs::create_dir_all(path.join("output/t-78886")).expect("evidence folder");
        std::fs::write(path.join("output/t-78886/shot.png"), PNG).expect("screenshot");
        Bench::build_tree(&path.join("target"));
        Bench::build_tree(&path.join("tools/target"));
        #[cfg(unix)]
        let shared = {
            let shared = bench.repo.with_file_name("shared-cache");
            Bench::build_tree(&shared);
            std::os::unix::fs::symlink(&shared, path.join("shared")).expect("shared link");
            shared
        };
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let candidate = finished(&path, "w-78886");
        let forget = || {
            judged()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&candidate.path);
        };

        // A process standing in it — a build, a shell — keeps the build.
        let standing = |_: &Path| Some(1);
        assert!(matches!(
            settle(
                &candidate,
                &Road::Report(bench.orchestrator()),
                &here,
                &nobody_left_and(&standing),
                1_000
            ),
            Settled::Kept
        ));
        assert!(
            path.join("target/debug/big").is_file(),
            "a build in use went"
        );
        forget();

        // Nobody, on the first look and the next one a whole wait later.
        let nobody = |_: &Path| Some(0);
        for at in [1_001, 1_001 + RE_JUDGE_AFTER] {
            assert!(matches!(
                settle(
                    &candidate,
                    &Road::Report(bench.orchestrator()),
                    &here,
                    &nobody_left_and(&nobody),
                    at
                ),
                Settled::Kept
            ));
        }
        for gone in ["target", "tools/target"] {
            assert!(
                !path.join(gone).exists(),
                "{gone} stood on in a checkout nobody stands in"
            );
        }
        assert!(
            set_aside_in(&path).is_empty(),
            "a build tree set aside stayed"
        );
        for survivor in ["tools/work.rs", "draft.rs", "output/t-78886/shot.png"] {
            assert!(
                path.join(survivor).is_file(),
                "{survivor} went with the build"
            );
        }
        #[cfg(unix)]
        assert!(
            path.join("shared").is_symlink() && shared.join("debug/big").is_file(),
            "a shared directory was followed through its link"
        );
        let said = std::fs::read_to_string(data_root.path().join("window-errors.log"))
            .expect("the window log");
        assert!(
            said.contains("stays, and its build tree(s) went:")
                && said.contains("tools/target")
                && !said.contains("worktree removed"),
            "the log does not say what went:\n{said}"
        );
        forget();
    }

    /// The build trees set aside in a checkout's own git directory that are
    /// still there.
    fn set_aside_in(checkout: &Path) -> Vec<String> {
        let own = linked_git_dir(checkout).expect("a linked checkout");
        std::fs::read_dir(own)
            .expect("its git directory")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(SET_ASIDE))
            .collect()
    }

    /// A finished worker's checkout with a commit of its own still to land
    /// and a cargo build in it — the shape a task sent back is seated in again.
    fn kept_with_a_build(bench: &Bench, name: &str) -> PathBuf {
        bench.ignore_on_main("target/\n");
        let path = bench.cut(name);
        std::fs::write(path.join("work.rs"), "fn waiting_to_land() {}\n").expect("work");
        git(&path, &["add", "work.rs"]);
        git(&path, &["commit", "-q", "-m", "the worker's own commit"]);
        Bench::build_tree(&path.join("target"));
        path
    }

    /// The first quiet look keeps the build (t-78886 r2, review r1 P2-3): a
    /// task sent back is seated in the same checkout within minutes, and a
    /// build gone by then made every retry a cold one. A look a whole wait
    /// later that still finds nobody, under the same hand-in, takes it; a
    /// retry's hand-in starts its own wait.
    #[test]
    fn a_kept_checkout_keeps_its_build_through_its_first_quiet_look_and_sheds_it_on_the_next() {
        let bench = Bench::open();
        let path = kept_with_a_build(&bench, "t-78886-r2-wait");
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let nobody = |_: &Path| Some(0);
        let look = |worker: &str, at: i64| {
            assert!(matches!(
                settle(
                    &finished(&path, worker),
                    &Road::Report(bench.orchestrator()),
                    &here,
                    &nobody_left_and(&nobody),
                    at
                ),
                Settled::Kept
            ));
        };
        let build = path.join("target/debug/big");
        look("w-1", 1_000);
        assert!(build.is_file(), "the first quiet look took the build");
        // A branch moving brings the next look early: still within the wait.
        look("w-1", 1_000 + RE_JUDGE_AFTER - 1);
        assert!(build.is_file(), "a look within the wait took the build");
        // The task was sent back, and its retry handed in from the same
        // checkout: its own first quiet look.
        look("w-2", 1_000 + RE_JUDGE_AFTER);
        assert!(build.is_file(), "a retry's first quiet look took the build");
        look("w-2", 1_000 + 2 * RE_JUDGE_AFTER);
        assert!(
            !path.join("target").exists(),
            "the second quiet look kept the build"
        );
        assert!(
            path.join("work.rs").is_file(),
            "the work went with the build"
        );
        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&path.to_string_lossy().into_owned());
    }

    /// Under the disk floor the build goes on the first quiet look (t-78886
    /// r2): a warm build waiting for a retry is not worth the next summons
    /// refused for room. At the floor itself it waits.
    #[test]
    fn a_kept_checkout_under_the_disk_floor_sheds_its_build_on_the_first_quiet_look() {
        let bench = Bench::open();
        let path = kept_with_a_build(&bench, "t-78886-r2-floor");
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let candidate = finished(&path, "w-1");
        let nobody = |_: &Path| Some(0);
        for (free, stays) in [(DISK_FLOOR, true), (DISK_FLOOR - 1, false)] {
            let free_bytes = move |_: &Path| Some(free);
            assert!(matches!(
                settle(
                    &candidate,
                    &Road::Report(bench.orchestrator()),
                    &here,
                    &Recheck {
                        free_bytes: Some(&free_bytes),
                        ..nobody_left_and(&nobody)
                    },
                    1_000
                ),
                Settled::Kept
            ));
            assert_eq!(
                path.join("target/debug/big").is_file(),
                stays,
                "{free} bytes free"
            );
        }
        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&candidate.path);
    }

    /// A build tree is set aside before it is deleted, and everyone is asked
    /// again in between (t-78886 r2, review r1 P2-2): a process that came to
    /// stand in the checkout meanwhile finds its build put back where it was,
    /// and nothing set aside is left behind.
    #[test]
    fn a_build_tree_set_aside_is_put_back_for_somebody_who_came_meanwhile() {
        let bench = Bench::open();
        let path = kept_with_a_build(&bench, "t-78886-r2-came");
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let candidate = finished(&path, "w-1");
        // Nobody when first asked; a build standing in it when asked again.
        let asked = std::cell::Cell::new(0_usize);
        let came = |_: &Path| {
            asked.set(asked.get() + 1);
            Some(usize::from(asked.get() > 1))
        };
        let low = |_: &Path| Some(0_u64);
        assert!(matches!(
            settle(
                &candidate,
                &Road::Report(bench.orchestrator()),
                &here,
                &Recheck {
                    free_bytes: Some(&low),
                    ..nobody_left_and(&came)
                },
                1_000
            ),
            Settled::Kept
        ));
        assert_eq!(asked.get(), 2, "everyone was not asked again");
        assert!(
            path.join("target/debug/big").is_file(),
            "the build was not put back"
        );
        assert!(set_aside_in(&path).is_empty(), "a set-aside tree stayed");
        let said = std::fs::read_to_string(data_root.path().join("window-errors.log"))
            .expect("the window log");
        assert!(
            said.contains("while its build was set aside; put back: target")
                && !said.contains("build tree(s) went"),
            "the log does not say it was put back:\n{said}"
        );
        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&candidate.path);
    }

    /// What a look could not delete is written down and deleted on a later
    /// look (t-78886 r2, review r1 P2-2) — a tree half gone, its cache tag
    /// with it, included: it is found by the name it was set aside under,
    /// not by the tag.
    #[cfg(unix)]
    #[test]
    fn a_build_tree_set_aside_that_would_not_go_is_written_down_and_goes_on_a_later_look() {
        use std::os::unix::fs::PermissionsExt;
        let bench = Bench::open();
        let path = kept_with_a_build(&bench, "t-78886-r2-stuck");
        let own = linked_git_dir(&path).expect("a linked checkout");
        let half = own.join(format!("{SET_ASIDE}5-0"));
        std::fs::create_dir_all(half.join("debug")).expect("half a tree");
        std::fs::write(half.join("debug/big"), vec![0u8; 4096]).expect("build output");
        let stuck = own.join(format!("{SET_ASIDE}6-0"));
        std::fs::create_dir_all(stuck.join("debug")).expect("a stuck tree");
        std::fs::write(stuck.join("debug/big"), vec![0u8; 4096]).expect("build output");
        let mode = |bits| {
            std::fs::set_permissions(stuck.join("debug"), std::fs::Permissions::from_mode(bits))
                .expect("mode");
        };
        mode(0o500);
        // A superuser deletes through the mode, and there is nothing stuck.
        let modes_hold = std::fs::write(stuck.join("debug/probe"), "").is_err();
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let candidate = finished(&path, "w-1");
        let nobody = |_: &Path| Some(0);
        let look = |at: i64| {
            assert!(matches!(
                settle(
                    &candidate,
                    &Road::Report(bench.orchestrator()),
                    &here,
                    &nobody_left_and(&nobody),
                    at
                ),
                Settled::Kept
            ));
        };

        look(1_000);
        assert!(
            !half.exists(),
            "a half-gone tree set aside was not found again"
        );
        assert!(
            path.join("target/debug/big").is_file(),
            "the first quiet look took the live build"
        );
        if modes_hold {
            assert!(stuck.is_dir(), "a tree that could not go went");
            let said = std::fs::read_to_string(data_root.path().join("window-errors.log"))
                .expect("the window log");
            assert!(
                said.contains("could not be deleted") && said.contains(&format!("{SET_ASIDE}6-0")),
                "the failed delete was not written down:\n{said}"
            );
        }

        mode(0o700);
        look(1_000 + RE_JUDGE_AFTER);
        assert!(set_aside_in(&path).is_empty(), "a set-aside tree stayed");
        assert!(
            !path.join("target").exists(),
            "the second quiet look kept the build"
        );
        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&candidate.path);
    }

    /// What is set aside lives in the checkout's own git directory, so the
    /// checkout's removal takes it too (t-78886 r2): `git worktree remove`
    /// deletes that directory whole, and nothing set aside outlives the
    /// checkout it came from.
    #[test]
    fn a_build_tree_set_aside_goes_with_its_checkout() {
        let bench = Bench::open();
        let path = kept_with_a_build(&bench, "t-78886-r2-removed");
        let own = linked_git_dir(&path).expect("a linked checkout");
        let parked = own.join(format!("{SET_ASIDE}7-0"));
        std::fs::create_dir_all(parked.join("debug")).expect("a tree set aside");
        std::fs::write(parked.join("debug/big"), vec![0u8; 4096]).expect("build output");
        git(
            &bench.repo,
            &["worktree", "remove", &path.to_string_lossy()],
        );
        assert!(!path.exists(), "the checkout stayed");
        assert!(!parked.exists(), "a tree set aside outlived its checkout");
    }

    /// The tag is read for its signature and no more (review t-79535 P3-8):
    /// a large file that starts with it is the tag, one that does not is not,
    /// and a pipe or a link in its place is neither — read at once, never
    /// waited on.
    #[test]
    fn a_cache_tag_is_read_for_its_signature_alone() {
        let temp = tempfile::tempdir().expect("a build tree");
        let tree = temp.path();
        assert!(!carries_cache_tag(tree), "no tag at all");

        let tag = tree.join(CACHE_DIRECTORY_TAG);
        let mut big = CACHE_DIRECTORY_SIGNATURE.to_vec();
        big.extend(std::iter::repeat_n(b'x', 8 * 1024 * 1024));
        std::fs::write(&tag, &big).expect("a large tag");
        assert!(carries_cache_tag(tree), "the signature leads a large file");

        std::fs::write(&tag, &CACHE_DIRECTORY_SIGNATURE[1..]).expect("a short tag");
        assert!(!carries_cache_tag(tree), "a file that is not the signature");
        std::fs::write(&tag, b"a file of the same name").expect("another file");
        assert!(!carries_cache_tag(tree));

        std::fs::remove_file(&tag).expect("clear");
        std::fs::create_dir(&tag).expect("a directory of the name");
        assert!(!carries_cache_tag(tree), "a directory is no tag");
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_or_a_link_in_the_tags_place_is_not_waited_on() {
        let temp = tempfile::tempdir().expect("a build tree");
        let tree = temp.path();
        let tag = tree.join(CACHE_DIRECTORY_TAG);
        let made = crate::proc::quiet_command("mkfifo")
            .arg(&tag)
            .status()
            .expect("mkfifo");
        assert!(made.success(), "mkfifo");
        let started = std::time::Instant::now();
        assert!(!carries_cache_tag(tree), "a pipe is no tag");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "a pipe held the look"
        );

        std::fs::remove_file(&tag).expect("clear");
        let real = tree.join("elsewhere");
        std::fs::write(&real, CACHE_DIRECTORY_SIGNATURE).expect("a real tag");
        std::os::unix::fs::symlink(&real, &tag).expect("a link");
        assert!(!carries_cache_tag(tree), "a link is no tag");
    }

    /// A checkout a person locked keeps its build as it keeps everything
    /// else (t-78886): the lock is their word to leave it be.
    #[test]
    fn a_locked_checkout_keeps_its_build_tree() {
        let bench = Bench::open();
        bench.ignore_on_main("target/\n");
        let path = bench.cut("t-78886-locked");
        Bench::build_tree(&path.join("target"));
        git(&bench.repo, &["worktree", "lock", &path.to_string_lossy()]);
        let data_root = tempfile::tempdir().expect("data root");
        let candidate = finished(&path, "w-78887");
        let nobody = |_: &Path| Some(0);
        assert!(matches!(
            settle(
                &candidate,
                &Road::Report(bench.orchestrator()),
                &Where {
                    active: bench.repo.clone(),
                    data_root: data_root.path().to_path_buf(),
                },
                &nobody_left_and(&nobody),
                1_000
            ),
            Settled::Kept
        ));
        assert!(
            path.join("target/debug/big").is_file(),
            "a locked checkout lost its build"
        );
        git(
            &bench.repo,
            &["worktree", "unlock", &path.to_string_lossy()],
        );
        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&candidate.path);
    }

    /// And the reclaim goes on (t-12773). A checkout whose base has taken
    /// every commit, with nothing ignored in it but a cargo build tree, is
    /// still taken by both roads: the gigabytes are what the reclaim exists
    /// to get back, and one judgment for two roads must not turn into two
    /// roads that keep everything.
    #[test]
    fn a_landed_checkout_holding_only_its_build_is_still_reclaimed_by_both_roads() {
        let bench = Bench::open();
        bench.ignore_on_main("target/\n");
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let (projects, prefs) = bench.window();
        for (name, road) in [
            ("t-12600", Road::Report(bench.orchestrator())),
            ("t-12601", Road::Sweep { projects, prefs }),
        ] {
            let path = bench.cut(name);
            let branch = format!("wt/{name}");
            let work = format!("{name}.rs");
            std::fs::write(path.join(&work), "fn landed() {}\n").expect("work");
            git(&path, &["add", &work]);
            git(&path, &["commit", "-q", "-m", "landed work"]);
            git(&bench.repo, &["merge", "-q", "--ff-only", &branch]);
            Bench::build_tree(&path.join("target"));

            let settled = settle(
                &finished(&path, "w-12600"),
                &road,
                &here,
                &nobody_left(),
                1_000,
            );
            let Settled::Reclaimed {
                branch: took, base, ..
            } = settled
            else {
                panic!("{name}: a clean, landed checkout holding only its build was kept");
            };
            assert_eq!(
                (took.as_str(), base.as_str()),
                (branch.as_str(), "main"),
                "{name}: the reclaim does not carry the two names its sentence is made of"
            );
            assert!(!path.exists(), "{name}: the checkout survived its reclaim");
            let said = std::fs::read_to_string(data_root.path().join("window-errors.log"))
                .expect("the window log");
            let removal = format!("worktree removed [{}] {}", road.source(), path.display());
            assert!(
                said.lines()
                    .any(|line| line.contains(&removal) && line.contains("it took target")),
                "{name}: no removal line names its road and what it took:\n{said}"
            );
        }
    }

    /// One road at a time (t-12773). While another road holds a checkout —
    /// here the report road, as if halfway through taking it apart — the
    /// sweep neither reads it nor writes a word about it, and remembers
    /// nothing, so the judgment after the claim is let go is an ordinary one.
    /// On 2026-09-28 the sweep read such a half-deleted tree and wrote "was
    /// kept: 1334 uncommitted change(s)" about a leftover that never existed.
    #[test]
    fn a_checkout_another_road_holds_is_neither_read_nor_taken() {
        let bench = Bench::open();
        let path = bench.cut("t-12773");
        let (projects, prefs) = bench.window();
        let road = Road::Sweep { projects, prefs };
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let candidate = finished(&path, "w-12773");
        let log = data_root.path().join("window-errors.log");

        let report = CheckoutHeld::take(&path).expect("the report road takes it first");
        assert!(
            CheckoutHeld::take(&path).is_none(),
            "a second road took a checkout another road was holding"
        );
        assert!(
            matches!(
                settle(&candidate, &road, &here, &nobody_left(), 1_000),
                Settled::Elsewhere
            ),
            "the sweep judged a checkout another road was holding"
        );
        assert!(
            path.is_dir(),
            "the sweep took a checkout another road was holding"
        );
        assert!(
            !log.exists(),
            "the sweep wrote about a checkout it was not holding:\n{}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
        assert!(
            judged()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&candidate.path)
                .is_none(),
            "a checkout the sweep never read is remembered as judged"
        );

        drop(report);
        assert!(
            matches!(
                settle(&candidate, &road, &here, &nobody_left(), 2_000),
                Settled::Reclaimed { .. }
            ),
            "the claim outlived the road that held it"
        );
        assert!(!path.exists(), "the checkout survived its reclaim");
    }

    /// What the worker handed in is kept before its checkout goes (t-32798).
    /// A hand-in still being kept, or one whose keeping failed, leaves the
    /// directory standing on both roads — the failed one with its reason in the
    /// window log, once — and the directory goes the moment nothing is owed.
    #[test]
    fn a_checkout_whose_hand_in_is_not_kept_stays_and_says_why_and_goes_when_it_is() {
        let bench = Bench::open();
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let (projects, prefs) = bench.window();
        let because = "the hand-in m-7 of task t-9 was not kept (copy_failed: No space left on \
                       device) — the checkout stays until it is";
        for (name, road) in [
            ("t-32798", Road::Report(bench.orchestrator())),
            ("t-32799", Road::Sweep { projects, prefs }),
        ] {
            let path = bench.cut(name);
            let candidate = finished(&path, "w-32798");
            fn settled(_: &str) -> bool {
                true
            }
            fn terminals(_: &Path) -> usize {
                0
            }
            let owed = |_: &Path| Clearance::Keeping("still being kept".to_string());
            let failed = |_: &Path| Clearance::Held(because.to_string());
            let clear = |_: &Path| Clearance::Clear;
            for (what, keeping) in [
                ("being kept", &owed as &dyn Fn(&Path) -> Clearance),
                ("failed", &failed as &dyn Fn(&Path) -> Clearance),
            ] {
                let recheck = Recheck {
                    settled: &settled,
                    terminals: &terminals,
                    keeping,
                    processes: None,
                    everyone: None,
                    free_bytes: None,
                };
                assert!(
                    matches!(
                        settle(&candidate, &road, &here, &recheck, 1_000),
                        Settled::Kept
                    ),
                    "{name}: a checkout whose hand-in is {what} was taken"
                );
                assert!(
                    path.is_dir(),
                    "{name}: the checkout went with its hand-in {what}"
                );
            }
            let said = std::fs::read_to_string(data_root.path().join("window-errors.log"))
                .unwrap_or_default();
            let line = format!(
                "the checkout worker w-32798 left at {} was kept: {because}",
                path.display()
            );
            assert_eq!(
                said.matches(&line).count(),
                1,
                "{name}: the refusal is not written down once, with its reason:\n{said}"
            );
            assert!(
                !said.contains("still being kept"),
                "{name}: a keeping that only waits its turn was written down:\n{said}"
            );
            let recheck = Recheck {
                settled: &settled,
                terminals: &terminals,
                keeping: &clear,
                processes: None,
                everyone: None,
                free_bytes: None,
            };
            assert!(
                matches!(
                    settle(&candidate, &road, &here, &recheck, 2_000),
                    Settled::Reclaimed { .. }
                ),
                "{name}: the checkout stayed once nothing was owed"
            );
            assert!(!path.exists(), "{name}: the checkout survived its reclaim");
            judged()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&candidate.path);
        }
    }

    /// The commit `what` names in `repo`, as git spells it.
    fn rev(repo: &Path, what: &str) -> String {
        let output = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", what])
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git rev-parse {what} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// The person's report (10-11, again 10-14): one task, t-26584, stood in
    /// the sidebar four times — `before`, `jev-evidence`, `jev-green`,
    /// `verify`, the checkouts its worker cut beside its own — each row saying
    /// it had unlanded commits and no agent, long after the worker had handed
    /// its head in, and finished work piled up (t-45336). The hand-in takes
    /// back every checkout beside it that the handed-in head holds, that is
    /// clean and that nobody stands in, without force, and the window log says
    /// which road took it. A dirty one, a locked one, one holding a commit the
    /// head does not, one a process stands in, and another task's all stay —
    /// and the branch a checkout was on, the worker's own checkout and every
    /// commit survive.
    #[test]
    fn a_hand_in_takes_back_the_checkouts_beside_it_that_its_head_holds() {
        let bench = Bench::open();
        bench.ignore_on_main("target/\n");
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        // The worker's own checkout, cut by the window: a red, then the green
        // it hands in.
        let own = bench.cut("t-26584");
        let commit = |at: &Path, file: &str| {
            std::fs::write(at.join(file), format!("// {file}\n")).expect("work");
            git(at, &["add", file]);
            git(at, &["commit", "-q", "-m", file]);
            rev(at, "HEAD")
        };
        let red = commit(&own, "red.rs");
        let head = commit(&own, "green.rs");
        // The checkouts beside it, cut the way a worker cuts them by hand:
        // detached at a commit, or on a branch of their own.
        let cut_beside = |name: &str, branch: Option<&str>, at: &str| {
            let path = bench.worktrees.join(name);
            let spelled = path.to_string_lossy().into_owned();
            let mut args = vec!["worktree", "add"];
            match branch {
                Some(branch) => args.extend(["-b", branch]),
                None => args.push("--detach"),
            }
            args.extend([spelled.as_str(), at]);
            git(&bench.repo, &args);
            // The hand-in discovers these paths through Git. Its listed
            // spelling is also what the log records (forward slashes on Windows).
            bench.listed(&path).path
        };
        let taken = [
            cut_beside("t-26584-before", None, "main"),
            cut_beside("t-26584-jev-evidence", None, &red),
            cut_beside("t-26584-jev-green", Some("wt/t-26584-jev-green"), &head),
            cut_beside("t-26584-verify", None, &head),
        ];
        Bench::build_tree(&taken[1].join("target"));
        let dirty = cut_beside("t-26584-dirty", None, &red);
        std::fs::write(dirty.join("notes.md"), "not committed\n").expect("a note");
        let locked = cut_beside("t-26584-locked", None, &red);
        git(
            &bench.repo,
            &["worktree", "lock", &locked.to_string_lossy()],
        );
        let stray = cut_beside("t-26584-stray", None, &red);
        commit(&stray, "stray.rs");
        let busy = cut_beside("t-26584-busy", None, &red);
        let another_task = cut_beside("t-26585-red", None, &red);

        let one = orchestration::HandedIn {
            task: "t-26584".to_string(),
            worker: "w-26585".to_string(),
            agent: "claude".to_string(),
            run: "run-1".to_string(),
            checkout: own.to_string_lossy().into_owned(),
            head: head.clone(),
        };
        // Nobody left but a process standing in `busy` — a person's shell.
        let settled = |_: &str| true;
        let terminals = |_: &Path| 0_usize;
        let clear = |_: &Path| Clearance::Clear;
        let processes = |path: &Path| Some(usize::from(same_worktree_path(path, &busy)));
        let recheck = Recheck {
            settled: &settled,
            terminals: &terminals,
            keeping: &clear,
            processes: Some(&processes as ProcessCount<'_>),
            everyone: None,
            free_bytes: None,
        };
        let mut found: Vec<PathBuf> = Vec::new();
        if let Besides::Found(road, beside) =
            besides_of_hand_in(bench.orchestrator(), bench.listed(&own), &one)
        {
            for known in &beside {
                found.push(known.path.clone());
                let candidate = finished(&known.path, &one.worker);
                settle(&candidate, &road, &here, &recheck, 1_000);
            }
        }

        let name = |path: &PathBuf| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let mut standing: Vec<String> = taken
            .iter()
            .chain([&dirty, &locked, &stray, &busy])
            .filter(|path| path.exists())
            .map(name)
            .collect();
        standing.sort();
        assert_eq!(
            standing,
            [
                "t-26584-busy",
                "t-26584-dirty",
                "t-26584-locked",
                "t-26584-stray"
            ],
            "after the hand-in, the checkouts its head holds still stand beside the worker's \
             own — one task, several rows"
        );
        let mut found: Vec<String> = found.iter().map(name).collect();
        found.sort();
        assert_eq!(
            found,
            [
                "t-26584-before",
                "t-26584-busy",
                "t-26584-dirty",
                "t-26584-jev-evidence",
                "t-26584-jev-green",
                "t-26584-locked",
                "t-26584-stray",
                "t-26584-verify"
            ],
            "the task's checkouts beside the worker's own are not what was found"
        );
        assert!(another_task.exists(), "another task's checkout was taken");
        assert!(own.exists(), "the worker's own checkout was taken");
        for commit in [&red, &head] {
            rev(&bench.repo, &format!("{commit}^{{commit}}"));
        }
        rev(&bench.repo, "refs/heads/wt/t-26584-jev-green");

        let said = std::fs::read_to_string(data_root.path().join("window-errors.log"))
            .expect("the window log");
        for path in &taken {
            let removal = format!("worktree removed [handed-in] {}", path.display());
            assert!(
                said.lines().any(|line| line.contains(&removal)),
                "no removal line names the road beside the hand-in for {}:\n{said}",
                path.display()
            );
        }
        for (path, because) in [
            (&dirty, "uncommitted"),
            (&locked, "locked"),
            (&stray, "not in the head t-26584 handed in"),
            (&busy, "process(es) on this machine stand in it"),
        ] {
            assert!(
                said.lines()
                    .any(|line| line.contains(&*path.to_string_lossy()) && line.contains(because)),
                "{} stays without saying `{because}`:\n{said}",
                path.display()
            );
        }
        let mut marks = judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for path in [&dirty, &locked, &stray, &busy] {
            marks.remove(&*path.to_string_lossy());
        }
    }

    /// A hand-in whose worker moved its branch away from the head, or whose own
    /// checkout is detached, names no branch to hold what goes: nothing beside
    /// it is judged at all (t-45336).
    #[test]
    fn a_hand_in_no_branch_holds_takes_nothing_back() {
        let bench = Bench::open();
        let own = bench.cut("t-30001");
        std::fs::write(own.join("work.rs"), "// work\n").expect("work");
        git(&own, &["add", "work.rs"]);
        git(&own, &["commit", "-q", "-m", "work"]);
        let head = rev(&own, "HEAD");
        let beside = bench.worktrees.join("t-30001-verify");
        git(
            &bench.repo,
            &[
                "worktree",
                "add",
                "--detach",
                &beside.to_string_lossy(),
                &head,
            ],
        );
        // The branch moved back: it no longer holds the head handed in.
        git(&own, &["reset", "-q", "--keep", "main"]);
        let one = orchestration::HandedIn {
            task: "t-30001".to_string(),
            worker: "w-30001".to_string(),
            agent: "claude".to_string(),
            run: "run-1".to_string(),
            checkout: own.to_string_lossy().into_owned(),
            head,
        };
        assert!(
            matches!(
                besides_of_hand_in(bench.orchestrator(), bench.listed(&own), &one),
                Besides::NoHolder
            ),
            "a head no branch holds was measured against"
        );
        assert!(beside.exists());
    }

    /// A checkout is a task's by the names this window gives them, read by the
    /// one reader of those names: its folder, and its branch under the window's
    /// prefix when it is on one (t-45336). A coordinator's `coord-…` checkout,
    /// a longer id, a branch outside the prefix and a branch of another task
    /// are nobody's intermediate.
    #[test]
    fn a_checkout_is_a_tasks_by_its_folder_and_its_branch_under_the_prefix() {
        let at = |path: &str, branch: Option<&str>| Worktree {
            path: PathBuf::from(path),
            head: Some("abc1234".to_string()),
            branch: branch.map(str::to_string),
            is_main: false,
            bare: false,
            detached: branch.is_none(),
            locked: false,
            prunable: false,
        };
        for (path, branch, carries) in [
            ("/w/t-26584-before", None, true),
            ("/w/t-26584-jev-green", Some("wt/t-26584-jev-green"), true),
            ("/w/t-26584", Some("wt/t-26584/fix-the-gate"), true),
            ("/w/t-265840-red", None, false),
            ("/w/coord-t26584", None, false),
            ("/w/t-26584-gate", Some("coord/t-26584-gate"), false),
            ("/w/t-26584-mixed", Some("wt/t-26585-mixed"), false),
        ] {
            assert_eq!(
                carries_task(&at(path, branch), "t-26584", "wt"),
                carries,
                "{path} on {branch:?}"
            );
        }
    }

    /// A commit of the worker's own in `path`, under a file named for it.
    fn commit_work(path: &Path, file: &str) {
        std::fs::write(path.join(file), "work\n").expect("work");
        git(path, &["add", file]);
        git(path, &["commit", "-q", "-m", file]);
    }

    /// A checkout cut the way a worker's is, whose worker then moved to a
    /// branch it named itself (t-76403): the cut's recorded base belongs to
    /// the branch it left, so the branch it is on has none.
    fn cut_then_switched(bench: &Bench, name: &str, to: &str) -> PathBuf {
        let path = bench.cut(name);
        git(&path, &["checkout", "-q", "-b", to]);
        path
    }

    /// The person's report (10-26 17:37, 「왜 또 다 회수 안해?」): two workers
    /// moved from the branch the window cut for them to one of their own, and
    /// when their heads landed in `main` the window kept both for good —
    /// "nothing recorded what wt/t-76028 was cut from". The head being in the
    /// repository's main line is the answer that record was for.
    #[test]
    fn a_worker_who_changed_branches_is_reclaimed_once_the_default_branch_holds_its_head() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();
        let path = cut_then_switched(&bench, "w-76032", "wt/t-76028");
        commit_work(&path, "landed.txt");

        let Verdict::Keep { because, .. } = judge(&orchestrator, &bench.listed(&path)) else {
            panic!("a branch main has not taken, with no recorded base, was reclaimable");
        };
        assert!(because.contains("cut from"), "{because}");

        git(&bench.repo, &["merge", "-q", "--ff-only", "wt/t-76028"]);
        let Verdict::Reclaim {
            branch,
            base,
            elsewhere,
            ..
        } = judge(&orchestrator, &bench.listed(&path))
        else {
            panic!("a branch main holds in full stayed kept for want of a recorded base");
        };
        assert_eq!(
            (branch.as_str(), base.as_str(), elsewhere),
            ("wt/t-76028", "main", false)
        );
    }

    /// The same for a base that was recorded and has since been deleted, and
    /// for a branch ahead of the base it was cut from while the default branch
    /// holds it all: neither keeps what the repository's main line has taken.
    #[test]
    fn a_recorded_base_that_is_gone_or_behind_does_not_keep_what_the_default_branch_holds() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();

        let gone = bench.cut("gone");
        git(
            &gone,
            &["config", "--local", "branch.wt/gone.base", "no-such-base"],
        );
        let ahead = bench.cut("ahead");
        git(&ahead, &["checkout", "-q", "-b", "wt/ahead-of-base"]);
        git(
            &ahead,
            &[
                "config",
                "--local",
                "branch.wt/ahead-of-base.base",
                "wt/gone",
            ],
        );
        commit_work(&ahead, "ahead.txt");

        assert!(
            matches!(
                judge(&orchestrator, &bench.listed(&gone)),
                Verdict::Reclaim { ref base, .. } if base == "main"
            ),
            "a branch with nothing past main stayed for want of its deleted base"
        );
        assert!(
            matches!(
                judge(&orchestrator, &bench.listed(&ahead)),
                Verdict::Keep { .. }
            ),
            "a branch main has not taken was reclaimable"
        );
        git(
            &bench.repo,
            &["merge", "-q", "--ff-only", "wt/ahead-of-base"],
        );
        assert!(
            matches!(
                judge(&orchestrator, &bench.listed(&ahead)),
                Verdict::Reclaim { ref base, .. } if base == "main"
            ),
            "the default branch holds every commit and the checkout stayed"
        );
    }

    /// When the remote's own word is the default branch (`origin/HEAD`), a
    /// head pushed to it is landed even before the local `main` is moved, and
    /// a head only the local `main` has is landed too.
    #[test]
    fn a_head_the_remote_default_branch_holds_is_landed() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();
        let remote = bench.repo.with_file_name("origin.git");
        git(
            bench.repo.parent().expect("parent"),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                &remote.to_string_lossy(),
            ],
        );
        git(
            &bench.repo,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );
        git(&bench.repo, &["push", "-q", "origin", "main"]);
        git(&bench.repo, &["remote", "set-head", "origin", "main"]);

        let path = cut_then_switched(&bench, "w-76159", "wt/t-76028-r3");
        commit_work(&path, "pushed.txt");
        assert!(
            matches!(
                judge(&orchestrator, &bench.listed(&path)),
                Verdict::Keep { .. }
            ),
            "work no default branch holds was reclaimable"
        );
        git(&path, &["push", "-q", "origin", "HEAD:main"]);
        let Verdict::Reclaim { base, .. } = judge(&orchestrator, &bench.listed(&path)) else {
            panic!("a head origin/main holds was kept");
        };
        assert_eq!(base, "origin/main");
    }

    /// A bare repository made as the `origin` of `bench.repo`, with `main`
    /// pushed and `origin/HEAD` left unset (t-76403 r2).
    fn origin_without_head(bench: &Bench, name: &str) -> PathBuf {
        let remote = bench.repo.with_file_name(name);
        git(
            bench.repo.parent().expect("parent"),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                &remote.to_string_lossy(),
            ],
        );
        git(
            &bench.repo,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );
        git(&bench.repo, &["push", "-q", "origin", "main"]);
        remote
    }

    /// With `origin/HEAD` never set, a head pushed to `origin/main` is landed
    /// although the local `main` stands behind it, and so is one in a
    /// repository that has no local `main` at all (t-76403 r2).
    #[test]
    fn a_head_origin_main_holds_is_landed_without_an_origin_head() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();
        let remote = origin_without_head(&bench, "origin-no-head.git");

        let path = cut_then_switched(&bench, "w-76648", "wt/t-76403-r2");
        commit_work(&path, "pushed.txt");
        assert!(
            matches!(
                judge(&orchestrator, &bench.listed(&path)),
                Verdict::Keep { .. }
            ),
            "work no default branch holds was reclaimable"
        );
        git(&path, &["push", "-q", "origin", "HEAD:main"]);
        let Verdict::Reclaim { base, .. } = judge(&orchestrator, &bench.listed(&path)) else {
            panic!("a head origin/main holds was kept while origin/HEAD is unset");
        };
        assert_eq!(base, "origin/main");

        // The local main line is gone altogether; the remote-tracking branch
        // is all the repository has.
        git(&bench.repo, &["checkout", "-q", "--detach"]);
        git(&bench.repo, &["branch", "-D", "main"]);
        let Verdict::Reclaim { base, .. } = judge(&orchestrator, &bench.listed(&path)) else {
            panic!("a repository with only origin/main kept a head origin/main holds");
        };
        assert_eq!(base, "origin/main");
        let _ = std::fs::remove_dir_all(&remote);
    }

    /// A worker's branch called like a default branch is not its own proof
    /// through the remote either: pushing `develop` to `origin/develop` makes
    /// the remote-tracking branch hold exactly what the worker holds.
    #[test]
    fn a_branch_pushed_under_a_default_name_is_not_its_own_proof() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();
        let remote = origin_without_head(&bench, "origin-self.git");

        let path = cut_then_switched(&bench, "w-76648-self", "develop");
        commit_work(&path, "own.txt");
        // No `main` is left to be asked first: only the worker's `develop`
        // and the `origin/develop` it pushed.
        git(&bench.repo, &["checkout", "-q", "--detach"]);
        git(&bench.repo, &["branch", "-m", "main", "side"]);
        git(
            &bench.repo,
            &["update-ref", "-d", "refs/remotes/origin/main"],
        );
        git(&path, &["push", "-q", "origin", "develop"]);
        let Verdict::Keep { .. } = judge(&orchestrator, &bench.listed(&path)) else {
            panic!("a branch pushed under a default name vouched for itself");
        };
        let _ = std::fs::remove_dir_all(&remote);
    }

    /// The default branch holding the head is an answer to where the commits
    /// belong, not to who stands in the directory (t-76403 r2): the machine's
    /// processes are asked on this road as on the one resting on other
    /// branches, and a process, or a table nobody could read, keeps the
    /// checkout — with no recorded cut, with a cut whose base is gone, and
    /// with a base the branch has moved ahead of.
    #[test]
    fn a_checkout_the_default_branch_holds_waits_for_every_process_to_leave() {
        let _window = crate::orchestration::tests::the_window();
        let bench = Bench::open();
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let road = Road::Report(bench.orchestrator());

        let unrecorded = cut_then_switched(&bench, "w-76648-a", "wt/t-76403-unrecorded");
        commit_work(&unrecorded, "unrecorded.txt");
        git(
            &bench.repo,
            &["merge", "-q", "--ff-only", "wt/t-76403-unrecorded"],
        );

        let gone = bench.cut("w-76648-b");
        git(
            &gone,
            &[
                "config",
                "--local",
                "branch.wt/w-76648-b.base",
                "no-such-base",
            ],
        );
        commit_work(&gone, "gone.txt");
        git(&bench.repo, &["merge", "-q", "--ff-only", "wt/w-76648-b"]);

        let behind_base = bench.cut("w-76648-c-base");
        let behind = bench.cut("w-76648-c");
        git(
            &behind,
            &[
                "config",
                "--local",
                "branch.wt/w-76648-c.base",
                "wt/w-76648-c-base",
            ],
        );
        commit_work(&behind, "behind.txt");
        git(&bench.repo, &["merge", "-q", "--ff-only", "wt/w-76648-c"]);
        assert!(behind_base.is_dir());

        for (path, scene) in [
            (&unrecorded, "no recorded cut"),
            (&gone, "a recorded cut that is gone"),
            (&behind, "a recorded cut the branch is ahead of"),
        ] {
            let worker = format!("w-{scene}");
            let kept = |recheck: &Recheck<'_>, why: &str| {
                let settled = settle(&finished(path, &worker), &road, &here, recheck, 1_000);
                assert!(
                    matches!(settled, Settled::Kept),
                    "{scene}, {why}: the checkout went"
                );
                assert!(path.is_dir(), "{scene}, {why}: the directory is gone");
            };
            kept(&nobody_left_and(&|_| Some(1)), "a process stands in it");
            kept(&nobody_left_and(&|_| None), "the table cannot be read");
            kept(&nobody_left(), "no table was asked at all");
            let settled = settle(
                &finished(path, &worker),
                &road,
                &here,
                &nobody_left_and(&|_| Some(0)),
                1_000,
            );
            let Settled::Reclaimed {
                base, elsewhere, ..
            } = settled
            else {
                panic!("{scene}: with nobody standing in it the checkout was kept");
            };
            assert_eq!(base, "main", "{scene}");
            assert!(
                !elsewhere,
                "{scene}: it was said to have no commit of its own"
            );
            assert!(!path.exists(), "{scene}: the checkout survived its reclaim");
        }
    }

    /// Reclaiming wider must not reach work nobody else holds: every other
    /// question is asked first and still keeps its checkout, and a commit
    /// the default branch lacks keeps it too — however the branch got its
    /// name, including one that is itself called like a default branch.
    #[test]
    fn a_landed_head_never_reclaims_what_another_question_keeps() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();
        bench.ignore_on_main("target/\n.env\n");

        let dirty = cut_then_switched(&bench, "dirty", "wt/dirty-swapped");
        std::fs::write(dirty.join("notes.txt"), "half a thought\n").expect("write");
        let locked = cut_then_switched(&bench, "locked", "wt/locked-swapped");
        git(
            &bench.repo,
            &["worktree", "lock", &locked.to_string_lossy()],
        );
        let kept_file = cut_then_switched(&bench, "kept-file", "wt/kept-file-swapped");
        std::fs::write(kept_file.join(".env"), "SECRET=1\n").expect("write");
        let one_more = cut_then_switched(&bench, "one-more", "wt/one-more-swapped");
        commit_work(&one_more, "not-in-main.txt");
        let halfway = cut_then_switched(&bench, "halfway", "wt/halfway-swapped");
        commit_work(&halfway, "conflict.txt");
        git(
            &halfway,
            &["checkout", "-q", "-b", "wt/halfway-theirs", "main"],
        );
        std::fs::write(halfway.join("conflict.txt"), "theirs\n").expect("write");
        git(&halfway, &["add", "conflict.txt"]);
        git(&halfway, &["commit", "-q", "-m", "theirs"]);
        git(&halfway, &["checkout", "-q", "wt/halfway-swapped"]);
        let merge = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(&halfway)
            .args(["merge", "wt/halfway-theirs"])
            .output()
            .expect("git merge");
        assert!(!merge.status.success(), "the merge should have conflicted");
        for (name, path) in [
            ("dirty", &dirty),
            ("locked", &locked),
            ("an ignored file", &kept_file),
            ("a commit main lacks", &one_more),
            ("a merge halfway", &halfway),
        ] {
            assert!(
                matches!(
                    judge(&orchestrator, &bench.listed(path)),
                    Verdict::Keep { .. }
                ),
                "{name}: a checkout with work only here was reclaimable"
            );
        }
        git(
            &bench.repo,
            &["worktree", "unlock", &locked.to_string_lossy()],
        );

        // A branch that is itself named like a default branch is not
        // "landed in itself": with the repository's own main line renamed
        // away, `develop` is the first default-looking name that resolves.
        let own = cut_then_switched(&bench, "own-name", "develop");
        commit_work(&own, "develop.txt");
        git(&bench.repo, &["branch", "-m", "main", "home"]);
        assert!(
            matches!(
                judge(&orchestrator, &bench.listed(&own)),
                Verdict::Keep { .. }
            ),
            "a branch was called landed in a default branch that is the branch itself"
        );
    }

    /// The checkout's shared `node_modules` link into the primary (t-76403
    /// ③): `unshare_project_directories` takes it off before a removal, and
    /// the judgment before it must not read it as a file that exists only here.
    #[test]
    fn a_shared_directory_link_does_not_keep_a_landed_checkout() {
        let bench = Bench::open();
        bench.ignore_on_main("node_modules\ntarget/\n");
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let (projects, prefs) = bench.window();
        let road = Road::Sweep { projects, prefs };
        std::fs::create_dir_all(bench.repo.join("node_modules/pkg")).expect("primary modules");
        let path = bench.cut("t-76403");
        #[cfg(unix)]
        std::os::unix::fs::symlink(bench.repo.join("node_modules"), path.join("node_modules"))
            .expect("link");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(
            bench.repo.join("node_modules"),
            path.join("node_modules"),
        )
        .expect("link");

        assert!(
            matches!(
                judge(&bench.orchestrator(), &bench.listed(&path)),
                Verdict::Reclaim { .. }
            ),
            "a link into the primary's node_modules kept a landed checkout"
        );
        let settled = settle(
            &finished(&path, "w-76403"),
            &road,
            &here,
            &nobody_left(),
            1_000,
        );
        assert!(matches!(settled, Settled::Reclaimed { .. }));
        assert!(!path.exists(), "the checkout survived its reclaim");
        assert!(
            bench.repo.join("node_modules/pkg").is_dir(),
            "the removal followed the link into the primary's modules"
        );
    }

    /// Back-date what a branch move writes, so a move shows as a change
    /// whatever the filesystem's clock grain is.
    fn age_branch_files(bench: &Bench) {
        let then = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        for file in [
            "packed-refs",
            "refs/heads",
            "refs/remotes/origin",
            "logs/HEAD",
        ] {
            let at = bench.repo.join(".git").join(file);
            if let Ok(open) = std::fs::File::open(&at) {
                open.set_modified(then).expect("back-date");
            }
        }
    }

    /// A refusal as the sweep holds it, stamped the way `judge_and_act`
    /// stamps one.
    fn refusal_stamped_at(path: &Path, at_ms: i64) -> Judged {
        Judged {
            reason: ReclaimReason {
                kind: ReclaimKind::Kept,
                detail: "kept".to_string(),
                checked_ms: None,
                waits: None,
            },
            at_ms,
            base_mark: Some(base_mark(path)),
            quiet: None,
        }
    }

    /// The landing the person waited fifteen minutes for (t-76403 ②): a kept
    /// checkout is asked again on the beat after the repository's main line
    /// moves, and not before — a worker committing on its own branch, or a
    /// refusal nobody stamped, waits out the clock as ever.
    #[test]
    fn a_kept_checkout_is_due_when_the_default_branch_moves_and_not_before() {
        let bench = Bench::open();
        let path = bench.cut("t-76403");
        age_branch_files(&bench);
        let now = 60_000;
        let held = refusal_stamped_at(&path, 0);
        let asked = |held: &Judged, at: i64| due_again(Some(held), at, || base_mark(&path));

        assert!(
            !asked(&held, now),
            "a refusal one minute old was asked again"
        );
        assert!(
            asked(&held, RE_JUDGE_AFTER),
            "a refusal as old as the wait was not asked again"
        );
        assert!(
            due_again(None, 0, || base_mark(&path)),
            "a new candidate waits"
        );

        // A worker's own commit moves its own branch, one directory down.
        commit_work(&path, "own.txt");
        assert!(
            !asked(&held, now),
            "a worker's own commit made every kept checkout due"
        );

        // Landing it in main moves the branch directory.
        git(&bench.repo, &["merge", "-q", "--ff-only", "wt/t-76403"]);
        assert!(
            asked(&held, now),
            "main moving did not make a kept checkout due"
        );

        // A stamp nobody took waits out the clock, and so does an unreadable
        // directory: neither is a move.
        let unstamped = Judged {
            base_mark: None,
            ..refusal_stamped_at(&path, 0)
        };
        assert!(!asked(&unstamped, now));
        let nowhere = tempfile::tempdir().expect("not a checkout");
        assert!(base_mark(nowhere.path()).is_empty());
        let unreadable = Judged {
            base_mark: Some(base_mark(nowhere.path())),
            ..refusal_stamped_at(&path, 0)
        };
        assert!(!due_again(Some(&unreadable), now, || {
            base_mark(nowhere.path())
        }));
    }

    /// A push or a fetch moves the remote-tracking branch, which is where
    /// the default branch of a repository with a remote lands.
    #[test]
    fn a_push_to_the_remote_default_branch_makes_a_kept_checkout_due() {
        let bench = Bench::open();
        let remote = bench.repo.with_file_name("origin.git");
        git(
            bench.repo.parent().expect("parent"),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                &remote.to_string_lossy(),
            ],
        );
        git(
            &bench.repo,
            &["remote", "add", "origin", &remote.to_string_lossy()],
        );
        git(&bench.repo, &["push", "-q", "origin", "main"]);
        let path = bench.cut("t-76403");
        age_branch_files(&bench);
        let held = refusal_stamped_at(&path, 0);
        assert!(!due_again(Some(&held), 1_000, || base_mark(&path)));

        git(
            &bench.repo,
            &["commit", "--allow-empty", "-q", "-m", "next"],
        );
        git(&bench.repo, &["push", "-q", "origin", "main"]);
        assert!(
            due_again(Some(&held), 1_000, || base_mark(&path)),
            "a push to origin/main did not make a kept checkout due"
        );
    }

    /// The stamp is written by the judging road: a refusal it leaves carries
    /// the stamp read before git was asked, and a checkout reclaimed leaves
    /// none behind.
    #[test]
    fn a_kept_checkout_carries_the_stamp_read_before_the_judgment() {
        let bench = Bench::open();
        let path = bench.cut("t-76403");
        commit_work(&path, "own.txt");
        age_branch_files(&bench);
        let before = base_mark(&path);
        let candidate = finished(&path, "w-76403");
        let data_root = tempfile::tempdir().expect("data root");
        let here = Where {
            active: bench.repo.clone(),
            data_root: data_root.path().to_path_buf(),
        };
        let (projects, prefs) = bench.window();
        let settled = settle(
            &candidate,
            &Road::Sweep { projects, prefs },
            &here,
            &nobody_left(),
            5_000,
        );
        assert!(matches!(settled, Settled::Kept), "unlanded work was taken");
        stamp_kept(&candidate.path, before.clone(), 5_000);
        let marks = judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let held = marks.get(&candidate.path).expect("the refusal");
        assert_eq!(held.base_mark.as_ref(), Some(&before));
        drop(marks);
        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&candidate.path);
    }
}
