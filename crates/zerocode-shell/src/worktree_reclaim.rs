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
//! refusal is [`RE_JUDGE_AFTER`] old, and never for more than
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
//!   * one whose branch carries commits its base has not taken, or whose base
//!     is not recorded, or is recorded and cannot be resolved.
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    kept: String,
    at_ms: i64,
}

/// How a refusal reads. A checkout with work in it is KEPT — the directory
/// stays because of what is in it. A checkout this window is standing in, a
/// repository's own checkout, or a path no catalogued project lists was never
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
    },
    /// It stays, and this is why. Prose, because the noes are many and each
    /// one is a different fact; they are written to the window log, where the
    /// existing automatic cleanup already writes exactly this news.
    Keep(String),
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
    let occupied = crate::occupied_checkouts(&app.state::<AppState>());
    let standing: Vec<orchestration::SettledCheckout> = standing
        .into_iter()
        .filter(|one| crate::occupancy_of(&occupied, Path::new(&one.path)) == 0)
        .collect();
    if standing.is_empty() {
        return;
    }

    // Everything below this point runs only on a pass that has something to
    // do. Which candidates are due is answered from memory alone, so an idle
    // beat — every beat, most of the time — never reads the settings
    // document or the project catalog, let alone starts git.
    let due: Vec<orchestration::SettledCheckout> = {
        let mut marks = judged().lock().unwrap_or_else(|held| held.into_inner());
        // Forget every checkout that stopped being a candidate, so a window
        // running for days holds one entry per standing orphan and not one
        // per worker it has ever settled. A checkout seated again by a
        // returning coordinator leaves this way too, and is judged afresh
        // when it next settles.
        let live: std::collections::HashSet<&str> =
            standing.iter().map(|one| one.path.as_str()).collect();
        marks.retain(|path, _| live.contains(path.as_str()));
        standing
            .iter()
            .filter(|one| {
                marks
                    .get(&one.path)
                    .is_none_or(|held| now_ms.saturating_sub(held.at_ms) >= RE_JUDGE_AFTER)
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
}

impl Road {
    /// The name the window log's removal line carries, so the line says which
    /// door took the directory (`note_worktree_removal`).
    fn source(&self) -> &'static str {
        match self {
            Self::Sweep { .. } => "reclaim",
            Self::Report(_) => "completed-worker",
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
}

/// What one judgment came to, for the road that asked.
enum Settled {
    /// It went, and these are the two names that made it safe.
    Reclaimed { branch: String, base: String },
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
    // a thread of its own and the checkout waits for the next beat.
    let keeping = |path: &Path| before_cleanup(path, Mode::Wait);
    let recheck = Recheck {
        settled: &orchestration::checkout_is_settled,
        terminals: &terminals,
        keeping: &keeping,
    };
    match settle(candidate, road, here, &recheck, now_ms) {
        Settled::Reclaimed { branch, base } => {
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
    let owner = match road {
        Road::Sweep { projects, prefs } => owning_repository(projects, prefs, &path),
        Road::Report(cutter) => listed_by(cutter, &path),
    };
    let (orchestrator, known) = match owner {
        Ok(found) => found,
        Err(refusal) => {
            orchestration::checkout_examined(
                &candidate.worker,
                Examined::Unknown {
                    why: refusal.clone(),
                },
                now_ms,
            );
            remember(candidate, Standing::NotOurs, &refusal, data_root, now_ms);
            return Settled::Kept;
        }
    };
    let (verdict, examined) = look(&orchestrator, &known, &here.active);
    let standing = if matches!(examined, Examined::Shared) {
        Standing::NotOurs
    } else {
        Standing::Kept
    };
    // What the look found reaches the hold the ledger opened at the worker's
    // death, whatever the verdict below does with the directory: the same
    // reading answers both, so the two can never disagree.
    orchestration::checkout_examined(&candidate.worker, examined, now_ms);
    let (branch, base, takes) = match verdict {
        Verdict::Keep(because) => {
            remember(candidate, standing, &because, data_root, now_ms);
            return Settled::Kept;
        }
        Verdict::Reclaim {
            branch,
            base,
            takes,
        } => (branch, base, takes),
    };
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
    match reclaim(&orchestrator, &held, data_root) {
        Ok(()) => {
            crate::note_worktree_removal(
                data_root,
                road.source(),
                &path,
                &format!(
                    "worker {} left it; {branch} is clean and fully in {base}; it took {}",
                    candidate.worker,
                    if takes.is_empty() {
                        "no ignored path with it".to_string()
                    } else {
                        takes.join(", ")
                    }
                ),
            );
            Settled::Reclaimed { branch, base }
        }
        Err(refusal) => {
            remember(candidate, Standing::Kept, &refusal, data_root, now_ms);
            Settled::Kept
        }
    }
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
    let repeat = {
        let mut marks = judged().lock().unwrap_or_else(|held| held.into_inner());
        let repeat = marks
            .get(&candidate.path)
            .is_some_and(|held| held.kept == because);
        marks.insert(
            candidate.path.clone(),
            Judged {
                kept: because.to_string(),
                at_ms: now_ms,
            },
        );
        repeat
    };
    if repeat {
        return;
    }
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
    (
        Verdict::Keep(because.clone()),
        Examined::Unknown { why: because },
    )
}

/// A refusal because the checkout is not a cut of its own — the window is
/// standing in it, it is the repository's own, or it is not ours: whatever a
/// worker left there is already in a tree the coordinator holds.
fn shared(because: String) -> (Verdict, Examined) {
    (Verdict::Keep(because), Examined::Shared)
}

/// The verdict alone, for the tests that only ever asked for it — the beat
/// itself reads both halves through `look`.
#[cfg(test)]
fn judge(orchestrator: &Orchestrator, known: &Worktree, active: &Path) -> Verdict {
    look(orchestrator, known, active).0
}

/// One reading of a settled checkout, answering two questions from it: what
/// the reclaimer may do with the directory, and what a dead worker left in
/// it. Kept as one function on purpose — a second reading for the second
/// question would be a second chance to disagree.
fn look(orchestrator: &Orchestrator, known: &Worktree, active: &Path) -> (Verdict, Examined) {
    let path = known.path.as_path();
    if same_worktree_path(active, path) {
        return shared("the window is standing in it".to_string());
    }
    if known.is_main {
        return shared("it is the repository's own checkout".to_string());
    }
    if known.locked {
        return unknown("git reports that the worktree is locked".to_string());
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
                    Verdict::Keep(format!(
                        "{} uncommitted change(s) live only here: {}",
                        changes.len(),
                        changes.join(", ")
                    )),
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
                return unknown(format!(
                    "{} ignored file(s) exist only here and would go with it: {}",
                    files.len(),
                    files.join(", ")
                ));
            }
            dirs
        }
        Err(error) => return unknown(format!("git could not read its status: {error}")),
    };
    let Some(branch) = known.branch.as_deref() else {
        return unknown(
            "its HEAD is detached, so removal would leave no branch holding its commits"
                .to_string(),
        );
    };
    let Some(base) = orchestrator.creation_bases().get(branch).cloned() else {
        return unknown(format!(
            "nothing recorded what {branch} was cut from, so where its commits belong is unknown"
        ));
    };
    match orchestrator.commits_beyond(path, &base, branch) {
        Ok(Some(0)) => (
            Verdict::Reclaim {
                branch: branch.to_string(),
                base,
                takes,
            },
            Examined::Landed,
        ),
        Ok(Some(outstanding)) => (
            Verdict::Keep(format!(
                "{outstanding} commit(s) on {branch} are not in {base} yet"
            )),
            Examined::Unlanded {
                branch: branch.to_string(),
                base,
                commits: outstanding,
            },
        ),
        Ok(None) => unknown(format!(
            "{base} cannot be resolved here, so whether {branch} has landed is unknown"
        )),
        Err(error) => unknown(format!(
            "git could not compare {branch} with {base}: {error}"
        )),
    }
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
) -> Result<(), String> {
    let path = held.path();
    crate::unshare_project_directories(orchestrator.repo_root(), path);
    crate::remove_automatic_worktree(orchestrator, held)?;
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
            let repo = temp.path().join("repo");
            std::fs::create_dir_all(&repo).expect("repo");
            git(&repo, &["init", "-q", "-b", "main"]);
            git(&repo, &["config", "user.name", "ZeroCode Test"]);
            git(&repo, &["config", "user.email", "test@zerocode"]);
            git(&repo, &["commit", "--allow-empty", "-q", "-m", "first"]);
            let worktrees = temp.path().join("managed");
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
        }
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
            matches!(
                judge(&orchestrator, &known, &bench.repo),
                Verdict::Reclaim { .. }
            ),
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
        let elsewhere = bench.repo.clone();

        let landed = bench.cut("landed");
        assert_eq!(
            look(&orchestrator, &bench.listed(&landed), &elsewhere).1,
            Examined::Landed
        );

        let unlanded = bench.cut("unlanded-look");
        std::fs::write(unlanded.join("work.txt"), "real work\n").expect("write");
        git(&unlanded, &["add", "work.txt"]);
        git(&unlanded, &["commit", "-q", "-m", "real work"]);
        assert!(
            matches!(
                look(&orchestrator, &bench.listed(&unlanded), &elsewhere).1,
                Examined::Unlanded { commits: 1, .. }
            ),
            "{:?}",
            look(&orchestrator, &bench.listed(&unlanded), &elsewhere).1
        );

        let dirty = bench.cut("dirty-look");
        std::fs::write(dirty.join("notes.txt"), "half a thought\n").expect("write");
        assert_eq!(
            look(&orchestrator, &bench.listed(&dirty), &elsewhere).1,
            Examined::Uncommitted { changes: 1 }
        );

        // Standing in it makes it the coordinator's own tree, not a cut to
        // harvest — and the verdict says so in the same breath.
        let (verdict, examined) = look(&orchestrator, &bench.listed(&landed), &landed);
        assert_eq!(examined, Examined::Shared);
        assert!(matches!(verdict, Verdict::Keep(ref because) if because.contains("standing")));
    }

    #[test]
    fn a_clean_landed_checkout_is_reclaimable_and_says_why() {
        let bench = Bench::open();
        let orchestrator = bench.orchestrator();
        let path = bench.cut("t-1140");
        let elsewhere = bench.repo.clone();

        let Verdict::Reclaim {
            branch,
            base,
            takes,
        } = judge(&orchestrator, &bench.listed(&path), &elsewhere)
        else {
            panic!("a clean, landed worker checkout was not reclaimable");
        };
        assert_eq!(
            (branch.as_str(), base.as_str()),
            ("wt/t-1140", "main"),
            "the reclaim does not carry the two names its sentence is made of"
        );
        assert!(takes.is_empty(), "{takes:?}");
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

        let Verdict::Reclaim { takes, .. } =
            judge(&orchestrator, &bench.listed(&path), &bench.repo)
        else {
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
        let Verdict::Keep(because) = judge(&orchestrator, &bench.listed(&path), &bench.repo) else {
            panic!("a checkout holding an ignored file was reclaimable");
        };
        assert!(
            because.contains(".env"),
            "the refusal does not name the file that would have gone: {because}"
        );
        std::fs::remove_file(path.join(".env")).expect("take the secret back out");
        assert!(matches!(
            judge(&orchestrator, &bench.listed(&path), &bench.repo),
            Verdict::Reclaim { .. }
        ));
        let data_root = tempfile::tempdir().expect("data root");
        let held = CheckoutHeld::take(&path).expect("no other road holds it");
        reclaim(&orchestrator, &held, data_root.path()).expect("reclaim");
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
        let Verdict::Keep(because) = judge(&orchestrator, &bench.listed(&dirty), &bench.repo)
        else {
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
        let Verdict::Keep(because) = judge(&orchestrator, &bench.listed(&unlanded), &bench.repo)
        else {
            panic!("a checkout holding unlanded commits was reclaimable");
        };
        assert!(
            because.contains("1 commit(s)") && because.contains("wt/unlanded"),
            "the refusal does not say what has not landed: {because}"
        );

        // A base nobody wrote down. The question has no answer here, and no
        // answer keeps the directory.
        let unrecorded = bench.cut("unrecorded");
        git(
            &unrecorded,
            &["config", "--local", "--unset", "branch.wt/unrecorded.base"],
        );
        let Verdict::Keep(because) = judge(&orchestrator, &bench.listed(&unrecorded), &bench.repo)
        else {
            panic!("a checkout with no recorded base was reclaimable");
        };
        assert!(because.contains("cut from"), "{because}");

        // A base that was written down and then deleted.
        let orphaned = bench.cut("orphaned");
        git(
            &orphaned,
            &[
                "config",
                "--local",
                "branch.wt/orphaned.base",
                "a-branch-that-went-away",
            ],
        );
        let Verdict::Keep(because) = judge(&orchestrator, &bench.listed(&orphaned), &bench.repo)
        else {
            panic!("a checkout whose base is gone was reclaimable");
        };
        assert!(because.contains("cannot be resolved"), "{because}");

        // Locked, which is somebody saying so out loud.
        let locked = bench.cut("locked");
        git(
            &bench.repo,
            &["worktree", "lock", &locked.to_string_lossy()],
        );
        let Verdict::Keep(because) = judge(&orchestrator, &bench.listed(&locked), &bench.repo)
        else {
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
        let Verdict::Keep(because) = judge(&orchestrator, &bench.listed(&halfway), &bench.repo)
        else {
            panic!("a checkout mid-merge was reclaimable");
        };
        assert!(because.contains("in progress"), "{because}");

        // And the window's own workspace, whatever else is true of it.
        let landed = bench.cut("landed");
        assert!(
            matches!(
                judge(&orchestrator, &bench.listed(&landed), &landed),
                Verdict::Keep(_)
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

        let Verdict::Keep(because) = judge(&orchestrator, &bench.listed(&foreign), &bench.repo)
        else {
            panic!("a hand-made checkout was reclaimable");
        };
        assert!(because.contains("ownership is `external`"), "{because}");
        assert!(foreign.is_dir(), "the hand-made checkout disappeared");
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
        };
        let log = data_root.path().join("window-errors.log");

        remember(
            &candidate,
            Standing::NotOurs,
            "the window is standing in it",
            data_root.path(),
            1_000,
        );
        let said = std::fs::read_to_string(&log).expect("the window log");
        assert!(
            said.contains("left at /repo/main-checkout is not this window's to reclaim: the window is standing in it")
                && !said.contains("was kept"),
            "a checkout that was never a candidate read as a leftover:\n{said}"
        );

        judged()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&candidate.path);
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
            let Settled::Reclaimed { branch: took, base } = settled else {
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
}
