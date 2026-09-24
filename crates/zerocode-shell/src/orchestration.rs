//! The window's half of the orchestration ledger.
//!
//! [`zerocode_core::orchestration`] decides everything; the decisions live in
//! ONE place now — the [`RuntimeActor`] that owns the durable authority — and
//! this file does the three things the actor cannot do by itself: speak the
//! shim's answer shape, carry out the [`Effect`] a decision asked for against
//! the real host, and ring the window's own bell when the ledger moves.
//!
//! **Why it shares a road with the teams shim.** The ledger's verbs and tmux's
//! verbs arrive the same way and want the same answer shape: an argv from a
//! blocked CLI, and a stdout/stderr/exit it can print. So there is one door
//! ([`crate::agent_teams::run`]) that reads the verb and hands it to whichever
//! dialect owns it. The two vocabularies do not overlap, and
//! [`speaks_here`] settles the question against the one table that lists them.
//!
//! **Why the effects are not carried out in the actor.** A `worker-start`
//! becomes the same [`Effect::Split`] a `split-window` becomes, and the pane
//! is cut by the same host either way — but a spawn can take longer than the
//! gap between two verbs, and an actor that forked processes would stand
//! between every other pane and its ledger. So the actor RESERVES the effect
//! in its journal, this file walks it against the host through the split
//! lane, and the outcome is settled back through the actor's own door —
//! prepared, walked, settled, with a durable record at every step a crash
//! could interrupt.
//!
//! [`RuntimeActor`]: zerocode_orchestrator::runtime_actor::RuntimeActor

pub(crate) mod coordinator_handover;
pub(crate) mod desk;
pub(crate) mod restart_census;
mod stall_cause;
mod step_effort;
mod summon_choice;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use zerocode_core::agent::{Injected, agent_spec, prompt_injection};
use zerocode_core::agent_teams::{Effect, capture_reply};
use zerocode_core::launch::{LaunchOverride, join_command_line, launch_plan};
use zerocode_core::orchestration::{
    Decided, Launcher, Ledger, LedgerProjectionV1, RESEAT_GRACE_MS, RebuildError, Restarted, VERBS,
    WAIT_SECONDS, Worker, WorkerState,
};
use zerocode_orchestrator::effect_journal::{
    BeginEffect, EffectPermit, EffectRequest, EffectSettlement, HostEffectFailure, HostEffectKind,
};
use zerocode_orchestrator::runtime_actor::{FederationAnswer, FederationCall};
use zerocode_orchestrator::runtime_actor::{
    MAX_RUNTIME_MAILBOX, PaneTable, PlanCommand, RuntimeActor, RuntimeBoot, RuntimeError,
    RuntimeImage,
};
use zerocode_orchestrator::workflow_store::WorkflowStore;

use crate::agent_teams::Host;
use crate::durable_split::{SplitEffectResult, SplitLeaseDraft, digest};
use crate::seat_triage::Triage;

/* ---- the ledger OUTLIVES the window ----------------------------------
 *
 * It used to die with the process, and that was the difference between an
 * orchestration and a session: the person had codex driving claude across
 * three panes, the window restarted, and the run, its tasks, its dispatches
 * and its mail were simply not there any more — a resumed coordinator asking
 * for any of them got `stale or unauthorized agent team`. Reported as the
 * thing it is: "아까 codex를 수동으로 내가 해서 오케스트레이션을 claude
 * 구현시켜서 돌앗짜나".
 *
 * The design already expected this. Lifecycle authority sits on the dispatch
 * and not on the worker precisely because "a terminal handle is routing
 * metadata that a restart replaces" — so a run survives a restart by
 * definition. The file this paragraph used to introduce has become the
 * store: the actor below owns it, and this window's copy of the rows lives
 * on the actor's thread rather than in a static anybody can lock. */

/// The overrides the actor's launcher reads at plan time.
///
/// A cell beside the actor because the actor's launcher is fixed at boot
/// while the person's launch settings are not: [`run`] writes the overrides
/// it was handed into the cell, and the launcher reads the cell at plan
/// time. The cell is its own lock, taken INSIDE the team-table lock by the
/// planning thread and never the other way around.
type LiveOverrides = Arc<Mutex<Vec<(String, LaunchOverride)>>>;

/// The one cached image for a runtime revision.
struct CachedLedger {
    revision: u64,
    ledger: Result<Arc<Ledger>, RebuildError>,
    report: Option<Arc<serde_json::Value>>,
}

/// Rebuilding an image is shared by every read path in this window. The cache
/// lives beside the actor rather than globally: a test or a restarted window
/// can have the same revision number while holding a different authority.
#[derive(Default)]
struct LedgerCache {
    current: Option<CachedLedger>,
}

impl LedgerCache {
    fn ledger(
        &mut self,
        revision: u64,
        projection: &LedgerProjectionV1,
    ) -> Result<Arc<Ledger>, RebuildError> {
        if let Some(current) = self
            .current
            .as_ref()
            .filter(|current| current.revision == revision)
        {
            return current.ledger.clone();
        }

        let ledger = Ledger::rebuild(projection.clone()).map(Arc::new);
        if let Ok(ledger) = &ledger {
            let workers = ledger
                .runs()
                .iter()
                .flat_map(|run| &run.workers)
                .filter(|worker| worker.state == WorkerState::Active)
                .count();
            let dispatches = ledger
                .runs()
                .iter()
                .map(|run| run.dispatches.len())
                .sum::<usize>();
            crate::crash::note_ledger(revision, workers as u64, dispatches as u64);
        }
        self.current = Some(CachedLedger {
            revision,
            ledger: ledger.clone(),
            report: None,
        });
        ledger
    }

    fn report(&mut self, revision: u64, ledger: &Ledger) -> Arc<serde_json::Value> {
        if let Some(report) = self
            .current
            .as_ref()
            .filter(|current| current.revision == revision)
            .and_then(|current| current.report.as_ref())
        {
            return Arc::clone(report);
        }

        let report = Arc::new(runtime_report_value(revision, ledger));
        if let Some(current) = self
            .current
            .as_mut()
            .filter(|current| current.revision == revision)
        {
            current.report = Some(Arc::clone(&report));
        }
        report
    }
}

type LiveLedgerCache = Arc<Mutex<LedgerCache>>;

/// The pane incarnation one worker claimed on the last liveness probe.
///
/// A worker can be reseated without changing its id. A missing observation
/// from the old seat must not help the new seat reach the confirmation
/// threshold, so all three routing facts belong to the streak even though
/// only the worker id and first-missing stamp cross into the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PaneIncarnation {
    team: String,
    pane: String,
    term: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MissingPaneStreak {
    incarnation: PaneIncarnation,
    since_ms: i64,
    confirmations: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownPane {
    incarnation: PaneIncarnation,
    /// The pane actor observed while the team table still proved ownership.
    /// When present, it fences a recycled terminal number after that table is
    /// gone. Some agents have no actor identity; their cached term remains a
    /// weak probe suitable for reporting only, never automatic adoption.
    actor: Option<String>,
}

/// The two level-triggered facts one beat hands to the ledger.
///
/// `missing` stays present after confirmation so a refused actor write is
/// retried on the next beat; the ledger owns durable event de-duplication.
/// `seen` deliberately names every live pane observed on this beat, so a
/// restarted window can clear a missing stamp written by its predecessor.
#[derive(Debug, Default, PartialEq, Eq)]
struct PaneReconciliation {
    missing: Vec<(String, i64)>,
    seen: Vec<String>,
}

#[derive(Default)]
struct PaneReconciler {
    missing: std::collections::HashMap<String, MissingPaneStreak>,
    /// Last machine-addressable incarnation for each live ledger worker.
    /// Leader teardown drops its whole team table while child terminals may
    /// remain alive; retaining the term lets later beats ask the host instead
    /// of turning an absent routing row into a fictional death.
    known: std::collections::HashMap<String, KnownPane>,
}

/// The one runtime this window drives: the actor, the launcher's overrides
/// cell beside it, and the revision-keyed image cache every reader shares.
/// Named fields rather than a tuple, so a road reads `held.actor` and not a
/// position it has to look up.
struct RuntimeSeat {
    actor: RuntimeActor,
    overrides: LiveOverrides,
    cache: LiveLedgerCache,
    board: std::sync::RwLock<Arc<BoardLedgerSnapshot>>,
    pane_reconciler: Mutex<PaneReconciler>,
    /// The same gauges the actor's launcher reads, for the beat's half of
    /// the quota-wall witness — read outside the actor, so the sweep asks
    /// the cache rather than the ledger for a number.
    usage: UsageSource,
    /// The silences this window put to Jev and the answers still waiting for
    /// what followed them (t-4538) — shared with the question asked off the
    /// beat, which writes its answer down when it arrives.
    stalls: Arc<Mutex<stall_cause::StallBook>>,
    /// The between-turn effort moves this window has judged, typed and not
    /// yet graded (t-5637) — shared with the question asked off the beat.
    moves: Arc<Mutex<step_effort::MoveBook>>,
}

type LiveRuntime = Arc<RuntimeSeat>;

/// The one runtime this window drives, and the overrides cell beside it.
///
/// A `static` for the same reason the team table is one: a run belongs to no
/// workspace and outlives no window. Set at boot by [`open`], and left empty
/// in a window whose boot could not stand the authority up — every road asks
/// [`runtime`] and answers the boot's own sentence when there is nothing
/// behind it.
///
/// An `Arc` in a mutex rather than a `OnceLock`: production installs exactly
/// one and never looks back, and the tests — which share this process — get
/// to stand up a runtime of their own and put the old one away. The actor
/// inside is never handed out by value, so the single-owner discipline holds
/// wherever the handle travels.
fn runtime_cell() -> &'static Mutex<Option<LiveRuntime>> {
    static RUNTIME: OnceLock<Mutex<Option<LiveRuntime>>> = OnceLock::new();
    RUNTIME.get_or_init(|| Mutex::new(None))
}

/// The runtime, if this window has one.
fn runtime() -> Option<LiveRuntime> {
    runtime_cell()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clone()
}

/// Read one runtime image through the cache belonging to the same runtime.
/// `runtime()` has already cloned the handle and released its cell lock before
/// callers reach here, so this lock serializes only the short cache lookup or
/// the one rebuild for a new revision.
fn cached_ledger(held: &LiveRuntime, image: &RuntimeImage) -> Result<Arc<Ledger>, RebuildError> {
    held.cache
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .ledger(image.revision(), image.projection())
}

/// Reuse the report tree that was built for the cached ledger revision.
fn cached_report(held: &LiveRuntime, revision: u64, ledger: &Ledger) -> Arc<serde_json::Value> {
    held.cache
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .report(revision, ledger)
}

/// The active assignment carried by one authenticated pane before a lifecycle
/// verb mutates it. Captured before `worker_done`, whose successful arrival
/// clears `Worker::dispatch` by design.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct SeatAssignment {
    pub(crate) run_id: String,
    pub(crate) worker_id: String,
    pub(crate) task_id: String,
    pub(crate) dispatch_id: String,
    pub(crate) checkout: Option<String>,
    pub(crate) agent: String,
    pub(crate) auto_release: bool,
}

pub(crate) fn seat_assignment(team: &str, pane: &str) -> Option<SeatAssignment> {
    let held = runtime()?;
    let image = held.actor.view().ok()?;
    let ledger = cached_ledger(&held, &image).ok()?;
    for run in ledger.runs() {
        if let Some(worker) = run.worker_in_pane(team, pane)
            && let Some(dispatch_id) = worker.dispatch.as_deref()
            && let Some(dispatch) = run.dispatch(dispatch_id)
        {
            return Some(SeatAssignment {
                run_id: run.id.clone(),
                worker_id: worker.id.clone(),
                task_id: dispatch.task.clone(),
                dispatch_id: dispatch.id.clone(),
                checkout: worker.checkout.clone(),
                agent: worker.agent.clone(),
                auto_release: worker.state == WorkerState::Active && !worker.taken_over,
            });
        }
    }
    None
}

/// Which Run a coordinator seat's next verb lands in.
pub(crate) fn bound_run(team: &str, pane: &str, actor: Option<&str>) -> Option<String> {
    let held = runtime()?;
    let image = held.actor.view().ok()?;
    let ledger = cached_ledger(&held, &image).ok()?;
    actor
        .and_then(|actor| ledger.bound_run(actor))
        .or_else(|| ledger.bound_run(&format!("{team}/{pane}")))
        .map(str::to_string)
}

/// Restore every sleeping worker belonging to the run bound to this
/// coordinator leader, one at a time.
///
/// The renderer supplies only a terminal number to the native trigger. By the
/// time this function runs the backend has resolved that term to the current
/// team's leader and derived `actor` from the provider session; neither the
/// run nor a checkout path crosses the renderer boundary.
pub(crate) fn reseat_sleeping(
    host: &dyn Host,
    overrides: Vec<(String, LaunchOverride)>,
    leader_term: u32,
    actor: Option<&str>,
) -> usize {
    static RESTORE_LINE: Mutex<()> = Mutex::new(());
    let _line = RESTORE_LINE.lock().unwrap_or_else(|held| held.into_inner());
    if unavailable().is_some() {
        return 0;
    }
    let Some(held) = runtime() else { return 0 };
    *held
        .overrides
        .lock()
        .unwrap_or_else(|held| held.into_inner()) = overrides;
    let coordinator = {
        let teams = crate::agent_teams::teams();
        teams.iter().find_map(|(id, team)| {
            (team.leader_term == leader_term).then(|| (id.clone(), team.leader_pane.clone()))
        })
    };
    let Some((team, pane)) = coordinator else {
        return 0;
    };
    let image = match held.actor.view() {
        Ok(image) => image,
        Err(_) => return 0,
    };
    let ledger = match cached_ledger(&held, &image) {
        Ok(ledger) => ledger,
        Err(_) => return 0,
    };
    let seat = format!("{team}/{pane}");
    let Some(run_id) = actor
        .and_then(|actor| ledger.bound_run(actor))
        .or_else(|| ledger.bound_run(&seat))
        .map(str::to_string)
    else {
        return 0;
    };
    drop(ledger);
    /* The coordinator is BACK, in this pane. A restart or a leader exit
     * vacated the run's seat; the window proving which leader pane returned
     * for which run is what fills it again — and only an empty chair is
     * taken, so a second copy of the same conversation restored beside the
     * first reads its own pane inbox until somebody types `run-takeover`
     * (t-2512). Asked before the walk below so the sleepers it seats are
     * adopted under the generation that seated them. */
    let Ok((moved, _)) = held.actor.coordinator_returned(
        &run_id,
        &team,
        &pane,
        actor.map(str::to_string),
        crate::now_epoch_ms(),
    ) else {
        return 0;
    };
    rang(moved);
    let image = match held.actor.view() {
        Ok(image) => image,
        Err(_) => return 0,
    };
    let ledger = match cached_ledger(&held, &image) {
        Ok(ledger) => ledger,
        Err(_) => return 0,
    };
    // A binding identifies the run; only its live seat may restore or end
    // its sleepers. A refused return must not reach either transition.
    let Some(run) = ledger
        .run(&run_id)
        .filter(|run| run.seat_is_coordinator(&seat) == Some(true))
    else {
        return 0;
    };
    // Sleeping rows, and orphans the window has CONFIRMED have no pane. Not
    // every orphan: a leader's exit leaves its children running in panes
    // that are still there, and cutting a second pane for an agent already
    // working is how one session becomes two. The proof of absence is the
    // reconciler's — five beats without the pane, written on the row by the
    // ledger as `pane_missing_since_ms` — and it is read here, at the
    // moment of the decision, off the ledger as it stands.
    let mut sleeping: Vec<(i64, String, String, Option<String>, bool)> = run
        .workers
        .iter()
        .filter(|worker| match worker.state {
            WorkerState::Sleeping => true,
            WorkerState::Orphaned => worker.pane_missing_since_ms.is_some(),
            _ => false,
        })
        .map(|worker| {
            (
                worker.started_ms,
                worker.id.clone(),
                worker.agent.clone(),
                worker.checkout.clone(),
                worker.taken_over,
            )
        })
        .collect();
    sleeping.sort_by_key(|(started, ..)| *started);
    drop(ledger);
    let installed: std::collections::HashSet<String> = crate::detected_agents(false)
        .into_iter()
        .filter(|agent| agent.installed)
        .map(|agent| agent.id.to_string())
        .collect();
    let mut restored = 0;
    for (_, worker, agent, checkout, taken_over) in sleeping {
        /* Three roads end here without a pane being cut, each with its own
         * sentence for the task's record. An orphan whose pane the window
         * proved gone and whose row never learned a checkout has nowhere to
         * be seated — that is the row a leader's exit used to abandon on the
         * spot, and it is retired now, after the proof, not before. A
         * person's pane is never reopened by the ledger (the core writes
         * that one down as abandoned rather than stopped). */
        /* A person's pane is not cut again by the ledger — and not ended
         * here either (t-3058). The window restores the person's own tabs,
         * and that restored tab seats the worker as its witness
         * (`pane_resumed`); a tab that never comes back is the grace's to
         * end (`expire_sleepers`), with the dispatch id a replacement needs. */
        if taken_over {
            continue;
        }
        let permanent = match &checkout {
            None => Some(
                "its pane is gone and no checkout was ever reported for it — nowhere to \
                 seat it again"
                    .to_string(),
            ),
            Some(checkout) if !Path::new(checkout).is_dir() => Some(format!(
                "the checkout this worker sat in is gone ({checkout})"
            )),
            Some(_) if !installed.contains(&agent) => {
                Some(format!("{agent} is not installed on this machine"))
            }
            Some(_) => None,
        };
        if let Some(reason) = permanent {
            if held
                .actor
                .finish_sleeping_reseat(&worker, reason, crate::now_epoch_ms())
                .is_ok()
            {
                rang(true);
            }
            continue;
        }
        // The same words a resumed pane's witness road carries (t-3058):
        // the seat sentence, where the checkout stands by git's word, and
        // the commands the restart cut under its pane (t-6428 ⑤).
        let cut = BLACKBOX
            .get()
            .map(|root| restart_census::take_cut(root, &worker))
            .unwrap_or_default();
        let nudge = crate::restart_nudge_runtime::resume_nudge(
            true,
            true,
            checkout
                .as_deref()
                .and_then(|checkout| {
                    crate::restart_nudge_runtime::worktree_state(
                        Path::new(checkout),
                        u64::try_from(crate::now_epoch_ms() / 1_000).unwrap_or_default(),
                    )
                })
                .as_ref(),
            &cut,
        );
        let decided = match held.actor.prepare_worker_reseat(
            &run_id,
            &worker,
            &team,
            &pane,
            leader_term,
            &nudge,
        ) {
            Ok(Ok(decided)) => *decided,
            Ok(Err(_)) | Err(_) => continue,
        };
        let now_ms = crate::now_epoch_ms();
        let answered = carried(host, &held.actor, decided, &team, &pane, "", now_ms);
        if answered.exit_code == 0 {
            restored += 1;
        }
    }
    restored
}

/// Renderer-safe view of current orchestration state. Message bodies, task
/// specs, capabilities and receipts deliberately stay behind the native
/// boundary; this report exists for status/ownership observability, not audit
/// transcript export.
/// The agent the ledger last seated in a checkout, whatever became of it.
///
/// Asked by the window when a workspace comes back with nothing stored for it.
/// A worker's tab is the ledger's to persist, never the pane layout's
/// (`persistPaneLayouts` skips `ledgerManaged`), so a checkout cut for a Codex
/// worker restores as an EMPTY workspace — and an empty workspace opened the
/// default agent, which is how a Codex worktree came back wearing Claude after
/// a restart. The ledger still knows who was there; `released` counts, because
/// the restart is exactly what released it. A sleeping OR orphaned seat wins
/// over all history because both reserve the checkout against a generic
/// launch: a sleeper will be restored, while an orphan may still be running
/// there and must not be given a second owner. Among seats with the same
/// reservation state, the newest wins when a checkout was handed on.
#[derive(Clone, serde::Serialize)]
pub(crate) struct LastAgentInCheckout {
    agent: String,
    /// The established wire spelling for "reserved by the ledger". It also
    /// covers every still-summoned worker without a seat; renderer callers
    /// withhold a generic launch while the ledger still owns that checkout.
    sleeping: bool,
    /// A current seat, read on the existing asynchronous restore road.
    term: Option<u32>,
}

pub(crate) fn last_agent_in_checkout(checkout: &str) -> Option<LastAgentInCheckout> {
    let wanted = checkout.trim_end_matches('/');
    let held = runtime()?;
    let image = held.actor.view().ok()?;
    let ledger = cached_ledger(&held, &image).ok()?;
    let teams = crate::agent_teams::teams();
    let seats = index_team_seats(&teams);
    drop(teams);
    ledger
        .runs()
        .iter()
        .flat_map(|run| run.workers.iter().map(move |worker| (run, worker)))
        .filter(|(_, worker)| {
            worker
                .checkout
                .as_deref()
                .map(|at| at.trim_end_matches('/'))
                == Some(wanted)
        })
        .max_by_key(|(_, worker)| {
            (
                matches!(worker.state, WorkerState::Sleeping | WorkerState::Orphaned),
                worker.state.still_summoned(),
                worker.started_ms,
            )
        })
        .map(|(run, worker)| {
            let term = seats
                .get(&worker.team)
                .and_then(|team| team.get(&worker.pane))
                .copied()
                .filter(|_| {
                    worker.state.still_summoned()
                        && run
                            .worker_in_pane(&worker.team, &worker.pane)
                            .is_some_and(|current| current.id == worker.id)
                });
            LastAgentInCheckout {
                agent: worker.agent.clone(),
                sleeping: matches!(worker.state, WorkerState::Sleeping | WorkerState::Orphaned)
                    || (worker.state.still_summoned() && term.is_none()),
                term,
            }
        })
}

/// A checkout the ledger is finished with, and whose worker it was.
///
/// The sibling of [`LastAgentInCheckout`] asked from the other end: that one
/// answers "who was here" for a checkout somebody is opening, and this one
/// answers "is anybody still coming back" for every checkout at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettledCheckout {
    /// The path as the ledger holds it, with a trailing separator shed so two
    /// spellings of one checkout are one entry. Everything downstream still
    /// resolves it through git's own worktree list.
    pub(crate) path: String,
    /// The last worker to sit in it — the name a reclaim gets reported under,
    /// because "reclaimed /wt/t-1140" without it makes a person go and look
    /// up whose it was.
    pub(crate) worker: String,
    pub(crate) agent: String,
}

/// Every checkout the ledger named and has now let go of.
///
/// A checkout is settled when at least one worker row names it and NOT ONE of
/// them is still summoned. [`WorkerState::still_summoned`] is the widest of
/// the four readings on purpose and that is exactly why it is the one asked
/// here: `Released` is the single word that ends the relationship, and every
/// other state — a sleeper waiting for its coordinator to come back and seat
/// it, a release nobody confirmed, a terminal whose fate is unknown — is a
/// worker somebody may still be owed. A narrower reading would hand a
/// reclaim the checkout a sleeper is reserved against, which is the checkout
/// `reseat_sleeping` is about to walk back into.
///
/// The row's own state, not `Run::seen_state`: the pane check makes a
/// reading LIVELIER than the record, and every direction this answer can be
/// wrong in has to point at keeping the directory.
///
/// Empty for a window with no durable runtime, which is the same answer it
/// gives everywhere else — a window that cannot read the ledger knows of no
/// finished worker, and must therefore reclaim nothing.
/// Tell the ledger what the reclaimer found in a dead worker's checkout.
///
/// The reclaimer already reads every settled checkout on the beat; this is
/// the one line that lets what it read reach the hold the ledger opened at
/// the worker's death (`Ledger::checkout_examined`). Rings the window when
/// the ledger moved — a task freed or a coordinator told is board news — and
/// says nothing when the runtime is down: the reclaimer's own verdict still
/// stands, and the hold is looked at again on a later beat.
pub(crate) fn checkout_examined(
    worker: &str,
    examined: zerocode_core::orchestration::Examined,
    now_ms: i64,
) {
    if let Some(held) = runtime()
        && let Ok((moved, _)) = held
            .actor
            .checkout_examined(worker.to_string(), examined, now_ms)
    {
        rang(moved);
    }
}

/// Mail a window observation to a ledger address (t-2733): CI moved on the
/// review a checkout is on, told to `@worktree:<path>` as one `status` line
/// from the ledger itself. Rings the window when a letter was filed; silent
/// when the runtime is down or nobody is seated there — the panel already
/// shows the change, and an agent that is not there has nobody to tell.
pub(crate) fn post_observation_once(to: &str, body: &str, receipt: &str, now_ms: i64) -> bool {
    let Some(held) = runtime() else {
        return false;
    };
    match held.actor.observation_once(
        to.to_string(),
        body.to_string(),
        Some(receipt.to_string()),
        now_ms,
    ) {
        Ok((moved, _)) => {
            rang(moved);
            moved
        }
        Err(_) => {
            note_ledger_unwritten("a checks observation");
            false
        }
    }
}

pub(crate) fn settled_checkouts() -> Vec<SettledCheckout> {
    let Some(held) = runtime() else {
        return Vec::new();
    };
    let Ok(image) = held.actor.view() else {
        return Vec::new();
    };
    let Ok(ledger) = cached_ledger(&held, &image) else {
        return Vec::new();
    };
    drop(image);
    let mut summoned: std::collections::HashSet<&str> = std::collections::HashSet::new();
    // Keyed by path, holding the newest settled worker seen for it — a
    // checkout handed from one worker to the next reports the last one.
    let mut settled: std::collections::HashMap<&str, (i64, &Worker)> =
        std::collections::HashMap::new();
    for run in ledger.runs() {
        for worker in &run.workers {
            let Some(checkout) = worker.checkout.as_deref() else {
                continue;
            };
            let at = checkout.trim_end_matches('/');
            if at.is_empty() {
                continue;
            }
            if worker.state.still_summoned() {
                summoned.insert(at);
                continue;
            }
            let entry = settled.entry(at).or_insert((worker.started_ms, worker));
            if worker.started_ms >= entry.0 {
                *entry = (worker.started_ms, worker);
            }
        }
    }
    let mut answered: Vec<SettledCheckout> = settled
        .into_iter()
        .filter(|(at, _)| !summoned.contains(at))
        .map(|(at, (_, worker))| SettledCheckout {
            path: at.to_string(),
            worker: worker.id.clone(),
            agent: worker.agent.clone(),
        })
        .collect();
    // Sorted so a sweep that judges only a few per pass walks them in the
    // same order every time rather than in a hash map's.
    answered.sort_by(|left, right| left.path.cmp(&right.path));
    answered
}

/// Ask the same question again about ONE checkout, right before acting on it.
///
/// The gap between a sweep listing its candidates and reaching a removal is
/// long enough for a coordinator to come back and seat a sleeper: the answer
/// is re-read against the ledger as it stands, and the removal only follows a
/// second yes.
pub(crate) fn checkout_is_settled(checkout: &str) -> bool {
    let wanted = checkout.trim_end_matches('/');
    settled_checkouts().iter().any(|one| one.path == wanted)
}

fn runtime_report_value(revision: u64, ledger: &Ledger) -> serde_json::Value {
    let runs = ledger
        .runs()
        .iter()
        .map(|run| {
            let tasks: Vec<_> = run
                .tasks
                .iter()
                .map(|task| {
                    serde_json::json!({
                        "task_id": task.id,
                        "name": task.display_name(),
                        "status": task.status.as_str(),
                        "deps": task.deps,
                        "parent": task.parent,
                        "failures": task.failures,
                    })
                })
                .collect();
            let workers: Vec<_> = run
                .workers
                .iter()
                .map(|worker| {
                    let task = worker
                        .dispatch
                        .as_deref()
                        .and_then(|id| run.dispatch(id))
                        .map(|dispatch| dispatch.task.clone());
                    /* The name first, the key beside it. A sidebar that
                     * lists `t-91 t-92 t-93` tells a person which rows exist
                     * and nothing about what any of them is; the id stays
                     * because every verb takes it. `null` for a worker with
                     * no task and for one whose task's row a sweep has
                     * compacted — a name we no longer hold is not a name to
                     * guess at. */
                    let task_name = task
                        .as_deref()
                        .and_then(|id| run.task(id))
                        .map(|held| held.display_name().to_string());
                    serde_json::json!({
                        "worker_id": worker.id,
                        "agent": worker.agent,
                        "pane": worker.pane,
                        "state": worker.state.as_str(),
                        "dispatch_id": worker.dispatch,
                        "task_name": task_name,
                        "task_id": task,
                        "checkout": worker.checkout,
                        "model": worker.model,
                        "effort": worker.effort,
                        "providerPeer": zerocode_core::orchestration::provider_peer(
                            &worker.agent,
                            &run.id,
                            &worker.id,
                        ),
                        "taken_over": worker.taken_over,
                    })
                })
                .collect();
            let gates: Vec<_> = run
                .gates
                .iter()
                .map(|gate| {
                    serde_json::json!({
                        "gate_id": gate.id,
                        "task_id": gate.task,
                        "status": gate.status.as_str(),
                        "question": gate.question.as_str(),
                        "resolution": gate.resolution.as_str(),
                    })
                })
                .collect();
            serde_json::json!({
                "run_id": run.id,
                "name": run.name,
                "created_ms": run.created_ms,
                /* Counts and headlines for a run whose rows a sweep took, so
                 * a roster showing zeroes can say WHY they are zero. Never
                 * the rows themselves — this report is for status, and a
                 * summary is counts plus the titles a person wrote. */
                "summary": run.summary.as_ref().map(|summary| serde_json::json!({
                    "headlines": summary.headlines,
                    "tasks": summary.tasks,
                    "completed": summary.completed,
                    "failed": summary.failed,
                    "dispatches": summary.dispatches,
                    "workers": summary.workers,
                    "messages": summary.messages,
                    "gates": summary.gates,
                    "last_activity_ms": summary.last_activity_ms,
                    "compacted_ms": summary.compacted_ms,
                    "sweeps": summary.sweeps,
                })),
                "auto": run.auto.as_ref().map(|auto| serde_json::json!({
                    "agent": auto.agent,
                    "max": auto.max,
                    "armed_ms": auto.armed_ms,
                })),
                "tasks": tasks,
                "workers": workers,
                "gates": gates,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "revision": revision,
        "retention": {
            "days": ledger.retention_days(),
            "swept_at_ms": ledger.swept_at_ms(),
        },
        "runs": runs,
    })
}

/// The ledger's own word for every terminal it seated a worker in.
///
/// The board draws a card from what the AGENT last reported about itself. That
/// report is the only thing it has for a pane a person opened, and it is the
/// right answer there. It is the wrong answer for a seat this window summoned
/// and has since retired: the agent's last word was `working`, nothing will
/// ever correct it, and the board kept saying a finished worker was busy.
///
/// So the ledger gets a say. `Run::seen_state` is the same reading
/// `worker-list` prints — the record checked against the pane table, so a
/// worker whose terminal is gone reads `released` however the row was last
/// written. Only seats the ledger knows appear here; a pane it never summoned
/// is absent, and absence leaves the card's own report untouched.
///
/// Published together with the worker list on the existing background beat.
/// The renderer reads an Arc; authority waits and revision rebuilds belong
/// to [`refresh_board_ledger`], never to a board paint.
pub(crate) fn ledger_states_by_term() -> Arc<std::collections::HashMap<u32, LedgerPaneState>> {
    Arc::clone(&board_ledger_snapshot().states)
}

/// The actor facts board paint needs for one seated pane. Kept together so
/// the existing actor view and retained-worker walk answer lifecycle and hook
/// reachability in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerPaneState {
    pub(crate) ledger: String,
    pub(crate) started_ms: i64,
    pub(crate) ready_by_ms: Option<i64>,
    pub(crate) hook_unreachable_since_ms: Option<i64>,
    /// When this seat's dispatch closed, if it has: the ledger's own word that
    /// the WORK ended, stamped when it was written. An agent that speaks no
    /// hooks (zo) never tells the window it finished, and a live pty with no
    /// hook fact reads as `working` forever — so this is the one fact that
    /// can say `done` for such a pane, and it is a stored epoch, not a guess.
    pub(crate) work_ended_ms: Option<i64>,
}

/// The ledger and this window's seat index, gathered once, for one reader.
///
/// [`refresh_board_ledger`] builds both board readings from this pair, and
/// internal fresh readers such as [`ledger_agents`] share the same ordering.
/// The main-thread commands read the published board view instead.
///
/// Ask the actor for its ledger image BEFORE taking the teams lock. The actor
/// owns the ledger on its thread and may be serving an earlier plan that needs
/// this same pane table; a reader that held the table while waiting on the
/// actor would meet that plan head-on.
///
/// The lock is then held for as short a walk as can be arranged, because the
/// actor wants it to PLAN under and this runs on every board paint. The seats
/// are indexed once — O(panes) — and each worker is afterwards a lookup rather
/// than a scan of its team's panes. Written the other way round it was
/// O(workers x panes) per paint over every run this window has ever held, and
/// the window holds nine of them with dozens of workers each. The owned index
/// lets the guard go before any run's rows are walked; cloning the handful of
/// live seat names is the bounded price.
///
/// `None` is "this window has no orchestration runtime to ask", which every
/// caller reads as an empty answer — never as a fact about any worker.
fn with_ledger_seats<T>(read: impl FnOnce(&Ledger, &TeamSeatIndex) -> T) -> Option<T> {
    let held = runtime()?;
    let image = held.actor.view().ok()?;
    let ledger = cached_ledger(&held, &image).ok()?;
    let teams = crate::agent_teams::teams();
    let seats = index_team_seats(&teams);
    drop(teams);
    Some(read(&ledger, &seats))
}

/// One worker the ledger still holds, as the board draws it.
///
/// The board's THIRD source. Its first two — the panes this window opened and
/// the lanes it supervises — are both answers to "what is this window
/// holding", so a worker whose seat this window does not have (a run whose
/// coordinator restarted, a pane that closed under a worker still carrying a
/// dispatch, a summons whose terminal never came up) appeared on no surface at
/// all. The ledger is the one place that knows every worker that was summoned,
/// and its rows already carry the [`checkout`](Self::checkout) the board hangs
/// a card's workspace and project off — so this is a third source in the
/// SAME ontology, not a fourth layer.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct LedgerAgent {
    /// The run that summoned it, so a person can go and ask about it.
    pub(crate) run: String,
    pub(crate) worker: String,
    pub(crate) agent: String,
    /// The board's own state vocabulary, from the RECORDED row — see
    /// [`board_word`].
    pub(crate) state: String,
    /// And what is actually true about its terminal, in the ledger's own word
    /// (`Run::seen_state_at_seat`). The same field a pane's card carries for
    /// the same purpose: `zerocode_core::board` lets this take a claim of work
    /// away and never add one, so a row that says `active` over a terminal
    /// nobody can find does not draw as "작업 중" (t-612).
    pub(crate) ledger: String,
    /// Whether hook silence is startup, a proven channel failure, a report
    /// that arrived, or a terminal this window no longer holds.
    pub(crate) hearing: &'static str,
    /// When that verdict began, in the board's epoch-millisecond clock.
    pub(crate) hearing_at: i64,
    /// The checkout its pane sat in, or empty where the row never said. Empty
    /// is absence of the fact, never a fact of absence — the same contract
    /// `Worker::checkout` keeps.
    pub(crate) checkout: String,
    /// What it was given to do, in the coordinator's own words. Empty for a
    /// worker nobody dispatched to, and for one whose task a sweep compacted.
    pub(crate) task: String,
    /// The task's id, so a row can say which piece of work it is beside the
    /// title. Empty where there is no dispatch.
    pub(crate) task_id: String,
    /// Whether the dispatch carrying that task has CLOSED — the worker
    /// reported, and the ledger wrote the ending down. This is the worker's
    /// own claim of being done; what a coordinator made of it is `review`.
    pub(crate) reported: bool,
    pub(crate) dispatch_id: String,
    pub(crate) dispatch_started_ms: i64,
    pub(crate) retry_of: Option<String>,
    /// What a coordinator wrote about the task's outcome, beyond the worker's
    /// claim: verified, merged, deployed — or nothing yet. Never inferred
    /// from a provider's turn ending or from `reported` above.
    pub(crate) review: zerocode_core::orchestration::ReviewFacts,
    /// The seat this window holds for it, when it holds one. `Some` means a
    /// pane card already speaks for this worker and the window drops this row
    /// rather than drawing the agent twice.
    pub(crate) term: Option<u32>,
    pub(crate) at: i64,
    /// What the summons asked it to run as — the launch receipt `worker-list`
    /// prints, `None` where the agent's own default was taken. The task
    /// board's worker roster reads these and the facts below (t-6588).
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    /// The pane id inside its team (`%3`) — the half of the seat a person
    /// types `worker-read` with when this window holds no terminal for it.
    pub(crate) pane: String,
    /// Whether it asked a question nobody has answered yet
    /// (`Run::awaiting_reply`, the silence the stall sweep keeps quiet).
    pub(crate) asking: bool,
    /// The newest quota wall its current attempt met, as the ledger reads it
    /// back (`newest_wall`): the reset the provider named and until when the
    /// wall explains its silence. Standing is the reader's `now` against
    /// `stands_until_ms`.
    pub(crate) wall: Option<zerocode_core::orchestration::WallAt>,
    /// The last quiet turn its hook reported (`Worker::quiet_at`).
    pub(crate) quiet_at: Option<i64>,
    /// The reconciler's proof that its pane is gone (`Worker::pane_missing_since_ms`).
    pub(crate) pane_missing_since_ms: Option<i64>,
}

/// Volatile relations layered over the board's two permanent graph edges.
/// Bodies and task prose deliberately do not cross this wire: the canvas only
/// needs endpoints, counts and recency; detail remains in the ledger/inspector.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub(crate) struct GraphOverlaySnapshot {
    pub(crate) latest: Option<String>,
    pub(crate) mail: Vec<GraphOverlayEdge>,
    pub(crate) dependencies: Vec<GraphOverlayEdge>,
    /// Task facts survive an upstream worker's release. They are not phantom
    /// agent nodes; the inspector can still explain the declared prerequisite.
    pub(crate) task_dependencies: Vec<GraphTaskDependency>,
    pub(crate) merge: Vec<GraphMergeOverlay>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct GraphOverlayEdge {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) count: u32,
    pub(crate) unread: u32,
    pub(crate) at: i64,
    pub(crate) verb: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_message: Option<GraphMessageReference>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct GraphMessageReference {
    pub(crate) id: String,
    pub(crate) run: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) kind: String,
    pub(crate) created_ms: i64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct GraphTaskDependency {
    pub(crate) run: String,
    pub(crate) task: String,
    pub(crate) dependency: String,
    pub(crate) task_state: &'static str,
    pub(crate) dependency_state: Option<&'static str>,
    pub(crate) task_created_ms: i64,
    pub(crate) from: Option<String>,
    pub(crate) to: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(crate) struct GraphMergeOverlay {
    pub(crate) workspace: String,
    pub(crate) base: String,
    pub(crate) ahead: u32,
    pub(crate) behind: u32,
    pub(crate) landed: bool,
}

/// Project unread mail and task dependencies onto the exact card ids the
/// window already draws. One worker is either seated (`term:N`) or represented
/// by its ledger-only card (`worker:id`); never both.
pub(crate) fn graph_overlay_snapshot() -> Arc<GraphOverlaySnapshot> {
    Arc::clone(&board_ledger_snapshot().overlays)
}

fn graph_overlay_snapshot_for_seats(
    ledger: &Ledger,
    seats: &TeamSeatIndex,
) -> GraphOverlaySnapshot {
    use std::collections::{BTreeMap, HashMap, HashSet};

    let listed = ledger_agents_for_seats(ledger, seats);
    let cards: HashMap<String, String> = listed
        .iter()
        .map(|row| {
            let card = row.term.map_or_else(
                || format!("worker:{}", row.worker),
                |term| format!("term:{term}"),
            );
            (row.worker.clone(), card)
        })
        .collect();
    let projected = ledger.export();
    let mut addresses: HashMap<String, String> = cards
        .iter()
        .map(|(worker, card)| {
            (
                zerocode_core::orchestration::worker_address(worker),
                card.clone(),
            )
        })
        .collect();
    for (team, panes) in seats {
        for (pane, term) in panes {
            addresses.insert(format!("pane:{team}/{pane}"), format!("term:{term}"));
        }
    }
    for run in ledger.runs() {
        if let Some(seat) = run
            .coordinator
            .as_ref()
            .filter(|seat| seat.vacated_ms.is_none())
            && let Some(card) = addresses.get(&format!("pane:{}", seat.seat)).cloned()
        {
            addresses.insert(run.address(), card);
        }
    }
    let card_at = |address: &str| addresses.get(address).cloned();

    let pending: HashMap<(&str, &str), HashSet<&str>> = projected
        .inboxes
        .iter()
        .map(|inbox| {
            (
                (inbox.run.as_str(), inbox.address.as_str()),
                inbox.pending.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let mut mail: BTreeMap<(String, String), GraphOverlayEdge> = BTreeMap::new();
    let mut latest: Option<(i64, String)> = listed.iter().max_by_key(|row| row.at).map(|row| {
        (
            row.at,
            row.term.map_or_else(
                || format!("worker:{}", row.worker),
                |term| format!("term:{term}"),
            ),
        )
    });
    for message in &projected.messages {
        let (Some(from), Some(to)) = (card_at(&message.from), card_at(&message.to)) else {
            continue;
        };
        let unread = u32::from(
            pending
                .get(&(message.run.as_str(), message.to.as_str()))
                .is_some_and(|ids| ids.contains(message.id.as_str())),
        );
        let edge = mail
            .entry((from.clone(), to.clone()))
            .or_insert(GraphOverlayEdge {
                from,
                to: to.clone(),
                count: 0,
                unread: 0,
                at: 0,
                verb: "mail",
                last_message: None,
            });
        edge.count = edge.count.saturating_add(1);
        edge.unread = edge.unread.saturating_add(unread);
        if message.created_ms >= edge.at {
            edge.at = message.created_ms;
            edge.last_message = Some(GraphMessageReference {
                id: message.id.clone(),
                run: message.run.clone(),
                from: message.from.clone(),
                to: message.to.clone(),
                kind: message.kind.as_str().to_string(),
                created_ms: message.created_ms,
            });
        }
        if latest
            .as_ref()
            .is_none_or(|(at, _)| *at < message.created_ms)
        {
            latest = Some((message.created_ms, to));
        }
    }

    // Choose the latest attempt even if its worker has already been released.
    // Falling back to an older retained attempt would connect the wrong agent.
    let mut latest_dispatch: HashMap<(&str, &str), &zerocode_core::orchestration::DispatchRow> =
        HashMap::new();
    for dispatch in &projected.dispatches {
        let key = (dispatch.run.as_str(), dispatch.task.as_str());
        if latest_dispatch
            .get(&key)
            .is_none_or(|held| dispatch.started_ms >= held.started_ms)
        {
            latest_dispatch.insert(key, dispatch);
        }
    }
    let current_cards: HashMap<(&str, &str), &String> = listed
        .iter()
        .filter_map(|row| {
            cards
                .get(&row.worker)
                .map(|card| ((row.run.as_str(), row.dispatch_id.as_str()), card))
        })
        .collect();
    let task_card = |run: &str, task: &str| {
        latest_dispatch
            .get(&(run, task))
            .and_then(|dispatch| current_cards.get(&(run, dispatch.id.as_str())))
            .map(|card| (*card).clone())
    };
    let tasks: HashMap<(&str, &str), &zerocode_core::orchestration::TaskRow> = projected
        .tasks
        .iter()
        .map(|task| ((task.run.as_str(), task.id.as_str()), task))
        .collect();
    let mut dependencies: BTreeMap<(String, String), GraphOverlayEdge> = BTreeMap::new();
    let mut task_dependencies = Vec::new();
    for task in &projected.tasks {
        let Some(to) = task_card(&task.run, &task.id) else {
            continue;
        };
        for dependency in &task.deps {
            let from = task_card(&task.run, dependency);
            task_dependencies.push(GraphTaskDependency {
                run: task.run.clone(),
                task: task.id.clone(),
                dependency: dependency.clone(),
                task_state: task.status.as_str(),
                dependency_state: tasks
                    .get(&(task.run.as_str(), dependency.as_str()))
                    .map(|held| held.status.as_str()),
                task_created_ms: task.created_ms,
                from: from.clone(),
                to: to.clone(),
            });
            let Some(from) = from else {
                continue;
            };
            if from == to {
                continue;
            }
            let edge = dependencies
                .entry((from.clone(), to.clone()))
                .or_insert(GraphOverlayEdge {
                    from: from.clone(),
                    to: to.clone(),
                    count: 0,
                    unread: 0,
                    at: task.created_ms,
                    verb: "blocks",
                    last_message: None,
                });
            edge.count = edge.count.saturating_add(1);
        }
    }

    GraphOverlaySnapshot {
        latest: latest.map(|(_, card)| card),
        mail: mail.into_values().collect(),
        dependencies: dependencies.into_values().collect(),
        task_dependencies,
        // Git ahead/behind is workspace state, not orchestration state. The
        // renderer accepts it in this shape once the catalog carries a cached
        // reading; returning none is preferable to inventing distance here.
        merge: Vec::new(),
    }
}

/// Every worker the ledger still holds, seat or no seat.
///
/// Bounded by the ledger's OUTSTANDING rows, not its history: a released
/// worker is a summons somebody answered and is not drawn. One run in this
/// window holds sixty-five released rows beside two live ones.
pub(crate) fn ledger_agents() -> Vec<LedgerAgent> {
    with_ledger_seats(ledger_agents_for_seats).unwrap_or_default()
}

/// A small, owned board view published by the existing standing-order beat.
/// A UI read never asks the actor, rebuilds a revision or takes the team table.
#[derive(Default)]
pub(crate) struct BoardLedgerSnapshot {
    pub(crate) agents: Arc<Vec<LedgerAgent>>,
    states: Arc<std::collections::HashMap<u32, LedgerPaneState>>,
    overlays: Arc<GraphOverlaySnapshot>,
    /// The checkouts live workers hold, counted the way a `--worktree`
    /// summons counts them ([`zerocode_core::orchestration::held_checkouts`]) —
    /// what the task board's machine strip judges the disk beside (t-6588).
    pub(crate) held_checkouts: usize,
    /// The task board's coordinator desk: the runs in play, their tasks by
    /// pipeline stage (t-6588, [`desk::desk_snapshot`]).
    pub(crate) desk: Arc<desk::DeskSnapshot>,
}

pub(crate) fn board_ledger_snapshot() -> Arc<BoardLedgerSnapshot> {
    let Some(held) = runtime() else {
        return Arc::default();
    };
    Arc::clone(&held.board.read().unwrap_or_else(|held| held.into_inner()))
}

pub(crate) fn refresh_board_ledger() {
    let Some(held) = runtime() else {
        return;
    };
    let Some(next) = with_ledger_seats(|ledger, seats| BoardLedgerSnapshot {
        agents: Arc::new(ledger_agents_for_seats(ledger, seats)),
        states: Arc::new(ledger_states_for_seats(ledger, seats)),
        overlays: Arc::new(graph_overlay_snapshot_for_seats(ledger, seats)),
        held_checkouts: zerocode_core::orchestration::held_checkouts(ledger).len(),
        desk: Arc::new(desk::desk_snapshot(ledger, |seat| {
            seat_is_held(seats, seat)
        })),
    }) else {
        return;
    };
    // Build, allocate and drop old rows outside the publication lock. The
    // main-thread reader holds it only long enough to clone an Arc.
    let next = Arc::new(next);
    let old = std::mem::replace(
        &mut *held.board.write().unwrap_or_else(|held| held.into_inner()),
        next,
    );
    drop(old);
}

/// Which word the BOARD's vocabulary has for a worker's recorded state.
///
/// The recorded row is the coordinator's side of the story — what it asked for
/// and has not been released from — and it is the only side that exists for a
/// worker whose pane this window never had: nothing ever reported a hook for
/// it, so there is no agent's own word to prefer. Two answers, because the
/// board only has two that matter here:
///
/// - the states that mean a terminal is running are `working`, which is the
///   coordinator's honest reading of a summons it is still waiting on;
/// - the rest — a sleeper waiting to be seated, a release nobody confirmed —
///   are `waiting`, the column for what will not move until a person moves it.
///
/// This never says `done`. Ending is the ledger's verdict, and it travels as
/// [`LedgerAgent::ledger`] where the tested rule can apply it; inventing it
/// here would be a second copy of that rule, and the copy that runs first.
fn board_word(state: zerocode_core::orchestration::WorkerState) -> &'static str {
    match state.is_live() {
        true => "working",
        false => "waiting",
    }
}

// A completed dispatch is unlinked from Worker, but remains the execution
// represented by its retained board row. Index once per run, not once per card.
fn latest_worker_dispatches(
    run: &zerocode_core::orchestration::Run,
) -> std::collections::HashMap<&str, &zerocode_core::orchestration::Dispatch> {
    let mut latest: std::collections::HashMap<&str, &zerocode_core::orchestration::Dispatch> =
        std::collections::HashMap::new();
    for dispatch in &run.dispatches {
        if latest
            .get(dispatch.worker.as_str())
            .is_none_or(|held| dispatch.started_ms >= held.started_ms)
        {
            latest.insert(dispatch.worker.as_str(), dispatch);
        }
    }
    latest
}

fn ledger_agents_for_seats(ledger: &Ledger, seats: &TeamSeatIndex) -> Vec<LedgerAgent> {
    let mut listed = Vec::new();
    for run in ledger.runs() {
        let latest_dispatches = latest_worker_dispatches(run);
        for worker in &run.workers {
            if !worker.state.still_summoned() {
                continue;
            }
            /* Pane names are reusable, so a seat only belongs to the worker
             * occupying it NOW — the same filter `worker-list` applies for
             * the same reason. A historical row that kept the pane name must
             * not borrow its replacement's terminal. */
            let term = seats
                .get(worker.team.as_str())
                .and_then(|team| team.get(worker.pane.as_str()))
                .copied()
                .filter(|_| {
                    run.worker_in_pane(&worker.team, &worker.pane)
                        .is_some_and(|current| current.id == worker.id)
                });
            let dispatch = worker
                .dispatch
                .as_ref()
                .and_then(|id| run.dispatch(id))
                .or_else(|| latest_dispatches.get(worker.id.as_str()).copied());
            let carried = dispatch.and_then(|one| run.task(&one.task));
            let task = carried
                .map(|held| held.display_name().to_string())
                .unwrap_or_default();
            let task_id = carried.map(|held| held.id.clone()).unwrap_or_default();
            let reported = dispatch.is_some_and(|one| !one.is_open());
            let dispatch_id = dispatch.map(|one| one.id.clone()).unwrap_or_default();
            let dispatch_started_ms = dispatch.map_or(0, |one| one.started_ms);
            let retry_of = dispatch.and_then(|one| one.retry_of.clone());
            let review = carried.map(|held| run.review_of(held)).unwrap_or_default();
            listed.push(LedgerAgent {
                run: run.id.clone(),
                worker: worker.id.clone(),
                agent: worker.agent.clone(),
                state: board_word(worker.state).to_string(),
                ledger: run
                    .seen_state_at_seat(worker, term.is_some())
                    .as_str()
                    .to_string(),
                hearing: if term.is_none() {
                    "gone"
                } else if worker.hook_unreachable_since_ms.is_some() {
                    "unreachable"
                } else {
                    // A takeover also retires readiness. This ledger-only row
                    // has no pane report with which to tell that from sound.
                    "pending"
                },
                hearing_at: if term.is_none() {
                    worker.started_ms
                } else {
                    worker
                        .hook_unreachable_since_ms
                        .unwrap_or(worker.started_ms)
                },
                checkout: worker.checkout.clone().unwrap_or_default(),
                task,
                task_id,
                reported,
                dispatch_id,
                dispatch_started_ms,
                retry_of,
                review,
                term,
                at: worker.started_ms,
                model: worker.model.clone(),
                effort: worker.effort.clone(),
                pane: worker.pane.clone(),
                asking: run.awaiting_reply(&worker.id),
                wall: dispatch
                    .and_then(|one| zerocode_core::orchestration::newest_wall(run, &one.id)),
                quiet_at: worker.quiet_at,
                pane_missing_since_ms: worker.pane_missing_since_ms,
            });
        }
    }
    listed
}

/// The runtime's current ledger revision, for a reader that wants to know
/// whether anything moved since it last looked — the beat, which tells the
/// window so surfaces reading ledger facts (the navigator's task titles and
/// review labels) follow the ledger rather than a boot-time snapshot.
/// `None` where there is no runtime to ask.
pub(crate) fn ledger_revision() -> Option<u64> {
    let held = runtime()?;
    held.actor.view().ok().map(|image| image.revision())
}

type TeamSeatIndex = std::collections::HashMap<String, std::collections::HashMap<String, u32>>;

/// Whether this window holds the pane a seat names (`team/pane`).
fn seat_is_held(seats: &TeamSeatIndex, seat: &str) -> bool {
    seat.split_once('/').is_some_and(|(team, pane)| {
        seats
            .get(team)
            .is_some_and(|panes| panes.contains_key(pane))
    })
}

fn index_team_seats(
    teams: &std::collections::HashMap<String, zerocode_core::agent_teams::Team>,
) -> TeamSeatIndex {
    let mut seats = TeamSeatIndex::new();
    for (id, team) in teams.iter() {
        let indexed = seats.entry(id.clone()).or_default();
        for pane in team.panes() {
            indexed.insert(pane.id.clone(), pane.term);
        }
    }
    seats
}

fn ledger_states_for_seats(
    ledger: &Ledger,
    seats: &TeamSeatIndex,
) -> std::collections::HashMap<u32, LedgerPaneState> {
    let mut found = std::collections::HashMap::new();
    for run in ledger.runs() {
        let latest_dispatches = latest_worker_dispatches(run);
        for worker in &run.workers {
            if !worker.state.may_occupy_pane() {
                continue;
            }
            let Some(term) = seats
                .get(worker.team.as_str())
                .and_then(|team| team.get(worker.pane.as_str()))
                .copied()
            else {
                continue;
            };
            // The seat's newest attempt, read off the run's own list: a
            // closed dispatch is unlinked from the worker row, so the row
            // cannot say when its work ended — the dispatch can.
            let work_ended_ms = latest_dispatches
                .get(worker.id.as_str())
                .and_then(|held| if held.is_open() { None } else { held.ended_ms });
            found.insert(
                term,
                LedgerPaneState {
                    ledger: worker.state.as_str().to_string(),
                    started_ms: worker.started_ms,
                    ready_by_ms: worker.ready_by_ms,
                    hook_unreachable_since_ms: worker.hook_unreachable_since_ms,
                    work_ended_ms,
                },
            );
        }
    }
    found
}

pub(crate) fn runtime_report() -> Result<Arc<serde_json::Value>, String> {
    let held = runtime().ok_or_else(|| "orchestration runtime is unavailable".to_string())?;
    let image = held.actor.view().map_err(|error| error.to_string())?;
    let ledger = cached_ledger(&held, &image).map_err(|error| error.to_string())?;
    /* Shared, not copied: the tree is built once per revision, and an
     * `Arc<Value>` serializes exactly as the `Value` would, so the command
     * boundary hands the same allocation out until the revision moves. */
    Ok(cached_report(&held, image.revision(), &ledger))
}

/// Put a booted runtime where every road finds it.
fn install_runtime(actor: RuntimeActor, overrides: LiveOverrides, usage: UsageSource) {
    *runtime_cell()
        .lock()
        .unwrap_or_else(|held| held.into_inner()) = Some(Arc::new(RuntimeSeat {
        actor,
        overrides,
        cache: Arc::new(Mutex::new(LedgerCache::default())),
        board: std::sync::RwLock::default(),
        pane_reconciler: Mutex::new(PaneReconciler::default()),
        usage,
        stalls: Arc::default(),
        moves: Arc::default(),
    }));
    // Boot seeds the first answer before any webview can restore worker seats.
    refresh_board_ledger();
}

/// This window's host generation, minted once at boot.
///
/// Every effect the journal reserves is bound to the epoch that reserved it,
/// so an operation recovered by a LATER window can prove the one thing that
/// is true of it — the backend that was walking it is gone — instead of
/// guessing what that backend reached.
static HOST_EPOCH: OnceLock<String> = OnceLock::new();

fn host_epoch() -> Option<&'static str> {
    HOST_EPOCH.get().map(String::as_str)
}

/// Where the window's black box lives — the data root `open` was given.
///
/// One writer, one sentence, and only for the settlements that have no
/// second chance in this window: `terminal_gone` runs once, right before
/// `forget_term` takes away the seat that names the worker, so a settlement
/// the store refuses there is a dispatch held open until the next boot with
/// no line anywhere saying why. The old road's whole-ledger save healed
/// that by accident; the actor's row-writes do not, so the black box is the
/// honest remainder.
static BLACKBOX: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Whether this process has said its goodbye's road line (t-6428). The
/// goodbye is heard up to three times on one way out — a close, the
/// embedded browser's own `app.exit(0)`, tauri's `Exit` — and the road is
/// said once; the workers are named whenever a call still finds them.
static GOODBYE_SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The authority store this window opened, for readers that only read it.
///
/// Set once, where the store is opened, and never derived a second time: a
/// reader that rebuilt `<data root>/authority/authority.sqlite` for itself
/// would be a second authority for where the store lives, and the first time
/// the vault moved the two would disagree. `None` is a window whose runtime
/// never started, which every reader answers as "there is no store here".
static AUTHORITY_STORE: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Where this window's authority store is, when it has one.
pub(crate) fn authority_store_path() -> Option<std::path::PathBuf> {
    AUTHORITY_STORE.get().cloned()
}

/// The ledger's own rows, as this window holds them.
///
/// An `Arc` clone of the image cached for the current revision — the same one
/// every board paint reads — so a caller takes the runtime's lock for the
/// clone and not for whatever it does with the rows afterwards. `None` is a
/// window with no orchestration runtime, which readers answer as an absence
/// of the SOURCE, never as a fact about any checkout.
pub(crate) fn ledger_image() -> Option<Arc<Ledger>> {
    let held = runtime()?;
    let image = held.actor.view().ok()?;
    cached_ledger(&held, &image).ok()
}

fn note_ledger_unwritten(what: &str) {
    let Some(root) = BLACKBOX.get() else { return };
    crate::note_window_event(root, &format!("orchestration: {what} was not written"));
}

/// The pointer had mail to name and did not name it, and why.
///
/// Two reasons reach here and they are the same news: mail is waiting, the one
/// mechanism that tells a pane about it without being asked did not fire, and
/// until this line existed nothing anywhere said so. Said once per pane per
/// watermark — a beat is a second long, and a line a second would bury the
/// very thing it reports — which is what the marks in [`pointed`] are for.
///
/// `check` still owns delivery either way: this is the window admitting its
/// advice did not go out, never the ledger giving up on the mail.
/// A pointer left where a working pane's own hook will collect it.
///
/// Written once per watermark rather than per beat — [`park`] answers whether
/// the shelf changed — so a long turn costs one line and not one line a
/// second. It is deliberately a different sentence from the typed road's: a
/// reader looking for why nothing was typed into a composer needs to find the
/// answer, and "nothing was typed because the agent is being told a better
/// way" is that answer.
///
/// [`park`]: crate::orchestration_pointer_mailbox::park
fn note_pointer_parked(run: &str, address: &str, term: u32) {
    let Some(root) = BLACKBOX.get() else { return };
    crate::note_window_event(
        root,
        &format!(
            "orchestration: mail waiting for {address} in {run} is parked for \
             terminal {term}'s own turn-end hook; no composer was used"
        ),
    );
}

/// A parked pointer whose hook never came for it.
///
/// The other half of [`note_pointer_parked`], and the line a reader needs when
/// the native road quietly does not work on some machine: it says the window
/// tried the road that needs no keystroke, waited, and went back to the one
/// that does. Written once per abandonment, because the pointer is taken away
/// in the same breath.
fn note_pointer_uncollected(run: &str, address: &str, term: u32) {
    let Some(root) = BLACKBOX.get() else { return };
    crate::note_window_event(
        root,
        &format!(
            "orchestration: terminal {term}'s turn-end hook never collected the \
             pointer for {address} in {run}; the composer road has it back"
        ),
    );
}

/// Mail is waiting for a pane whose own last answer was a wall, and the
/// pointer holds its line back (t-6560).
///
/// Said when the hold begins — for this mail, at this pane — and not once a
/// beat: the reader looking for why a coordinator at its limit was not told
/// about its mail needs the wall's own words and how long the window will
/// wait on them, once.
fn note_pointer_walled(
    run: &str,
    address: &str,
    term: u32,
    wall: &crate::quota_wall::PaneWall,
    now_ms: i64,
) {
    let Some(root) = BLACKBOX.get() else { return };
    let minutes =
        zerocode_core::orchestration::minutes_up(wall.stands_until_ms.saturating_sub(now_ms));
    crate::note_window_event(
        root,
        &format!(
            "orchestration: mail waiting for {address} in {run} is held at terminal \
             {term}: its last answer was its {} wall (\"{}\"), and a line typed there \
             would only meet it again — nothing is typed until it answers again, or \
             for {minutes} min at most",
            wall.cause.word(),
            wall.line.as_str()
        ),
    );
}

/// A held pointer's wall stopped standing: the pane is pointed at again, once.
fn note_pointer_wall_lifted(
    run: &str,
    address: &str,
    term: u32,
    cause: crate::quota_wall::StallCause,
) {
    let Some(root) = BLACKBOX.get() else { return };
    crate::note_window_event(
        root,
        &format!(
            "orchestration: terminal {term}'s {} wall stopped standing; the pointer \
             for {address} in {run} is offered again",
            cause.word()
        ),
    );
}

fn note_pointer_silent(run: &str, address: &str, term: u32, why: &str) {
    let Some(root) = BLACKBOX.get() else { return };
    crate::note_window_event(
        root,
        &format!(
            "orchestration: mail waiting for {address} in {run} could not be \
             pointed at terminal {term} {why}"
        ),
    );
}

/// Mail is waiting for a holder this window has no seat for at all.
///
/// The sibling of [`note_pointer_silent`] for the case that has no terminal to
/// name. The pane table is process memory: a window that restarts comes back
/// with it empty, so every address of every run it was carrying is suddenly
/// seatless — and until this line existed, the pointer's loop simply had
/// nothing to iterate and the mail piled up in a silence that no surface, no
/// log and no verb reported.
///
/// Said once per address per watermark, like every other line here: a beat is
/// a second long, and a run left seatless overnight would otherwise write the
/// same sentence thirty thousand times.
///
/// `check` still owns delivery: this mail is not lost, it is unannounced. A
/// pane that comes back and binds to the run reads all of it.
fn note_seatless_mail(run: &str, address: &str) {
    let Some(root) = BLACKBOX.get() else { return };
    crate::note_window_event(
        root,
        &format!(
            "orchestration: mail waiting for {address} in {run} has no seat in \
             this window — nothing can be pointed at it until a pane binds to \
             the run"
        ),
    );
}

/// No road would carry the advice: the native fast path is a hint that falls
/// back, and the fallback is a write at a pty. When BOTH decline there is
/// nothing left to try. The beat still retries on the next tick.
const NO_ROAD_CARRIED_IT: &str = "by any road";

/// Nothing has ever been heard from this pane, so it can never pass the
/// pointer's first door — see [`pane_turns`]. An agent whose hooks are not
/// installed reports nothing, ever.
const NOTHING_WAS_EVER_HEARD: &str = "— the window has heard nothing from this pane";

/// The pane's last turn ended under a person's hand, which makes it theirs.
/// Only their next finished turn hands it back, so unlike a running turn this
/// does not come undone on its own.
const THE_PERSON_HOLDS_IT: &str = "— a person interrupted the last turn in it";

/// The pane's last turn ended at rest, and then its agent left: the pane's own
/// shell is in front ([`Host::shell_in_front`]). Advice typed there would run
/// as a command, so nothing is typed until an agent holds the terminal again.
const NO_AGENT_IN_FRONT: &str = "— its shell is in front; the agent that ended the last turn left";

/// The pane's turn is under way, or a question of its own is parked in it —
/// [`pane_turn_began`] writes both down as running.
const A_TURN_IS_RUNNING: &str = "— a turn is under way in it";

/// Whether the window may type at this pane's composer on its own, off the
/// turn facts it measured: `Ok` for a pane whose last turn ended at rest with
/// its agent still in front, and otherwise the reason it may not.
///
/// ONE reading for the two producers that type unasked — the mail pointer
/// ([`point_at_waiting_mail`]) and the transient-error continuation
/// ([`resume_stalled_workers`]) — so a door one of them learns to respect is
/// a door both keep. Only a measured, uninterrupted end licenses a keystroke
/// (see [`pane_turns`]); a turn ended by the person's hand is theirs, a pane
/// never heard from may have no hook at all, and a shell in front would run
/// the words as a command.
fn composer_at_rest(
    host: &dyn Host,
    term: u32,
    heard: Option<PaneTurn>,
) -> Result<(), &'static str> {
    match heard {
        Some(PaneTurn::Ended { interrupted: false }) => {
            if host.shell_in_front(term) {
                Err(NO_AGENT_IN_FRONT)
            } else {
                Ok(())
            }
        }
        Some(PaneTurn::Ended { interrupted: true }) => Err(THE_PERSON_HOLDS_IT),
        Some(PaneTurn::Running) => Err(A_TURN_IS_RUNNING),
        None => Err(NOTHING_WAS_EVER_HEARD),
    }
}

/// The one pane table, as the actor borrows it: the process-global team
/// table, under its own lock, with the pane capabilities beside it.
///
/// The actor plans UNDER this lock, which is what closes the gap between a
/// capability check and the plan it authorizes — the same generation of the
/// same lock holds both. The disk is never touched inside these closures.
struct ShellPaneTable;

impl PaneTable for ShellPaneTable {
    fn incarnation(
        &self,
        team: &str,
        pane: &str,
    ) -> Option<zerocode_core::agent_teams::PaneIncarnation> {
        let teams = crate::agent_teams::teams();
        let term = teams.get(team)?.term_of(pane)?;
        let capability = crate::agent_teams::current_pane_capability(team, pane)?;
        Some(zerocode_core::agent_teams::PaneIncarnation { term, capability })
    }

    fn with_team(
        &self,
        team: &str,
        action: &mut dyn FnMut(Option<&mut zerocode_core::agent_teams::Team>),
    ) {
        let mut held = crate::agent_teams::teams();
        action(held.get_mut(team));
    }

    fn with_seat_of_term(&self, term: u32, action: &mut dyn FnMut(Option<(&str, &str)>)) {
        let held = crate::agent_teams::teams();
        let seat = held.iter().find_map(|(id, team)| {
            team.panes()
                .find(|pane| pane.term == term)
                .map(|pane| (id.clone(), pane.id.clone()))
        });
        match &seat {
            Some((team, pane)) => action(Some((team, pane))),
            None => action(None),
        }
    }

    fn with_pane_authority(
        &self,
        team: &str,
        pane: &str,
        presented: &str,
        action: &mut dyn FnMut(Option<&mut zerocode_core::agent_teams::Team>),
    ) {
        let mut held = crate::agent_teams::teams();
        action(crate::agent_teams::authorized_team_mut(
            &mut held, team, pane, presented,
        ));
    }
}

/// The launcher the actor holds: [`Catalog`], read through the overrides
/// cell so the person's launch settings reach a launcher that was moved into
/// the actor at boot.
struct LiveCatalog {
    overrides: Arc<Mutex<Vec<(String, LaunchOverride)>>>,
    /// Where the ledger's store lives — the volume a `--worktree` summons is
    /// measured against, because that is the volume an `ENOSPC` would take
    /// the ledger down on.
    ledger_dir: PathBuf,
    headroom: HeadroomSource,
    usage: UsageSource,
}

/// Where a runtime reads the provider gauges it reports to the ledger.
///
/// The product reads the window's own usage caches — the same snapshots the
/// status bar shows, filled by the window's poll and by nothing on this road.
/// Tests hand the rows over: a summons judged against whatever the
/// developer's account stood at that minute would be a summons judged by luck.
#[derive(Clone)]
enum UsageSource {
    Cached {
        local_data_root: PathBuf,
    },
    /// Rows a test handed over — behind a lock, because a wall is reached
    /// AFTER a summons, and a scenario has to be able to move the number
    /// between the two without rebooting the window.
    #[cfg(test)]
    Fixed(Arc<Mutex<Vec<(&'static str, crate::usage::ProviderUsage)>>>),
}

#[cfg(test)]
impl UsageSource {
    fn fixed(rows: Vec<(&'static str, crate::usage::ProviderUsage)>) -> Self {
        Self::Fixed(Arc::new(Mutex::new(rows)))
    }

    /// Replace every row — the provider's number moving under a live pane.
    fn replace(&self, rows: Vec<(&'static str, crate::usage::ProviderUsage)>) {
        if let Self::Fixed(held) = self {
            *held.lock().unwrap_or_else(|held| held.into_inner()) = rows;
        }
    }
}

/// The cache one gauge name reads, by the name `quota_gauge_for` answers.
///
/// A lookup and nothing else — the accessor hands back the process-global
/// cell the poll writes, loading the last snapshot off disk the first time it
/// is asked (a file read, never a request). A name outside this table is a
/// gauge this window does not keep.
fn cached_usage(
    local_data_root: &Path,
    gauge: &str,
) -> Option<&'static Mutex<Option<crate::usage::ProviderUsage>>> {
    use crate::usage_runtime as caches;
    Some(match gauge {
        "claude" => caches::claude_usage_cache(local_data_root),
        "codex" => caches::codex_usage_cache(local_data_root),
        "opencode" => caches::opencode_usage_cache(local_data_root),
        "grok" => caches::grok_usage_cache(local_data_root),
        "kimi" => caches::kimi_usage_cache(local_data_root),
        "antigravity" => caches::antigravity_usage_cache(local_data_root),
        _ => return None,
    })
}

/// One snapshot as a [`zerocode_core::orchestration::Headroom`]: the binding
/// window is the fullest of session, week and month, session first on a tie.
///
/// A snapshot with no window figures — a failed read, a signed-out account —
/// is `None`: "nobody knows" is what it says, and 0% is what it must not.
pub(crate) fn headroom_of(
    snapshot: &crate::usage::ProviderUsage,
) -> Option<zerocode_core::orchestration::Headroom> {
    use zerocode_core::orchestration::{Headroom, QuotaWindow};
    let (window, figures) = [
        (QuotaWindow::Session, snapshot.session.as_ref()),
        (QuotaWindow::Weekly, snapshot.weekly.as_ref()),
        (QuotaWindow::Monthly, snapshot.monthly.as_ref()),
    ]
    .into_iter()
    .filter_map(|(window, figures)| figures.map(|held| (window, held)))
    // `max_by_key` keeps the LAST maximum; the fold keeps the first, so a
    // session and a week both at 98% report the session.
    .fold(
        None,
        |best: Option<(QuotaWindow, &crate::usage::UsageWindow)>, next| match best {
            Some(held) if held.1.used_percent >= next.1.used_percent => Some(held),
            _ => Some(next),
        },
    )?;
    Some(Headroom {
        provider: snapshot.provider.clone(),
        used_percent: figures.used_percent,
        window,
        resets_at_ms: figures.resets_at,
        updated_at_ms: snapshot.updated_at,
        status: snapshot.status.clone(),
        failure_kind: snapshot.failure_kind,
    })
}

/// Where a runtime gets the disk headroom it reports to the ledger.
///
/// The product measures the volume holding its authority store. Tests name
/// their disk explicitly: consulting the developer's filesystem would make a
/// scenario's answer depend on what else happened to be built that day.
#[derive(Clone, Copy)]
enum HeadroomSource {
    Measured,
    #[cfg(test)]
    Fixed(u64),
}

impl Launcher for LiveCatalog {
    fn command_for(&self, agent: &str, prompt: &str, tuning: &[String]) -> Result<String, String> {
        let overrides = self
            .overrides
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone();
        Catalog::new(overrides).command_for(agent, prompt, tuning)
    }

    fn command_for_resume(
        &self,
        agent: &str,
        session: &zerocode_core::ProviderSession,
        nudge: &str,
        tuning: &[String],
    ) -> Result<String, String> {
        let overrides = self
            .overrides
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone();
        Catalog::new(overrides).command_for_resume(agent, session, nudge, tuning)
    }

    fn presence(&self) -> Option<Vec<zerocode_core::AgentPresence>> {
        machine_presence()
    }

    /// What the window last saw of the agent's binary and login (t-3996) —
    /// peeked, never probed: the actor is holding the ledger, and a witness
    /// that spawns `security` or `gh` under it would stall every verb. A
    /// stale row is refreshed on its own thread for the next reader.
    fn readiness(&self, agent: &str) -> Option<zerocode_core::readiness::AgentReadinessSnapshot> {
        crate::readiness_runtime::observe(agent)
    }

    /// One `stat`, no subprocess — the actor is holding the ledger. A path
    /// that is not a directory is a checkout that is gone; a path that is
    /// one may still be refused by the placement road for its own reasons.
    fn checkout_present(&self, path: &str) -> Option<bool> {
        Some(Path::new(path).is_dir())
    }

    fn worktree_headroom(&self) -> Option<zerocode_core::orchestration::DiskHeadroom> {
        let free_bytes = match self.headroom {
            HeadroomSource::Measured => free_bytes_at(&self.ledger_dir)?,
            #[cfg(test)]
            HeadroomSource::Fixed(free_bytes) => free_bytes,
        };
        Some(zerocode_core::orchestration::DiskHeadroom {
            free_bytes,
            at: self.ledger_dir.display().to_string(),
        })
    }

    /// The gauge, read off the cache the status bar reads — and nothing
    /// fetched: no scan, no subprocess, no request. `MIN_REFETCH` is the
    /// poll's rule and is honoured here by construction, since this road
    /// never asks a provider at all. The cache cell is a leaf lock held for
    /// one clone, inside the actor that is already holding the ledger.
    fn provider_headroom(
        &self,
        agent: &str,
        model: Option<&str>,
    ) -> Option<zerocode_core::orchestration::Headroom> {
        usage_headroom(&self.usage, agent, model)
    }
}

/// The gauge `agent` (launched with `model`) draws on, read off `usage` —
/// the one reading both the launcher's probe and the beat's witness make,
/// so the summons and the wall are judged against the same number.
fn usage_headroom(
    usage: &UsageSource,
    agent: &str,
    model: Option<&str>,
) -> Option<zerocode_core::orchestration::Headroom> {
    with_usage_headroom(usage, agent, model, |headroom| headroom)
}

/// The same cache read, with its writer excluded through the committed effect.
fn with_usage_headroom<R>(
    usage: &UsageSource,
    agent: &str,
    model: Option<&str>,
    read: impl FnOnce(Option<zerocode_core::orchestration::Headroom>) -> R,
) -> R {
    let Some(gauge) = zerocode_core::orchestration::quota_gauge_for(agent, model) else {
        return read(None);
    };
    match usage {
        UsageSource::Cached { local_data_root } => {
            let Some(cache) = cached_usage(local_data_root, gauge) else {
                return read(None);
            };
            let snapshot = cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            read(snapshot.as_ref().and_then(headroom_of))
        }
        #[cfg(test)]
        UsageSource::Fixed(rows) => {
            let rows = rows.lock().unwrap_or_else(|held| held.into_inner());
            read(
                rows.iter()
                    .find(|(named, _)| *named == gauge)
                    .and_then(|(_, snapshot)| headroom_of(snapshot)),
            )
        }
    }
}

/// Bytes an unprivileged writer could still put on the volume holding `path`.
///
/// One `statvfs`, no subprocess: this runs inside the actor while it holds
/// the ledger, where a `git` or a `df` would be the main-thread stall this
/// window has already been bitten by. `f_bavail` rather than `f_bfree` —
/// the blocks a non-root process can actually use, which is what `df`
/// reports as available and what the ledger's own writes are bounded by.
///
/// `None` is "nobody looked": a path that does not exist, a NUL in it, a
/// platform with no `statvfs` — all answered as unmeasured, never as empty.
pub(crate) fn free_bytes_at(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `statvfs` writes into the zeroed struct we hand it and reads
        // only the NUL-terminated path; both live for the whole call.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let answered = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
        if answered != 0 {
            return None;
        }
        // `f_bavail` is 32 bits wide on macOS and 64 on Linux, and `f_frsize`
        // is `c_ulong` on both: a lossless widening on one platform is an
        // identity cast on the other, so the cast is spelled once and the
        // identity lint waived rather than the spelling forked per OS. The
        // product is what can overflow, never either factor.
        #[allow(clippy::unnecessary_cast)]
        let blocks = stat.f_bavail as u64;
        #[allow(clippy::unnecessary_cast)]
        let block_size = stat.f_frsize as u64;
        Some(blocks.saturating_mul(block_size))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

const LEDGER_FILE: &str = "orchestration.json";

/// How long a second spawn waits in line while the journal walks one pane,
/// and the step it waits in. Cutting a pane is a few host calls — the line
/// moves in milliseconds — so the ceiling is for a walk that has died with
/// its permit, and hitting it earns an honest refusal, not a recovery story.
const EFFECT_LINE_CEILING: std::time::Duration = std::time::Duration::from_secs(5);
const EFFECT_LINE_STEP: std::time::Duration = std::time::Duration::from_millis(25);

/// What the ledger file had to say for itself.
#[derive(Debug)]
enum Loaded {
    /// There is no file. A first boot, and an empty ledger is not a guess — it
    /// is the truth.
    Fresh,
    /// The last window's ledger, read whole — beside the digest of the exact
    /// bytes it was read from, which is the name the import goes in under.
    /// The digest of the BYTES and not of any reserialization: the person's
    /// file is the identity, and two windows reading the same file must
    /// arrive at the same name whatever their serializers do.
    Read(Box<Ledger>, String),
    /// The file is THERE and could not be understood: the read failed, or the
    /// bytes are not the JSON this window knows how to read. Carries the
    /// sentence a person will be shown, because by the time anyone asks, the
    /// `io::Error` is long gone.
    Unreadable(String),
}

/// Read the ledger file, without deciding anything about it.
///
/// Split out from [`open`] so the three answers can be tested without a window
/// around them, and — the reason it exists at all — so that "the file is
/// missing" and "the file is damaged" stop being the same answer.
///
/// They were the same answer, and the consequence was not a degraded boot: it
/// was DATA LOSS. `open` folded both into an empty ledger, and then wrote that
/// empty ledger back over the file — so a single unreadable byte, a permission
/// the installer got wrong, a half-written file from a power cut, became a
/// window that silently deleted every run, task, dispatch and message the
/// person had, at boot, with no way back.
fn read_ledger(path: &Path) -> Loaded {
    let bytes = match crate::durable_file::read_plain_file(path) {
        Ok(bytes) => bytes,
        // The one benign failure, and the only one. Everything else — a
        // permission, an I/O fault, a directory where the file should be — is a
        // file we must assume is real and must not touch.
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => return Loaded::Fresh,
        Err(why) => return Loaded::Unreadable(format!("it could not be read ({why})")),
    };
    // Invalid UTF-8 arrives here too: `from_slice` is given bytes and answers
    // for both the encoding and the shape, which is why this reads the file as
    // bytes rather than as a string.
    let read = match serde_json::from_slice::<Ledger>(&bytes) {
        Ok(read) => read,
        Err(why) => {
            return Loaded::Unreadable(format!(
                "its {} bytes are not a ledger this window can read ({why})",
                bytes.len()
            ));
        }
    };
    /* Parsing it is not the same as being able to act on it.
     *
     * Raised by the Codex session, with two files that parse perfectly and
     * still cost the person their ledger. A counter standing below an id the
     * file already holds mints that id a second time — two runs with one name,
     * neither addressable apart from the other. A reference to something that
     * is not there sends a verb into a run that does not exist.
     *
     * Neither is repairable from here. Guessing a counter or dropping a
     * dangling reference is this window deciding what the person's ledger
     * should have said, so it is refused with the file untouched, exactly like
     * bytes we could not parse at all.
     */
    if let Err(wrong) = read.validate_loaded() {
        return Loaded::Unreadable(format!("it parses, and then {wrong}"));
    }
    let named = digest(b"zerocode.orchestration.legacy-file.v1", &[&bytes]);
    Loaded::Read(Box::new(read), named)
}

/// Why orchestration is unavailable in this window, or `None` if it is not.
///
/// Set once at boot by [`open`] and never cleared, because nothing this window
/// can do repairs the file — the person has to fix or move it and start again,
/// and a window that quietly recovered halfway through would be a window with
/// half a ledger.
static UNAVAILABLE: Mutex<Option<String>> = Mutex::new(None);

/// What every road asks before it plans anything.
///
/// Per-THREAD in tests, because a process-global switch would put every other
/// test sharing this binary into a degraded window, and one of them would
/// duly fail.
fn unavailable() -> Option<String> {
    #[cfg(test)]
    if let Some(pretend) = tests::pretend_unavailable() {
        return Some(pretend);
    }
    UNAVAILABLE
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clone()
}

/// The whole of what a caller is told, and it says all three things.
///
/// That the ledger is gone, that the FILE is not — nobody should go looking for
/// a backup that was never overwritten — and what actually ends this state.
fn ledger_is_unavailable(why: &str) -> String {
    // `concat!` and not a backslash-continued literal: rustfmt leaves this
    // alone, and the continued form is one stray edit away from carrying its
    // own indentation into what a person reads on their terminal.
    format!(
        concat!(
            "orchestration is unavailable in this window because the ledger {why}. ",
            "The file has been left exactly as it was, byte for byte — nothing ",
            "has been written over it. Repair or move it and restart the window.",
        ),
        why = why
    )
}

/// The same whole answer, for the durable authority rather than the file.
///
/// The file's sentence is kept word for word where the file is the problem;
/// this one exists because "the store could not be opened" is not a fact
/// about the person's ledger file, and telling them to repair THAT file
/// would send them to the wrong door.
fn authority_is_unavailable(why: &str) -> String {
    format!(
        concat!(
            "orchestration is unavailable in this window because the durable ",
            "authority {why}. The ledger file has been left exactly as it was ",
            "— nothing has been written over it. Restart the window; if this ",
            "repeats, the authority store beside the ledger file needs repair. ",
            "With all windows using that store closed, preserve authority.sqlite ",
            "and its WAL/SHM files as an authority-before-<UTC timestamp>.sqlite ",
            "backup set. Work on a separate consistent SQLite backup copy. ",
            "Validate the copy with PRAGMA integrity_check, a strict ",
            "ledger_store::read (byte counts and table digests), and Ledger::rebuild. ",
            "Apply only a supported content repair and repeat those checks; ",
            "raw SQL updates invalidate the table digests. After checkpointing and ",
            "closing the verified copy, replace the original by an atomic ",
            "rename in the same directory, with old sidecars kept with the backup ",
            "and no windows running. Keep the backup until restart succeeds.",
        ),
        why = why
    )
}

/// Refuse to orchestrate in this window, with the sentence that says why.
fn stand_down(local_data_root: &Path, said: String) {
    *UNAVAILABLE.lock().unwrap_or_else(|held| held.into_inner()) = Some(said.clone());
    crate::note_window_event(local_data_root, &format!("orchestration: {said}"));
}

/// The environment variable that moves this window's orchestration files.
const ORCHESTRATION_DATA_ROOT_ENV: &str = "ZEROCODE_ORCHESTRATION_DATA_ROOT";

/// Where this window's orchestration files live — the legacy ledger file,
/// the durable authority under `authority/`, the federation book under
/// `federation/`, the blackbox — resolved in this one place.
///
/// The window's own data root, unless `ZEROCODE_ORCHESTRATION_DATA_ROOT`
/// names another directory. The override is for a harness that runs more
/// than one window, or more than one test process, on one machine: each
/// gets a ledger of its own instead of writing into the same one. Every
/// other reader of these paths goes through `BLACKBOX`, which `open` sets
/// from this answer, so the override cannot be honoured here and missed
/// there. With the variable unset or empty the answer is the root given,
/// byte for byte — the product's behaviour is unchanged. The window's own
/// event log stays where the window keeps it: that file is the window's,
/// not the ledger's.
fn orchestration_data_root(local_data_root: &Path) -> PathBuf {
    resolve_orchestration_data_root(
        local_data_root,
        std::env::var_os(ORCHESTRATION_DATA_ROOT_ENV),
    )
}

/// `orchestration_data_root` with the environment's word passed in, so a
/// test can ask without touching the process environment.
fn resolve_orchestration_data_root(
    local_data_root: &Path,
    named: Option<std::ffi::OsString>,
) -> PathBuf {
    match named {
        Some(named) if !named.is_empty() => PathBuf::from(named),
        _ => local_data_root.to_path_buf(),
    }
}

/// Stand the durable authority up, and say how many terminals the last
/// window lost.
///
/// Called once at boot, before any verb can arrive. The person's legacy file
/// is read HERE and never again: its bytes are digested, its rows are handed
/// to the actor's cutover boot, and from then on the store answers — a stale
/// copy of the file cannot argue with a revision written since. Every worker
/// the last window had running is settled through the actor's own restart
/// door — its pane died with that window — while the runs, the tasks and the
/// mail stand exactly as they were. That asymmetry is the point: an
/// orchestration is a plan, and a plan does not stop being true because the
/// screen showing it went away.
///
/// A file that is not there is a first boot. A file that cannot be READ
/// stops orchestration for this window rather than starting it over: see
/// [`read_ledger`] for what that used to cost. A store or an actor that
/// cannot come up stops it the same way, with its own sentence — the file
/// has still not been touched.
pub(crate) fn open(local_data_root: &Path, now_ms: i64) -> Restarted {
    open_with_headroom(local_data_root, now_ms, HeadroomSource::Measured)
}

fn open_with_headroom(local_data_root: &Path, now_ms: i64, headroom: HeadroomSource) -> Restarted {
    let root = orchestration_data_root(local_data_root);
    let path = root.join(LEDGER_FILE);
    let legacy = match read_ledger(&path) {
        Loaded::Fresh => None,
        Loaded::Read(read, named) => Some(Box::new((read.export(), named))),
        Loaded::Unreadable(why) => {
            stand_down(local_data_root, ledger_is_unavailable(&why));
            // And nothing else — nothing is written over a file this window
            // refuses to trust.
            return Restarted::default();
        }
    };
    let Some(epoch_seed) = crate::hooks::random_token() else {
        stand_down(
            local_data_root,
            authority_is_unavailable("this window could not mint a host epoch"),
        );
        return Restarted::default();
    };
    let _ = BLACKBOX.set(root.clone());
    let _ = BOOTED_AT_MS.set(now_ms);
    let _ = HOST_EPOCH.set(digest(
        b"zerocode.orchestration.host-epoch.v1",
        &[epoch_seed.as_bytes()],
    ));
    /* The store demands a private parent (its preflight refuses anything
     * else), and the app's data root is not one — so the authority gets a
     * directory of its own, made private before the store ever opens it. */
    let vault = root.join("authority");
    if let Err(why) = std::fs::create_dir_all(&vault) {
        stand_down(
            local_data_root,
            authority_is_unavailable(&format!("its directory could not be made ({why})")),
        );
        return Restarted::default();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(why) = std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(0o700)) {
            stand_down(
                local_data_root,
                authority_is_unavailable(&format!(
                    "its directory could not be made private ({why})"
                )),
            );
            return Restarted::default();
        }
    }
    let authority = vault.join("authority.sqlite");
    let store = match WorkflowStore::open(&authority) {
        Ok(store) => {
            let _ = AUTHORITY_STORE.set(authority);
            store
        }
        Err(why) => {
            stand_down(
                local_data_root,
                authority_is_unavailable(&format!("its SQLite store could not be opened ({why})")),
            );
            return Restarted::default();
        }
    };
    let overrides = Arc::new(Mutex::new(Vec::new()));
    let usage = UsageSource::Cached {
        local_data_root: local_data_root.to_path_buf(),
    };
    let actor = match RuntimeActor::start(
        &store,
        "main-ledger",
        RuntimeBoot::Cutover { legacy, now_ms },
        MAX_RUNTIME_MAILBOX,
        Box::new(ShellPaneTable),
        Box::new(LiveCatalog {
            overrides: Arc::clone(&overrides),
            ledger_dir: vault.clone(),
            headroom,
            usage: usage.clone(),
        }),
    ) {
        Ok(actor) => actor,
        Err(why) => {
            stand_down(
                local_data_root,
                authority_is_unavailable(&format!("its runtime could not start ({why})")),
            );
            return Restarted::default();
        }
    };
    /* An effect the last window died inside of is reconciled FIRST — the
     * actor fences every mutation until it is, the restart sweep below
     * included. What this window can prove about that operation is exactly
     * one thing: the backend that was walking it died with its window. That
     * is a fence — the old epoch can never act again — so the operation is
     * settled `NotStarted` for the world this window can see, and the sweep
     * puts down whatever rows the old attempt had already written. */
    let recoveries = match actor.view() {
        Ok(image) => {
            for note in image.repairs() {
                crate::note_window_event(local_data_root, &format!("orchestration: {note}"));
            }
            image.recoveries().len()
        }
        Err(why) => {
            stand_down(
                local_data_root,
                authority_is_unavailable(&format!("its runtime could not be read ({why})")),
            );
            return Restarted::default();
        }
    };
    for _ in 0..recoveries {
        let tombstone = digest(
            b"zerocode.orchestration.window-died-tombstone.v1",
            &[host_epoch().unwrap_or_default().as_bytes()],
        );
        /* Retryable: the request name whose effect died with the old
         * window may be asked again in this one — a dead window must not
         * burn its callers' names for good. */
        if let Err(why) = actor.settle_recovered(EffectSettlement::NotStarted {
            failure: HostEffectFailure::Disconnected,
            tombstone_digest: tombstone,
            retryable: true,
            retry_not_before_ms: Some(now_ms.saturating_add(1)),
            settled_at_ms: now_ms,
        }) {
            stand_down(
                local_data_root,
                authority_is_unavailable(&format!(
                    "an effect from the last window could not be reconciled ({why})"
                )),
            );
            return Restarted::default();
        }
    }
    let swept = match actor.window_restarted(now_ms) {
        Ok((swept, _)) => swept,
        Err(why) => {
            stand_down(
                local_data_root,
                authority_is_unavailable(&format!(
                    "the last window's terminals could not be settled ({why})"
                )),
            );
            return Restarted::default();
        }
    };
    install_runtime(actor, overrides, usage);
    /* The federation heartbeat starts with the authority it relays for —
     * in production. The test binary shares one process across a thousand
     * parallel worlds, and a background thread knocking on the actor once
     * a second is weather every timing test would have to survive; the
     * tests that need a pass call it themselves. */
    #[cfg(not(test))]
    spawn_federation_relay();
    swept
}

/// Ring the bell for an answer whose revision moved the ledger.
///
/// The wake lives HERE and in the actor's replies nowhere else: an answer
/// that says it moved something is the one place that already knows, and a
/// `check` that read an empty inbox and changed nothing wakes no one — which
/// is what keeps a window full of pollers from waking a window full of
/// sleepers. The actor has already made the change durable by the time any
/// reply exists, so there is no "rung but not written" state left to hold a
/// second cell for.
fn rang(moved: bool) {
    if moved {
        mail_arrived();
    }
}

/// A refusal in the runtime's words, in the shim's shape.
///
/// One mapping so the sentences stay the file's own: a write the disk
/// refused keeps the exact sentence this road has always given for it, and
/// everything else speaks the runtime error's display — including
/// `UnauthorizedSeat`, whose display IS the one answer this road has always
/// given for a seat it will not plan for.
fn refused_by_runtime(why: RuntimeError) -> zerocode_hookd::TeamAnswer {
    match why {
        RuntimeError::NotDurable => {
            refused("the ledger could not be written — the change is not durable")
        }
        other => refused(other),
    }
}

/// A count of ledger writes, and the bell that goes with it.
///
/// `check --wait` is not a poll: it sleeps here until something is written and
/// then looks again. A counter rather than a flag because a sleeper has to be
/// able to say "since I last looked" — a flag that was set and cleared while it
/// was being woken is a message it would sleep straight through.
///
/// **This is the LAST lock: nothing is ever taken while holding it.** Every
/// verb takes the team table then the ledger; a reader of this counter may take
/// it under the ledger (that is how a sleeper reads the count without a write
/// slipping past between its look and its sleep), and the bell-ringer takes it
/// alone. One direction only, so there is no cycle to deadlock on.
///
/// And a sleeper never holds the LEDGER, which the `Condvar` guarantees by
/// giving up this lock while it sleeps: a waiter holding the ledger would be a
/// window where nothing can arrive, because it would be blocking the very write
/// it is waiting for.
///
/// `Condvar` and not a channel because the waiters are on blocking threads
/// (`teams_loop` spawns each verb as one) and because it is the same primitive
/// on every platform this ships to.
fn mail() -> &'static (Mutex<u64>, std::sync::Condvar) {
    static MAIL: OnceLock<(Mutex<u64>, std::sync::Condvar)> = OnceLock::new();
    MAIL.get_or_init(|| (Mutex::new(0), std::sync::Condvar::new()))
}

/// Ring the bell: the ledger moved.
fn mail_arrived() {
    let (count, bell) = mail();
    let mut held = count
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *held = held.wrapping_add(1);
    // Every sleeper, not one: several coordinators can be waiting on several
    // inboxes, and this bell does not know whose mail arrived. Each wakes,
    // looks in its own inbox, and goes back to sleep if the write was not for
    // it.
    bell.notify_all();
}

/// What the bell has rung up to, right now.
fn mail_seen() -> u64 {
    *mail()
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sleep until the ledger moves past `seen`, or until `deadline`.
///
/// Answers the new count, which the caller passes back as `seen` if it looked
/// and found nothing — otherwise a write that lands between the look and the
/// next sleep is a wake-up nobody ever gets.
fn wait_for_mail(seen: u64, deadline: std::time::Instant) -> u64 {
    let (count, bell) = mail();
    let mut held = count
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while *held == seen {
        let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) else {
            break;
        };
        let (next, timed_out) = bell
            .wait_timeout(held, left)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        held = next;
        if timed_out.timed_out() {
            break;
        }
    }
    *held
}

/* ---- the mail pointer --------------------------------------------------
 *
 * The one mechanism by which an idle pane learns it has mail without asking:
 * a single advice line typed at its composer, and Enter one beat later. The
 * design note refused pane-typing once ("§4-B: 판에 타이핑하지 않는다") and
 * the reversal is a work-order decision (2026-08-25) carrying three
 * conditions, each a door below: only panes THIS window's teams hold, never
 * after an interrupted turn, never while the holder waits on a question or a
 * sleeper of its own. The pointer carries no mail — `Run::pointer_wanted` is
 * the ledger's half, and the lease machinery stays the one delivery road. */

/// What the window last OBSERVED about a pane's turn.
#[derive(Clone, Copy)]
enum PaneTurn {
    /// A turn is under way — the window heard this pane say something that
    /// was not a turn ending.
    Running,
    /// The last turn ended, and how it ended.
    Ended { interrupted: bool },
}

/// What each pane's turn was last measured doing, off the same hook event that
/// stamps a worker quiet.
///
/// Three-valued on purpose, the discipline [`pane_attention`] already keeps for
/// its own half of the same reports: **absent from this map is NEVER HEARD** —
/// an agent whose hooks are not installed, a pane before its first report, a
/// window that has only just restarted — and never-heard is not the same fact
/// as [`PaneTurn::Running`]. Only a measured, uninterrupted [`PaneTurn::Ended`]
/// licenses typing at a pane; the other two both mean "do not type", but they
/// mean it for different reasons and for different lengths of time. A running
/// turn ends and is measured; a silence nobody wired can outlast the run, and
/// [`point_at_waiting_mail`] says so out loud rather than waiting forever.
///
/// In memory on purpose, and rebuilt from nothing: rest is a fact about a live
/// pane in THIS window, so a restart is right to forget it. What a restart must
/// not do is go on looking like a pane at work.
fn pane_turns() -> &'static Mutex<std::collections::HashMap<u32, PaneTurn>> {
    static TURNS: OnceLock<Mutex<std::collections::HashMap<u32, PaneTurn>>> = OnceLock::new();
    TURNS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// How many `check --wait` sleepers each address currently has.
///
/// A sleeper IS the better pointer: the bell answers it the instant mail
/// lands, with the mail itself. Typing advice at a pane whose agent is
/// blocked inside a wait would land the advice in a composer nobody is
/// reading — so an address with a sleeper is skipped, and the registry is
/// kept by the sleep loop itself.
fn address_waiters() -> &'static Mutex<std::collections::HashMap<(String, String), usize>> {
    static WAITERS: OnceLock<Mutex<std::collections::HashMap<(String, String), usize>>> =
        OnceLock::new();
    WAITERS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Which SEATS sleep on which address — the one-sleeper rule's own key.
///
/// Per seat rather than per address since t-2512: `run:<id>` is read by
/// exactly one seat at a time, but the seat can change under a sleeper
/// (`run-takeover`), and the new coordinator's first `check --wait` used to
/// be refused as "a second sleeper" because the unseated pane's card was
/// still in the map — its wait ends on the next bell, but the bell and the
/// refusal raced. Two seats may hold cards on one address; only the seat's
/// look ever hands mail over (`look_again` asks), so the race the address
/// rule guarded against — two leases cut from one queue — cannot happen
/// between two seats.
fn seat_waiters() -> &'static Mutex<std::collections::HashSet<(String, String, String)>> {
    static SEATS: OnceLock<Mutex<std::collections::HashSet<(String, String, String)>>> =
        OnceLock::new();
    SEATS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// One sleeper's presence in [`address_waiters`] and [`seat_waiters`], given
/// back on every exit road including an unwind.
struct WaiterCard {
    key: (String, String),
    seat: (String, String, String),
}

impl WaiterCard {
    fn hold(run: &str, address: &str, seat: &str) -> Self {
        let key = (run.to_string(), address.to_string());
        *address_waiters()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .entry(key.clone())
            .or_insert(0) += 1;
        let seat = (run.to_string(), address.to_string(), seat.to_string());
        seat_waiters()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(seat.clone());
        Self { key, seat }
    }

    /// Whether THIS seat already sleeps on this address — the first-come
    /// rule behind refusing a second waiter. Asked before holding, and the
    /// small race between asking and holding is harmless in the one
    /// direction it can go: two callers passing the gate together each hold
    /// a card, and the bell simply answers both, which is exactly the world
    /// before this rule existed.
    fn occupied(run: &str, address: &str, seat: &str) -> bool {
        seat_waiters()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .contains(&(run.to_string(), address.to_string(), seat.to_string()))
    }
}

impl Drop for WaiterCard {
    fn drop(&mut self) {
        let mut held = address_waiters()
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if let Some(count) = held.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.key);
            }
        }
        drop(held);
        seat_waiters()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .remove(&self.seat);
    }
}

/// Where each address's pointer stands: what it was pointed at, at which
/// terminal, and how far the pointing got.
///
/// In memory on purpose. The map is derivable advice — the ledger holds the
/// mail, the idle map holds the pane — so a restart forgetting it costs one
/// repeated pointer line, and persisting it would be a second authority over
/// facts two other tables already own.
#[derive(Clone)]
struct Pointed {
    newest: String,
    /// The terminal the pointer stood at, and `None` where there was none to
    /// stand at — a run this window holds no seat for at all. Optional rather
    /// than a sentinel because every `u32` here is a real terminal id.
    term: Option<u32>,
    standing: Standing,
}

/// How far one address's pointer has got with the mail it names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// The advice is registered with the pump as one guarded delivery — the
    /// paste and its Enter — and its receipt is in [`pointer_receipts`]. The
    /// next beats read the receipt; nothing is retyped while it stands.
    /// `noted` carries whether an earlier beat already wrote down that this
    /// mail could not be delivered, so a pane that stays shut costs one line
    /// and not one line per second.
    Advised { noted: bool },
    /// Advice and Enter both landed — nothing more is owed for this mail.
    Delivered,
    /// Advice was pasted but its Enter was withheld. Keep this transient
    /// marker to avoid stacking text; unread mail still needs notification
    /// through a later native hook or after recovery.
    Unsubmitted,
    /// The guard refused the advice at the write: a person's draft, a parked
    /// question, a relaunched pane. Written down once; the next beat offers
    /// the same line again, because every one of those clears on its own —
    /// the draft is sent, the question answered — and the guard is the door
    /// that will know.
    Withheld,
    /// No road would carry the advice, and the window has said so once. The
    /// next beat tries the same line again; the black box is not told twice.
    Unreachable,
    /// No advice was composed at all — nobody this pointer may speak to is at
    /// this pane, and nothing it can wait for will change that — and the
    /// window has said so once. Held against the watermark like the rest: the
    /// next message waiting behind the same silence is new news and earns its
    /// own line, while the beats in between cost nothing.
    Unattended,
    /// This window holds no seat for this address at all, and the window has
    /// said so once. The pointer never got as far as a door: there is no
    /// terminal to try. Only `check` can reach this mail now, and only from a
    /// pane that comes back.
    Seatless,
    /// The pane's own last answer was a wall that answers every prompt the
    /// same way — a quota, an expired login (t-6560) — and the window has
    /// said so once. Nothing is typed: a line typed there opens a turn the
    /// CLI ends at once, that turn's own start strikes these marks out, and
    /// the next beat at rest used to type the same line again — every two to
    /// five seconds, 1,391 times on the coordinator's pane in four days.
    ///
    /// Held until `until_ms`, the wall's own window (`QUOTA_WAIT_POLICY`,
    /// through `wall_stands_until`), and looked at again only then: a pane
    /// that answers again or new mail strikes the mark out and is looked at
    /// afresh, and a wall still standing at its deadline is held anew.
    Walled {
        until_ms: i64,
        cause: crate::quota_wall::StallCause,
    },
}

fn pointed() -> &'static Mutex<std::collections::HashMap<(String, String), Pointed>> {
    static POINTED: OnceLock<Mutex<std::collections::HashMap<(String, String), Pointed>>> =
        OnceLock::new();
    POINTED.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// The receipt of each advice line the pump is carrying, by the mail it is
/// about.
type PointerReceipts = std::collections::HashMap<
    (String, String),
    std::sync::mpsc::Receiver<zerocode_pty::DeliveryOutcome>,
>;

/// The receipts, beside [`pointed`] rather than inside it because a receiver
/// is not `Clone` and the marks are; taken out the moment it answers.
fn pointer_receipts() -> &'static Mutex<PointerReceipts> {
    static RECEIPTS: OnceLock<Mutex<PointerReceipts>> = OnceLock::new();
    RECEIPTS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// What the pump has said about an advice line in flight, if anything yet.
enum Receipt {
    /// Still being carried — the composer has not settled, or the gap has
    /// not passed.
    Pending,
    /// The pump answered.
    Settled(zerocode_pty::DeliveryOutcome),
    /// The delivery was dropped without an answer: the pane died under it,
    /// or the window forgot the terminal. Nothing landed that anybody knows
    /// of, and the next beat looks again.
    Lost,
}

fn read_pointer_receipt(key: &(String, String)) -> Receipt {
    let mut receipts = pointer_receipts()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    let Some(receipt) = receipts.get(key) else {
        return Receipt::Lost;
    };
    match receipt.try_recv() {
        Ok(outcome) => {
            receipts.remove(key);
            Receipt::Settled(outcome)
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => Receipt::Pending,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            receipts.remove(key);
            Receipt::Lost
        }
    }
}

/// What the pump's answer about an advice line makes of its standing.
///
/// `None` while the pump has not answered; the caller leaves the mark as it
/// is. A delivery that landed is spoken for, and so is one that pasted and
/// withheld its Enter — the line is on the person's screen, the Enter is
/// theirs, and retyping would stack a second copy under it. One the guard
/// refused, or that no road carried, is written down once and offered again
/// on the next beat. `Lost` clears the mark so the next beat looks afresh.
fn advice_standing(
    run: &str,
    address: &str,
    term: u32,
    newest: &str,
    noted: bool,
) -> Option<Option<Standing>> {
    let stood = match read_pointer_receipt(&(run.to_string(), address.to_string())) {
        Receipt::Pending => return None,
        Receipt::Lost => return Some(None),
        Receipt::Settled(outcome) => outcome,
    };
    Some(Some(match stood {
        zerocode_pty::DeliveryOutcome::Delivered => {
            note_delivered(run, address, newest);
            Standing::Delivered
        }
        zerocode_pty::DeliveryOutcome::Unsubmitted(_) => {
            note_pointer_withheld(run, address, term, stood);
            Standing::Unsubmitted
        }
        zerocode_pty::DeliveryOutcome::Refused(_) => {
            if !noted {
                note_pointer_withheld(run, address, term, stood);
            }
            Standing::Withheld
        }
        zerocode_pty::DeliveryOutcome::TimedOut => {
            if !noted {
                note_pointer_silent(run, address, term, NO_ROAD_CARRIED_IT);
            }
            Standing::Unreachable
        }
    }))
}

/// A pointer's advice reached the composer and stopped there: the guard
/// withheld the Enter, or refused the paste. Said once per refusal, in the
/// guard's own words, beside the line that would have carried it.
fn note_pointer_withheld(
    run: &str,
    address: &str,
    term: u32,
    outcome: zerocode_pty::DeliveryOutcome,
) {
    let Some(root) = BLACKBOX.get() else { return };
    let what = match outcome {
        zerocode_pty::DeliveryOutcome::Refused(why) => {
            format!("was not typed at terminal {term}: {}", why.says())
        }
        zerocode_pty::DeliveryOutcome::Unsubmitted(why) => format!(
            "was pasted at terminal {term} and left unsubmitted: {}",
            why.says()
        ),
        zerocode_pty::DeliveryOutcome::Delivered | zerocode_pty::DeliveryOutcome::TimedOut => {
            return;
        }
    };
    crate::note_window_event(
        root,
        &format!("orchestration: the pointer for {address} in {run} {what}"),
    );
}

/// How long a waiting message counts as news the pointer may type about.
/// Past this it is history: `check` still delivers it, the black box notes it
/// once, and no composer hears about it.
const POINTER_NEWS_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

/// How many delivered watermarks the window keeps on disk. A watermark is one
/// message id per address; a run's mail is a few dozen lines, so this holds
/// weeks of them and bounds the file either way.
const DELIVERED_WATERMARKS_KEPT: usize = 512;

/// The file, under the window's data root, naming every `(run, address,
/// message)` a pointer reached Enter for. Absent when there is no root yet
/// (tests without a window, a beat before startup finished).
fn delivered_watermarks_path() -> Option<std::path::PathBuf> {
    BLACKBOX
        .get()
        .map(|root| root.join("orchestration-pointed.json"))
}

/// `(run, address, message)` triples a pointer reached Enter for, oldest first.
type DeliveredWatermarks = std::collections::VecDeque<(String, String, String)>;

fn delivered_store() -> &'static Mutex<Option<DeliveredWatermarks>> {
    static DELIVERED: OnceLock<Mutex<Option<DeliveredWatermarks>>> = OnceLock::new();
    DELIVERED.get_or_init(|| Mutex::new(None))
}

fn load_delivered_watermarks() -> DeliveredWatermarks {
    let Some(path) = delivered_watermarks_path() else {
        return DeliveredWatermarks::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return DeliveredWatermarks::new();
    };
    serde_json::from_str::<Vec<(String, String, String)>>(&raw)
        .map(DeliveredWatermarks::from)
        .unwrap_or_default()
}

/// Whether a pointer already reached Enter for `newest` at `address` in
/// `run` — in this life of the window or an earlier one.
fn delivered_before(run: &str, address: &str, newest: &str) -> bool {
    let mut store = delivered_store()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    let known = store.get_or_insert_with(load_delivered_watermarks);
    known.iter().any(|(held_run, held_address, held_newest)| {
        held_run == run && held_address == address && held_newest == newest
    })
}

/// Write down that a pointer reached Enter for `newest`, so no later beat —
/// and no later window — types it again.
pub(crate) fn note_delivered(run: &str, address: &str, newest: &str) {
    let mut store = delivered_store()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    let known = store.get_or_insert_with(load_delivered_watermarks);
    let mark = (run.to_string(), address.to_string(), newest.to_string());
    if known.contains(&mark) {
        return;
    }
    known.push_back(mark);
    while known.len() > DELIVERED_WATERMARKS_KEPT {
        known.pop_front();
    }
    let Some(path) = delivered_watermarks_path() else {
        return;
    };
    let rows: Vec<&(String, String, String)> = known.iter().collect();
    if let Ok(payload) = serde_json::to_vec(&rows) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, payload).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Forget every pointer this process remembers and read the watermarks back
/// from disk — what a window restart does, minus the window.
#[cfg(test)]
pub(crate) fn restart_pointer_memory_for_tests() {
    pointed()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clear();
    pointer_receipts()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clear();
    *delivered_store()
        .lock()
        .unwrap_or_else(|held| held.into_inner()) = None;
}

fn note_pointer_stale(run: &str, address: &str, newest: &str) {
    let Some(root) = BLACKBOX.get() else { return };
    crate::note_window_event(
        root,
        &format!(
            "orchestration: {newest} for {address} in {run} is older than the pointer's news \
             window — left to `check`, nothing typed"
        ),
    );
}

/// A new turn began in this pane: it is not idle, and whatever pointer state
/// it carried is stale.
/// Terminals the window has HEARD since the last beat — a turn beginning or
/// ending, either is a sound. Only the window sees beginnings, so only the
/// window can carry them; the readiness sweep drains this on the beat and
/// hands it to the actor, whose seat table knows whose worker each is.
fn spoken_terms() -> &'static Mutex<std::collections::HashSet<u32>> {
    static SPOKEN: OnceLock<Mutex<std::collections::HashSet<u32>>> = OnceLock::new();
    SPOKEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// What the hooks last said about each terminal's need for a PERSON —
/// the window's half of `agentWait`. Three values on purpose (Orca's own
/// contract, `orchestration-worker-observation.ts`): absent from the map is
/// "never evaluated" — an agent that wires no hooks, a pane before its first
/// report — and absence is never "not waiting"; `None` is "looked, nothing
/// waiting"; `Some(since)` is a composer holding a question for the person,
/// stamped with when that state began.
fn pane_attention() -> &'static Mutex<std::collections::HashMap<u32, Option<i64>>> {
    static ATTENTION: OnceLock<Mutex<std::collections::HashMap<u32, Option<i64>>>> =
        OnceLock::new();
    ATTENTION.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// A hook report said whether this terminal is waiting on the person.
pub(crate) fn pane_attention_noted(term: u32, waiting_since: Option<i64>) {
    pane_attention()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .insert(term, waiting_since);
}

/// The terminal is gone; its number's next life starts unevaluated.
pub(crate) fn forget_pane_attention(term: u32) {
    pane_attention()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .remove(&term);
}

/// Lay the window's word for each row's PANE over a worker-observation answer.
///
/// `seat: live | gone | unknown` (t-2512 §1.4). The ledger's rows travel
/// untouched; this adds the one fact only the window holds — whether the
/// terminal behind `team/pane` is there — resolved against whichever table
/// holds that seat, the caller's or another leader's. `live` and `gone` are
/// the window's own machine answer; `unknown` is a seat no table maps, which
/// is where the ledger's `paneMissingSinceMs` (the reconciler's proof, written
/// on the row) is the only word left, and where nobody has looked otherwise.
/// A foreign row's `term` is filled in here too, so `agentWait` and the
/// coordinator reading the roster get the terminal the ledger could not name.
fn garnish_seats(host: &dyn Host, verb: Option<&str>, reply: &mut zerocode_hookd::TeamAnswer) {
    if reply.exit_code != 0 || !matches!(verb, Some("worker-show") | Some("worker-list")) {
        return;
    }
    let Ok(mut answer) = serde_json::from_str::<serde_json::Value>(&reply.stdout) else {
        return;
    };
    // Resolve every seat under one table lock, then let the lock go before
    // the host is asked anything — the lock order the file keeps everywhere.
    let resolved: Vec<Option<u32>> = {
        let tables = crate::agent_teams::teams();
        let seat_of = |worker: &serde_json::Value| -> Option<u32> {
            let team = worker["team"].as_str()?;
            let pane = worker["pane"].as_str()?;
            tables.get(team)?.term_of(pane)
        };
        match answer["workers"].as_array() {
            Some(workers) => workers.iter().map(seat_of).collect(),
            None => vec![seat_of(&answer)],
        }
    };
    let garnish = |worker: &mut serde_json::Value, term: Option<u32>| {
        let seat = match term {
            Some(term) => {
                if worker["term"].is_null() {
                    worker["term"] = serde_json::json!(term);
                }
                if host.pane_exists(term) {
                    "live"
                } else {
                    "gone"
                }
            }
            None if worker["paneMissingSinceMs"].is_number() => "gone",
            None => "unknown",
        };
        worker["seat"] = serde_json::json!(seat);
    };
    match answer["workers"].as_array_mut() {
        Some(workers) => {
            for (worker, term) in workers.iter_mut().zip(resolved) {
                garnish(worker, term);
            }
        }
        None => garnish(&mut answer, resolved.into_iter().next().flatten()),
    }
    reply.stdout = format!("{answer}\n");
}

/// Lay the window's `agentWait` verdict over a worker-observation answer.
///
/// The ledger's rows travel untouched — this adds ONE field the ledger
/// cannot know, keyed off the `term` the answer already carries, and only
/// for the two verbs that observe workers. The three-valued contract is the
/// map's own (see [`pane_attention`]); a worker whose pane has no terminal
/// stays absent, because an unattached pane was never looked at.
fn garnish_agent_wait(verb: Option<&str>, reply: &mut zerocode_hookd::TeamAnswer) {
    if reply.exit_code != 0 || !matches!(verb, Some("worker-show") | Some("worker-list")) {
        return;
    }
    let Ok(mut answer) = serde_json::from_str::<serde_json::Value>(&reply.stdout) else {
        return;
    };
    let looked = pane_attention()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clone();
    let garnish = |worker: &mut serde_json::Value| {
        let Some(term) = worker["term"].as_u64() else {
            return;
        };
        match looked.get(&(term as u32)) {
            Some(Some(since)) => {
                let source = match worker["agent"].as_str() {
                    Some("zo") => "channel",
                    _ => "hook",
                };
                worker["agentWait"] = serde_json::json!({ "source": source, "since": since });
            }
            Some(None) => worker["agentWait"] = serde_json::Value::Null,
            // Never evaluated: the field stays ABSENT, which must not print
            // the same as an evaluated "none".
            None => {}
        }
    };
    match answer["workers"].as_array_mut() {
        Some(workers) => workers.iter_mut().for_each(garnish),
        None => garnish(&mut answer),
    }
    reply.stdout = format!("{answer}\n");
}

/// Terminals already reported taken, so the hot key path pays one set probe
/// after the first report instead of an actor round trip per keystroke.
/// Cleared with the terminal (`agent_teams::forget_term` calls back here):
/// the NEXT incarnation of that number is a different pane.
fn taken_terms() -> &'static Mutex<std::collections::HashSet<u32>> {
    static TAKEN: OnceLock<Mutex<std::collections::HashSet<u32>>> = OnceLock::new();
    TAKEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Provider sessions already written for each terminal incarnation.
///
/// The whole value is the key: a later hook can attach a transcript path to
/// the same provider id. An entry lands only after the actor confirms a
/// durable move, so a store refusal or a session observed before a worker is
/// seated is retried by the next hook report.
type ReportedSessions = std::collections::HashMap<u32, zerocode_core::ProviderSession>;

fn reported_sessions() -> &'static Mutex<ReportedSessions> {
    static REPORTED: OnceLock<Mutex<ReportedSessions>> = OnceLock::new();
    REPORTED.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// A person's hand landed real keys in this terminal.
///
/// Called from the window's OWN key road — never from paste delivery or any
/// programmatic write — because "the person took this pane" must mean a
/// hand, not a machine. One report per terminal per incarnation: the ledger
/// marks the worker sitting there as the person's, durably, and the gate
/// keeps every later keystroke to a lock-and-probe.
pub(crate) fn pane_taken_over(term: u32, now_ms: i64) {
    {
        let mut taken = taken_terms()
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if !taken.insert(term) {
            return;
        }
    }
    if unavailable().is_some() {
        return;
    }
    let Some(held) = runtime() else {
        return;
    };
    if let Ok((moved, _)) = held.actor.pane_taken_over(term, now_ms) {
        rang(moved);
    }
}

/// The window is on its way out, and every pane in it goes with it (t-3058).
///
/// Said at the first exit signal — a relaunch, a quit, the run loop's own
/// exit — and idempotent after it. Seated workers sleep NOW, with their
/// dispatch open and their task still dispatched, so a pane that exits on
/// the way out settles nothing: before this road existed, the pane reaper
/// could reach `terminal_gone` first and the worker was written down as
/// dead (`the terminal holding this worker exited`, a gate on its task)
/// while the next window resumed the very same conversation into the very
/// same checkout with no seat to report from.
///
/// And before any seat sleeps, the census is read (t-6428): the goodbye
/// names the road the window is leaving by and, for every live worker, the
/// turn and the commands it cuts — `census` is the window's reading
/// ([`restart_census::take`]), asked on every call and cheap on every call
/// after the first, when nobody live is left to read.
pub(crate) fn window_exiting(
    now_ms: i64,
    road: crate::exit_runtime::ExitRoad,
    census: &dyn Fn() -> restart_census::RestartCensus,
) {
    let taken = census();
    // Once per exit for the road, and whenever there are workers to name:
    // the first call is the one that finds them, before they sleep.
    let first = !GOODBYE_SAID.swap(true, std::sync::atomic::Ordering::SeqCst);
    if (first || !taken.workers.is_empty())
        && let Some(root) = BLACKBOX.get()
    {
        // What each worker's wake will be told was cut (t-6428 ⑤) — left
        // before the lines, and replaced whole: an older note is stale.
        if let Err(error) = restart_census::leave_cut(root, &taken) {
            crate::note_window_event(
                root,
                &format!("exit: the cut commands were not left for the wakes: {error}"),
            );
        }
        for line in restart_census::goodbye_lines(
            &road.to_string(),
            crate::exit_runtime::choice().word(),
            &taken,
        ) {
            crate::note_window_event(root, &line);
        }
    }
    if unavailable().is_some() {
        return;
    }
    let Some(held) = runtime() else {
        return;
    };
    match held.actor.window_exiting(now_ms) {
        Ok((swept, _)) => {
            if swept.sleeping > 0
                && let Some(root) = BLACKBOX.get()
            {
                crate::note_window_event(
                    root,
                    &format!(
                        "orchestration: {} terminals go to sleep with the window",
                        swept.sleeping
                    ),
                );
            }
            rang(swept.moved);
        }
        Err(_) => note_ledger_unwritten("the window's goodbye"),
    }
}

/// The sleeper a conversation the window is about to resume would be, if
/// it is one — asked before the pane exists, so the resume nudge can say
/// so in words (t-3058). A read; [`pane_resumed`] is the write.
pub(crate) fn sleeper_awaiting(checkout: &str, agent: &str, session_id: &str) -> Option<String> {
    if unavailable().is_some() {
        return None;
    }
    let held = runtime()?;
    let image = held.actor.view().ok()?;
    let ledger = cached_ledger(&held, &image).ok()?;
    ledger
        .sleeper_awaiting(checkout, agent, session_id)
        .map(|worker| worker.id.clone())
}

/// The window resumed a conversation into `term` (t-3058): this checkout,
/// this agent, this provider session. If a sleeper is that conversation,
/// the ledger seats it there as the same worker — dispatch and task kept,
/// no handover — and the window says so in its log. The witness is the
/// window's own fact about the pane it just opened; nothing is inferred.
///
/// Answers the worker seated, or `None` for the ordinary conversation a
/// person reopened.
pub(crate) fn pane_resumed(
    term: u32,
    checkout: &str,
    agent: &str,
    session_id: &str,
    now_ms: i64,
) -> Option<String> {
    if unavailable().is_some() {
        return None;
    }
    let held = runtime()?;
    let seated = match held
        .actor
        .worker_pane_resumed(term, checkout, agent, session_id, now_ms)
    {
        Ok((seated, _)) => seated,
        Err(_) => {
            note_ledger_unwritten("a resumed pane's seat");
            return None;
        }
    };
    if let Some(worker) = &seated {
        if let Some(root) = BLACKBOX.get() {
            crate::note_window_event(
                root,
                &format!(
                    "orchestration: worker {worker} is seated again by its resumed pane, \
                     terminal {term} in {checkout}"
                ),
            );
        }
        rang(true);
    }
    seated
}

/// When this window booted, on the ledger's clock — the moment from which
/// a sleeper's grace ([`RESEAT_GRACE_MS`]) is measured (t-3058).
///
/// Set once by [`open`]. A test binary never opens the production window,
/// so a test that drives the grace names its own boot on its own thread.
static BOOTED_AT_MS: OnceLock<i64> = OnceLock::new();

fn booted_at_ms() -> Option<i64> {
    #[cfg(test)]
    if let Some(named) = tests::booted_at_override() {
        return Some(named);
    }
    BOOTED_AT_MS.get().copied()
}

/// Ask every live coordinator team to seat its own run's sleepers, once,
/// before the grace kills them.
///
/// The overrides cell is handed back exactly as it stands: [`reseat_sleeping`]
/// takes ownership of what a caller passes and writes it into the cell, so a
/// sweep that passed an empty list would erase the launcher's stored
/// overrides on its way past. Reading them out and handing them straight back
/// makes this call a no-op for that cell.
///
/// The native host supplies the returned conversation's actor, just as it
/// does for a mounted tab. A new team need not have a seat binding yet.
fn seat_what_the_grace_would_kill(host: &dyn Host, held: &RuntimeSeat) {
    let leaders: Vec<u32> = {
        let teams = crate::agent_teams::teams();
        teams.values().map(|team| team.leader_term).collect()
    };
    for leader in leaders {
        let overrides = held
            .overrides
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone();
        let actor = host.actor_for(leader);
        let seated = reseat_sleeping(host, overrides, leader, actor.as_deref());
        if seated > 0
            && let Some(root) = BLACKBOX.get()
        {
            crate::note_window_event(
                root,
                &format!(
                    "orchestration: the grace seated {seated} sleeping worker(s) under                      leader term {leader} instead of ending them"
                ),
            );
        }
    }
}

/// The grace, on the beat (t-3058): a sleeper that nothing seated again —
/// neither the ledger's own reseat nor a resumed pane as its witness —
/// within [`RESEAT_GRACE_MS`] of this window's boot dies, announced to its
/// run with the dispatch id a `--retry-of` needs.
///
/// Measured from the boot rather than from the exit that put it to sleep:
/// the grace is the time THIS window had to bring the pane back, and a
/// window that stayed closed overnight has had none of it. A sleeper the
/// beat finds under a window younger than the grace is left exactly as it
/// is.
///
/// The grace ends with one more attempt, not with the killing.
/// [`reseat_sleeping`] is what a returned coordinator tab calls, and until
/// now nothing else called it: a window could boot with a live coordinator
/// team and three sleepers on its run, and the beat would announce all three
/// dead without once asking that team to seat them. It happened
/// (2026-09-18): the work was finished and committed in every one of them,
/// and a person had to notice and say so. So the sweep now walks the live
/// teams first and only expires what is still asleep afterwards. A reseat
/// that seats nothing — no coordinator for that run, no empty chair — leaves
/// the sleeper exactly where the killing finds it, which is the old
/// behaviour and the right one.
fn expire_sleepers(host: &dyn Host, now_ms: i64) {
    let Some(booted) = booted_at_ms() else {
        return;
    };
    if now_ms < booted.saturating_add(RESEAT_GRACE_MS) {
        return;
    }
    let Some(held) = runtime() else {
        return;
    };
    seat_what_the_grace_would_kill(host, &held);
    let Ok(image) = held.actor.view() else {
        return;
    };
    let Ok(ledger) = cached_ledger(&held, &image) else {
        return;
    };
    let overdue: Vec<String> = ledger
        .runs()
        .iter()
        .flat_map(|run| run.workers.iter())
        .filter(|worker| worker.state == WorkerState::Sleeping)
        .map(|worker| worker.id.clone())
        .collect();
    drop(ledger);
    for worker in overdue {
        if held.actor.sleeper_expired(&worker, now_ms).is_ok() {
            if let Some(root) = BLACKBOX.get() {
                crate::note_window_event(
                    root,
                    &format!(
                        "orchestration: sleeping worker {worker} was not resumed within the \
                         grace and its attempt ended"
                    ),
                );
            }
            rang(true);
        }
    }
}

/// The live workers seated in THIS window's panes, each with the terminal
/// that holds it — who leaving the window would cut (t-3058, t-6428). A live
/// row with a seat this window maps; sleepers, orphans nobody maps and
/// released rows are not panes a restart would cut. The restart census
/// ([`restart_census::take`]) reads its workers here and nowhere else.
pub(crate) fn seated_live_workers() -> Vec<restart_census::Seated> {
    with_ledger_seats(|ledger, seats| {
        ledger
            .runs()
            .iter()
            .flat_map(|run| run.workers.iter())
            .filter(|worker| worker.state.is_live() && worker.state.may_occupy_pane())
            .filter_map(|worker| {
                let term = seats
                    .get(worker.team.as_str())
                    .and_then(|panes| panes.get(worker.pane.as_str()))?;
                Some(restart_census::Seated {
                    worker: worker.id.clone(),
                    agent: worker.agent.clone(),
                    term: *term,
                })
            })
            .collect()
    })
    .unwrap_or_default()
}

/// A hook observed a provider session in this terminal.
///
/// Ordinary panes and reports that race ahead of worker seating answer
/// `moved: false` and are deliberately not cached. The same event can then be
/// replayed once the term is a ledger worker. Errors follow the same rule.
pub(crate) fn pane_session_reported(
    term: u32,
    session: &zerocode_core::ProviderSession,
    now_ms: i64,
) {
    if reported_sessions()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .get(&term)
        == Some(session)
    {
        return;
    }
    if unavailable().is_some() {
        return;
    }
    let Some(held) = runtime() else {
        return;
    };
    if let Ok((true, _)) = held
        .actor
        .worker_session_reported(term, session.clone(), now_ms)
    {
        reported_sessions()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(term, session.clone());
        rang(true);
    }
}

/// The terminal is gone; its number may be minted again for a fresh pane.
pub(crate) fn forget_taken_term(term: u32) {
    taken_terms()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .remove(&term);
    reported_sessions()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .remove(&term);
}

pub(crate) fn pane_turn_began(term: u32) {
    spoken_terms()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .insert(term);
    /* Written down as RUNNING rather than struck out. Both readings refuse to
     * type here, so the old erasure looked equivalent — but it threw away the
     * one fact that separates a pane which will be measured at rest in a
     * minute from a pane nothing will ever measure at all. Only removal, on
     * the roads where the terminal itself is gone, may leave this map. */
    pane_turns()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .insert(term, PaneTurn::Running);
    pointed()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .retain(|_, held| held.term != Some(term));
}

/// A pane's turn ended. Tell the ledger, in case that pane is a worker's.
///
/// The window is where the two vocabularies meet: a hook report names a
/// TERMINAL, the ledger names a `(team, pane)` seat, and nothing but the team
/// table can turn one into the other. The actor does that turning ITSELF,
/// under the same table lock it settles in — the fence this road used to keep
/// by hand ("the seat has to still mean THIS term at the moment it is
/// settled") is structural there, and reports arrive for every pane in the
/// window — most of them nobody's worker — so a seat that resolves to nothing
/// is the ordinary case and not a failure.
pub(crate) fn pane_turn_ended(term: u32, turn_started_ms: i64, interrupted: bool, now_ms: i64) {
    // A sound is a sound: the actor road below also retires the readiness
    // window, but it does not run in a degraded window — the beat's sweep
    // still must not report a pane the window plainly heard.
    spoken_terms()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .insert(term);
    // Nobody to refuse on this road — it is a hook event, not a verb — so it
    // simply does not run in a degraded window. See [`unavailable`]; the
    // window already said why, once, at boot. A silence the store refuses is
    // the same nobody-to-answer shape: the actor keeps the fact out of its
    // rows entirely (poison rewinds), so the next turn's end reports it
    // again rather than a half-written silence surviving.
    /* The idle fact is written down whatever the runtime's state: it is a
     * window fact, not a ledger one, and the pointer pass reads it on the
     * next beat. An interrupted end is remembered AS interrupted — the person
     * typed, and the second condition says that pane is theirs, not ours. */
    pane_turns()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .insert(term, PaneTurn::Ended { interrupted });
    if unavailable().is_some() {
        return;
    }
    let Some(held) = runtime() else {
        return;
    };
    let actor = &held.actor;
    if let Ok((moved, _)) = actor.pane_turn_ended(term, turn_started_ms, interrupted, now_ms) {
        rang(moved);
    }
}

/// One beat of every standing order, and how many workers it summoned.
///
/// This is the whole of "automatic": the decision is `next_dispatch`, which
/// is pure and tested without a pane; the carrying-out is the SAME verb a
/// coordinator would have typed, through the same door, cutting the pane with
/// the same tmux. Nothing new opens a terminal — an automatic worker and a
/// typed one are the same code, which is the only way they can be trusted to
/// end the same way.
///
/// **One per run per beat.** The window-side cost of cutting a pane grows with
/// how many are already open, so N cut inside one beat is quadratic in IPC and
/// visibly janks the layout. The next beat takes the next one, and a coordinator
/// that wanted five workers gets them over five beats.
///
/// **A seat that is gone stops the order without ending it.** The team table
/// answers `None` for a coordinator that closed its pane or a window that
/// restarted, and this simply does nothing that beat. Standing an order down
/// for a missing seat would be inferring a decision from an absence, which is
/// the mistake this whole road is written to avoid.
/// Whether a beat is already being carried out.
///
/// Cutting a pane forks and execs, which takes longer than the gap between two
/// beats. Without this, a slow spawn would have the next beat plan against a
/// ledger the previous one has not written to yet, and the same task would be
/// dispatched twice — the second one arriving as a surprise worker on work
/// already being done.
static BEATING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ownership of the one standing-order beat this window may run at a time.
///
/// An atomic has no poison state, and `Drop` gives the door back on both a
/// normal return and an unwind through a host effect.
struct BeatTurn;

impl BeatTurn {
    fn enter() -> Option<Self> {
        use std::sync::atomic::Ordering;
        if BEATING.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(Self)
        }
    }
}

impl Drop for BeatTurn {
    fn drop(&mut self) {
        BEATING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A terminal closed, so whatever attempt was sitting in it is over.
///
/// Called BEFORE the team table forgets the pane, because the seat is how the
/// ledger finds the worker — after `forget_term` there is no longer a pane id
/// to look up, and the attempt would stay open with nothing left that could
/// name it.
///
/// This is the single-terminal twin of `window_restarted`. That one settled
/// every live worker when the whole window came back; this one had no caller
/// at all, so a teammate's pane exiting was reflected only in the live table:
/// the ledger's dispatch stayed open, and a standing order counted it against
/// its ceiling for the rest of the session — the run quietly stopped
/// dispatching and nothing on screen said why.
/// A seat's terminal ended while its pane record lives on — the respawn and
/// kill-pane roads, whose PANE bookkeeping stays with the caller.
///
/// The narrow half of `terminal_gone`: the seat settles, and nothing else.
/// No dissolution is read here on purpose — a respawned leader pane keeps
/// its team, and reading `leader_term` against a terminal the caller is
/// deliberately replacing would dissolve a team that is standing right
/// there. The actor resolves the seat by THIS term under its own table
/// lock, so a seat re-pointed in the caller's gap resolves to nothing and
/// nothing settles — the wrong incarnation cannot be settled, only
/// possibly none.
pub(crate) fn seat_left_its_terminal(term: u32, now_ms: i64) {
    if unavailable().is_some() {
        return;
    }
    let Some(held) = runtime() else {
        return;
    };
    let actor = &held.actor;
    match actor.terminal_gone(term, now_ms) {
        Ok((moved, _)) => rang(moved),
        Err(_) => note_ledger_unwritten("a terminal's settlement"),
    }
}

#[cfg(test)]
pub(crate) fn terminal_gone(term: u32, now_ms: i64) {
    terminal_gone_with_archive(term, None, now_ms);
}

pub(crate) fn terminal_gone_with_archive(term: u32, screen: Option<String>, now_ms: i64) {
    // As in [`pane_turn_ended`]: a window with no durable authority settles
    // nothing, because the ledger it would settle INTO is not the person's.
    if unavailable().is_some() {
        return;
    }
    let Some(held) = runtime() else {
        return;
    };
    let actor = &held.actor;
    /* The SEAT settles inside the actor, which resolves "which seat is term
     * N" and writes the settlement under one table-lock generation — the
     * respawn fence this road used to keep by holding both locks by hand is
     * structural there (`respawn-pane` keeps a pane id on purpose and
     * re-points it; a gap between resolve and settle lands the settlement on
     * the wrong incarnation).
     *
     * Whether this shell was somebody's LEADER is read here, from the table,
     * BEFORE the caller's next line drops the team (`forget_term`) — after
     * that there is no table left to ask. A leader's death ends its whole
     * team, and a road that settled only the leader's own seat left every
     * child worker `Active` with its dispatch open, counting against a
     * standing order's ceiling forever.
     *
     * Two doors, two revisions, where one transaction stood: the seat and
     * the dissolution land separately now. A window that dies between them
     * leaves the dissolution unwritten — and the next boot's restart sweep
     * settles every worker regardless, so the gap heals instead of
     * lingering. */
    /* A dead terminal is not idle and owes nobody an Enter — its pointer
     * state goes with it, so the maps cannot grow by one entry per pane the
     * window has ever closed. Removal, not `Running`: the next pane minted as
     * this number is a different terminal that has said nothing yet, and
     * absence is exactly how [`pane_turns`] spells that. */
    pane_turns()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .remove(&term);
    pointed()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .retain(|_, held| held.term != Some(term));
    let dissolving: Vec<String> = {
        let tables = crate::agent_teams::teams();
        tables
            .iter()
            .filter(|(_, team)| team.leader_term == term)
            .map(|(id, _)| id.clone())
            .collect()
    };
    match actor.terminal_gone_with_archive(term, screen, now_ms) {
        Ok((moved, _)) => rang(moved),
        Err(_) => note_ledger_unwritten("a terminal's settlement"),
    }
    for team in dissolving {
        match actor.team_dissolved(team, now_ms) {
            Ok((moved, _)) => rang(moved),
            Err(_) => note_ledger_unwritten("a team's dissolution"),
        }
    }
}

/// Five accepted one-second beats keep a pane absent across four complete
/// intervals, long enough for a loaded window's short respawn/reseat gap not
/// to become noisy news. A delayed beat does not advance this count by time:
/// the cheap host question must actually answer `false` five times. Waiting
/// about five seconds is still prompt beside the manual recovery this replaces,
/// while a shorter three-beat grace made transient reports plausible enough
/// that people could learn to ignore them. This confirms a contradiction only
/// — it never settles or restarts the worker from absence.
const MISSING_PANE_CONFIRMATIONS: u8 = 5;

impl PaneReconciler {
    fn observe(
        &mut self,
        host: &dyn Host,
        ledger: &Ledger,
        seats: &TeamSeatIndex,
        now_ms: i64,
    ) -> PaneReconciliation {
        let mut answer = PaneReconciliation::default();
        let mut candidates = std::collections::HashSet::new();
        // Pane ids are scoped to a team, but terminal ids are window-wide. If
        // corrupt or retained history names one term twice, ask the host once
        // and apply that one machine fact to both claims.
        let mut terms = std::collections::HashMap::new();

        for run in ledger.runs() {
            for worker in &run.workers {
                if !worker.state.is_live() || !worker.state.may_occupy_pane() {
                    continue;
                }
                candidates.insert(worker.id.clone());
                let mapped_term = seats
                    .get(worker.team.as_str())
                    .and_then(|team| team.get(worker.pane.as_str()))
                    .copied();
                let remembered = self
                    .known
                    .get(&worker.id)
                    .filter(|known| {
                        known.incarnation.team == worker.team
                            && known.incarnation.pane == worker.pane
                    })
                    .cloned();
                let remembered_term = remembered.as_ref().and_then(|known| known.incarnation.term);
                let mut term = mapped_term.or(remembered_term);
                if let Some(term) = mapped_term {
                    self.known.insert(
                        worker.id.clone(),
                        KnownPane {
                            incarnation: PaneIncarnation {
                                team: worker.team.clone(),
                                pane: worker.pane.clone(),
                                term: Some(term),
                            },
                            actor: host.actor_for(term),
                        },
                    );
                } else if let Some(known) = remembered {
                    /* A terminal number can be reused after the team table
                     * that proved its owner is gone. Where the host knew an
                     * actor while that proof stood, require the same actor
                     * now; otherwise this is somebody else's pane. `None`
                     * cannot fence reuse — some agents never publish an
                     * actor — so that cached term is a weak liveness probe
                     * used only for news, never as automatic-adoption proof. */
                    if let (Some(term_id), Some(expected)) = (term, known.actor.as_deref())
                        && host.actor_for(term_id).as_deref() != Some(expected)
                    {
                        term = None;
                        self.known.remove(&worker.id);
                    }
                } else {
                    // A prior incarnation under another seat is not evidence
                    // about this one.
                    self.known.remove(&worker.id);
                }

                /* A routing row can disappear while its terminal stays alive
                 * (leader teardown proves that for orphans). Without a
                 * current or remembered term there is no machine question we
                 * can ask, so every live state is UNKNOWN, not absent.
                 * Folding unknown into missing would make a bookkeeping gap
                 * look like process evidence and eventually teach adoption
                 * to spawn a second pane beside a live first one. */
                if term.is_none() {
                    self.missing.remove(&worker.id);
                    continue;
                }
                let incarnation = PaneIncarnation {
                    team: worker.team.clone(),
                    pane: worker.pane.clone(),
                    term,
                };
                let pane_exists = term.is_some_and(|term| {
                    *terms.entry(term).or_insert_with(|| host.pane_exists(term))
                });
                if pane_exists {
                    self.missing.remove(&worker.id);
                    answer.seen.push(worker.id.clone());
                    continue;
                }

                // A reseat begins a new observation. Absence at an old term
                // is no evidence at all about the new pane incarnation.
                let new_streak = self
                    .missing
                    .get(&worker.id)
                    .is_none_or(|streak| streak.incarnation != incarnation);
                if new_streak {
                    self.missing.insert(
                        worker.id.clone(),
                        MissingPaneStreak {
                            incarnation,
                            since_ms: now_ms,
                            confirmations: 1,
                        },
                    );
                } else if let Some(streak) = self.missing.get_mut(&worker.id) {
                    streak.confirmations = streak.confirmations.saturating_add(1);
                }
                let streak = self
                    .missing
                    .get(&worker.id)
                    .expect("the missing pane was just observed");
                if streak.confirmations >= MISSING_PANE_CONFIRMATIONS {
                    answer.missing.push((worker.id.clone(), streak.since_ms));
                }
            }
        }

        // Ended and sleeping workers make no pane claim. Forget their partial
        // streak so becoming live again must earn fresh confirmations.
        self.missing.retain(|worker, _| candidates.contains(worker));
        self.known.retain(|worker, _| candidates.contains(worker));
        answer
    }
}

fn reconcile_pane_liveness(host: &dyn Host, now_ms: i64) {
    // The test ledger is process-global and retains live rows from many
    // independent scenarios whose tiny hosts model only their own pane. One
    // end-to-end test opts this pass in on its thread; production has one
    // window/host and always runs it.
    #[cfg(test)]
    if !tests::pane_reconciliation_enabled() {
        return;
    }
    let Some(held) = runtime() else {
        return;
    };
    // The actor can need the team table while serving an earlier request, so
    // preserve the established lock order: actor image first, owned seat
    // index second, then release the pane table before probing the host.
    let Ok(image) = held.actor.view() else {
        return;
    };
    let Ok(ledger) = cached_ledger(&held, &image) else {
        return;
    };
    let teams = crate::agent_teams::teams();
    let seats = index_team_seats(&teams);
    drop(teams);
    let observed = held
        .pane_reconciler
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .observe(host, &ledger, &seats, now_ms);
    let mut moved = false;
    for workers in observed.seen.chunks(zerocode_core::orchestration::MAX_LIST) {
        if let Ok((cleared, _)) = held.actor.panes_seen(workers.to_vec(), now_ms) {
            moved |= cleared;
        }
    }
    for missing in observed
        .missing
        .chunks(zerocode_core::orchestration::MAX_LIST)
    {
        if let Ok((reported, _)) = held.actor.panes_missing(missing.to_vec(), now_ms) {
            moved |= reported;
        }
    }
    rang(moved);
}

/// One quiet worker, as the stall probe found it, with what the wall
/// witness needs to ask its second question.
struct Stalled {
    run: String,
    worker: String,
    since_ms: i64,
    term: u32,
    agent: String,
    model: Option<String>,
    /// Whether this attempt's wall is already written down and still stands
    /// — a walled worker is not asked again, and is not ALSO a quiet one,
    /// until its wall stops standing (t-6427).
    walled_already: bool,
    /// Its newest wall and what the beat owes it now (t-6427): the wait
    /// rung's gauge question while it stands, its word about the lift after.
    wall: Option<(
        zerocode_core::orchestration::WallAt,
        zerocode_core::orchestration::WallPhase,
    )>,
    /// Whether the run declared `--on-transient-error resume` — asked before
    /// a transcript is read, so an undeclared run costs nothing more.
    resume_declared: bool,
    /// The record keys of this attempt's declines told already (t-6747):
    /// the same decline's silence is ordinary quiet news after that,
    /// reminded as any other — and a decline on another record of the same
    /// attempt is news again (t-7153, R4).
    declines_told: Vec<String>,
    /// The attempt it carries, and that attempt's task.
    dispatch: String,
    task: String,
    /// The checkout its pane sits in, as the window reported it.
    checkout: Option<String>,
    /// Whether it waits on an answer it asked for — a silence the ledger keeps
    /// out of `went_quiet`, and so one nobody asks Jev about either.
    awaiting_reply: bool,
}

/// Ask the window which active worker panes have truly stalled, then let the
/// ledger decide whether their durable quiet episodes are due to notify —
/// and, for a stalled pane, ask the ONE more question §2.2 adds: is this
/// the quota wall? Two witnesses (the agent's words, the provider's number)
/// make it `quota_walled` news instead of a `went_quiet` one; anything less
/// is the silence it always was.
fn notify_stalled_workers(host: &dyn Host, now_ms: i64) {
    let Some(held) = runtime() else {
        return;
    };
    // Preserve the established lock order: actor image first, then a short
    // seat snapshot, and no host probe while the team table is held.
    let Ok(image) = held.actor.view() else {
        return;
    };
    let Ok(ledger) = cached_ledger(&held, &image) else {
        return;
    };
    let teams = crate::agent_teams::teams();
    let seats = index_team_seats(&teams);
    drop(teams);
    let stalled: Vec<Stalled> = ledger
        .runs()
        .iter()
        .flat_map(|run| {
            run.workers.iter().filter_map(|worker| {
                if !worker.state.is_live() || !worker.state.may_occupy_pane() || worker.taken_over {
                    return None;
                }
                let dispatch = run.dispatch(worker.dispatch.as_deref()?)?;
                if !dispatch.is_open()
                    || run
                        .worker_in_pane(&worker.team, &worker.pane)
                        .is_none_or(|current| current.id != worker.id)
                {
                    return None;
                }
                let term = seats
                    .get(worker.team.as_str())?
                    .get(worker.pane.as_str())
                    .copied()?;
                let since_ms = host.quiet_since(term, worker.started_ms, now_ms)?;
                let wall =
                    zerocode_core::orchestration::wall_phase(run, worker, &dispatch.id, now_ms);
                let walled_already = wall.as_ref().is_some_and(|(_, phase)| {
                    matches!(
                        phase,
                        zerocode_core::orchestration::WallPhase::Stands { .. }
                            | zerocode_core::orchestration::WallPhase::Lifting
                    )
                });
                Some(Stalled {
                    run: run.id.clone(),
                    worker: worker.id.clone(),
                    since_ms,
                    term,
                    agent: worker.agent.clone(),
                    model: worker.model.clone(),
                    walled_already,
                    wall,
                    resume_declared: run
                        .handover
                        .as_ref()
                        .is_some_and(|policy| policy.on_transient_error.is_some()),
                    declines_told: zerocode_core::orchestration::declines_told(run, &dispatch.id),
                    dispatch: dispatch.id.clone(),
                    task: dispatch.task.clone(),
                    checkout: worker.checkout.clone(),
                    awaiting_reply: run.awaiting_reply(&worker.id),
                })
            })
        })
        .collect();
    // What followed the silences put to Jev on earlier beats, read off this
    // same image before anything below moves it (t-4538).
    stall_cause::label(host, &held.stalls, &ledger, now_ms);
    drop(ledger);
    let still_quiet: std::collections::HashSet<String> =
        stalled.iter().map(|one| one.dispatch.clone()).collect();
    /* The wall witness, asked of the quiet panes only and outside every
     * lock: the agent's words from the host, the provider's number from the
     * same cache the summons was judged by. Both or nothing — the pure
     * function decides, and a pane with less stays on the quiet road. A
     * wall already written down is asked nothing and reminded of nothing. */
    /* The seat's own readings of silences asked about on earlier beats, for
     * the coordinator's inbox — gathered before the loop below moves each
     * silence, sent after the quiet news so a reading never outruns the
     * silence it reads. */
    let judged = stall_cause::acting_judgments(
        host,
        &held.stalls,
        stalled
            .iter()
            .filter(|one| !one.walled_already)
            .map(|one| (one.worker.clone(), one.dispatch.clone(), one.since_ms)),
    );
    let mut quiet: Vec<(String, i64)> = Vec::new();
    let mut walled: Vec<zerocode_core::orchestration::QuotaWallWitness> = Vec::new();
    let mut declined: Vec<zerocode_core::orchestration::ClassifierDeclineWitness> = Vec::new();
    let mut stopped: Vec<(Stalled, zerocode_core::orchestration::TransientErrorMarker)> =
        Vec::new();
    let mut unmarked: Vec<stall_cause::Silence> = Vec::new();
    let mut lifted: Vec<zerocode_core::orchestration::QuotaLift> = Vec::new();
    let mut asks: Vec<&'static str> = Vec::new();
    for one in stalled {
        use zerocode_core::orchestration::{LiftReading, WallPhase};
        let ask = |asks: &mut Vec<&'static str>| {
            if let Some(gauge) =
                zerocode_core::orchestration::quota_gauge_for(&one.agent, one.model.as_deref())
                && !asks.contains(&gauge)
            {
                asks.push(gauge);
            }
        };
        /* The wall's own road first (t-6427): while it stands the silence is
         * the wall's, and under a declared wait the gauge is asked for a
         * reading from the reset on; once it stops standing the wait rung
         * judges the lift — words still at the wall and a number read after
         * the reset under it are told, a number not read yet is waited for,
         * and anything else is the ordinary road below. */
        let mut read_already = None;
        match &one.wall {
            Some((_, WallPhase::Stands { reread })) => {
                if *reread {
                    ask(&mut asks);
                }
                continue;
            }
            Some((wall, WallPhase::Lifting)) => {
                let marker = host.quota_wall_marker(one.term, &one.agent);
                let headroom = usage_headroom(&held.usage, &one.agent, one.model.as_deref());
                match zerocode_core::orchestration::read_lift(
                    &one.worker,
                    wall,
                    one.since_ms,
                    marker.clone(),
                    headroom.as_ref(),
                    now_ms,
                ) {
                    LiftReading::Lifted(lift) => {
                        lifted.push(lift);
                        continue;
                    }
                    LiftReading::Unread => {
                        ask(&mut asks);
                        continue;
                    }
                    LiftReading::StillWalled | LiftReading::MovedOn => read_already = Some(marker),
                }
            }
            Some((_, WallPhase::Past)) | None => {}
        }
        let marker = read_already.unwrap_or_else(|| host.quota_wall_marker(one.term, &one.agent));
        // The wall's own words name the silence even when the provider's
        // number does not make it news.
        let wall_words = marker.is_some();
        let witness = marker.and_then(|marker| {
            let headroom = usage_headroom(&held.usage, &one.agent, one.model.as_deref());
            zerocode_core::orchestration::quota_wall_witness(
                &one.worker,
                Some(marker),
                headroom.as_ref(),
                now_ms,
            )
        });
        if let Some(witness) = witness {
            walled.push(witness);
            continue;
        }
        /* No wall: a decline, next (t-6747). The CLI's sentence on the
         * screen AND its record as the conversation's last word — or its
         * pause dialog, stood past every dialog a person answered here. Both
         * or nothing, the pure function decides; a pane at a decline is not
         * quiet news until its notice is told, and ordinary quiet news after
         * that — told by the RECORD (t-7153, R4): the pane is read again
         * every beat, and a decline on a record not told yet is news, where
         * the one told already is the silence it was. */
        if !wall_words {
            let witness = host
                .classifier_decline_reading(one.term, &one.agent)
                .and_then(|reading| {
                    zerocode_core::orchestration::classifier_decline_witness(
                        &one.worker,
                        &one.dispatch,
                        reading.screen,
                        reading.record,
                        one.since_ms,
                        now_ms,
                    )
                })
                .filter(|witness| !one.declines_told.contains(&witness.record.key));
            if let Some(witness) = witness {
                declined.push(witness);
                continue;
            }
        }
        /* No wall: under a declared `--on-transient-error resume`, ask the
         * table's second cause of the same transcript tail (t-4537). The wall
         * was asked first and wins; a pane with neither stays quiet, and a
         * run that declared nothing reads no transcript for it. */
        let asked = one.resume_declared
            && crate::quota_wall::has_rule(
                &one.agent,
                crate::quota_wall::StallCause::TransientApiError,
            );
        let transient = asked
            .then(|| host.provider_session(one.term)?.transcript_path)
            .flatten()
            .and_then(|path| crate::quota_wall::transient_error_for(&one.agent, Path::new(&path)))
            /* The stall seat's own answer, when the seat acts (§4): a silence
             * the provider's words did not name, that the seat read as a
             * transient error, is typed a continuation exactly as a marker
             * would be — the same plan, the same receipts — with the seat as
             * the standing order (`JEV_MARKER_SOURCE`). Keyed by the silence,
             * so one answer is typed once however many beats see it. */
            .or_else(|| {
                (stall_cause::acting_cause(host, &held.stalls, &one.dispatch)
                    == Some(zerocode_core::stall_cause::Cause::TransientApiError))
                .then(|| zerocode_core::orchestration::TransientErrorMarker {
                    source: zerocode_core::orchestration::JEV_MARKER_SOURCE.to_string(),
                    line: zerocode_core::stall_cause::Cause::TransientApiError
                        .word()
                        .into(),
                    key: format!(
                        "{}:{}@{}",
                        zerocode_core::orchestration::JEV_MARKER_SOURCE,
                        one.dispatch,
                        one.since_ms
                    ),
                })
            });
        match transient {
            Some(marker) => stopped.push((one, marker)),
            None => {
                if !wall_words && !one.awaiting_reply {
                    unmarked.push(stall_cause::Silence {
                        run: one.run,
                        worker: one.worker.clone(),
                        dispatch: one.dispatch,
                        task: one.task,
                        agent: one.agent,
                        term: one.term,
                        since_ms: one.since_ms,
                        checkout: one.checkout,
                    });
                }
                quiet.push((one.worker, one.since_ms));
            }
        }
    }
    let mut moved = false;
    for workers in walled.chunks(zerocode_core::orchestration::MAX_LIST) {
        if let Ok((told, _)) = held.actor.quota_walls(workers.to_vec(), now_ms) {
            moved |= told;
        }
    }
    for lifts in lifted.chunks(zerocode_core::orchestration::MAX_LIST) {
        if let Ok((told, _)) = held.actor.quota_lifts(lifts.to_vec(), now_ms) {
            moved |= told;
        }
    }
    for workers in declined.chunks(zerocode_core::orchestration::MAX_LIST) {
        if let Ok((told, _)) = held.actor.classifier_declines(workers.to_vec(), now_ms) {
            moved |= told;
        }
    }
    // Never forced, never waited on: the answer is the cache a later beat
    // reads (t-6427).
    for gauge in asks {
        host.ask_usage(gauge);
    }
    moved |= resume_stalled_workers(host, stopped, &mut quiet, now_ms);
    for workers in quiet.chunks(zerocode_core::orchestration::MAX_LIST) {
        if let Ok((notified, _)) = held.actor.quiet_sweep(workers.to_vec(), now_ms) {
            moved |= notified;
        }
    }
    for readings in judged.chunks(zerocode_core::orchestration::MAX_LIST) {
        if let Ok((told, _)) = held.actor.stall_causes(readings.to_vec(), now_ms) {
            moved |= told;
        }
    }
    rang(moved);
    /* The silences the table could not name, put to Jev under the person's
     * stall switch — after the news is written, so a question changes
     * nothing the coordinator hears, and off the beat (t-4538). */
    stall_cause::ask_about(host, &held.stalls, unmarked, &still_quiet, now_ms);
}

/* ---- a classifier decline's switch of model (t-6747) ------------------ */

/// A live worker this window seats in its pane now, with the open attempt it
/// carries — one reading of a ledger image ([`with_ledger_seats`]) for the
/// beats that ask each such pane a question of their own (t-6747).
struct SeatedAttempt {
    worker: String,
    agent: String,
    started_ms: i64,
    term: u32,
    dispatch: String,
    /// When the attempt began — the interval a switch of model is read
    /// against, never the worker's summons (t-7153, R3).
    dispatch_started_ms: i64,
    /// Where the LEDGER holds this worker's conversation is written, when
    /// it has heard (t-7153, R3).
    transcript: Option<String>,
    taken_over: bool,
    /// The record keys of this attempt's classifier declines told already.
    declines_told: Vec<String>,
}

fn seated_open_attempts(ledger: &Ledger, seats: &TeamSeatIndex) -> Vec<SeatedAttempt> {
    ledger
        .runs()
        .iter()
        .flat_map(|run| {
            run.workers.iter().filter_map(|worker| {
                if !worker.state.is_live() {
                    return None;
                }
                let dispatch = run.dispatch(worker.dispatch.as_deref()?)?;
                if !dispatch.is_open()
                    || run
                        .worker_in_pane(&worker.team, &worker.pane)
                        .is_none_or(|current| current.id != worker.id)
                {
                    return None;
                }
                let term = seats
                    .get(worker.team.as_str())?
                    .get(worker.pane.as_str())
                    .copied()?;
                Some(SeatedAttempt {
                    worker: worker.id.clone(),
                    agent: worker.agent.clone(),
                    started_ms: worker.started_ms,
                    term,
                    dispatch: dispatch.id.clone(),
                    dispatch_started_ms: dispatch.started_ms,
                    transcript: worker
                        .session
                        .as_ref()
                        .and_then(|session| session.transcript_path.clone()),
                    taken_over: worker.taken_over,
                    declines_told: zerocode_core::orchestration::declines_told(run, &dispatch.id),
                })
            })
        })
        .collect()
}

/// Tell a classifier decline its pause dialog is hiding behind a stale
/// `working` hook (t-6747). Claude Code's pause dialog ends no turn, so the
/// stall sweep — which reads quiet panes only — never sees the pane; but its
/// pty goes still, where a turn that is working redraws its spinner
/// (measured on 2.1.281 against a local stand-in for the API: a pane waiting
/// on a slow answer wrote 135 times in 20 s; the dialog's pane wrote once in
/// the 75 s after it, and nothing after its 17th second). The dialog on
/// screen and a pty silent past every dialog a person answered here are the
/// two witnesses ([`zerocode_core::orchestration::classifier_decline_witness`]);
/// a pane the sweep can read is the sweep's. A notice is one RECORD's
/// (t-7153, R4): a told dialog — one notice per attempt, its key the
/// attempt's — is not told again, and a record the reading finds, told
/// already, is not told again either; but the pane is read again every
/// beat it stands behind the stale hook, because the record the CLI writes
/// once a key answers the dialog — or another request declined — is news
/// of its own, told and planned on its own key. Before this a told dialog
/// closed the reading, and a record behind a hook that stayed `working`
/// was never seen by either sweep.
fn note_paused_declines(host: &dyn Host, now_ms: i64) {
    let (Some(seated), Some(held)) = (with_ledger_seats(seated_open_attempts), runtime()) else {
        return;
    };
    let mut declined = Vec::new();
    for one in seated {
        if one.taken_over
            || !crate::quota_wall::has_rule(
                &one.agent,
                crate::quota_wall::StallCause::ClassifierDecline,
            )
            || host.quiet_since(one.term, one.started_ms, now_ms).is_some()
        {
            continue;
        }
        let Some(since_ms) = host.decline_quiet_since(
            one.term,
            one.started_ms,
            now_ms,
            zerocode_core::orchestration::DECLINE_DIALOG_UNANSWERED_MS,
        ) else {
            continue;
        };
        let witness = host
            .classifier_decline_reading(one.term, &one.agent)
            .and_then(|reading| {
                zerocode_core::orchestration::classifier_decline_witness(
                    &one.worker,
                    &one.dispatch,
                    reading.screen,
                    reading.record,
                    since_ms,
                    now_ms,
                )
            })
            .filter(|witness| !one.declines_told.contains(&witness.record.key));
        declined.extend(witness);
    }
    let mut moved = false;
    for chunk in declined.chunks(zerocode_core::orchestration::MAX_LIST) {
        if let Ok((told, _)) = held.actor.classifier_declines(chunk.to_vec(), now_ms) {
            moved |= told;
        }
    }
    rang(moved);
}

/// Where a live worker's switch scan stands (t-7153): the transcript path
/// its pane reported, the attempt it read under, the cursor into THAT file
/// — the bytes the LEDGER holds the switches of, moved only after the rows
/// are durable, never on the read — and what it read that the ledger has
/// not yet held.
#[derive(Clone)]
struct HeldScan {
    path: String,
    /// The attempt the scan is bound to: the cursor and what waits on it
    /// are this attempt's, and die with it (t-7153, R3).
    dispatch: String,
    cursor: crate::quota_wall::ScanCursor,
    /// A beat's switches the ledger refused to hold — recovering, its store
    /// full — with where the cursor stands once it does: kept here, bound to
    /// this file and attempt, asked again first on the next beat, and
    /// nothing more is read while they wait (t-7153, R3). Before this a
    /// refused beat's switches were let go and read again from the file —
    /// which a rotation could have replaced by then, and the switch was in
    /// the ledger never.
    pending: Option<PendingSwitches>,
}

/// Switches read and not yet held, each bound to the attempt and the file
/// it was read under, and the cursor into THAT file that follows them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingSwitches {
    switches: Vec<zerocode_core::orchestration::ModelDeviation>,
    /// The file they were read from.
    source: String,
    next: crate::quota_wall::ScanCursor,
}

/// Each live worker's switch scan, by worker (t-7153, R3): a scan is an
/// ATTEMPT's own and dies with it — a worker seated later at the same path,
/// a session another summons resumed, the same worker handed a second task,
/// begins its own count, and reads as its own only what was written inside
/// its attempt ([`record_scanned_switches`]), and what the ended attempt's
/// scan still held waiting is let go with it, never written for the next.
/// A path that changed under the same attempt — its CLI began another
/// session — begins a fresh cursor, and what waits, bound to the file it
/// was read from, is still asked first; the ledger's fence says whether
/// that file is still the worker's. In memory on purpose — the ledger keys
/// each row by its record, so a restart that reads a file again from the
/// start writes nothing twice.
fn deviation_scans() -> &'static Mutex<std::collections::HashMap<String, HeldScan>> {
    static READ: OnceLock<Mutex<std::collections::HashMap<String, HeldScan>>> = OnceLock::new();
    READ.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// What a beat's reading of a worker's transcript is bound to (t-7153, R3):
/// the worker, the attempt that was open when it read, the file it read
/// and when that attempt began. Every switch carries the first three to
/// the ledger's fence ([`zerocode_core::orchestration::ModelDeviation::is_bound_to`]),
/// which re-reads them against its own rows as it writes.
struct SwitchBinding<'a> {
    worker: &'a str,
    dispatch: &'a str,
    source: &'a str,
    attempt_started_ms: i64,
}

/// Write down every switch of model a live worker's CLI made to answer a
/// classifier decline on the category's route (t-6747). A summons' model is
/// a binding choice; leaving it, however well, is the coordinator's to read —
/// the two models, the category, how long the CLI keeps the switch, why.
///
/// Read off each worker's own transcript from where the ledger's holdings
/// end ([`crate::quota_wall::scan_fallbacks`]), one window a beat, outside
/// every lock — the scan table is taken to read a cursor and again to keep
/// one, never across the file or the actor — for every live worker whose CLI
/// records such switches; each beat is [`scan_switches_a_beat`]'s, and the
/// scans of workers no longer seated, or seated on another attempt, are let
/// go. The file read is the one the LEDGER holds the worker's conversation
/// is written in, and the pane's own report only while the ledger has heard
/// none (t-7153, R3): the ledger's fence writes a switch only for the file
/// it names, so a reading of a file the pane reported first — a session
/// the ledger has not yet heard of — would be refused there and its cursor
/// moved past it; read once the ledger names it, it is read from its start
/// and lands.
fn note_model_deviations(host: &dyn Host, now_ms: i64) {
    let (Some(seated), Some(held)) = (with_ledger_seats(seated_open_attempts), runtime()) else {
        return;
    };
    let mut moved = false;
    let mut live = std::collections::HashSet::new();
    for SeatedAttempt {
        worker,
        term,
        dispatch,
        dispatch_started_ms,
        transcript,
        ..
    } in seated
        .into_iter()
        .filter(|one| crate::quota_wall::reads_fallbacks(&one.agent))
    {
        live.insert(worker.clone());
        let Some(path) = transcript.or_else(|| {
            host.provider_session(term)
                .and_then(|session| session.transcript_path)
        }) else {
            continue;
        };
        let (cursor, pending) = deviation_scans()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&worker)
            .filter(|standing| standing.dispatch == dispatch)
            .map_or_else(
                || (crate::quota_wall::ScanCursor::default(), None),
                |standing| {
                    let cursor = if standing.path == path {
                        standing.cursor.clone()
                    } else {
                        crate::quota_wall::ScanCursor::default()
                    };
                    (cursor, standing.pending.clone())
                },
            );
        let binding = SwitchBinding {
            worker: &worker,
            dispatch: &dispatch,
            source: &path,
            attempt_started_ms: dispatch_started_ms,
        };
        let (cursor, pending, moved_here) = scan_switches_a_beat(
            &binding,
            Path::new(&path),
            cursor,
            pending,
            &mut |switches| {
                held.actor
                    .model_deviations(switches, now_ms)
                    .map(|(told, _)| told)
            },
        );
        moved |= moved_here;
        deviation_scans()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(
                worker,
                HeldScan {
                    path,
                    dispatch,
                    cursor,
                    pending,
                },
            );
    }
    deviation_scans()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .retain(|worker, _| live.contains(worker));
    rang(moved);
}

/// One beat of one worker's switch scan (t-7153): what an earlier beat read
/// and the ledger refused is asked of `record` again FIRST, and the beat
/// reads nothing more until it is held — so what waits is ever one beat's
/// reading, never a file's; once held, the cursor stands where that reading
/// ended, in the file it read, and the beat reads on from there — the file
/// at `path` now, when it is another, from its start
/// ([`crate::quota_wall::scan_fallbacks`]), its own cursor untouched by a
/// reading of another path. Answers the cursor to keep — with the round of
/// its checks the reading walked on — what still waits, and whether a row
/// moved.
fn scan_switches_a_beat(
    binding: &SwitchBinding<'_>,
    path: &Path,
    cursor: crate::quota_wall::ScanCursor,
    pending: Option<PendingSwitches>,
    record: &mut dyn FnMut(
        Vec<zerocode_core::orchestration::ModelDeviation>,
    ) -> Result<bool, RuntimeError>,
) -> (crate::quota_wall::ScanCursor, Option<PendingSwitches>, bool) {
    let mut moved = false;
    let cursor = match pending {
        Some(waiting) => {
            let (unheld, told) = hold_switches(waiting.switches, record);
            moved |= told;
            if !unheld.is_empty() {
                return (
                    cursor,
                    Some(PendingSwitches {
                        switches: unheld,
                        ..waiting
                    }),
                    moved,
                );
            }
            if waiting.source == binding.source {
                waiting.next
            } else {
                cursor
            }
        }
        None => cursor,
    };
    let scan = crate::quota_wall::scan_fallbacks(path, &cursor);
    let (keep, pending, moved_here) = record_scanned_switches(binding, scan, cursor, record);
    (keep, pending, moved || moved_here)
}

/// Ask `record` to hold `switches`, a list's worth at a time: answers the
/// switches it has not held — every one from the first chunk it refused —
/// and whether a row moved.
fn hold_switches(
    switches: Vec<zerocode_core::orchestration::ModelDeviation>,
    record: &mut dyn FnMut(
        Vec<zerocode_core::orchestration::ModelDeviation>,
    ) -> Result<bool, RuntimeError>,
) -> (Vec<zerocode_core::orchestration::ModelDeviation>, bool) {
    let mut moved = false;
    let mut chunks = switches.chunks(zerocode_core::orchestration::MAX_LIST);
    while let Some(chunk) = chunks.next() {
        match record(chunk.to_vec()) {
            Ok(told) => moved |= told,
            Err(_) => {
                return (
                    chunk.iter().chain(chunks.flatten()).cloned().collect(),
                    moved,
                );
            }
        }
    }
    (Vec::new(), moved)
}

/// Where a switch scan's cursor may stand after `record` was asked to hold
/// its switches (t-7153, P2): at `scan.next` when every chunk was answered —
/// or when there was nothing to hold — and back at `held_at`, where it was,
/// when any chunk was refused (the actor recovering, its store full): the
/// chunks not yet held are answered as PENDING, with `scan.next` for the
/// cursor to take once they are, and [`scan_switches_a_beat`] asks them
/// again first on the next beat; the ledger writes each switch once
/// (`workers_model_deviated` keys a row by its record's uuid). Before this
/// the cursor moved on the read, and a refused row was gone until a
/// restart.
///
/// A switch is the ATTEMPT's that was open when the CLI wrote it (t-7153,
/// R3): one dated before the attempt began — an earlier worker's, in a
/// session this one resumed; the same worker's, on the task it carried
/// before — is not this attempt's, and a record with no time of its own
/// cannot say whose it is; neither is asked of the ledger, and the cursor
/// moves past both, since neither will ever be this attempt's — nor is a
/// row the ledger could never hold ([`zerocode_core::orchestration::ModelDeviation::fits`]).
/// What is asked carries the binding it was read under, for the ledger's
/// own fence. Answers the cursor to keep, what waits, and whether a row
/// moved.
fn record_scanned_switches(
    binding: &SwitchBinding<'_>,
    scan: crate::quota_wall::DeviationScan,
    held_at: crate::quota_wall::ScanCursor,
    record: &mut dyn FnMut(
        Vec<zerocode_core::orchestration::ModelDeviation>,
    ) -> Result<bool, RuntimeError>,
) -> (crate::quota_wall::ScanCursor, Option<PendingSwitches>, bool) {
    let crate::quota_wall::DeviationScan { switches, next } = scan;
    let named: Vec<zerocode_core::orchestration::ModelDeviation> = switches
        .into_iter()
        .filter(|switch| {
            zerocode_core::orchestration::switch_is_the_attempts(
                switch.at_ms,
                binding.attempt_started_ms,
            )
        })
        .map(|mut switch| {
            switch.worker = binding.worker.to_string();
            switch.dispatch = binding.dispatch.to_string();
            switch.source = binding.source.to_string();
            switch
        })
        .filter(zerocode_core::orchestration::ModelDeviation::fits)
        .collect();
    if named.is_empty() {
        return (next, None, false);
    }
    let (unheld, moved) = hold_switches(named, record);
    if unheld.is_empty() {
        return (next, None, moved);
    }
    (
        held_at,
        Some(PendingSwitches {
            switches: unheld,
            source: binding.source.to_string(),
            next,
        }),
        moved,
    )
}

/* ---- the transient-error continuation (t-4537) ------------------------ */

/// A continuation the beat typed and has not settled: the pump's receipt,
/// what it answered once read, and whether the provider has since reported a
/// prompt going in at the pane.
struct Resuming {
    run: String,
    term: u32,
    delivery: std::sync::mpsc::Receiver<zerocode_pty::DeliveryOutcome>,
    /// The delivery's answer, and when this beat first read it.
    answered: Option<(Option<zerocode_pty::DeliveryOutcome>, i64)>,
    /// A prompt event arrived at the pane after the words went to the door.
    prompt_seen: bool,
}

/// The continuations being typed, by receipt row id. In memory on purpose:
/// the row in the ledger is the durable half, and a restart that forgets
/// these reports every one of them `interrupted` ([`Ledger::window_restarted`]).
fn resumes_in_flight() -> &'static Mutex<std::collections::HashMap<String, Resuming>> {
    static FLYING: OnceLock<Mutex<std::collections::HashMap<String, Resuming>>> = OnceLock::new();
    FLYING.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// The provider reported a prompt going in at this pane — the submission
/// evidence a typed continuation waits for (the same `UserPromptSubmit` a
/// launch waits on to know its briefing was consumed).
pub(crate) fn pane_prompt_submitted(term: u32) {
    for one in resumes_in_flight()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .values_mut()
        .filter(|one| one.term == term)
    {
        one.prompt_seen = true;
    }
}

/// Type one continuation at each quiet worker whose own transcript ends on
/// a transient API error, under its run's declared order (t-4537).
///
/// Asked outside every lock, after the wall's road wrote what it had. The
/// composer has to be at rest by the pointer's own reading
/// ([`composer_at_rest`]) — never a pane at a question of its own, never one
/// a person's hand interrupted — and the ledger has to agree
/// ([`zerocode_core::orchestration::resume_plan`]): a continuation already
/// being typed is neither typed again nor quiet news; any other refusal
/// (the ceiling, a marker whose words may be on the line, the spacing) leaves
/// the silence the news it always was. The receipt row is reserved BEFORE
/// the words go to the pump, through the one guarded typed-prompt door
/// ([`Host::point`], the pointer's), and nothing here waits on it.
fn resume_stalled_workers(
    host: &dyn Host,
    stopped: Vec<(Stalled, zerocode_core::orchestration::TransientErrorMarker)>,
    quiet: &mut Vec<(String, i64)>,
    now_ms: i64,
) -> bool {
    use zerocode_core::orchestration::{NotResumed, RESUME_LINE, ResumeOutcome, resume_plan};
    if stopped.is_empty() {
        return false;
    }
    let Some(held) = runtime() else {
        quiet.extend(
            stopped
                .into_iter()
                .map(|(one, _)| (one.worker, one.since_ms)),
        );
        return false;
    };
    let actor = &held.actor;
    let mut moved = false;
    for (one, marker) in stopped {
        let heard = pane_turns()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&one.term)
            .copied();
        if composer_at_rest(host, one.term, heard).is_err() {
            quiet.push((one.worker, one.since_ms));
            continue;
        }
        // A fresh image per candidate: the reservation just above it moved
        // the revision, and the plan must read that row.
        let planned = actor.view().ok().and_then(|image| {
            let rows = cached_ledger(&held, &image).ok()?;
            let run = rows.run(&one.run)?;
            Some(resume_plan(run, &one.worker, &marker, now_ms))
        });
        let plan = match planned {
            Some(Ok(plan)) => plan,
            Some(Err(NotResumed::InFlight)) => continue,
            Some(Err(NotResumed::Refused(_))) | None => {
                quiet.push((one.worker, one.since_ms));
                continue;
            }
        };
        let run = plan.run.clone();
        let Ok((Some(receipt), _)) = actor.resume_begin(plan, now_ms) else {
            quiet.push((one.worker, one.since_ms));
            continue;
        };
        moved = true;
        let Some(delivery) = host.point(one.term, RESUME_LINE, true) else {
            let outcome = ResumeOutcome {
                submitted: false,
                typed: false,
                detail: "the pane could not be addressed; nothing was typed"
                    .to_string()
                    .into(),
            };
            let _ = actor.resume_settle(run, receipt, outcome, now_ms);
            continue;
        };
        resumes_in_flight()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(
                receipt,
                Resuming {
                    run,
                    term: one.term,
                    delivery,
                    answered: None,
                    prompt_seen: false,
                },
            );
    }
    moved
}

/// What a typed continuation came to, once there is something to say.
///
/// `None` while the pump is still carrying the words, or while a delivered
/// paste still has [`zerocode_pty::ready::SUBMIT_ACK_TIMEOUT`] — the window's
/// one budget for a provider's prompt-submit report — to be heard. Only a
/// door that refused or timed out before it pasted says `typed: false`.
fn resume_outcome(
    one: &Resuming,
    now_ms: i64,
) -> Option<zerocode_core::orchestration::ResumeOutcome> {
    use zerocode_pty::DeliveryOutcome;
    let (answer, read_ms) = one.answered?;
    let settled = |submitted: bool, typed: bool, detail: String| {
        Some(zerocode_core::orchestration::ResumeOutcome {
            submitted,
            typed,
            detail: detail.into(),
        })
    };
    match answer {
        Some(DeliveryOutcome::Delivered) if one.prompt_seen => settled(
            true,
            true,
            "typed and entered; the provider reported the prompt going in".into(),
        ),
        Some(DeliveryOutcome::Delivered) => {
            let waited = now_ms.saturating_sub(read_ms);
            let budget = i64::try_from(zerocode_pty::ready::SUBMIT_ACK_TIMEOUT.as_millis())
                .unwrap_or(i64::MAX);
            (waited >= budget).then(|| zerocode_core::orchestration::ResumeOutcome {
                submitted: false,
                typed: true,
                detail: format!(
                    "typed and entered, and no prompt report came within {budget} ms — the \
                     words may be sitting in the composer"
                )
                .into(),
            })
        }
        Some(DeliveryOutcome::Unsubmitted(why)) => settled(
            false,
            true,
            format!("pasted and not entered: {}", why.says()),
        ),
        Some(DeliveryOutcome::Refused(why)) => {
            settled(false, false, format!("nothing was typed: {}", why.says()))
        }
        Some(DeliveryOutcome::TimedOut) => settled(
            false,
            false,
            "nothing was typed: the composer never showed ready".into(),
        ),
        None => settled(
            false,
            true,
            "the pane's door closed without an answer; whether the words landed is unknown".into(),
        ),
    }
}

/// Read every in-flight continuation's receipt and settle the ones that
/// have something to say — on the beat, before the stall sweep plans new
/// ones, so a settled row is what that sweep reads.
fn settle_resumes(now_ms: i64) {
    let Some(held) = runtime() else {
        return;
    };
    let settled: Vec<(String, String, zerocode_core::orchestration::ResumeOutcome)> = {
        let mut flying = resumes_in_flight()
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        let mut settled = Vec::new();
        flying.retain(|receipt, one| {
            if one.answered.is_none() {
                one.answered = match one.delivery.try_recv() {
                    Ok(answer) => Some((Some(answer), now_ms)),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some((None, now_ms)),
                };
            }
            match resume_outcome(one, now_ms) {
                Some(outcome) => {
                    settled.push((one.run.clone(), receipt.clone(), outcome));
                    false
                }
                None => true,
            }
        });
        settled
    };
    for (run, receipt, outcome) in settled {
        if let Ok((moved, _)) = held.actor.resume_settle(run, receipt, outcome, now_ms) {
            rang(moved);
        }
    }
}

pub(crate) fn tick(host: &dyn Host, overrides: &[(String, LaunchOverride)], now_ms: i64) -> usize {
    // The installed scripts leave level-triggered evidence outside the bridge
    // precisely because a broken bridge cannot report itself through that
    // bridge. Read it on this existing beat, even when orchestration is
    // degraded; board paint reads the resulting memory and never polls disk.
    let delivery_failures = crate::hooks::sweep_delivery_failures(now_ms);
    // Before the beat, so the host is never asked to cut a pane for a dispatch
    // that lives only in this window's empty copy of a ledger it could not read.
    if unavailable().is_some() {
        return 0;
    }
    let Some(_turn) = BeatTurn::enter() else {
        return 0;
    };
    coordinator_handover::walk(host, now_ms);
    let summoned = beat(host, overrides, now_ms);
    // What the continuations typed on earlier beats came to, before the
    // sweep below reads their rows (t-4537).
    settle_resumes(now_ms);
    // A turn ending is only a row. The existing beat revisits it after the
    // grace interval and is the sole producer of quiet notifications.
    notify_stalled_workers(host, now_ms);
    // A decline whose pause dialog stands behind a stale `working` hook, which
    // the sweep cannot see, is told on its own two witnesses (t-6747).
    note_paused_declines(host, now_ms);
    // Every switch of model a worker's CLI made to answer a classifier
    // decline is written down, whether or not the worker went quiet (t-6747).
    note_model_deviations(host, now_ms);
    // And a wall with a standing order behind it is walked — one handover
    // per beat, outside every lock, through the one door (§2.3).
    walk_handovers(host, overrides, now_ms);
    // A worker whose turn just ended is read, and its effort moved before
    // the pointer below can start its next turn (t-5637).
    step_effort::sweep(host, now_ms);
    // And the placement seat's quiet labels: a pane the person left where
    // the seat put it for the whole window is graded on this beat (t-5806).
    crate::cmd::worker_room::sweep(host, now_ms);
    point_at_waiting_mail(host, now_ms);
    // The readiness sweep, on the beat that already exists: the sounds heard
    // since the last one retire their windows, and whoever stayed silent past
    // their own is reported to their coordinator — once, as news (§7.2).
    let spoken: Vec<u32> = spoken_terms()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .drain()
        .collect();
    if let Some(held) = runtime()
        && let Ok((moved, _)) = held
            .actor
            .readiness_sweep(spoken, delivery_failures, now_ms)
    {
        rang(moved);
    }
    // Reconcile the ledger's live-pane claims on this same beat. The shell
    // confirms absence over several probes; the actor owns durable one-event
    // de-duplication and deliberately changes no lifecycle state.
    reconcile_pane_liveness(host, now_ms);
    // And the sleepers' grace, on the same beat (t-3058).
    expire_sleepers(host, now_ms);
    /* And retention, on the same beat.
     *
     * The actor decides whether it is DUE — once an hour, judged against the
     * ledger's own stamp rather than against anything this window
     * remembers — so on the three thousand five hundred and ninety-nine
     * beats out of every three thousand six hundred where it is not, this
     * costs a lock and nothing else. A sweep that gave something up rings
     * the window, because a roster that just lost a month of runs is news to
     * anything showing one.
     */
    if let Some(held) = runtime()
        && let Ok((swept, _)) = held.actor.retention_sweep(now_ms)
    {
        rang(swept.moved());
    }
    // And the artifact store, on the ledger's own hour: one pass per ledger
    // sweep, with the days the artifact table (or its settings overlay) says
    // — a second retention clock would be a second number to keep honest.
    if let Some(store) = crate::artifact_runtime::store()
        && let Some((swept_at, days)) =
            with_ledger_seats(|ledger, _| (ledger.swept_at_ms(), ledger.retention_days()))
    {
        // The recall trace (t-2931) rides the same sweep with the same
        // days: one retention clock for everything an agent left behind.
        if store
            .sweep_beside_ledger((swept_at > 0).then_some(swept_at), days, now_ms)
            .is_some()
        {
            let effective = store.limits().effective_retention_days(days);
            let _ = crate::cmd::second_brain::sweep_recall_trace(now_ms, effective);
        }
    }
    summoned
}

/// One pass of the mail pointer, on the beat that already exists.
///
/// One move at most per address per beat: the advice line, registered with
/// the pump as ONE guarded delivery — the paste and its Enter — through the
/// same typed-prompt door every other producer uses ([`Host::point`]). The
/// beat never writes a byte itself and never blocks on the outcome; it reads
/// the receipt on later beats. No new loop, no timer thread; §6.10's rule
/// against fresh polling stands.
///
/// The pass used to write the advice with a raw `Host::send` and the Enter
/// with another on the next beat, revalidating its OWN doors in between —
/// and consulting nobody about the composer: a draft on the line before the
/// first beat, or a hand between the two, was appended to and submitted
/// (integration review 2026-09-05, item 1). Now the delivery's guard decides
/// at each write, with the line facts the pump holds: a draft refuses the
/// paste, a hand after it withholds the Enter, a parked question or a
/// relaunch refuses either. An advice line left unsubmitted in a composer is
/// a person's decision to make, which is where the reversal of "판에
/// 타이핑하지 않는다" ends.
///
/// The first door refuses two panes for two reasons, and the difference is
/// what this pass says out loud. A pane at WORK is refused for a second: its
/// turn will end, the end is measured, and the next beat points at it. A pane
/// the window has never heard from is refused for as long as the run lasts —
/// no hook of that vendor's is installed, or none has fired yet — and mail
/// piling up behind that silence is the one outcome nothing anywhere reported.
/// Neither is typed at. Only one is news.
///
/// Two more refusals, both about time. Mail older than [`POINTER_NEWS_WINDOW_MS`]
/// is not news — it is what `check` is for — so it is never typed at, only
/// written down once. And a pointer that reached Enter is written down in the
/// window's own data root ([`delivered_before`]), so a restart does not type
/// the same line about the same message into the same composer again: that
/// was the pointer's one repeated cost, and a window that restarts several
/// times a day paid it several times a day.
///
/// And one refusal about the pane's own last answer (t-6560). A pane whose
/// CLI answered its last prompt with a wall — a quota, an expired login —
/// answers the next one the same way, at once, without asking any model: a
/// line typed there is a turn that ends before the next beat, and that turn's
/// own start strikes the marks this pass keeps. The coordinator's pane took
/// the same line 1,391 times in four days that way, every two to five
/// seconds. So a fresh line is not typed at a pane standing at its wall
/// ([`Standing::Walled`]): the window says so once, and speaks again when the
/// pane answers again or the wall's own window runs out.
fn point_at_waiting_mail(host: &dyn Host, now_ms: i64) {
    let Some(held) = runtime() else {
        return;
    };
    let Ok(image) = held.actor.view() else {
        return;
    };
    let Ok(rows) = cached_ledger(&held, &image) else {
        return;
    };
    drop(image);
    // The turn facts, copied out so no pointer types under a lock.
    let turns: std::collections::HashMap<u32, PaneTurn> = pane_turns()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clone();
    let waiting: std::collections::HashSet<(String, String)> = address_waiters()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .keys()
        .cloned()
        .collect();
    /// Advice one pass decided to type: the pane, and the line.
    struct Pointing {
        run: String,
        address: String,
        term: u32,
        newest: String,
        text: String,
        notice: Option<zerocode_hookd::session_notify::PointerNotice>,
        /// Whether an earlier beat already wrote down that this pane cannot
        /// be reached, so a pane that stays shut costs one line and not one
        /// line per second.
        unreachable_noted: bool,
    }
    let mut pointing: Vec<Pointing> = Vec::new();
    {
        let tables = crate::agent_teams::teams();
        let mut marks = pointed().lock().unwrap_or_else(|held| held.into_inner());
        for run in rows.runs() {
            /* Which teams this run has actually cut panes in. A worker sits
             * in a pane its coordinator cut, so `worker.team` names the
             * coordinator's own team — the run says who its coordinator is,
             * without anybody having to have written a binding down.
             *
             * That matters because the binding is filed under the
             * coordinator's ACTOR, and an actor is its provider session: a
             * `/clear`, a resume, a respawn-pane, and the same agent at the
             * same pane comes back under a new name. `bound_run` then misses,
             * the leader never gets a seat, and its run's mail goes quiet
             * for as long as the coordinator does not happen to type a bound
             * verb — which a coordinator waiting on mail has no reason to do.
             * Built per run and read below; O(workers) with no lock of its
             * own. */
            let cut_panes_in: std::collections::HashSet<&str> = run
                .workers
                .iter()
                .filter(|worker| worker.state.still_summoned())
                .map(|worker| worker.team.as_str())
                .collect();
            // Every holder with a seat: the coordinator at each bound
            // leader's pane, and every live worker at its own.
            /* Address, pane, and the SEAT behind it. The seat is what tells
             * two readers of one address apart — `run:<id>` is read by every
             * leader pane bound to the run — so it is what the ledger is
             * asked with, and the pointer then counts exactly the mail this
             * seat's own `check` would be handed. */
            let mut seats: Vec<(String, u32, String)> = Vec::new();
            let mut leader_seated = false;
            for (id, team) in tables.iter() {
                /* A leaderless team — its leader exited, its orphans still
                 * work in it — has no leader pane to point at; and where the
                 * run has a coordinator SEAT, a bound leader that is not the
                 * seat does not read `run:` and is not pointed at its mail
                 * (t-2512). A run with no seat keeps the rule below. */
                if team.pane(&team.leader_pane).is_none() {
                    continue;
                }
                if run.seat_is_coordinator(&format!("{id}/{}", team.leader_pane)) == Some(false) {
                    continue;
                }
                /* Two spellings, and the run has to be recognised under
                 * either. `run-create` is a mutation, a mutation requires an
                 * actor, and `bind` files under that actor — so the actor key
                 * is the only spelling a binding is ever WRITTEN under, and it
                 * is the first thing asked. What it is not is stable: see
                 * `cut_panes_in` above. The second reading is derived rather
                 * than written, which is why no seat-key fallback could ever
                 * have worked here; a team id is minted fresh each launch, and
                 * the ledger's worker rows are the only place it survives.
                 *
                 * A beat where the actor is momentarily unknown and the run
                 * has cut no panes yet skips the seat, and the next beat
                 * recovers it. */
                let leader_bound = host
                    .actor_for(team.leader_term)
                    .is_some_and(|actor| rows.bound_run(&actor) == Some(run.id.as_str()))
                    || cut_panes_in.contains(id.as_str());
                if leader_bound {
                    leader_seated = true;
                    seats.push((
                        run.address(),
                        team.leader_term,
                        format!("{id}/{}", team.leader_pane),
                    ));
                }
            }
            /* Addresses whose mail has a READER but no seat in this window.
             *
             * The silence nothing anywhere reported. Every refusal below is
             * about a door this window can see and will not open; this is the
             * case where there is no door — the pane table is process memory,
             * so a window that restarted comes back holding no seats at all,
             * and the loop over `seats` then has nothing to iterate and says
             * nothing about a run whose mail is piling up. */
            let mut seatless: Vec<String> = Vec::new();
            if !leader_seated {
                seatless.push(run.address());
            }
            for worker in &run.workers {
                if !worker.state.is_live() {
                    /* Not `is_live`'s opposite: a sleeper still READS. Its
                     * mail is not lost — the coordinator's return seats it
                     * again — so a sleeper is neither pointed at nor reported
                     * on, and only a row that has ENDED is neither. */
                    if worker.state.reads_mail() {
                        seatless.push(zerocode_core::orchestration::worker_address(&worker.id));
                    }
                    continue;
                }
                // A taken pane is the person's composer: advice typed there
                // would land under their hands.
                if worker.taken_over {
                    continue;
                }
                match tables
                    .get(&worker.team)
                    .and_then(|team| team.term_of(&worker.pane))
                {
                    Some(term) => seats.push((
                        zerocode_core::orchestration::worker_address(&worker.id),
                        term,
                        format!("{}/{}", worker.team, worker.pane),
                    )),
                    None => seatless.push(zerocode_core::orchestration::worker_address(&worker.id)),
                }
            }
            for address in seatless {
                let key = (run.id.clone(), address.clone());
                /* One lookup per seatless address per beat, and nothing else:
                 * `newest_pending` walks a run's handful of inboxes and reads
                 * the back of one queue. The walk that COUNTS the mail
                 * (`pointer_wanted`) is deliberately not reached here — there
                 * is no line to compose, so there is nothing to count. */
                let Some(newest) = run.newest_pending(&address) else {
                    marks.remove(&key);
                    continue;
                };
                let said = marks
                    .get(&key)
                    .is_some_and(|stood| stood.newest == newest && stood.term.is_none());
                if said {
                    continue;
                }
                note_seatless_mail(&run.id, &address);
                marks.insert(
                    key,
                    Pointed {
                        newest: newest.to_string(),
                        term: None,
                        standing: Standing::Seatless,
                    },
                );
            }
            for (address, term, seat) in seats {
                let key = (run.id.clone(), address.clone());
                /* The first two conditions, and the line between them: a door
                 * that a turn's own ending will open needs no report, and a
                 * door that only a person can open is news.
                 *
                 * All three of these refuse to type, and refusing is right in
                 * all three. What was wrong was that they refused the same
                 * way. A running turn ends, the end is measured, and the next
                 * beat points at it — a second's wait nobody needs telling
                 * about. The other two wait on a person who may never come:
                 * an interrupted pane is handed back only by their next
                 * finished turn, and a pane the window has never heard from
                 * may have no hook wired at all, in which case its silence
                 * outlasts the run. */
                let heard = turns.get(&term).copied();
                /* The moment a WORKING pane can be reached without a
                 * keystroke, and the one this pass used to have nothing to
                 * say about.
                 *
                 * A running turn was skipped outright: wait, it will end, and
                 * the next beat types the advice into whatever composer is
                 * then on screen. That wait is exactly where the person kept
                 * finding a draft — the composer took the words and the Enter
                 * did not follow — and it is also the moment the agent is
                 * most reachable, because its own hook is about to knock at
                 * the window's door.
                 *
                 * So the pointer is left on the shelf for that knock. The
                 * measured providers answer a turn-end hook with a
                 * continuation, and the agent goes and reads its mail itself.
                 *
                 * Nothing is marked. If the turn is interrupted, if the pane
                 * exits, if the hook never comes, this beat promised nothing:
                 * the mail is still pending, `check` still owns delivery, and
                 * the ordinary road below speaks about it the moment the turn
                 * is over. A parked pointer can only make the composer road
                 * unnecessary; it can never make it unavailable. */
                if matches!(heard, Some(PaneTurn::Running)) {
                    if crate::orchestration_pointer_mailbox::agent_has_hook_route(
                        host.agent_of(term).as_deref(),
                    ) && let Some(newest) = run.newest_pending(&address)
                        && let Some(count) = run.pointer_wanted(&address, Some(&seat))
                        && let Some(notice) = zerocode_hookd::session_notify::PointerNotice::new(
                            run.id.clone(),
                            address.clone(),
                            newest.to_string(),
                            count,
                        )
                        && crate::orchestration_pointer_mailbox::park(
                            term,
                            &run.id,
                            &address,
                            newest,
                            host.launch_token_of(term),
                            notice,
                        )
                    {
                        note_pointer_parked(&run.id, &address, term);
                    }
                    continue;
                }
                // At rest, and still the agent's only while it holds the
                // terminal: a hand-started agent quit after its last turn
                // leaves the shell in front, and nothing reports that. A
                // running turn was answered above.
                let unattended = composer_at_rest(host, term, heard).err();
                // The third: a sleeper is already the better pointer — for an
                // unattended pane most of all. An agent asleep in a
                // `check --wait` is told the instant mail lands, with the mail
                // itself, whether or not a hook of its vendor's was ever
                // wired; there is nothing wrong here to report.
                if waiting.contains(&key) {
                    continue;
                }
                /* The watermark before the walk. `newest_pending` is a lookup
                 * and `pointer_wanted` walks the whole run's mail, so an
                 * unattended pane already named for this mail turns back
                 * here — one walk per message rather than one per beat, which
                 * is the cost that walk was written to avoid. */
                let Some(newest) = run.newest_pending(&address).map(str::to_owned) else {
                    marks.remove(&key);
                    continue;
                };
                let standing = marks
                    .get(&key)
                    .filter(|stood| stood.newest == newest && stood.term == Some(term))
                    .map(|stood| stood.standing);
                if unattended.is_some() && standing == Some(Standing::Unattended) {
                    continue;
                }
                /* Spoken for in an earlier life of this window: the advice and
                 * its Enter both landed for this very message before a restart
                 * emptied the map above. Nothing more is owed, and saying it
                 * again would be the line the person keeps finding in their
                 * composer. */
                /* The hook road's turn to answer for this mail, if it has
                 * one standing. A pointer on the shelf is a knock either
                 * arriving now or never coming, and one already handed over
                 * is an answer this process cannot watch land — so the pass
                 * holds off for exactly as long as each of those can honestly
                 * take, and then takes it back and speaks the ordinary way.
                 * Two roads carrying the same sentence is a person reading
                 * the window's stutter; neither carrying it is mail nobody
                 * hears about, and only the second is unrecoverable. */
                match crate::orchestration_pointer_mailbox::collect_stale(
                    term,
                    &run.id,
                    &address,
                    &newest,
                    crate::orchestration_pointer_mailbox::HOOK_COLLECTION_GRACE,
                ) {
                    crate::orchestration_pointer_mailbox::Parked::Fresh => continue,
                    crate::orchestration_pointer_mailbox::Parked::Abandoned => {
                        note_pointer_uncollected(&run.id, &address, term);
                    }
                    crate::orchestration_pointer_mailbox::Parked::Empty => {}
                }
                if standing.is_none() && delivered_before(&run.id, &address, &newest) {
                    marks.insert(
                        key,
                        Pointed {
                            newest,
                            term: Some(term),
                            standing: Standing::Delivered,
                        },
                    );
                    continue;
                }
                /* Old mail is not news. A message that has waited longer than
                 * the news window belongs to `check`, which replays it whenever
                 * the reader asks; a pointer typed about it now would be the
                 * window announcing history into somebody's composer. Written
                 * down once, then held like a delivered pointer. */
                let stale = run.message(&newest).is_some_and(|message| {
                    now_ms.saturating_sub(message.created_ms) > POINTER_NEWS_WINDOW_MS
                });
                if stale {
                    if standing != Some(Standing::Delivered) {
                        note_pointer_stale(&run.id, &address, &newest);
                    }
                    marks.insert(
                        key,
                        Pointed {
                            newest,
                            term: Some(term),
                            standing: Standing::Delivered,
                        },
                    );
                    continue;
                }
                let Some(count) = run.pointer_wanted(&address, Some(&seat)) else {
                    marks.remove(&key);
                    continue;
                };
                /* Nothing may be typed here, and no beat will change that on
                 * its own — so the window says once that mail is waiting where
                 * its advice cannot reach, and leaves the mail to `check`,
                 * which owns delivery whether or not a pointer was ever made.
                 * A pointer is only a pointer; its failure is still news. */
                if let Some(why) = unattended {
                    note_pointer_silent(&run.id, &address, term, why);
                    marks.insert(
                        key,
                        Pointed {
                            newest,
                            term: Some(term),
                            standing: Standing::Unattended,
                        },
                    );
                    continue;
                }
                match standing {
                    // Said and submitted: this mail is spoken for.
                    Some(Standing::Delivered | Standing::Unsubmitted) => continue,
                    /* Pointed at this very mail already: the pump is carrying
                     * the line, and this beat reads what it has to say. A
                     * delivery that landed is spoken for; one the guard
                     * refused is written down once and offered again next
                     * beat; one that pasted and withheld its Enter is on the
                     * person's screen, which is as far as advice goes — the
                     * Enter is theirs now, and retyping the line would only
                     * stack a second copy under it. */
                    Some(Standing::Advised { noted }) => {
                        match advice_standing(&run.id, &address, term, &newest, noted) {
                            None => continue,
                            Some(None) => {
                                marks.remove(&key);
                            }
                            Some(Some(standing)) => {
                                marks.insert(
                                    key,
                                    Pointed {
                                        newest,
                                        term: Some(term),
                                        standing,
                                    },
                                );
                            }
                        }
                    }
                    // Held at a wall whose own window has not run out.
                    Some(Standing::Walled { until_ms, .. }) if now_ms < until_ms => continue,
                    /* New mail, a new pane, nothing yet — or an advice line
                     * that no road would carry, which is the same beat over
                     * again minus the black-box line it has already earned.
                     * `Unattended` lands here too, and lands as new: a pane
                     * whose person came back, or one that ended a turn without
                     * the window ever hearing it begin one, has just become
                     * pointable — and the line it earned while nobody was
                     * there was a different line about a different failure.
                     * `Withheld` is the same beat over again too: what the
                     * guard refused clears on its own, and the guard is the
                     * door that will know. A wall whose window ran out lands
                     * here as well, to be looked at again. */
                    Some(
                        Standing::Unreachable
                        | Standing::Withheld
                        | Standing::Unattended
                        | Standing::Seatless
                        | Standing::Walled { .. },
                    )
                    | None => {
                        /* The wall's door (t-6560), asked where this pass
                         * would type a FRESH line — new mail, a pane just
                         * become pointable, a wall whose window ran out — and
                         * never on a retry of a line the guard or the roads
                         * refused, which this same look already let through:
                         * the pane's record moves only with a turn, and a
                         * turn strikes the mark out. One bounded read of the
                         * pane's transcript, never once a beat. */
                        let was_walled = match standing {
                            Some(Standing::Walled { cause, .. }) => Some(cause),
                            _ => None,
                        };
                        let fresh = matches!(
                            standing,
                            None | Some(Standing::Unattended | Standing::Walled { .. })
                        );
                        if fresh
                            && let Some(wall) = host
                                .agent_of(term)
                                .and_then(|agent| host.pane_wall(term, &agent))
                                .filter(|wall| wall.stands(now_ms))
                        {
                            if was_walled.is_none() {
                                note_pointer_walled(&run.id, &address, term, &wall, now_ms);
                            }
                            marks.insert(
                                key,
                                Pointed {
                                    newest,
                                    term: Some(term),
                                    standing: Standing::Walled {
                                        until_ms: wall.stands_until_ms,
                                        cause: wall.cause,
                                    },
                                },
                            );
                            continue;
                        }
                        if let Some(cause) = was_walled {
                            note_pointer_wall_lifted(&run.id, &address, term, cause);
                        }
                        let notice = crate::orchestration_notify::has_route(term)
                            .then(|| {
                                zerocode_hookd::session_notify::PointerNotice::new(
                                    run.id.clone(),
                                    address.clone(),
                                    newest.clone(),
                                    count,
                                )
                            })
                            .flatten();
                        let text = notice.as_ref().map_or_else(
                            || zerocode_core::orchestration::pointer_text(count),
                            |notice| notice.text().to_string(),
                        );
                        pointing.push(Pointing {
                            run: run.id.clone(),
                            address: address.clone(),
                            term,
                            newest,
                            text,
                            notice,
                            unreachable_noted: matches!(
                                standing,
                                Some(Standing::Unreachable | Standing::Withheld)
                            ),
                        });
                    }
                }
            }
        }
    }
    for one in pointing {
        let key = (one.run, one.address);
        if one
            .notice
            .is_some_and(|notice| !crate::orchestration_notify::offer(one.term, notice).needs_pty())
        {
            continue;
        }
        /* A `cursor` composer submits what lands in it on its own, so the
         * Enter never follows — typed advice is the whole delivery there. */
        let submits_itself =
            host.agent_of(one.term).as_deref() == Some(zerocode_core::AgentKind::Cursor.slug());
        /* Through the guarded door, as one delivery. Nothing is written on
         * this beat; the receipt is read on the next ones. */
        let Some(receipt) = host.point(one.term, &one.text, !submits_itself) else {
            /* The fast road declined and the proven one refused, so this
             * pane has mail nothing told it about. Remembered as unreachable
             * — the next beat tries the same line again, and the black box
             * hears about it once. */
            if !one.unreachable_noted {
                note_pointer_silent(&key.0, &key.1, one.term, NO_ROAD_CARRIED_IT);
            }
            pointed()
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .insert(
                    key,
                    Pointed {
                        newest: one.newest,
                        term: Some(one.term),
                        standing: Standing::Unreachable,
                    },
                );
            continue;
        };
        pointer_receipts()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(key.clone(), receipt);
        /* Read once right away: the pump has not turned yet in the real
         * window, so this is `Pending` there and the next beats read it —
         * but a host that answers on the spot (a `send-keys` dialect, a
         * test's fake) is settled on this very beat, exactly as the raw
         * road used to be. */
        let standing =
            match advice_standing(&key.0, &key.1, one.term, &one.newest, one.unreachable_noted) {
                None => Standing::Advised {
                    noted: one.unreachable_noted,
                },
                Some(None) => continue,
                Some(Some(standing)) => standing,
            };
        pointed()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(
                key,
                Pointed {
                    newest: one.newest,
                    term: Some(one.term),
                    standing,
                },
            );
    }
}

/* ---- the handover walk (§2.3) ------------------------------------------ */

/// A current wall on one exact terminal incarnation. Never persisted as
/// authority: every destructive boundary asks the existing witnesses again.
pub(crate) struct HandoverWall {
    term: u32,
    capability: String,
    witness: HandoverWitness,
}

/// The two witnesses a walk re-reads before each step, for its cause: the
/// wall's (§2.2), whose numbers the receipt carries as read today, or the
/// decline's (t-6747) — which IS the plan's cause, re-proven the same at
/// every reading (`HandoverCause::is_witnessed_by`, t-7153), so nothing of
/// it travels here.
#[derive(Clone, Debug)]
pub(crate) enum HandoverWitness {
    Quota(zerocode_core::orchestration::QuotaWallWitness),
    Decline,
}

fn current_handover_wall(
    host: &dyn Host,
    plan: &zerocode_core::orchestration::HandoverPlan,
    now_ms: i64,
) -> Option<HandoverWall> {
    let held = runtime()?;
    let image = held.actor.view().ok()?;
    let rows = cached_ledger(&held, &image).ok()?;
    let run = rows.run(&plan.run)?;
    if !zerocode_core::orchestration::handover_order_current(run, plan, false) {
        return None;
    }
    let worker = run.worker(&plan.worker)?;
    let model = worker.model.clone();
    drop(rows);
    let (term, capability) = {
        let teams = crate::agent_teams::teams();
        let term = teams.get(&plan.worker_team)?.term_of(&plan.worker_pane)?;
        let capability =
            crate::agent_teams::current_pane_capability(&plan.worker_team, &plan.worker_pane)?;
        (term, capability)
    };
    // A decline's pause dialog stands behind a stale `working` hook: its
    // pane is quiet by the decline's own rule (t-6747).
    let quiet = || match &plan.cause {
        zerocode_core::orchestration::HandoverCause::QuotaWall { .. } => {
            host.quiet_since(term, plan.worker_started_ms, now_ms)
        }
        zerocode_core::orchestration::HandoverCause::ClassifierDecline { .. } => host
            .decline_quiet_since(
                term,
                plan.worker_started_ms,
                now_ms,
                zerocode_core::orchestration::DECLINE_DIALOG_UNANSWERED_MS,
            ),
    };
    let since_ms = quiet()?;
    let witness = match &plan.cause {
        zerocode_core::orchestration::HandoverCause::QuotaWall { .. } => {
            let marker = host.quota_wall_marker(term, &plan.agent)?;
            let headroom = usage_headroom(&held.usage, &plan.agent, model.as_deref());
            HandoverWitness::Quota(zerocode_core::orchestration::quota_wall_witness(
                &plan.worker,
                Some(marker),
                headroom.as_ref(),
                now_ms,
            )?)
        }
        zerocode_core::orchestration::HandoverCause::ClassifierDecline { .. } => {
            let reading = host.classifier_decline_reading(term, &plan.agent)?;
            let witness = zerocode_core::orchestration::classifier_decline_witness(
                &plan.worker,
                &plan.dispatch,
                reading.screen,
                reading.record,
                since_ms,
                now_ms,
            )?;
            // The decline the plan was made on, and no other (t-7153): the
            // same record, the same routed category, never the screen alone.
            if !plan.cause.is_witnessed_by(&witness) {
                return None;
            }
            HandoverWitness::Decline
        }
    };
    // Activity and respawn may race the marker/cache reads. Ask quiet again
    // and prove both host and ledger identities after the probes return.
    quiet()?;
    let image = held.actor.view().ok()?;
    let rows = cached_ledger(&held, &image).ok()?;
    if !zerocode_core::orchestration::handover_order_current(rows.run(&plan.run)?, plan, false) {
        return None;
    }
    drop(rows);
    let mut teams = crate::agent_teams::teams();
    crate::agent_teams::authorized_team_mut(
        &mut teams,
        &plan.worker_team,
        &plan.worker_pane,
        &capability,
    )
    .filter(|team| team.term_of(&plan.worker_pane) == Some(term))?;
    Some(HandoverWall {
        term,
        capability,
        witness,
    })
}

fn settle_handover_terminal(
    host: &dyn Host,
    actor: &RuntimeActor,
    decided: &Decided,
    plan: &zerocode_core::orchestration::HandoverPlan,
    term: u32,
    now_ms: i64,
) -> Result<(WorkerState, u64), RuntimeError> {
    let held = runtime().ok_or(RuntimeError::Closed)?;
    let image = actor.view()?;
    let rows = cached_ledger(&held, &image).map_err(|_| RuntimeError::AuthorityRejected)?;
    let model = rows
        .run(&plan.run)
        .and_then(|run| run.worker(&plan.worker))
        .ok_or(RuntimeError::AuthorityRejected)?
        .model
        .clone();
    drop(rows);
    actor.worker_terminal_settled_fenced(decided.effect.clone(), None, now_ms, |commit| {
        match &plan.cause {
            zerocode_core::orchestration::HandoverCause::QuotaWall { .. } => host
                .with_quota_wall_observation(
                    term,
                    plan.worker_started_ms,
                    &plan.agent,
                    &mut |marker| {
                        with_usage_headroom(
                            &held.usage,
                            &plan.agent,
                            model.as_deref(),
                            |headroom| {
                                // Time of use, after the actor mailbox and the
                                // witness locks.
                                let at_ms = crate::now_epoch_ms();
                                if zerocode_core::orchestration::quota_wall_witness(
                                    &plan.worker,
                                    Some(marker),
                                    headroom.as_ref(),
                                    at_ms,
                                )
                                .is_some()
                                {
                                    commit(at_ms);
                                }
                            },
                        );
                    },
                ),
            zerocode_core::orchestration::HandoverCause::ClassifierDecline { .. } => host
                .with_classifier_decline_observation(
                    term,
                    plan.worker_started_ms,
                    &plan.agent,
                    &mut |reading, since_ms| {
                        let at_ms = crate::now_epoch_ms();
                        // The last boundary re-proves the SAME decline the
                        // plan was made on (t-7153): a witness that changed
                        // — its category gone or another, another request's
                        // record, the screen alone — commits nothing, and the
                        // beat plans again from what it reads next.
                        if let Some(witness) =
                            zerocode_core::orchestration::classifier_decline_witness(
                                &plan.worker,
                                &plan.dispatch,
                                reading.screen,
                                reading.record,
                                since_ms,
                                at_ms,
                            )
                            && plan.cause.is_witnessed_by(&witness)
                        {
                            commit(at_ms);
                        }
                    },
                ),
        }
    })
}

fn handover_order_still_current(
    actor: &RuntimeActor,
    plan: &zerocode_core::orchestration::HandoverPlan,
    capability: &str,
    stopped: bool,
) -> bool {
    let Ok(image) = actor.view() else {
        return false;
    };
    let Some(held) = runtime() else {
        return false;
    };
    let Ok(rows) = cached_ledger(&held, &image) else {
        return false;
    };
    if !rows
        .run(&plan.run)
        .is_some_and(|run| zerocode_core::orchestration::handover_order_current(run, plan, stopped))
    {
        return false;
    }
    drop(rows);
    let mut teams = crate::agent_teams::teams();
    crate::agent_teams::authorized_team_mut(&mut teams, &plan.team, &plan.pane, capability)
        .is_some()
}

/// The two things a handover does to the world, behind one door so a test
/// can stand in for either: a commit in a checkout, and a verb through the
/// ledger's one road from the coordinator's seat.
pub(crate) trait HandoverDoor {
    fn current_wall(
        &self,
        plan: &zerocode_core::orchestration::HandoverPlan,
        now_ms: i64,
    ) -> Option<HandoverWall>;

    /// Step ①: `Ok(None)` clean, `Ok(Some(sha))` committed, `Err` git's word.
    fn wip_commit(&self, checkout: &Path, message: &str) -> Result<Option<String>, String>;
    /// Between ① and ②, while the walled worker's pane still stands: its
    /// transcript's path and the bounded tail of it, for the recap the
    /// replacement is briefed with. A door that cannot see transcripts (every
    /// test door but one) answers nothing, and the walk goes on without it.
    fn predecessor_tail(
        &self,
        _plan: &zerocode_core::orchestration::HandoverPlan,
    ) -> Option<(String, Vec<String>)> {
        None
    }
    /// Steps ② and ③: one verb, presented with the seat's capability.
    fn verb(
        &self,
        team: &str,
        pane: &str,
        capability: &str,
        argv: &[String],
        now_ms: i64,
        authority: (&zerocode_core::orchestration::HandoverPlan, &HandoverWall),
    ) -> zerocode_hookd::TeamAnswer;
}

/// The production door: git in the checkout, [`run`] for the verbs.
struct LiveDoor<'a> {
    host: &'a dyn Host,
    overrides: &'a [(String, LaunchOverride)],
}

impl HandoverDoor for LiveDoor<'_> {
    fn current_wall(
        &self,
        plan: &zerocode_core::orchestration::HandoverPlan,
        now_ms: i64,
    ) -> Option<HandoverWall> {
        current_handover_wall(self.host, plan, now_ms)
    }

    fn wip_commit(&self, checkout: &Path, message: &str) -> Result<Option<String>, String> {
        crate::quota_wall::wip_commit(checkout, message)
    }

    fn predecessor_tail(
        &self,
        plan: &zerocode_core::orchestration::HandoverPlan,
    ) -> Option<(String, Vec<String>)> {
        // The worker's pane, read off the team table and the table let go
        // before the host is asked anything — the same order the stall probe
        // keeps (`index_team_seats`).
        let term = {
            let teams = crate::agent_teams::teams();
            let seats = index_team_seats(&teams);
            seats
                .get(plan.worker_team.as_str())?
                .get(plan.worker_pane.as_str())
                .copied()?
        };
        let path = self.host.provider_session(term)?.transcript_path?;
        let tail = zerocode_core::transcript::tail_lines(Path::new(&path))?;
        Some((path, tail))
    }

    fn verb(
        &self,
        team: &str,
        pane: &str,
        capability: &str,
        argv: &[String],
        now_ms: i64,
        authority: (&zerocode_core::orchestration::HandoverPlan, &HandoverWall),
    ) -> zerocode_hookd::TeamAnswer {
        run_from_seat(
            self.host,
            self.overrides.to_vec(),
            (team, pane, capability),
            argv,
            now_ms,
            Some(authority),
        )
    }
}

/// What one beat's handover walk did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Walked {
    /// No wall under a standing order, no seat to present, or a reservation
    /// the ledger refused — nothing moved.
    Nothing,
    /// Every step walked; the replacement sits.
    Done {
        handover: String,
        replacement: String,
    },
    /// A step refused; the receipt says which and why.
    Failed { handover: String, at: String },
}

/// The beat's half: plan from a snapshot, then walk one handover.
///
/// One per beat, for the standing order's O(N²) reason and one more: a walk
/// is a commit and two verbs, and the second wall can wait a second. The
/// seat is the run's live coordinator seat, presented like the standing
/// order presents its own (`beat`); a seat whose capability is gone is a
/// pane that is gone, and nothing walks until somebody sits.
fn walk_handovers(host: &dyn Host, overrides: &[(String, LaunchOverride)], now_ms: i64) -> Walked {
    let Some(held) = runtime() else {
        return Walked::Nothing;
    };
    let Ok(image) = held.actor.view() else {
        return Walked::Nothing;
    };
    let Ok(rows) = cached_ledger(&held, &image) else {
        return Walked::Nothing;
    };
    let mut candidates = Vec::new();
    for run in rows.runs() {
        let _ = zerocode_core::orchestration::next_handover_witnessed(run, now_ms, |plan| {
            candidates.push(plan.clone());
            false
        });
    }
    drop(rows);
    let Some(plan) = candidates
        .into_iter()
        .find(|plan| current_handover_wall(host, plan, now_ms).is_some())
    else {
        return Walked::Nothing;
    };
    let Some(capability) = crate::agent_teams::current_pane_capability(&plan.team, &plan.pane)
    else {
        return Walked::Nothing;
    };
    let door = LiveDoor { host, overrides };
    walk_handover(&held.actor, &door, &plan, &capability, now_ms)
}

/// Walk one handover: reserve → ① WIP commit → ② `worker-stop` → ③
/// `worker-start --retry-of … --inherit-checkout` → settle (§2.3).
///
/// The order is structural, not stylistic: `--retry-of` follows an ENDED
/// attempt only, so the stop comes before the start, and the tree is
/// committed before the worker whose tree it is has its pane closed. Every
/// step lands on the receipt as it happens, so a window that dies between
/// two of them leaves a row that says so; a refused step ends the walk and
/// settles the receipt `failed` — the coordinator reads what stopped it. The
/// two verbs are argv through [`HandoverDoor::verb`] and nothing else: no
/// second ledger API, the same names a person would type, each with its own
/// `--retry-request` so a walk asked twice is answered once.
pub(crate) fn walk_handover(
    actor: &RuntimeActor,
    door: &dyn HandoverDoor,
    plan: &zerocode_core::orchestration::HandoverPlan,
    capability: &str,
    now_ms: i64,
) -> Walked {
    use zerocode_core::orchestration::{
        HANDOVER_STEPS, HandoverStep, Text, handover_paragraph, handover_recap,
    };
    let began = std::time::Instant::now();
    let current_time =
        || now_ms.saturating_add(i64::try_from(began.elapsed().as_millis()).unwrap_or(i64::MAX));
    let Some(wall) = door.current_wall(plan, current_time()) else {
        return Walked::Nothing;
    };
    if !handover_order_still_current(actor, plan, capability, false) {
        return Walked::Nothing;
    }
    // Receipt/briefing describe today's witness, while the retained mail is
    // left exactly as it was. The declaration and seats remain unchanged.
    let mut current = plan.clone();
    match &wall.witness {
        HandoverWitness::Quota(witness) => {
            current.cause = zerocode_core::orchestration::HandoverCause::QuotaWall {
                provider: witness.headroom.provider.clone(),
                used_percent: witness.headroom.used_percent,
                resets_at_ms: witness.headroom.resets_at_ms,
            };
        }
        // A decline's witness is the plan's own or the wall above answered
        // nothing (`HandoverCause::is_witnessed_by`, t-7153): the same
        // record in the same routed category, so the cause stands as read.
        HandoverWitness::Decline => {}
    }
    let plan = &current;
    // The reservation: refused, nothing moved and the next beat plans again.
    let Ok((Some(handover), _)) = actor.handover_begin(plan.clone(), now_ms) else {
        return Walked::Nothing;
    };
    let step = |name: &str, ok: bool, detail: String| {
        let _ = actor.handover_step(
            plan.run.clone(),
            handover.clone(),
            HandoverStep {
                name: name.to_string(),
                ok,
                detail: Text::from(detail),
            },
            now_ms,
        );
    };
    let failed = |at: &str| {
        let _ = actor.handover_settle(plan.run.clone(), handover.clone(), None, now_ms);
        Walked::Failed {
            handover: handover.clone(),
            at: at.to_string(),
        }
    };
    let first_line = |said: &str| said.lines().next().unwrap_or_default().to_string();
    let [wip, stop, start] = HANDOVER_STEPS;
    let authorized = || {
        door.current_wall(plan, current_time())
            .is_some_and(|current| {
                current.term == wall.term && current.capability == wall.capability
            })
            && handover_order_still_current(actor, plan, capability, false)
    };
    let revoked = |at: &str| {
        step(
            at,
            false,
            "current handover order, seat, or witness changed".into(),
        );
        let _ = actor.handover_revoke(plan.run.clone(), handover.clone(), current_time());
        Walked::Failed {
            handover: handover.clone(),
            at: at.into(),
        }
    };
    if !authorized() {
        return revoked(wip);
    }
    // ① The tree, before the worker whose tree it is loses its pane.
    let wip_sha = if plan.wip_commit {
        let message = crate::quota_wall::wip_message(
            &plan.worker,
            &plan.cause,
            crate::automation_runtime::local_offset_secs(),
        );
        match door.wip_commit(Path::new(&plan.checkout), &message) {
            Ok(Some(sha)) => {
                step(wip, true, format!("committed {sha} in {}", plan.checkout));
                Some(sha)
            }
            Ok(None) => {
                step(wip, true, "skipped: the checkout was clean".to_string());
                None
            }
            Err(why) => {
                step(wip, false, why);
                return failed(wip);
            }
        }
    } else {
        step(
            wip,
            true,
            "skipped: not asked (handover-policy --wip-commit arms it); the tree is left as \
             it is"
                .to_string(),
        );
        None
    };
    // Where it left off, read while its pane still stands: ② closes the pane
    // and, with it, the window's record of which transcript was its own.
    let recap = door
        .predecessor_tail(plan)
        .and_then(|(transcript, tail)| handover_recap(&transcript, &tail));
    if !authorized() {
        return revoked(stop);
    }
    // ② The attempt ends — now `--retry-of` will take it.
    let stopped = door.verb(
        &plan.team,
        &plan.pane,
        capability,
        &[
            "worker-stop".to_string(),
            "--worker".to_string(),
            plan.worker.clone(),
            "--reason".to_string(),
            plan.cause.reason().to_string(),
            "--retry-request".to_string(),
            format!("handover-{}-stop", plan.dispatch),
        ],
        current_time(),
        (plan, &wall),
    );
    if stopped.exit_code != 0 {
        if actor.view().ok().is_some_and(|image| {
            image
                .projection()
                .dispatches
                .iter()
                .any(|dispatch| dispatch.id == plan.dispatch && dispatch.ended_ms.is_none())
        }) {
            return revoked(stop);
        }
        step(stop, false, first_line(&stopped.stderr));
        return failed(stop);
    }
    step(
        stop,
        true,
        format!("dispatch {} ended; the pane was closed", plan.dispatch),
    );
    // ③ The replacement, in the same checkout, linked to the ended attempt,
    // briefed as a continuation.
    let mut argv = vec![
        "worker-start".to_string(),
        "--agent".to_string(),
        plan.to.agent.clone(),
    ];
    if let Some(model) = plan.to.model.as_ref() {
        argv.push("--model".to_string());
        argv.push(model.clone());
        if let Some(effort) = plan.to.effort.as_ref() {
            argv.push("--effort".to_string());
            argv.push(effort.clone());
        }
    }
    argv.extend([
        "--task".to_string(),
        plan.task.clone(),
        "--retry-of".to_string(),
        plan.dispatch.clone(),
        "--inherit-checkout".to_string(),
        "--retry-request".to_string(),
        format!("handover-{}-start", plan.dispatch),
        "--prompt".to_string(),
        format!(
            "{}{}",
            handover_paragraph(plan, wip_sha.as_deref(), recap.as_ref(), now_ms),
            plan.spec.as_str()
        ),
    ]);
    if !handover_order_still_current(actor, plan, capability, true) {
        step(
            start,
            false,
            "handover order or coordinator changed after stop; task remains ready".into(),
        );
        return failed(start);
    }
    let started = door.verb(
        &plan.team,
        &plan.pane,
        capability,
        &argv,
        current_time(),
        (plan, &wall),
    );
    if started.exit_code != 0 {
        step(start, false, first_line(&started.stderr));
        return failed(start);
    }
    let said: serde_json::Value = serde_json::from_str(&started.stdout).unwrap_or_default();
    let replacement = said["workerId"].as_str().unwrap_or_default().to_string();
    let dispatch = said["dispatchId"].as_str().unwrap_or_default().to_string();
    if replacement.is_empty() || dispatch.is_empty() {
        step(
            start,
            false,
            "the ledger answered without a worker id".to_string(),
        );
        return failed(start);
    }
    step(
        start,
        true,
        format!("{replacement} on {dispatch} in {}", plan.checkout),
    );
    let _ = actor.handover_settle(
        plan.run.clone(),
        handover.clone(),
        Some((replacement.clone(), dispatch)),
        now_ms,
    );
    Walked::Done {
        handover,
        replacement,
    }
}

/// The crash that becomes a task, walked down the ledger's one door
/// (t-3014 §2.4).
///
/// The same road the standing-order beat takes for `worker-start`, for the
/// same reason (`agent_teams.rs:293`): the window has no request behind it,
/// so it reads a seat's own capability and presents it like anybody else.
/// The seat is a coordinator's — the leader pane of a team standing in this
/// window that holds a run's seat — because `task-create` lands in the run
/// its caller is standing in, and a task nobody is coordinating is a row
/// nobody reads. Nothing here reaches past `run`: the argv is the whole of
/// what this function knows how to say to the ledger.
///
/// `Nothing` when no incident wants filing — none shown, a kill, already
/// stamped, stale; the item is the report id, which is also the request
/// name, and `triaged.json` beside the report is the stamp.
pub(crate) fn file_crash_task(
    host: &dyn Host,
    overrides: &[(String, LaunchOverride)],
    local_data_root: &Path,
    now_ms: i64,
) -> Triage {
    let Some(pending) = crate::crash::pending_triage(local_data_root, now_ms.max(0) as u64) else {
        return Triage::Nothing;
    };
    let answered = file_task_through_seat(host, overrides, &pending.argv, now_ms);
    crate::seat_triage::triage(pending.report, answered, |report, task| {
        crate::crash::note_triaged(local_data_root, report, task)
    })
}

/// Why a task could not be filed through the seat — sorted by what the
/// caller should do: wait a beat, give up for this boot, or count a refusal
/// (`seat_triage` does each).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeatFiling {
    Waiting(String),
    Unavailable(String),
    Refused(String),
}

/// File a `task-create` through the newest seated coordinator whose leader
/// pane this window can sign for (the one argv door), and answer the task
/// id the ledger gave. The crash, QA and scoreboard roads all file here.
pub(crate) fn file_task_through_seat(
    host: &dyn Host,
    overrides: &[(String, LaunchOverride)],
    argv: &[String],
    now_ms: i64,
) -> Result<String, SeatFiling> {
    if let Some(why) = unavailable() {
        return Err(SeatFiling::Unavailable(why));
    }
    let Some(held) = runtime() else {
        return Err(SeatFiling::Unavailable(
            "the runtime never started".to_string(),
        ));
    };
    let Ok(image) = held.actor.view() else {
        return Err(SeatFiling::Waiting(
            "the ledger could not be read".to_string(),
        ));
    };
    let Ok(rows) = cached_ledger(&held, &image) else {
        return Err(SeatFiling::Waiting(
            "the ledger could not be rebuilt".to_string(),
        ));
    };
    // The newest seated coordinator whose leader pane this window can sign
    // for. A seat proven gone (`vacated_ms`) is a chair, not a caller.
    let mut seat: Option<(i64, String, String, String)> = None;
    for run in rows.runs() {
        let Some(held_seat) = run
            .coordinator
            .as_ref()
            .filter(|one| one.vacated_ms.is_none())
        else {
            continue;
        };
        let Some((team, pane)) = held_seat.seat.split_once('/') else {
            continue;
        };
        let Some(capability) = crate::agent_teams::current_pane_capability(team, pane) else {
            continue;
        };
        if seat
            .as_ref()
            .is_none_or(|(since, ..)| held_seat.since_ms > *since)
        {
            seat = Some((
                held_seat.since_ms,
                team.to_string(),
                pane.to_string(),
                capability,
            ));
        }
    }
    drop(rows);
    let Some((_, team, pane, capability)) = seat else {
        return Err(SeatFiling::Waiting(
            "no seated coordinator in this window yet".to_string(),
        ));
    };
    let answered = run(
        host,
        overrides.to_vec(),
        &team,
        &pane,
        &capability,
        argv,
        now_ms,
    );
    if answered.exit_code != 0 {
        return Err(SeatFiling::Refused(
            answered
                .stderr
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
        ));
    }
    serde_json::from_str::<serde_json::Value>(&answered.stdout)
        .ok()
        .and_then(|answer| answer["taskId"].as_str().map(str::to_string))
        .ok_or_else(|| SeatFiling::Refused("the ledger answered without a task id".to_string()))
}

/// The newest task still open — not final either way — whose name carries
/// `needle`, read off this window's own ledger view; `None` when there is
/// none, or no ledger to ask. The scoreboard inbox asks before it files, so a
/// regression that stays is the task already open for it (whichever road
/// filed that one), not a new task every refile window.
pub(crate) fn open_task_titled(needle: &str) -> Option<String> {
    if unavailable().is_some() {
        return None;
    }
    let held = runtime()?;
    let image = held.actor.view().ok()?;
    let ledger = cached_ledger(&held, &image).ok()?;
    open_task_titled_in(&ledger, needle)
}

fn open_task_titled_in(ledger: &Ledger, needle: &str) -> Option<String> {
    ledger
        .runs()
        .iter()
        .flat_map(|run| run.tasks.iter())
        .filter(|task| !task.status.is_final() && task.display_name().contains(needle))
        .max_by_key(|task| task.created_ms)
        .map(|task| task.id.clone())
}

/// What the beat calls its own request.
///
/// One function so a test can hold it to the property that matters: the SAME
/// string for the same summoning, a different one for the next attempt. A beat
/// has no agent to choose a name for it, and a name that changed between the
/// first try and the retry would be no name at all — the retry would cut a
/// second pane for work already being done.
fn beat_request_name(one: &zerocode_core::orchestration::Dispatchable) -> String {
    format!("beat-{}-{}", one.task, one.attempt)
}

/// One beat, with the door already held.
fn beat(host: &dyn Host, overrides: &[(String, LaunchOverride)], now_ms: i64) -> usize {
    #[cfg(test)]
    tests::a_beat_has_begun();
    // The plans are taken from a snapshot and carried out against the doors:
    // the image is the actor's answer at one revision, and a dispatch that
    // goes stale between this look and the verb below earns the verb's own
    // refusal, which is the correct answer to a beat planned against a world
    // that moved. The rebuild is a validation walk over rows this window's
    // own actor wrote — a snapshot's cost, paid once per beat, not per verb.
    let Some(held) = runtime() else {
        return 0;
    };
    let actor = &held.actor;
    let Ok(image) = actor.view() else {
        return 0;
    };
    let Ok(rows) = cached_ledger(&held, &image) else {
        return 0;
    };
    let planned: Vec<zerocode_core::orchestration::Dispatchable> = rows
        .runs()
        .iter()
        .filter_map(zerocode_core::orchestration::next_dispatch)
        .collect();
    let mut summoned = 0;
    for one in planned {
        // The verb, built element by element rather than as a line: a task's
        // spec is a sentence, and a line split on whitespace would hand the
        // worker the first word of its own instructions.
        // The beat's own name for this summoning. It has no agent to choose
        // one for it, and the road now requires a name from everything that
        // changes the ledger — for the beat above all, because a beat whose
        // save failed comes back on the very next tick and would otherwise cut
        // a SECOND pane for work already being done. Task plus attempt is the
        // same string for the same summoning and a different one for the next.
        let named = beat_request_name(&one);
        let argv = vec![
            "worker-start".to_string(),
            "--agent".to_string(),
            one.agent.clone(),
            "--task".to_string(),
            one.task.clone(),
            "--retry-request".to_string(),
            named,
            "--prompt".to_string(),
            one.spec.as_str().to_string(),
        ];
        // The beat has no request behind it, so it reads the seat's own
        // capability and presents it like anybody else. Cloned, with the token
        // guard given back before `run` takes the team table — the other order
        // is the deadlock `forget_term` would win. A seat whose capability is
        // gone is a pane that is gone: nothing is summoned. And a capability
        // that rotates between here and there earns a refusal inside `run`,
        // which is the correct answer to a beat planned against a pane that has
        // since been respawned.
        let Some(capability) = crate::agent_teams::current_pane_capability(&one.team, &one.pane)
        else {
            continue;
        };
        // The identity is not the beat's business: it goes down the same road
        // a typed verb does, and that road asks the host under its own guard.
        // A seat whose agent has not reported yet is refused there, which is
        // the same answer a coordinator would get.
        let answered = run(
            host,
            overrides.to_vec(),
            &one.team,
            &one.pane,
            &capability,
            &argv,
            now_ms,
        );
        if answered.exit_code == 0 {
            summoned += 1;
        }
        // A refusal is left where it fell. There is nobody to hand it to — the
        // coordinator did not ask for this one — and the ledger has already
        // written down whatever it decided. The next beat asks again, and the
        // three-attempt circuit is what stops that being forever.
    }
    summoned
}

/* `seat_of` used to live here, and it is gone rather than merely unused.
 *
 * It answered "which team's pane is this terminal" by taking the team table,
 * reading one row out of it, and GIVING IT BACK — so every caller settled
 * against a seat it no longer held. That is the gap a `respawn-pane` fits in,
 * and both callers now read the seat under a guard they keep through the write.
 * Left as a helper it is a ready-made way to reintroduce the bug in the next
 * road that needs a seat.
 */

/// Whether this command line is the ledger's to answer.
///
/// Asked of the verb table itself rather than of a second list kept in step
/// with it: a verb added to [`VERBS`] is answerable the moment it is added, and
/// one that was only added to a private list here would be advertised by `help`
/// and refused by the door.
///
/// The two vocabularies cannot collide. tmux's verbs are hyphenated around a
/// noun it owns (`split-window`, `capture-pane`, `send-keys`); the ledger's are
/// hyphenated around nouns tmux has never heard of (`run-`, `task-`,
/// `worker-`). The one bare word each has — `send` here, `send-keys` there — is
/// not the same word.
pub fn speaks_here(argv: &[String]) -> bool {
    argv.first()
        .is_some_and(|verb| VERBS.iter().any(|(name, _, _)| name == verb))
}

/// Builds a worker's command line out of the agent catalog.
///
/// Constructed with the person's own launch overrides rather than reading the
/// defaults, and that is a decision worth naming: [`launch_plan`] with no
/// override hands back the measured YOLO arguments, so a catalog built without
/// them would give every coordinator-summoned worker
/// `--dangerously-skip-permissions` — a permission answer nobody was asked for,
/// arrived at by omission. A worker gets exactly what a launch from the window
/// would give the same agent.
pub struct Catalog {
    overrides: Vec<(String, LaunchOverride)>,
}

impl Catalog {
    pub fn new(overrides: Vec<(String, LaunchOverride)>) -> Self {
        Self { overrides }
    }

    fn override_for(&self, agent: &str) -> Option<&LaunchOverride> {
        self.overrides
            .iter()
            .find(|(id, _)| id == agent)
            .map(|(_, held)| held)
    }
}

impl Launcher for Catalog {
    fn choose_agent(
        &self,
        look: &zerocode_core::summon_choice::SummonLook<'_>,
        options: &[zerocode_core::summon_choice::Summonable],
    ) -> Option<String> {
        summon_choice::choose(look, options)
    }

    fn command_for(&self, agent: &str, prompt: &str, tuning: &[String]) -> Result<String, String> {
        let Some(spec) = agent_spec(agent) else {
            return Err(format!("no agent is called {agent}"));
        };
        if !spec.runs_on(std::env::consts::OS) {
            return Err(format!("{agent} does not run on {}", std::env::consts::OS));
        }
        let mut words = vec![spec.launch.to_string()];
        words.extend(launch_plan(agent, self.override_for(agent)).args);
        // The ledger's launch tuning — `--model`, and effort where the agent
        // takes one — rides after the launch flags and BEFORE the prompt:
        // past the separator it would be part of the instruction, and an
        // agent would read its own model id as work.
        words.extend(tuning.iter().cloned());
        match prompt_injection(spec, prompt) {
            Some(Injected::Argv(said)) => words.extend(said),
            // The typed road needs the readiness machine, which belongs to the
            // launcher and is not on this road yet. Refused rather than
            // silently dropped: a worker started without the instruction it was
            // summoned for would sit at an empty composer looking exactly like
            // a worker that is thinking.
            Some(Injected::AfterStart(_)) => {
                return Err(format!(
                    "{agent} takes its prompt after it starts — start it bare and speak to it"
                ));
            }
            None => {}
        }
        // This line is parsed back into argv by the native PTY launcher, not
        // by a POSIX shell. The codec has to be that parser's exact inverse:
        // `resume_word` is shell quoting and its apostrophe escape was split
        // into many Claude positional arguments on this road.
        Ok(join_command_line(&words))
    }

    fn command_for_resume(
        &self,
        agent: &str,
        session: &zerocode_core::ProviderSession,
        nudge: &str,
        tuning: &[String],
    ) -> Result<String, String> {
        let Some(spec) = agent_spec(agent) else {
            return Err(format!("no agent is called {agent}"));
        };
        if !spec.runs_on(std::env::consts::OS) {
            return Err(format!("{agent} does not run on {}", std::env::consts::OS));
        }
        let command = crate::cmd::board::resume_command(
            agent,
            session,
            (!nudge.is_empty()).then_some(nudge),
            tuning,
            self.override_for(agent),
        )?;
        Ok(join_command_line(&command.argv))
    }

    /// What this machine has, measured against the PATH a worker would be
    /// LAUNCHED with.
    ///
    /// That is the whole point of taking the hydrated shell PATH here:
    /// `hooks::pty_env` hands the child `shell_path::hydrated()` — or the
    /// process's own when hydration has not landed — so this answer and the
    /// launch resolve the same binary against the same list. Measuring some
    /// other PATH would answer honestly about a machine nobody is going to
    /// launch on.
    ///
    /// `hydrated`, never `hydrate`: this runs inside the ledger actor, which
    /// holds the ledger while it plans, and hydration spawns a login shell
    /// with a five-second ceiling. A verb that answers late is a verb that
    /// blocks every other verb, so the un-hydrated case falls back to the
    /// process PATH exactly as a launch before hydration does. The window's
    /// own picker asks `detected_agents`, which MAY wait — that is a person
    /// pressing a button, not a verb holding a lock.
    fn presence(&self) -> Option<Vec<zerocode_core::AgentPresence>> {
        machine_presence()
    }

    /// The same peek [`LiveCatalog`] takes: one snapshot, whichever launcher
    /// answers the verb.
    fn readiness(&self, agent: &str) -> Option<zerocode_core::readiness::AgentReadinessSnapshot> {
        crate::readiness_runtime::observe(agent)
    }
}

/// The measurement behind [`Launcher::presence`], as a free function because
/// two launchers answer with it and neither of them varies it: launch
/// overrides change how an agent is STARTED, never whether it is here.
///
/// `None` — nobody looked — is reachable: a process with no `PATH` at all and
/// no hydrated answer has nothing to resolve a name against, and saying "not
/// installed" for all thirty-six would be an invention.
fn machine_presence() -> Option<Vec<zerocode_core::AgentPresence>> {
    let path = crate::shell_path::launch_path()?;
    Some(zerocode_core::agent_presence(
        Some(path.as_os_str()),
        std::env::consts::OS,
    ))
}

/// Carry out one ledger verb, and say what the shim should print.
///
/// The shape [`crate::agent_teams::run`] already has, for the same reason: an
/// agent is blocked on the answer, and what it reads decides whether it
/// believes a worker started.
///
/// Both locks are held across the plan AND the write-down, and dropped before
/// anything that could reach back into them. That is not caution, it is a
/// measured bug: a close called under the team lock walks
/// `forget_term_state → forget_term`, which takes that same `Mutex`, and froze
/// the whole window once (live report 2026-08-17).
pub fn run(
    host: &dyn Host,
    overrides: Vec<(String, LaunchOverride)>,
    team_id: &str,
    pane: &str,
    pane_token: &str,
    argv: &[String],
    now_ms: i64,
) -> zerocode_hookd::TeamAnswer {
    run_from_seat(
        host,
        overrides,
        (team_id, pane, pane_token),
        argv,
        now_ms,
        None,
    )
}

fn run_from_seat(
    host: &dyn Host,
    overrides: Vec<(String, LaunchOverride)>,
    seat: (&str, &str, &str),
    argv: &[String],
    now_ms: i64,
    authority: Option<(&zerocode_core::orchestration::HandoverPlan, &HandoverWall)>,
) -> zerocode_hookd::TeamAnswer {
    let answered = run_seated(host, overrides, seat, argv, now_ms, authority);
    /* One door out for the one verb whose failures are structured.
     *
     * A refusal can be written in five places before this — the window being
     * down, the capability, the actor, the plan, the carrying of the effect —
     * and all but the last speak the sentence every other verb speaks. Asking
     * each of them to know which verb they are refusing would be five copies
     * of one exception; this is the single place that knows it. */
    match argv.first().map(String::as_str) {
        Some(zerocode_core::orchestration::WORKTREE_EVIDENCE_VERB) => {
            crate::worktree_evidence_runtime::structured_refusal(answered)
        }
        _ => answered,
    }
}

fn run_seated(
    host: &dyn Host,
    overrides: Vec<(String, LaunchOverride)>,
    seat: (&str, &str, &str),
    argv: &[String],
    now_ms: i64,
    authority: Option<(&zerocode_core::orchestration::HandoverPlan, &HandoverWall)>,
) -> zerocode_hookd::TeamAnswer {
    let (team_id, pane, pane_token) = seat;
    if argv.first().is_some_and(|verb| {
        matches!(
            verb.as_str(),
            zerocode_core::orchestration::coordinator_handover::POLICY_VERB
                | zerocode_core::orchestration::coordinator_handover::APPLY_VERB
                | zerocode_core::orchestration::coordinator_handover::CLAIM_VERB
        )
    }) {
        return refused("coordinator handover requires a human declaration through the window");
    }
    if let Some(verb) = argv.first().filter(|_| speaks_here(argv)) {
        crate::crumbs::record("ledger", format_args!("{verb}"));
    }
    /* Fail closed, and fail BEFORE the plan.
     *
     * Not after: a refusal written at the bottom of this function would already
     * have summoned panes, killed terminals and moved the ledger, and "the
     * change did not reach the disk" would be the smallest part of what had
     * gone wrong. The host is not called, the actor is not asked, and the
     * agent is told what actually ends this.
     */
    if let Some(why) = unavailable() {
        return refused(why);
    }
    /* The address-book verbs live HERE, before any seat is proved: they
     * touch the machine's federation files, never the ledger, and the
     * transport token that admitted this request is the trust boundary the
     * files themselves live behind — the same secret every launched pane
     * already carries in its environment. */
    if let Some(said) = federation_book_verbs(argv) {
        return said;
    }
    let Some(held) = runtime() else {
        return refused("orchestration is unavailable in this window — the runtime never started");
    };
    let (actor, live_overrides) = (&held.actor, &held.overrides);
    // The launcher reads these at plan time, inside the actor; written before
    // the plan so a settings change the person just made reaches the very
    // next worker.
    *live_overrides
        .lock()
        .unwrap_or_else(|held| held.into_inner()) = overrides;
    /* WHO is in that seat, read here and pinned by the token.
     *
     * The capability proves the pane; it does not prove the incarnation. This
     * read happens one hop before the actor plans, and the gap is closed by
     * the presented token: the actor re-verifies it under the SAME table-lock
     * generation that plans, and a respawn rotates the token — so a caller
     * whose incarnation changed in the hop is refused before anything is
     * planned under the wrong name. What remains is an agent handing its seat
     * to another agent inside one incarnation, which is the same one-hop
     * staleness the old single-guard read had on the other side of its plan.
     *
     * NEVER call into the actor while holding this table: the actor takes it
     * itself, and a caller that held it while waiting on the mailbox would
     * deadlock the window. The guard ends at the end of this statement.
     */
    let caller = crate::agent_teams::teams()
        .get(team_id)
        .and_then(|team| team.term_of(pane))
        .and_then(|term| host.actor_for(term));
    let command =
        match PlanCommand::checked(argv.to_vec(), team_id, pane, pane_token, caller, now_ms) {
            Ok(command) => command,
            Err(why) => return refused_by_runtime(why),
        };
    /* What comes back has already been carried out against the rows AND made
     * durable — a verb the disk refused arrives here as an error, and its
     * decision is never seen. The window's remaining half is the effect. */
    let command = match authority {
        Some((plan, wall)) => match command.for_handover(
            plan.clone(),
            zerocode_core::agent_teams::PaneIncarnation {
                term: wall.term,
                capability: wall.capability.clone(),
            },
        ) {
            Ok(command) => command,
            Err(why) => return refused_by_runtime(why),
        },
        None => command,
    };
    let decided = match actor.plan(command) {
        Ok((decided, _)) => *decided,
        Err(why) => return refused_by_runtime(why),
    };
    rang(decided.requires_durability);
    let mut answered = carried(host, actor, decided, team_id, pane, pane_token, now_ms);
    // The window's own half of a worker observation, laid over the answer on
    // the way out. `seat` first — it resolves a foreign row's terminal, which
    // `agentWait` then reads — and `agentWait` is a hook fact no ledger row
    // carries.
    garnish_seats(host, argv.first().map(String::as_str), &mut answered);
    garnish_agent_wait(argv.first().map(String::as_str), &mut answered);
    garnish_federation_help(argv.first().map(String::as_str), &mut answered);
    garnish_artifacts(argv.first().map(String::as_str), &mut answered);
    note_worker_report(argv, team_id, pane, &answered, now_ms);
    answered
}

/// Lay the artifact store's rows over a worker-observation answer (t-2720
/// §4): `artifacts`, the ids this worker left, newest first, and `report`,
/// the newest report among them — the id 「보고서 보기」 opens. The ledger's
/// rows travel untouched; a window without a store adds nothing.
fn garnish_artifacts(verb: Option<&str>, reply: &mut zerocode_hookd::TeamAnswer) {
    if reply.exit_code != 0 || !matches!(verb, Some("worker-show") | Some("worker-list")) {
        return;
    }
    let Some(store) = crate::artifact_runtime::store() else {
        return;
    };
    let Ok(mut answer) = serde_json::from_str::<serde_json::Value>(&reply.stdout) else {
        return;
    };
    let garnish = |worker: &mut serde_json::Value| {
        let Some(id) = worker["workerId"].as_str() else {
            return;
        };
        let rows = store.ids_for_worker(id);
        let report = rows
            .iter()
            .find(|(_, kind)| *kind == zerocode_core::artifact::ArtifactKind::Report)
            .map(|(id, _)| id.clone());
        worker["artifacts"] = serde_json::json!(rows.iter().map(|(id, _)| id).collect::<Vec<_>>());
        worker["report"] = serde_json::json!(report);
    };
    match answer["workers"].as_array_mut() {
        Some(workers) => workers.iter_mut().for_each(garnish),
        None => garnish(&mut answer),
    }
    reply.stdout = format!("{answer}\n");
}

/// A report a `send` names — `--payload {"reportPath":…}` first, an absolute
/// `.md` path in the body second — copied into the artifact store under the
/// origin the seat's ledger row vouches for (t-2720 §4: reports leave
/// `/tmp`). Read only after the send succeeded, so a refused message registers
/// nothing; best-effort, so a copy that failed leaves the answer as it was.
fn note_worker_report(
    argv: &[String],
    team_id: &str,
    pane: &str,
    reply: &zerocode_hookd::TeamAnswer,
    now_ms: i64,
) {
    if reply.exit_code != 0 || argv.first().map(String::as_str) != Some("send") {
        return;
    }
    let value = |flag: &str| {
        argv.iter()
            .position(|word| word == flag)
            .and_then(|at| argv.get(at + 1))
            .map(String::as_str)
    };
    let Some(path) =
        crate::artifact_runtime::report_path_in(value("--payload"), value("--body").unwrap_or(""))
    else {
        return;
    };
    let Some(store) = crate::artifact_runtime::store() else {
        return;
    };
    let origin = with_ledger_seats(|ledger, _| {
        ledger.runs().iter().find_map(|run| {
            run.worker_in_pane(team_id, pane)
                .map(|worker| crate::artifact_runtime::origin_of_worker(run, worker))
        })
    })
    .flatten()
    .unwrap_or_default();
    let _ = store.register_report(&path, origin, now_ms);
}

/// Carry out the EFFECT of one decision, and say what the shim should print.
///
/// The decision itself is behind this function — authenticated, planned and
/// made durable on the actor's thread, under the same table-lock generation
/// that verified the caller's capability. Every arm here is the window's
/// half: hosts, panes, sleeps, and the receipts that only become true once
/// those have happened.
fn carried(
    host: &dyn Host,
    actor: &RuntimeActor,
    decided: Decided,
    team_id: &str,
    pane: &str,
    pane_token: &str,
    now_ms: i64,
) -> zerocode_hookd::TeamAnswer {
    match decided.effect.clone() {
        Effect::None => match decided.waiting.clone() {
            None => {
                // A remote reservation's effect is the WIRE, not nothing:
                // walk it, and only a far answer that came back whole files
                // the receipt (the abort road unwinds the claim otherwise).
                if let Some(prepared) = decided.prepared_remote_start.clone() {
                    return federation_attach_from_home(actor, decided, prepared, now_ms);
                }
                // The effect for this verb is nothing at all, so it has
                // already "happened" — and its receipt was filed by the actor
                // in the same transaction as the plan, so a crash between the
                // answer and the record cannot separate them.
                answer(decided.reply)
            }
            // The inbox was empty and the caller would rather sleep than ask
            // again. A `check --ack D --wait` has already spent D — and the
            // actor made that spend durable before this decision came back,
            // which is what the pre-sleep save on this road used to buy.
            Some(waiting) => {
                /* One sleeper per address, first come. Two sleepers on one
                 * inbox would race the same batch: the bell wakes both, one
                 * takes the delivery, and the other's look re-serves the SAME
                 * open lease — two callers each holding "their" batch, one
                 * ack retiring both copies. The second caller is told to look
                 * without waiting instead; its `--ack`, if it carried one,
                 * was spent and made durable before this refusal, and the
                 * refusal says so rather than letting "refused" read as
                 * "nothing happened".
                 *
                 * A THREAD wait is exempt on the same ground the rule stands
                 * on: it leases nothing — a woken `ask` reads its answer out
                 * of the log — so two askers on one inbox cost two looks and
                 * no race, and neither takes the seat an inbox sleeper needs. */
                if waiting.thread.is_none()
                    && WaiterCard::occupied(&waiting.run, &waiting.address, &waiting.seat)
                {
                    return refused(match waiting.acked {
                        true => {
                            "this inbox already has a sleeper — the \
                             acknowledgement stood; check again without --wait, \
                             or let the sleeper drain the mail"
                        }
                        false => {
                            "this inbox already has a sleeper — check without \
                             --wait, or let the sleeper drain the mail"
                        }
                    });
                }
                let mut seen = mail_seen();
                // Past the first empty look: this caller is now certainly
                // going to sleep. The one test that has to land a message
                // INTO a sleeping wait counts this rather than guessing with
                // a clock — a sleep long enough "under load" is not a proof,
                // and a message that arrives before the wait begins is
                // answered by the first look, which is the old deadlock
                // passing by luck.
                #[cfg(test)]
                tests::a_wait_has_begun();
                // The pointer pass skips an address somebody already sleeps
                // on — the bell will answer them with the mail itself. Held
                // as a card so an unwind gives the seat back. A thread wait
                // holds no card: it is not the sleeper the pointer defers to,
                // and it must not make the next `check --wait` a "second"
                // sleeper on an inbox nobody is actually draining.
                let _waiting_here = waiting
                    .thread
                    .is_none()
                    .then(|| WaiterCard::hold(&waiting.run, &waiting.address, &waiting.seat));
                // The caller's own budget when it brought one — already
                // clamped by the ledger to a range the bridge and the shim
                // outwait — and the window's short default when it did not.
                let deadline = std::time::Instant::now()
                    + waiting.deadline_ms.map_or(
                        std::time::Duration::from_secs(u64::from(WAIT_SECONDS)),
                        |budget| std::time::Duration::from_millis(u64::from(budget)),
                    );
                loop {
                    /* LOOK first, wait second. The bell baseline above was
                     * read after the plan's empty look, so a message landing
                     * in that gap has already rung: waiting on the bell first
                     * would sleep through it until the deadline. Looking
                     * first answers it now, and a message that lands after an
                     * empty look moves the counter past `seen`, so the wait
                     * below returns at once.
                     *
                     * The woken look, the delivery it takes and the receipt
                     * that records it are ONE actor transition — a second
                     * verb cannot land between the answer and the record of
                     * it, which is what this road used to hold the ledger
                     * across the pair for. A look the store refuses is a
                     * refusal to the caller: the delivery never happened
                     * (the actor rewinds to the disk's word), so the caller
                     * asks again and the delivery is taken again. */
                    match actor.look_again(waiting.clone(), decided.receipt.clone(), now_ms) {
                        Ok((Some(found), _)) => {
                            rang(true);
                            return answer(found.reply);
                        }
                        Ok((None, _)) => {}
                        Err(why) => return refused_by_runtime(why),
                    }
                    if std::time::Instant::now() >= deadline {
                        // Silence is not failure — the ninth invariant, and
                        // the reason this is the answer the first look
                        // already wrote rather than a refusal. A coordinator
                        // reads `{"count":0}` and asks again; a refusal
                        // would have it killing healthy workers. The empty
                        // answer is served as the receipt now that it is the
                        // answer the wait actually gave.
                        match actor.serve_receipt(Box::new(decided.clone()), now_ms) {
                            Ok((moved, _)) => rang(moved),
                            Err(why) => return refused_by_runtime(why),
                        }
                        return answer(decided.reply);
                    }
                    seen = wait_for_mail(seen, deadline);
                }
            }
        },
        Effect::Split {
            pane: new_pane,
            from,
            direction,
            command,
            // A ledger worker is a seat, never a zo helper's pane: the
            // journaled split carries no helper, and the lane hands none on.
            helper: _,
        } => {
            /* The journaled effect, walked through the split lane.
             *
             * The ledger already carries the worker row — the actor wrote it,
             * durably, when it planned — so every refusal below has the same
             * second half: the row goes back through `seat_never_opened`,
             * because a worker row pointing at a pane that never opened is a
             * roster entry nobody can click and an `@all` that waits forever
             * for an answer from nothing.
             *
             * The child's capability is minted HERE, before the journal binds
             * the effect: the lane digests it into the destination generation,
             * so the recorded operation names the exact incarnation the pane
             * will hold — and the host receives the token instead of minting
             * its own, or the record and the pane would name two different
             * children. */
            let reseating = decided.prepared_worker_reseat.clone();
            let minted = reseating
                .is_none()
                .then(|| worker_named(&decided.reply.stdout))
                .flatten();
            let Some(epoch) = host_epoch() else {
                return roll_back_the_seat(
                    actor,
                    minted.as_deref(),
                    now_ms,
                    "this window has no host epoch",
                );
            };
            let Some(child_token) = crate::hooks::random_token() else {
                return roll_back_the_seat(
                    actor,
                    minted.as_deref(),
                    now_ms,
                    "could not mint a capability for the new pane",
                );
            };
            let worker_host = decided
                .prepared_worker_start
                .as_ref()
                .map(|prepared| crate::agent_teams::WorkerHostSpec {
                    // An inherited checkout is the reseat's placement — the
                    // exact existing tree — for a pane that is not a reseat:
                    // the ended attempt's tree under a new pair of hands
                    // (`--inherit-checkout`, §2.3). The host refuses a tree
                    // that is gone by now with this reservation's rollback.
                    placement: match (&prepared.inherit_checkout, prepared.worktree) {
                        (Some(path), _) => {
                            crate::agent_teams::WorkerHostPlacement::Existing(path.clone())
                        }
                        (None, true) => crate::agent_teams::WorkerHostPlacement::New(
                            prepared.worktree_title.clone(),
                        ),
                        (None, false) => crate::agent_teams::WorkerHostPlacement::Inherited,
                    },
                    prompt: prepared.prompt.clone(),
                    timeout_ms: prepared.prompt_timeout_ms,
                    resumed: None,
                    seat: Some(crate::agent_teams::WorkerSeatWords::of(prepared)),
                })
                .or_else(|| {
                    reseating
                        .as_ref()
                        .map(|prepared| crate::agent_teams::WorkerHostSpec {
                            placement: crate::agent_teams::WorkerHostPlacement::Existing(
                                prepared.checkout.clone(),
                            ),
                            prompt: prepared.prompt.clone(),
                            timeout_ms: prepared.prompt_timeout_ms,
                            resumed: Some(prepared.resumed),
                            seat: None,
                        })
                });
            let _worker_host = worker_host
                .map(|spec| crate::agent_teams::WorkerHostAsk::place(&child_token, spec));
            // The same key opens both sidechannels: the ask rides out on it,
            // and the host's seat answer rides back on it below.
            let seat_token = child_token.clone();
            let draft = match SplitLeaseDraft::capture(
                team_id,
                &from,
                &new_pane,
                child_token,
                epoch.to_string(),
                direction,
                command.clone(),
            ) {
                Ok(draft) => draft,
                Err(why) => {
                    return roll_back_the_seat(
                        actor,
                        minted.as_deref(),
                        now_ms,
                        &format!("the pane changed while the worker was starting ({why:?})"),
                    );
                }
            };
            /* The request's durable identity is the caller's retry identity
             * when one was named — the journal is what makes a worker-start
             * replayable across a crash — and the worker id itself otherwise:
             * unique per attempt, so an unnamed start simply has no replay to
             * find, which is the same promise the receipts make. */
            let (slot, fingerprint) = match (decided.receipt.as_ref(), reseating.as_ref()) {
                (Some(key), _) => (
                    key.durable_identity().slot().to_string(),
                    key.durable_identity().fingerprint().to_string(),
                ),
                (None, Some(prepared)) => {
                    // One restore attempt per worker per host incarnation. A
                    // later window has a new epoch and may try the sleeping
                    // row again; two calls in this window reconcile the same
                    // journaled split instead of opening siblings.
                    let restore = digest(
                        b"zerocode.orchestration.worker-reseat.v1",
                        &[prepared.worker.as_bytes(), epoch.as_bytes()],
                    );
                    (restore.clone(), restore)
                }
                (None, None) => {
                    let unnamed = digest(
                        b"zerocode.orchestration.unnamed-split.v1",
                        &[minted.as_deref().unwrap_or_default().as_bytes()],
                    );
                    (unnamed.clone(), unnamed)
                }
            };
            let queued_at = std::time::Instant::now();
            /* The journal cuts one pane at a time. A second spawn arriving
             * while one is mid-walk is not an error and not a recovery — it
             * waits in line, the way the mailbox already makes callers wait,
             * and is refused honestly only if the line never moves. */
            let begun = loop {
                let request = match EffectRequest::new(
                    "main-ledger",
                    slot.clone(),
                    fingerprint.clone(),
                    draft.binding_digest().to_string(),
                    epoch.to_string(),
                    HostEffectKind::Split,
                ) {
                    Ok(request) => request,
                    Err(why) => {
                        return roll_back_the_seat(
                            actor,
                            minted.as_deref(),
                            now_ms,
                            &format!("the effect could not be described ({why:?})"),
                        );
                    }
                };
                match actor.prepare_effect(request, now_ms) {
                    Err(RuntimeError::EffectInFlight)
                        if queued_at.elapsed() < EFFECT_LINE_CEILING =>
                    {
                        std::thread::sleep(EFFECT_LINE_STEP);
                    }
                    answer => break answer,
                }
            };
            let permit = match begun {
                Ok((BeginEffect::Execute(permit), _)) => permit,
                Ok((BeginEffect::Replay(_), _)) => {
                    /* The journal already carried this exact request out — a
                     * crash fell between its settlement and its receipt. The
                     * plan above minted a SECOND row for it; that row goes
                     * back, and the caller is pointed at the roster where the
                     * first answer lives. */
                    return roll_back_the_seat(
                        actor,
                        minted.as_deref(),
                        now_ms,
                        "this request was already carried out — read worker-list for the worker it started",
                    );
                }
                Ok((BeginEffect::Refused(_), _)) => {
                    return roll_back_the_seat(
                        actor,
                        minted.as_deref(),
                        now_ms,
                        "this request was already refused for good — ask again under a new name",
                    );
                }
                Ok((BeginEffect::Reconcile(permit), _)) => {
                    /* An earlier attempt of this very request is still open in
                     * the journal. In one process that can only be a crash
                     * this window survived without dying; the honest move is
                     * to close the old attempt as fenced — its lease never
                     * outlived its walk — and have the caller ask again. The
                     * permit MUST be settled, not dropped: a dropped permit is
                     * a fence nothing ever lifts. */
                    let tombstone = digest(
                        b"zerocode.orchestration.reconciled-in-window-tombstone.v1",
                        &[epoch.as_bytes()],
                    );
                    /* `retry_not_before_ms` MUST be present when retryable —
                     * the journal's validator refuses `(true, None)` before
                     * touching the database, and a refused settlement here
                     * would leave exactly the fence this arm exists to lift.
                     * Best-effort beyond that: if the store itself refuses,
                     * the operation stays open for the next boot's road. */
                    if let Err(why) = actor.settle_effect(
                        permit,
                        EffectSettlement::NotStarted {
                            failure: HostEffectFailure::Disconnected,
                            tombstone_digest: tombstone,
                            retryable: true,
                            retry_not_before_ms: Some(now_ms.saturating_add(1)),
                            settled_at_ms: now_ms,
                        },
                    ) {
                        /* The old attempt would not close — say THAT, not
                         * "ask again": asking again meets the same open
                         * operation until the store takes the settlement or
                         * the next boot reconciles it. */
                        let said = refused_by_runtime(why);
                        let _ = roll_back_the_seat(actor, minted.as_deref(), now_ms, "");
                        return said;
                    }
                    return roll_back_the_seat(
                        actor,
                        minted.as_deref(),
                        now_ms,
                        "an earlier attempt of this request was being reconciled — ask again",
                    );
                }
                Err(why) => {
                    let said = refused_by_runtime(why);
                    let _ = roll_back_the_seat(actor, minted.as_deref(), now_ms, "");
                    return said;
                }
            };
            let lease = match draft.finalize(permit) {
                Ok(lease) => lease,
                /* Unreachable by construction — the permit was minted for
                 * this very draft's digests one call ago — and left as a
                 * refusal rather than a panic.
                 *
                 * The door hands the permit back now rather than eating it,
                 * and the promise every other arm of this walk keeps is that
                 * the operation is settled HERE, not left standing open for
                 * the next boot: an arm that leaves it open refuses every
                 * spawn behind it until a restart. */
                Err(refused) => {
                    let (permit, why) = *refused;
                    settle_permit_unstarted(actor, permit, HostEffectFailure::Disconnected, now_ms);
                    return roll_back_the_seat(
                        actor,
                        minted.as_deref(),
                        now_ms,
                        &format!("the effect lease did not match its own draft ({why:?})"),
                    );
                }
            };
            /* The host runs INSIDE the lane's fence: the source incarnation
             * and the empty destination are re-proved under the table lock,
             * the pane is cut, and the seat is recorded into the same table —
             * one generation, exactly the shape the old single-guard arm had.
             * The spawn under the table lock is today's cost kept, not a new
             * one; the host must not reach back into the table (its own
             * contract, `Host::split`). */
            /* A host that PANICS mid-walk must not take the permit down
             * with it: a dropped permit is a fence nothing ever lifts, and
             * every later spawn in this window would wait out the line and be
             * refused for a pane nobody is cutting. The walk is caught, the
             * operation settled the way boot recovery settles a dead
             * window's — Disconnected, retryable — and the panic resumes. */
            let walked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let apply = || {
                    lease.apply_fenced(epoch, |team, binding, token, command| {
                        let spawned = host.split(
                            &team.id.clone(),
                            binding.leader_term(),
                            binding.source().term(),
                            binding.destination_pane(),
                            binding.direction(),
                            command,
                            token,
                        );
                        if let Some(term) = spawned {
                            team.record_split(
                                binding.destination_pane(),
                                term,
                                binding.source().pane(),
                                binding.direction(),
                            );
                            /* The capability is registered HERE, where the seat is —
                             * not left to the host's discretion. The recorded
                             * generation is a digest of this exact token, and a host
                             * that spawned without registering would leave a pane
                             * whose incarnation nothing can verify. The window's own
                             * host registers it too, with its spawn-failure rollback;
                             * the same value twice is the same value. Tokens under
                             * teams — the established order. */
                            let _ = crate::agent_teams::remember_pane_token(
                                &team.id,
                                binding.destination_pane(),
                                token.to_string(),
                            );
                        }
                        spawned
                    })
                };
                if let Some(prepared) = decided
                    .prepared_worker_start
                    .as_ref()
                    .filter(|prepared| prepared.handover.is_some())
                {
                    actor
                        .handover_split(
                            prepared.clone(),
                            lease.binding().operation_id().to_string(),
                            now_ms,
                            apply,
                        )
                        .unwrap_or(Err(crate::durable_split::SplitLeaseError::StaleIncarnation))
                } else {
                    apply()
                }
            }));
            let walked = match walked {
                Ok(walked) => walked,
                Err(panic) => {
                    settle_unstarted(actor, lease, HostEffectFailure::Disconnected, now_ms);
                    let _ = roll_back_the_seat(actor, minted.as_deref(), now_ms, "");
                    std::panic::resume_unwind(panic);
                }
            };
            /* The host's seat answer, taken on every road out — a refusal
             * below simply drops it, the way a refused split's worktree ask
             * is taken back. Where the pane actually sits is the window's
             * placement fact, and this is the one road that fact has into
             * the ledger (`worker_seated`): the ledger cannot derive it,
             * because the cut runs through the person's own
             * workspace-creation preferences. */
            let seated = crate::agent_teams::take_seat_checkout(&seat_token);
            let host_failure = crate::agent_teams::take_worker_host_failure(&seat_token);
            match walked {
                Ok(Some(term)) => {
                    if let Some(prepared) = reseating.as_ref()
                        && seated.as_deref() != Some(prepared.checkout.as_str())
                    {
                        let destination = lease.binding().destination_pane().to_string();
                        {
                            let mut tables = crate::agent_teams::teams();
                            if let Some(team) = tables.get_mut(team_id)
                                && team.term_of(&destination) == Some(term)
                            {
                                team.remove_pane(&destination);
                            }
                        }
                        host.close(term);
                        settle_unstarted(actor, lease, HostEffectFailure::Disconnected, now_ms);
                        return refused(
                            "the restored pane did not open in the worker's durable checkout",
                        );
                    }
                    /* Spawning is the fenced effect; waiting for a TUI is not.
                     * The production waiter can time out and clean the term,
                     * which walks `forget_term_state → forget_term` back into
                     * the team table. Calling it from `apply_fenced` would
                     * re-acquire this same non-reentrant mutex and freeze the
                     * entire window. At this point the fence guard is gone.
                     *
                     * A failed waiter owns host cleanup. We defensively remove
                     * the recorded seat if it still names this term so a fake
                     * or future host cannot leave a ghost table row behind. */
                    if let Err(failure) = host.await_worker_ready(term) {
                        let destination = lease.binding().destination_pane().to_string();
                        {
                            let mut tables = crate::agent_teams::teams();
                            if let Some(team) = tables.get_mut(team_id)
                                && team.term_of(&destination) == Some(term)
                            {
                                team.remove_pane(&destination);
                            }
                        }
                        settle_unstarted(actor, lease, HostEffectFailure::Refused, now_ms);
                        return roll_back_the_seat(
                            actor,
                            minted.as_deref(),
                            now_ms,
                            &failure.to_string(),
                        );
                    }
                    let result = match SplitEffectResult::applied(lease.binding(), term) {
                        Ok(result) => result,
                        Err(why) => {
                            /* The pane EXISTS — the walk cut and recorded it —
                             * but its identity can no longer be proven into an
                             * Applied result. Undo what the walk did (the
                             * recorded seat, then the pane itself), and settle
                             * the permit: the rule above is absolute — a
                             * dropped permit is a fence nothing ever lifts,
                             * and every sibling arm of this match settles.
                             *
                             * A `--worktree` checkout cut for this pane is
                             * NOT swept here, on purpose: the host spent the
                             * ask and owns the tree, and this layer has no
                             * handle to it. An orphan from this hair-thin arm
                             * stands in the workspace list where a person can
                             * see and remove it — a wrong `remove` from the
                             * wrong layer could take a tree that gained work. */
                            let destination = lease.binding().destination_pane().to_string();
                            {
                                let mut tables = crate::agent_teams::teams();
                                if let Some(team) = tables.get_mut(team_id)
                                    && team.term_of(&destination) == Some(term)
                                {
                                    team.remove_pane(&destination);
                                }
                            }
                            host.close(term);
                            settle_unstarted(actor, lease, HostEffectFailure::Disconnected, now_ms);
                            return roll_back_the_seat(
                                actor,
                                minted.as_deref(),
                                now_ms,
                                &format!("the pane opened and then changed ({why:?})"),
                            );
                        }
                    };
                    let outcome = digest(
                        b"zerocode.orchestration.split-applied.v1",
                        &[result.operation_id().as_bytes(), &term.to_le_bytes()],
                    );
                    let (permit, _) = match lease.finish(result) {
                        Ok(finished) => finished,
                        Err(returned) => {
                            /* Unreachable by construction — the result was
                             * built from this very lease's binding one call
                             * ago — and treated exactly like the arm above if
                             * it ever happens: the pane exists, so it is
                             * undone, and the permit is settled, not
                             * dropped. */
                            let (lease, why) = *returned;
                            let destination = lease.binding().destination_pane().to_string();
                            {
                                let mut tables = crate::agent_teams::teams();
                                if let Some(team) = tables.get_mut(team_id)
                                    && team.term_of(&destination) == Some(term)
                                {
                                    team.remove_pane(&destination);
                                }
                            }
                            host.close(term);
                            settle_unstarted(actor, lease, HostEffectFailure::Disconnected, now_ms);
                            return roll_back_the_seat(
                                actor,
                                minted.as_deref(),
                                now_ms,
                                &format!(
                                    "the pane opened but its effect could not be closed ({why:?})"
                                ),
                            );
                        }
                    };
                    if let Err(why) = actor.settle_effect(
                        permit,
                        EffectSettlement::Applied {
                            result_digest: outcome,
                            settled_at_ms: now_ms,
                        },
                    ) {
                        return refused_by_runtime(why);
                    }
                    rang(true);
                    if let Some(prepared) = reseating.as_ref() {
                        if let Err(why) =
                            actor.worker_reseated(&prepared.worker, team_id, &new_pane, now_ms)
                        {
                            // The pane exists but never acquired dispatch
                            // authority. Close it before anything in it can
                            // report as a coordinator; the durable worker row
                            // remains Sleeping when the write did not land.
                            {
                                let mut tables = crate::agent_teams::teams();
                                if let Some(team) = tables.get_mut(team_id)
                                    && team.term_of(&new_pane) == Some(term)
                                {
                                    team.remove_pane(&new_pane);
                                }
                            }
                            host.close(term);
                            return refused_by_runtime(why);
                        }
                        rang(true);
                        // A fresh process can report its session between spawn
                        // and the durable move. Replay the backend's whole
                        // value now that the term resolves to the worker.
                        if let Some(session) = host.provider_session(term)
                            && let Ok((moved, _)) =
                                actor.worker_session_reported(term, session, now_ms)
                        {
                            rang(moved);
                        }
                        /* The replacement was started at an idle composer.
                         * Only now, after the row and run binding are durable,
                         * may it see the interrupted work. A tiny task can run
                         * `worker_done` synchronously from this callback; that
                         * is the adversarial ordering this fence exists for. */
                        if !host.paste(term, &prepared.prompt) {
                            host.close(term);
                            return refused(
                                "the restored worker became unreachable before its continuation was delivered",
                            );
                        }
                        host.announce_reseated_worker(
                            term,
                            &prepared.checkout,
                            &prepared.agent,
                            prepared.resumed,
                        );
                        return answer(decided.reply);
                    }
                    /* The judgment's row, beside what was actually summoned
                     * (t-4711). Here rather than at the plan, because a
                     * summons whose split was refused is not a decision
                     * anybody made — and the checkout the pane landed in is
                     * the workspace the Jev door asks consent for. Off the
                     * beat, and nothing below waits on it. */
                    if let Some(prepared) = decided.prepared_worker_start.as_ref() {
                        summon_choice::record(host, prepared, seated.as_deref(), now_ms);
                    }
                    /* The seat report lands beside the receipt, best-effort:
                     * the pane is open and the answer below stands whatever
                     * happens here, and a report that could not land leaves
                     * the row honestly UNREPORTED — `@worktree:`'s refusal
                     * counts such rows out loud rather than guessing. */
                    if let Some(checkout) = seated
                        && let Ok((moved, _)) =
                            actor.worker_seated(team_id, &new_pane, &checkout, now_ms)
                    {
                        rang(moved);
                    }
                    // The pane really opened. NOW the receipt may be written —
                    // the failure arms are the whole reason it could not be
                    // written earlier (`Decided::receipt`).
                    match actor.serve_receipt(Box::new(decided.clone()), now_ms) {
                        Ok((moved, _)) => rang(moved),
                        Err(why) => return refused_by_runtime(why),
                    }
                    answer(decided.reply)
                }
                Ok(None) => {
                    // The host refused to spawn. The refusal is itself the
                    // evidence the journal keeps: the backend answered and
                    // said no, so nothing started.
                    settle_unstarted(actor, lease, HostEffectFailure::Refused, now_ms);
                    let reason = host_failure.as_ref().map_or_else(
                        || "could not open a pane".to_string(),
                        |failure| failure.to_string(),
                    );
                    if let (Some(prepared), Some(failure)) =
                        (reseating.as_ref(), host_failure.as_ref())
                        && failure.permanent
                    {
                        match actor.finish_sleeping_reseat(
                            &prepared.worker,
                            &failure.reason,
                            now_ms,
                        ) {
                            Ok(_) => rang(true),
                            Err(why) => return refused_by_runtime(why),
                        }
                    }
                    roll_back_the_seat(actor, minted.as_deref(), now_ms, &reason)
                }
                Err(why) => {
                    // The fence refused to run the host at all — the source
                    // incarnation moved or the destination filled between the
                    // plan and the walk. Nothing started, provably: the fence
                    // is the tombstone.
                    settle_unstarted(actor, lease, HostEffectFailure::Refused, now_ms);
                    roll_back_the_seat(
                        actor,
                        minted.as_deref(),
                        now_ms,
                        &format!("the pane changed while the worker was starting ({why:?})"),
                    )
                }
            }
        }
        Effect::WorkerTerminal {
            seat,
            incarnation,
            stop,
            lines,
            handover,
        } => {
            let unobserved = || match actor.release_unobserved(Box::new(decided.clone()), now_ms) {
                Ok(reply) => {
                    rang(true);
                    answer(reply)
                }
                Err(why) => refused_by_runtime(why),
            };
            let Some(incarnation) = incarnation else {
                return if stop.is_none() {
                    unobserved()
                } else {
                    refused("worker is gone")
                };
            };
            let term = incarnation.term;
            let generation = incarnation.capability;
            // Prove the actor's planned incarnation before even reading it.
            let still_ours = || {
                let mut tables = crate::agent_teams::teams();
                crate::agent_teams::authorized_team_mut(&mut tables, team_id, pane, pane_token)
                    .is_some()
                    && crate::agent_teams::authorized_team_mut(
                        &mut tables,
                        &seat.team,
                        &seat.pane,
                        &generation,
                    )
                    .is_some_and(|team| team.term_of(&seat.pane) == Some(term))
            };
            if !still_ours() {
                return if stop.is_none() {
                    unobserved()
                } else {
                    refused("worker changed before its terminal was retired")
                };
            }
            let screen = if stop.is_none() {
                host.capture(term).map(|tail| screen_tail(&tail, lines))
            } else {
                None
            };
            let archived = screen.is_some();
            let settled = if !still_ours() {
                Err(RuntimeError::AuthorityRejected)
            } else if let Some(plan) = handover.as_ref() {
                settle_handover_terminal(host, actor, &decided, plan, term, now_ms)
            } else {
                actor.worker_terminal_settled(decided.effect.clone(), screen, now_ms)
            };
            let (state, _) = match settled {
                Ok(answer) => answer,
                Err(RuntimeError::AuthorityRejected) if stop.is_none() => return unobserved(),
                Err(why) => return refused_by_runtime(why),
            };
            rang(true);
            if state == zerocode_core::orchestration::WorkerState::Released {
                let mut tables = crate::agent_teams::teams();
                let Some(target) = crate::agent_teams::authorized_team_mut(
                    &mut tables,
                    &seat.team,
                    &seat.pane,
                    &generation,
                )
                .filter(|target| target.term_of(&seat.pane) == Some(term)) else {
                    return refused("worker changed while its terminal was being retired");
                };
                target.remove_pane(&seat.pane);
                drop(tables);
                host.close(term);
            }
            let reply = zerocode_core::agent_teams::Reply::ok(format!(
                "{}\n",
                if stop.is_some() {
                    serde_json::json!({"workerId": seat.worker, "state": state.as_str(), "outcome": "stopped"})
                } else {
                    serde_json::json!({"workerId": seat.worker, "state": state.as_str(), "archived": archived})
                }
            ));
            if let Some(key) = decided.receipt.clone()
                && let Err(why) = actor.remember_served(key, reply.stdout.clone(), now_ms)
            {
                return refused_by_runtime(why);
            }
            answer(reply)
        }
        Effect::CaptureSeat {
            team,
            pane: seat,
            lines,
        } => {
            /* A read by SEAT — another leader's pane, resolved against the
             * table that holds it rather than the caller's (t-2512). The
             * same shape as the ordinary read below: remember the incarnation
             * first, capture with no lock held, then prove the seat still
             * names the same terminal before the screen is answered. No
             * receipt: `worker-read` is a `HostRead`. */
            let resolve = || {
                let tables = crate::agent_teams::teams();
                tables.get(&team).and_then(|held| held.term_of(&seat))
            };
            let Some(term) = resolve() else {
                return refused("worker is gone");
            };
            match host.capture(term) {
                Some(tail) => {
                    let reply = capture_reply_of(&decided, &screen_tail(&tail, lines));
                    if resolve() != Some(term) {
                        return refused("worker changed while its screen was read — ask again");
                    }
                    answer(reply)
                }
                None => refused("worker is gone"),
            }
        }
        /* One checkout's evidence, assembled where the sources live.
         *
         * The decision named the tree; this resolves it and reads. Three
         * things are deliberately NOT here. No test is run — the receipts are
         * whatever the verifier already wrote. No GitHub call is made — `ci`
         * comes back unsupported and says so. And nothing is written: the
         * authority store is opened read-only and never created, so a window
         * that has none answers "there is no store" instead of making one.
         *
         * A refusal is the structured shape the sections use, not a bare
         * sentence: this verb's whole contract is that a caller can tell an
         * absence from a failure.
         */
        Effect::WorktreeEvidence { checkout } => {
            let own = crate::agent_teams::teams()
                .get(team_id)
                .and_then(|team| team.term_of(pane))
                .and_then(|term| host.worktree_of(term));
            match crate::worktree_evidence_runtime::for_seat(checkout, own, now_ms) {
                Ok(evidence) => match serde_json::to_string(&evidence) {
                    Ok(said) => answer(capture_reply_of(&decided, &said)),
                    Err(_) => refused("the evidence could not be written as JSON"),
                },
                Err(why) => refused(zerocode_core::orchestration::evidence_refusal(
                    why.code,
                    why.message,
                    why.retryable,
                )),
            }
        }
        Effect::Capture { term, lines } => {
            let Some(worker) = decided.releasing.clone() else {
                // An ordinary read. No lock is held across it: a capture
                // reaches the pty, and holding the table across it would
                // block every other agent in the window behind one screen.
                //
                // Remember both halves of the target incarnation first. A
                // respawn may reuse the pane name (and, after a process
                // restart, even a terminal number); its unlogged capability is
                // the generation that distinguishes the replacement.
                let target_pane = {
                    let tables = crate::agent_teams::teams();
                    tables.get(team_id).and_then(|team| {
                        team.panes()
                            .find(|one| one.term == term)
                            .map(|one| one.id.clone())
                    })
                };
                let Some(target_pane) = target_pane else {
                    return refused("worker is gone");
                };
                let Some(target_generation) =
                    crate::agent_teams::current_pane_capability(team_id, &target_pane)
                else {
                    return refused("worker is gone");
                };
                return match host.capture(term) {
                    Some(tail) => {
                        let reply = capture_reply_of(&decided, &screen_tail(&tail, lines));
                        /* The screen came back; now prove it still belongs to
                         * the request that captured it.
                         *
                         * BOTH checks matter. The caller capability may have
                         * rotated while the request was in flight, and the
                         * target pane may have been respawned behind the same
                         * name. The target's token is checked with the same
                         * constant-time authorization door as a request, then
                         * its exact terminal is checked too. A missing token is
                         * not guessed through.
                         *
                         * The receipt is filed while this table guard still
                         * pins both incarnations. The lock order is the file's
                         * established one — team table, then ledger — and no
                         * host call happens under either. */
                        let mut tables = crate::agent_teams::teams();
                        let caller_is_same = crate::agent_teams::authorized_team_mut(
                            &mut tables,
                            team_id,
                            pane,
                            pane_token,
                        )
                        .is_some();
                        let target_is_same = crate::agent_teams::authorized_team_mut(
                            &mut tables,
                            team_id,
                            &target_pane,
                            &target_generation,
                        )
                        .is_some_and(|team| team.term_of(&target_pane) == Some(term));
                        if !caller_is_same || !target_is_same {
                            return refused("worker changed while its screen was read — ask again");
                        }
                        /* Unreachable since `worker-read` became a
                         * `Doing::HostRead`: a read that writes nothing down
                         * is refused a retry name before it ever gets here, so
                         * `decided.receipt` is always `None` on this road. Left
                         * standing rather than deleted because the refusal is
                         * new and the shape of this arm is not — if a later
                         * verb reads a host AND files receipts, this is where
                         * it would go. */
                        drop(tables);
                        if let Some(key) = decided.receipt.clone() {
                            match actor.remember_served(key, reply.stdout.clone(), now_ms) {
                                Ok(_) => rang(true),
                                Err(why) => return refused_by_runtime(why),
                            }
                        }
                        answer(reply)
                    }
                    None => refused("worker is gone"),
                };
            };
            // A release. The read happens FIRST and its answer is kept, and
            // only then is the terminal closed — that order is the whole
            // difference between a release and a close, and it is why the two
            // are one verb.
            // The SEAT, not a bare pane id. A pane id is unique only inside
            // its team — every team numbers its first teammate `%2` — and the
            // worker row carries the team for exactly this reason. Read from
            // the actor's snapshot: the seat was true at the revision that
            // planned this release, and everything below re-proves the
            // incarnation before acting on it.
            let seat = actor.view().ok().and_then(|image| {
                image
                    .projection()
                    .workers
                    .iter()
                    .find(|one| one.id == worker)
                    .map(|one| (one.team.clone(), one.pane.clone()))
            });
            let screen = host.capture(term).map(|tail| screen_tail(&tail, lines));

            /* Reading a screen takes time, and everything can move while it
             * does. So the table is taken AGAIN and what comes next is checked
             * against the incarnation this read belonged to.
             *
             * Two defects lived in the version without these checks, and both
             * were found by review rather than by a failing test:
             *
             * · It removed the pane id from EVERY team. Pane ids repeat across
             *   teams, so releasing team A's `%2` silently unseated team B's
             *   live `%2` — and a worker with no seat can never be settled by
             *   `terminal_gone` again: its dispatch stays open for the rest of
             *   the session.
             *
             * · It acted on a stale incarnation. A `respawn-pane` during the
             *   capture keeps the pane id and replaces the terminal behind it,
             *   so the release would mark the ledger released, delete the
             *   REPLACEMENT pane, and close only the old term — retiring a
             *   worker that had just been started.
             *
             * A read that cannot be completed is `release_unknown`, which is
             * the state this road already had for "we asked and were not
             * answered". It is the honest answer here too: the terminal is left
             * alone rather than a retirement being claimed that did not happen.
             */
            let retiring = {
                let mut tables = crate::agent_teams::teams();
                let still_ours =
                    crate::agent_teams::authorized_team_mut(&mut tables, team_id, pane, pane_token)
                        .is_some();
                let same_incarnation = seat.as_ref().is_some_and(|(team, seat_pane)| {
                    tables.get(team).and_then(|held| held.term_of(seat_pane)) == Some(term)
                });
                screen.is_some() && still_ours && same_incarnation
            };
            /* The settlement goes through the actor's door — and the table is
             * NOT held across that call: the actor takes the table itself for
             * other requests, and a caller that held it while waiting on the
             * mailbox would deadlock the window. The pane's removal below
             * re-proves the incarnation after the door answers; a respawn
             * that lands in between leaves the replacement pane standing,
             * which is the honest side to fail on. */
            let settled = actor.release_settled(
                worker.clone(),
                match retiring {
                    true => screen,
                    false => None,
                },
                now_ms,
            );
            let now = match settled {
                Ok((state, _)) => {
                    rang(true);
                    state
                }
                Err(why) => return refused_by_runtime(why),
            };
            let removed = retiring
                && seat.as_ref().is_some_and(|(team, seat_pane)| {
                    let mut tables = crate::agent_teams::teams();
                    let current = tables.get(team).and_then(|held| held.term_of(seat_pane));
                    match current == Some(term) {
                        true => {
                            if let Some(held) = tables.get_mut(team) {
                                held.remove_pane(seat_pane);
                            }
                            true
                        }
                        false => false,
                    }
                });
            if removed {
                // Outside every lock. The window's close walks back into
                // `forget_term`, which takes the team table — calling it under
                // that guard froze the whole window once.
                host.close(term);
            }
            // A release is a read AND a close, and both have now happened —
            // so this is the moment its receipt becomes true. The answer filed
            // is the one this arm built, not the empty one `plan` wrote before
            // the screen was in hand.
            let said = zerocode_core::agent_teams::Reply::ok(format!(
                "{}\n",
                serde_json::json!({
                    "workerId": worker,
                    "state": now.as_str(),
                    "archived": retiring,
                })
            ));
            if let Some(key) = decided.receipt.clone() {
                match actor.remember_served(key, said.stdout.clone(), now_ms) {
                    Ok(_) => rang(true),
                    Err(why) => return refused_by_runtime(why),
                }
            }
            answer(said)
        }
        // A stop. The ledger has already written the attempt off — that is what
        // `worker-stop` means and it happens before this — so the terminal has
        // to actually end, or the record and the screen say different things
        // about the same worker. Until this arm existed they did: the ledger
        // spent the attempt and marked the worker released, and the agent read
        // `this window cannot carry out Close` while the pane stayed open.
        Effect::Close {
            pane: gone_pane,
            term,
        } => {
            /* The pane leaves the table only while it still means THIS term.
             * The plan ran one hop ago on the actor's thread; a respawn in
             * the hop keeps the pane id and replaces the terminal behind it,
             * and removing — worse, closing — the replacement would retire an
             * agent that just started. A mismatch refuses WITHOUT touching
             * the host; a team already dissolved has no pane left to remove
             * and its terminal died with it, so the close below is a no-op
             * the host can absorb. */
            {
                let mut tables = crate::agent_teams::teams();
                if let Some(team) = tables.get_mut(team_id) {
                    match team.term_of(&gone_pane) == Some(term) {
                        true => team.remove_pane(&gone_pane),
                        false => {
                            return refused(
                                "the pane changed while the worker was being stopped — ask again",
                            );
                        }
                    }
                }
            }
            // Outside every lock: the window's close walks back into
            // `forget_term`, which takes the team table. Calling it under
            // that guard froze the whole window once (live report
            // 2026-08-17), which is why the release arm above does the same
            // thing in the same order.
            host.close(term);
            match actor.serve_receipt(Box::new(decided.clone()), now_ms) {
                Ok((moved, _)) => rang(moved),
                Err(why) => return refused_by_runtime(why),
            }
            answer(decided.reply)
        }
        /* A `dispatch --inject`: the preamble pasted at the worker's own
         * terminal. The attachment was made durable on the actor's thread and
         * stands whatever happens here — the paste is delivery, not
         * authority, and a pane that dies in the hop is settled by
         * `terminal_gone` like any other death. The receipt is filed only
         * once the paste has actually landed, so a caller that never heard
         * back retries into the attach guard's own sentence rather than into
         * a second attempt. `host.paste` is the liveness check: a respawn
         * retires the term it replaces, and a retired term answers false.
         *
         * The sanitizing happens HERE, where a fake host can witness it: a
         * task's spec is prose nobody vetted for key grammar, and an escape
         * left in it would close the window's paste envelope early and have
         * the rest read as live typing. Made inert on this side of the trait,
         * every host — real or fake — receives text that cannot steer a
         * terminal, and the window's own paste road (which sanitizes again,
         * idempotently) only adds the envelope the pane really asked for. */
        Effect::Paste { term, text } => {
            // A pane whose exact launch zo refused holds a program that will
            // not run this briefing (t-2773): the window says why instead of
            // typing at it. The attach stands, as it does for a paste the
            // terminal lost — the refusal is the pane's, not the task's.
            if let Some(why) = host.delivery_refusal(term) {
                return refused(format!(
                    "{why} — the dispatch stands; give the pane a launch zo can honour, \
                     or stop the worker"
                ));
            }
            if !host.paste(term, &zerocode_pty::sanitize_paste(&text)) {
                return refused(
                    "the terminal is gone, so the preamble was not pasted — the \
                     dispatch stands, and the pane's death will settle it",
                );
            }
            match actor.serve_receipt(Box::new(decided.clone()), now_ms) {
                Ok((moved, _)) => rang(moved),
                Err(why) => return refused_by_runtime(why),
            }
            answer(decided.reply)
        }
        // The ledger plans no other effect. Said rather than silently
        // ignored, so the day one is added and this arm is not, an agent is
        // told instead of being handed a success it did not get.
        other => refused(format!("this window cannot carry out {other:?}")),
    }
}

/// Put a minted worker row back and refuse — the shared tail of every road
/// where the pane this plan asked for never opened.
///
/// The rollback is best-effort ON PURPOSE: the refusals it rides already
/// carry the honest answer, and a rollback the actor cannot take (the seat
/// somehow open, the store refusing) leaves a row the restart sweep or a
/// person's `worker-stop` can still reach. An empty `why` answers with the
/// caller's own sentence already built — the runtime-error road uses it.
fn roll_back_the_seat(
    actor: &RuntimeActor,
    minted: Option<&str>,
    now_ms: i64,
    why: &str,
) -> zerocode_hookd::TeamAnswer {
    if let Some(worker) = minted
        && let Ok((moved, _)) = actor.seat_never_opened(worker, now_ms)
    {
        rang(moved);
    }
    refused(why)
}

/// Settle a walked lease as not-started, with the fence as its tombstone.
///
/// Best-effort like the rollback beside it: the caller is already refusing,
/// and a settlement the store refuses leaves the operation for the next
/// boot's reconciliation road rather than losing it.
/// The one name a fenced split's tombstone has.
///
/// Read twice — once to build the result a lease is finished with, once to
/// settle the permit that comes out of it — and they must be the same value
/// or the journal refuses the settlement it was handed. One function so
/// there is one answer.
fn split_tombstone(operation_id: &str) -> String {
    digest(
        b"zerocode.orchestration.split-fenced-tombstone.v1",
        &[operation_id.as_bytes()],
    )
}

/// Settle a permit this window holds and has no road for.
///
/// The INNER door. It takes the bare permit rather than a lease because the
/// lease's pairing dance proves that a result belongs to a lease, and a
/// permit needs no such proof: the actor's own `require_permit` compares the
/// row revision the permit names. Every arm that ends up holding a permit
/// and nowhere to walk ends here — which is what makes "every arm settles"
/// a shape rather than a habit.
fn settle_permit_unstarted(
    actor: &RuntimeActor,
    permit: EffectPermit,
    failure: HostEffectFailure,
    now_ms: i64,
) {
    let tombstone = split_tombstone(permit.operation_id());
    /* Retryable, due at once. The beat asks again under a DETERMINISTIC
     * name (`beat-{task}-{attempt}`), and a spawn that failed once — a full
     * pty table, a fork refusal, a pane that moved mid-walk — is not a
     * request that must never run: `retryable: false` here would pin that
     * name to `Refused` forever, silently parking the task and every task
     * behind it. The journal re-proves everything on the next attempt. */
    if let Err(_why) = actor.settle_effect(
        permit,
        EffectSettlement::NotStarted {
            failure,
            tombstone_digest: tombstone,
            retryable: true,
            retry_not_before_ms: Some(now_ms.saturating_add(1)),
            settled_at_ms: now_ms,
        },
    ) {
        /* Still best-effort — the caller is already refusing — but no longer
         * SILENT: this was the one settlement of its kind with no sentence
         * anywhere when the disk refused it, and unlike a terminal's, the
         * permit is spent, so this window cannot try again. */
        note_ledger_unwritten("a failed spawn's settlement");
    }
}

/// The OUTER door: the same settlement, reached with a lease in hand.
fn settle_unstarted(
    actor: &RuntimeActor,
    lease: crate::durable_split::SplitLease,
    failure: HostEffectFailure,
    now_ms: i64,
) {
    let binding = lease.binding().clone();
    let tombstone = split_tombstone(binding.operation_id());
    let Ok(result) = SplitEffectResult::not_started(&binding, failure, tombstone) else {
        return;
    };
    match lease.finish(result) {
        Ok((permit, _)) => settle_permit_unstarted(actor, permit, failure, now_ms),
        Err(returned) => {
            /* The settlement result itself would not pair with the permit —
             * nothing further can be settled from this window. The
             * abandonment is explicit and final here: the operation stays
             * open for the next boot's road, which is the honest remainder.
             * It is SAID rather than dropped, so that every unsaid drop
             * stays loud. */
            let (lease, _why) = *returned;
            lease.abandon("a not-started settlement would not pair with its permit");
        }
    }
}

/// The last `lines` lines of a screen.
///
/// `capture-pane` answers a whole screen and the ledger's `--lines` asks for
/// less of it. Trimmed here rather than in the host, because the host is the
/// tmux road's too and its answer is already the right one for that road.
/// [`capture_reply`] fills the placeholder in a tmux plan. Ours is a
/// [`Decided`], which is the same two fields plus one — so it is handed the
/// pair it actually reads rather than being taught a second type.
fn capture_reply_of(decided: &Decided, tail: &str) -> zerocode_core::agent_teams::Reply {
    capture_reply(
        &zerocode_core::agent_teams::Planned {
            effect: decided.effect.clone(),
            reply: decided.reply.clone(),
        },
        tail,
    )
}

fn screen_tail(tail: &str, lines: usize) -> String {
    let held: Vec<&str> = tail.lines().collect();
    held[held.len().saturating_sub(lines)..].join("\n")
}

/// The worker a `worker-start` answer named, for the rollback.
///
/// Read back out of the reply rather than threaded through the plan: the plan's
/// job is to decide, and giving it a second output channel for one caller's
/// undo would put a rollback concern inside a pure function.
fn worker_named(stdout: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(stdout)
        .ok()?
        .get("workerId")?
        .as_str()
        .map(str::to_string)
}

fn answer(reply: zerocode_core::agent_teams::Reply) -> zerocode_hookd::TeamAnswer {
    zerocode_hookd::TeamAnswer {
        stdout: reply.stdout,
        stderr: reply.stderr,
        exit_code: reply.exit_code,
    }
}

/* ---- federation: the wire ---------------------------------------------
 *
 * One ledger per machine, so this whole section is about the far side of
 * an ssh. The transport is deliberately narrow: HTTP/1.1 over a loopback
 * TCP address — the远 server is reached through an ssh tunnel
 * (`ssh -L 7791:127.0.0.1:<port> box`), so no plaintext ever leaves the
 * machine and no TLS stack rides along for it. The address book and the
 * home identity live under the data root's `federation/` directory; the
 * identity is minted once and holds, because it is the OWNERSHIP key the
 * far ledger checks per dispatch. */

/// The one protocol this window speaks. The original negotiates v1/v2/v3
/// off a capability list; our v1 already carries what its v2 added
/// (coordinator control mail rides `federation-import` from the first
/// day), so the number exists for the FUTURE: a newer home names a higher
/// one, is refused by name, and lowers — no capability table to drift.
pub(crate) const FEDERATION_PROTOCOL: u64 = 1;

/// Where the machine's federation files live.
fn federation_root() -> Option<std::path::PathBuf> {
    BLACKBOX.get().map(|root| root.join("federation"))
}

/// This machine's federation identity, minted on first use and held.
/// 64 hex chars, file mode 0600 — the same trust boundary as the hook
/// token: same OS user, same secrets.
///
/// Held, not rotated, and carried without a nonce — and that is a decision,
/// not an omission. The wire's retry road IS replay: `receive_federation_call`
/// answers 503 when the window is slow and the home resends the same durable
/// request, `attach` lands on one `--retry-request` name so a replayed summons
/// answers the standing seat, and `pull`/`ack`/`import` are cursored by
/// sequence. A nonce that refused a duplicate would refuse the retry, and the
/// seen-nonce window it needs is per-home state this side deliberately does not
/// keep. An expiry needs a clock two machines do not share and a renewal verb
/// on a wire whose only distribution channel is a person pasting
/// `federation-invite` output.
///
/// What replay would buy an attacker is bounded by the transport instead:
/// someone placed to capture a request already holds the loopback socket, which
/// is this same OS user — the boundary the 0600 above names, and the one
/// [`loopback_only`] exists to keep. The remote leg is ssh's, and ssh already
/// gives confidentiality, integrity and anti-replay. The blast radius is
/// narrowed independently: `federation-invite` derives a one-way scoped bearer
/// that the bridge accepts only on `/federation`. A bridge-wide token from an
/// older invite remains accepted there during migration, but new homes no
/// longer receive the key to `/hook`, `/agent-teams` or `/browser`.
pub(crate) fn home_identity() -> Option<String> {
    let root = federation_root()?;
    let path = root.join("identity");
    if let Ok(held) = std::fs::read_to_string(&path) {
        let held = held.trim().to_string();
        if !held.is_empty() {
            return Some(held);
        }
    }
    let minted = crate::hooks::random_token()?;
    std::fs::create_dir_all(&root).ok()?;
    std::fs::write(&path, &minted).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Some(minted)
}

/// One worker server the home can name: a canonical numeric loopback (or SSH
/// tunnel) address and the far window's transport token.
#[derive(Clone, serde::Deserialize)]
pub(crate) struct ServerEntry {
    pub addr: String,
    pub token: String,
}

impl std::fmt::Debug for ServerEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The address is routing metadata, but the bearer is private transport
        // material. Keep the same diagnostic shape as ProviderSession: retain
        // its byte size without putting the secret in a debug log.
        formatter
            .debug_struct("ServerEntry")
            .field("addr", &self.addr)
            .field("token_bytes", &self.token.len())
            .finish()
    }
}

/// The address book: `federation/servers.json`, `{"<name>": {"addr":
/// "127.0.0.1:7791", "token": "…"}}`. Read per use — an edit lands on the
/// next call, and a book that fails to parse reads as empty rather than
/// half-read. New entries are canonical numeric endpoints; older entries stay
/// visible so `federation-servers` can explain how to migrate them.
pub(crate) fn server_book() -> std::collections::HashMap<String, ServerEntry> {
    federation_root()
        .and_then(|root| std::fs::read_to_string(root.join("servers.json")).ok())
        .and_then(|held| serde_json::from_str(&held).ok())
        .unwrap_or_default()
}

/// A minimal HTTP/1.1 POST to a loopback address. `Connection: close`, so
/// the read runs to EOF and no keep-alive state survives a call.
fn http_post(
    addr: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
    read_timeout: std::time::Duration,
) -> Result<(u16, String), String> {
    use std::io::{Read as _, Write as _};
    let mut stream = std::net::TcpStream::connect(addr)
        .map_err(|why| format!("could not reach {addr}: {why}"))?;
    stream
        .set_read_timeout(Some(read_timeout))
        .map_err(|why| format!("socket refused a deadline: {why}"))?;
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream
        .write_all(request.as_bytes())
        .map_err(|why| format!("could not write to {addr}: {why}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|why| format!("could not read from {addr}: {why}"))?;
    let text = String::from_utf8_lossy(&raw);
    let mut lines = text.splitn(2, "\r\n\r\n");
    let head = lines.next().unwrap_or_default();
    let body = lines.next().unwrap_or_default().to_string();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("no HTTP status from {addr}"))?;
    Ok((status, body))
}

/// One federation call to a named server, addressed by the book and signed
/// by this machine's identity. The far refusal comes back verbatim.
fn call_server(
    server: &str,
    body: &serde_json::Value,
    read_timeout: std::time::Duration,
) -> Result<serde_json::Value, String> {
    let book = server_book();
    let entry = book.get(server).ok_or_else(|| {
        format!(
            "unknown federation server: {server} — add it to federation/servers.json \
             beside this window's data"
        )
    })?;
    let endpoint = canonical_server_address(server, &entry.addr)?;
    let home = home_identity().ok_or("this machine could not mint a federation identity")?;
    let (status, said) = http_post(
        &endpoint.to_string(),
        zerocode_hookd::FEDERATION_PATH,
        &[
            (zerocode_hookd::HOOK_TOKEN_HEADER, entry.token.as_str()),
            (zerocode_hookd::FEDERATION_HOME_HEADER, home.as_str()),
        ],
        &body.to_string(),
        read_timeout,
    )?;
    if status != 200 {
        let migration = legacy_federation_token_warning(server, &entry.token)
            .map(|warning| format!("; {warning}"))
            .unwrap_or_default();
        return Err(format!("{server} answered {status}: {said}{migration}"));
    }
    serde_json::from_str(&said).map_err(|_| format!("{server} answered non-JSON: {said}"))
}

/// The home half of `worker-start --on`: walk the wire, and keep the
/// reservation honest either way — the receipt is served only when the far
/// side answered whole, and an attach that never landed is unwound so the
/// task goes back to ready.
fn federation_attach_from_home(
    actor: &RuntimeActor,
    decided: Decided,
    prepared: zerocode_core::orchestration::PreparedRemoteStart,
    now_ms: i64,
) -> zerocode_hookd::TeamAnswer {
    let asked = serde_json::json!({
        "verb": "attach",
        "protocol": FEDERATION_PROTOCOL,
        "dispatch": prepared.dispatch,
        "agent": prepared.agent,
        "prompt": prepared.prompt,
        "model": prepared.model,
        "effort": prepared.effort,
        "timeoutMs": prepared.timeout_ms,
    });
    let attach_deadline =
        std::time::Duration::from_millis(u64::from(prepared.timeout_ms).saturating_add(15_000));
    match call_server(&prepared.server, &asked, attach_deadline) {
        Ok(remote) => {
            /* The far side answered whole. NOW the receipt may be written —
             * a receipt filed before this moment would replay a reservation
             * the abort below may have already unwound. */
            match actor.serve_receipt(Box::new(decided.clone()), now_ms) {
                Ok((moved, _)) => rang(moved),
                Err(why) => return refused_by_runtime(why),
            }
            let mut said: serde_json::Value = serde_json::from_str(&decided.reply.stdout)
                .unwrap_or_else(|_| serde_json::json!({}));
            said["stage"] = serde_json::Value::from("ready");
            said["remote"] = remote;
            answer(zerocode_core::agent_teams::Reply::ok(format!("{said}\n")))
        }
        Err(why) => {
            /* The wire never carried it whole. The reservation is unwound —
             * durably — and the caller is refused with the transport's own
             * sentence; the same retry name asks fresh, which is exactly
             * what an unserved receipt means. */
            let unwound = actor.federation(FederationCall::AbortRemote {
                run: prepared.run.clone(),
                dispatch: prepared.dispatch.clone(),
                task_preimage: prepared.task_preimage.clone(),
                now_ms,
            });
            match unwound {
                Ok((FederationAnswer::Done, _)) => rang(true),
                Ok((FederationAnswer::Refused(stale), _)) => {
                    return refused(format!(
                        "the attach failed ({why}) and the reservation could not be \
                         unwound ({stale}) — read task-list before retrying"
                    ));
                }
                Ok(_) => {}
                Err(dead) => return refused_by_runtime(dead),
            }
            refused(format!("could not attach on {}: {why}", prepared.server))
        }
    }
}

/// The lines `help` grows in a window that can federate: the book verbs
/// are the shell's, not the planner's, so the planner's own table cannot
/// print them — and a guide the binary does not print is a guide that
/// drifts.
fn garnish_federation_help(verb: Option<&str>, reply: &mut zerocode_hookd::TeamAnswer) {
    if verb != Some("help") || reply.exit_code != 0 {
        return;
    }
    reply.stdout.push_str(
        "federation-invite · this window's own address and federation-scoped token, for a home's book\n\
         federation-join <name> --addr <loopback:port> --token <token> · add a worker server\n\
         federation-servers · the address book, tokens shortened\n\
         federation-forget <name> · drop one entry\n",
    );
}

/// The address book on disk, written whole and private. The read half is
/// [`server_book`]; both halves tolerate a missing directory, and neither
/// ever prints a full token back out.
fn write_server_book(
    book: &std::collections::BTreeMap<String, serde_json::Value>,
) -> Result<(), String> {
    let root = federation_root().ok_or("this window has no data root")?;
    std::fs::create_dir_all(&root).map_err(|why| format!("could not make {root:?}: {why}"))?;
    let path = root.join("servers.json");
    let held = serde_json::to_string_pretty(book).map_err(|why| why.to_string())?;
    std::fs::write(&path, held).map_err(|why| format!("could not write the book: {why}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

const FEDERATION_PLAINTEXT_REASON: &str = "the federation wire is plaintext and has no TLS";
const FEDERATION_TUNNEL_HINT: &str = "reach a remote window through an ssh tunnel (ssh -L <port>:127.0.0.1:<far-port> <box>) and join the tunnel's loopback endpoint";

/// A transport token travels as an HTTP header value, written into the
/// request verbatim by [`http_post`]. A token that is not visible ASCII
/// therefore does not travel as ONE header: a line break forges a second one
/// on the socket that carries the far window's whole bridge, and a stray byte
/// makes a request the far side cannot read at all. The far bridge only ever
/// compares visible ASCII (`HeaderValue::to_str`), so that is the alphabet —
/// and, like the address, it is refused at JOIN, where the person can re-copy
/// it, rather than at relay time, where nobody is watching.
fn usable_transport_token(token: &str) -> Result<(), String> {
    match token
        .bytes()
        .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    {
        true => Ok(()),
        false => Err(
            "that transport token is not header material — a federation token is \
             the visible-ASCII string federation-invite printed on the far window; re-copy \
             it without the line break"
                .to_string(),
        ),
    }
}

/// Explain how to replace a bridge-wide token written by an older invite.
/// The old bearer remains usable on `/federation`; this warning makes the
/// wider grant visible while a person still has the coordinates to replace it.
fn legacy_federation_token_warning(server: &str, token: &str) -> Option<String> {
    if zerocode_hookd::is_scoped_federation_token(token) {
        return None;
    }
    Some(format!(
        "{server} uses a legacy bridge-wide token; on that worker run \
         federation-invite again, then run federation-forget {server} and \
         federation-join {server} with the new scoped token"
    ))
}

fn with_legacy_federation_token_warning(
    mut answer: serde_json::Value,
    server: &str,
    token: &str,
) -> serde_json::Value {
    if let Some(warning) = legacy_federation_token_warning(server, token) {
        answer["warning"] = serde_json::Value::String(warning);
    }
    answer
}

/// The one address predicate for the federation wire. `IpAddr::is_loopback`
/// answers ordinary IPv4/IPv6 addresses; an IPv4-MAPPED IPv6 address
/// (`::ffff:127.0.0.1`) is an IPv4 address wearing a v6 coat, so unwrap it and
/// ask the IPv4 type the same question.
///
/// `to_ipv4` is the wrong unwrapper for that: it also answers `Some` for the
/// deprecated IPv4-COMPATIBLE form (`::127.0.0.1`, RFC 4291 §2.5.5.1), which is
/// an ordinary v6 address inside `::/96` that no kernel routes to 127.0.0.1.
/// Calling that loopback would be this predicate lying about the one thing it
/// exists to say, so ask for the mapped form by name.
fn is_loopback_ip(ip: std::net::IpAddr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    matches!(
        ip,
        std::net::IpAddr::V6(ip)
            if ip.to_ipv4_mapped().is_some_and(|mapped| mapped.is_loopback())
    )
}

fn checked_loopback_socket(
    endpoint: std::net::SocketAddr,
    address: &str,
) -> Result<std::net::SocketAddr, String> {
    if is_loopback_ip(endpoint.ip()) {
        return Ok(endpoint);
    }
    Err(format!(
        "{address} is not a loopback address — {FEDERATION_PLAINTEXT_REASON}; {FEDERATION_TUNNEL_HINT}"
    ))
}

/// Which of a name's resolutions the address book may keep — or why none of
/// them may be.
///
/// Every result must be loopback: accepting one loopback answer from a name
/// that also resolves publicly would leave the transport's destination
/// ambiguous, and that ambiguity would resolve at relay time, where nobody is
/// watching.
///
/// Among the survivors the choice is not free either. The book stores ONE
/// endpoint and the relay dials exactly that one — `TcpStream::connect` on a
/// NAME walks every answer until one is reached, but a stored `SocketAddr` has
/// no second answer to fall back to. The far end is a `zerocode_hookd` bridge,
/// which binds IPv4 loopback and nothing else, while `getaddrinfo` answers
/// `localhost` with `::1` first on most hosts. So prefer the IPv4 answer, and
/// keep the head of the list only when the person named a v6 endpoint
/// themselves.
fn chosen_loopback_endpoint(
    resolved: &[std::net::SocketAddr],
    address: &str,
) -> Result<std::net::SocketAddr, String> {
    for endpoint in resolved {
        checked_loopback_socket(*endpoint, address)?;
    }
    resolved
        .iter()
        .find(|endpoint| endpoint.is_ipv4())
        .or_else(|| resolved.first())
        .copied()
        .ok_or_else(|| {
            format!(
                "{address} did not resolve to a loopback endpoint — \
                 {FEDERATION_PLAINTEXT_REASON}"
            )
        })
}

/// Resolve and validate one address before it enters the address book. The
/// chosen socket is stored canonically, so normal relay calls do not do a fresh
/// DNS lookup or put this check inside the blocking HTTP write.
fn loopback_only(addr: &str) -> Result<std::net::SocketAddr, String> {
    use std::net::ToSocketAddrs as _;

    let resolved: Vec<std::net::SocketAddr> = addr
        .to_socket_addrs()
        .map_err(|why| {
            format!(
                "{addr} is not a usable loopback endpoint — {FEDERATION_PLAINTEXT_REASON}; \
                 address resolution failed: {why}"
            )
        })?
        .collect();
    chosen_loopback_endpoint(&resolved, addr)
}

/// A new address-book entry is canonical numeric loopback. Existing entries
/// from the pre-validation format are refused at use, with an actionable
/// migration message, instead of being sent to an untrusted destination.
fn canonical_server_address(server: &str, address: &str) -> Result<std::net::SocketAddr, String> {
    let endpoint = address.parse::<std::net::SocketAddr>().map_err(|why| {
        format!(
            "{server} uses a legacy federation address ({address}: {why}); \
             run federation-forget {server}, then federation-join with a numeric \
             loopback endpoint"
        )
    })?;
    checked_loopback_socket(endpoint, address).map_err(|why| {
        format!(
            "{server}: {why}; run federation-forget {server}, then federation-join with a \
             numeric loopback endpoint"
        )
    })
}

/// The four address-book verbs, or `None` when the argv is the planner's
/// business. Answers in the shim's own JSON voice.
fn federation_book_verbs(argv: &[String]) -> Option<zerocode_hookd::TeamAnswer> {
    let verb = argv.first().map(String::as_str)?;
    let value_of = |flag: &str| {
        argv.iter()
            .position(|word| word == flag)
            .and_then(|at| argv.get(at + 1))
            .map(String::as_str)
    };
    let ok = |said: serde_json::Value| {
        Some(answer(zerocode_core::agent_teams::Reply::ok(format!(
            "{said}\n"
        ))))
    };
    match verb {
        "federation-invite" => {
            let Some(bridge) = crate::hooks::bridge() else {
                return Some(refused("this window has no bridge to invite through"));
            };
            let token = zerocode_hookd::federation_token(&bridge.token);
            ok(serde_json::json!({
                "addr": format!("127.0.0.1:{}", bridge.port),
                "token": token.as_str(),
                "hint": "on the home machine: zerocode-orc federation-join <name> \
                         --addr 127.0.0.1:<forwarded-port> --token <this token>",
            }))
        }
        "federation-servers" => {
            let book = server_book();
            let listed: Vec<serde_json::Value> = book
                .iter()
                .map(|(name, entry)| {
                    let mut listed = serde_json::json!({
                        "name": name,
                        "addr": entry.addr,
                        "token": format!(
                            "{}…",
                            entry.token.chars().take(8).collect::<String>()
                        ),
                    });
                    if let Err(why) = canonical_server_address(name, &entry.addr) {
                        listed["warning"] = serde_json::Value::String(why);
                    } else if let Some(warning) =
                        legacy_federation_token_warning(name, &entry.token)
                    {
                        listed["warning"] = serde_json::Value::String(warning);
                    }
                    listed
                })
                .collect();
            ok(serde_json::json!({ "servers": listed }))
        }
        "federation-join" => {
            let Some(name) = argv.get(1).filter(|held| !held.starts_with("--")) else {
                return Some(refused("federation-join needs a name first"));
            };
            let Some(addr) = value_of("--addr") else {
                return Some(refused("federation-join needs --addr"));
            };
            let endpoint = match loopback_only(addr) {
                Ok(endpoint) => endpoint,
                Err(why) => return Some(refused(why)),
            };
            let canonical_addr = endpoint.to_string();
            let Some(token) = value_of("--token").filter(|held| !held.is_empty()) else {
                return Some(refused("federation-join needs --token"));
            };
            if let Err(why) = usable_transport_token(token) {
                return Some(refused(why));
            }
            let mut book: std::collections::BTreeMap<String, serde_json::Value> = federation_root()
                .and_then(|root| std::fs::read_to_string(root.join("servers.json")).ok())
                .and_then(|held| serde_json::from_str(&held).ok())
                .unwrap_or_default();
            if let Some(standing) = book.get(name.as_str()) {
                let Some(standing_addr) = standing["addr"].as_str() else {
                    return Some(refused(format!(
                        "{name} has an invalid saved federation address — run \
                         federation-forget {name} before joining it again"
                    )));
                };
                let standing_endpoint = match loopback_only(standing_addr) {
                    Ok(endpoint) => endpoint,
                    Err(why) => {
                        return Some(refused(format!(
                            "{name} has an unsafe saved federation address — {why}; \
                             run federation-forget {name}, then join the SSH tunnel's \
                             numeric loopback endpoint"
                        )));
                    }
                };
                let same = standing_endpoint == endpoint && standing["token"] == token;
                return match same {
                    // The retry road: the entry already says this.
                    true if standing_addr == canonical_addr => {
                        ok(with_legacy_federation_token_warning(
                            serde_json::json!({ "joined": name, "already": true }),
                            name,
                            token,
                        ))
                    }
                    true => {
                        // Upgrade a pre-validation hostname entry after proving
                        // its current resolution is loopback-only.
                        book.insert(
                            name.to_string(),
                            serde_json::json!({ "addr": canonical_addr, "token": token }),
                        );
                        if let Err(why) = write_server_book(&book) {
                            Some(refused(why))
                        } else {
                            ok(with_legacy_federation_token_warning(
                                serde_json::json!({
                                    "joined": name,
                                    "addr": canonical_addr,
                                    "upgraded": true,
                                }),
                                name,
                                token,
                            ))
                        }
                    }
                    false => Some(refused(format!(
                        "{name} is already joined with different coordinates — \
                         federation-forget {name} first, so a typo cannot silently \
                         repoint a standing relay"
                    ))),
                };
            }
            book.insert(
                name.to_string(),
                serde_json::json!({ "addr": canonical_addr, "token": token }),
            );
            if let Err(why) = write_server_book(&book) {
                return Some(refused(why));
            }
            ok(with_legacy_federation_token_warning(
                serde_json::json!({ "joined": name, "addr": canonical_addr }),
                name,
                token,
            ))
        }
        "federation-forget" => {
            let Some(name) = argv.get(1).filter(|held| !held.starts_with("--")) else {
                return Some(refused("federation-forget needs a name"));
            };
            let mut book: std::collections::BTreeMap<String, serde_json::Value> = federation_root()
                .and_then(|root| std::fs::read_to_string(root.join("servers.json")).ok())
                .and_then(|held| serde_json::from_str(&held).ok())
                .unwrap_or_default();
            let stood = book.remove(name.as_str()).is_some();
            if stood && let Err(why) = write_server_book(&book) {
                return Some(refused(why));
            }
            ok(serde_json::json!({ "forgot": name, "stood": stood }))
        }
        _ => None,
    }
}

/// The leader seat of one team, as a federated verb presents it to `run`:
/// the team, its leader pane, and the capability that pane holds — or the
/// sentence for a table nobody can sign for.
fn leader_seat_of(team: &str) -> Result<(String, String, String), String> {
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let Some(capability) = crate::agent_teams::current_pane_capability(team, leader) else {
        return Err(format!(
            "this window's leader pane of {team} holds no capability"
        ));
    };
    Ok((team.to_string(), leader.to_string(), capability))
}

/// The seat a federated worker is released from: the leader of the team
/// that OWNS it, read off the worker's own ledger row.
///
/// The row, and not the team table. A pane id is unique only inside its
/// team, and the table holds every team standing in this window — other
/// runs', and since t-2512 every LEADERLESS table a leader's exit left over
/// its children — so any pick from it is a guess. The pick used to be
/// whichever entry the map yielded first, and a leaderless one has no
/// capability to present: the runtime refused the empty request as invalid
/// and the home was told `stop_unknown` for a pane that was still there
/// (2026-09-05: red on four of six concurrent `cargo test -p zerocode-shell`
/// rounds). The attach wrote the team on the row when it summoned through
/// that team's `worker-start`; the stop reads the same fact back from the
/// same authority, and nothing here walks the table.
fn federated_worker_seat(
    actor: &RuntimeActor,
    worker: &str,
) -> Result<(String, String, String), String> {
    let image = actor
        .view()
        .map_err(|why| format!("the ledger could not be read ({why})"))?;
    let team = image
        .projection()
        .workers
        .iter()
        .find(|row| row.id == worker)
        .map(|row| row.team.clone())
        .ok_or_else(|| format!("no ledger row names worker {worker}"))?;
    leader_seat_of(&team)
}

/// The worker-server half: one federation call from a home window, walked
/// through ledger law — and, for the two verbs that touch a screen, through
/// the same verbs a local coordinator would type, so the summons and the
/// stop keep every local rule (journal, rollback, archive) for free.
pub(crate) fn federation_serve(host: &dyn Host, home: &str, body: &serde_json::Value) -> String {
    let refuse = |why: String| serde_json::json!({ "refused": why }).to_string();
    if unavailable().is_some() {
        return refuse("this window's orchestration is unavailable".to_string());
    }
    let Some(held) = runtime() else {
        return refuse("this window has no ledger authority".to_string());
    };
    let actor = &held.actor;
    let now_ms = crate::now_epoch_ms();
    if let Some(spoken) = body["protocol"].as_u64()
        && spoken != FEDERATION_PROTOCOL
    {
        return refuse(format!(
            "this window speaks federation v{FEDERATION_PROTOCOL} — asked for v{spoken}"
        ));
    }
    let verb = body["verb"].as_str().unwrap_or_default();
    let dispatch = body["dispatch"].as_str().unwrap_or_default().to_string();
    let call = |call: FederationCall| -> Result<FederationAnswer, String> {
        match actor.federation(call) {
            Ok((FederationAnswer::Refused(why), _)) => Err(why),
            Ok((answer, _)) => Ok(answer),
            Err(why) => Err(format!("{why:?}")),
        }
    };
    match verb {
        "attach" => {
            let agent = body["agent"].as_str().unwrap_or_default();
            let prompt = body["prompt"].as_str().unwrap_or_default();
            if dispatch.is_empty() || agent.is_empty() || prompt.is_empty() {
                return refuse("attach needs dispatch, agent and prompt".to_string());
            }
            let fed_run = match call(FederationCall::EnsureRun {
                home: home.to_string(),
                now_ms,
            }) {
                Ok(FederationAnswer::Run(id)) => id,
                Ok(_) => return refuse("the ledger answered out of shape".to_string()),
                Err(why) => return refuse(why),
            };
            /* Whose screen may a federated summons cut? A NAMED team's —
             * `federation/host-team` beside the address book — or, with no
             * name written, the only team standing. Several leaders and no
             * name is a refusal: this window cannot guess whose layout to
             * grow, and guessing is how a borrowed pane lands on somebody's
             * presentation. */
            let named = federation_root()
                .and_then(|root| std::fs::read_to_string(root.join("host-team")).ok())
                .map(|held| held.trim().to_string())
                .filter(|held| !held.is_empty());
            let (team_id, leader_pane, capability) = {
                let tables = crate::agent_teams::teams();
                let id = match &named {
                    Some(wanted) => match tables.contains_key(wanted) {
                        true => wanted.clone(),
                        false => {
                            return refuse(format!(
                                "the named host team ({wanted}) is not standing in this \
                                 window — fix federation/host-team or start it"
                            ));
                        }
                    },
                    None => {
                        // A leaderless team — its leader exited and its
                        // orphans are still working — is not a team a
                        // borrowed pane can be cut from: nobody sits in
                        // its leader pane to hold the capability.
                        let mut standing = tables
                            .iter()
                            .filter(|(_, team)| team.pane(&team.leader_pane).is_some());
                        let Some((id, _)) = standing.next() else {
                            return refuse(
                                "this window has no standing team to seat a federated \
                                 worker — open an orchestration run here first"
                                    .to_string(),
                            );
                        };
                        if standing.next().is_some() {
                            return refuse(
                                "this window runs several teams — name one in \
                                 federation/host-team so a borrowed pane cannot land on \
                                 the wrong screen"
                                    .to_string(),
                            );
                        }
                        id.clone()
                    }
                };
                drop(tables);
                match leader_seat_of(&id) {
                    Ok(seat) => seat,
                    Err(why) => return refuse(why),
                }
            };
            let mut argv = vec![
                "worker-start".to_string(),
                "--run".to_string(),
                fed_run.clone(),
                "--agent".to_string(),
                agent.to_string(),
                "--prompt".to_string(),
                prompt.to_string(),
                "--retry-request".to_string(),
                format!("fed-{}-{dispatch}", &home[..home.len().min(8)]),
            ];
            if let Some(model) = body["model"].as_str() {
                argv.extend(["--model".to_string(), model.to_string()]);
            }
            if let Some(effort) = body["effort"].as_str() {
                argv.extend(["--effort".to_string(), effort.to_string()]);
            }
            if let Some(timeout) = body["timeoutMs"].as_u64() {
                argv.extend(["--timeout-ms".to_string(), timeout.to_string()]);
            }
            let summoned = run(
                host,
                Vec::new(),
                &team_id,
                &leader_pane,
                &capability,
                &argv,
                now_ms,
            );
            if summoned.exit_code != 0 {
                return refuse(format!(
                    "the summons was refused: {}",
                    summoned.stderr.trim()
                ));
            }
            let said: serde_json::Value = match serde_json::from_str(&summoned.stdout) {
                Ok(said) => said,
                Err(_) => return refuse("the summons answered non-JSON".to_string()),
            };
            let worker = said["workerId"].as_str().unwrap_or_default().to_string();
            match call(FederationCall::Attach {
                run: fed_run.clone(),
                dispatch: dispatch.clone(),
                home: home.to_string(),
                worker: worker.clone(),
                now_ms,
            }) {
                Ok(_) => {}
                /* The home replayed an attach that already stands — the
                 * summons above replayed too (same durable retry name), so
                 * the honest answer is the standing seat, not a refusal. */
                Err(why) if why.contains("already attached") => {}
                Err(why) => return refuse(why),
            }
            serde_json::json!({
                "dispatchId": dispatch,
                "state": "ready",
                "protocol": FEDERATION_PROTOCOL,
                "workerId": worker,
                "pane": said["pane"],
                "run": fed_run,
            })
            .to_string()
        }
        "pull" => {
            let after = body["afterSeq"].as_i64().unwrap_or(0);
            let limit = body["limit"].as_u64().unwrap_or(50) as usize;
            match call(FederationCall::Pull {
                dispatch,
                home: home.to_string(),
                after_seq: after,
                limit,
            }) {
                Ok(FederationAnswer::Items(items)) => {
                    serde_json::json!({ "items": items }).to_string()
                }
                Ok(_) => refuse("the ledger answered out of shape".to_string()),
                Err(why) => refuse(why),
            }
        }
        "ack" => {
            let through = body["throughSeq"].as_i64().unwrap_or(0);
            let settlements: Vec<zerocode_core::orchestration::Settlement> =
                serde_json::from_value(body["settlements"].clone()).unwrap_or_default();
            match call(FederationCall::Ack {
                dispatch,
                home: home.to_string(),
                through_seq: through,
                settlements,
                now_ms,
            }) {
                Ok(FederationAnswer::Cursor(seq)) => {
                    serde_json::json!({ "acknowledgedThrough": seq }).to_string()
                }
                Ok(_) => refuse("the ledger answered out of shape".to_string()),
                Err(why) => refuse(why),
            }
        }
        "import" => {
            let items: Vec<zerocode_core::orchestration::RelayItem> =
                match serde_json::from_value(body["items"].clone()) {
                    Ok(items) => items,
                    Err(_) => return refuse("import items are not relay items".to_string()),
                };
            match call(FederationCall::Import {
                dispatch,
                home: home.to_string(),
                items,
                now_ms,
            }) {
                Ok(FederationAnswer::Cursor(seq)) => {
                    serde_json::json!({ "acknowledgedThrough": seq }).to_string()
                }
                Ok(_) => refuse("the ledger answered out of shape".to_string()),
                Err(why) => refuse(why),
            }
        }
        "stop" => {
            let (state, worker) = match call(FederationCall::Stop {
                dispatch: dispatch.clone(),
                home: home.to_string(),
                now_ms,
            }) {
                Ok(FederationAnswer::Stopped { state, worker }) => (state, worker),
                Ok(_) => return refuse("the ledger answered out of shape".to_string()),
                Err(why) => return refuse(why),
            };
            let Some(worker) = worker else {
                return serde_json::json!({
                    "dispatchId": dispatch,
                    "state": state.as_str(),
                    "alreadySettled": true,
                })
                .to_string();
            };
            /* The pane ends the way a local release ends it: screen read and
             * archived first. A release this window cannot confirm leaves the
             * honest word — stop_unknown — rather than a pretended ending. */
            let (team_id, leader_pane, capability) = match federated_worker_seat(actor, &worker) {
                Ok(seat) => seat,
                Err(why) => {
                    let _ = call(FederationCall::StopUnknown {
                        dispatch: dispatch.clone(),
                        home: home.to_string(),
                        now_ms,
                    });
                    return serde_json::json!({
                        "dispatchId": dispatch,
                        "state": "stop_unknown",
                        "lastError": why,
                    })
                    .to_string();
                }
            };
            // The same home always answers the same federation run, so the
            // release names its run without anybody being bound to it.
            let fed_run = match call(FederationCall::EnsureRun {
                home: home.to_string(),
                now_ms,
            }) {
                Ok(FederationAnswer::Run(id)) => id,
                _ => String::new(),
            };
            let argv = vec![
                "worker-release".to_string(),
                "--run".to_string(),
                fed_run,
                "--worker".to_string(),
                worker.clone(),
                "--retry-request".to_string(),
                format!("fed-stop-{dispatch}"),
            ];
            let released = run(
                host,
                Vec::new(),
                &team_id,
                &leader_pane,
                &capability,
                &argv,
                now_ms,
            );
            if released.exit_code != 0 {
                let _ = call(FederationCall::StopUnknown {
                    dispatch: dispatch.clone(),
                    home: home.to_string(),
                    now_ms,
                });
                return serde_json::json!({
                    "dispatchId": dispatch,
                    "state": "stop_unknown",
                    "lastError": released.stderr.trim(),
                })
                .to_string();
            }
            serde_json::json!({
                "dispatchId": dispatch,
                "state": "stopped",
                "workerId": worker,
            })
            .to_string()
        }
        other => refuse(format!("unknown federation verb: {other}")),
    }
}

/// One relay pass for every open remote seat this home holds: pull what the
/// worker said, absorb it exactly once, acknowledge the cursor (with the
/// settlement when a `worker_done` rode in), then carry the home's own
/// outbox over. Errors are per-seat and quiet — the far window may simply
/// be off, and the next pass asks again.
pub(crate) fn federation_relay_pass() {
    // An empty address book is the common case and the cheap one: no
    // actor round trip a second, every second, for a window that never
    // federates.
    if server_book().is_empty() {
        return;
    }
    let Some(held) = runtime() else { return };
    let actor = &held.actor;
    let now_ms = crate::now_epoch_ms();
    let seats = match actor.federation(FederationCall::HomeSeats) {
        Ok((FederationAnswer::Seats(seats), _)) => seats,
        _ => return,
    };
    for (run_id, dispatch, server, absorbed) in seats {
        let pulled = call_server(
            &server,
            &serde_json::json!({
                "verb": "pull", "dispatch": dispatch, "afterSeq": absorbed, "limit": 50,
            }),
            std::time::Duration::from_secs(10),
        );
        let Ok(pulled) = pulled else { continue };
        let items: Vec<zerocode_core::orchestration::RelayItem> =
            serde_json::from_value(pulled["items"].clone()).unwrap_or_default();
        if !items.is_empty() {
            let settlements: Vec<serde_json::Value> = items
                .iter()
                .filter(|item| item.kind == zerocode_core::orchestration::MessageKind::WorkerDone)
                .filter_map(|item| {
                    serde_json::from_str::<serde_json::Value>(item.body.as_str())
                        .ok()
                        .and_then(|body| body["ok"].as_bool())
                        .map(|ok| serde_json::json!({ "seq": item.seq, "ok": ok }))
                })
                .collect();
            let through = items.last().map(|item| item.seq).unwrap_or(absorbed);
            let landed = actor.federation(FederationCall::Absorb {
                run: run_id.clone(),
                dispatch: dispatch.clone(),
                items,
                now_ms,
            });
            if !matches!(landed, Ok((FederationAnswer::Cursor(_), _))) {
                continue;
            }
            let _ = call_server(
                &server,
                &serde_json::json!({
                    "verb": "ack", "dispatch": dispatch,
                    "throughSeq": through, "settlements": settlements,
                }),
                std::time::Duration::from_secs(10),
            );
        }
        let outbox = match actor.federation(FederationCall::Outbox {
            run: run_id.clone(),
            dispatch: dispatch.clone(),
        }) {
            Ok((FederationAnswer::Items(items), _)) => items,
            _ => continue,
        };
        if outbox.is_empty() {
            continue;
        }
        let through = outbox.last().map(|item| item.seq).unwrap_or(0);
        let carried = call_server(
            &server,
            &serde_json::json!({ "verb": "import", "dispatch": dispatch, "items": outbox }),
            std::time::Duration::from_secs(10),
        );
        if let Ok(said) = carried
            && said["acknowledgedThrough"].as_i64() == Some(through)
        {
            let _ = actor.federation(FederationCall::Exported {
                run: run_id.clone(),
                dispatch: dispatch.clone(),
                through_seq: through,
                now_ms,
            });
        }
    }
}

/// The relay heartbeat: one thread, one pass a second — the original's own
/// cadence — living as long as the window does.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn spawn_federation_relay() {
    std::thread::Builder::new()
        .name("federation-relay".to_string())
        .spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                federation_relay_pass();
            }
        })
        .ok();
}

/// A refusal in this road's own name, said the way the shim expects.
fn refused(why: impl std::fmt::Display) -> zerocode_hookd::TeamAnswer {
    zerocode_hookd::TeamAnswer {
        stdout: String::new(),
        stderr: format!("orchestration: {why}\n"),
        exit_code: 1,
    }
}

#[cfg(test)]
pub(crate) mod tests;
